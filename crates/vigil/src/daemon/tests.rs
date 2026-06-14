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
