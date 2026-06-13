//! Cargo port of `tests/refcount_activity_test.sh` (count/list) + the
//! `tests/parser_test.sh` field() cases + pidfile body byte-shape + the
//! `gc_decision` unit table. count/list parse ONLY the filename, so the tests
//! seed EMPTY-body `*.pid` files (matching the bash tests).

use std::fs::File;
use std::path::Path;

use vigil::refcount::{
    GcDecision, count, count_total, field, gc_decision, list, pidfile_body, wrapper_pidfile_body,
};

/// Create an empty-body `<name>.pid` file under `dir`.
fn make_pidfile(dir: &Path, name: &str) {
    File::create(dir.join(format!("{name}.pid"))).unwrap();
}

// ── count / list (refcount_activity_test) ───────────────────────────────────

#[test]
fn count_with_claude_active_only() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path();
    make_pidfile(a, "cli-claude-1001");
    make_pidfile(a, "cli-claude-1002");
    make_pidfile(a, "cli-codex-1003");
    make_pidfile(a, "wrapper-1004");
    assert_eq!(count(a, true, false, false, false), 3, "2 claude + wrapper");
    assert_eq!(count_total(a), 4);
}

#[test]
fn count_with_app_codex_gated_on_codex_flag() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path();
    make_pidfile(a, "app-codex-2700");
    make_pidfile(a, "cli-claude-1001");
    assert_eq!(
        count(a, true, false, false, false),
        1,
        "codex idle: app-codex gated out"
    );
    assert_eq!(
        count(a, true, true, false, false),
        2,
        "codex active: app-codex joins"
    );
    assert_eq!(count(a, false, true, false, false), 1, "only app-codex");
}

#[test]
fn count_with_vscode_copilot_chat_gated_on_activity_flag() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path();
    make_pidfile(a, "app-vscode-copilot-chat-22222");
    assert_eq!(
        count(a, false, false, false, false),
        0,
        "idle vscode chat gated out"
    );
    assert_eq!(
        count(a, false, false, false, true),
        1,
        "active vscode chat counts"
    );
}

#[test]
fn count_when_all_idle() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path();
    make_pidfile(a, "cli-claude-1001");
    make_pidfile(a, "cli-claude-1002");
    make_pidfile(a, "cli-codex-1003");
    make_pidfile(a, "wrapper-1004");
    assert_eq!(count(a, false, false, false, false), 1, "wrapper only");
}

#[test]
fn list_state_column_matches_flags() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path();
    make_pidfile(a, "cli-claude-1001");
    make_pidfile(a, "cli-codex-1002");
    make_pidfile(a, "app-codex-2700");
    make_pidfile(a, "wrapper-1003");
    let rows = list(a, 0, true, false, false, false);
    let find = |pid: u32| rows.iter().find(|(p, _, _, _)| *p == pid).unwrap();
    assert_eq!(find(1001).3, "active", "claude row active");
    assert_eq!(find(1002).3, "idle", "codex row idle");
    assert_eq!(find(2700).3, "idle", "app-codex mirrors codex flag");
    assert_eq!(find(1003).3, "active", "wrapper always active");
    // names parsed from filename.
    assert_eq!(find(1001).1, "cli-claude");
    assert_eq!(find(2700).1, "app-codex");
}

#[test]
fn filename_parser_handles_all_prefixes() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path();
    make_pidfile(a, "cli-claude-1");
    make_pidfile(a, "cli-codex-2");
    make_pidfile(a, "cli-copilot-3");
    make_pidfile(a, "app-codex-4");
    make_pidfile(a, "app-vscode-copilot-chat-6");
    make_pidfile(a, "wrapper-5");
    assert_eq!(
        count(a, true, true, true, true),
        6,
        "all six prefixes count"
    );
}

#[test]
fn wrappers_count_regardless_of_agent_flags() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path();
    make_pidfile(a, "wrapper-1234");
    assert_eq!(count(a, false, false, false, false), 1);
}

// ── field extraction (parser_test) ──────────────────────────────────────────

#[test]
fn field_extracts_pid() {
    let body = r#"{"pid":1234,"comm":"claude","start_ts":1700000000,"name":"cli-claude"}"#;
    assert_eq!(field(body, "pid").as_deref(), Some("1234"));
}

#[test]
fn field_extracts_start_ts_not_pid() {
    let body = r#"{"pid":1234,"comm":"claude","start_ts":1700000000,"name":"cli-claude"}"#;
    assert_eq!(
        field(body, "start_ts").as_deref(),
        Some("1700000000"),
        "must NOT return the pid"
    );
}

