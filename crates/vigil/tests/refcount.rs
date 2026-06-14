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

/// Seed an active-dir tempdir with empty-body `<name>.pid` files. Returns the
/// `TempDir` guard (keep it alive so the dir isn't unlinked) and its path.
fn seed(names: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().to_path_buf();
    for n in names {
        make_pidfile(&a, n);
    }
    (d, a)
}

// ── count / list (refcount_activity_test) ───────────────────────────────────

#[test]
fn count_with_claude_active_only() {
    let (_d, a) = seed(&[
        "cli-claude-1001",
        "cli-claude-1002",
        "cli-codex-1003",
        "wrapper-1004",
    ]);
    assert_eq!(
        count(&a, true, false, false, false),
        3,
        "2 claude + wrapper"
    );
    assert_eq!(count_total(&a), 4);
}

#[test]
fn count_with_app_codex_gated_on_codex_flag() {
    let (_d, a) = seed(&["app-codex-2700", "cli-claude-1001"]);
    assert_eq!(
        count(&a, true, false, false, false),
        1,
        "codex idle: app-codex gated out"
    );
    assert_eq!(
        count(&a, true, true, false, false),
        2,
        "codex active: app-codex joins"
    );
    assert_eq!(count(&a, false, true, false, false), 1, "only app-codex");
}

#[test]
fn count_with_vscode_copilot_chat_gated_on_activity_flag() {
    let (_d, a) = seed(&["app-vscode-copilot-chat-22222"]);
    assert_eq!(
        count(&a, false, false, false, false),
        0,
        "idle vscode chat gated out"
    );
    assert_eq!(
        count(&a, false, false, false, true),
        1,
        "active vscode chat counts"
    );
}

#[test]
fn count_when_all_idle() {
    // All agent flags false: every cli-* is gated out and only the wrapper counts,
    // both alongside idle agents AND as the lone entry (wrapper ALWAYS counts,
    // regardless of agent flags — formerly wrappers_count_regardless_of_agent_flags).
    let (_d, a) = seed(&[
        "cli-claude-1001",
        "cli-claude-1002",
        "cli-codex-1003",
        "wrapper-1004",
    ]);
    assert_eq!(count(&a, false, false, false, false), 1, "wrapper only");

    // Lone-wrapper branch: no agents present at all.
    let (_d2, b) = seed(&["wrapper-1234"]);
    assert_eq!(
        count(&b, false, false, false, false),
        1,
        "lone wrapper counts with all flags false"
    );
}

#[test]
fn list_state_column_matches_flags() {
    let (_d, a) = seed(&[
        "cli-claude-1001",
        "cli-codex-1002",
        "app-codex-2700",
        "wrapper-1003",
    ]);
    let rows = list(&a, 0, true, false, false, false);
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
    let (_d, a) = seed(&[
        "cli-claude-1",
        "cli-codex-2",
        "cli-copilot-3",
        "app-codex-4",
        "app-vscode-copilot-chat-6",
        "wrapper-5",
    ]);
    assert_eq!(
        count(&a, true, true, true, true),
        6,
        "all six prefixes count"
    );
}

#[test]
fn count_on_missing_active_dir_is_zero() {
    // read_entries' read_dir fails on a path that does not exist -> empty Vec, so
    // both the activity-filtered count and the raw total report 0 (no panic).
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("does-not-exist");
    assert!(!missing.exists());
    assert_eq!(
        count(&missing, true, true, true, true),
        0,
        "missing dir -> filtered count 0"
    );
    assert_eq!(count_total(&missing), 0, "missing dir -> total 0");
}

#[test]
fn unknown_prefix_never_counts_but_is_in_total() {
    // prefix gating's `_ => false` arm: an unrecognized name is gated out of the
    // activity-filtered count under EVERY flag-on, yet count_total still sees the
    // raw `*.pid` file (total parses the filename, not the prefix table).
    let (_d, a) = seed(&["unknown-77", "wrapper-1234"]);
    assert_eq!(
        count(&a, true, true, true, true),
        1,
        "unknown prefix gated out under all flags; only wrapper counts"
    );
    assert_eq!(
        count_total(&a),
        2,
        "total still sees the unknown *.pid file"
    );
}

// ── field extraction (parser_test) ──────────────────────────────────────────

