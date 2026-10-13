//! Live OS collection acceptance using synthetic provider hosts. The helpers
//! are copies of this test executable named claude/codex/copilot; they never
//! invoke providers. Process identity, argv, ancestry and marker reads are real.
//! Each helper has an alarm; command shells have a self-deadline. Every tree
//! lives in a private process group, killed and reaped by the harness on Drop.
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::json;
use vigil::activity::{ActivityCollector, ActivitySnapshot, State};
use vigil::config::{self, VigilConfig};
use vigil::procscan::{ProcRecord, ProcScanner};

const BOUND: u32 = 45;
static OS_FIXTURES: Mutex<()> = Mutex::new(());

#[test]
#[ignore = "subprocess fixture only"]
// Intentional reparenting: each descendant is self-bounded; outer Group kills
// the complete private group and reaps its direct host. launchd reaps orphans.
#[allow(clippy::zombie_processes)]
fn synthetic_host_helper() {
    let Some(root) = std::env::var_os("VIGIL_PHASE8_FIXTURE_ROOT") else {
        return;
    };
    // SAFETY: this bounded, disposable subprocess has default SIGALRM behavior.
    unsafe {
        libc::alarm(BOUND);
    }
    let root = PathBuf::from(root);
    std::fs::create_dir_all(&root).unwrap();
    let mode = std::env::var("VIGIL_PHASE8_FIXTURE_MODE").unwrap();
    let _files: Vec<File> = std::env::var("VIGIL_PHASE8_FIXTURE_OPEN_FILES")
        .ok()
        .map(|raw| serde_json::from_str::<Vec<PathBuf>>(&raw).unwrap())
        .unwrap_or_default()
        .iter()
        .map(|path| File::open(path).unwrap())
        .collect();
    if mode == "shell" || mode == "shell-spawner" {
        let command_exe = std::env::current_exe().unwrap().with_file_name("python3");
        if !command_exe.exists() {
            std::fs::copy(std::env::current_exe().unwrap(), &command_exe).unwrap();
        }
        let command_root = root.join("command");
        let child_mode = if mode == "shell-spawner" {
            "spawner"
        } else {
            "host"
        };
        let child = fixture_command(&command_exe, &command_root, child_mode)
            .spawn()
            .unwrap();
        std::fs::write(root.join("shell.pid"), child.id().to_string()).unwrap();
        wait_for(|| command_root.join("ready").is_file());
    } else if mode == "platform-shell" {
        // Both quiet shell and watchdog are bounded independently of the host.
        let child = Command::new("/bin/sh")
            .args(["-c", "printf '%s' \"$CLAUDE_CODE_SESSION_ID$COPILOT_AGENT_SESSION_ID\" > \"$VIGIL_PHASE8_FIXTURE_ROOT/shell.marker\"; (sleep 30; kill -KILL $$) & sleep 30 & wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(root.join("shell.pid"), child.id().to_string()).unwrap();
        wait_for(|| root.join("shell.marker").is_file());
        // Deliberately not waited: host-exit tests need a surviving descendant.
    } else if mode == "nested" {
        let nested = root.join("nested");
        let child = fixture_command(&std::env::current_exe().unwrap(), &nested, "shell")
            .spawn()
            .unwrap();
        std::fs::write(root.join("nested.pid"), child.id().to_string()).unwrap();
        wait_for(|| nested.join("ready").is_file());
    }
    std::fs::write(root.join("ready"), "ready").unwrap();
    let mut spawned = false;
    while !root.join("exit").exists() {
        if mode == "spawner" && !spawned && root.join("spawn").exists() {
            let descendant_root = root.join("descendant");
            let child =
                fixture_command(&std::env::current_exe().unwrap(), &descendant_root, "host")
                    .spawn()
                    .unwrap();
            std::fs::write(root.join("descendant.pid"), child.id().to_string()).unwrap();
            wait_for(|| descendant_root.join("ready").is_file());
            spawned = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fixture_command(exe: &Path, root: &Path, mode: &str) -> Command {
    let mut command = Command::new(exe);
    command
        .args([
            "--ignored",
            "--exact",
            "synthetic_host_helper",
            "--nocapture",
        ])
        .env("VIGIL_PHASE8_FIXTURE_ROOT", root)
        .env("VIGIL_PHASE8_FIXTURE_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if ready() {
            return;
        }
        assert!(Instant::now() < deadline, "fixture readiness deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Group {
    child: Child,
    pgid: u32,
}
impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: this is the private group created for this owned subprocess.
        unsafe {
            libc::kill(-(self.pgid as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

struct Sandbox {
    root: tempfile::TempDir,
    cfg: VigilConfig,
    groups: Vec<Group>,
    _serial: MutexGuard<'static, ()>,
}
impl Sandbox {
    fn new() -> Self {
        let serial = OS_FIXTURES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = tempfile::tempdir().unwrap();
        let mut cfg =
            config::load(root.path().join("absent.conf").to_str().unwrap(), None).unwrap();
        cfg.claude_home = root
            .path()
            .join("claude-state")
            .to_string_lossy()
            .into_owned();
        cfg.codex_home = root
            .path()
            .join("codex-state")
            .to_string_lossy()
            .into_owned();
        cfg.copilot_home = root
            .path()
            .join("copilot-state")
            .to_string_lossy()
            .into_owned();
        cfg.active_dir = root.path().join("active").to_string_lossy().into_owned();
        cfg.idle_after_sec = 300;
        for path in [
            &cfg.claude_home,
            &cfg.codex_home,
            &cfg.copilot_home,
            &cfg.active_dir,
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        Self {
            root,
            cfg,
            groups: Vec::new(),
            _serial: serial,
        }
    }
    fn spawn(
        &mut self,
        provider: &str,
        tag: &str,
        mode: &str,
        marker: Option<(&str, &str)>,
        opened: &[PathBuf],
    ) -> (u32, PathBuf) {
        let bin = self.root.path().join("bin").join(provider);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        if !bin.exists() {
            std::fs::copy(std::env::current_exe().unwrap(), &bin).unwrap();
        }
        let root = self.root.path().join(tag);
        let mut command = fixture_command(&bin, &root, mode);
        command
            .process_group(0)
            .env(
                "VIGIL_PHASE8_FIXTURE_OPEN_FILES",
                serde_json::to_string(opened).unwrap(),
            )
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("COPILOT_AGENT_SESSION_ID");
        if let Some((key, value)) = marker {
            command.env(key, value);
        }
        let child = command.spawn().unwrap();
        let pid = child.id();
        self.groups.push(Group { child, pgid: pid });
        wait_for(|| root.join("ready").is_file());
        (pid, root)
    }
    fn records(&self, scanner: &mut ProcScanner) -> Vec<ProcRecord> {
        scanner
            .collect()
            .into_iter()
            .filter(|record| {
                self.groups.iter().any(|group| {
                    // SAFETY: getpgid reads identity; only our private groups are retained.
                    record.pid == group.pgid
                        || unsafe { libc::getpgid(record.pid as libc::pid_t) }
                            == group.pgid as libc::pid_t
                })
            })
            .collect()
    }
    fn snapshot(
        &self,
        collector: &mut ActivityCollector,
        scanner: &mut ProcScanner,
    ) -> ActivitySnapshot {
        let records = self.records(scanner);
        collector.collect(&self.cfg, scanner, &records, chrono::Utc::now().timestamp())
    }
    fn claude(&self, pid: u32, id: &str, status: &str, start_offset: i64) {
        let start = ProcScanner::process_start(pid).unwrap() + start_offset;
        let proc_start = chrono::DateTime::from_timestamp(start, 0)
            .unwrap()
            .format("%a %b %e %H:%M:%S %Y")
            .to_string();
        let sessions = Path::new(&self.cfg.claude_home).join("sessions");
        let project = Path::new(&self.cfg.claude_home).join("projects/-phase8");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(sessions.join(format!("{pid}.json")), json!({"pid":pid,"sessionId":id,"procStart":proc_start,"kind":"interactive","status":status,"cwd":"/phase8","waitingFor":"user"}).to_string()).unwrap();
        std::fs::write(project.join(format!("{id}.jsonl")), "{}\n").unwrap();
    }
    fn finish_host(&mut self, pid: u32, root: &Path) {
        std::fs::write(root.join("exit"), "exit").unwrap();
        let group = self.groups.iter_mut().find(|g| g.pgid == pid).unwrap();
        wait_for(|| group.child.try_wait().unwrap().is_some());
        assert!(group.child.wait().unwrap().success());
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        self.groups.clear();
    }
}

fn pid_file(path: &Path) -> u32 {
    std::fs::read_to_string(path).unwrap().parse().unwrap()
}

#[test]
fn native_pid_start_identity_and_two_sessions_are_measured_live() {
    let mut sandbox = Sandbox::new();
    let (busy, _) = sandbox.spawn("claude", "busy", "host", None, &[]);
    let (waiting, _) = sandbox.spawn("claude", "waiting", "host", None, &[]);
    sandbox.claude(busy, "busy-session", "busy", 0);
    sandbox.claude(waiting, "waiting-session", "waiting", 0);
    let mut scanner = ProcScanner::new();
    let mut collector = ActivityCollector::default();
    let snapshot = sandbox.snapshot(&mut collector, &mut scanner);
    assert_eq!(
        (
            snapshot.detected_hosts,
            snapshot.working(),
            snapshot.waiting(),
            snapshot.uncertain()
        ),
        (2, 1, 1, 0)
    );
    sandbox.claude(busy, "busy-session", "busy", 1);
    let mismatched = sandbox.snapshot(&mut collector, &mut scanner);
    assert!(!mismatched.sessions.iter().any(|s| s.id == "busy-session"));
    assert_eq!(
        mismatched.working(),
        0,
        "mismatched start must not borrow native busy status"
    );
    assert!(mismatched.uncertain() > 0);
}

#[test]
fn witnessed_quiet_shell_survives_parent_wait_and_real_host_exit() {
    let mut sandbox = Sandbox::new();
    let (host, root) = sandbox.spawn(
        "claude",
        "owned",
        "shell",
        Some(("CLAUDE_CODE_SESSION_ID", "owned-session")),
        &[],
    );
    let shell = pid_file(&root.join("shell.pid"));
    sandbox.claude(host, "owned-session", "waiting", 0);
    let mut scanner = ProcScanner::new();
    let mut collector = ActivityCollector::default();
    let records = sandbox.records(&mut scanner);
    assert_eq!(
        records.iter().find(|r| r.pid == shell).unwrap().parent,
        Some(host)
    );
    let snapshot = collector.collect(
        &sandbox.cfg,
        &mut scanner,
        &records,
        chrono::Utc::now().timestamp(),
    );
    assert_eq!(
        snapshot.working(),
        1,
        "records={records:?}; selected markers={:?}; snapshot={snapshot:?}",
        scanner.session_markers(&[shell])
    );
    assert_eq!(
        snapshot.sessions[0].foreground_wait.as_deref(),
        Some("user")
    );
    assert_eq!(snapshot.sessions[0].evidence, "verified-local-process");
    sandbox.finish_host(host, &root);
    let records = sandbox.records(&mut scanner);
    assert_ne!(
        records.iter().find(|r| r.pid == shell).unwrap().parent,
        Some(host)
    );
    let snapshot = collector.collect(
        &sandbox.cfg,
        &mut scanner,
        &records,
        chrono::Utc::now().timestamp(),
    );
    assert_eq!(snapshot.detected_hosts, 0);
    assert_eq!(snapshot.working(), 1);
    assert_eq!(snapshot.sessions[0].id, "owned-session");
}

#[test]
fn cold_orphan_marker_is_unknown_instead_of_confirmed_work_or_idle() {
    let mut sandbox = Sandbox::new();
    let (host, root) = sandbox.spawn(
        "claude",
        "cold",
        "shell",
        Some(("CLAUDE_CODE_SESSION_ID", "cold-session")),
        &[],
    );
    let shell = pid_file(&root.join("shell.pid"));
    sandbox.claude(host, "cold-session", "waiting", 0);
    sandbox.finish_host(host, &root);
    let mut scanner = ProcScanner::new();
    let records = sandbox.records(&mut scanner);
    assert!(records.iter().any(|r| r.pid == shell));
    let snapshot = ActivityCollector::default().collect(
        &sandbox.cfg,
        &mut scanner,
        &records,
        chrono::Utc::now().timestamp(),
    );
    assert_eq!(snapshot.working(), 0);
    assert_eq!(
        snapshot.provider_state(vigil::activity::scan::Agent::Claude),
        "unknown"
    );
}

#[test]
fn inherited_outer_marker_cannot_skip_the_nearest_validated_host() {
    let mut sandbox = Sandbox::new();
    let (outer, root) = sandbox.spawn(
        "claude",
        "outer",
        "nested",
        Some(("CLAUDE_CODE_SESSION_ID", "outer-session")),
        &[],
    );
    let inner = pid_file(&root.join("nested.pid"));
    let shell = pid_file(&root.join("nested/shell.pid"));
    sandbox.claude(outer, "outer-session", "waiting", 0);
    sandbox.claude(inner, "inner-session", "idle", 0);
    let mut scanner = ProcScanner::new();
    let markers = scanner.session_markers(&[shell]);
    assert_eq!(
        markers
            .get(&shell)
            .and_then(|m| m.get(&vigil::activity::scan::Agent::Claude))
            .map(String::as_str),
        Some("outer-session"),
        "records={:?}; selected markers={markers:?}",
        sandbox.records(&mut scanner)
    );
    let snapshot = sandbox.snapshot(&mut ActivityCollector::default(), &mut scanner);
    assert_eq!(
        snapshot.working(),
        0,
        "inherited marker must not bypass inner host"
    );
    assert_eq!(snapshot.detected_hosts, 2);
    assert_eq!(
        snapshot.provider_state(vigil::activity::scan::Agent::Claude),
        "unknown",
        "conflicting inherited marker must remain explicit"
    );
}

#[test]
fn unavailable_platform_shell_marker_cannot_justify_exact_waiting() {
    let mut sandbox = Sandbox::new();
    let (host, root) = sandbox.spawn(
        "claude",
        "platform",
        "platform-shell",
        Some(("CLAUDE_CODE_SESSION_ID", "platform-session")),
        &[],
    );
    assert_eq!(
        std::fs::read_to_string(root.join("shell.marker")).unwrap(),
        "platform-session"
    );
    let shell = pid_file(&root.join("shell.pid"));
    sandbox.claude(host, "platform-session", "waiting", 0);
    let mut scanner = ProcScanner::new();
    let marker = scanner
        .session_markers(&[shell])
        .get(&shell)
        .and_then(|m| m.get(&vigil::activity::scan::Agent::Claude))
        .cloned();
    let snapshot = sandbox.snapshot(&mut ActivityCollector::default(), &mut scanner);
    if marker.is_some() {
        assert_eq!(snapshot.working(), 1);
    } else {
        assert_eq!(
            snapshot
                .sessions
                .iter()
                .find(|s| s.id == "platform-session")
                .unwrap()
                .state,
            State::Unknown
        );
        assert!(
            snapshot.uncertain() > 0,
            "unavailable environment cannot prove no independent demand"
        );
    }
}

#[test]
fn collected_command_start_mismatch_is_rechecked_and_exposed_as_unknown() {
    let mut sandbox = Sandbox::new();
    let (host, root) = sandbox.spawn(
        "claude",
        "race",
        "shell",
        Some(("CLAUDE_CODE_SESSION_ID", "race-session")),
        &[],
    );
    let shell = pid_file(&root.join("shell.pid"));
    sandbox.claude(host, "race-session", "waiting", 0);
    let mut scanner = ProcScanner::new();
    let mut records = sandbox.records(&mut scanner);
    // Synthetic PID-reuse race over a live OS process; cannot force OS reuse.
    records
        .iter_mut()
        .find(|r| r.pid == shell)
        .unwrap()
        .start_time += 1;
    let snapshot = ActivityCollector::default().collect(
        &sandbox.cfg,
        &mut scanner,
        &records,
        chrono::Utc::now().timestamp(),
    );
    assert_eq!(snapshot.working(), 0);
    assert_eq!(
        snapshot.provider_state(vigil::activity::scan::Agent::Claude),
        "unknown"
    );
}

#[test]
fn one_real_codex_host_can_load_two_independent_synthetic_rollouts() {
    let mut sandbox = Sandbox::new();
    let first = Path::new(&sandbox.cfg.codex_home).join("sessions/rollout-first.jsonl");
    let second = Path::new(&sandbox.cfg.codex_home).join("sessions/rollout-second.jsonl");
    std::fs::create_dir_all(first.parent().unwrap()).unwrap();
    for (path, id) in [(&first, "first-session"), (&second, "second-session")] {
        std::fs::write(
            path,
            format!("{}\n", json!({"type":"session_meta","payload":{"id":id}})),
        )
        .unwrap();
    }
    let (host, _) = sandbox.spawn(
        "codex",
        "shared",
        "host",
        None,
        &[first.clone(), second.clone()],
    );
    let start = ProcScanner::process_start(host).unwrap();
    for (path, id) in [(&first, "first-turn"), (&second, "second-turn")] {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "{}", json!({"type":"event_msg","payload":{"type":"task_started","turn_id":id,"started_at":start}})).unwrap();
    }
    let mut scanner = ProcScanner::new();
    let mut collector = ActivityCollector::default();
    let snapshot = sandbox.snapshot(&mut collector, &mut scanner);
    assert_eq!(snapshot.detected_hosts, 1);
    assert_eq!(snapshot.working(), 2);
    assert_eq!(
        sandbox
            .snapshot(&mut collector, &mut scanner)
            .transcript_bytes,
        0
    );
}

#[test]
fn newly_loaded_historical_codex_turn_does_not_regain_foreground_ownership() {
    let mut sandbox = Sandbox::new();
    let path = Path::new(&sandbox.cfg.codex_home).join("sessions/rollout-historical.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    let (host, _) = sandbox.spawn(
        "codex",
        "historical",
        "host",
        None,
        std::slice::from_ref(&path),
    );
    let start = ProcScanner::process_start(host).unwrap();
    let records = [
        json!({"type":"session_meta","payload":{"id":"historical-session"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"old-turn","started_at":start-1}}),
    ];
    std::fs::write(
        &path,
        records.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    let snapshot = sandbox.snapshot(&mut ActivityCollector::default(), &mut ProcScanner::new());
    assert_eq!(snapshot.working(), 0);
    assert_eq!(snapshot.unknown(), 1);
    assert!(
        !snapshot.sessions[0].uncertain_demand,
        "new host cannot restore indefinite unfinished-turn demand"
    );
    assert!(snapshot.sessions[0].reason.contains("host changed"));
}

#[test]
fn a_temporary_collection_gap_does_not_erase_a_live_witness() {
    let mut sandbox = Sandbox::new();
    let (host, root) = sandbox.spawn(
        "claude",
        "gap",
        "shell",
        Some(("CLAUDE_CODE_SESSION_ID", "gap-session")),
        &[],
    );
    let shell = pid_file(&root.join("shell.pid"));
    sandbox.claude(host, "gap-session", "waiting", 0);
    let mut collector = ActivityCollector::default();
    let mut scanner = ProcScanner::new();
    assert_eq!(sandbox.snapshot(&mut collector, &mut scanner).working(), 1);
    let mut records = sandbox.records(&mut scanner);
    records.retain(|r| r.pid != shell);
    assert!(ProcScanner::process_start(shell).is_some());
    let snapshot = collector.collect(
        &sandbox.cfg,
        &mut scanner,
        &records,
        chrono::Utc::now().timestamp(),
    );
    assert!(
        snapshot.demand() > 0,
        "missing record without verified exit must not release independent work: {snapshot:?}"
    );
}

#[test]
fn unique_copilot_child_marker_joins_native_work_but_survives_its_completion() {
    let mut sandbox = Sandbox::new();
    let (host, root) = sandbox.spawn(
        "copilot",
        "copilot-child",
        "shell",
        Some(("COPILOT_AGENT_SESSION_ID", "internal-child-marker")),
        &[],
    );
    let session = Path::new(&sandbox.cfg.copilot_home).join("session-state/root-session");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join(format!("inuse.{host}.lock")), "fixture").unwrap();
    let events = session.join("events.jsonl");
    let body = [
        json!({"id":"start","timestamp":chrono::Utc::now().to_rfc3339(),"type":"session.start","data":{"sessionId":"root-session"}}),
        json!({"id":"child","timestamp":chrono::Utc::now().to_rfc3339(),"type":"subagent.started","data":{"toolCallId":"child-call"}}),
    ];
    std::fs::write(
        &events,
        body.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    let mut scanner = ProcScanner::new();
    let mut collector = ActivityCollector::default();
    assert_eq!(sandbox.snapshot(&mut collector, &mut scanner).working(), 1);
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&events)
            .unwrap(),
        "{}",
        json!({"id":"done","timestamp":chrono::Utc::now().to_rfc3339(),"type":"subagent.completed","data":{"toolCallId":"child-call"}})
    )
    .unwrap();
    let snapshot = sandbox.snapshot(&mut collector, &mut scanner);
    assert_eq!(
        snapshot.working(),
        1,
        "child completion must preserve previously witnessed command"
    );
    assert_eq!(snapshot.sessions[0].evidence, "verified-local-process");
    sandbox.finish_host(host, &root);
    assert_eq!(sandbox.snapshot(&mut collector, &mut scanner).working(), 1);
}

#[test]
fn new_descendant_after_host_exit_inherits_a_live_witness_and_outlives_it() {
    let mut sandbox = Sandbox::new();
    let (host, root) = sandbox.spawn(
        "claude",
        "descendants",
        "shell-spawner",
        Some(("CLAUDE_CODE_SESSION_ID", "descendant-session")),
        &[],
    );
    let command = pid_file(&root.join("shell.pid"));
    sandbox.claude(host, "descendant-session", "waiting", 0);
    let mut collector = ActivityCollector::default();
    let mut scanner = ProcScanner::new();
    assert_eq!(sandbox.snapshot(&mut collector, &mut scanner).working(), 1);
    sandbox.finish_host(host, &root);
    let command_root = root.join("command");
    std::fs::write(command_root.join("spawn"), "spawn").unwrap();
    wait_for(|| {
        command_root.join("descendant.pid").is_file()
            && command_root.join("descendant/ready").is_file()
    });
    let descendant = pid_file(&command_root.join("descendant.pid"));
    let records = sandbox.records(&mut scanner);
    assert_eq!(
        records.iter().find(|r| r.pid == descendant).unwrap().parent,
        Some(command)
    );
    let snapshot = collector.collect(
        &sandbox.cfg,
        &mut scanner,
        &records,
        chrono::Utc::now().timestamp(),
    );
    assert_eq!(snapshot.detected_hosts, 0);
    assert_eq!(snapshot.working(), 1);
    std::fs::write(command_root.join("exit"), "exit").unwrap();
    wait_for(|| ProcScanner::process_start(command).is_none());
    let records = sandbox.records(&mut scanner);
    assert!(records.iter().any(|r| r.pid == descendant));
    let snapshot = collector.collect(
        &sandbox.cfg,
        &mut scanner,
        &records,
        chrono::Utc::now().timestamp(),
    );
    assert_eq!(
        snapshot.working(),
        1,
        "new descendant must have established its own witness before parent's verified exit"
    );
    assert_eq!(snapshot.sessions[0].id, "descendant-session");
}

#[test]
fn wrapper_registration_uses_live_start_identity_and_gc_keeps_quiet_wrapper() {
    let sandbox = Sandbox::new();
    let root = sandbox.root.path();
    let mut command = Command::new(env!("CARGO_BIN_EXE_vigil"));
    command
        .args(["run", "/bin/sleep", "3"])
        .env("VIGIL_CONFIG_FILE", root.join("absent.conf"))
        .env("VIGIL_STATE_DIR", root.join("wrapper-state"))
        .env("VIGIL_LOG_DIR", root.join("logs"))
        .env("VIGIL_INSTALL_DIR", root.join("install"))
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn().unwrap();
    let pid = child.id();
    let mut group = Group { child, pgid: pid };
    let active = root.join("wrapper-state/active");
    let path = active.join(format!("wrapper-{pid}.pid"));
    wait_for(|| path.is_file());
    let body = std::fs::read_to_string(&path).unwrap();
    let registered: i64 = vigil::refcount::field(&body, "start_ts")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(registered, ProcScanner::process_start(pid).unwrap());
    let mut system = sysinfo::System::new();
    vigil::refcount::gc(&active, &mut system, 0, 0.0, chrono::Utc::now().timestamp());
    assert!(path.is_file(), "quiet live wrapper must survive GC");
    wait_for(|| group.child.try_wait().unwrap().is_some());
    assert!(group.child.wait().unwrap().success());
    assert!(!path.exists(), "normal wrapper exit cleans registration");
}

#[test]
fn recent_resolved_claude_session_cannot_refresh_an_older_unknown_sessions_fallback() {
    let mut sandbox = Sandbox::new();
    let (old_host, _) = sandbox.spawn("claude", "old-unknown", "host", None, &[]);
    let (waiting_host, _) = sandbox.spawn("claude", "recent-waiting", "host", None, &[]);
    sandbox.claude(old_host, "old-unknown-session", "unsupported", 0);
    sandbox.claude(waiting_host, "recent-waiting-session", "waiting", 0);
    let old_path =
        Path::new(&sandbox.cfg.claude_home).join("projects/-phase8/old-unknown-session.jsonl");
    std::fs::write(&old_path, "{malformed required history}\n").unwrap();
    let now = chrono::Utc::now().timestamp();
    let old_mtime = std::time::UNIX_EPOCH
        + Duration::from_secs((now - i64::from(sandbox.cfg.idle_after_sec) - 60) as u64);
    File::open(&old_path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old_mtime))
        .unwrap();
    let mut scanner = ProcScanner::new();
    let mut records = sandbox.records(&mut scanner);
    // Deliberately put A first. B must not lend its own recency to A's unknown.
    records.sort_by_key(|r| if r.pid == old_host { 0 } else { 1 });
    assert_eq!(records.first().unwrap().pid, old_host);
    let snapshot = ActivityCollector::default().collect(&sandbox.cfg, &mut scanner, &records, now);
    let old = snapshot
        .sessions
        .iter()
        .find(|s| s.id == "old-unknown-session")
        .unwrap();
    let waiting = snapshot
        .sessions
        .iter()
        .find(|s| s.id == "recent-waiting-session")
        .unwrap();
    assert_eq!(old.state, State::Unknown);
    assert!(
        !old.uncertain_demand,
        "unowned malformed history must not become indefinite demand"
    );
    assert_eq!(waiting.state, State::Waiting);
    assert!(!waiting.uncertain_demand);
    assert_eq!(
        snapshot.uncertain(),
        0,
        "recent resolved session cannot extend another session's bounded fallback: {snapshot:?}"
    );
    assert_eq!(snapshot.demand(), 0);
}

#[test]
fn unregistered_claude_host_cannot_borrow_resolved_siblings_recent_transcript() {
    let mut sandbox = Sandbox::new();
    let (unregistered, _) = sandbox.spawn("claude", "unregistered", "host", None, &[]);
    let (waiting, _) = sandbox.spawn("claude", "registered-waiting", "host", None, &[]);
    sandbox.claude(waiting, "registered-waiting-session", "waiting", 0);
    let now = chrono::Utc::now().timestamp();
    let mut scanner = ProcScanner::new();
    let records = sandbox.records(&mut scanner);
    let mut outcomes = Vec::new();
    for first in [unregistered, waiting] {
        let mut ordered = records.clone();
        ordered.sort_by_key(|record| if record.pid == first { 0 } else { 1 });
        let snapshot =
            ActivityCollector::default().collect(&sandbox.cfg, &mut scanner, &ordered, now);
        assert_eq!(snapshot.detected_hosts, 2);
        assert_eq!(snapshot.waiting(), 1);
        outcomes.push((first, snapshot));
    }
    // An unmatched recent transcript remains valid bounded provider fallback.
    let unmatched =
        Path::new(&sandbox.cfg.claude_home).join("projects/-phase8/unmatched-session.jsonl");
    std::fs::write(unmatched, "{}\n").unwrap();
    let positive = ActivityCollector::default().collect(&sandbox.cfg, &mut scanner, &records, now);
    assert_eq!(positive.working(), 0);
    assert_eq!(positive.uncertain(), 1);
    assert_eq!(positive.demand(), 1);
    for (first, snapshot) in outcomes {
        assert_eq!(
            snapshot.uncertain(),
            0,
            "host ordering first={first}: only resolved sibling history is recent: {snapshot:?}"
        );
        assert_eq!(snapshot.demand(), 0);
    }
}

#[test]
fn old_claude_session_with_unattributed_child_cannot_borrow_recent_waiting_sibling() {
    let mut sandbox = Sandbox::new();
    let (old, _) = sandbox.spawn(
        "claude",
        "old-child",
        "shell",
        Some(("CLAUDE_CODE_SESSION_ID", "unmatched-child-marker")),
        &[],
    );
    let (waiting, _) = sandbox.spawn("claude", "recent-clean", "host", None, &[]);
    sandbox.claude(old, "old-child-session", "waiting", 0);
    sandbox.claude(waiting, "recent-clean-session", "waiting", 0);
    let old_path =
        Path::new(&sandbox.cfg.claude_home).join("projects/-phase8/old-child-session.jsonl");
    let now = chrono::Utc::now().timestamp();
    let old_mtime = std::time::UNIX_EPOCH
        + Duration::from_secs((now - i64::from(sandbox.cfg.idle_after_sec) - 60) as u64);
    File::open(old_path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old_mtime))
        .unwrap();
    let mut scanner = ProcScanner::new();
    let snapshot = sandbox.snapshot(&mut ActivityCollector::default(), &mut scanner);
    let old_session = snapshot
        .sessions
        .iter()
        .find(|session| session.id == "old-child-session")
        .unwrap();
    let waiting_session = snapshot
        .sessions
        .iter()
        .find(|session| session.id == "recent-clean-session")
        .unwrap();
    assert_eq!(old_session.state, State::Unknown);
    assert!(!old_session.uncertain_demand);
    assert_eq!(waiting_session.state, State::Waiting);
    assert_eq!(snapshot.working(), 0);
    assert_eq!(
        snapshot.uncertain(),
        0,
        "unattributed child fallback must use its owner's old transcript: {snapshot:?}"
    );
    assert_eq!(snapshot.demand(), 0);
}
