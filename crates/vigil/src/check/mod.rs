//! `src/check/` — the unified CheckEngine — Phase 5.7 §2.3 + §5.
//!
//! ONE engine produces a [`StatusSnapshot`] (every `vigil status --json` value)
//! plus a `Vec<Check>` checklist, consumed by BOTH `vigil status` (always exit 0)
//! and `vigil doctor` (three-state). This commit builds + tests the engine and
//! its hand-emitted `--json` writer; it is NOT yet wired to any dispatch arm
//! (status/doctor stay shimmed to bash this stage — §6.3 Commit 2).
//!
//! ## Read-only
//! Like [`crate::debug::assemble`], the snapshot scans live state and reads files
//! but MUST NOT write/refresh/GC any pid or state file. The vscode flag is read
//! the same read-only way `debug` reads it (parse the state file, never rescan).
//! The single addition over `debug::assemble` is the daemon round-trip the
//! status path needs: the launchctl `print` load check, the daemon pidfile read,
//! and the tick-file read that drives [`DaemonScanState`].
//!
//! ## `--json` byte-stability (§5)
//! [`StatusSnapshot::to_json`] HAND-EMITS the object in the FROZEN key order
//! (§5.1 table) — it does NOT derive `Serialize` (whose field order/escaping
//! could drift). It prepends the new `"version": 1,` first key (decision Q2); the
//! remaining 21 keys keep the exact order/types/escaping of the captured bash
//! golden, so `to_json()` MINUS the version line equals
//! `tests/golden/status_clean.json` byte-for-byte.

use std::path::Path;

use crate::activity::scan::{self, Agent, AgentState};
use crate::activity::vscode::{self, VscodeState};
use crate::config::VigilConfig;
use crate::ipc::HelperClient;
use crate::power::assertions::{self, Assertion, AssertionsSummary};
use crate::procscan::{AgentKind, AgentMatch, ProcScanner};
use crate::refcount;

mod json;
#[cfg(test)]
mod tests;

// ── Check / report shapes (§2.3.1) ────────────────────────────────────────────

/// Per-check severity. `Info` is informational (no counter); `Ok` is a passing
/// check; `Warn` is the (sole) lock-helper-missing third state; `Error` is any
/// other failed check.
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

/// Parsed daemon tick-file fields the scan classifier needs (§2.1.6 frozen ABI).
#[derive(Debug, Default, Clone)]
struct TickFields {
    pid: Option<String>,
    updated_at: Option<String>,
    tick_secs: Option<String>,
}

/// `awk -F=` first-match parse of the daemon tick file (Contract 1 §5: `=` is the
/// first separator, one field per line). Returns `None` only when the file is
/// absent; individual fields are `None` when the key is missing.
fn read_tick_fields(tick_file: &Path) -> Option<TickFields> {
    let text = std::fs::read_to_string(tick_file).ok()?;
    let mut f = TickFields::default();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let slot = match k {
                "pid" => &mut f.pid,
                "updated_at" => &mut f.updated_at,
                "tick_secs" => &mut f.tick_secs,
                _ => continue,
            };
            // awk first-match-wins: don't clobber an earlier value.
            if slot.is_none() {
                *slot = Some(v.to_string());
            }
        }
    }
    Some(f)
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
    tick: Option<&TickFields>,
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

    let tick_pid = tick.and_then(|t| t.pid.as_deref());
    let updated = tick.and_then(|t| t.updated_at.as_deref());

    // pending / missing — tick pid ≠ daemon pid OR updated_at non-numeric.
    let pid_match = tick_pid == Some(daemon_pid);
    let updated_numeric = updated.is_some_and(is_numeric);
    if !pid_match || !updated_numeric {
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
    let updated_val: i64 = updated.unwrap().parse().unwrap_or(0);
    let mut age = now - updated_val;
    if age < 0 {
        age = 0;
    }
    // tick_secs from the tick file if numeric, else cfg.
    let tick_secs = tick
        .and_then(|t| t.tick_secs.as_deref())
        .filter(|s| is_numeric(s))
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(cfg_tick_secs as i64);
    let stale_after = (tick_secs * 2 + 5).max(15);
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

/// The per-agent tri-state (`active`|`idle`|`none`) shown in the agents
/// sub-object (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriState {
    None,
    Active,
    Idle,
}

