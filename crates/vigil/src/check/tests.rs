//! Read-only diagnostic classification and version 2 JSON golden checks.

use super::*;
use crate::config::VigilConfig;
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

// Deterministic launchd and power-reading seams.

struct FakeProbe(bool);
impl LoadProbe for FakeProbe {
    fn is_loaded(&self, _label: &str) -> bool {
        self.0
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

fn tick(pid: &str, updated: &str, tick_secs: &str) -> TickSnapshot {
    tick::parse(&format!(
        "pid={pid}\nupdated_at={updated}\ntick_secs={tick_secs}\nrefcount_active=0\ndesired_hold=0\nengaged=0\nthermal_cut=0\nbattery_cut=0\ncooling=0\nmode=auto\n"
    )).unwrap()
}

#[test]
fn classify_scan_state_threshold_table() {
    // One labeled row per former classify_scan_state assertion. Each boundary /
    // floor / stale-exclusive test that asserted TWO points becomes TWO rows, and
    // tick_secs is carried per-row (the tick-file tick_secs=20 case must win over
    // the cfg tick_secs=5). The age column is asserted for every row (None for
    // Unloaded/Starting/Pending/Missing; the computed age for Fresh/Stale).
    use DaemonScanState::{Fresh, Missing, Pending, Stale, Starting, Unloaded};
    let now = FIXED_NOW;
    // (label, loaded, daemon_pid, tick, pidfile_mtime, cfg_tick, wait, want_state, want_age)
    #[rustfmt::skip]
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, bool, Option<&str>, Option<TickSnapshot>, Option<i64>, u32, u32, DaemonScanState, Option<i64>)> = vec![
        ("unloaded", false, Some("4242"), None, Some(now), 5, 6, Unloaded, None),
        ("starting: daemon pid not numeric", true, None, None, None, 5, 6, Starting, None),
        ("pending: tick pid != daemon pid, pidfile fresh", true, Some("4242"), Some(tick("9999", "1699999998", "5")), Some(now), 5, 6, Pending, None),
        ("pending: no tick file", true, Some("4242"), None, Some(now), 5, 6, Pending, None),
        ("missing: pidfile age 15 > missing_after(14)", true, Some("4242"), None, Some(now - 15), 5, 6, Missing, None),
        ("missing boundary: age 14 == missing_after => pending", true, Some("4242"), None, Some(now - 14), 5, 6, Pending, None),
        ("missing boundary: age 15 > 14 => missing", true, Some("4242"), None, Some(now - 15), 5, 6, Missing, None),
        ("missing floor: wait0/tick0 age 11 > 10 => missing", true, Some("4242"), None, Some(now - 11), 0, 0, Missing, None),
        ("missing floor: wait0/tick0 age 10 == 10 => pending", true, Some("4242"), None, Some(now - 10), 0, 0, Pending, None),
        ("fresh: pid match, age 2 < stale_after(15)", true, Some("4242"), Some(tick("4242", &(now - 2).to_string(), "5")), Some(now), 5, 6, Fresh, Some(2)),
        ("stale: age 16 > stale_after(15)", true, Some("4242"), Some(tick("4242", &(now - 16).to_string(), "5")), Some(now), 5, 6, Stale, Some(16)),
        ("stale boundary: age 15 == stale_after => fresh", true, Some("4242"), Some(tick("4242", &(now - 15).to_string(), "5")), Some(now), 5, 6, Fresh, Some(15)),
        ("stale boundary: age 16 > 15 => stale", true, Some("4242"), Some(tick("4242", &(now - 16).to_string(), "5")), Some(now), 5, 6, Stale, Some(16)),
        ("tick-file tick_secs=20 wins: age 30 < 45 => fresh", true, Some("4242"), Some(tick("4242", &(now - 30).to_string(), "20")), Some(now), 5, 6, Fresh, Some(30)),
        ("pending: pid match but updated_at non-numeric", true, Some("4242"), None, Some(now), 5, 6, Pending, None),
    ];
    for (label, loaded, dpid, t, mtime, ct, wait, want_st, want_age) in &cases {
        let (st, age) = classify_scan_state(*loaded, *dpid, t.as_ref(), *mtime, now, *ct, *wait);
        assert_eq!(st, *want_st, "{label}: state");
        assert_eq!(age, *want_age, "{label}: age");
    }
}

#[test]
fn classify_scan_state_edge_cases_table() {
    // Future mtimes clamp to zero; a malformed atomic tick is unavailable.
    use DaemonScanState::{Fresh, Pending};
    let now = FIXED_NOW;
    let future_tick = (now + 100).to_string();
    // (label, loaded, daemon_pid, tick, pidfile_mtime, cfg_tick, wait, want_state, want_age)
    #[rustfmt::skip]
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, bool, Option<&str>, Option<TickSnapshot>, Option<i64>, u32, u32, DaemonScanState, Option<i64>)> = vec![
        ("neg-age clamp: future-dated tick (updated_at=now+100) → age 0, Fresh", true, Some("4242"), Some(tick("4242", &future_tick, "5")), Some(now), 5, 6, Fresh, Some(0)),
        ("neg-age clamp: future-dated pidfile (mtime=now+100), pid mismatch → pid_age 0, Pending", true, Some("4242"), Some(tick("9999", "1699999998", "5")), Some(now + 100), 5, 6, Pending, None),
        ("mtime==0 sentinel: pid mismatch, mtime Some(0) → skip missing check → Pending", true, Some("4242"), Some(tick("9999", "1699999998", "5")), Some(0), 5, 6, Pending, None),
        ("malformed tick is unavailable", true, Some("4242"), None, Some(now), 20, 6, Pending, None),
    ];
    for (label, loaded, dpid, t, mtime, ct, wait, want_st, want_age) in &cases {
        let (st, age) = classify_scan_state(*loaded, *dpid, t.as_ref(), *mtime, now, *ct, *wait);
        assert_eq!(st, *want_st, "{label}: state");
        assert_eq!(age, *want_age, "{label}: age");
    }
}

