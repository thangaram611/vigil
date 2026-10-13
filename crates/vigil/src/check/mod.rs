//! Shared read-only collection for status, doctor and debug.
//!
//! Provider activity uses the same collector as the daemon. Diagnostics read
//! the cached VS Code gate and daemon policy, and never issue helper requests.

use std::path::Path;

use crate::activity::scan::{self, Agent};
use crate::activity::vscode::{self, VscodeState};
use crate::activity::{ActivityCollector, ActivitySnapshot};
use crate::config::VigilConfig;
use crate::daemon::tick::{self, TickSnapshot};
use crate::power::assertions::{self, Assertion, AssertionsSummary};
use crate::procscan::{ProcRecord, ProcScanner};

mod json;
#[cfg(test)]
mod tests;

// ── Check / report shapes (§2.3.1) ────────────────────────────────────────────

/// Per-check severity. `Info` is informational (no counter); `Ok` is a passing
/// check; `Warn` reports an unchecked or optional condition; `Error` is a
/// concrete failed check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Ok,
    Warn,
    Error,
}

/// One doctor/status check row.
#[derive(Debug, Clone)]
pub struct Check {
    /// Group header: "dependencies", "privileged helper", "user agent", …
    pub group: &'static str,
    /// Short label: "caffeinate", "LaunchAgent", "lock helper", …
    pub label: String,
    pub severity: Severity,
    /// Human detail: "ok", "missing (run vigil setup)", "ok (mode 700)", …
    pub detail: String,
    /// Whether this check contributes to `install_markers` (LaunchAgent plist,
    /// daemon binary, state dir).
    pub install_marker: bool,
}

/// The aggregate report: the checklist plus the operational snapshot.
#[derive(Debug, Clone)]
pub struct CheckReport {
    pub checks: Vec<Check>,
    /// `count(Severity::Error)`.
    pub errs: u32,
    /// `count(Severity::Warn)`.
    pub warns: u32,
    /// `count(install_marker && severity != Error)`.
    pub install_markers: u32,
    /// The data model backing `--json` + the status text blocks.
    pub snapshot: StatusSnapshot,
}

/// Which groups the engine populates. Both `doctor` and `status` share the
/// engine; the mode selects the breadth. The snapshot is ALWAYS fully populated
/// (status needs every field); `mode` only gates which `Check` rows are built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckMode {
    /// Full doctor checklist (every group).
    Doctor,
    /// `doctor --power` subset.
    Power,
    /// `status` — snapshot only, no doctor checklist rows.
    Status,
}

// ── daemon_scan_state (§2.3.4) ────────────────────────────────────────────────

/// The six daemon-scan states (Contract 1 §5, Contract 4 §1a). EXACT thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonScanState {
    /// Agent not loaded.
    Unloaded,
    /// Loaded but the daemon pidfile is not yet a number (pidfile not written).
    Starting,
    /// Loaded, pid mismatch / non-numeric `updated_at`, pidfile age ≤ missing_after.
    Pending,
    /// Loaded, pid mismatch / non-numeric `updated_at`, pidfile age > missing_after.
    Missing,
    /// Loaded, tick pid == daemon pid, numeric updated_at, age > stale_after.
    Stale,
    /// Loaded, tick pid == daemon pid, numeric updated_at, age ≤ stale_after.
    Fresh,
}

impl DaemonScanState {
    /// The `daemon_scan_state` JSON enum string (byte-identical to bash).
    pub fn as_str(&self) -> &'static str {
        match self {
            DaemonScanState::Unloaded => "unloaded",
            DaemonScanState::Starting => "starting",
            DaemonScanState::Pending => "pending",
            DaemonScanState::Missing => "missing",
            DaemonScanState::Stale => "stale",
            DaemonScanState::Fresh => "fresh",
        }
    }
}

fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Classify the daemon scan state + age (§2.3.4). `now` and the pidfile mtime are
/// supplied so the classifier is pure/testable. Returns `(state, age_secs)`;
/// `age_secs` is `Some` only in the `Stale`/`Fresh` branches (when `updated_at`
/// is numeric and the tick pid matches).
fn classify_scan_state(
    loaded: bool,
    daemon_pid: Option<&str>,
    tick: Option<&TickSnapshot>,
    pidfile_mtime: Option<i64>,
    now: i64,
    cfg_tick_secs: u32,
    wait_secs: u32,
) -> (DaemonScanState, Option<i64>) {
    if !loaded {
        return (DaemonScanState::Unloaded, None);
    }
    // `starting` — loaded but daemon_pid not numeric.
    let daemon_pid = match daemon_pid {
        Some(p) if is_numeric(p) => p,
        _ => return (DaemonScanState::Starting, None),
    };

    let tick_pid = tick.map(|t| t.pid.to_string());

    // pending / missing — tick pid ≠ daemon pid OR updated_at non-numeric.
    let pid_match = tick_pid.as_deref() == Some(daemon_pid);
    if !pid_match {
        // missing iff pidfile age > missing_after = max(10, wait + tick + 3).
        if let Some(mtime) = pidfile_mtime
            && mtime != 0
        {
            let mut pid_age = now - mtime;
            if pid_age < 0 {
                pid_age = 0;
            }
            let missing_after = (wait_secs as i64 + cfg_tick_secs as i64 + 3).max(10);
            if pid_age > missing_after {
                return (DaemonScanState::Missing, None);
            }
        }
        return (DaemonScanState::Pending, None);
    }

    // stale / fresh — pid matches and updated_at numeric.
    let updated_val = tick.unwrap().updated_at;
    let mut age = now - updated_val;
    if age < 0 {
        age = 0;
    }
    let tick_secs = tick
        .map(|t| i64::from(t.tick_secs))
        .unwrap_or(i64::from(cfg_tick_secs));
    let stale_after = tick::stale_after(tick_secs);
    if age > stale_after {
        (DaemonScanState::Stale, Some(age))
    } else {
        (DaemonScanState::Fresh, Some(age))
    }
}

// ── StatusSnapshot (§2.3.2 / §5.1) ────────────────────────────────────────────

/// One provider's session-root view (§5.1 key 10 sub-object).
#[derive(Debug, Clone)]
pub struct ProviderRoot {
    pub home: String,
    pub session_dir: String,
    pub exists: bool,
    pub latest_activity_age_secs: Option<i64>,
}

/// Carries EVERY `--json` value (§5.1) plus the bits the status text blocks need.
/// Built once by [`CheckEngine::run`]; rendered by [`StatusSnapshot::to_json`]
/// (machine schema) or the status text renderer.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    // 1..8
    pub launchd_loaded: bool,
    pub daemon_pid: Option<u32>,
    pub daemon_scan_state: DaemonScanState,
    pub daemon_scan_age_secs: Option<i64>,
    /// Fresh read-only collection, independent of the cached daemon policy.
    pub activity: ActivitySnapshot,
    pub cached_policy: Option<TickSnapshot>,
    pub cached_policy_age_secs: Option<i64>,
    pub cached_policy_identity_matches: bool,
    pub idle_window_minutes: u32,
    // 9 agents
    pub agent_vscode_copilot_chat: &'static str,
    // 10 provider_roots (claude, codex, copilot — order preserved)
    pub provider_claude: ProviderRoot,
    pub provider_codex: ProviderRoot,
    pub provider_copilot: ProviderRoot,
    // 11..18
    pub power_hold_mode: String,
    pub pmset_disablesleep: u8,
    pub baseline: Option<u8>,
    pub caffeinate_pid: Option<u32>,
    pub caffeinate_alive: bool,
    pub thermal: String,
    pub battery: String,
    pub power_helper: HelperInspection,
    // 19..20
    pub power_assertions_state: String,
    pub power_assertions: Vec<Assertion>,

    // ── status-text-only extras (NOT in --json) ──────────────────────────────
    /// Live thermal cut decision (status `expected_hold` uses live should_cut,
    /// NOT the tick file — §5.3).
    pub cut_thermal: bool,
    /// Live battery cut decision.
    pub cut_battery: bool,
    /// Whether a power hold is currently engaged (caffeinate alive identity).
    pub hold_engaged: bool,
    /// The requested manual mode (the mode file, not the tick's applied mode).
    pub mode: crate::mode::Mode,
}

// ── The engine ────────────────────────────────────────────────────────────────

/// Trait abstracting the launchctl `print` load probe so the engine is testable
/// without a real `launchctl`/sudo. Production uses [`RealLoadProbe`].
pub trait LoadProbe {
    /// True iff `launchctl print gui/{uid}/{label}` succeeds (agent loaded).
    fn is_loaded(&self, label: &str) -> bool;
}

/// Production load probe (`launchctl print gui/{uid}/{label}`).
pub struct RealLoadProbe;