impl TriState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriState::None => "none",
            TriState::Active => "active",
            TriState::Idle => "idle",
        }
    }
    fn from_agent_state(s: AgentState) -> Self {
        match s {
            AgentState::None => TriState::None,
            AgentState::Active => TriState::Active,
            AgentState::Idle => TriState::Idle,
        }
    }
}

/// Carries EVERY `--json` value (§5.1) plus the bits the status text blocks need.
/// Built once by [`CheckEngine::run`]; rendered by [`StatusSnapshot::to_json`]
/// (machine schema) or the (future) status text renderer.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    // 1..8
    pub launchd_loaded: bool,
    pub daemon_pid: Option<u32>,
    pub daemon_scan_state: DaemonScanState,
    pub daemon_scan_age_secs: Option<i64>,
    pub refcount_active: u32,
    pub refcount_total: u32,
    pub pending_active_matches: u32,
    pub idle_window_minutes: u32,
    // 9 agents
    pub agent_claude: TriState,
    pub agent_codex: TriState,
    pub agent_copilot: TriState,
    pub agent_vscode_copilot_chat: TriState,
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
    pub power_helper_ok: bool,
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

/// The status/doctor helper liveness probe must stay snappy: it only needs to
/// know whether the privileged helper answers a `status` ping *now*. The helper
/// `--serve` loop polls its request dir every `poll_secs` (default 1s), so a
/// couple of cycles is plenty. We cap the probe FAR below the 10s power-
/// OPERATION timeout (`power_helper_timeout_secs`) — at the full timeout a single
/// dead/slow helper blocks the ENTIRE status/doctor paint (the ~10s blank wait).
/// This cap only affects the diagnostic snapshot; the daemon's real engage/
/// release path builds its own client with the full timeout.
const STATUS_PROBE_TIMEOUT_SECS: u32 = 2;

/// The unified check engine.
pub struct CheckEngine;

impl CheckEngine {
    /// Run with the production seams (real launchctl probe + real helper client +
    /// real `pmset -g` SleepDisabled reader).
    pub fn run(cfg: &VigilConfig, mode: CheckMode, now: i64) -> CheckReport {
        let probe = RealLoadProbe;
        let helper = crate::ipc::MacHelperClient {
            request_dir: std::path::PathBuf::from(&cfg.power_request_dir),
            response_dir: std::path::PathBuf::from(&cfg.power_response_dir),
            // Liveness probe, not a power operation — cap it (see the const docs).
            timeout_secs: cfg.power_helper_timeout_secs.min(STATUS_PROBE_TIMEOUT_SECS),
        };
        let sleep = crate::power::pmset::MacSleepReader;
        Self::run_with(cfg, mode, now, &probe, &helper, &sleep)
    }

