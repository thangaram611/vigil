//! Power facade for the daemon.
//!
//! Separates the daemon's hold/release decision loop from the concrete macOS
//! `pmset` + `caffeinate` implementation. The macOS adapter below delegates to
//! [`super::PowerMachine`], preserving its pure decision tests and side-effect
//! ordering; the daemon tests drive the loop through a fake controller.

use std::path::PathBuf;

use crate::ipc::HelperClient;
use crate::power::caffeinate::CaffeinateAssertion;
use crate::power::pmset::SleepReader;

use super::PowerMachine;

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
}

impl<I, C, S> PowerController for PowerMachine<'_, I, C, S>
where
    I: HelperClient,
    C: CaffeinateAssertion,
    S: SleepReader,
{
    fn recover_startup(&mut self, active_count: u32, can_hold: bool, now_unix: i64) -> bool {
        PowerMachine::recover_startup(self, active_count, can_hold, now_unix)
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
}

/// macOS production controller. It owns the long-lived seams and borrows them
/// into a zero-cost [`PowerMachine`] for each operation.
pub struct MacPowerController {
    ipc: crate::ipc::MacHelperClient,
    caffeinate: crate::power::caffeinate::MacCaffeinate,
    sleep: crate::power::pmset::MacSleepReader,
    baseline_file: PathBuf,
    caffeinate_pidfile: PathBuf,
}

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
}