#[test]
fn field_extracts_string_field() {
    let body = r#"{"pid":1234,"comm":"claude","start_ts":1700000000,"name":"cli-claude"}"#;
    assert_eq!(field(body, "name").as_deref(), Some("cli-claude"));
}

#[test]
fn field_returns_none_when_key_missing() {
    assert_eq!(field(r#"{"pid":1234}"#, "nope"), None);
}

#[test]
fn field_baseline_sleepdisabled() {
    let body = r#"{"SleepDisabled":1,"captured_at":1700000000}"#;
    assert_eq!(field(body, "SleepDisabled").as_deref(), Some("1"));
}

// ── pidfile body byte-shape ─────────────────────────────────────────────────

#[test]
fn pidfile_body_byte_shape() {
    assert_eq!(
        pidfile_body("cli-claude", 1234, "claude", 1700000000),
        "{\"pid\":1234,\"comm\":\"claude\",\"start_ts\":1700000000,\"name\":\"cli-claude\"}\n"
    );
}

#[test]
fn pidfile_body_strips_quotes() {
    // exe a"b -> comm":"ab"
    assert_eq!(
        pidfile_body("cli-claude", 1, "a\"b", 5),
        "{\"pid\":1,\"comm\":\"ab\",\"start_ts\":5,\"name\":\"cli-claude\"}\n"
    );
}

#[test]
fn wrapper_body_byte_shape() {
    assert_eq!(
        wrapper_pidfile_body(1004, "sleep 60", 1700000000),
        "{\"pid\":1004,\"comm\":\"wrapper\",\"start_ts\":1700000000,\"cmd\":\"sleep 60\"}\n"
    );
    // quote stripping in cmd.
    assert_eq!(
        wrapper_pidfile_body(1, "a\"b", 5),
        "{\"pid\":1,\"comm\":\"wrapper\",\"start_ts\":5,\"cmd\":\"ab\"}\n"
    );
}

// ── gc_decision unit table ──────────────────────────────────────────────────

#[test]
fn gc_dead_pid_drops_any_name() {
    assert_eq!(
        gc_decision("cli-claude", 0, false, Some(1), Some(1), Some(0.9), 30, 0.5),
        GcDecision::DropDead
    );
    assert_eq!(
        gc_decision("wrapper", 0, false, Some(1), Some(1), Some(0.9), 30, 0.5),
        GcDecision::DropDead
    );
}

#[test]
fn gc_pid_reuse_drops_any_name() {
    assert_eq!(
        gc_decision("cli-claude", 0, true, Some(1), Some(2), Some(0.9), 30, 0.5),
        GcDecision::DropPidReuse
    );
    assert_eq!(
        gc_decision("wrapper", 0, true, Some(1), Some(2), Some(0.0), 30, 0.5),
        GcDecision::DropPidReuse
    );
}

#[test]
fn gc_wrapper_carved_out_of_idle() {
    // wrapper: old + low cpu but alive + matching start -> Keep.
    assert_eq!(
        gc_decision("wrapper", 9999, true, Some(1), Some(1), Some(0.0), 30, 0.5),
        GcDecision::Keep
    );
}

#[test]
fn gc_idle_drops_agent() {
    assert_eq!(
        gc_decision(
            "cli-claude",
            9999,
            true,
            Some(1),
            Some(1),
            Some(0.0),
            30,
            0.5
        ),
        GcDecision::DropIdle
    );
}

#[test]
fn gc_busy_agent_kept() {
    assert_eq!(
        gc_decision(
            "cli-claude",
            9999,
            true,
            Some(1),
            Some(1),
            Some(0.9),
            30,
            0.5
        ),
        GcDecision::Keep
    );
}

#[test]
fn gc_branch_order_dead_beats_reuse_beats_idle() {
    // dead beats reuse (start mismatch present, but pid dead).
    assert_eq!(
        gc_decision(
            "cli-claude",
            9999,
            false,
            Some(1),
            Some(2),
            Some(0.0),
            30,
            0.5
        ),
        GcDecision::DropDead
    );
    // reuse beats idle (alive, start mismatch, also old+low-cpu).
    assert_eq!(
        gc_decision(
            "cli-claude",
            9999,
            true,
            Some(1),
            Some(2),
            Some(0.0),
            30,
            0.5
        ),
        GcDecision::DropPidReuse
    );
}
