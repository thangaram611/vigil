//! The resident tick loop (§2.1 + §3.1). Run by `launchd` via the LaunchAgent
//! plist as `vigil daemon` (the hidden subcommand).
//!
//! Owns ONE long-lived [`ProcScanner`] (the single `sysinfo::System` for
//! detect/start-time) plus a SECOND bare `sysinfo::System` for the gc cpu probe
//! (Q9: two Systems, simplest correct). Power hold/release goes through the
//! [`PowerController`](crate::power::platform::PowerController) facade, which
//! delegates to the macOS `pmset` + `caffeinate` state machine.
//!
//! Per-tick order (§2.1.3): detect+touch → gc → per-agent activity → activity-
//! filtered count → manual-mode demand → cutoff checks + cooldown re-arm →
//! decide → act → closed-lid follow-ups → write tick (POST-action engaged) →
//! sleep.

pub mod grace;
pub mod lock;
pub mod refresh;
pub mod tick;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::activity::scan::Agent;
use crate::activity::vscode;
use crate::activity::{ActivityCollector, ActivitySnapshot};
use crate::config::{self, VigilConfig};
use crate::mode::{self, Mode};
use crate::power::lid::{self, Lid, MacLid};
use crate::power::platform::{MacPowerController, PowerController};
use crate::procscan::ProcScanner;
use crate::refcount;
use crate::{battery, thermal};

use lock::{DaemonLock, LockOutcome};
use tick::TickSnapshot;

/// Current epoch seconds — the daemon's single clock (bash `vigil_now_unix`).
fn now_unix() -> i64 {
    chrono::Local::now().timestamp()
}

/// The four per-tick activity flags, computed once per tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct ActivityFlags {
    pub claude: bool,
    pub codex: bool,
    pub copilot: bool,
    pub vscode: bool,
}

/// The desired-hold predicate: hold iff there is demand AND no thermal cut AND
/// no battery cut AND not in the post-thermal cooldown window. `demand` is the
/// active-agent count after the manual mode ([`Mode::demand`]), so a mode can
/// override detection but never a safety cut.
pub fn desired_hold(demand: u32, cut_thermal: bool, cut_battery: bool, cooling: bool) -> bool {
    demand > 0 && !cut_thermal && !cut_battery && !cooling
}

/// The act-branch result: the new `engaged` value after acting on `(desired,
/// engaged)`. PURE over the seam side effects: the machine mutates power state,
/// this returns the bookkeeping bool the tick file records.
///
/// Release-reason priority is LOAD-BEARING (`bin/vigil-daemon:161-186`):
/// **thermal → SOFT (keep baseline) > battery → FULL > count==0 → FULL.**
#[allow(clippy::too_many_arguments)]
pub fn act(
    power: &mut dyn PowerController,
    desired: bool,
    engaged: bool,
    demand: u32,
    cut_thermal: bool,
    cut_battery: bool,
    cooldown_secs: u32,
    battery_summary: &str,
    flags: ActivityFlags,
    now: i64,
) -> bool {
    match (desired, engaged) {
        (true, false) => {
            let cs = if flags.claude { "active" } else { "idle" };
            let xs = if flags.codex { "active" } else { "idle" };
            let ps = if flags.copilot { "active" } else { "idle" };
            let vs = if flags.vscode { "active" } else { "idle" };
            tracing::info!(
                "engage — demand={demand} thermal=ok battery=ok claude={cs} codex={xs} copilot={ps} vscode_copilot_chat={vs}"
            );
            // engaged := true ONLY if the helper engage succeeds (bash:
            // `if vigil_pmset_engage; then DAEMON_ENGAGED=1; fi`).
            match power.engage(now) {
                Ok(()) => true,
                Err(e) => {
                    // The IPC round-trip failed, but the privileged helper may
                    // have ALREADY applied `pmset disablesleep 1` before the
                    // client could read the response (e.g. a response-dir perms
                    // problem, or a round-trip that ran past the client timeout).
                    // Trust the OBSERVABLE platform state over the IPC verdict:
                    // if the hold landed, adopt it rather than leak an untracked
                    // platform hold that the idle release path would otherwise
                    // never undo.
                    if power.observable_engaged() {
                        tracing::warn!(
                            "engage failed ({e}) but the platform hold is observable — adopting"
                        );
                        if let Err(se) = power.adopt_after_failed_engage() {
                            tracing::warn!("adopt failed: {se}");
                        }
                        true
                    } else {
                        tracing::error!("engage failed: {e}");
                        false
                    }
                }
            }
        }
        (true, true) => {
            // Reassert drift; engaged stays true unless reconcile errors.
            match power.reconcile_engaged() {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!("reconcile failed: {e}");
                    false
                }
            }
        }
        (false, true) => {
            // Release sub-branch — priority order is load-bearing.
            if cut_thermal {
                tracing::warn!("release — thermal cutoff (cooldown {cooldown_secs}s)");
                power.soft_release();
            } else if cut_battery {
                tracing::warn!("release — battery floor ({battery_summary})");
                power.full_release();
            } else if demand == 0 {
                tracing::info!("release — no demand");
                power.full_release();
            }
            // engaged := false ALWAYS, even if a release no-ops.
            false
        }
        (false, false) => {
            // Partial-engage reconcile (defense in depth). We are idle and not
            // tracking a hold, yet `SleepDisabled` is observably 1 while our
            // recorded baseline was 0 — the signature of a prior engage that the
            // privileged layer applied but the client never confirmed (so
            // `engaged` was never set, and the normal `(false,true)` release path
            // never runs). Release to restore the original state. Idempotent and
            // safe: the helper no-ops a release when it is not actually engaged
            // and never clobbers an externally-set value, and full_release clears
            // the stale baseline so this fires at most once per orphan.
            //
            // Gated on `baseline_value()==0` so a legitimately-kept soft-release
            // baseline of 1 — where SleepDisabled=1 is the CORRECT restored state,
            // not a leak — is left untouched.
            if power.orphaned_hold_requires_release() {
                tracing::warn!("reconciling orphaned platform sleep hold while idle — releasing");
                power.full_release();
            }
            engaged
        }
    }
}

