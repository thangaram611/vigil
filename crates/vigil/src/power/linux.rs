use super::platform::{PowerController, PowerSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogindInhibitorKind {
    Idle,
    Sleep,
}

impl LogindInhibitorKind {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Sleep => "sleep",
        }
    }

    #[cfg(target_os = "linux")]
    fn to_logind(self) -> logind_zbus::manager::InhibitType {
        match self {
            Self::Idle => logind_zbus::manager::InhibitType::Idle,
            Self::Sleep => logind_zbus::manager::InhibitType::Sleep,
        }
    }
}

pub trait LogindClient {
    type Fd;

    fn acquire(&mut self, kind: LogindInhibitorKind) -> Result<Self::Fd, String>;
}

pub struct LinuxLogindPower<C: LogindClient> {
    client: C,
    inhibitors: Vec<LogindInhibitorKind>,
    fds: Vec<C::Fd>,
    last_error: Option<String>,
}

impl<C: LogindClient> LinuxLogindPower<C> {
    pub fn new(client: C, inhibitors: Vec<LogindInhibitorKind>) -> Self {
        Self {
            client,
            inhibitors,
            fds: Vec::new(),
            last_error: None,
        }
    }

    pub fn default_inhibitors() -> Vec<LogindInhibitorKind> {
        vec![LogindInhibitorKind::Idle, LogindInhibitorKind::Sleep]
    }

    fn acquire_all(&mut self) -> Result<(), String> {
        let mut fds = Vec::with_capacity(self.inhibitors.len());
        for &kind in &self.inhibitors {
            match self.client.acquire(kind) {
                Ok(fd) => fds.push(fd),
                Err(err) => {
                    self.last_error = Some(err.clone());
                    return Err(err);
                }
            }
        }

        self.fds = fds;
        self.last_error = None;
        Ok(())
    }

    fn mode_label(&self) -> String {
        let inhibitors = self
            .inhibitors
            .iter()
            .map(|kind| kind.label())
            .collect::<Vec<_>>()
            .join(":");
        format!("logind-{inhibitors}")
    }
}

#[cfg(target_os = "linux")]
pub struct SystemLogindClient {
    connection: Option<zbus::blocking::Connection>,
}

#[cfg(target_os = "linux")]
impl SystemLogindClient {
    pub const fn new() -> Self {
        Self { connection: None }
    }

    fn connection(&mut self) -> Result<&zbus::blocking::Connection, String> {
        if self.connection.is_none() {
            self.connection = Some(
                zbus::blocking::Connection::system()
                    .map_err(|err| format!("failed to connect to system bus: {err}"))?,
            );
        }

        Ok(self
            .connection
            .as_ref()
            .expect("connection initialized above"))
    }
}

#[cfg(target_os = "linux")]
impl Default for SystemLogindClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl LogindClient for SystemLogindClient {
    type Fd = zbus::zvariant::OwnedFd;

    fn acquire(&mut self, kind: LogindInhibitorKind) -> Result<Self::Fd, String> {
        let connection = self.connection()?;
        let manager = logind_zbus::manager::ManagerProxyBlocking::new(connection)
            .map_err(|err| format!("failed to create logind manager proxy: {err}"))?;
        manager
            .inhibit(
                kind.to_logind(),
                "vigil",
                "AI agents are actively working",
                "block",
            )
            .map_err(|err| format!("failed to acquire logind {} inhibitor: {err}", kind.label()))
    }
}

#[cfg(target_os = "linux")]
impl LinuxLogindPower<SystemLogindClient> {
    pub fn system_default() -> Self {
        Self::new(SystemLogindClient::new(), Self::default_inhibitors())
    }
}

impl<C: LogindClient> PowerController for LinuxLogindPower<C> {
    fn recover_startup(&mut self, active_count: u32, can_hold: bool, _now_unix: i64) -> bool {
        if active_count > 0 && can_hold {
            self.acquire_all().is_ok()
        } else {
            self.full_release();
            false
        }
    }

    fn engage(&mut self, _now_unix: i64) -> Result<(), String> {
        if self.fds.is_empty() {
            self.acquire_all()
        } else {
            Ok(())
        }
    }

    fn reconcile_engaged(&mut self) -> Result<(), String> {
        if self.fds.is_empty() {
            self.acquire_all()
        } else {
            Ok(())
        }
    }

    fn full_release(&mut self) {
        self.fds.clear();
    }

    fn soft_release(&mut self) {
        self.full_release()
    }

    fn observable_engaged(&self) -> bool {
        !self.fds.is_empty()
    }

