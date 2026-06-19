//! Platform power facade for the daemon.
//!
//! Phase 5.8 starts by separating the daemon's hold/release decision loop from
//! the concrete macOS `pmset` + `caffeinate` implementation. The macOS adapter
//! below delegates to the existing [`super::PowerMachine`], preserving its pure
//! decision tests and side-effect ordering. Non-macOS targets get an explicit
//! unsupported controller until the Linux/Windows platform impls are filled in.

#[cfg(target_os = "macos")]
use std::path::PathBuf;

use crate::ipc::HelperClient;
use crate::power::caffeinate::CaffeinateAssertion;
use crate::power::pmset::SleepReader;

use super::{PowerMachine, recover_decision};

/// Read-only summary of the platform hold state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerSummary {
    pub mode: String,
    pub platform_hold: bool,
    pub baseline: Option<u8>,
    pub helper_ok: Option<bool>,
    pub assertion_pid: Option<u32>,
    pub assertion_alive: bool,
}

/// The daemon-facing power controller contract.
pub trait PowerController {
    fn recover_startup(&mut self, active_count: u32, can_hold: bool, now_unix: i64) -> bool;
    fn engage(&mut self, now_unix: i64) -> Result<(), String>;
    fn reconcile_engaged(&mut self) -> Result<(), String>;
    fn full_release(&mut self);
    fn soft_release(&mut self);
    fn observable_engaged(&self) -> bool;

    /// Called when `engage` reported an error but [`Self::observable_engaged`]
    /// proves the OS hold landed anyway. macOS uses this to spawn the tracked
    /// `caffeinate` assertion that the failed engage path skipped.
    fn adopt_after_failed_engage(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// True when the daemon is idle/not-tracking but the platform state shows a
    /// leaked self-owned hold that must be released.
    fn orphaned_hold_requires_release(&self) -> bool {
        false
    }

    fn summary(&self) -> PowerSummary;
}

impl<I, C, S> PowerController for PowerMachine<'_, I, C, S>
where
    I: HelperClient,
    C: CaffeinateAssertion,
    S: SleepReader,
{
    fn recover_startup(&mut self, active_count: u32, can_hold: bool, now_unix: i64) -> bool {
        let baseline_present = self.baseline_file.exists();
        let pidfile_present = self.caffeinate_pidfile.exists();

        match recover_decision(active_count, can_hold, baseline_present, pidfile_present) {
            super::RecoverAction::NotEngaged => false,
            super::RecoverAction::Reconcile => {
                if !baseline_present && pidfile_present {
                    let _ = self.capture_baseline(now_unix);
                }
                self.reconcile_engaged().is_ok()
            }
            super::RecoverAction::Release => {
                if !baseline_present && pidfile_present {
                    let _ = self.capture_baseline(now_unix);
                }
                self.full_release();
                false
            }
        }
    }

    fn engage(&mut self, now_unix: i64) -> Result<(), String> {
        PowerMachine::engage(self, now_unix)
    }

    fn reconcile_engaged(&mut self) -> Result<(), String> {
        PowerMachine::reconcile_engaged(self)
    }

    fn full_release(&mut self) {
        PowerMachine::full_release(self);
    }

    fn soft_release(&mut self) {
        PowerMachine::soft_release(self);
    }

    fn observable_engaged(&self) -> bool {
        self.sleep_disabled()
    }

    fn adopt_after_failed_engage(&mut self) -> Result<(), String> {
        self.spawn_caffeinate()
            .map_err(|e| format!("adopt spawn caffeinate: {e}"))
    }

    fn orphaned_hold_requires_release(&self) -> bool {
        self.baseline_present() && self.sleep_disabled() && self.baseline_value() == 0
    }

    fn summary(&self) -> PowerSummary {
        let assertion_pid = self.caffeinate_pid();
        PowerSummary {
            mode: "best-effort".to_string(),
            platform_hold: self.sleep_disabled(),
            baseline: self.baseline_present().then(|| self.baseline_value()),
            helper_ok: None,
            assertion_pid,
            assertion_alive: assertion_pid
                .is_some_and(|pid| self.caffeinate.is_alive_by_identity(pid)),
        }
    }
}

/// macOS production controller. It owns the long-lived seams and borrows them
/// into a zero-cost [`PowerMachine`] for each operation.
#[cfg(target_os = "macos")]
pub struct MacPowerController {
    ipc: crate::ipc::MacHelperClient,
    caffeinate: crate::power::caffeinate::MacCaffeinate,
    sleep: crate::power::pmset::MacSleepReader,
    baseline_file: PathBuf,
    caffeinate_pidfile: PathBuf,
}

#[cfg(target_os = "macos")]
impl MacPowerController {
    pub fn new(cfg: &crate::config::VigilConfig) -> Self {
        Self {
            ipc: crate::ipc::MacHelperClient {
                request_dir: PathBuf::from(&cfg.power_request_dir),
                response_dir: PathBuf::from(&cfg.power_response_dir),
                timeout_secs: cfg.power_helper_timeout_secs,
            },
            caffeinate: crate::power::caffeinate::MacCaffeinate,
            sleep: crate::power::pmset::MacSleepReader,
            baseline_file: PathBuf::from(&cfg.baseline_file),
            caffeinate_pidfile: PathBuf::from(&cfg.caffeinate_pidfile),
        }
    }