#[test]
fn field_extracts_keys() {
    // field(body, key) -> Option<String>. The load-bearing case: start_ts must NOT
    // return the pid (no substring/prefix confusion). Rows carry their own body so
    // the baseline (SleepDisabled) JSON shape is covered alongside the agent one.
    const AGENT: &str = r#"{"pid":1234,"comm":"claude","start_ts":1700000000,"name":"cli-claude"}"#;
    const BASELINE: &str = r#"{"SleepDisabled":1,"captured_at":1700000000}"#;
    let cases: &[(&str, &str, Option<&str>)] = &[
        (AGENT, "pid", Some("1234")),
        (AGENT, "start_ts", Some("1700000000")), // must NOT return the pid
        (AGENT, "name", Some("cli-claude")),
        (r#"{"pid":1234}"#, "nope", None), // missing key
        (BASELINE, "SleepDisabled", Some("1")),
    ];
    for (body, key, want) in cases {
        assert_eq!(
            field(body, key).as_deref(),
            *want,
            "field({body:?}, {key:?})"
        );
    }
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
fn gc_decision_table() {
    // Pure (name, age, alive, on_disk_start, live_start, cpu) -> GcDecision over the
    // constant (stale_age=30, stale_cpu=0.5) thresholds. Priority is
    // dead > pid-reuse > idle; the wrapper is carved out of the idle drop. Each
    // former assertion (incl. the dead-beats-reuse and reuse-beats-idle ordering
    // cases) is one labeled row.
    const STALE_AGE: u32 = 30;
    const STALE_CPU: f64 = 0.5;
    use GcDecision::*;
    #[rustfmt::skip]
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &str, i64, bool, Option<i64>, Option<i64>, Option<f64>, GcDecision)] = &[
        ("dead drops agent",        "cli-claude",    0, false, Some(1), Some(1), Some(0.9), DropDead),
        ("dead drops wrapper",      "wrapper",       0, false, Some(1), Some(1), Some(0.9), DropDead),
        ("reuse drops agent",       "cli-claude",    0, true,  Some(1), Some(2), Some(0.9), DropPidReuse),
        ("reuse drops wrapper",     "wrapper",       0, true,  Some(1), Some(2), Some(0.0), DropPidReuse),
        ("wrapper carved from idle","wrapper",    9999, true,  Some(1), Some(1), Some(0.0), Keep),
        ("idle agent dropped",      "cli-claude", 9999, true,  Some(1), Some(1), Some(0.0), DropIdle),
        ("busy agent kept",         "cli-claude", 9999, true,  Some(1), Some(1), Some(0.9), Keep),
        ("dead beats reuse",        "cli-claude", 9999, false, Some(1), Some(2), Some(0.0), DropDead),
        ("reuse beats idle",        "cli-claude", 9999, true,  Some(1), Some(2), Some(0.0), DropPidReuse),
    ];
    for (label, name, age, alive, on_disk, live, cpu, want) in cases {
        assert_eq!(
            gc_decision(
                name, *age, *alive, *on_disk, *live, *cpu, STALE_AGE, STALE_CPU
            ),
            *want,
            "{label}"
        );
    }
}

#[test]
fn gc_decision_probe_gap_keeps() {
    // The live-probe gap: kill(pid,0) says alive, but sysinfo did NOT see the
    // process this tick, so `live_start` and `cpu` (both derived from the same
    // `proc_.map(...)`) come back None TOGETHER. gc_decision must fall through to
    // Keep — neither the reuse branch (needs both start_ts Some) nor the idle
    // branch (needs cpu Some) can fire on a missing probe, even for an aged agent.
    const STALE_AGE: u32 = 30;
    const STALE_CPU: f64 = 0.5;
    use GcDecision::*;
    #[rustfmt::skip]
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, &str, i64, bool, Option<i64>, Option<i64>, Option<f64>, GcDecision)] = &[
        // cpu None alone short-circuits the idle branch even with a matching start.
        ("none-cpu keeps aged agent",         "cli-claude", 9999, true, Some(1), Some(1), None, Keep),
        // Whole probe gap: live_start AND cpu None -> reuse skipped, idle skipped.
        ("none-live+none-cpu keeps agent",    "cli-claude", 9999, true, Some(1), None,    None, Keep),
    ];
    for (label, name, age, alive, on_disk, live, cpu, want) in cases {
        assert_eq!(
            gc_decision(
                name, *age, *alive, *on_disk, *live, *cpu, STALE_AGE, STALE_CPU
            ),
            *want,
            "{label}"
        );
    }
}
