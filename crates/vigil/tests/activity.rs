//! Cargo port of `tests/activity_test.sh` — session-dir model, mtime scan, and
//! the vscode hash-gate. Uses `tempfile::tempdir()` for HOME roots and sets
//! mtimes via `std::fs::FileTimes` (whole seconds) to avoid a new dependency.

use std::fs::{File, FileTimes};
use std::path::Path;
use std::time::{Duration, SystemTime};

use vigil::activity::scan::{
    Agent, AgentState, agent_state, is_active, latest_activity_age_secs, session_dir_from_home,
};
use vigil::activity::vscode::{
    RecentFile, VscodeState, chat_is_active, host_running, sha256_hex, vscode_transition,
};

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Set a file's mtime to a unix-seconds value.
fn set_mtime(path: &Path, unix_secs: i64) {
    let f = File::options().write(true).open(path).unwrap();
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(unix_secs as u64);
    f.set_times(FileTimes::new().set_accessed(t).set_modified(t))
        .unwrap();
}

// ── session-dir model ───────────────────────────────────────────────────────

#[test]
fn session_dir_for_known_agents() {
    let home = tempfile::tempdir().unwrap();
    let h = home.path();
    assert_eq!(
        session_dir_from_home(Agent::Claude, h),
        h.join(".claude/projects")
    );
    assert_eq!(
        session_dir_from_home(Agent::Codex, h),
        h.join(".codex/sessions")
    );
    assert_eq!(
        session_dir_from_home(Agent::Copilot, h),
        h.join(".copilot/session-state")
    );
}

#[test]
fn patterns_for_each_agent() {
    assert!(Agent::Claude.pattern().matches("abc.jsonl"));
    assert!(Agent::Codex.pattern().matches("rollout-x.jsonl"));
    assert!(Agent::Copilot.pattern().matches("events.jsonl"));
    assert!(!Agent::Copilot.pattern().matches("notes.txt"));
}

// ── is_active / agent_state ─────────────────────────────────────────────────

#[test]
fn agent_active_when_recent_jsonl() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".claude/projects/some-cwd");
    std::fs::create_dir_all(&dir).unwrap();
    File::create(dir.join("abc.jsonl")).unwrap(); // mtime = now
    let sdir = session_dir_from_home(Agent::Claude, home.path());
    assert!(is_active(&sdir, Agent::Claude.pattern(), 300, now_unix()));
}

#[test]
fn agent_inactive_when_old_file() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".claude/projects/some-cwd");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("abc.jsonl");
    File::create(&f).unwrap();
    set_mtime(&f, 946684800); // 2000-01-01
    let sdir = session_dir_from_home(Agent::Claude, home.path());
    assert!(!is_active(&sdir, Agent::Claude.pattern(), 300, now_unix()));
}

#[test]
fn agent_inactive_when_dir_missing() {
    let home = tempfile::tempdir().unwrap();
    let sdir = session_dir_from_home(Agent::Claude, home.path());
    assert!(!is_active(&sdir, Agent::Claude.pattern(), 300, now_unix()));
}

#[test]
fn agent_active_for_codex_subdir_layout() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".codex/sessions/2026/05/06");
    std::fs::create_dir_all(&dir).unwrap();
    File::create(dir.join("rollout-2026-05-06T10-10-10-uuid.jsonl")).unwrap();
    let sdir = session_dir_from_home(Agent::Codex, home.path());
    assert!(
        is_active(&sdir, Agent::Codex.pattern(), 300, now_unix()),
        "codex deep subdir must be detected (recursive)"
    );
}

#[test]
fn pattern_filter_rejects_wrong_extension() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".copilot/session-state/abc-uuid");
    std::fs::create_dir_all(&dir).unwrap();
    File::create(dir.join("notes.txt")).unwrap();
    let sdir = session_dir_from_home(Agent::Copilot, home.path());
    assert!(!is_active(&sdir, Agent::Copilot.pattern(), 300, now_unix()));
}

#[test]
fn agent_state_returns_none_when_dir_missing() {
    let home = tempfile::tempdir().unwrap();
    let sdir = session_dir_from_home(Agent::Copilot, home.path());
    assert_eq!(
        agent_state(&sdir, Agent::Copilot.pattern(), 300, now_unix()),
        AgentState::None
    );
}

#[test]
fn latest_activity_age_uses_newest_matching_file() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".codex/sessions/2026/06/12");
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("rollout-old.jsonl");
    File::create(&old).unwrap();
    set_mtime(&old, 946684800);
    File::create(dir.join("rollout-new.jsonl")).unwrap(); // mtime now
    let sdir = session_dir_from_home(Agent::Codex, home.path());
    let age = latest_activity_age_secs(&sdir, Agent::Codex.pattern(), now_unix()).unwrap();
    assert!(
        (0..=60).contains(&age),
        "expected newest age <= 60s, got {age}"
    );
}