/// The resident daemon (§2.1.2). Owns the long-lived scanner + the power seams.
pub struct Daemon {
    cfg: VigilConfig,
    /// THE one long-lived `sysinfo::System` (via the scanner) for detect +
    /// start-time.
    scanner: ProcScanner,
    /// Incremental session reconstruction and in-memory process ownership.
    activity_collector: ActivityCollector,
    last_records: Vec<crate::procscan::ProcRecord>,
    grace: grace::Grace,
    started: Instant,
    identity: refresh::DaemonIdentity,
    evaluation_sequence: u64,
    refresh_token: Option<String>,
    wakeup: Arc<AtomicBool>,
    /// The joined `ps`-style cmdline text from THIS tick's single `scanner`
    /// pass, cached by `detect_and_touch` so the vscode host probe reuses it
    /// instead of spinning up a SECOND full process scan per tick.
    last_ps_text: String,
    power: Box<dyn PowerController>,
    lid: Box<dyn Lid>,
    // ── transition state ──
    engaged: bool,
    cooldown_until: i64,
    /// Last tick's manual mode, so a change is logged once.
    mode: Mode,
    /// A hold was engaged under a closed lid last tick (see [`lid::follow_up`]).
    held_closed: bool,
    /// The lock-dir guard (held for the daemon's whole life).
    lock: DaemonLock,
    /// Set by the INT/TERM signal handler; checked at the loop top + during sleep.
    shutdown: Arc<AtomicBool>,
}

