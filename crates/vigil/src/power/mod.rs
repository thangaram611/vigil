//! Power state machine — Rust port of `lib/pmset.sh` (Phase 5.5).
//!
//! DEAD-FROM-RUST until 5.7: library-only. Nothing here is wired into the live
//! bash daemon. The privileged pmset transitions go through the [`crate::ipc`]
//! `HelperClient` (the file-based IPC client); this module never runs pmset
//! itself — only the helper (root side) does.
//!
//! ## Pure vs side-effecting
//! PURE (unit-testable without root/pmset):
//! - [`caffeinate::is_stale_display`] (in [`crate::power::caffeinate`]),
//! - [`baseline_value_from_json`] (FAIL-SAFE to 0),
//! - [`reconcile_decision`],
//! - [`recover_decision`] (the [`RecoverAction`] truth table).
//!
//! SIDE-EFFECTING orchestration over the seams ([`HelperClient`],
//! [`CaffeinateAssertion`], [`SleepReader`]):
//! - [`PowerMachine::capture_baseline`] (idempotent, byte-identical JSON),
//! - [`PowerMachine::engage`] / [`PowerMachine::full_release`] /
//!   [`PowerMachine::soft_release`] / [`PowerMachine::reconcile_engaged`] /
//!   [`PowerMachine::recover_startup`].

pub mod assertions;
pub mod caffeinate;
pub mod pmset;

use std::path::{Path, PathBuf};

use crate::ipc::HelperClient;
use crate::power_guard::PowerGuard;
use caffeinate::CaffeinateAssertion;
use pmset::SleepReader;

/// Parse the `SleepDisabled` value out of the daemon `baseline.json`. FAIL-SAFE:
/// returns `0` (sleep-enabled = release target 0) on missing/corrupt/non-(0|1).
/// Never panics, never reports a stuck `1`.
///
/// Mirrors the bash `_vigil_pidfile_field`/parameter-expansion parse, but is
/// total. The byte format is `{"SleepDisabled":N,"captured_at":TS}`.
pub fn baseline_value_from_json(text: &str) -> u8 {
    // Find `"SleepDisabled":` then the first 0|1 token after it.
    let key = "\"SleepDisabled\":";
    let Some(pos) = text.find(key) else {
        return 0;
    };
    let after = &text[pos + key.len()..];
    // Skip whitespace, then read the next char(s) up to a non-digit.
    let trimmed = after.trim_start();
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    match digits.as_str() {
        "0" => 0,
        "1" => 1,
        _ => 0,
    }
}

/// What [`recover_decision`] tells the caller to do at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverAction {
    /// Neither baseline.json nor caffeinate pidfile exist => not engaged.
    NotEngaged,
    /// Active refs remain AND can_hold => reconcile + report engaged.
    Reconcile,
    /// No active refs (or cannot hold) => full release + report not engaged.
    Release,
}

/// PURE startup-recovery decision. Ports the bash `vigil_pmset_recover_startup`
/// branch structure (the side-effecting recapture-baseline-when-pidfile-only is
/// handled by [`PowerMachine::recover_startup`]; THIS fn decides the terminal
/// action).
///
/// - `baseline_present || pidfile_present == false` => [`RecoverAction::NotEngaged`].
/// - `active_count > 0 && can_hold` => [`RecoverAction::Reconcile`].
/// - else => [`RecoverAction::Release`].
pub fn recover_decision(
    active_count: u32,
    can_hold: bool,
    baseline_present: bool,
    pidfile_present: bool,
) -> RecoverAction {
    if !baseline_present && !pidfile_present {
        return RecoverAction::NotEngaged;
    }
    if active_count > 0 && can_hold {
        RecoverAction::Reconcile
    } else {
        RecoverAction::Release
    }
}

