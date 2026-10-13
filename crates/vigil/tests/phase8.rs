//! Phase 8 regressions. Native projections are fixtures, not live coverage claims.
use vigil::procscan::{AgentKind, detect_line};

#[test]
fn current_desktop_worker_is_detected_and_exec_server_is_excluded() {
    let exe =
        "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex";
    assert_eq!(
        detect_line(42, exe, &format!("{exe} app-server"))
            .unwrap()
            .kind,
        AgentKind::AppCodex
    );
    assert!(detect_line(43, exe, &format!("{exe} exec-server")).is_none());
    assert_eq!(
        detect_line(
            42,
            exe,
            &format!(
                "{exe} -c features.code_mode_host=true app-server --analytics-default-enabled"
            )
        )
        .unwrap()
        .kind,
        AgentKind::AppCodex
    );
}

use vigil::activity::{ActivitySnapshot, SessionActivity, State};
fn session(id: &str, state: State) -> SessionActivity {
    SessionActivity {
        provider: "claude".into(),
        id: id.into(),
        state,
        evidence: "native".into(),
        reason: "fixture".into(),
        foreground_wait: None,
        uncertain_demand: false,
    }
}
#[test]
fn one_busy_session_does_not_count_every_provider_pidfile() {
    let s = ActivitySnapshot {
        sessions: vec![
            session("busy", State::Working),
            session("wait", State::Waiting),
        ],
        detected_hosts: 2,
        ..Default::default()
    };
    assert_eq!((s.working(), s.waiting(), s.demand()), (1, 1, 1));
    let shared = ActivitySnapshot {
        sessions: vec![session("a", State::Working), session("b", State::Working)],
        detected_hosts: 1,
        ..Default::default()
    };
    assert_eq!((shared.working(), shared.demand()), (2, 2));
}
fn fixture(name: &str) -> Vec<serde_json::Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/fixtures/phase-8")
        .join(name);
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
#[test]
fn copilot_native_checkpoints() {
    let events = fixture("copilot-subagents-1.0.95.jsonl");
    for (time, want) in [
        ("2026-10-13T08:26:40.066Z", State::Working),
        ("2026-10-13T08:26:49.000Z", State::Waiting),
        ("2026-10-13T08:33:54.000Z", State::Waiting),
        ("2026-10-13T08:34:29.000Z", State::Working),
        ("2026-10-13T08:35:43.000Z", State::Idle),
    ] {
        let mut reader = vigil::activity::copilot::Copilot::default();
        for v in events
            .iter()
            .filter(|v| v["timestamp"].as_str().unwrap() <= time)
        {
            reader.event(v);
        }
        assert_eq!(
            reader.state(),
            want,
            "checkpoint {time}; gap={}",
            reader.gap
        );
    }
}
#[test]
fn codex_opaque_approval_and_late_terminal() {
    let mut r = vigil::activity::codex::Codex::default();
    for v in fixture("codex-approval-pending.jsonl") {
        r.event(&v);
    }
    assert_eq!(r.state(), State::Unknown);
    assert!(r.unfinished());
    let mut r = vigil::activity::codex::Codex::default();
    for v in fixture("codex-terminals.jsonl") {
        r.event(&v);
    }
    assert_eq!(r.state(), State::Idle);
    assert!(!r.unfinished());
}
#[test]
fn claude_quiet_synthesis_then_duplicate_completion() {
    let parent = fixture("claude-parent-2.1.296.jsonl");
    let child = fixture("claude-child-2.1.296.jsonl");
    let mut r = vigil::activity::claude::Claude::default();
    for v in parent
        .iter()
        .filter(|v| v["timestamp"].as_str().unwrap() < "2026-10-12T10:48:27.000Z")
    {
        r.event(v);
    }
    for v in child
        .iter()
        .filter(|v| v["timestamp"].as_str().unwrap() < "2026-10-12T10:48:27.000Z")
    {
        r.event(v);
    }
    assert_eq!(
        r.child_state(true),
        State::Working,
        "quiet synthesis has no arbitrary timeout"
    );
    for v in parent
        .iter()
        .filter(|v| v["timestamp"].as_str().unwrap() >= "2026-10-12T10:48:27.000Z")
    {
        r.event(v);
        r.event(v);
    }
    assert_eq!(r.child_state(true), State::Idle);
    assert!(!r.unfinished());
}
#[test]
fn native_loop_pending_cancelled_and_stale_job_state() {
    for (name, want) in [
        ("claude-loop-pending.json", State::Working),
        ("claude-loop-cancelled.json", State::Idle),
        ("claude-loop-final.json", State::Idle),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/fixtures/phase-8")
            .join(name);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let (session, job, wake) = if name == "claude-loop-final.json" {
            (
                &v["final_sample"]["sessions"][0],
                &v["final_sample"]["jobs"][0],
                None,
            )
        } else {
            (
                &v["session"],
                &v["job"],
                v.get("pending_wakeup").filter(|w| w.is_object()),
            )
        };
        assert_eq!(
            vigil::activity::claude::loop_state(session, job, wake),
            want,
            "{name}"
        );
    }
}
#[test]
fn wrapper_actual_birth_preserves_pid_reuse_protection() {
    use vigil::refcount::{GcDecision, gc_decision};
    assert_eq!(
        gc_decision(
            "wrapper",
            0,
            true,
            Some(1010),
            Some(1000),
            Some(0.0),
            300,
            1.0
        ),
        GcDecision::DropPidReuse
    );
    let start = vigil::procscan::ProcScanner::process_start(std::process::id()).unwrap();
    let body = vigil::refcount::wrapper_pidfile_body(std::process::id(), "fixture", start);
    assert_eq!(
        vigil::refcount::field(&body, "start_ts"),
        Some(start.to_string())
    );
    assert_eq!(
        gc_decision(
            "wrapper",
            0,
            true,
            Some(start),
            Some(start),
            Some(0.0),
            300,
            1.0
        ),
        GcDecision::Keep
    );
}
#[test]
fn incremental_partial_replaced_truncated_oversized_and_zero_reread() {
    use std::cell::RefCell;
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("events");
    std::fs::write(&p, b"{\"n\":1}\n{\"n\":").unwrap();
    let mut reader = vigil::activity::jsonl::Reader::default();
    let events = RefCell::new(Vec::new());
    let read = |r: &mut vigil::activity::jsonl::Reader| {
        let mut budget = vigil::activity::jsonl::EVALUATION_BYTES;
        r.read(
            &p,
            &mut budget,
            || events.borrow_mut().clear(),
            |v| events.borrow_mut().push(v),
        )
        .unwrap()
    };
    assert!(read(&mut reader) > 0);
    assert!(!reader.caught_up);
    assert_eq!(events.borrow().len(), 1);
    let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    f.write_all(b"2}\n").unwrap();
    read(&mut reader);
    assert!(reader.caught_up);
    assert_eq!(events.borrow().len(), 2);
    assert_eq!(read(&mut reader), 0);
    std::fs::write(&p, b"{\"n\":3}\n").unwrap();
    read(&mut reader);
    assert_eq!(events.borrow().len(), 1);
    let replacement = tmp.path().join("new");
    std::fs::write(&replacement, b"{\"n\":4}\n").unwrap();
    std::fs::rename(replacement, &p).unwrap();
    read(&mut reader);
    assert_eq!(events.borrow()[0]["n"], 4);
    let huge = format!("{{\"text\":\"{}\"}}\n", "x".repeat(1024 * 1024 + 100));
    std::fs::write(&p, huge).unwrap();
    read(&mut reader);
    assert!(!reader.gap);
    std::fs::write(
        &p,
        format!("{}\n{{\"n\":5}}\n", "x".repeat(8 * 1024 * 1024 + 1)),
    )
    .unwrap();
    read(&mut reader);
    assert!(reader.gap);
    assert_eq!(events.borrow()[0]["n"], 5);
}

