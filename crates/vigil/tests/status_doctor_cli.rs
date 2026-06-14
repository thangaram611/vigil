//! End-to-end CLI tests for the native `vigil status` / `vigil doctor` commands
//! (Phase 5.7 Commit 4) driving the built binary via CARGO_BIN_EXE_vigil in a
//! hermetic sandbox.
//!
//! The byte-exact `--json` schema is proven at the snapshot level in
//! `src/check/tests.rs` against the Gate-0 golden (with `FakeSleep`/`FakeProbe`
//! seams). Driving the LIVE binary cannot pin the real `/usr/bin/pmset`
//! SleepDisabled read or the real-wall-clock `find -mmin` activity age, so these
//! tests assert the COMMAND-path wiring: the version line is present and first,
//! the schema is valid + complete, the deterministic keys match the golden, the
//! text blocks carry the machine-relevant strings, and the doctor three-state
//! exit codes are correct.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build a `Command` for the binary under test with a clean, deterministic env.
fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vigil"))
}

/// A throwaway sandbox dir tree mirroring the Gate-0 fixtures' shape (codex
/// session present + pinned mtime so `exists:true`).
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "vigil-sd-cli-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let codex_sessions = root.join("home/provider/codex/sessions/2026/06/12");
        std::fs::create_dir_all(&codex_sessions).unwrap();
        std::fs::create_dir_all(root.join("state/active")).unwrap();
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let codex_file = codex_sessions.join("rollout-2026-06-12T00-00-00-test.jsonl");
        std::fs::write(&codex_file, "").unwrap();
        // Pin the codex session mtime well into the past so the real-wall-clock
        // `find -mmin` activity probe reads it as IDLE (not active) — matching the
        // Gate-0 golden capture (which used a 2023 mtime for the same reason).
        pin_old_mtime(&codex_file);

        // A PATH-prepended stub dir with a `launchctl` that always fails (= "not
        // loaded"), so launchd / scan / root-helper readings are deterministic on
        // any host (the live binary resolves `launchctl` via PATH). pmset /
        // caffeinate still resolve from the real PATH appended after it.
        let stub_bin = root.join("stub-bin");
        std::fs::create_dir_all(&stub_bin).unwrap();
        write_exec(&stub_bin.join("launchctl"), "#!/bin/sh\nexit 1\n");

        Sandbox { root }
    }

    /// The common deterministic env shared by every status capture. Pins the
    /// fixture seams so thermal/battery/assertions/vscode are reproducible and
    /// prepends the launchctl-failing stub dir to PATH; the live `/usr/bin/pmset`
    /// SleepDisabled read remains machine-dependent (asserted as a 0|1 wildcard).
    fn apply_env(&self, cmd: &mut Command) {
        let home = self.root.join("home");
        let real_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{}", self.root.join("stub-bin").display(), real_path);
        cmd.env("PATH", path)
            .env("HOME", &home)
            .env("VIGIL_INSTALL_DIR", self.root.join("install"))
            // Point the privileged root tree at an absent sandbox path so the
            // helper IPC dirs are missing on ANY host → `power_helper_ok=false`
            // (the real `/Library/Application Support/vigil` may exist on a dev
            // box where vigil is actually installed). status/doctor never call
            // `validate_security_paths`, so this override is safe here.
            .env("VIGIL_ROOT_DIR", self.root.join("root"))
            .env("VIGIL_STATE_DIR", self.root.join("state"))
            .env("VIGIL_LOG_DIR", self.root.join("logs"))
            .env("VIGIL_CONFIG_FILE", self.root.join("no.conf"))
            .env("VIGIL_CLAUDE_HOME", home.join("provider/claude"))
            .env("VIGIL_CODEX_HOME", home.join("provider/codex"))
            .env("VIGIL_COPILOT_HOME", home.join("provider/copilot"))
            .env(
                "VIGIL_THERMAL_FIXTURE",
                "Note: No CPU power status has been recorded",
            )
            .env(
                "VIGIL_BATTERY_FIXTURE",
                "Now drawing from 'AC Power'\n -InternalBattery-0\t90%; charged; 0:00 remaining present: true",
            )
            .env("VIGIL_ASSERTIONS_FIXTURE", "")
            .env("VIGIL_VSCODE_PS_FIXTURE", "");
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Set a file's mtime far in the past (via `touch -t`) so `find -mmin +N` reads it
/// as idle. Uses the system `touch` to stay dependency-free in the test crate.
fn pin_old_mtime(path: &Path) {
    let _ = Command::new("touch")
        .arg("-t")
        .arg("202001010000")
        .arg(path)
        .status();
}

/// Write an executable shell stub.
fn write_exec(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    let mut perm = std::fs::metadata(path).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(path, perm).unwrap();
}

fn run(args: &[&str], sbx: &Sandbox) -> (i32, String, String) {
    let mut cmd = bin();
    sbx.apply_env(&mut cmd);
    cmd.args(args.iter().map(OsString::from));
    let out = cmd.output().expect("spawn vigil");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A tiny flat-JSON reader: returns the top-level `"key": value` strings (value
/// verbatim up to the line-ending comma). Sufficient for the flat status schema
/// (sub-objects are returned as their raw `{...}` / `[...]` text).
fn flat_json(s: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in s.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('"')
            && let Some((k, v)) = rest.split_once("\": ")
        {
            let v = v.trim_end_matches(',').to_string();
            m.insert(k.to_string(), v);
        }
    }
    m
}

// ── status --json ─────────────────────────────────────────────────────────────

#[test]
fn status_json_version_is_first_key_and_one() {
    let sbx = Sandbox::new("jsonver");
    let (code, out, _err) = run(&["status", "--json"], &sbx);
    assert_eq!(code, 0, "status --json always exits 0");
    // The opening `{` then `"version": 1,` as the FIRST key.
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("{"));
    assert_eq!(
        lines.next(),
        Some("  \"version\": 1,"),
        "version must be the first key with value 1"
    );
}

#[test]
fn status_json_schema_is_complete_and_deterministic() {
    let sbx = Sandbox::new("jsonschema");
    let (code, out, _err) = run(&["status", "--json"], &sbx);
    assert_eq!(code, 0);
    let m = flat_json(&out);

    // Every frozen key (+ version) is present.
    for key in [
        "version",
        "launchd_loaded",
        "daemon_pid",
        "daemon_scan_state",
        "daemon_scan_age_secs",
        "refcount_active",
        "refcount_total",
        "pending_active_matches",
        "idle_window_minutes",
        "agents",
        "provider_roots",
        "power_hold_mode",
        "pmset_disablesleep",
        "baseline",
        "caffeinate_pid",
        "caffeinate_alive",
        "thermal",
        "battery",
        "power_helper_ok",
        "power_assertions_state",
        "power_assertions",
    ] {
        assert!(m.contains_key(key), "missing --json key: {key}");
    }

    // The deterministic (fixture-pinned / clean-state) values.
    assert_eq!(m["version"], "1");
    assert_eq!(m["launchd_loaded"], "false");
    assert_eq!(m["daemon_pid"], "null");
    assert_eq!(m["daemon_scan_state"], "\"unloaded\"");
    assert_eq!(m["refcount_active"], "0");
    assert_eq!(m["refcount_total"], "0");
    assert_eq!(m["pending_active_matches"], "0");
    assert_eq!(m["idle_window_minutes"], "5");
    assert_eq!(m["power_hold_mode"], "\"best-effort\"");
    assert_eq!(m["thermal"], "\"ok\"");
    assert_eq!(m["battery"], "\"AC 90%\"");
    assert_eq!(m["power_helper_ok"], "false");
    assert_eq!(m["power_assertions_state"], "\"none\"");
    assert_eq!(m["power_assertions"], "[]");
    // pmset_disablesleep is a live `/usr/bin/pmset` read (machine-dependent) →
    // assert only that it is the unquoted 0|1 the schema requires.
    assert!(
        m["pmset_disablesleep"] == "0" || m["pmset_disablesleep"] == "1",
        "pmset_disablesleep must be unquoted 0|1, got {}",
        m["pmset_disablesleep"]
    );
    // codex provider session dir exists (pinned fixture); agents object closed enum.
    assert!(out.contains("\"exists\":true"), "codex session dir exists");
    assert!(out.contains("\"vscode_copilot_chat\":\"none\""));
}

#[test]
fn status_json_is_valid_json() {
    let sbx = Sandbox::new("jsonvalid");
    let (_code, out, _err) = run(&["status", "--json"], &sbx);
    serde_json::from_str::<serde_json::Value>(&out).expect("status --json must be parseable JSON");
}

// ── status text ───────────────────────────────────────────────────────────────

#[test]
fn status_text_blocks_and_hint() {
    let sbx = Sandbox::new("text");
    let (code, out, _err) = run(&["status"], &sbx);
    assert_eq!(code, 0);
    assert!(out.starts_with("vigil status\n"));
    for needle in [
        "  service",
        "    launchd:      no",
        "    scan:          not running",
        "    root helper:   not loaded",
        "  activity",
        "    refcount:      0 active / 0 total (idle window 5m)",
        "  power",
        "    thermal:       ok",
        "    battery:       AC 90%",
        "    assertions:    none",
        "  detail: use 'vigil status --verbose' for provider paths and assertion rows",
    ] {
        assert!(
            out.contains(needle),
            "status text missing: {needle:?}\n{out}"
        );
    }
    assert!(
        !out.contains("provider roots:"),
        "default hides provider roots"
    );
}

#[test]
fn status_verbose_adds_provider_roots_and_assertions() {
    let sbx = Sandbox::new("verbose");
    let (code, out, _err) = run(&["status", "--verbose"], &sbx);
    assert_eq!(code, 0);
    assert!(out.contains("  provider roots:"));
    assert!(
        out.contains("exists=yes state=idle"),
        "codex provider state"
    );
    assert!(out.contains("  power assertions:"));
    assert!(out.contains("    (none)"));
}

#[test]
fn usage_error_exits_one_with_subcommand_usage() {
    // A bad flag on status/doctor is a usage violation => exit 1 (NOT 64) with that
    // subcommand's own usage line on stderr. Each row keeps its own Sandbox.
    let cases: &[(&str, &str, &str)] = &[
        ("usage", "status", "usage: vigil status [--json|--verbose]"),
        (
            "docusage",
            "doctor",
            "usage: vigil doctor [--power] [--verbose]",
        ),
    ];
    for &(label, sub, usage) in cases {
        let sbx = Sandbox::new(label);
        let (code, _out, err) = run(&[sub, "--bogus"], &sbx);
        assert_eq!(code, 1, "{sub} usage violation exits 1, not 64");
        assert!(err.contains(usage), "{sub} stderr should contain {usage:?}");
    }
}

// ── doctor ────────────────────────────────────────────────────────────────────

#[test]
fn doctor_not_installed_exits_one() {
    let sbx = Sandbox::new("docfresh");
    // Remove the state dir so no install_markers exist → not installed.
    std::fs::remove_dir_all(sbx.state_dir()).unwrap();
    let (code, out, _err) = run(&["doctor"], &sbx);
    assert_eq!(code, 1, "not-installed doctor exits 1");
    for needle in [
        "vigil doctor",
        "  platform",
        "  dependencies",
        "  privileged helper",
        "  user agent",
        "  providers",
        "state:  not installed",
        "result: setup required",
        "next:   vigil setup",
        "use 'vigil doctor --verbose'",
    ] {
        assert!(out.contains(needle), "doctor missing: {needle:?}\n{out}");
    }
    assert!(
        !out.contains("provider roots:"),
        "default doctor hides provider roots"
    );
}

#[test]
fn doctor_partial_install_needs_repair() {
    let sbx = Sandbox::new("docpartial");
    // state dir EXISTS (Sandbox::new created it) → one install marker → needs repair.
    assert!(Path::new(&sbx.state_dir()).is_dir());
    let (code, out, _err) = run(&["doctor"], &sbx);
    assert_eq!(code, 1, "needs-repair doctor exits 1");
    assert!(out.contains("state:  needs repair"), "{out}");
    assert!(out.contains("next:   vigil setup"));
}

#[test]
fn doctor_verbose_shows_paths_and_provider_roots() {
    let sbx = Sandbox::new("docverbose");
    std::fs::remove_dir_all(sbx.state_dir()).unwrap();
    let (code, out, _err) = run(&["doctor", "--verbose"], &sbx);
    assert_eq!(code, 1);
    assert!(out.contains("  paths"));
    assert!(out.contains("LaunchAgent:"));
    assert!(out.contains("  provider roots:"));
    assert!(
        out.contains("state=idle"),
        "codex provider state in doctor verbose"
    );
}

#[test]
fn doctor_power_nonzero_when_helper_unavailable() {
    let sbx = Sandbox::new("docpower");
    let (code, out, _err) = run(&["doctor", "--power"], &sbx);
    assert_eq!(code, 1, "power doctor fails when IPC unavailable");
    for needle in [
        "vigil power doctor",
        "  power hold mode:    best-effort",
        "  display sleep:      allowed",
        "  root helper:        FAIL",
        "result: 1 power path check(s) failed",
        "next:   vigil setup",
    ] {
        assert!(
            out.contains(needle),
            "power doctor missing: {needle:?}\n{out}"
        );
    }
}