/// PURE reconcile decision: given the live SleepDisabled and whether caffeinate
/// is alive-by-identity, decide whether to reassert (helper engage) and/or
/// respawn caffeinate. Never touches baseline.
///
/// Returns `(reassert, respawn)`.
pub fn reconcile_decision(sleep_disabled: u8, caffeinate_alive: bool) -> (bool, bool) {
    (sleep_disabled != 1, !caffeinate_alive)
}

/// The power state machine over the three seams. Side-effecting; the pure
/// decisions above are tested independently.
pub struct PowerMachine<'a, I: HelperClient, C: CaffeinateAssertion, S: SleepReader> {
    pub ipc: &'a I,
    pub caffeinate: &'a C,
    pub sleep: &'a S,
    /// Daemon-side `baseline.json` path.
    pub baseline_file: PathBuf,
    /// `caffeinate.pid` path. Content is the bare pid, byte-identical to bash.
    pub caffeinate_pidfile: PathBuf,
}

impl<I: HelperClient, C: CaffeinateAssertion, S: SleepReader> PowerMachine<'_, I, C, S> {
    /// Capture the current SleepDisabled into `baseline.json` IDEMPOTENTLY (if
    /// the file already exists, leave it). Byte-identical JSON to bash:
    /// `{"SleepDisabled":N,"captured_at":TS}\n`.
    pub fn capture_baseline(&self, now_unix: i64) -> std::io::Result<()> {
        if self.baseline_file.exists() {
            return Ok(());
        }
        let prior = self.sleep.read();
        let json = format!("{{\"SleepDisabled\":{prior},\"captured_at\":{now_unix}}}\n");
        std::fs::write(&self.baseline_file, json)
    }

    /// Read the baseline value from `baseline.json` (FAIL-SAFE to 0).
    pub fn baseline_value(&self) -> u8 {
        match std::fs::read_to_string(&self.baseline_file) {
            Ok(s) => baseline_value_from_json(&s),
            Err(_) => 0,
        }
    }

    /// Clear the daemon baseline.json.
    pub fn clear_baseline(&self) {
        let _ = std::fs::remove_file(&self.baseline_file);
    }

    /// Read the caffeinate pid from the pidfile (None if absent/unparseable).
    fn caffeinate_pid(&self) -> Option<u32> {
        let s = std::fs::read_to_string(&self.caffeinate_pidfile).ok()?;
        s.trim().parse::<u32>().ok()
    }

    /// True iff the recorded caffeinate pid is alive BY IDENTITY.
    pub fn caffeinate_alive(&self) -> bool {
        match self.caffeinate_pid() {
            Some(pid) => self.caffeinate.is_alive_by_identity(pid),
            None => false,
        }
    }

    /// Spawn a fresh `caffeinate -i`, replacing a stale/display-holding one.
    /// Writes the BARE pid to the pidfile (byte-identical to bash `echo $!`).
    pub fn spawn_caffeinate(&self) -> std::io::Result<()> {
        if self.caffeinate_alive() {
            return Ok(());
        }
        // Replace a stale-but-caffeinate pid (kill it first), mirroring bash.
        if let Some(old) = self.caffeinate_pid() {
            // Bash gates the kill on `[[ "$old_base" == "caffeinate" ]]`: it
            // kills a stale display-holding caffeinate (alive but not
            // alive-by-identity) yet REFUSES to kill a pid the OS has recycled
            // onto an unrelated, live, non-caffeinate process. We mirror that
            // exactly via the basename-only identity check — NEVER an
            // unconditional SIGTERM to whatever pid the pidfile holds.
            if self.caffeinate.is_caffeinate_basename(old) {
                self.caffeinate.kill(old);
            }
        }
        let _ = std::fs::remove_file(&self.caffeinate_pidfile);
        let pid = self.caffeinate.spawn()?;
        // Bare pid, no trailing format drift beyond the newline bash writes.
        std::fs::write(&self.caffeinate_pidfile, format!("{pid}\n"))
    }

    /// Kill + clear the caffeinate child (best-effort). Used by both releases.
    fn kill_caffeinate(&self) {
        if let Some(pid) = self.caffeinate_pid() {
            self.caffeinate.kill(pid);
        }
        let _ = std::fs::remove_file(&self.caffeinate_pidfile);
    }

    /// 0 → >0 transition: capture-baseline-idempotent → helper engage → ONLY
    /// THEN spawn `caffeinate -i`. Returns Err if the helper engage fails (the
    /// caffeinate child is NOT spawned in that case, matching bash's early
    /// return).
    pub fn engage(&self, now_unix: i64) -> Result<(), String> {
        self.capture_baseline(now_unix)
            .map_err(|e| format!("capture baseline: {e}"))?;
        self.ipc
            .engage()
            .map_err(|e| format!("helper engage: {e}"))?;
        self.spawn_caffeinate()
            .map_err(|e| format!("spawn caffeinate: {e}"))?;
        Ok(())
    }

    /// Full release: helper release → ALWAYS kill caffeinate even if release
    /// failed → clear daemon baseline.json. Mirrors bash `vigil_pmset_release`,
    /// which does not return early on a failed helper release.
    pub fn full_release(&self) {
        // helper release (failure logged but does not short-circuit cleanup).
        let _ = self.ipc.release();
        self.kill_caffeinate();
        self.clear_baseline();
    }

    /// Soft release (thermal path): helper release → kill caffeinate → KEEP
    /// baseline.json so a later re-engage can still restore the original state.
    pub fn soft_release(&self) {
        let _ = self.ipc.release();
        self.kill_caffeinate();
        // baseline.json intentionally kept.
    }

    /// Reassert the live engaged state after drift (per-tick / crash recovery).
    /// Re-reads SleepDisabled; if != 1 → helper engage; if caffeinate not
    /// alive-by-identity → respawn. NEVER captures/clears baseline.
    pub fn reconcile_engaged(&self) -> Result<(), String> {
        let sd = self.sleep.read();
        let (reassert, respawn) = reconcile_decision(sd, self.caffeinate_alive());
        if reassert {
            self.ipc
                .engage()
                .map_err(|e| format!("reconcile helper engage: {e}"))?;
        }
        if respawn {
            self.spawn_caffeinate()
                .map_err(|e| format!("reconcile respawn: {e}"))?;
        }
        Ok(())
    }

    /// Startup recovery for an unclean daemon restart. Returns `true` iff the
    /// caller should treat the daemon as engaged.
    ///
    /// - neither baseline.json nor pidfile => not engaged (`false`).
    /// - pidfile present WITHOUT baseline => recapture baseline first.
    /// - then the [`recover_decision`] terminal action: Reconcile (engaged) or
    ///   Release (not engaged).
    ///
    /// `can_hold` evaluates thermal+battery at startup via the 5.4
    /// [`PowerGuard`].
    pub fn recover_startup<G: PowerGuard>(
        &self,
        active_count: u32,
        guard: &G,
        now_unix: i64,
    ) -> bool {
        let baseline_present = self.baseline_file.exists();
        let pidfile_present = self.caffeinate_pidfile.exists();
        let can_hold = guard.can_hold();

        match recover_decision(active_count, can_hold, baseline_present, pidfile_present) {
            RecoverAction::NotEngaged => false,
            RecoverAction::Reconcile => {
                // pidfile-without-baseline => recapture baseline first (bash).
                if !baseline_present && pidfile_present {
                    let _ = self.capture_baseline(now_unix);
                }
                self.reconcile_engaged().is_ok()
            }
            RecoverAction::Release => {
                if !baseline_present && pidfile_present {
                    let _ = self.capture_baseline(now_unix);
                }
                self.full_release();
                false
            }
        }
    }
}