impl LoadProbe for RealLoadProbe {
    fn is_loaded(&self, label: &str) -> bool {
        let uid = crate::config::get_uid();
        let target = format!("gui/{uid}/{label}");
        std::process::Command::new("launchctl")
            .args(["print", &target])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Static helper evidence. Presence never implies verified responsiveness.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HelperInspection {
    pub binary_valid: bool,
    pub launchdaemon_valid: bool,
    pub launchd_loaded: bool,
    pub process_pid: Option<u32>,
    pub process_start: Option<i64>,
    pub ipc_permissions_valid: bool,
    pub responsiveness: &'static str,
}

impl HelperInspection {
    pub fn installation_valid(&self) -> bool {
        self.binary_valid && self.launchdaemon_valid && self.ipc_permissions_valid
    }
}

/// The unified check engine.
pub struct CheckEngine;

impl CheckEngine {
    /// Read live launchd state, processes, provider files and `pmset -g`.
    pub fn run(cfg: &VigilConfig, mode: CheckMode, now: i64) -> CheckReport {
        let probe = RealLoadProbe;
        let sleep = crate::power::pmset::MacSleepReader;
        Self::run_with(cfg, mode, now, &probe, &sleep)
    }

    /// Run with injected seams (the testable core). `mode` currently gates only
    /// the doctor checklist breadth; the snapshot is always fully built. The
    /// [`SleepReader`](crate::power::pmset::SleepReader) seam keeps the snapshot
    /// hermetic in tests (production reads live `pmset -g`).
    pub fn run_with<P: LoadProbe, S: crate::power::pmset::SleepReader>(
        cfg: &VigilConfig,
        _mode: CheckMode,
        now: i64,
        probe: &P,
        sleep: &S,
    ) -> CheckReport {
        let snapshot = Self::snapshot(cfg, now, probe, sleep);
        // Doctor renders the installation checklist from this shared snapshot.
        CheckReport {
            checks: Vec::new(),
            errs: 0,
            warns: 0,
            install_markers: 0,
            snapshot,
        }
    }

    /// Classify ONLY the daemon scan state (+age), reading the pidfile + tick
    /// file + the supplied load probe. Lightweight relative to [`Self::run`]
    /// (no helper IPC / pmset reads), so the `start` bounded first-scan wait can
    /// call it in a tight 100ms poll loop (§2.3.5). Pure over `now` for testing.
    pub fn daemon_scan<P: LoadProbe>(
        cfg: &VigilConfig,
        now: i64,
        probe: &P,
    ) -> (DaemonScanState, Option<i64>) {
        let loaded = probe.is_loaded(crate::service::USER_AGENT_LABEL);
        let daemon_pid_raw = std::fs::read_to_string(&cfg.daemon_pidfile).ok();
        let daemon_pid_str = daemon_pid_raw.as_ref().map(|s| s.trim().to_string());
        let tick = tick::read(Path::new(&cfg.daemon_tick_file));
        let pidfile_mtime = file_mtime_secs(Path::new(&cfg.daemon_pidfile));
        classify_scan_state(
            loaded,
            daemon_pid_str.as_deref().filter(|s| is_numeric(s)),
            tick.as_ref(),
            pidfile_mtime,
            now,
            cfg.tick_secs,
            cfg.start_wait_secs,
        )
    }

    /// Assemble fresh activity and separately labeled cached policy read-only.
    fn snapshot<P: LoadProbe, S: crate::power::pmset::SleepReader>(
        cfg: &VigilConfig,
        now: i64,
        probe: &P,
        sleep: &S,
    ) -> StatusSnapshot {
        // 1. launchd_loaded.
        let loaded = probe.is_loaded(crate::service::USER_AGENT_LABEL);

        // 2. daemon_pid (pidfile if ^[0-9]+$).
        let daemon_pid_raw = std::fs::read_to_string(&cfg.daemon_pidfile).ok();
        let daemon_pid_str = daemon_pid_raw.as_ref().map(|s| s.trim().to_string());
        let daemon_pid: Option<u32> = daemon_pid_str
            .as_deref()
            .filter(|s| is_numeric(s))
            .and_then(|s| s.parse().ok());

        // 3/4. daemon_scan_state (+age).
        let tick = tick::read(Path::new(&cfg.daemon_tick_file));
        let pidfile_mtime = file_mtime_secs(Path::new(&cfg.daemon_pidfile));
        let (scan_state, scan_age) = classify_scan_state(
            loaded,
            daemon_pid_str.as_deref().filter(|s| is_numeric(s)),
            tick.as_ref(),
            pidfile_mtime,
            now,
            cfg.tick_secs,
            cfg.start_wait_secs,
        );

        // Per-agent activity (read-only scan; same `now`).
        let claude_dir =
            scan::session_dir_from_provider_home(Path::new(&cfg.claude_home), Agent::Claude);
        let codex_dir =
            scan::session_dir_from_provider_home(Path::new(&cfg.codex_home), Agent::Codex);
        let copilot_dir =
            scan::session_dir_from_provider_home(Path::new(&cfg.copilot_home), Agent::Copilot);

        // A single process snapshot feeds the shared classifier and helper
        // identity view. No GC, provider commands or mutable VS Code scan.
        let mut scanner = ProcScanner::new();
        let records = scanner.collect();
        let (activity, vscode_state) = collect_activity_readonly(cfg, &mut scanner, &records, now);
        let cached_policy_age_secs = tick.as_ref().map(|t| (now - t.updated_at).max(0));
        let installed_exe = Path::new(&cfg.install_dir)
            .join("bin/vigil")
            .canonicalize()
            .ok();
        let caller_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.canonicalize().ok());
        let cached_policy_identity_matches = tick.as_ref().is_some_and(|t| {
            loaded
                && Some(t.pid) == daemon_pid
                && records.iter().any(|r| {
                    r.pid == t.pid
                        && !r.zombie
                        && r.same_user
                        && t.daemon_start.is_some_and(|start| r.start_time == start)
                        && Path::new(&r.exe).canonicalize().ok().is_some_and(|exe| {
                            installed_exe.as_ref() == Some(&exe)
                                || caller_exe.as_ref() == Some(&exe)
                        })
                        && r.cmdline
                            .strip_prefix(&r.exe)
                            .or_else(|| r.cmdline.strip_prefix("vigil"))
                            .is_some_and(|args| args.trim() == "daemon")
                })
        });

        // 8. idle_window_minutes = ceil(idle_after_sec / 60) (bash `(s+59)/60`).
        let idle_window_minutes = cfg.idle_after_sec.div_ceil(60);

        // 10. provider_roots.
        let provider_claude = provider_root(&cfg.claude_home, &claude_dir, Agent::Claude, now);
        let provider_codex = provider_root(&cfg.codex_home, &codex_dir, Agent::Codex, now);
        let provider_copilot = provider_root(&cfg.copilot_home, &copilot_dir, Agent::Copilot, now);

        // 12. pmset_disablesleep (read-only, via the injected reader seam).
        let pmset_disablesleep = sleep.read();

        // 13. baseline (value if file exists).
        let baseline = if Path::new(&cfg.baseline_file).exists() {
            std::fs::read_to_string(&cfg.baseline_file)
                .ok()
                .map(|s| crate::power::baseline_value_from_json(&s))
        } else {
            None
        };

        // 14/15. caffeinate pid + alive.
        let caffeinate_pid = read_caffeinate_pid(&cfg.caffeinate_pidfile);
        let caffeinate_alive = caffeinate_pid.is_some_and(caffeinate_alive_identity);

        // 16/17. thermal/battery summaries (live raw via the fixture seams).
        let therm_raw = crate::thermal::read_therm_raw();
        let therm_reading = crate::thermal::parse_therm(&therm_raw);
        let thermal = crate::thermal::thermal_summary(&therm_reading, cfg.thermal_cpu_limit_floor);
        let cut_thermal = crate::thermal::live_should_cut(&therm_raw, cfg.thermal_cpu_limit_floor);

        let batt_raw = crate::battery::read_ps_raw();
        let batt_reading = crate::battery::parse_ps(&batt_raw);
        let battery = crate::battery::battery_summary(&batt_reading, cfg.battery_floor_pct);
        let cut_battery = crate::battery::live_should_cut(&batt_raw, cfg.battery_floor_pct);

        // Installation and process evidence only. A health request writes IPC
        // files, so responsiveness is deliberately unchecked in diagnostics.
        let power_helper = inspect_helper(cfg, &records);

        // 19/20. power_assertions tri-state.
        let assertions_raw = assertions::read_assertions_raw();
        let summary = assertions::parse_assertions(&assertions_raw, caffeinate_pid);
        let power_assertions_state = summary.state().to_string();
        let power_assertions = match summary {
            AssertionsSummary::Holders(h) => h,
            _ => Vec::new(),
        };

        StatusSnapshot {
            launchd_loaded: loaded,
            daemon_pid,
            daemon_scan_state: scan_state,
            daemon_scan_age_secs: scan_age,
            activity,
            cached_policy: tick,
            cached_policy_age_secs,
            cached_policy_identity_matches,
            idle_window_minutes,
            agent_vscode_copilot_chat: vscode_state,
            provider_claude,
            provider_codex,
            provider_copilot,
            power_hold_mode: "best-effort".to_string(),
            pmset_disablesleep,
            baseline,
            caffeinate_pid,
            caffeinate_alive,
            thermal,
            battery,
            power_helper,
            power_assertions_state,
            power_assertions,
            cut_thermal,
            cut_battery,
            hold_engaged: caffeinate_alive || (baseline.is_some() && pmset_disablesleep == 1),
            mode: crate::mode::read(Path::new(&cfg.mode_file)),
        }
    }
}

// ── read-only helpers ─────────────────────────────────────────────────────────

/// File mtime in unix secs (None if absent/unreadable). Mirrors bash
/// `stat -f %m`.
fn file_mtime_secs(path: &Path) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.mtime())
}

