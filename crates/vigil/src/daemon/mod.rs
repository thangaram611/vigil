//! The resident tick loop (§2.1 + §3.1). Run by `launchd` via the LaunchAgent
//! plist as `vigil daemon` (the hidden subcommand).
//!
//! Owns ONE long-lived [`ProcScanner`] (the single `sysinfo::System` for
//! detect/start-time) plus a SECOND bare `sysinfo::System` for the gc cpu probe
//! (Q9: two Systems, simplest correct). The three [`PowerMachine`] seams
//! ([`MacHelperClient`], [`MacCaffeinate`], [`MacSleepReader`]) are owned as
//! struct fields; a thin [`PowerMachine`] borrow is constructed at each point of
//! use (it is a zero-cost reference struct), so the seams' lifetime spans the
//! whole daemon without a self-referential struct.
//!
//! Per-tick order (§2.1.3): detect+touch → gc → per-agent activity → activity-
//! filtered count → cutoff checks + cooldown re-arm → decide → act → write tick
//! (POST-action engaged) → sleep.

pub mod lock;
pub mod tick;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::activity::scan::{self, Agent};
use crate::activity::vscode;
use crate::config::{self, VigilConfig};
use crate::ipc::{HelperClient, IpcError, MacHelperClient};
use crate::power::PowerMachine;
use crate::power::caffeinate::{CaffeinateAssertion, MacCaffeinate};
use crate::power::pmset::{MacSleepReader, SleepReader};
use crate::procscan::ProcScanner;
use crate::refcount;
use crate::{battery, thermal};

use lock::{DaemonLock, LockOutcome};
use tick::TickSnapshot;

/// Current epoch seconds — the daemon's single clock (bash `vigil_now_unix`).
fn now_unix() -> i64 {
    chrono::Local::now().timestamp()
}

/// Read `pmset -g ps` (or its fixture) UNLESS forced (same rationale).
fn read_ps_or_skip(force: bool) -> String {
    if force {
        String::new()
    } else {
        battery::read_ps_raw()
    }
}

/// The four per-tick activity flags, computed once per tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct ActivityFlags {
    pub claude: bool,
    pub codex: bool,
    pub copilot: bool,
    pub vscode: bool,
}

/// The desired-hold predicate, VERBATIM from the bash contract
/// (`bin/vigil-daemon:156-159`): hold iff there is active work AND no thermal
/// cut AND no battery cut AND not in the post-thermal cooldown window.
pub fn desired_hold(count: u32, cut_thermal: bool, cut_battery: bool, cooling: bool) -> bool {
    count > 0 && !cut_thermal && !cut_battery && !cooling
}

/// The act-branch result: the new `engaged` value after acting on `(desired,
/// engaged)`. PURE over the seam side effects: the machine mutates power state,
/// this returns the bookkeeping bool the tick file records.
///
/// Release-reason priority is LOAD-BEARING (`bin/vigil-daemon:161-186`):
/// **thermal → SOFT (keep baseline) > battery → FULL > count==0 → FULL.**
#[allow(clippy::too_many_arguments)]
pub fn act<I, C, S>(
    machine: &PowerMachine<'_, I, C, S>,
    desired: bool,
    engaged: bool,
    count: u32,
    cut_thermal: bool,
    cut_battery: bool,
    cooldown_secs: u32,
    battery_summary: &str,
    flags: ActivityFlags,
    now: i64,
) -> bool
where
    I: HelperClient,
    C: CaffeinateAssertion,
    S: SleepReader,
{
    match (desired, engaged) {
        (true, false) => {
            let cs = if flags.claude { "active" } else { "idle" };
            let xs = if flags.codex { "active" } else { "idle" };
            let ps = if flags.copilot { "active" } else { "idle" };
            let vs = if flags.vscode { "active" } else { "idle" };
            tracing::info!(
                "engage — count={count} thermal=ok battery=ok claude={cs} codex={xs} copilot={ps} vscode_copilot_chat={vs}"
            );
            // engaged := true ONLY if the helper engage succeeds (bash:
            // `if vigil_pmset_engage; then DAEMON_ENGAGED=1; fi`).
            match machine.engage(now) {
                Ok(()) => true,
                Err(e) => {
                    tracing::error!("engage failed: {e}");
                    false
                }
            }
        }
        (true, true) => {
            // Reassert drift; engaged stays true unless reconcile errors.
            match machine.reconcile_engaged() {
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
                machine.soft_release();
            } else if cut_battery {
                tracing::warn!("release — battery floor ({battery_summary})");
                machine.full_release();
            } else if count == 0 {
                tracing::info!("release — no active agents");
                machine.full_release();
            }
            // engaged := false ALWAYS, even if a release no-ops.
            false
        }
        (false, false) => engaged,
    }
}

