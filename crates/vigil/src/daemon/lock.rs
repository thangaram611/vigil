//! Single-instance guard — an atomic-`mkdir` directory lock (NOT `flock`).
//!
//! macOS does not ship `flock(1)`, so the bash daemon (and this port) use the
//! atomic-`mkdir` pattern: `create_dir` succeeds atomically iff the directory
//! did not exist. The winner writes its PID into `{dir}/pid`. Stale-lock
//! recovery: if the recorded PID is dead (`kill(pid, 0)` fails), take the lock
//! from the corpse; a LIVE holder means a real second instance and we exit
//! cleanly (`exit(0)`), which `launchd KeepAlive` + `ThrottleInterval=10`
//! tolerates.
//!
//! Byte-faithful to `bin/vigil-daemon:36-54`.

use std::path::{Path, PathBuf};

/// Outcome of [`acquire`]. The caller maps each to the bash exit/continue path.
#[derive(Debug)]
pub enum LockOutcome {
    /// Lock dir was absent and we created it — we own it.
    Acquired(DaemonLock),
    /// A LIVE holder owns the dir — a real second instance. Exit(0).
    LiveContention { other: u32 },
    /// The recorded holder is dead/absent; we removed the stale dir and took
    /// over.
    TookOver(DaemonLock),
    /// The re-`mkdir` after removing a stale dir failed. Exit(1).
    Failed,
}

/// RAII-ish lock-dir guard. The daemon removes the dir on clean shutdown
/// (`cleanup_and_exit`) rather than via `Drop`, because the cleanup runs on the
/// main thread after a release; but [`DaemonLock`] also removes the dir on
/// `Drop` as a backstop for early-return error paths.
#[derive(Debug)]
pub struct DaemonLock {
    dir: PathBuf,
    /// Set false once the daemon has done its explicit cleanup so `Drop` does
    /// not double-remove (harmless, but avoids a spurious error log).
    armed: bool,
}

impl DaemonLock {
    /// The lock directory path.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Disarm the `Drop` backstop (the daemon's explicit cleanup already removed
    /// the dir).
    pub fn disarm(&mut self) {
        self.armed = false;
    }

    /// Remove the lock directory (best-effort).
    pub fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// `kill(pid, 0) == 0` — true iff the PID exists and we may signal it. Same
/// primitive `refcount` uses (signal 0 delivers nothing; only existence /
/// permission is checked).
fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 performs no signal delivery; always safe.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Acquire the daemon lock. `lock_file` is `cfg.lock_file`; the lock DIR is
/// `lock_file + ".d"` (bash `"$VIGIL_LOCK_FILE.d"`).
///
/// On success returns [`LockOutcome::Acquired`] / [`LockOutcome::TookOver`] with
/// the guard; the caller then [`finalize_acquire`]s to write `{dir}/pid`, the
/// daemon pidfile, and remove any stale tick file. (The pid is written there,
/// not here, so the `mkdir` step matches bash's argument-free `mkdir "$LOCK_DIR"`.)
pub fn acquire(lock_file: &Path) -> LockOutcome {
    let mut dir = lock_file.as_os_str().to_os_string();
    dir.push(".d");
    let dir = PathBuf::from(dir);

    match std::fs::create_dir(&dir) {
        Ok(()) => LockOutcome::Acquired(DaemonLock { dir, armed: true }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Read the recorded holder pid (bash `cat "$LOCK_DIR/pid"`).
            let other = std::fs::read_to_string(dir.join("pid"))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok());

            if let Some(other) = other
                && pid_alive(other)
            {
                // Live holder — a real second instance.
                return LockOutcome::LiveContention { other };
            }

            // Stale: recorded pid is dead/absent. Take over.
            tracing::warn!(
                "stale lock at {} (pid={} not running) — taking over",
                dir.display(),
                other
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "?".to_string()),
            );
            let _ = std::fs::remove_dir_all(&dir);
            match std::fs::create_dir(&dir) {
                Ok(()) => LockOutcome::TookOver(DaemonLock { dir, armed: true }),
                Err(_) => LockOutcome::Failed,
            }
        }
        Err(_) => LockOutcome::Failed,
    }
}