// ── vscode hash-gate ────────────────────────────────────────────────────────

/// Create a vscode state.json with the given epoch under a temp home (Insiders).
/// Returns the state.json path.
fn mk_vscode_state_file(home: &Path, epoch: i64) -> std::path::PathBuf {
    let dir = home.join(
        "Library/Application Support/Code - Insiders/User/workspaceStorage/hash/chatEditingSessions/session",
    );
    std::fs::create_dir_all(&dir).unwrap();
    let body = format!(
        "{{\"version\":1,\"timeline\":{{\"checkpoints\":[{{\"checkpointId\":\"c{epoch}\",\"epoch\":{epoch},\"label\":\"l\",\"description\":\"d\"}}],\"currentEpoch\":{epoch},\"fileBaselines\":[],\"operations\":[],\"epochCounter\":{epoch}}},\"recentSnapshot\":{{\"entries\":[]}},\"initialFileContents\":[]}}\n"
    );
    let p = dir.join("state.json");
    std::fs::write(&p, body).unwrap();
    p
}

/// Mirror `_reset_vscode_scan_timer`: set last_scan to 0 in the serialized state.
fn reset_scan_timer(state_file: &Path) {
    let Ok(text) = std::fs::read_to_string(state_file) else {
        return;
    };
    let mut st = VscodeState::parse(&text);
    st.last_scan = 0;
    std::fs::write(state_file, st.serialize()).unwrap();
}

const INSIDERS_PS: &str =
    "/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Code - Insiders";

#[test]
fn vscode_hash_change_drives_activity() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let state_file = root.path().join("vscode-copilot.state");
    let chat_state = mk_vscode_state_file(&home, 1);

    // 1. First scan -> primes, no count.
    assert!(
        !chat_is_active(
            &home,
            &state_file,
            now_unix(),
            300,
            5,
            10,
            Some(INSIDERS_PS)
        ),
        "first scan should prime without counting active"
    );

    // 2. mtime-only rewrite (touch) + reset timer -> stays idle.
    let t = SystemTime::now();
    File::options()
        .write(true)
        .open(&chat_state)
        .unwrap()
        .set_times(FileTimes::new().set_modified(t))
        .unwrap();
    reset_scan_timer(&state_file);
    assert!(
        !chat_is_active(
            &home,
            &state_file,
            now_unix(),
            300,
            5,
            10,
            Some(INSIDERS_PS)
        ),
        "mtime-only rewrite (unchanged hash) should stay idle"
    );

    // 3. semantic change (epoch 2) + reset timer -> active.
    let _ = mk_vscode_state_file(&home, 2);
    reset_scan_timer(&state_file);
    assert!(
        chat_is_active(
            &home,
            &state_file,
            now_unix(),
            300,
            5,
            10,
            Some(INSIDERS_PS)
        ),
        "semantic state change should count active"
    );
}

#[test]
fn vscode_retains_hashes_after_file_ages_out() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let state_file = root.path().join("vscode-copilot.state");
    let chat_state = mk_vscode_state_file(&home, 1);

    // recent_mins = 1.
    assert!(
        !chat_is_active(&home, &state_file, now_unix(), 300, 5, 1, Some(INSIDERS_PS)),
        "first scan should only prime"
    );
    // Age the file out of the recent window.
    set_mtime(&chat_state, 946684800);
    reset_scan_timer(&state_file);
    assert!(
        !chat_is_active(&home, &state_file, now_unix(), 300, 5, 1, Some(INSIDERS_PS)),
        "aged-out known file should not count active"
    );
    // Bring it back recent with UNCHANGED hash.
    let t = SystemTime::now();
    File::options()
        .write(true)
        .open(&chat_state)
        .unwrap()
        .set_times(FileTimes::new().set_modified(t))
        .unwrap();
    reset_scan_timer(&state_file);
    assert!(
        !chat_is_active(&home, &state_file, now_unix(), 300, 5, 1, Some(INSIDERS_PS)),
        "unchanged known file reappearing as recent should not count (retained hash)"
    );
}

#[test]
fn vscode_state_is_none_without_host() {
    // host_running(Some("")) is the "no host" case -> false (state would be none).
    assert!(
        !host_running(Some("")),
        "empty ps text means no VS Code host"
    );
}

#[test]
fn sha256_spot_check() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn vscode_transition_primed_first_run_suppression() {
    // Pure-core cross-check of primed-first-run suppression.
    let prior = VscodeState::default(); // primed = false
    let current = vec![RecentFile {
        path: "/x".into(),
        sha256: "h1".into(),
    }];
    let (new, active) = vscode_transition(&prior, &current, 1000, 300, 5);
    assert!(!active);
    assert!(new.unwrap().primed, "first run must set primed");
}