/// The resident daemon (§2.1.2). Owns the long-lived scanner + the power seams.
pub struct Daemon {
    cfg: VigilConfig,
    /// THE one long-lived `sysinfo::System` (via the scanner) for detect +
    /// start-time.
    scanner: ProcScanner,
    /// A SECOND bare `System` for the gc cpu probe (Q9 default).
    sys_for_gc: sysinfo::System,
    /// The joined `ps`-style cmdline text from THIS tick's single `scanner`
    /// pass, cached by `detect_and_touch` so the vscode host probe reuses it
    /// instead of spinning up a SECOND full process scan per tick.
    last_ps_text: String,
    // ── owned PowerMachine seams (borrowed by a thin PowerMachine per use) ──
    ipc: MacHelperClient,
    caffeinate: MacCaffeinate,
    sleep: MacSleepReader,
    baseline_file: PathBuf,
    caffeinate_pidfile: PathBuf,
    // ── transition state ──
    engaged: bool,
    cooldown_until: i64,
    /// The lock-dir guard (held for the daemon's whole life).
    lock: DaemonLock,
    /// Set by the INT/TERM signal handler; checked at the loop top + during sleep.
    shutdown: Arc<AtomicBool>,
}

impl Daemon {
    /// Build a thin [`PowerMachine`] borrowing the owned seams. Cheap: a
    /// reference struct + two `PathBuf` clones. Constructed at each point of use
    /// so the seams' lifetime spans the whole daemon (no self-referential field).
    fn machine(&self) -> PowerMachine<'_, MacHelperClient, MacCaffeinate, MacSleepReader> {
        PowerMachine {
            ipc: &self.ipc,
            caffeinate: &self.caffeinate,
            sleep: &self.sleep,
            baseline_file: self.baseline_file.clone(),
            caffeinate_pidfile: self.caffeinate_pidfile.clone(),
        }
    }

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
    }

    /// GC stale pidfiles on the shared gc System (§2.1.3 step 2).
    fn gc(&mut self, now: i64) {
        refcount::gc(
            Path::new(&self.cfg.active_dir),
            &mut self.sys_for_gc,
            self.cfg.stale_age_secs,
            self.cfg.stale_cpu_pct,
            now,
        );
    }

    /// Per-agent activity, computed once per tick (§2.1.3 step 3).
    ///
    /// vscode reuses THIS tick's single `scanner` pass via `last_ps_text` rather
    /// than letting `host_running` spin up a second full process scan. The
    /// `VIGIL_VSCODE_PS_FIXTURE` seam is preserved: when the fixture is set
    /// (tests), pass `None` so `host_running` reads the fixture; otherwise hand it
    /// the cached scan text.
    fn activity(&self, now: i64) -> ActivityFlags {
        let idle = self.cfg.idle_after_sec;
        let claude = scan::is_active(
            &scan::session_dir_from_provider_home(Path::new(&self.cfg.claude_home), Agent::Claude),
            Agent::Claude.pattern(),
            idle,
            now,
        );
        let codex = scan::is_active(
            &scan::session_dir_from_provider_home(Path::new(&self.cfg.codex_home), Agent::Codex),
            Agent::Codex.pattern(),
            idle,
            now,
        );
        let copilot = scan::is_active(
            &scan::session_dir_from_provider_home(
                Path::new(&self.cfg.copilot_home),
                Agent::Copilot,
            ),
            Agent::Copilot.pattern(),
            idle,
            now,
        );
        // Reuse this tick's single scan unless a ps fixture is set (test seam),
        // in which case pass None so host_running reads VIGIL_VSCODE_PS_FIXTURE.
        let ps_override = if std::env::var_os("VIGIL_VSCODE_PS_FIXTURE").is_some() {
            None
        } else {
            Some(self.last_ps_text.as_str())
        };
        let vscode = vscode::chat_is_active(
            Path::new(&self.cfg.copilot_home),
            Path::new(&self.cfg.vscode_copilot_state_file),
            now,
            idle,
            self.cfg.vscode_copilot_discover_secs,
            self.cfg.vscode_copilot_recent_mins,
            ps_override,
        );
        ActivityFlags {
            claude,
            codex,
            copilot,
            vscode,
        }
    }

    /// `VIGIL_FORCE` (bash `1` => true; anything else/unset => false). Checked
    /// FIRST inside the thermal/battery guards so a per-tick force never spawns
    /// pmset.
    fn force() -> bool {
        std::env::var("VIGIL_FORCE")
            .map(|v| v == "1")
            .unwrap_or(false)
    }

    /// One tick of the loop (§2.1.3). Returns nothing; mutates `self.engaged` and
    /// `self.cooldown_until`.
    fn tick(&mut self) {
        // 1. detect + touch.
        self.detect_and_touch();
        // 2. gc.
        let now_gc = now_unix();
        self.gc(now_gc);
        // 3. per-agent activity (computed once/tick).
        let flags = self.activity(now_gc);
        // 4. activity-filtered count.
        let active_dir = Path::new(&self.cfg.active_dir);
        let count = refcount::count(
            active_dir,
            flags.claude,
            flags.codex,
            flags.copilot,
            flags.vscode,
        );
        // 5. cutoff checks (+ cooldown re-arm). VIGIL_FORCE first: skip the pmset
        // read entirely on force (bash short-circuits before forking pmset).
        let now = now_unix();
        let force = Self::force();
        // Fail CLOSED: a genuine pmset read failure cuts the hold so a keep-awake
        // is never sustained while blind to heat. `force` still short-circuits
        // the read (no fork), preserving the force contract.
        let cut_thermal = thermal::cut_thermal_failclosed(force, self.cfg.thermal_cpu_limit_floor);
        let battery_raw = read_ps_or_skip(force);
        let cut_battery = battery::live_should_cut(force, &battery_raw, self.cfg.battery_floor_pct);
        let (cooldown_until, cooling) = crate::power_guard::cooldown_state(
            now,
            cut_thermal,
            self.cooldown_until,
            self.cfg.thermal_cooldown_secs,
        );
        self.cooldown_until = cooldown_until;
        // 6. decide.
        let desired = desired_hold(count, cut_thermal, cut_battery, cooling);
        // 7. act. The battery summary reuses the already-read raw (no second read).
        let battery_summary =
            battery::battery_summary(&battery::parse_ps(&battery_raw), self.cfg.battery_floor_pct);
        let machine = self.machine();
        self.engaged = act(
            &machine,
            desired,
            self.engaged,
            count,
            cut_thermal,
            cut_battery,
            self.cfg.thermal_cooldown_secs,
            &battery_summary,
            flags,
            now,
        );
        // 8. write tick (POST-action engaged).
        let snap = TickSnapshot {
            pid: std::process::id(),
            updated_at: now,
            tick_secs: self.cfg.tick_secs,
            refcount_active: count,
            desired_hold: desired,
            engaged: self.engaged,
            thermal_cut: cut_thermal,
            battery_cut: cut_battery,
            cooling,
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
        let flags = self.activity(now);
        let active_dir = Path::new(&self.cfg.active_dir);
        let startup_count = refcount::count(
            active_dir,
            flags.claude,
            flags.codex,
            flags.copilot,
            flags.vscode,
        );
        // 3. startup_can_hold = !thermal && !battery (both at startup).
        let force = Self::force();
        let thermal_should =
            thermal::cut_thermal_failclosed(force, self.cfg.thermal_cpu_limit_floor);
        let battery_should =
            battery::live_should_cut(force, &read_ps_or_skip(force), self.cfg.battery_floor_pct);
        let guard = StartupGuard {
            thermal: thermal_should,
            battery: battery_should,
        };
        // 4. if baseline exists → recover_startup.
        if Path::new(&self.cfg.baseline_file).exists() {
            let machine = self.machine();
            self.engaged = machine.recover_startup(startup_count, &guard, now);
        }
    }

    /// The resident loop. NEVER returns except via the signal-driven clean exit
    /// or an error exit handled by [`run`].
    fn run_loop(mut self) -> ! {
        tracing::info!(
            "vigil-daemon started (tick={}s, lock={})",
            self.cfg.tick_secs,
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
            let machine = self.machine();
            machine.full_release();
        }
        let _ = std::fs::remove_file(Path::new(&self.cfg.daemon_pidfile));
        let _ = std::fs::remove_file(Path::new(&self.cfg.daemon_tick_file));
        self.lock.disarm();
        self.lock.remove();
        std::process::exit(0);
    }
}