/// After acquiring: write `{dir}/pid` and the daemon pidfile, and remove a
/// previous run's stale tick file so consumers never read it. Mirrors bash
/// `echo $$ > "$LOCK_DIR/pid"; echo $$ > "$VIGIL_DAEMON_PIDFILE"; rm -f
/// "$VIGIL_DAEMON_TICK_FILE"`.
pub fn finalize_acquire(
    lock: &DaemonLock,
    daemon_pidfile: &Path,
    daemon_tick_file: &Path,
    my_pid: u32,
) -> std::io::Result<()> {
    std::fs::write(lock.dir.join("pid"), format!("{my_pid}\n"))?;
    std::fs::write(daemon_pidfile, format!("{my_pid}\n"))?;
    let _ = std::fs::remove_file(daemon_tick_file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_on_absent_dir_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let lock_file = dir.path().join("state.lock");
        match acquire(&lock_file) {
            LockOutcome::Acquired(g) => {
                assert!(g.dir().is_dir(), "lock dir created");
                assert_eq!(
                    g.dir().file_name().unwrap().to_str().unwrap(),
                    "state.lock.d"
                );
            }
            other => panic!("expected Acquired, got {other:?}"),
        }
    }

    #[test]
    fn live_holder_yields_contention() {
        let dir = tempfile::tempdir().unwrap();
        let lock_file = dir.path().join("state.lock");
        let lockdir = {
            let mut d = lock_file.as_os_str().to_os_string();
            d.push(".d");
            PathBuf::from(d)
        };
        std::fs::create_dir(&lockdir).unwrap();
        // Our own pid is, by definition, alive → live contention.
        std::fs::write(lockdir.join("pid"), format!("{}\n", std::process::id())).unwrap();
        match acquire(&lock_file) {
            LockOutcome::LiveContention { other } => {
                assert_eq!(other, std::process::id());
            }
            other => panic!("expected LiveContention, got {other:?}"),
        }
        // The dir must NOT have been removed (a live holder keeps it).
        assert!(lockdir.is_dir(), "live holder dir preserved");
    }

    #[test]
    fn dead_holder_pid_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let lock_file = dir.path().join("state.lock");
        let lockdir = {
            let mut d = lock_file.as_os_str().to_os_string();
            d.push(".d");
            PathBuf::from(d)
        };
        std::fs::create_dir(&lockdir).unwrap();
        // Find a pid that is (almost certainly) not alive. pid 0x7fffffff is far
        // above any real pid on macOS; kill(pid,0) returns ESRCH.
        let dead_pid: u32 = 0x7fff_fffe;
        assert!(
            !pid_alive(dead_pid),
            "test precondition: dead_pid not alive"
        );
        std::fs::write(lockdir.join("pid"), format!("{dead_pid}\n")).unwrap();
        // Plant a marker file the takeover must wipe via remove_dir_all.
        std::fs::write(lockdir.join("marker"), "x").unwrap();
        match acquire(&lock_file) {
            LockOutcome::TookOver(g) => {
                assert!(g.dir().is_dir(), "took-over dir re-created");
                assert!(!g.dir().join("marker").exists(), "stale contents wiped");
            }
            other => panic!("expected TookOver, got {other:?}"),
        }
    }

    #[test]
    fn empty_pid_file_is_treated_as_stale() {
        let dir = tempfile::tempdir().unwrap();
        let lock_file = dir.path().join("state.lock");
        let lockdir = {
            let mut d = lock_file.as_os_str().to_os_string();
            d.push(".d");
            PathBuf::from(d)
        };
        std::fs::create_dir(&lockdir).unwrap();
        // No pid file at all (bash: `cat ... || true` → empty `$other`).
        match acquire(&lock_file) {
            LockOutcome::TookOver(g) => assert!(g.dir().is_dir()),
            other => panic!("expected TookOver on missing pid file, got {other:?}"),
        }
    }

    #[test]
    fn non_numeric_pid_file_is_treated_as_stale() {
        // A garbage (non-numeric) pid file: `cat | trim | parse::<u32>()` yields
        // None (exactly like an absent/empty pid file), so the `if let Some(other)`
        // liveness check is skipped and acquire falls through to the stale-takeover
        // path → TookOver (NOT LiveContention, NOT Failed).
        let dir = tempfile::tempdir().unwrap();
        let lock_file = dir.path().join("state.lock");
        let lockdir = {
            let mut d = lock_file.as_os_str().to_os_string();
            d.push(".d");
            PathBuf::from(d)
        };
        std::fs::create_dir(&lockdir).unwrap();
        // Non-numeric content (trailing junk a real pid never has).
        std::fs::write(lockdir.join("pid"), "not-a-pid\n").unwrap();
        // Plant a marker the takeover's remove_dir_all must wipe.
        std::fs::write(lockdir.join("marker"), "x").unwrap();
        match acquire(&lock_file) {
            LockOutcome::TookOver(g) => {
                assert!(g.dir().is_dir(), "took-over dir re-created");
                assert!(
                    !g.dir().join("marker").exists(),
                    "stale contents wiped on takeover"
                );
            }
            other => panic!("expected TookOver on non-numeric pid file, got {other:?}"),
        }
    }

    #[test]
    fn finalize_writes_pidfiles_and_clears_tick() {
        let dir = tempfile::tempdir().unwrap();
        let lock_file = dir.path().join("state.lock");
        let pidfile = dir.path().join("daemon.pid");
        let tick = dir.path().join("daemon.tick");
        std::fs::write(&tick, "stale\n").unwrap();
        let lock = match acquire(&lock_file) {
            LockOutcome::Acquired(g) => g,
            other => panic!("expected Acquired, got {other:?}"),
        };
        finalize_acquire(&lock, &pidfile, &tick, 12345).unwrap();
        assert_eq!(
            std::fs::read_to_string(lock.dir().join("pid")).unwrap(),
            "12345\n"
        );
        assert_eq!(std::fs::read_to_string(&pidfile).unwrap(), "12345\n");
        assert!(!tick.exists(), "stale tick file removed on acquire");
    }

    #[test]
    fn drop_backstop_removes_dir_unless_disarmed() {
        let dir = tempfile::tempdir().unwrap();
        let lock_file = dir.path().join("state.lock");
        let lockdir = {
            let mut d = lock_file.as_os_str().to_os_string();
            d.push(".d");
            PathBuf::from(d)
        };
        {
            let _g = match acquire(&lock_file) {
                LockOutcome::Acquired(g) => g,
                other => panic!("expected Acquired, got {other:?}"),
            };
            assert!(lockdir.is_dir());
        }
        assert!(!lockdir.is_dir(), "Drop backstop removed the lock dir");

        // Disarmed guard must NOT remove on drop.
        let lock_file2 = dir.path().join("state2.lock");
        let lockdir2 = {
            let mut d = lock_file2.as_os_str().to_os_string();
            d.push(".d");
            PathBuf::from(d)
        };
        {
            let mut g = match acquire(&lock_file2) {
                LockOutcome::Acquired(g) => g,
                other => panic!("expected Acquired, got {other:?}"),
            };
            g.disarm();
        }
        assert!(lockdir2.is_dir(), "disarmed guard left the dir in place");
        let _ = std::fs::remove_dir_all(&lockdir2);
    }
}