// The tick parser is shared with daemon, menu and refresh clients.
#[test]
fn malformed_tick_is_not_partially_classified_as_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = golden_config_under(dir.path());
    std::fs::create_dir_all(Path::new(&cfg.state_dir)).unwrap();
    std::fs::write(&cfg.daemon_pidfile, "4242\n").unwrap();
    std::fs::write(
        &cfg.daemon_tick_file,
        format!("pid=4242\nupdated_at={FIXED_NOW}\ntick_secs=5\n"),
    )
    .unwrap();
    assert_eq!(
        CheckEngine::daemon_scan(&cfg, FIXED_NOW, &FakeProbe(true)),
        (DaemonScanState::Pending, None)
    );
}

#[test]
fn readonly_collection_keeps_busy_and_waiting_sessions_separate() {
    // Real, self-bounded local identities with synthetic native state. No
    // provider session is started and no existing session is touched.
    struct BoundedSleep(std::process::Child);
    impl Drop for BoundedSleep {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let cfg = golden_config_under(dir.path());
    let executable = dir.path().join("claude");
    std::fs::copy("/bin/sleep", &executable).unwrap();
    let mut children = Vec::new();
    for _ in 0..2 {
        children.push(BoundedSleep(
            std::process::Command::new(&executable)
                .arg("15")
                .spawn()
                .unwrap(),
        ));
    }
    let mut scanner = ProcScanner::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let records = loop {
        let records = scanner.collect();
        if children.iter().all(|c| {
            records.iter().any(|r| {
                r.pid == c.0.id()
                    && Path::new(&r.exe)
                        .file_name()
                        .is_some_and(|name| name == "claude")
            })
        }) {
            break records;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "disposable hosts must become visible"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let records: Vec<_> = records
        .into_iter()
        .filter(|r| children.iter().any(|c| c.0.id() == r.pid))
        .collect();
    let native = Path::new(&cfg.claude_home).join("sessions");
    std::fs::create_dir_all(&native).unwrap();
    let transcripts = Path::new(&cfg.claude_home).join("projects/-fixture");
    std::fs::create_dir_all(&transcripts).unwrap();
    for (record, (id, state)) in records.iter().zip([("busy", "busy"), ("wait", "waiting")]) {
        let proc_start = chrono::DateTime::from_timestamp(record.start_time, 0)
            .unwrap()
            .format("%a %b %e %H:%M:%S %Y")
            .to_string();
        let value = serde_json::json!({"pid":record.pid,"sessionId":id,"procStart":proc_start,
            "kind":"interactive","status":state,"waitingFor":"input needed","cwd":"/fixture"});
        std::fs::write(
            native.join(format!("{}.json", record.pid)),
            value.to_string(),
        )
        .unwrap();
        std::fs::write(transcripts.join(format!("{id}.jsonl")), "").unwrap();
    }
    let direct = ActivityCollector::default().collect(&cfg, &mut scanner, &records, FIXED_NOW);
    let (diagnostic, _) = collect_activity_readonly(&cfg, &mut scanner, &records, FIXED_NOW);
    assert_eq!(
        (
            diagnostic.working(),
            diagnostic.waiting(),
            diagnostic.uncertain()
        ),
        (1, 1, 0)
    );
    assert_eq!(
        serde_json::to_value(direct.sessions).unwrap(),
        serde_json::to_value(diagnostic.sessions).unwrap(),
        "same reducer, same evidence"
    );
}

// ── --json byte-stability vs the Gate-0 golden (§5.1) ─────────────────────────

/// Set the deterministic golden status fixtures shared by the byte-for-byte
/// `--json` tests: the constant thermal/battery/vscode seams plus the per-test
/// ASSERTIONS fixture (the sole thing those two tests vary).
/// SAFETY: callers hold ENV_LOCK and clean up via `ScopedSandbox::clear_fixture_env`.
unsafe fn set_golden_fixture_env(assertions: &str) {
    unsafe {
        std::env::set_var(
            "VIGIL_THERMAL_FIXTURE",
            "Note: No CPU power status has been recorded",
        );
        std::env::set_var(
            "VIGIL_BATTERY_FIXTURE",
            "Now drawing from 'AC Power'\n -InternalBattery-0\t90%; charged; 0:00 remaining present: true",
        );
        std::env::set_var("VIGIL_ASSERTIONS_FIXTURE", assertions);
        std::env::set_var("VIGIL_VSCODE_PS_FIXTURE", ""); // no vscode host → none
    }
}

#[test]
fn json_clean_matches_golden_byte_for_byte() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sbx = ScopedSandbox::new();
    let cfg = sbx.golden_config();

    // Reproduce the golden's deterministic fixture seams (clean: no assertions).
    // SAFETY: serialized by ENV_LOCK; cleaned up via clear_fixture_env.
    unsafe { set_golden_fixture_env("") };

    let mut report = CheckEngine::run_with(
        &cfg,
        CheckMode::Status,
        FIXED_NOW,
        &FakeProbe(false), // launchctl → not loaded (golden clean)
        &FakeSleep(0),     // golden $ROOT/sleepdisabled = 0
    );
    report.snapshot.power_helper = HelperInspection {
        binary_valid: false,
        launchdaemon_valid: false,
        launchd_loaded: false,
        process_pid: None,
        process_start: None,
        ipc_permissions_valid: false,
        responsiveness: "unchecked",
    };
    // The fixture has no observed host: a recent file is uncertain fallback,
    // never an exact active session. Ignore unrelated real hosts in this golden.
    report.snapshot.activity = ActivitySnapshot {
        uncertain_providers: vec!["codex".into()],
        ..ActivitySnapshot::default()
    };
    let got = report.snapshot.to_json();

    let expected = read_golden("status_clean.json");
    sbx.clear_fixture_env();
    assert_eq!(got, expected, "status --json version 2 golden");
}

#[test]
fn json_assertions_ok_state_projects_holders() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sbx = ScopedSandbox::new();
    let cfg = sbx.golden_config();

    // The status_assertions golden fixture (two holders, neither is caffeinate).
    let fixture = "Assertion status system-wide:\n   PreventUserIdleSystemSleep            1\n   PreventUserIdleDisplaySleep           0\nListed by owning process:\n  pid 312(powerd): [0x0000000a00000457] 00:10:23 PreventUserIdleSystemSleep named: \"com.apple.powermanagement.ttydisksleep\"\n  pid 988(Music): [0x0000000b000004a1] 01:02:03 PreventUserIdleDisplaySleep named: \"com.apple.Music.playback\"\nNo new entries.";
    // SAFETY: serialized by ENV_LOCK.
    unsafe { set_golden_fixture_env(fixture) };

    let mut report = CheckEngine::run_with(
        &cfg,
        CheckMode::Status,
        FIXED_NOW,
        &FakeProbe(false),
        &FakeSleep(0),
    );
    report.snapshot.power_helper = HelperInspection {
        binary_valid: false,
        launchdaemon_valid: false,
        launchd_loaded: false,
        process_pid: None,
        process_start: None,
        ipc_permissions_valid: false,
        responsiveness: "unchecked",
    };
    // The fixture has no observed host: a recent file is uncertain fallback,
    // never an exact active session. Ignore unrelated real hosts in this golden.
    report.snapshot.activity = ActivitySnapshot {
        uncertain_providers: vec!["codex".into()],
        ..ActivitySnapshot::default()
    };
    let got = report.snapshot.to_json();

    let golden = read_golden("status_assertions.json");
    let expected = golden;

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
        mode_file: format!("{r}/state/mode"),
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
        thermal_cpu_limit_floor: None,
    }
}