    fn summary(&self) -> PowerSummary {
        PowerSummary {
            mode: self.mode_label(),
            platform_hold: self.observable_engaged(),
            baseline: None,
            helper_ok: Some(self.last_error.is_none()),
            assertion_pid: None,
            assertion_alive: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    #[derive(Default)]
    struct FakeLogindState {
        acquired: Vec<LogindInhibitorKind>,
        dropped: Vec<LogindInhibitorKind>,
        fail_on: Option<LogindInhibitorKind>,
    }

    struct FakeFd {
        kind: LogindInhibitorKind,
        state: Rc<RefCell<FakeLogindState>>,
    }

    impl Drop for FakeFd {
        fn drop(&mut self) {
            self.state.borrow_mut().dropped.push(self.kind);
        }
    }

    struct FakeLogindClient {
        state: Rc<RefCell<FakeLogindState>>,
    }

    impl FakeLogindClient {
        fn new(state: Rc<RefCell<FakeLogindState>>) -> Self {
            Self { state }
        }
    }

    impl LogindClient for FakeLogindClient {
        type Fd = FakeFd;

        fn acquire(&mut self, kind: LogindInhibitorKind) -> Result<Self::Fd, String> {
            let mut state = self.state.borrow_mut();
            if state.fail_on == Some(kind) {
                return Err(format!("failed on {}", kind.label()));
            }
            state.acquired.push(kind);
            Ok(FakeFd {
                kind,
                state: Rc::clone(&self.state),
            })
        }
    }

    fn fake_power(state: Rc<RefCell<FakeLogindState>>) -> LinuxLogindPower<FakeLogindClient> {
        LinuxLogindPower::new(
            FakeLogindClient::new(state),
            LinuxLogindPower::<FakeLogindClient>::default_inhibitors(),
        )
    }

    #[test]
    fn default_inhibitors_hold_only_idle_and_sleep() {
        assert_eq!(
            LinuxLogindPower::<FakeLogindClient>::default_inhibitors(),
            vec![LogindInhibitorKind::Idle, LogindInhibitorKind::Sleep]
        );
    }

    #[test]
    fn engage_acquires_all_inhibitors_and_release_drops_fds() {
        let state = Rc::new(RefCell::new(FakeLogindState::default()));
        let mut power = fake_power(Rc::clone(&state));

        power.engage(10).expect("engage");

        assert!(power.observable_engaged());
        assert_eq!(
            state.borrow().acquired,
            vec![LogindInhibitorKind::Idle, LogindInhibitorKind::Sleep]
        );

        power.full_release();

        assert!(!power.observable_engaged());
        assert_eq!(
            state.borrow().dropped,
            vec![LogindInhibitorKind::Idle, LogindInhibitorKind::Sleep]
        );
    }

    #[test]
    fn partial_acquire_failure_drops_acquired_fds_and_reports_not_engaged() {
        let state = Rc::new(RefCell::new(FakeLogindState {
            fail_on: Some(LogindInhibitorKind::Sleep),
            ..FakeLogindState::default()
        }));
        let mut power = fake_power(Rc::clone(&state));

        let err = power.engage(10).expect_err("sleep inhibitor should fail");

        assert_eq!(err, "failed on sleep");
        assert!(!power.observable_engaged());
        assert_eq!(state.borrow().acquired, vec![LogindInhibitorKind::Idle]);
        assert_eq!(state.borrow().dropped, vec![LogindInhibitorKind::Idle]);
    }

    #[test]
    fn reconcile_reacquires_when_fds_are_missing() {
        let state = Rc::new(RefCell::new(FakeLogindState::default()));
        let mut power = fake_power(Rc::clone(&state));

        power.reconcile_engaged().expect("reconcile");

        assert!(power.observable_engaged());
        assert_eq!(
            state.borrow().acquired,
            vec![LogindInhibitorKind::Idle, LogindInhibitorKind::Sleep]
        );
    }

    #[test]
    fn recover_releases_when_no_active_references_or_cannot_hold() {
        let state = Rc::new(RefCell::new(FakeLogindState::default()));
        let mut power = fake_power(Rc::clone(&state));

        assert!(power.recover_startup(1, true, 10));
        assert!(!power.recover_startup(0, true, 10));
        assert!(!power.recover_startup(1, false, 10));

        assert!(!power.observable_engaged());
    }

    #[test]
    fn summary_reports_mode_and_client_availability() {
        let state = Rc::new(RefCell::new(FakeLogindState::default()));
        let mut power = fake_power(Rc::clone(&state));

        let summary = power.summary();
        assert_eq!(summary.mode, "logind-idle:sleep");
        assert_eq!(summary.helper_ok, Some(true));
        assert!(!summary.platform_hold);

        state.borrow_mut().fail_on = Some(LogindInhibitorKind::Idle);
        assert_eq!(
            power.engage(10).expect_err("engage should fail"),
            "failed on idle"
        );

        let summary = power.summary();
        assert_eq!(summary.helper_ok, Some(false));
    }
}