/// Helper: does `path` exist as a plain file (no symlink-follow concern here —
/// these are daemon-owned state files in a 0700 dir).
#[allow(dead_code)]
fn file_exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{IpcError, Response};
    use crate::power_guard::PowerGuard;
    use std::cell::RefCell;
    use std::path::PathBuf;

    // ── PURE: baseline_value_from_json (FAIL-SAFE) ────────────────────────────

    #[test]
    fn baseline_value_parses_0_and_1() {
        assert_eq!(
            baseline_value_from_json("{\"SleepDisabled\":0,\"captured_at\":1700000000}\n"),
            0
        );
        assert_eq!(
            baseline_value_from_json("{\"SleepDisabled\":1,\"captured_at\":1700000000}\n"),
            1
        );
    }

    #[test]
    fn baseline_value_fail_safe_on_corrupt() {
        assert_eq!(baseline_value_from_json(""), 0);
        assert_eq!(baseline_value_from_json("not json"), 0);
        assert_eq!(baseline_value_from_json("{\"SleepDisabled\":}"), 0);
        assert_eq!(baseline_value_from_json("{\"SleepDisabled\":2}"), 0);
        assert_eq!(baseline_value_from_json("{\"Other\":1}"), 0);
        // whitespace tolerance
        assert_eq!(baseline_value_from_json("{\"SleepDisabled\": 1 }"), 1);
    }

    // ── PURE: recover_decision truth table ────────────────────────────────────

    #[test]
    fn recover_decision_table() {
        // neither present => not engaged
        assert_eq!(
            recover_decision(1, true, false, false),
            RecoverAction::NotEngaged
        );
        // active + can_hold => reconcile
        assert_eq!(
            recover_decision(1, true, true, false),
            RecoverAction::Reconcile
        );
        // active but cannot hold => release
        assert_eq!(
            recover_decision(1, false, true, false),
            RecoverAction::Release
        );
        // no active refs => release
        assert_eq!(
            recover_decision(0, true, true, false),
            RecoverAction::Release
        );
        // pidfile-only still counts as "something present"
        assert_eq!(
            recover_decision(1, true, false, true),
            RecoverAction::Reconcile
        );
    }

    #[test]
    fn reconcile_decision_table() {
        // drifted to 0, caffeinate alive => reassert only
        assert_eq!(reconcile_decision(0, true), (true, false));
        // at 1, caffeinate dead => respawn only
        assert_eq!(reconcile_decision(1, false), (false, true));
        // both wrong
        assert_eq!(reconcile_decision(0, false), (true, true));
        // both right
        assert_eq!(reconcile_decision(1, true), (false, false));
    }

    // ── Fakes for the side-effecting machine ──────────────────────────────────

    struct FakeIpc {
        sleep_file: PathBuf,
        baseline_file: PathBuf,
        events: RefCell<Vec<String>>,
    }
    impl FakeIpc {
        fn read_sd(&self) -> u8 {
            std::fs::read_to_string(&self.sleep_file)
                .ok()
                .map(|s| if s.trim() == "1" { 1 } else { 0 })
                .unwrap_or(0)
        }
        fn baseline_value(&self) -> u8 {
            std::fs::read_to_string(&self.baseline_file)
                .ok()
                .map(|s| baseline_value_from_json(&s))
                .unwrap_or(0)
        }
    }
    impl HelperClient for FakeIpc {
        fn request(&self, action: crate::helper::validate::Action) -> Result<Response, IpcError> {
            use crate::helper::validate::Action;
            self.events
                .borrow_mut()
                .push(format!("helper {}", action.as_str()));
            match action {
                Action::Engage => {
                    std::fs::write(&self.sleep_file, "1\n").unwrap();
                    Ok(Response {
                        status: "ok".into(),
                        action: "engage".into(),
                        baseline: "0".into(),
                        current: "1".into(),
                        message: "ok".into(),
                    })
                }
                Action::Release => {
                    let target = self.baseline_value();
                    std::fs::write(&self.sleep_file, format!("{target}\n")).unwrap();
                    Ok(Response {
                        status: "ok".into(),
                        action: "release".into(),
                        baseline: "none".into(),
                        current: target.to_string(),
                        message: "ok".into(),
                    })
                }
                Action::Status => Ok(Response {
                    status: "ok".into(),
                    action: "status".into(),
                    baseline: "none".into(),
                    current: self.read_sd().to_string(),
                    message: "ok".into(),
                }),
            }
        }
    }

    struct FakeCaffeinate {
        next_pid: RefCell<u32>,
        /// Pids that are alive AND are a live, non-stale `caffeinate` (our own
        /// spawns) — alive-by-identity AND basename==caffeinate.
        alive: RefCell<std::collections::HashSet<u32>>,
        /// Pids that are alive but are NOT caffeinate (a recycled impostor the
        /// OS reassigned). These are alive-by-NOTHING here, and crucially
        /// `is_caffeinate_basename` returns false — so bash (and now Rust) must
        /// refuse to SIGTERM them.
        impostors: RefCell<std::collections::HashSet<u32>>,
        killed: RefCell<Vec<u32>>,
        spawns: RefCell<Vec<u32>>,
    }
    impl FakeCaffeinate {
        fn new() -> Self {
            FakeCaffeinate {
                next_pid: RefCell::new(1000),
                alive: RefCell::new(std::collections::HashSet::new()),
                impostors: RefCell::new(std::collections::HashSet::new()),
                killed: RefCell::new(Vec::new()),
                spawns: RefCell::new(Vec::new()),
            }
        }
        /// Plant a LIVE, non-caffeinate impostor pid (PID-reuse victim). It is
        /// not alive-by-identity and not caffeinate-by-basename, so the spawn
        /// path must spare it.
        fn plant_impostor(&self, pid: u32) {
            self.impostors.borrow_mut().insert(pid);
        }
    }
    impl CaffeinateAssertion for FakeCaffeinate {
        fn spawn(&self) -> std::io::Result<u32> {
            let mut p = self.next_pid.borrow_mut();
            *p += 1;
            let pid = *p;
            self.alive.borrow_mut().insert(pid);
            self.spawns.borrow_mut().push(pid);
            Ok(pid)
        }
        fn is_alive_by_identity(&self, pid: u32) -> bool {
            self.alive.borrow().contains(&pid)
        }
        fn is_caffeinate_basename(&self, pid: u32) -> bool {
            // Our own spawns are caffeinate; planted impostors are NOT. (A stale
            // display-holding caffeinate would be in `alive`-by-basename but not
            // alive-by-identity; the fake's spawns model the not-stale case,
            // which suffices for the kill-gating tests.)
            self.alive.borrow().contains(&pid)
        }
        fn kill(&self, pid: u32) {
            self.alive.borrow_mut().remove(&pid);
            self.impostors.borrow_mut().remove(&pid);
            self.killed.borrow_mut().push(pid);
        }
    }

    struct FakeSleep {
        sleep_file: PathBuf,
    }
    impl SleepReader for FakeSleep {
        fn read(&self) -> u8 {
            std::fs::read_to_string(&self.sleep_file)
                .ok()
                .map(|s| if s.trim() == "1" { 1 } else { 0 })
                .unwrap_or(0)
        }
    }

    struct FixedGuard {
        hold: bool,
    }
    impl PowerGuard for FixedGuard {
        fn thermal_cut(&self) -> bool {
            !self.hold
        }
        fn battery_cut(&self) -> bool {
            false
        }
    }

    struct Harness {
        _dir: tempfile::TempDir,
        sleep_file: PathBuf,
        baseline_file: PathBuf,
        caffeinate_pidfile: PathBuf,
        ipc: FakeIpc,
        caffeinate: FakeCaffeinate,
        sleep: FakeSleep,
    }
    impl Harness {
        fn new(initial_sd: u8) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let sleep_file = dir.path().join("sleepdisabled");
            let baseline_file = dir.path().join("baseline.json");
            let caffeinate_pidfile = dir.path().join("caffeinate.pid");
            std::fs::write(&sleep_file, format!("{initial_sd}\n")).unwrap();
            Harness {
                ipc: FakeIpc {
                    sleep_file: sleep_file.clone(),
                    baseline_file: baseline_file.clone(),
                    events: RefCell::new(Vec::new()),
                },
                caffeinate: FakeCaffeinate::new(),
                sleep: FakeSleep {
                    sleep_file: sleep_file.clone(),
                },
                sleep_file,
                baseline_file,
                caffeinate_pidfile,
                _dir: dir,
            }
        }
        fn machine(&self) -> PowerMachine<'_, FakeIpc, FakeCaffeinate, FakeSleep> {
            PowerMachine {
                ipc: &self.ipc,
                caffeinate: &self.caffeinate,
                sleep: &self.sleep,
                baseline_file: self.baseline_file.clone(),
                caffeinate_pidfile: self.caffeinate_pidfile.clone(),
            }
        }
        fn sd(&self) -> u8 {
            std::fs::read_to_string(&self.sleep_file)
                .map(|s| if s.trim() == "1" { 1 } else { 0 })
                .unwrap_or(0)
        }
        fn events(&self) -> Vec<String> {
            self.ipc.events.borrow().clone()
        }
    }

    #[test]
    fn engage_and_release_restore_baseline_with_best_effort_hold() {
        let h = Harness::new(0);
        let m = h.machine();
        m.engage(1700000000).unwrap();
        assert_eq!(h.sd(), 1, "engage sets SleepDisabled=1");
        assert_eq!(m.baseline_value(), 0, "baseline captured before engage");
        assert!(
            h.caffeinate_pidfile.exists(),
            "engage writes caffeinate pidfile"
        );
        assert!(m.caffeinate_alive(), "caffeinate alive after engage");
        assert!(h.events().contains(&"helper engage".to_string()));

        m.full_release();
        assert_eq!(h.sd(), 0, "release restores baseline");
        assert!(!h.baseline_file.exists(), "release clears baseline.json");
        assert!(
            !h.caffeinate_pidfile.exists(),
            "release clears caffeinate pidfile"
        );
        assert!(h.events().contains(&"helper release".to_string()));
    }

    #[test]
    fn release_uses_helper_release_when_baseline_is_one() {
        let h = Harness::new(1);
        let m = h.machine();
        m.engage(1700000000).unwrap();
        assert_eq!(h.sd(), 1);
        assert_eq!(
            m.baseline_value(),
            1,
            "baseline captures pre-existing SleepDisabled=1"
        );
        m.full_release();
        assert_eq!(h.sd(), 1, "release restores SleepDisabled=1 baseline");
        assert!(!h.baseline_file.exists(), "release clears baseline.json");
        assert!(h.events().contains(&"helper release".to_string()));
    }

    #[test]
    fn reconcile_reasserts_sleepdisabled_drift() {
        let h = Harness::new(0);
        let m = h.machine();
        m.engage(1700000000).unwrap();
        std::fs::write(&h.sleep_file, "0\n").unwrap(); // drift
        m.reconcile_engaged().unwrap();
        assert_eq!(h.sd(), 1, "reconcile restores SleepDisabled=1");
        // helper engage requested at least twice (engage + reconcile reassert).
        assert!(
            h.events().iter().filter(|e| *e == "helper engage").count() >= 2,
            "reconcile requested helper engage"
        );
    }

    #[test]
    fn reconcile_restarts_missing_caffeinate() {
        let h = Harness::new(0);
        let m = h.machine();
        m.engage(1700000000).unwrap();
        let old_pid: u32 = std::fs::read_to_string(&h.caffeinate_pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // simulate caffeinate death
        h.caffeinate.kill(old_pid);
        m.reconcile_engaged().unwrap();
        let new_pid: u32 = std::fs::read_to_string(&h.caffeinate_pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_ne!(old_pid, new_pid, "reconcile respawned a new caffeinate");
        assert!(m.caffeinate_alive());
    }

    #[test]
    fn reconcile_rejects_reused_non_caffeinate_pid() {
        let h = Harness::new(1);
        let m = h.machine();
        // Plant a pidfile pointing at a LIVE, non-caffeinate process the OS has
        // recycled onto our old recorded pid (an impostor). caffeinate_alive()
        // is false (not alive-by-identity) AND is_caffeinate_basename() is false.
        // Bash's `basename==caffeinate` guard spares this innocent process; the
        // Rust spawn path must NOT SIGTERM it.
        const IMPOSTOR: u32 = 424242;
        h.caffeinate.plant_impostor(IMPOSTOR);
        std::fs::write(&h.caffeinate_pidfile, format!("{IMPOSTOR}\n")).unwrap();
        assert!(
            !m.caffeinate_alive(),
            "impostor pid is not alive-by-identity"
        );
        m.reconcile_engaged().unwrap();
        let new_pid: u32 = std::fs::read_to_string(&h.caffeinate_pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_ne!(new_pid, IMPOSTOR, "reconcile replaced the impostor pid");
        assert!(m.caffeinate_alive());
        // The crux: the live recycled non-caffeinate process was NEVER SIGTERMed.
        assert!(
            !h.caffeinate.killed.borrow().contains(&IMPOSTOR),
            "spawn path must NOT kill a live, recycled non-caffeinate pid (bash basename guard)"
        );
    }

    #[test]
    fn startup_recovery_keeps_hold_when_refs_remain() {
        let h = Harness::new(0);
        let m = h.machine();
        std::fs::write(
            &h.baseline_file,
            "{\"SleepDisabled\":0,\"captured_at\":1700000000}\n",
        )
        .unwrap();
        let engaged = m.recover_startup(1, &FixedGuard { hold: true }, 1700000000);
        assert!(engaged, "recovery reports engaged");
        assert_eq!(h.sd(), 1, "startup recovery reasserts SleepDisabled");
        assert!(
            h.baseline_file.exists(),
            "recovery preserves baseline while active"
        );
        assert!(m.caffeinate_alive());
    }

    #[test]
    fn startup_recovery_releases_when_no_refs() {
        let h = Harness::new(1);
        let m = h.machine();
        std::fs::write(
            &h.baseline_file,
            "{\"SleepDisabled\":0,\"captured_at\":1700000000}\n",
        )
        .unwrap();
        let engaged = m.recover_startup(0, &FixedGuard { hold: true }, 1700000000);
        assert!(!engaged, "recovery should not report engaged with no refs");
        assert_eq!(h.sd(), 0, "startup recovery restores baseline when idle");
        assert!(!h.baseline_file.exists(), "idle recovery clears baseline");
    }

    #[test]
    fn startup_recovery_not_engaged_when_nothing_present() {
        let h = Harness::new(0);
        let m = h.machine();
        let engaged = m.recover_startup(1, &FixedGuard { hold: true }, 1700000000);
        assert!(!engaged, "no baseline + no pidfile => not engaged");
    }

    #[test]
    fn capture_baseline_is_idempotent_and_byte_identical() {
        let h = Harness::new(1);
        let m = h.machine();
        m.capture_baseline(1700000000).unwrap();
        let first = std::fs::read_to_string(&h.baseline_file).unwrap();
        assert_eq!(first, "{\"SleepDisabled\":1,\"captured_at\":1700000000}\n");
        // second call leaves it untouched even if SleepDisabled changed.
        std::fs::write(&h.sleep_file, "0\n").unwrap();
        m.capture_baseline(1700009999).unwrap();
        let second = std::fs::read_to_string(&h.baseline_file).unwrap();
        assert_eq!(second, first, "idempotent: baseline.json untouched");
    }

    #[test]
    fn soft_release_keeps_baseline() {
        let h = Harness::new(0);
        let m = h.machine();
        m.engage(1700000000).unwrap();
        m.soft_release();
        assert_eq!(h.sd(), 0, "soft release restores baseline value");
        assert!(h.baseline_file.exists(), "soft release KEEPS baseline.json");
        assert!(
            !h.caffeinate_pidfile.exists(),
            "soft release kills caffeinate"
        );
    }
}
