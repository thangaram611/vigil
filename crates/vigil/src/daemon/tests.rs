//! Daemon unit tests with FAKE seams: the desired-hold predicate truth table,
//! the four `(desired, engaged)` act-branches (including release-reason
//! priority — soft KEEPS baseline, full CLEARS it), and the start-time/pidfile
//! write contract. NO real daemon launch, no real pmset/launchctl.
//!
//! The lock stale-recovery + tick-file byte tests live in the `lock` and `tick`
//! submodules respectively.

use std::cell::RefCell;
use std::path::PathBuf;

use super::{ActivityFlags, act, desired_hold};
use crate::ipc::{HelperClient, IpcError, Response};
use crate::power::PowerMachine;
use crate::power::caffeinate::CaffeinateAssertion;
use crate::power::pmset::SleepReader;

// ── desired-hold predicate truth table (bin/vigil-daemon:156-159) ─────────────

#[test]
fn desired_hold_truth_table() {
    // hold iff count>0 && !thermal && !battery && !cooling.
    assert!(
        desired_hold(1, false, false, false),
        "active, all clear → hold"
    );
    assert!(!desired_hold(0, false, false, false), "no agents → no hold");
    assert!(
        !desired_hold(3, true, false, false),
        "thermal cut → no hold"
    );
    assert!(
        !desired_hold(3, false, true, false),
        "battery cut → no hold"
    );
    assert!(!desired_hold(3, false, false, true), "cooling → no hold");
    // any one cut suppresses regardless of count.
    assert!(!desired_hold(99, true, true, true), "all cuts → no hold");
    assert!(
        desired_hold(2, false, false, false),
        "multi-agent clear → hold"
    );
}