#[test]
fn synthetic_child_rebuild_clears_old_calls_and_dedup_state() {
    use serde_json::json;
    let mut r = vigil::activity::claude::Claude::default();
    r.event(&json!({"type":"user","uuid":"launch","message":{"content":[{"type":"tool_result","tool_use_id":"dispatch"}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"child"}}));
    r.event(&json!({"uuid":"old","agentId":"child","message":{"content":[{"type":"tool_use","id":"call"}],"stop_reason":"tool_use"}}));
    assert_eq!(r.child_state(true), State::Unknown);
    r.mark_child_gap("child");
    assert_eq!(r.child_state(true), State::Unknown);
    r.reset_child("child");
    r.event(
        &json!({"uuid":"old","agentId":"child","message":{"content":[],"stop_reason":"end_turn"}}),
    );
    assert_eq!(r.child_state(true), State::Idle);
}
#[test]
fn synthetic_restarted_worker_cannot_resurrect_old_loop() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/fixtures/phase-8/claude-loop-pending.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["session"]["procStart"] = "Tue Oct 13 06:15:20 2026".into();
    assert_eq!(
        vigil::activity::claude::loop_state(&v["session"], &v["job"], Some(&v["pending_wakeup"])),
        State::Unknown
    );
}
#[test]
fn provider_prompt_words_are_not_roles() {
    for prompt in ["daemon", "agents", "attach"] {
        assert!(
            detect_line(
                12,
                "/opt/homebrew/bin/claude",
                &format!("/opt/homebrew/bin/claude -p review the {prompt}")
            )
            .is_some()
        );
    }
}
#[test]
fn preserved_native_fixture_hashes_and_envelopes_are_intact() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixtures/phase-8");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for entry in manifest["files"].as_array().unwrap() {
        let name = entry["file"].as_str().unwrap();
        let bytes = std::fs::read(root.join(name)).unwrap();
        assert_eq!(
            Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            entry["sha256"].as_str().unwrap(),
            "{name}"
        );
        if name.ends_with(".jsonl") {
            for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                serde_json::from_slice::<serde_json::Value>(line).unwrap();
            }
        } else {
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
        }
    }
}

