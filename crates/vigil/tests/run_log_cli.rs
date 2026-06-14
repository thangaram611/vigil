//! End-to-end CLI tests for the native `vigil run` (NON-exec wrapper) and
//! `vigil log` (paging) commands (Phase 5.7 Commit 5) driving the built binary
//! via CARGO_BIN_EXE_vigil in a hermetic sandbox.
//!
//! These close the §6.2 GAP #5 (the `wrapper_test.sh` non-exec half): they assert
//! that `vigil run` writes `wrapper-{pid}.pid` while the child lives, REMOVES it
//! after the child exits (proving the binary did NOT exec — exec would skip the
//! cleanup), and propagates the child's exit code. The log test asserts the
//! intentional paging deviation: a large log is NOT dumped whole.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Build a `Command` for the binary under test with a clean, deterministic env.
fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vigil"))
}

/// A throwaway sandbox state-dir tree (state/active + logs), TCC-safe (under the
/// temp dir, never `~/Documents`).
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "vigil-runlog-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("state/active")).unwrap();
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        Sandbox { root }
    }

    fn active_dir(&self) -> PathBuf {
        self.root.join("state/active")
    }

    fn log_file(&self) -> PathBuf {
        self.root.join("logs/daemon.log")
    }

    /// Apply the deterministic env: a sandboxed HOME + state/log dirs so the
    /// wrapper pidfile (`{state_dir}/active/wrapper-{pid}.pid`) and the log
    /// (`{log_dir}/daemon.log`) resolve inside the sandbox on any host. `active_dir`
    /// is unconditionally re-derived from `state_dir`, so pinning `VIGIL_STATE_DIR`
    /// is what fixes the wrapper pidfile location.
    fn apply_env(&self, cmd: &mut Command) {
        cmd.env("HOME", self.root.join("home"))
            .env("VIGIL_STATE_DIR", self.root.join("state"))
            .env("VIGIL_LOG_DIR", self.root.join("logs"))
            .env("VIGIL_INSTALL_DIR", self.root.join("install"))
            .env("VIGIL_ROOT_DIR", self.root.join("root"));
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Send SIGTERM to a process (the wrapper). `Child::kill()` sends SIGKILL, which
/// the wrapper CANNOT catch — so it would never run its cleanup handler. The bash
/// `wrapper_test.sh` used a default `kill` (SIGTERM), which the wrapper traps; we
/// replicate that so the INT/TERM/HUP cleanup path is what gets exercised.
fn sigterm(pid: u32) {
    // SAFETY: kill(2) with SIGTERM is async-signal-safe and side-effect-free here.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

/// Count the `wrapper-*.pid` files currently in `active_dir`.
fn wrapper_pidfiles(active_dir: &std::path::Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(active_dir) else {
        return Vec::new();
    };
    rd.filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wrapper-") && n.ends_with(".pid"))
        .collect()
}

/// `vigil run sleep <n>` writes a wrapper pidfile DURING the child's life and
/// REMOVES it after — proving (a) the pidfile lifecycle and (b) that the binary
/// did NOT exec (exec would replace the process and the cleanup would never run).
#[test]
fn run_creates_pidfile_during_child_and_removes_after() {
    let sb = Sandbox::new("lifecycle");
    let active = sb.active_dir();

    // Run a wrapper around `sleep 5` in the background, then poll for the pidfile.
    let mut cmd = bin();
    sb.apply_env(&mut cmd);
    cmd.args(["run", "sleep", "5"]);
    let mut child = cmd.spawn().expect("spawn vigil run");

    // Poll up to ~5s for the wrapper pidfile to appear (binary startup latency).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut during = Vec::new();
    while Instant::now() < deadline {
        during = wrapper_pidfiles(&active);
        if !during.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !during.is_empty(),
        "a wrapper-*.pid must exist DURING the child's lifetime, found none in {active:?}"
    );

    // SIGTERM the wrapper so we don't wait the full 5s; its TERM handler removes
    // the pidfile + _exit(128+15). (Child::kill would send uncatchable SIGKILL,
    // bypassing the handler — the opposite of what we want to test.)
    sigterm(child.id());
    let _ = child.wait();

    // After the wrapper exits, the active dir must be free of wrapper pidfiles
    // (cleanup ran → NOT exec).
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut after = wrapper_pidfiles(&active);
    while Instant::now() < deadline && !after.is_empty() {
        std::thread::sleep(Duration::from_millis(50));
        after = wrapper_pidfiles(&active);
    }
    assert!(
        after.is_empty(),
        "after the wrapper exits the pidfile must be removed (cleanup ran ⇒ no exec), found {after:?}"
    );
}

/// `vigil run sh -c 'exit 7'` propagates the child's exit code 7, AND removes the
/// wrapper pidfile (the cleanup ran before exit — proof of non-exec on the normal
/// exit path).
#[test]
fn run_propagates_child_exit_code_and_cleans_up() {
    // `vigil run <cmd>` propagates the child's exit code AND removes the wrapper
    // pidfile on the normal exit path (RAII guard ran) — for both a success (0)
    // and a non-zero exit (7). (The zero-arg usage-die path is asserted below.)
    let cases: &[(&str, &[&str], i32)] = &[
        ("zero", &["run", "true"], 0),
        ("exitcode", &["run", "sh", "-c", "exit 7"], 7),
    ];
    for &(label, args, want) in cases {
        let sb = Sandbox::new(label);
        let mut cmd = bin();
        sb.apply_env(&mut cmd);
        cmd.args(args);
        let status = cmd.status().expect("run vigil run <cmd>");
        assert_eq!(
            status.code(),
            Some(want),
            "{label}: vigil run must propagate the child exit code {want}"
        );
        assert!(
            wrapper_pidfiles(&sb.active_dir()).is_empty(),
            "{label}: wrapper pidfile must be removed on normal child exit"
        );
    }
}

/// Zero args → usage die, exit 1, no pidfile written.
#[test]
fn run_zero_args_usage_dies_exit_1() {
    let sb = Sandbox::new("usage");
    let mut cmd = bin();
    sb.apply_env(&mut cmd);
    cmd.arg("run");
    let out = cmd.output().expect("run vigil run with no args");
    assert_eq!(out.status.code(), Some(1), "zero-arg run exits 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("usage: vigil run <cmd> [args...]"),
        "must print the run usage line, got: {stderr}"
    );
    assert!(
        wrapper_pidfiles(&sb.active_dir()).is_empty(),
        "usage death must not leave a wrapper pidfile"
    );
}

/// `vigil log` on a missing log prints the soft message to STDOUT and exits 0.
#[test]
fn log_missing_prints_soft_message_exit_0() {
    let sb = Sandbox::new("nolog");
    let mut cmd = bin();
    sb.apply_env(&mut cmd);
    cmd.arg("log");
    let out = cmd.output().expect("run vigil log (no log)");
    assert_eq!(out.status.code(), Some(0), "missing log exits 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no log yet at") && stdout.contains("daemon.log"),
        "must print 'no log yet at {{log_file}}', got: {stdout}"
    );
}

/// `vigil log` (no-follow) on a LARGE log does NOT dump the whole file: it caps
/// STDOUT to the line limit (the one intentional deviation from bash `cat`).
#[test]
fn log_large_file_is_paged_not_dumped_whole() {
    let sb = Sandbox::new("biglog");
    // Write 20_000 numbered lines — far above the 2000-line cap.
    let total = 20_000usize;
    let mut body = String::with_capacity(total * 8);
    for i in 0..total {
        body.push_str(&format!("line-{i}\n"));
    }
    std::fs::write(sb.log_file(), &body).unwrap();

    let mut cmd = bin();
    sb.apply_env(&mut cmd);
    cmd.arg("log");
    let out = cmd.output().expect("run vigil log (big)");
    assert_eq!(out.status.code(), Some(0), "log of a big file exits 0");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let emitted = stdout.lines().count();
    assert!(
        emitted < total,
        "the whole {total}-line file must NOT be dumped; emitted {emitted}"
    );
    assert!(
        emitted <= 2000,
        "no-follow output must be capped at the 2000-line window, emitted {emitted}"
    );
    // The tail (most-recent lines) must be present; the head must be dropped.
    assert!(
        stdout.contains(&format!("line-{}", total - 1)),
        "the last line must be in the tail window"
    );
    assert!(
        !stdout.contains("line-0\n"),
        "the very first line must be dropped from the capped tail"
    );
    // The truncation hint goes to STDERR (keeps STDOUT clean log content).
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("showing the last"),
        "a truncation hint must be emitted to stderr, got: {stderr}"
    );
}

