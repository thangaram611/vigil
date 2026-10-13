//! Serialized, identity-bound refresh requests. SIGUSR1 is only a wakeup;
//! success requires the captured token in the daemon's post-action tick.
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::tick::{self, TickSnapshot};

const DEADLINE: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonIdentity {
    pub pid: u32,
    pub start: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshRequest {
    pub token: String,
    pub target: DaemonIdentity,
}

pub fn valid_token(token: &str) -> bool {
    (8..=64).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_hexdigit())
}

impl RefreshRequest {
    fn parse(body: &str) -> Option<Self> {
        let lines = body.lines().collect::<Vec<_>>();
        if lines.len() != 4 || lines[0] != "version=1" {
            return None;
        }
        let token = lines[1].strip_prefix("token=")?;
        if !valid_token(token) {
            return None;
        }
        let pid = lines[2].strip_prefix("pid=")?.parse().ok()?;
        let start = lines[3].strip_prefix("start=")?.parse().ok()?;
        if pid == 0 || start <= 0 {
            return None;
        }
        Some(Self {
            token: token.into(),
            target: DaemonIdentity { pid, start },
        })
    }

    fn render(&self) -> String {
        format!(
            "version=1\ntoken={}\npid={}\nstart={}\n",
            self.token, self.target.pid, self.target.start
        )
    }
}

fn private_file(file: &File) -> io::Result<bool> {
    let metadata = file.metadata()?;
    // SAFETY: geteuid is a side-effect-free UID query.
    Ok(metadata.is_file()
        && metadata.uid() == unsafe { libc::geteuid() }
        && metadata.mode() & 0o077 == 0)
}

