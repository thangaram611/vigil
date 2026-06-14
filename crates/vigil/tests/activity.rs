//! Cargo port of `tests/activity_test.sh` — session-dir model, mtime scan, and
//! the vscode hash-gate. Uses `tempfile::tempdir()` for HOME roots and sets
//! mtimes via `std::fs::FileTimes` (whole seconds) to avoid a new dependency.

use std::fs::{File, FileTimes};
use std::path::Path;
use std::time::{Duration, SystemTime};

use vigil::activity::scan::{
    Agent, AgentState, agent_state, is_active, latest_activity_age_secs, session_dir_from_home,
};
use vigil::activity::vscode::{VscodeState, chat_is_active, host_running};

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
    let cases: &[(Agent, &str)] = &[
        (Agent::Claude, ".claude/projects"),
        (Agent::Codex, ".codex/sessions"),
        (Agent::Copilot, ".copilot/session-state"),
    ];
    for &(agent, suffix) in cases {
        assert_eq!(session_dir_from_home(agent, h), h.join(suffix), "{agent:?}");
    }
}

// ── is_active / agent_state ─────────────────────────────────────────────────

#[test]
fn is_active_scenarios() {
    // Each row builds a fresh temp HOME, runs its `setup` closure (which creates
    // the session dir/file layout under that HOME and returns the agent to scan),
    // then asserts `is_active` against the resolved session dir. `setup` returning
    // `None` for the agent means "create nothing" (dir-missing case). The two
    // active rows create the file with mtime=now (no `set_mtime`) and are mildly
    // time-sensitive, so their setup is preserved byte-for-byte from the originals.
    struct Case {
        label: &'static str,
        agent: Agent,
        setup: fn(&Path),
        want_active: bool,
    }
    let cases: &[Case] = &[
        Case {
            // agent_active_when_recent_jsonl
            label: "claude recent jsonl is active",
            agent: Agent::Claude,
            setup: |home| {
                let dir = home.join(".claude/projects/some-cwd");
                std::fs::create_dir_all(&dir).unwrap();
                File::create(dir.join("abc.jsonl")).unwrap(); // mtime = now
            },
            want_active: true,
        },
        Case {
            // agent_inactive_when_old_file
            label: "claude old jsonl is inactive",
            agent: Agent::Claude,
            setup: |home| {
                let dir = home.join(".claude/projects/some-cwd");
                std::fs::create_dir_all(&dir).unwrap();
                let f = dir.join("abc.jsonl");
                File::create(&f).unwrap();
                set_mtime(&f, 946684800); // 2000-01-01
            },
            want_active: false,
        },
        Case {
            // agent_inactive_when_dir_missing
            label: "claude missing dir is inactive",
            agent: Agent::Claude,
            setup: |_home| {},
            want_active: false,
        },
        Case {
            // agent_active_for_codex_subdir_layout (recursive deep subdir)
            label: "codex deep subdir must be detected (recursive)",
            agent: Agent::Codex,
            setup: |home| {
                let dir = home.join(".codex/sessions/2026/05/06");
                std::fs::create_dir_all(&dir).unwrap();
                File::create(dir.join("rollout-2026-05-06T10-10-10-uuid.jsonl")).unwrap();
            },
            want_active: true,
        },
        Case {
            // pattern_filter_rejects_wrong_extension
            label: "copilot wrong extension is rejected",
            agent: Agent::Copilot,
            setup: |home| {
                let dir = home.join(".copilot/session-state/abc-uuid");
                std::fs::create_dir_all(&dir).unwrap();
                File::create(dir.join("notes.txt")).unwrap();
            },
            want_active: false,
        },
    ];
    for c in cases {
        let home = tempfile::tempdir().unwrap();
        (c.setup)(home.path());
        let sdir = session_dir_from_home(c.agent, home.path());
        assert_eq!(
            is_active(&sdir, c.agent.pattern(), 300, now_unix()),
            c.want_active,
            "{}",
            c.label
        );
    }
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