/// The no-follow paging boundary is EXACT at LINE_LIMIT (2000): a file with
/// exactly 2000 lines prints all 2000 and emits NO truncation hint (total ==
/// window.len(), so the `total > window.len()` guard is false); a file with 2001
/// lines drops the oldest line and DOES emit the hint ("showing the last 2000 of
/// 2001 lines"). This pins the off-by-one around `if window.len() > LINE_LIMIT`.
#[test]
fn log_paging_boundary_is_exact_at_line_limit() {
    const LINE_LIMIT: usize = 2000;

    // Build a body of `n` numbered lines ("line-0\n".."line-{n-1}\n").
    let body_of = |n: usize| -> String {
        let mut s = String::with_capacity(n * 8);
        for i in 0..n {
            s.push_str(&format!("line-{i}\n"));
        }
        s
    };

    // (a) Exactly LINE_LIMIT lines: all printed, no hint.
    {
        let sb = Sandbox::new("boundary-eq");
        std::fs::write(sb.log_file(), body_of(LINE_LIMIT)).unwrap();
        let mut cmd = bin();
        sb.apply_env(&mut cmd);
        cmd.arg("log");
        let out = cmd.output().expect("run vigil log (==limit)");
        assert_eq!(out.status.code(), Some(0), "==limit exits 0");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            stdout.lines().count(),
            LINE_LIMIT,
            "exactly {LINE_LIMIT} lines must all be printed"
        );
        // The first AND last line are present (nothing dropped).
        assert!(stdout.contains("line-0\n"), "first line kept at ==limit");
        assert!(
            stdout.contains(&format!("line-{}\n", LINE_LIMIT - 1)),
            "last line kept at ==limit"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("showing the last"),
            "no hint when total == LINE_LIMIT, got: {stderr}"
        );
    }

    // (b) LINE_LIMIT + 1 lines: oldest dropped, hint emitted with exact counts.
    {
        let sb = Sandbox::new("boundary-plus1");
        std::fs::write(sb.log_file(), body_of(LINE_LIMIT + 1)).unwrap();
        let mut cmd = bin();
        sb.apply_env(&mut cmd);
        cmd.arg("log");
        let out = cmd.output().expect("run vigil log (limit+1)");
        assert_eq!(out.status.code(), Some(0), "limit+1 exits 0");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            stdout.lines().count(),
            LINE_LIMIT,
            "output capped to exactly {LINE_LIMIT} lines"
        );
        // The very first line (line-0) is dropped; the last (line-2000) is kept.
        assert!(
            !stdout.contains("line-0\n"),
            "oldest line dropped at limit+1"
        );
        assert!(
            stdout.contains(&format!("line-{}\n", LINE_LIMIT)),
            "newest line kept at limit+1"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&format!(
                "showing the last {} of {} lines",
                LINE_LIMIT,
                LINE_LIMIT + 1
            )),
            "exact truncation hint required at limit+1, got: {stderr}"
        );
    }
}