impl Daemon {
    /// Detect agents + write/refresh a pidfile per match (§2.1.3 step 1). Writes
    /// `start_ts` = the pid's sysinfo `start_time()` so the gc pid-reuse branch
    /// compares like-with-like.
    fn detect_and_touch(&mut self) {
        // ONE process scan per tick. Cache the joined cmdline text so the vscode
        // host probe in `activity()` reuses it instead of building a fresh
        // `ProcScanner` and re-walking the whole process table a second time.
        let records = self.scanner.collect();
        self.last_ps_text = records
            .iter()
            .map(|r| r.cmdline.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let active_dir = Path::new(&self.cfg.active_dir);
        for r in &records {
            if !r.same_user || r.zombie {
                continue;
            }
            let Some(m) = crate::procscan::detect_line(r.pid, &r.exe, &r.cmdline) else {
                continue;
            };
            if m.pid == 0 {
                continue;
            }
            let start_ts = self.scanner.start_time(m.pid).unwrap_or(0);
            let body = refcount::pidfile_body(m.kind.name(), m.pid, &m.exe, start_ts);
            let pidfile = active_dir.join(format!("{}-{}.pid", m.kind.name(), m.pid));
            let _ = std::fs::write(&pidfile, body);
        }
        self.last_records = records;
    }

    /// Reuse the scanner's identity snapshot; low CPU never disproves work.
    fn gc(&mut self, _now: i64) {
        for e in refcount::read_entries(Path::new(&self.cfg.active_dir)) {
            let disk = refcount::field(&e.body, "start_ts").and_then(|s| s.parse::<i64>().ok());
            let live = self
                .last_records
                .iter()
                .find(|r| r.pid == e.pid && !r.zombie);
            if live.is_none() || live.is_some_and(|r| disk != Some(r.start_time)) {
                let _ = std::fs::remove_file(e.path);
            }
        }
    }
    fn activity(&mut self, now: i64) -> ActivitySnapshot {
        let mut activity =
            self.activity_collector
                .collect(&self.cfg, &mut self.scanner, &self.last_records, now);
        let ps_override = if std::env::var_os("VIGIL_VSCODE_PS_FIXTURE").is_some() {
            None
        } else {
            Some(self.last_ps_text.as_str())
        };
        activity.vscode = vscode::chat_is_active(
            Path::new(&self.cfg.copilot_home),
            Path::new(&self.cfg.vscode_copilot_state_file),
            now,
            self.cfg.idle_after_sec,
            self.cfg.vscode_copilot_discover_secs,
            self.cfg.vscode_copilot_recent_mins,
            ps_override,
        );
        activity
    }

    /// One tick of the loop (§2.1.3). Returns nothing; mutates `self.engaged` and
    /// `self.cooldown_until`.
    fn tick(&mut self) {
        let request = refresh::capture_request(
            Path::new(&self.cfg.state_dir),
            self.identity,
            self.refresh_token.as_deref(),
        )
        .unwrap_or_else(|e| {
            tracing::warn!("refresh request: {e}");
            None
        });
        // 1. detect + touch.
        self.detect_and_touch();
        // 2. gc.
        let now_gc = now_unix();
        self.gc(now_gc);
        // 3. per-agent activity (computed once/tick).
        let activity = self.activity(now_gc);
        let flags = ActivityFlags {
            claude: activity.provider_state(Agent::Claude) == "working",
            codex: activity.provider_state(Agent::Codex) == "working",
            copilot: activity.provider_state(Agent::Copilot) == "working",
            vscode: activity.vscode,
        };
        let count = activity.demand();
        // 4b. the manual mode turns the count into the hold demand.
        let mode = mode::read(Path::new(&self.cfg.mode_file));
        if mode != self.mode {
            tracing::info!("mode {} → {}", self.mode.as_str(), mode.as_str());
            self.mode = mode;
        }

        // 5. cutoff checks (+ cooldown re-arm).
        let now = now_unix();
        // Fail CLOSED: a genuine pmset read failure cuts the hold so a keep-awake
        // is never sustained while blind to heat.
        let cut_thermal = thermal::cut_thermal_failclosed(self.cfg.thermal_cpu_limit_floor);
        let battery_raw = battery::read_ps_raw();
        let cut_battery = battery::live_should_cut(&battery_raw, self.cfg.battery_floor_pct);
        let (cooldown_until, cooling) = crate::power_guard::cooldown_state(
            now,
            cut_thermal,
            self.cooldown_until,
            self.cfg.thermal_cooldown_secs,
        );
        self.cooldown_until = cooldown_until;
        let grace = self.grace.evaluate(
            self.started.elapsed(),
            activity.working() + activity.uncertain() + u32::from(activity.vscode) > 0,
            (request.is_some()
                && activity.working() + activity.uncertain() + u32::from(activity.vscode) == 0)
                || mode == Mode::Off
                || cut_thermal
                || cut_battery
                || cooling,
        );
        let demand = mode.demand(
            count
                + u32::from(
                    grace.automatic_hold
                        && activity.working() + activity.uncertain() + u32::from(activity.vscode)
                            == 0,
                ),
        );
        // 6. decide.
        let desired = desired_hold(demand, cut_thermal, cut_battery, cooling);
        // 7. act. The battery summary reuses the already-read raw (no second read).
        let battery_summary =
            battery::battery_summary(&battery::parse_ps(&battery_raw), self.cfg.battery_floor_pct);
        let was_engaged = self.engaged;
        self.engaged = act(
            self.power.as_mut(),
            desired,
            self.engaged,
            demand,
            cut_thermal,
            cut_battery,
            self.cfg.thermal_cooldown_secs,
            &battery_summary,
            flags,
            now,
        );
        // 7b. closed-lid follow-ups: sleep after a release, lock under a hold.
        self.held_closed = lid::follow_up(
            self.lid.as_ref(),
            was_engaged,
            self.engaged,
            self.held_closed,
        );
        // 8. write tick (POST-action engaged).
        self.evaluation_sequence += 1;
        if let Some(request) = request {
            self.refresh_token = Some(request.token);
        }
        let snap = TickSnapshot {
            version: 2,
            daemon_start: Some(self.identity.start),
            evaluation_sequence: self.evaluation_sequence,
            working: Some(activity.working()),
            waiting: Some(activity.waiting()),
            idle: Some(activity.idle()),
            unknown: Some(activity.unknown()),
            detected_hosts: Some(activity.detected_hosts),
            wrappers: Some(activity.wrappers),
            uncertain: Some(activity.uncertain()),
            vscode: Some(activity.vscode),
            grace_remaining: grace.remaining_secs,
            refresh_token: self.refresh_token.clone(),
            pid: std::process::id(),
            updated_at: now,
            tick_secs: self.cfg.tick_secs,
            refcount_active: count,
            desired_hold: desired,
            engaged: self.engaged,
            thermal_cut: cut_thermal,
            battery_cut: cut_battery,
            cooling,
            mode,
        };
        if let Err(e) = tick::write_tick(Path::new(&self.cfg.daemon_tick_file), &snap) {
            tracing::warn!("write tick file: {e}");
        }
        // 9. sleep is handled by the caller (interruptible).
    }

    /// Crash recovery before the loop (§2.1.7): refresh evidence FIRST (the same
    /// detect→touch→gc pass), then judge a leftover baseline against CURRENT work.
    fn recover(&mut self) {
        // 1. refresh evidence first.
        self.detect_and_touch();
        let now = now_unix();
        self.gc(now);
        // 2. startup_count via the same activity-filtered count.
        let startup_count = self.activity(now).demand();
        // 3. startup_can_hold = !thermal && !battery (both at startup).
        let thermal_should = thermal::cut_thermal_failclosed(self.cfg.thermal_cpu_limit_floor);
        let battery_should =
            battery::live_should_cut(&battery::read_ps_raw(), self.cfg.battery_floor_pct);
        let can_hold = !thermal_should && !battery_should;
        // 4. if baseline exists → recover_startup, judged against the demand.
        self.mode = mode::read(Path::new(&self.cfg.mode_file));
        if Path::new(&self.cfg.baseline_file).exists() {
            self.engaged =
                self.power
                    .recover_startup(self.mode.demand(startup_count), can_hold, now);
        }
    }

    /// The resident loop. NEVER returns except via the signal-driven clean exit
    /// or an error exit handled by [`run`].
    fn run_loop(mut self) -> ! {
        tracing::info!(
            "vigil-daemon started (tick={}s, mode={}, lock={})",
            self.cfg.tick_secs,
            self.mode.as_str(),
            self.cfg.lock_file,
        );
        let tick_secs = self.cfg.tick_secs.max(1);
        loop {
            // Signal flag checked at the loop TOP (§2.1.7).
            if self.shutdown.load(Ordering::SeqCst) {
                self.cleanup_and_exit();
            }
            self.tick();
            // Interruptible sleep: poll the flag at 100ms granularity so a
            // shutdown during the (default 5s) tick wait is honored promptly,
            // well within launchd's ExitTimeOut=60 window.
            let mut slept = 0u32;
            let total_ms = tick_secs * 1000;
            while slept < total_ms {
                if self.shutdown.load(Ordering::SeqCst) {
                    self.cleanup_and_exit();
                }
                if self.wakeup.swap(false, Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
                slept += 100;
            }
        }
    }

    /// Clean shutdown on the MAIN thread (§2.1.7): full_release if engaged, rm
    /// pidfile + tickfile, rm lockdir, exit(0).
    fn cleanup_and_exit(&mut self) -> ! {
        tracing::info!("shutting down — releasing sleep prevention and cleaning up");
        if self.engaged {
            self.power.full_release();
        }
        let _ = std::fs::remove_file(Path::new(&self.cfg.daemon_pidfile));
        let _ = std::fs::remove_file(Path::new(&self.cfg.daemon_tick_file));
        self.lock.disarm();
        self.lock.remove();
        std::process::exit(0);
    }
}

/// Entry point for the hidden `vigil daemon` subcommand. NEVER returns except via
/// `exit(0)` (clean) / `exit(1)` (fatal setup error).
pub fn run() -> ! {
    // Resolve config (no CLI overrides; env seams honored).
    let cfg = match config::load_default() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("vigil-daemon: {e}");
            std::process::exit(1);
        }
    };