/// Capture once BEFORE evaluation. Requests arriving during evaluation remain
/// pending; acknowledge only this returned token AFTER the power action.
pub fn capture_request(
    state_dir: &Path,
    target: DaemonIdentity,
    acknowledged: Option<&str>,
) -> io::Result<Option<RefreshRequest>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(state_dir.join("refresh.request"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !private_file(&file)? || file.metadata()?.len() > 512 {
        return Ok(None);
    }
    let mut body = String::new();
    file.read_to_string(&mut body)?;
    Ok(RefreshRequest::parse(&body)
        .filter(|r| r.target == target && Some(r.token.as_str()) != acknowledged))
}

fn write_request(state_dir: &Path, request: &RefreshRequest) -> io::Result<()> {
    let tmp = state_dir.join(format!("refresh.request.{}", request.token));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        file.write_all(request.render().as_bytes())?;
        std::fs::rename(&tmp, state_dir.join("refresh.request"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

struct ClientLock(File);
impl Drop for ClientLock {
    fn drop(&mut self) {
        // SAFETY: fd belongs to this live File; unlock has no effect on daemon.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn try_lock(file: &File) -> io::Result<bool> {
    // SAFETY: file owns a valid fd; LOCK_NB never blocks.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}

/// Verify exact executable, sole daemon role, same user, non-zombie PID/start.
/// Scope sysinfo to this PID; do not scan providers or retain environments.
pub fn live_target_matches(target: DaemonIdentity, allowed_exes: &[PathBuf]) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};
    let pid = Pid::from_u32(target.pid);
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always)
            .with_user(UpdateKind::Always),
    );
    let Some(process) = sys.process(pid) else {
        return false;
    };
    let Some(exe) = process.exe().and_then(|p| p.canonicalize().ok()) else {
        return false;
    };
    // SAFETY: geteuid is a side-effect-free UID query.
    let uid = unsafe { libc::geteuid() };
    process.start_time() as i64 == target.start
        && process.status() != ProcessStatus::Zombie
        && process.user_id().is_some_and(|user| **user == uid)
        && process.cmd().len() == 2
        && process.cmd()[1] == "daemon"
        && allowed_exes
            .iter()
            .any(|allowed| allowed.canonicalize().ok().as_ref() == Some(&exe))
}

trait Runtime {
    fn elapsed(&self) -> Duration;
    fn sleep(&mut self, duration: Duration);
    fn matches(&mut self, target: DaemonIdentity) -> bool;
    fn signal(&mut self, target: DaemonIdentity) -> io::Result<()>;
}

struct LiveRuntime<'a> {
    began: Instant,
    allowed: &'a [PathBuf],
}
impl Runtime for LiveRuntime<'_> {
    fn elapsed(&self) -> Duration {
        self.began.elapsed()
    }
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn matches(&mut self, target: DaemonIdentity) -> bool {
        live_target_matches(target, self.allowed)
    }
    fn signal(&mut self, target: DaemonIdentity) -> io::Result<()> {
        // SAFETY: target role/start is revalidated immediately before delivery.
        if unsafe { libc::kill(target.pid as libc::pid_t, libc::SIGUSR1) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

fn token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn timeout() -> String {
    "refresh timed out after 2 seconds; outcome unknown".into()
}

fn target(tick: &TickSnapshot) -> Option<DaemonIdentity> {
    tick.supports_refresh().then(|| DaemonIdentity {
        pid: tick.pid,
        start: tick.daemon_start.unwrap(),
    })
}

/// Total deadline includes serialization, identity checks and acknowledgement.
/// Never starts an absent daemon, and never treats a normal tick as an ack.
pub fn request_refresh(
    state_dir: &Path,
    tick_file: &Path,
    daemon_pidfile: &Path,
    allowed_exes: &[PathBuf],
) -> Result<TickSnapshot, String> {
    request_with(
        state_dir,
        tick_file,
        daemon_pidfile,
        &mut LiveRuntime {
            began: Instant::now(),
            allowed: allowed_exes,
        },
    )
}

fn request_with(
    state_dir: &Path,
    tick_file: &Path,
    daemon_pidfile: &Path,
    runtime: &mut dyn Runtime,
) -> Result<TickSnapshot, String> {
    let metadata = std::fs::metadata(state_dir)
        .map_err(|_| "daemon unavailable; refresh does not start it".to_string())?;
    // SAFETY: geteuid is a side-effect-free UID query.
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("refresh requires a user-only state directory".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(state_dir.join("refresh.lock"))
        .map_err(|e| format!("refresh serialization: {e}"))?;
    if !private_file(&file).map_err(|e| e.to_string())? {
        return Err("refresh lock is not user-only".into());
    }
    while !try_lock(&file).map_err(|e| format!("refresh serialization: {e}"))? {
        if runtime.elapsed() >= DEADLINE {
            return Err(timeout());
        }
        runtime.sleep(RETRY.min(DEADLINE.saturating_sub(runtime.elapsed())));
    }
    let _lock = ClientLock(file);
    if runtime.elapsed() >= DEADLINE {
        return Err(timeout());
    }
    let initial = tick::read(tick_file).ok_or_else(|| {
        "daemon unavailable or tick malformed; refresh does not start it".to_string()
    })?;
    let expected = target(&initial)
        .ok_or_else(|| "daemon tick protocol does not support refresh; reload Vigil".to_string())?;
    let pid = std::fs::read_to_string(daemon_pidfile)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    if pid != Some(expected.pid) || !runtime.matches(expected) {
        return Err("daemon identity changed or unavailable; refresh not signalled".into());
    }
    if runtime.elapsed() >= DEADLINE {
        return Err(timeout());
    }
    let request = RefreshRequest {
        token: token().map_err(|e| format!("refresh token: {e}"))?,
        target: expected,
    };
    write_request(state_dir, &request).map_err(|e| format!("refresh request: {e}"))?;
    if !runtime.matches(expected) {
        return Err("daemon identity changed; refresh not signalled".into());
    }
    if runtime.elapsed() >= DEADLINE {
        return Err(timeout());
    }
    runtime
        .signal(expected)
        .map_err(|e| format!("refresh wakeup failed: {e}; outcome unknown"))?;
    loop {
        if runtime.elapsed() >= DEADLINE {
            return Err(timeout());
        }
        if let Some(evaluated) = tick::read(tick_file) {
            if target(&evaluated) != Some(expected) {
                return Err("daemon replaced during refresh; outcome unknown".into());
            }
            if evaluated.refresh_token.as_deref() == Some(request.token.as_str())
                && evaluated.evaluation_sequence > initial.evaluation_sequence
            {
                if !runtime.matches(expected) {
                    return Err("daemon replaced during refresh; outcome unknown".into());
                }
                if runtime.elapsed() >= DEADLINE {
                    return Err(timeout());
                }
                return Ok(evaluated);
            }
        }
        if !runtime.matches(expected) {
            return Err("daemon replaced or stopped during refresh; outcome unknown".into());
        }
        runtime.sleep(RETRY.min(DEADLINE.saturating_sub(runtime.elapsed())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        elapsed: Duration,
        live: bool,
        tick: PathBuf,
        state: PathBuf,
        signal_count: usize,
        ack: bool,
        replace: bool,
        matches_count: usize,
        fail_on_match: Option<usize>,
        match_cost: Duration,
        same_sequence: bool,
    }
    impl Runtime for Fake {
        fn elapsed(&self) -> Duration {
            self.elapsed
        }
        fn sleep(&mut self, d: Duration) {
            self.elapsed += d;
        }
        fn matches(&mut self, _target: DaemonIdentity) -> bool {
            self.matches_count += 1;
            self.elapsed += self.match_cost;
            self.live && self.fail_on_match != Some(self.matches_count)
        }
        fn signal(&mut self, expected: DaemonIdentity) -> io::Result<()> {
            self.signal_count += 1;
            if self.ack || self.replace {
                let request = capture_request(&self.state, expected, None)?.unwrap();
                let mut evaluated = tick::read(&self.tick).unwrap();
                if !self.same_sequence {
                    evaluated.evaluation_sequence += 1;
                }
                if self.replace {
                    evaluated.daemon_start = Some(expected.start + 1);
                }
                if self.ack {
                    evaluated.refresh_token = Some(request.token);
                }
                tick::write_tick(&self.tick, &evaluated)?;
            }
            Ok(())
        }
    }
    fn setup() -> (tempfile::TempDir, Fake, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let tick = dir.path().join("daemon.tick");
        let pid = dir.path().join("daemon.pid");
        let initial = TickSnapshot {
            pid: 42,
            daemon_start: Some(100),
            evaluation_sequence: 7,
            updated_at: 200,
            ..Default::default()
        };
        tick::write_tick(&tick, &initial).unwrap();
        std::fs::write(&pid, "42\n").unwrap();
        let fake = Fake {
            elapsed: Duration::ZERO,
            live: true,
            tick,
            state: dir.path().into(),
            signal_count: 0,
            ack: false,
            replace: false,
            matches_count: 0,
            fail_on_match: None,
            match_cost: Duration::ZERO,
            same_sequence: false,
        };
        (dir, fake, pid)
    }

    #[test]
    fn same_second_ack_requires_matching_token_and_new_sequence() {
        let (dir, mut fake, pid) = setup();
        fake.ack = true;
        let result = request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake).unwrap();
        assert_eq!(result.updated_at, 200);
        assert_eq!(result.evaluation_sequence, 8);
        assert!(result.refresh_token.is_some());
        assert_eq!(fake.signal_count, 1);
    }

    #[test]
    fn ordinary_ticks_and_old_tokens_cannot_ack_and_timeout_uses_total_deadline() {
        let (dir, mut fake, pid) = setup();
        let mut initial = tick::read(&fake.tick).unwrap();
        initial.refresh_token = Some("aaaaaaaa".into());
        tick::write_tick(&fake.tick, &initial).unwrap();
        let error = request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake).unwrap_err();
        assert!(error.contains("outcome unknown"));
        assert_eq!(fake.elapsed, DEADLINE);
    }

    #[test]
    fn replaced_reused_pid_and_absent_daemon_never_succeed() {
        let (dir, mut fake, pid) = setup();
        fake.replace = true;
        fake.ack = true;
        assert!(
            request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake)
                .unwrap_err()
                .contains("replaced")
        );
        fake.live = false;
        fake.signal_count = 0;
        assert!(request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake).is_err());
        assert_eq!(fake.signal_count, 0);
        std::fs::remove_file(&fake.tick).unwrap();
        assert!(request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake).is_err());
    }

    #[test]
    fn legacy_tick_is_explicitly_unsupported() {
        let (dir, mut fake, pid) = setup();
        let mut initial = tick::read(&fake.tick).unwrap();
        initial.version = 1;
        tick::write_tick(&fake.tick, &initial).unwrap();
        assert!(
            request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake)
                .unwrap_err()
                .contains("does not support")
        );
        assert_eq!(fake.signal_count, 0);
    }

    #[test]
    fn concurrent_caller_serialization_spends_same_two_second_budget() {
        let (dir, mut fake, pid) = setup();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.path().join("refresh.lock"))
            .unwrap();
        assert!(try_lock(&file).unwrap());
        let _held = ClientLock(file);
        assert!(
            request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake)
                .unwrap_err()
                .contains("timed out")
        );
        assert_eq!(fake.elapsed, DEADLINE);
        assert_eq!(fake.signal_count, 0);
        assert!(!dir.path().join("refresh.request").exists());
    }

    #[test]
    fn request_arriving_during_evaluation_stays_pending_and_capture_is_exact() {
        let dir = tempfile::tempdir().unwrap();
        let target = DaemonIdentity {
            pid: 42,
            start: 100,
        };
        let first = RefreshRequest {
            token: "aaaaaaaa".into(),
            target,
        };
        write_request(dir.path(), &first).unwrap();
        let captured = capture_request(dir.path(), target, None).unwrap().unwrap();
        let second = RefreshRequest {
            token: "bbbbbbbb".into(),
            target,
        };
        write_request(dir.path(), &second).unwrap();
        assert_eq!(captured.token, first.token);
        assert_eq!(
            capture_request(dir.path(), target, Some(&captured.token)).unwrap(),
            Some(second.clone())
        );
        assert_eq!(
            capture_request(dir.path(), target, Some(&second.token)).unwrap(),
            None
        );
        assert_eq!(
            capture_request(
                dir.path(),
                DaemonIdentity {
                    start: 101,
                    ..target
                },
                None
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn malformed_partial_and_nonprivate_requests_are_not_acknowledged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = DaemonIdentity {
            pid: 42,
            start: 100,
        };
        let request = RefreshRequest {
            token: "aaaaaaaa".into(),
            target,
        };
        write_request(dir.path(), &request).unwrap();
        let path = dir.path().join("refresh.request");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(capture_request(dir.path(), target, None).unwrap().is_none());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&path, "version=1\ntoken=aaaaaaaa\npid=42\n").unwrap();
        assert!(capture_request(dir.path(), target, None).unwrap().is_none());
    }

    #[test]
    fn rejects_real_process_with_wrong_role_without_signalling() {
        let start = crate::procscan::ProcScanner::process_start(std::process::id()).unwrap();
        assert!(!live_target_matches(
            DaemonIdentity {
                pid: std::process::id(),
                start
            },
            &[std::env::current_exe().unwrap()]
        ));
    }
    #[test]
    fn pid_reuse_between_request_and_signal_is_rejected_without_signal() {
        let (dir, mut fake, pid) = setup();
        fake.fail_on_match = Some(2);
        let error = request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake).unwrap_err();
        assert!(error.contains("identity changed"));
        assert_eq!(fake.signal_count, 0);
        let old_target = DaemonIdentity {
            pid: 42,
            start: 100,
        };
        assert!(
            capture_request(
                dir.path(),
                DaemonIdentity {
                    start: 101,
                    ..old_target
                },
                None
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn identity_checks_are_part_of_total_deadline() {
        let (dir, mut fake, pid) = setup();
        fake.match_cost = Duration::from_millis(1000);
        let error = request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake).unwrap_err();
        assert!(error.contains("timed out"));
        assert_eq!(fake.elapsed, DEADLINE);
        assert_eq!(fake.signal_count, 0);
    }

    #[test]
    fn matching_token_without_new_evaluation_does_not_ack() {
        let (dir, mut fake, pid) = setup();
        fake.ack = true;
        fake.same_sequence = true;
        assert!(
            request_with(dir.path(), &fake.tick.clone(), &pid, &mut fake)
                .unwrap_err()
                .contains("timed out")
        );
    }
}