/// `vigil log` (no-follow) on a SMALL log prints the whole file (no truncation
/// hint) — paging only kicks in past the cap.
#[test]
fn log_small_file_prints_all_no_hint() {
    let sb = Sandbox::new("smalllog");
    std::fs::write(sb.log_file(), "alpha\nbeta\ngamma\n").unwrap();

    let mut cmd = bin();
    sb.apply_env(&mut cmd);
    cmd.arg("log");
    let out = cmd.output().expect("run vigil log (small)");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "alpha\nbeta\ngamma\n", "small log printed verbatim");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("showing the last"),
        "no truncation hint for a sub-cap file"
    );
}

/// `vigil log -f` / `vigil log --follow` on a MISSING log takes the same
/// missing-log fast path as no-follow: the soft message to STDOUT, exit 0. The
/// missing-file check runs BEFORE the follow branch, so neither flag spins up the
/// unbounded `tail -f` loop when there is no file.
#[test]
fn log_follow_on_missing_file_exits_0_with_soft_message() {
    let cases: &[(&str, &str)] = &[("short flag", "-f"), ("long flag", "--follow")];
    for &(label, flag) in cases {
        let sb = Sandbox::new("nolog-follow");
        let mut cmd = bin();
        sb.apply_env(&mut cmd);
        cmd.args(["log", flag]);
        let out = cmd.output().expect("run vigil log -f (no log)");
        assert_eq!(
            out.status.code(),
            Some(0),
            "{label}: follow on a missing log still exits 0"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("no log yet at") && stdout.contains("daemon.log"),
            "{label}: must print 'no log yet at {{log_file}}', got: {stdout}"
        );
    }
}

/// `vigil log` ignores a non-`-f` first argument (no error) — bash-faithful.
#[test]
fn log_ignores_unknown_first_arg() {
    let sb = Sandbox::new("ignorearg");
    std::fs::write(sb.log_file(), "only-line\n").unwrap();

    let mut cmd = bin();
    sb.apply_env(&mut cmd);
    // A bogus arg must be ignored (treated as no-follow), not an error.
    cmd.args(["log", "--bogus"]);
    let out = cmd.output().expect("run vigil log --bogus");
    assert_eq!(
        out.status.code(),
        Some(0),
        "unknown first arg is ignored, not an error"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "only-line\n");
}

/// A trivial guard: `run` joins all args with single spaces into the pidfile's
/// `cmd` field, byte-identically to bash `"$*"`. We verify by reading the pidfile
/// during a longer child's life.
#[test]
fn run_pidfile_cmd_field_is_space_joined() {
    let sb = Sandbox::new("cmdfield");
    let active = sb.active_dir();

    let mut cmd = bin();
    sb.apply_env(&mut cmd);
    cmd.args(["run", "sleep", "5"]);
    let mut child = cmd.spawn().expect("spawn vigil run sleep 5");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut body = String::new();
    while Instant::now() < deadline {
        if let Some(name) = wrapper_pidfiles(&active).into_iter().next()
            && let Ok(b) = std::fs::read_to_string(active.join(&name))
        {
            body = b;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    sigterm(child.id());
    let _ = child.wait();

    assert!(
        body.contains("\"cmd\":\"sleep 5\""),
        "pidfile cmd field must be the space-joined args, got: {body}"
    );
    assert!(
        body.contains("\"comm\":\"wrapper\""),
        "pidfile comm field must be 'wrapper', got: {body}"
    );
}
