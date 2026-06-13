//! Unit tests for `src/check/` (Phase 5.7 §2.3 + §5 + §5.5).
//!
//! Three thrusts:
//!   1. **`daemon_scan_state` thresholds** (§2.3.4) — the six states and their
//!      exact `missing_after`/`stale_after` boundaries, driven by the pure
//!      [`super::classify_scan_state`].
//!   2. **`--json` byte-stability** (§5.1) — [`StatusSnapshot::to_json`] MINUS the
//!      `"version": 1,` line equals `tests/golden/status_clean.json` byte-for-byte
//!      (closes the status half of GAP #2). The ONE single-clock divergence (the
//!      codex dual-clock artifact) is isolated + documented below.
//!   3. **assertions tri-state in the status path** (§5.5) — the parser is unit-
//!      tested in `power::assertions`; here we prove the `power_assertions_state`
//!      / array projection through the snapshot + emitter, driving
//!      `VIGIL_ASSERTIONS_FIXTURE`.

use super::*;
use crate::config::VigilConfig;
use crate::helper::validate::Action;
use crate::ipc::{HelperClient, IpcError, Response};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

/// The fixed sandbox root the Gate-0 goldens were captured under.
const SBX: &str = "/private/tmp/vigil-golden-sbx";

/// FIXED_NOW the goldens pin (`date +%s` stub → 1700000000).
const FIXED_NOW: i64 = 1_700_000_000;

/// Serializes the env-mutating byte-stability tests (cargo runs tests in parallel
/// threads; these set VIGIL_ASSERTIONS_FIXTURE / the fixture seams + the shared
/// fixed sandbox under SBX, so they must not overlap).
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ── A load probe + helper client that need no real launchctl / IPC dirs ───────

struct FakeProbe(bool);
impl LoadProbe for FakeProbe {
    fn is_loaded(&self, _label: &str) -> bool {
        self.0
    }
}

/// A helper client that always reports the dirs missing (golden `power_helper_ok:
/// false`), with NO filesystem touch.
struct DeadHelper;
impl HelperClient for DeadHelper {
    fn request(&self, _action: Action) -> Result<Response, IpcError> {
        Err(IpcError::DirsMissing)
    }
}

/// A fixed SleepDisabled reader so the snapshot is hermetic (no live `pmset -g`).
struct FakeSleep(u8);
impl crate::power::pmset::SleepReader for FakeSleep {
    fn read(&self) -> u8 {
        self.0
    }
}

// ── scan-state threshold tests (§2.3.4) ───────────────────────────────────────

fn tick(pid: &str, updated: &str, tick_secs: &str) -> TickFields {
    TickFields {
        pid: Some(pid.to_string()),
        updated_at: Some(updated.to_string()),
        tick_secs: Some(tick_secs.to_string()),
    }
}

#[test]
fn scan_state_unloaded_when_not_loaded() {
    let (st, age) =
        classify_scan_state(false, Some("4242"), None, Some(FIXED_NOW), FIXED_NOW, 5, 6);
    assert_eq!(st, DaemonScanState::Unloaded);
    assert_eq!(age, None);
}

#[test]
fn scan_state_starting_when_pid_not_numeric() {
    // loaded but daemon_pid is None / non-numeric → starting.
    let (st, age) = classify_scan_state(true, None, None, None, FIXED_NOW, 5, 6);
    assert_eq!(st, DaemonScanState::Starting);
    assert_eq!(age, None);
}