    // File-based tracing subscriber to the daemon log. Hold the guard for the
    // process lifetime (it is moved into a leak so it never drops early — the
    // daemon never returns normally, so leaking is the simplest correct shape).
    if let Ok(guard) = crate::log::init_file_subscriber(&cfg.log_file, None) {
        Box::leak(Box::new(guard));
    }

    let my_pid = std::process::id();

    // ── single-instance guard ──
    let lock = match lock::acquire(Path::new(&cfg.lock_file)) {
        LockOutcome::Acquired(g) | LockOutcome::TookOver(g) => g,
        LockOutcome::LiveContention { other } => {
            tracing::warn!(
                "another vigil-daemon (pid={other}) holds {}.d — exiting",
                cfg.lock_file
            );
            std::process::exit(0);
        }
        LockOutcome::Failed => {
            tracing::error!("could not acquire lock");
            std::process::exit(1);
        }
    };

    // ── pre-flight: dirs + root helper wired up? ──
    if let Err(e) = cfg.ensure_state_dir() {
        tracing::error!("could not create state dir: {e}");
        std::process::exit(1);
    }
    if let Err(e) = lock::finalize_acquire(
        &lock,
        Path::new(&cfg.daemon_pidfile),
        Path::new(&cfg.daemon_tick_file),
        my_pid,
    ) {
        tracing::error!("could not finalize lock acquire: {e}");
        std::process::exit(1);
    }