// ── fake seams (a trimmed copy of the power-module test fakes) ─────────────────

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
            .map(|s| crate::power::baseline_value_from_json(&s))
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
    alive: RefCell<std::collections::HashSet<u32>>,
    killed: RefCell<Vec<u32>>,
}
impl FakeCaffeinate {
    fn new() -> Self {
        FakeCaffeinate {
            next_pid: RefCell::new(2000),
            alive: RefCell::new(std::collections::HashSet::new()),
            killed: RefCell::new(Vec::new()),
        }
    }
}
impl CaffeinateAssertion for FakeCaffeinate {
    fn spawn(&self) -> std::io::Result<u32> {
        let mut p = self.next_pid.borrow_mut();
        *p += 1;
        let pid = *p;
        self.alive.borrow_mut().insert(pid);
        Ok(pid)
    }
    fn is_alive_by_identity(&self, pid: u32) -> bool {
        self.alive.borrow().contains(&pid)
    }
    fn is_caffeinate_basename(&self, pid: u32) -> bool {
        self.alive.borrow().contains(&pid)
    }
    fn kill(&self, pid: u32) {
        self.alive.borrow_mut().remove(&pid);
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
    /// Like [`Self::machine`] but with an injected IPC seam (e.g. a failing helper)
    /// reusing the harness's tempdir-backed caffeinate/sleep/baseline paths.
    fn machine_with_ipc<'a, I: HelperClient>(
        &'a self,
        ipc: &'a I,
    ) -> PowerMachine<'a, I, FakeCaffeinate, FakeSleep> {
        PowerMachine {
            ipc,
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

const NOW: i64 = 1_700_000_000;

// ── act-branch: (true, false) → engage ────────────────────────────────────────

#[test]
fn act_engage_when_desired_and_not_engaged() {
    let h = Harness::new(0);
    let m = h.machine();
    let engaged = act(
        &m,
        true,  // desired
        false, // engaged
        2,     // count
        false, // cut_thermal
        false, // cut_battery
        60,    // cooldown_secs
        "on AC",
        ActivityFlags {
            claude: true,
            ..Default::default()
        },
        NOW,
    );
    assert!(engaged, "engage succeeded → engaged=true");
    assert_eq!(h.sd(), 1, "engage set SleepDisabled=1");
    assert!(h.baseline_file.exists(), "engage captured baseline");
    assert!(h.events().contains(&"helper engage".to_string()));
}

// ── act-branch: (true, true) → reconcile ──────────────────────────────────────

#[test]
fn act_reconcile_when_desired_and_engaged() {
    let h = Harness::new(0);
    let m = h.machine();
    // First engage to set up baseline + caffeinate.
    m.engage(NOW).unwrap();
    // Drift SleepDisabled to 0; reconcile must reassert.
    std::fs::write(&h.sleep_file, "0\n").unwrap();
    let engaged = act(
        &m,
        true,
        true,
        2,
        false,
        false,
        60,
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(engaged, "reconcile keeps engaged=true");
    assert_eq!(h.sd(), 1, "reconcile reasserted SleepDisabled=1");
    assert!(
        h.events().iter().filter(|e| *e == "helper engage").count() >= 2,
        "reconcile requested helper engage again"
    );
}

// ── act-branch: (false, true) release priority — THERMAL → SOFT (keep baseline) ─

#[test]
fn act_release_thermal_is_soft_and_keeps_baseline() {
    let h = Harness::new(0);
    let m = h.machine();
    m.engage(NOW).unwrap();
    assert!(h.baseline_file.exists(), "engaged: baseline present");
    // desired=false, engaged=true, thermal cut set → SOFT release (keep baseline).
    let engaged = act(
        &m,
        false,
        true,
        3,
        true,
        /*thermal*/ false,
        60,
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(!engaged, "release → engaged=false");
    assert!(
        h.baseline_file.exists(),
        "THERMAL soft release KEEPS baseline.json"
    );
    assert!(
        !h.caffeinate_pidfile.exists(),
        "soft release kills caffeinate"
    );
    assert!(h.events().contains(&"helper release".to_string()));
}

// ── act-branch: release priority — BATTERY → FULL (clear baseline) ────────────

// ── act-branch: FULL release (clear baseline) — battery cut OR count==0 idle ──

#[test]
fn act_full_release_clears_baseline() {
    // Both a battery cut (count>0) and a count==0 idle drop are FULL releases that
    // CLEAR baseline.json. (The thermal SOFT release and the thermal-wins priority
    // case assert different post-state and stay separate.) Each row engages first,
    // then fires its own FULL-release trigger.
    let cases: &[(&str, u32, bool, &str)] = &[
        ("battery cut", 3, true, "on battery 18% (floor 20%)"),
        ("no agents", 0, false, "on AC"),
    ];
    for (label, count, cut_battery, summary) in cases {
        let h = Harness::new(0);
        let m = h.machine();
        m.engage(NOW).unwrap();
        let engaged = act(
            &m,
            false, // desired
            true,  // engaged
            *count,
            false, // cut_thermal
            *cut_battery,
            60,
            summary,
            ActivityFlags::default(),
            NOW,
        );
        assert!(!engaged, "{label}: release → engaged=false");
        assert!(
            !h.baseline_file.exists(),
            "{label}: FULL release CLEARS baseline.json"
        );
    }
}

// ── release priority is ordered: thermal WINS over battery+count ──────────────

#[test]
fn act_release_thermal_wins_over_battery_when_both_cut() {
    let h = Harness::new(0);
    let m = h.machine();
    m.engage(NOW).unwrap();
    // BOTH thermal and battery cut → thermal branch wins → SOFT → keep baseline.
    let engaged = act(
        &m,
        false,
        true,
        0,
        true,
        /*thermal*/ true,
        /*battery*/ 60,
        "on battery",
        ActivityFlags::default(),
        NOW,
    );
    assert!(!engaged);
    assert!(
        h.baseline_file.exists(),
        "thermal priority → SOFT release → baseline KEPT even though battery also cut"
    );
}

// ── act-branch: (false, false) → no-op ────────────────────────────────────────

#[test]
fn act_noop_when_not_desired_and_not_engaged() {
    let h = Harness::new(0);
    let m = h.machine();
    let engaged = act(
        &m,
        false,
        false,
        0,
        false,
        false,
        60,
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(!engaged, "no-op keeps engaged=false");
    assert!(
        h.events().is_empty(),
        "no helper traffic on the no-op branch"
    );
    assert!(!h.baseline_file.exists());
}

// ── engage-failure: engaged STAYS false when the helper engage errors ─────────

struct FailingIpc;
impl HelperClient for FailingIpc {
    fn request(&self, _action: crate::helper::validate::Action) -> Result<Response, IpcError> {
        Err(IpcError::Timeout)
    }
}

#[test]
fn act_reconcile_failure_flips_engaged_false() {
    // (true, true) branch: reconcile_engaged() must ERROR → the arm returns false.
    // Setup: engage with the real FakeIpc so baseline + caffeinate exist and the
    // caffeinate reads alive-by-identity (respawn=false), then drift SleepDisabled
    // to 0 so reconcile_decision yields reassert=true. Swapping in FailingIpc makes
    // the reassert helper engage Err → reconcile_engaged returns Err → act → false.
    let h = Harness::new(0);
    h.machine().engage(NOW).unwrap();
    assert!(h.baseline_file.exists(), "engaged: baseline captured");
    // Drift SleepDisabled=0 → reconcile_decision(0, alive)=(reassert=true, respawn=false).
    std::fs::write(&h.sleep_file, "0\n").unwrap();
    let ipc = FailingIpc;
    let m = h.machine_with_ipc(&ipc);
    let engaged = act(
        &m,
        true,  // desired
        true,  // engaged
        2,     // count
        false, // cut_thermal
        false, // cut_battery
        60,    // cooldown_secs
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(
        !engaged,
        "reconcile helper-engage error → reconcile arm returns engaged=false"
    );
    assert!(
        h.baseline_file.exists(),
        "reconcile NEVER clears baseline (no release on the (true,true) arm)"
    );
}

#[test]
fn act_release_with_no_reason_still_flips_engaged_false() {
    // (false, true) release arm with NO release reason: cut_thermal=false,
    // cut_battery=false, count>0 (so the count==0 idle branch does NOT fire).
    // None of the three release sub-branches run, so NO release call is made — yet
    // `engaged := false` ALWAYS (the trailing unconditional false), and baseline +
    // caffeinate are left untouched (neither soft_release nor full_release ran).
    let h = Harness::new(0);
    let m = h.machine();
    m.engage(NOW).unwrap();
    assert!(h.baseline_file.exists(), "engaged: baseline present");
    assert!(
        h.caffeinate_pidfile.exists(),
        "engaged: caffeinate pidfile present"
    );
    let engaged = act(
        &m,
        false, // desired
        true,  // engaged
        3,     // count > 0 → the count==0 idle release branch is skipped
        false, // cut_thermal
        false, // cut_battery
        60,    // cooldown_secs
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(!engaged, "no-reason release arm still flips engaged=false");
    assert!(
        !h.events().contains(&"helper release".to_string()),
        "no release sub-branch fired → NO helper release request"
    );
    assert!(
        h.baseline_file.exists(),
        "no release ran → baseline.json untouched"
    );
    assert!(
        h.caffeinate_pidfile.exists(),
        "no release ran → caffeinate pidfile untouched"
    );
}

#[test]
fn act_engage_failure_keeps_engaged_false() {
    let h = Harness::new(0);
    let ipc = FailingIpc;
    let m = h.machine_with_ipc(&ipc);
    let engaged = act(
        &m,
        true,
        false,
        1,
        false,
        false,
        60,
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(!engaged, "helper engage error → engaged stays false");
    assert!(
        !h.caffeinate_pidfile.exists(),
        "caffeinate NOT spawned when helper engage fails (bash early return)"
    );
}

// ── partial-engage robustness: trust the observable pmset state ───────────────

/// A helper that APPLIES the privileged `pmset disablesleep 1` (writes the fake
/// SleepDisabled file) but then reports a CLIENT failure — the exact "partial
/// engage" shape where the root helper ran pmset but the daemon's response read
/// timed out (e.g. an unreadable response dir).
struct PartialEngageIpc {
    sleep_file: PathBuf,
}
impl HelperClient for PartialEngageIpc {
    fn request(&self, action: crate::helper::validate::Action) -> Result<Response, IpcError> {
        if let crate::helper::validate::Action::Engage = action {
            std::fs::write(&self.sleep_file, "1\n").unwrap();
        }
        Err(IpcError::Timeout)
    }
}

#[test]
fn act_engage_partial_adopts_observable_hold() {
    // (true,false): machine.engage() errors at the CLIENT, but the privileged
    // pmset change DID land (SleepDisabled=1). act() must trust the observable
    // state and ADOPT the hold — engaged=true + spawn the caffeinate the failed
    // engage skipped — so it is tracked and the later release path can undo it,
    // instead of leaking an untracked pmset=1.
    let h = Harness::new(0);
    let ipc = PartialEngageIpc {
        sleep_file: h.sleep_file.clone(),
    };
    let m = h.machine_with_ipc(&ipc);
    let engaged = act(
        &m,
        true,
        false,
        2,
        false,
        false,
        60,
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(
        engaged,
        "partial engage + observable SleepDisabled=1 → adopted"
    );
    assert_eq!(h.sd(), 1, "privileged pmset change is observable");
    assert!(
        h.caffeinate_pidfile.exists(),
        "adopt spawns the caffeinate the failed engage skipped"
    );
    assert!(
        h.baseline_file.exists(),
        "baseline was captured before the failed engage"
    );
}

#[test]
fn act_idle_reconciles_orphaned_partial_engage() {
    // (false,false): idle and NOT tracking a hold, yet SleepDisabled=1 with a
    // baseline of 0 — the orphan signature of a partial engage the client never
    // confirmed. act() must release to restore SleepDisabled=0 and clear the
    // stale baseline (fires once).
    let h = Harness::new(1); // leaked: SleepDisabled stuck at 1
    std::fs::write(
        &h.baseline_file,
        "{\"SleepDisabled\":0,\"captured_at\":1700000000}\n",
    )
    .unwrap();
    let m = h.machine();
    let engaged = act(
        &m,
        false,
        false,
        0,
        false,
        false,
        60,
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(!engaged, "stays not-engaged after reconcile");
    assert_eq!(
        h.sd(),
        0,
        "orphaned hold released → SleepDisabled restored to baseline 0"
    );
    assert!(
        !h.baseline_file.exists(),
        "reconcile clears the stale baseline"
    );
    assert!(
        h.events().contains(&"helper release".to_string()),
        "reconcile issued a helper release"
    );
}

#[test]
fn act_idle_leaves_kept_soft_release_baseline_untouched() {
    // (false,false) NEGATIVE: after a thermal SOFT release the baseline.json is
    // intentionally KEPT and SleepDisabled was restored to its (1) baseline.
    // SleepDisabled=1 is the CORRECT state here, not a leak — the reconcile is
    // gated on baseline_value==0, so it must NOT fire.
    let h = Harness::new(1);
    std::fs::write(
        &h.baseline_file,
        "{\"SleepDisabled\":1,\"captured_at\":1700000000}\n",
    )
    .unwrap();
    let m = h.machine();
    let engaged = act(
        &m,
        false,
        false,
        0,
        false,
        false,
        60,
        "on AC",
        ActivityFlags::default(),
        NOW,
    );
    assert!(!engaged);
    assert_eq!(
        h.sd(),
        1,
        "baseline=1 is the correct restored state, untouched"
    );
    assert!(
        h.baseline_file.exists(),
        "kept soft-release baseline preserved"
    );
    assert!(
        h.events().is_empty(),
        "no helper traffic when baseline=1 (not a leak)"
    );
}