#[test]
fn synthetic_mixed_old_and_owned_work_keeps_current_generation_and_uncertainty() {
    use serde_json::json;
    let mut codex = vigil::activity::codex::Codex::default();
    for (id, start) in [("old", 10), ("new", 110)] {
        codex.event(&json!({"type":"event_msg","payload":{"type":"task_started","turn_id":id,"started_at":start}}));
    }
    assert_eq!(codex.owned_state(100), (State::Working, true));
    codex.event(&json!({"type":"response_item","payload":{"type":"custom_tool_call","call_id":"opaque","internal_chat_message_metadata_passthrough":{"turn_id":"new"}}}));
    assert_eq!(codex.owned_state(100), (State::Unknown, true));
    let mut claude = vigil::activity::claude::Claude::default();
    for (id, time) in [
        ("old", "1970-01-01T00:00:10Z"),
        ("new", "1970-01-01T00:01:50Z"),
    ] {
        claude.event(&json!({"type":"user","uuid":id,"timestamp":time,"message":{"content":[{"type":"tool_result","tool_use_id":id}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":id}}));
    }
    let mut incomplete = std::collections::BTreeSet::new();
    incomplete.insert("old".into());
    assert_eq!(
        claude.owned_state(true, 100, &incomplete),
        (State::Working, true)
    );
    claude.event(&json!({"uuid":"pending","agentId":"new","message":{"content":[{"type":"tool_use","id":"approval"}]}}));
    assert_eq!(
        claude.owned_state(true, 100, &incomplete),
        (State::Unknown, true)
    );
    let mut copilot = vigil::activity::copilot::Copilot::default();
    copilot.event(&json!({"id":"old","type":"subagent.started","timestamp":"1970-01-01T00:00:10Z","data":{"toolCallId":"old-child"}}));
    copilot.event(&json!({"id":"new","type":"assistant.turn_start","timestamp":"1970-01-01T00:01:50Z","data":{"turnId":"0"}}));
    assert_eq!(copilot.owned_state(100), (State::Working, true));
    copilot.event(&json!({"id":"wait","type":"tool.execution_start","timestamp":"1970-01-01T00:01:51Z","data":{"toolCallId":"question","toolName":"ask_user"}}));
    assert_eq!(copilot.owned_state(100), (State::Waiting, true));
}

#[test]
fn nonregular_transcript_returns_unknown_input_without_blocking() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("fifo.jsonl");
    nix::unistd::mkfifo(
        &path,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    let mut reader = vigil::activity::jsonl::Reader::default();
    let mut budget = 1024;
    assert!(
        reader
            .read(
                &path,
                &mut budget,
                || {},
                |_| panic!("FIFO is not native history")
            )
            .is_err()
    );
}
#[test]
fn implementation_copilot_smoke_projection_replays_to_idle() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/fixtures/phase-8-implementation");
    let mut copilot = vigil::activity::copilot::Copilot::default();
    for line in std::fs::read_to_string(root.join("copilot-smoke-1.0.95.jsonl"))
        .unwrap()
        .lines()
    {
        copilot.event(&serde_json::from_str(line).unwrap());
    }
    assert_eq!(copilot.state(), State::Idle);
    let summary: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("copilot-smoke-summary.json")).unwrap())
            .unwrap();
    assert_eq!(summary["exit"], 0);
    assert_eq!(summary["cleanup_host_gone"], true);
    assert_eq!(summary["registered_daemon_seen"], true);
}

#[test]
fn source_only_ambiguous_claude_resume_never_reuses_old_terminal_as_idle() {
    use serde_json::json;
    let mut r = vigil::activity::claude::Claude::default();
    let launch = |uuid: &str| json!({"type":"user","uuid":uuid,"timestamp":"2026-10-13T10:00:00Z","message":{"content":[{"type":"tool_result","tool_use_id":"same-call"}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"same-agent"}});
    let terminal = json!({"type":"queue-operation","content":"<task-notification><task-id>same-agent</task-id><tool-use-id>same-call</tool-use-id><status>completed</status></task-notification>"});
    r.event(&launch("first"));
    r.event(&terminal);
    assert_eq!(r.child_state(true), State::Idle);
    r.event(&launch("resume"));
    r.event(&terminal);
    assert_eq!(r.child_state(true), State::Unknown);
    assert!(r.unfinished());
}

#[test]
fn source_only_relaunch_after_child_generation_ends_remains_unknown() {
    use serde_json::json;
    let mut r = vigil::activity::claude::Claude::default();
    for id in ["first", "resume"] {
        r.event(&json!({"type":"user","uuid":id,"timestamp":"2026-10-13T10:00:00Z","message":{"content":[{"type":"tool_result","tool_use_id":"same-call"}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"same-agent"}}));
        if id == "first" {
            r.event(
                &json!({"uuid":"end","agentId":"same-agent","message":{"stop_reason":"end_turn"}}),
            );
            assert_eq!(r.child_state(true), State::Idle);
        }
    }
    assert_eq!(r.child_state(true), State::Unknown);
    assert!(r.unfinished());
}
#[test]
fn unknown_old_file_does_not_borrow_recent_sibling_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let old = tmp.path().join("old.jsonl");
    let fresh = tmp.path().join("fresh.jsonl");
    std::fs::write(&old, "{}\n").unwrap();
    std::fs::write(fresh, "{}\n").unwrap();
    let now = chrono::Utc::now().timestamp();
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs((now - 3600) as u64);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(time))
        .unwrap();
    assert!(!vigil::activity::scan::recent_file(&old, 300, now));
}