    /// Run with injected seams (the testable core). `mode` currently gates only
    /// the doctor checklist breadth; the snapshot is always fully built. The
    /// [`SleepReader`](crate::power::pmset::SleepReader) seam keeps the snapshot
    /// hermetic in tests (production reads live `pmset -g`).
    pub fn run_with<P: LoadProbe, H: HelperClient, S: crate::power::pmset::SleepReader>(
        cfg: &VigilConfig,
        _mode: CheckMode,
        now: i64,
        probe: &P,
        helper: &H,
        sleep: &S,
    ) -> CheckReport {
        let snapshot = Self::snapshot(cfg, now, probe, helper, sleep);
        // The doctor checklist is built by the (future) doctor command on top of
        // this snapshot; Commit 2 lands the snapshot + emitter only. The report
        // exposes an empty checklist with zeroed counters for now so the shape is
        // stable for the doctor command to fill in Commit 4.
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
        let tick = read_tick_fields(Path::new(&cfg.daemon_tick_file));
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

    /// Assemble the full [`StatusSnapshot`] (read-only). Mirrors bash
    /// `cmd_status_json` field-by-field (§5.1) so `to_json` is byte-stable.
    fn snapshot<P: LoadProbe, H: HelperClient, S: crate::power::pmset::SleepReader>(
        cfg: &VigilConfig,
        now: i64,
        probe: &P,
        helper: &H,
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
        let tick = read_tick_fields(Path::new(&cfg.daemon_tick_file));
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

        let claude_state = scan::agent_state(
            &claude_dir,
            Agent::Claude.pattern(),
            cfg.idle_after_sec,
            now,
        );
        let codex_state =
            scan::agent_state(&codex_dir, Agent::Codex.pattern(), cfg.idle_after_sec, now);
        let copilot_state = scan::agent_state(
            &copilot_dir,
            Agent::Copilot.pattern(),
            cfg.idle_after_sec,
            now,
        );

        // vscode: read-only (host probe + state-file parse). `host_running(None)`
        // honors VIGIL_VSCODE_PS_FIXTURE; the active flag is read from the
        // existing state file (active_until > now) WITHOUT rescanning, mirroring
        // `debug::vscode_active_readonly`.
        let vscode_host = vscode::host_running(None);
        let vscode_active =
            vscode_host && vscode_active_readonly(&cfg.vscode_copilot_state_file, now);
        let vscode_tri = if !vscode_host {
            TriState::None
        } else if vscode_active {
            TriState::Active
        } else {
            TriState::Idle
        };

        let claude_active = claude_state == AgentState::Active;
        let codex_active = codex_state == AgentState::Active;
        let copilot_active = copilot_state == AgentState::Active;

        // 5/6. refcount.
        let active_dir = Path::new(&cfg.active_dir);
        let refcount_active = refcount::count(
            active_dir,
            claude_active,
            codex_active,
            copilot_active,
            vscode_active,
        );
        let refcount_total = refcount::count_total(active_dir);

        // 7. pending_active_matches — only when active==0 yet some agent is
        //    active AND launchd is loaded (bash gate).
        let any_agent_active = claude_active || codex_active || copilot_active || vscode_active;
        let pending_active_matches = if refcount_active == 0 && any_agent_active && loaded {
            live_active_match_count(claude_active, codex_active, copilot_active, vscode_active)
        } else {
            0
        };

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

        // 18. power_helper_ok — status round-trip (DirsMissing/timeout → false).
        let power_helper_ok = helper.status().is_ok();

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
            refcount_active,
            refcount_total,
            pending_active_matches,
            idle_window_minutes,
            agent_claude: TriState::from_agent_state(claude_state),
            agent_codex: TriState::from_agent_state(codex_state),
            agent_copilot: TriState::from_agent_state(copilot_state),
            agent_vscode_copilot_chat: vscode_tri,
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
            power_helper_ok,
            power_assertions_state,
            power_assertions,
            cut_thermal,
            cut_battery,
            hold_engaged: caffeinate_alive,
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
/// `active_until > now`. NEVER rescans (mirrors `debug::vscode_active_readonly`).
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

/// Count live detect matches gated by per-agent activity (bash
/// `cmd_live_active_match_count`). Builds its own scanner (this is the live
/// re-scan the status path does ONLY in the pending-match sub-case).
fn live_active_match_count(
    claude_active: bool,
    codex_active: bool,
    copilot_active: bool,
    vscode_active: bool,
) -> u32 {
    let matches: Vec<AgentMatch> = ProcScanner::new().detect();
    let mut count = 0u32;
    for m in matches {
        let inc = match m.kind {
            AgentKind::CliClaude => claude_active,
            AgentKind::CliCodex | AgentKind::AppCodex => codex_active,
            AgentKind::CliCopilot => copilot_active,
            AgentKind::AppVscodeCopilotChat => vscode_active,
        };
        if inc {
            count += 1;
        }
    }
    count
}