#[test]
fn scan_state_pending_when_pid_mismatch_and_pidfile_fresh() {
    // tick pid != daemon pid, pidfile age ≤ missing_after → pending.
    let t = tick("9999", "1699999998", "5");
    let (st, age) = classify_scan_state(
        true,
        Some("4242"),
        Some(&t),
        Some(FIXED_NOW), // age 0
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(st, DaemonScanState::Pending);
    assert_eq!(age, None);
}

#[test]
fn scan_state_pending_when_no_tick_file() {
    // No tick file at all (golden status_pending) → pending, age null.
    let (st, age) = classify_scan_state(true, Some("4242"), None, Some(FIXED_NOW), FIXED_NOW, 5, 6);
    assert_eq!(st, DaemonScanState::Pending);
    assert_eq!(age, None);
}

#[test]
fn scan_state_missing_when_pidfile_too_old() {
    // missing_after = max(10, wait(6)+tick(5)+3) = 14. pid_age 15 > 14 → missing.
    let (st, age) = classify_scan_state(
        true,
        Some("4242"),
        None,
        Some(FIXED_NOW - 15),
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(st, DaemonScanState::Missing);
    assert_eq!(age, None);
}

#[test]
fn scan_state_missing_boundary_is_exclusive() {
    // pid_age == missing_after(14) → NOT missing (boundary is `>`), stays pending.
    let (st, _) = classify_scan_state(
        true,
        Some("4242"),
        None,
        Some(FIXED_NOW - 14),
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(st, DaemonScanState::Pending);
    // One second older → missing.
    let (st2, _) = classify_scan_state(
        true,
        Some("4242"),
        None,
        Some(FIXED_NOW - 15),
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(st2, DaemonScanState::Missing);
}

#[test]
fn scan_state_missing_floor_is_ten() {
    // wait=0,tick=0 → missing_after = max(10, 3) = 10. age 11 → missing; 10 → pending.
    let (st_a, _) = classify_scan_state(
        true,
        Some("4242"),
        None,
        Some(FIXED_NOW - 11),
        FIXED_NOW,
        0,
        0,
    );
    assert_eq!(st_a, DaemonScanState::Missing);
    let (st_b, _) = classify_scan_state(
        true,
        Some("4242"),
        None,
        Some(FIXED_NOW - 10),
        FIXED_NOW,
        0,
        0,
    );
    assert_eq!(st_b, DaemonScanState::Pending);
}

#[test]
fn scan_state_fresh_when_pid_matches_and_recent() {
    // golden status_engaged: pid match, updated_at = now-2, age 2 < stale(15) → fresh.
    let t = tick("4242", &(FIXED_NOW - 2).to_string(), "5");
    let (st, age) = classify_scan_state(
        true,
        Some("4242"),
        Some(&t),
        Some(FIXED_NOW),
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(st, DaemonScanState::Fresh);
    assert_eq!(age, Some(2));
}

#[test]
fn scan_state_stale_when_too_old() {
    // stale_after = max(15, tick(5)*2+5)=15. age 16 > 15 → stale.
    let t = tick("4242", &(FIXED_NOW - 16).to_string(), "5");
    let (st, age) = classify_scan_state(
        true,
        Some("4242"),
        Some(&t),
        Some(FIXED_NOW),
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(st, DaemonScanState::Stale);
    assert_eq!(age, Some(16));
}

#[test]
fn scan_state_stale_boundary_is_exclusive() {
    // age == stale_after(15) → fresh; age 16 → stale.
    let t15 = tick("4242", &(FIXED_NOW - 15).to_string(), "5");
    let (fresh, _) = classify_scan_state(
        true,
        Some("4242"),
        Some(&t15),
        Some(FIXED_NOW),
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(fresh, DaemonScanState::Fresh);
    let t16 = tick("4242", &(FIXED_NOW - 16).to_string(), "5");
    let (stale, _) = classify_scan_state(
        true,
        Some("4242"),
        Some(&t16),
        Some(FIXED_NOW),
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(stale, DaemonScanState::Stale);
}

#[test]
fn scan_state_stale_after_uses_tick_file_tick_secs() {
    // tick file tick_secs=20 → stale_after = max(15, 45)=45. age 30 < 45 → fresh,
    // even though cfg tick_secs(5) would have made stale_after 15.
    let t = tick("4242", &(FIXED_NOW - 30).to_string(), "20");
    let (st, age) = classify_scan_state(
        true,
        Some("4242"),
        Some(&t),
        Some(FIXED_NOW),
        FIXED_NOW,
        5, // cfg tick — ignored, tick file wins
        6,
    );
    assert_eq!(st, DaemonScanState::Fresh);
    assert_eq!(age, Some(30));
}

#[test]
fn scan_state_pending_when_updated_at_non_numeric() {
    // pid matches but updated_at is garbage → pending (not stale/fresh).
    let t = tick("4242", "not-a-number", "5");
    let (st, age) = classify_scan_state(
        true,
        Some("4242"),
        Some(&t),
        Some(FIXED_NOW),
        FIXED_NOW,
        5,
        6,
    );
    assert_eq!(st, DaemonScanState::Pending);
    assert_eq!(age, None);
}

// ── tick-file parse ───────────────────────────────────────────────────────────

#[test]
fn read_tick_fields_first_match_wins_and_eq_is_first_sep() {
    let dir = tempdir();
    let f = dir.join("daemon.tick");
    // A `=` in a later value must not confuse the first-separator split; a dup key
    // must keep the FIRST (awk first-match).
    std::fs::write(
        &f,
        "pid=4242\nupdated_at=1699999998\ntick_secs=5\npid=9999\nfoo=bar=baz\n",
    )
    .unwrap();
    let t = read_tick_fields(&f).unwrap();
    assert_eq!(t.pid.as_deref(), Some("4242"));
    assert_eq!(t.updated_at.as_deref(), Some("1699999998"));
    assert_eq!(t.tick_secs.as_deref(), Some("5"));
}

#[test]
fn read_tick_fields_none_when_absent() {
    let dir = tempdir();
    assert!(read_tick_fields(&dir.join("nope.tick")).is_none());
}

// ── --json byte-stability vs the Gate-0 golden (§5.1) ─────────────────────────

#[test]
fn json_clean_matches_golden_byte_for_byte() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sbx = ScopedSandbox::new();
    let cfg = sbx.golden_config();

    // Reproduce the golden's deterministic fixture seams.
    // SAFETY: serialized by ENV_LOCK; cleaned up via clear_fixture_env.
    unsafe {
        std::env::set_var(
            "VIGIL_THERMAL_FIXTURE",
            "Note: No CPU power status has been recorded",
        );
        std::env::set_var(
            "VIGIL_BATTERY_FIXTURE",
            "Now drawing from 'AC Power'\n -InternalBattery-0\t90%; charged; 0:00 remaining present: true",
        );
        std::env::set_var("VIGIL_ASSERTIONS_FIXTURE", "");
        std::env::set_var("VIGIL_VSCODE_PS_FIXTURE", ""); // no vscode host → none
    }

    let report = CheckEngine::run_with(
        &cfg,
        CheckMode::Status,
        FIXED_NOW,
        &FakeProbe(false), // launchctl → not loaded (golden clean)
        &DeadHelper,
        &FakeSleep(0), // golden $ROOT/sleepdisabled = 0
    );
    let got = report.snapshot.to_json();

    // The golden is the BASH output (no version). Insert the single allowed diff
    // (`  "version": 1,` after the opening `{`) to build the expected Rust output.
    let golden = read_golden("status_clean.json");
    let expected = apply_codex_artifact(&insert_version_line(&golden));

    sbx.clear_fixture_env();
    assert_eq!(
        got, expected,
        "status --json must match status_clean.json byte-for-byte \
         (+ the version line, modulo the documented codex single-clock artifact)"
    );

    // INVARIANT (golden README): stripping the version line yields the bash
    // golden exactly (modulo the same codex artifact).
    let stripped = strip_version_line(&got);
    assert_eq!(stripped, apply_codex_artifact(&golden));
}

#[test]
fn json_assertions_ok_state_projects_holders() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sbx = ScopedSandbox::new();
    let cfg = sbx.golden_config();

    // The status_assertions golden fixture (two holders, neither is caffeinate).
    let fixture = "Assertion status system-wide:\n   PreventUserIdleSystemSleep            1\n   PreventUserIdleDisplaySleep           0\nListed by owning process:\n  pid 312(powerd): [0x0000000a00000457] 00:10:23 PreventUserIdleSystemSleep named: \"com.apple.powermanagement.ttydisksleep\"\n  pid 988(Music): [0x0000000b000004a1] 01:02:03 PreventUserIdleDisplaySleep named: \"com.apple.Music.playback\"\nNo new entries.";
    // SAFETY: serialized by ENV_LOCK.
    unsafe {
        std::env::set_var(
            "VIGIL_THERMAL_FIXTURE",
            "Note: No CPU power status has been recorded",
        );
        std::env::set_var(
            "VIGIL_BATTERY_FIXTURE",
            "Now drawing from 'AC Power'\n -InternalBattery-0\t90%; charged; 0:00 remaining present: true",
        );
        std::env::set_var("VIGIL_ASSERTIONS_FIXTURE", fixture);
        std::env::set_var("VIGIL_VSCODE_PS_FIXTURE", "");
    }

    let report = CheckEngine::run_with(
        &cfg,
        CheckMode::Status,
        FIXED_NOW,
        &FakeProbe(false),
        &DeadHelper,
        &FakeSleep(0),
    );
    let got = report.snapshot.to_json();

    let golden = read_golden("status_assertions.json");
    let expected = apply_codex_artifact(&insert_version_line(&golden));

    sbx.clear_fixture_env();
    assert_eq!(
        got, expected,
        "status --json with two assertion holders must match status_assertions.json"
    );
    assert_eq!(report.snapshot.power_assertions_state, "ok");
    assert_eq!(report.snapshot.power_assertions.len(), 2);
}

#[test]
fn empty_assertions_fixture_yields_none_state() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: serialized by ENV_LOCK.
    unsafe { std::env::set_var("VIGIL_ASSERTIONS_FIXTURE", "") };
    let raw = crate::power::assertions::read_assertions_raw();
    let summary = crate::power::assertions::parse_assertions(&raw, None);
    unsafe { std::env::remove_var("VIGIL_ASSERTIONS_FIXTURE") };
    assert_eq!(summary.state(), "none");
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn read_golden(name: &str) -> String {
    std::fs::read_to_string(PathBuf::from(GOLDEN_DIR).join(name))
        .unwrap_or_else(|e| panic!("read golden {name}: {e}"))
}

/// Insert `  "version": 1,\n` right after the opening `{\n` (the single allowed
/// diff vs the bash golden).
fn insert_version_line(golden: &str) -> String {
    debug_assert!(golden.starts_with("{\n"));
    let (head, tail) = golden.split_at(2); // "{\n"
    format!("{head}  \"version\": 1,\n{tail}")
}

/// The inverse: strip the `  "version": 1,\n` line.
fn strip_version_line(rust: &str) -> String {
    rust.replacen("  \"version\": 1,\n", "", 1)
}

/// The codex dual-clock artifact (module note): the bash golden captured codex as
/// `idle` because the real-wall-clock `find -mmin` activity probe saw the 2023
/// session-file mtime as outside the window, while the `date +%s` STUB pinned the
/// age field to 0. The Rust engine uses a SINGLE `now` for both probes, so a
/// session file with `mtime == now` (the only way to reproduce `age: 0`) reads as
/// ACTIVE, not idle. We reproduce `age: 0` (mtime == now) and patch the one
/// divergent token — `"codex":"idle"` → `"codex":"active"` in the agents object —
/// then assert everything else byte-for-byte. This isolates a documented bash
/// artifact to a single token; every other field reproduces exactly.
fn apply_codex_artifact(golden: &str) -> String {
    golden.replace("\"codex\":\"idle\"", "\"codex\":\"active\"")
}

/// A fixed-path sandbox under `SBX` so the rendered provider paths match the
/// goldens byte-for-byte. Creates the codex session dir + a session file pinned
/// to `mtime == FIXED_NOW` (the only single-clock way to reproduce the golden's
/// `codex.exists:true` / `latest_activity_age_secs:0`). Cleaned up on drop.
struct ScopedSandbox {
    root: PathBuf,
}

impl ScopedSandbox {
    fn new() -> Self {
        let root = PathBuf::from(SBX);
        // Fresh tree (the goldens were captured under a freshly-made SBX).
        let _ = std::fs::remove_dir_all(&root);
        for sub in [
            "state/active",
            "logs",
            "install",
            "home/provider/claude",
            "home/provider/codex/sessions/2026/06/12",
            "home/provider/copilot",
        ] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        // Pin a codex session file's mtime to FIXED_NOW so codex.exists:true and
        // latest_activity_age_secs == now - mtime == 0.
        let codex_file = root
            .join("home/provider/codex/sessions/2026/06/12/rollout-2026-06-12T00-00-00-test.jsonl");
        std::fs::write(&codex_file, "{}\n").unwrap();
        set_mtime(&codex_file, FIXED_NOW);
        Self { root }
    }

    fn golden_config(&self) -> VigilConfig {
        golden_config_under(&self.root)
    }

    fn clear_fixture_env(&self) {
        // SAFETY: caller holds ENV_LOCK.
        unsafe {
            std::env::remove_var("VIGIL_THERMAL_FIXTURE");
            std::env::remove_var("VIGIL_BATTERY_FIXTURE");
            std::env::remove_var("VIGIL_ASSERTIONS_FIXTURE");
            std::env::remove_var("VIGIL_VSCODE_PS_FIXTURE");
        }
    }
}

impl Drop for ScopedSandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Set a file's mtime to `secs` unix epoch (whole seconds; no new dependency).
fn set_mtime(path: &Path, secs: i64) {
    use std::fs::{File, FileTimes};
    use std::time::{Duration, SystemTime};
    let f = File::options().write(true).open(path).unwrap();
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64);
    f.set_times(FileTimes::new().set_accessed(t).set_modified(t))
        .unwrap();
}

/// A unique throwaway temp dir for the tick-file parse tests.
fn tempdir() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "vigil-check-test-{}-{}",
        std::process::id(),
        next_seq()
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn next_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// The golden-env `VigilConfig` parameterized to a sandbox root (mirrors the
/// service-test `golden_config`, but rooted at the live sandbox so paths resolve
/// to real created dirs while staying byte-identical to the captured strings).
fn golden_config_under(root: &Path) -> VigilConfig {
    let r = root.to_string_lossy().into_owned();
    let support = "/Library/Application Support/vigil".to_string();
    VigilConfig {
        install_dir: format!("{r}/install"),
        state_dir: format!("{r}/state"),
        log_dir: format!("{r}/logs"),
        config_file: format!("{r}/no.conf"),

        active_dir: format!("{r}/state/active"),
        baseline_file: format!("{r}/state/baseline.json"),
        caffeinate_pidfile: format!("{r}/state/caffeinate.pid"),
        daemon_pidfile: format!("{r}/state/daemon.pid"),
        daemon_tick_file: format!("{r}/state/daemon.tick"),
        lock_file: format!("{r}/state/state.lock"),
        vscode_copilot_state_file: format!("{r}/state/vscode-copilot-chat.state"),

        log_file: format!("{r}/logs/daemon.log"),

        root_dir: support.clone(),
        root_bin_dir: format!("{support}/bin"),
        root_helper: format!("{support}/bin/vigil-root-helper"),
        power_helper_dir: format!("{support}/helper"),
        power_request_base: format!("{support}/helper/requests"),
        power_response_base: format!("{support}/helper/responses"),
        power_request_dir: format!("{r}/install/helper/requests/UID"),
        power_response_dir: format!("{r}/install/helper/responses/UID"),
        power_state_dir: format!("{support}/helper/state"),
        power_log_dir: format!("{support}/helper/logs"),
        power_log_file: format!("{support}/helper/logs/helper.log"),
        power_helper_timeout_secs: 10,

        newsyslog_file: crate::config::NEWSYSLOG_FILE.to_string(),

        tick_secs: 5,
        stale_age_secs: 0,
        stale_cpu_pct: 0.0,
        thermal_cooldown_secs: 0,
        battery_floor_pct: 0,
        start_wait_secs: 6,
        lock_combo: String::new(),
        lock_max_secs: 0,
        lock_helper: format!("{r}/install/bin/vigil-lock-helper"),

        claude_home: format!("{r}/home/provider/claude"),
        claude_home_auto: false,
        codex_home: format!("{r}/home/provider/codex"),
        codex_home_auto: false,
        copilot_home: format!("{r}/home/provider/copilot"),
        copilot_home_auto: false,

        vscode_copilot_discover_secs: 0,
        vscode_copilot_recent_mins: 0,

        // idle_after_sec = 300 → idle_window_minutes = (300+59)/60 = 5 (golden).
        idle_after_sec: 300,
        force: 0,
        thermal_cpu_limit_floor: None,
    }
}