/// READ-ONLY vscode active: parse the existing state file, compare
/// `active_until > now`. Never rescan or rewrite the state file.
fn vscode_active_readonly(state_file: &str, now: i64) -> bool {
    match std::fs::read_to_string(state_file) {
        Ok(text) => VscodeState::parse(&text).active_until > now,
        Err(_) => false,
    }
}

/// Build one provider-root view (read-only).
fn provider_root(home: &str, session_dir: &Path, agent: Agent, now: i64) -> ProviderRoot {
    ProviderRoot {
        home: home.to_string(),
        session_dir: session_dir.to_string_lossy().into_owned(),
        exists: session_dir.is_dir(),
        latest_activity_age_secs: scan::latest_activity_age_secs(session_dir, agent.pattern(), now),
    }
}

/// Read the caffeinate pid from its pidfile (None if absent/non-numeric).
fn read_caffeinate_pid(pidfile: &str) -> Option<u32> {
    let s = std::fs::read_to_string(pidfile).ok()?;
    let t = s.trim();
    if is_numeric(t) { t.parse().ok() } else { None }
}

/// Caffeinate-alive BY IDENTITY (kill(0) + basename == caffeinate, not
/// display-holding). Mirrors bash `vigil_pmset_caffeinate_alive`.
fn caffeinate_alive_identity(pid: u32) -> bool {
    use crate::power::caffeinate::{CaffeinateAssertion, MacCaffeinate};
    MacCaffeinate.is_alive_by_identity(pid)
}