    let power = match build_power_controller(&cfg) {
        Ok(power) => power,
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(1);
        }
    };

    // ── install INT/TERM handlers (AtomicBool flag; NOT HUP) ──
    let shutdown = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(sig, Arc::clone(&shutdown)) {
            tracing::error!("could not install signal handler: {e}");
            std::process::exit(1);
        }
    }

    let wakeup = Arc::new(AtomicBool::new(false));
    if let Err(e) = signal_hook::flag::register(signal_hook::consts::SIGUSR1, Arc::clone(&wakeup)) {
        tracing::error!("refresh signal: {e}");
        std::process::exit(1);
    }
    let start = ProcScanner::process_start(my_pid).unwrap_or_else(|| {
        tracing::error!("daemon process identity unavailable");
        std::process::exit(1)
    });
    let mut daemon = Daemon {
        scanner: ProcScanner::new(),
        activity_collector: ActivityCollector::default(),
        last_records: Vec::new(),
        grace: grace::Grace::default(),
        started: Instant::now(),
        identity: refresh::DaemonIdentity { pid: my_pid, start },
        evaluation_sequence: 0,
        refresh_token: None,
        wakeup,
        last_ps_text: String::new(),
        power,
        lid: Box::new(MacLid),
        engaged: false,
        cooldown_until: 0,
        mode: Mode::Auto,
        held_closed: false,
        lock,
        shutdown,
        cfg,
    };

    // ── crash recovery before the loop ──
    daemon.recover();

    daemon.run_loop();
}

fn build_power_controller(cfg: &VigilConfig) -> Result<Box<dyn PowerController>, String> {
    let controller = MacPowerController::new(cfg);
    // Helper round-trip: a missing-dirs error means setup/doctor is needed. Any
    // other outcome (ok / timeout / helper error) is tolerated — the bash daemon
    // only hard-fails on the dirs-missing check (`vigil_power_helper_check`).
    if let Err(crate::ipc::IpcError::DirsMissing) = controller.helper_status() {
        return Err(
            "root helper is not available — run 'vigil setup' or 'vigil doctor'".to_string(),
        );
    }
    Ok(Box::new(controller))
}

#[cfg(test)]
mod tests;
