//! Drives the built `vigil` binary's `debug` subcommand:
//!   - `debug --json` produces valid JSON with agents/processes/refcount keys.
//!   - `debug` (default + --json) is verifiably READ-ONLY: a seeded active-dir
//!     pidfile is byte- and mtime-identical after the run.
//!   - `debug detect --ps-comm --ps-cmd` is the pure oracle (cross-checked
//!     against `procscan::detect_all_text` in-process).

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn vigil_bin() -> PathBuf {
    // The integration test binary lives in target/debug/deps; the built vigil
    // binary is target/debug/vigil. CARGO_BIN_EXE_vigil is set by cargo for the
    // binary target.
    PathBuf::from(env!("CARGO_BIN_EXE_vigil"))
}

/// Run `vigil debug ...` with an isolated HOME + state dir. `args` are the
/// trailing args after `debug`. Returns (stdout, status_success).
fn run_debug(home: &Path, state_dir: &Path, args: &[&str]) -> (String, bool) {
    // Use an empty conf file so config::load takes pure defaults under HOME, and
    // VIGIL_INSTALL_DIR to point state under our temp dir (active_dir derives
    // from state_dir = install_dir/state).
    let conf = home.join("vigil.conf");
    std::fs::write(&conf, "").unwrap();
    let out = Command::new(vigil_bin())
        .arg("debug")
        .args(args)
        .env("HOME", home)
        .env("VIGIL_CONFIG_FILE", &conf)
        .env("VIGIL_STATE_DIR", state_dir)
        .env("VIGIL_VSCODE_PS_FIXTURE", "") // no vscode host
        .output()
        .expect("run vigil debug");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    )
}

#[test]
fn debug_json_has_expected_keys() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let (stdout, ok) = run_debug(home.path(), state.path(), &["--json"]);
    assert!(ok, "vigil debug --json should exit 0");
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("debug --json must be valid JSON: {e}\n{stdout}"));
    assert!(v.get("now").is_some(), "json has now");
    assert!(v.get("agents").is_some(), "json has agents");
    assert!(v.get("processes").is_some(), "json has processes");
    assert!(v.get("refcount").is_some(), "json has refcount");
    let rc = v.get("refcount").unwrap();
    assert!(rc.get("total").is_some());
    assert!(rc.get("filtered").is_some());
    assert!(rc.get("by_prefix").is_some());
}

#[test]
fn debug_is_read_only_for_active_dir() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    // active_dir = state_dir/active.
    let active = state.path().join("active");
    std::fs::create_dir_all(&active).unwrap();
    let pidfile = active.join("cli-claude-4242.pid");
    let body =
        "{\"pid\":4242,\"comm\":\"claude\",\"start_ts\":1700000000,\"name\":\"cli-claude\"}\n";
    std::fs::write(&pidfile, body).unwrap();
    let before_meta = std::fs::metadata(&pidfile).unwrap();
    let before_mtime = before_meta.modified().unwrap();

    // Run both the default dump and the JSON dump.
    let (_, ok1) = run_debug(home.path(), state.path(), &[]);
    let (_, ok2) = run_debug(home.path(), state.path(), &["--json"]);
    assert!(ok1 && ok2, "debug runs should succeed");

    // Pidfile must be byte- and mtime-identical (read-only invariant).
    let after_body = std::fs::read_to_string(&pidfile).expect("pidfile must still exist");
    assert_eq!(after_body, body, "debug must NOT modify pidfile content");
    let after_mtime = std::fs::metadata(&pidfile).unwrap().modified().unwrap();
    assert_eq!(
        before_mtime, after_mtime,
        "debug must NOT touch pidfile mtime"
    );
    // And no new files appeared in active_dir.
    let count = std::fs::read_dir(&active).unwrap().count();
    assert_eq!(count, 1, "debug must NOT create new pid files");
}

#[test]
fn debug_detect_oracle_matches_in_process() {
    // The CLI oracle output should equal the in-process pure detect over the
    // same two fixtures.
    let comm = repo_root().join("tests/fixtures/ps-axww-comm-snapshot.txt");
    let cmd = repo_root().join("tests/fixtures/ps-axww-snapshot.txt");
    let out = Command::new(vigil_bin())
        .args(["debug", "detect", "--ps-comm"])
        .arg(&comm)
        .arg("--ps-cmd")
        .arg(&cmd)
        .output()
        .expect("run debug detect");
    assert!(out.status.success());
    let mut cli_lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect();
    cli_lines.sort();

    let comm_text = std::fs::read_to_string(&comm).unwrap();
    let cmd_text = std::fs::read_to_string(&cmd).unwrap();
    let mut in_proc: Vec<String> = vigil::procscan::detect_all_text(&comm_text, &cmd_text)
        .iter()
        .map(vigil::procscan::agent_match_tsv)
        .collect();
    in_proc.sort();

    assert_eq!(
        cli_lines, in_proc,
        "CLI oracle must equal in-process detect"
    );
}