/// A fixed startup [`PowerGuard`] over the two booleans evaluated at startup —
/// so `recover_startup` does not re-read pmset.
struct StartupGuard {
    thermal: bool,
    battery: bool,
}
impl crate::power_guard::PowerGuard for StartupGuard {
    fn thermal_cut(&self) -> bool {
        self.thermal
    }
    fn battery_cut(&self) -> bool {
        self.battery
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

    let ipc = MacHelperClient {
        request_dir: PathBuf::from(&cfg.power_request_dir),
        response_dir: PathBuf::from(&cfg.power_response_dir),
        timeout_secs: cfg.power_helper_timeout_secs,
    };
    // Helper round-trip: a missing-dirs error means setup/doctor is needed. Any
    // other outcome (ok / timeout / helper error) is tolerated — the bash daemon
    // only hard-fails on the dirs-missing check (`vigil_power_helper_check`).
    if let Err(IpcError::DirsMissing) = ipc.status() {
        tracing::error!("root helper is not available — run 'vigil setup' or 'vigil doctor'");
        std::process::exit(1);
    }

    // ── install INT/TERM handlers (AtomicBool flag; NOT HUP) ──
    let shutdown = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        if let Err(e) = signal_hook::flag::register(sig, Arc::clone(&shutdown)) {
            tracing::error!("could not install signal handler: {e}");
            std::process::exit(1);
        }
    }

    let mut daemon = Daemon {
        baseline_file: PathBuf::from(&cfg.baseline_file),
        caffeinate_pidfile: PathBuf::from(&cfg.caffeinate_pidfile),
        scanner: ProcScanner::new(),
        sys_for_gc: sysinfo::System::new(),
        last_ps_text: String::new(),
        ipc,
        caffeinate: MacCaffeinate,
        sleep: MacSleepReader,
        engaged: false,
        cooldown_until: 0,
        lock,
        shutdown,
        cfg,
    };

    // ── crash recovery before the loop ──
    daemon.recover();

    daemon.run_loop();
}

#[cfg(test)]
mod tests;