/// Read provider activity and the existing VS Code gate without changing files.
pub(crate) fn collect_activity_readonly(
    cfg: &VigilConfig,
    scanner: &mut ProcScanner,
    records: &[ProcRecord],
    now: i64,
) -> (ActivitySnapshot, &'static str) {
    let mut activity = ActivityCollector::default().collect(cfg, scanner, records, now);
    let host_text = std::env::var_os("VIGIL_VSCODE_PS_FIXTURE")
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            records
                .iter()
                .map(|r| r.cmdline.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
    let host = vscode::host_running(Some(&host_text));
    activity.vscode = host && vscode_active_readonly(&cfg.vscode_copilot_state_file, now);
    let state = if !host {
        "none"
    } else if activity.vscode {
        "active"
    } else {
        "idle"
    };
    (activity, state)
}

fn inspect_helper(cfg: &VigilConfig, records: &[ProcRecord]) -> HelperInspection {
    use std::os::unix::fs::MetadataExt;
    let valid_file = |path: &Path, executable: bool| {
        std::fs::symlink_metadata(path).is_ok_and(|m| {
            m.is_file()
                && m.uid() == 0
                && m.mode() & 0o022 == 0
                && (!executable || m.mode() & 0o111 != 0)
        })
    };
    let valid_dir = |path: &str, uid: u32, mode: u32| {
        std::fs::symlink_metadata(path)
            .is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o777 == mode)
    };
    let identity = records.iter().find(|r| {
        r.exe == cfg.root_helper
            && r.start_time > 0
            && !r.zombie
            && r.cmdline.split_whitespace().any(|arg| arg == "--serve")
    });
    let target = format!("system/{}", crate::service::HELPER_LABEL);
    let loaded = std::process::Command::new("launchctl")
        .args(["print", &target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    HelperInspection {
        binary_valid: valid_file(Path::new(&cfg.root_helper), true),
        launchdaemon_valid: valid_file(
            Path::new(&format!(
                "/Library/LaunchDaemons/{}.plist",
                crate::service::HELPER_LABEL
            )),
            false,
        ),
        launchd_loaded: loaded,
        process_pid: identity.map(|r| r.pid),
        process_start: identity.map(|r| r.start_time),
        ipc_permissions_valid: valid_dir(&cfg.power_request_dir, crate::config::get_uid(), 0o700)
            && valid_dir(&cfg.power_response_dir, 0, 0o755),
        responsiveness: "unchecked",
    }
}