    pub fn helper_status(&self) -> Result<crate::ipc::Response, crate::ipc::IpcError> {
        self.ipc.status()
    }

    fn machine(
        &self,
    ) -> PowerMachine<
        '_,
        crate::ipc::MacHelperClient,
        crate::power::caffeinate::MacCaffeinate,
        crate::power::pmset::MacSleepReader,
    > {
        PowerMachine {
            ipc: &self.ipc,
            caffeinate: &self.caffeinate,
            sleep: &self.sleep,
            baseline_file: self.baseline_file.clone(),
            caffeinate_pidfile: self.caffeinate_pidfile.clone(),
        }
    }
}

#[cfg(target_os = "macos")]
impl PowerController for MacPowerController {
    fn recover_startup(&mut self, active_count: u32, can_hold: bool, now_unix: i64) -> bool {
        let mut machine = self.machine();
        PowerController::recover_startup(&mut machine, active_count, can_hold, now_unix)
    }

    fn engage(&mut self, now_unix: i64) -> Result<(), String> {
        let mut machine = self.machine();
        PowerController::engage(&mut machine, now_unix)
    }

    fn reconcile_engaged(&mut self) -> Result<(), String> {
        let mut machine = self.machine();
        PowerController::reconcile_engaged(&mut machine)
    }

    fn full_release(&mut self) {
        let mut machine = self.machine();
        PowerController::full_release(&mut machine);
    }

    fn soft_release(&mut self) {
        let mut machine = self.machine();
        PowerController::soft_release(&mut machine);
    }

    fn observable_engaged(&self) -> bool {
        let machine = self.machine();
        PowerController::observable_engaged(&machine)
    }

    fn adopt_after_failed_engage(&mut self) -> Result<(), String> {
        let mut machine = self.machine();
        PowerController::adopt_after_failed_engage(&mut machine)
    }

    fn orphaned_hold_requires_release(&self) -> bool {
        let machine = self.machine();
        PowerController::orphaned_hold_requires_release(&machine)
    }

    fn summary(&self) -> PowerSummary {
        let machine = self.machine();
        PowerController::summary(&machine)
    }
}

/// Explicit placeholder for targets whose real platform controller has not
/// shipped yet.
pub struct UnsupportedPowerController {
    platform: &'static str,
}

impl UnsupportedPowerController {
    pub const fn new(platform: &'static str) -> Self {
        Self { platform }
    }
}

impl PowerController for UnsupportedPowerController {
    fn recover_startup(&mut self, _active_count: u32, _can_hold: bool, _now_unix: i64) -> bool {
        false
    }

    fn engage(&mut self, _now_unix: i64) -> Result<(), String> {
        Err(format!(
            "{} power controller is not implemented yet",
            self.platform
        ))
    }

    fn reconcile_engaged(&mut self) -> Result<(), String> {
        Err(format!(
            "{} power controller is not implemented yet",
            self.platform
        ))
    }

    fn full_release(&mut self) {}

    fn soft_release(&mut self) {}

    fn observable_engaged(&self) -> bool {
        false
    }

    fn summary(&self) -> PowerSummary {
        PowerSummary {
            mode: "unsupported".to_string(),
            platform_hold: false,
            baseline: None,
            helper_ok: Some(false),
            assertion_pid: None,
            assertion_alive: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_controller_is_explicitly_inert() {
        let mut c = UnsupportedPowerController::new("linux");
        assert!(!c.recover_startup(1, true, 1_700_000_000));
        assert!(c.engage(1_700_000_000).unwrap_err().contains("linux"));
        assert!(c.reconcile_engaged().unwrap_err().contains("linux"));
        assert!(!c.observable_engaged());

        let summary = c.summary();
        assert_eq!(summary.mode, "unsupported");
        assert!(!summary.platform_hold);
        assert_eq!(summary.baseline, None);
        assert_eq!(summary.helper_ok, Some(false));
        assert_eq!(summary.assertion_pid, None);
        assert!(!summary.assertion_alive);
    }
}
