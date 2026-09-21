//! The closed-lid follow-ups: what the daemon must do itself because macOS will
//! not, measured on 2026-09-21 (`docs/apple-silicon-lid-closed.md`).
//!
//! - Clamshell sleep is edge-triggered on the lid closing. If sleep was disabled
//!   at that moment, a later release leaves the Mac awake until the idle timer —
//!   which an agent's own `caffeinate -i` also stops. So a release with the lid
//!   closed is followed by `pmset sleepnow`; without it the thermal and battery
//!   guards would release the hold and leave the Mac running in a bag.
//! - A held Mac neither sleeps nor display-sleeps when its lid closes, so it
//!   does not lock. `pmset displaysleepnow` locks it without breaking the hold.
//!
//! Both apply only when closing the lid would sleep this Mac. With an external
//! display attached a closed lid is normal desk use and is left alone.

use super::pmset::PINNED_PATH;

/// The lid seam: one read and the two unprivileged `pmset` requests.
pub trait Lid {
    /// True iff the lid is closed AND closing it sleeps this Mac.
    fn closed(&self) -> bool;
    /// Sleep the display, which locks when a password is required immediately.
    fn lock_display(&self);
    /// Sleep the system now.
    fn sleep_now(&self);
}

/// `AppleClamshellState` and `AppleClamshellCausesSleep` from
/// `ioreg -rd1 -c IOPMrootDomain`. A missing key reads as "not closed".
pub fn parse_lid_closed(ioreg: &str) -> bool {
    let is_yes = |key: &str| {
        ioreg
            .lines()
            .find(|line| line.contains(key))
            .is_some_and(|line| line.trim_end().ends_with("= Yes"))
    };
    is_yes("\"AppleClamshellState\"") && is_yes("\"AppleClamshellCausesSleep\"")
}

pub struct MacLid;

impl MacLid {
    fn pmset(arg: &str) {
        let ran = std::process::Command::new("/usr/bin/pmset")
            .arg(arg)
            .env_clear()
            .env("PATH", PINNED_PATH)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !ran.is_ok_and(|s| s.success()) {
            tracing::warn!("pmset {arg} failed");
        }
    }
}

impl Lid for MacLid {
    fn closed(&self) -> bool {
        std::process::Command::new("/usr/sbin/ioreg")
            .args(["-rd1", "-c", "IOPMrootDomain"])
            .env_clear()
            .env("PATH", PINNED_PATH)
            .output()
            .is_ok_and(|o| parse_lid_closed(&String::from_utf8_lossy(&o.stdout)))
    }

    fn lock_display(&self) {
        Self::pmset("displaysleepnow");
    }

    fn sleep_now(&self) {
        Self::pmset("sleepnow");
    }
}

/// Apply the follow-ups after one act step and return the new `held_closed`
/// (a hold is engaged under a closed lid). `held_closed` makes the lock
/// edge-triggered: it fires once when that state begins, not every tick.
///
/// The lid is read only when a hold is or was involved, so an idle daemon does
/// not spawn `ioreg` every tick.
pub fn follow_up(lid: &dyn Lid, was_engaged: bool, engaged: bool, held_closed: bool) -> bool {
    if !was_engaged && !engaged {
        return false;
    }
    let closed = lid.closed();
    if was_engaged && !engaged && closed {
        tracing::info!("released with the lid closed — sleeping now");
        lid.sleep_now();
    }
    let now_held_closed = engaged && closed;
    if now_held_closed && !held_closed {
        tracing::info!("lid closed under a hold — locking the display");
        lid.lock_display();
    }
    now_held_closed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeLid {
        closed: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FakeLid {
        fn new(closed: bool) -> Self {
            FakeLid {
                closed,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Lid for FakeLid {
        fn closed(&self) -> bool {
            self.calls.borrow_mut().push("read");
            self.closed
        }
        fn lock_display(&self) {
            self.calls.borrow_mut().push("lock");
        }
        fn sleep_now(&self) {
            self.calls.borrow_mut().push("sleep");
        }
    }

    #[test]
    fn parse_needs_closed_and_causes_sleep() {
        let ioreg = |state: &str, causes: &str| {
            format!(
                "      \"AppleClamshellCausesSleep\" = {causes}\n      \"SleepDisabled\" = Yes\n      \"AppleClamshellState\" = {state}\n"
            )
        };
        assert!(parse_lid_closed(&ioreg("Yes", "Yes")));
        assert!(!parse_lid_closed(&ioreg("No", "Yes")), "lid open");
        assert!(
            !parse_lid_closed(&ioreg("Yes", "No")),
            "closed at a desk with an external display"
        );
        assert!(
            !parse_lid_closed(""),
            "no lid (desktop Mac) or a failed read"
        );
    }

    #[test]
    fn follow_up_table() {
        // (label, closed, was_engaged, engaged, held_closed) -> (calls, held_closed)
        type Case = (
            &'static str,
            bool,
            bool,
            bool,
            bool,
            &'static [&'static str],
            bool,
        );
        let cases: &[Case] = &[
            (
                "idle: lid is not even read",
                true,
                false,
                false,
                false,
                &[],
                false,
            ),
            (
                "holding, lid open",
                false,
                true,
                true,
                false,
                &["read"],
                false,
            ),
            (
                "lid closes under a hold: lock once",
                true,
                true,
                true,
                false,
                &["read", "lock"],
                true,
            ),
            (
                "still held and closed: no second lock",
                true,
                true,
                true,
                true,
                &["read"],
                true,
            ),
            (
                "hold engages under a closed lid: lock",
                true,
                false,
                true,
                false,
                &["read", "lock"],
                true,
            ),
            (
                "release with the lid closed: sleep",
                true,
                true,
                false,
                true,
                &["read", "sleep"],
                false,
            ),
            (
                "release with the lid open: nothing",
                false,
                true,
                false,
                false,
                &["read"],
                false,
            ),
            (
                "lid reopened under a hold: re-arm",
                false,
                true,
                true,
                true,
                &["read"],
                false,
            ),
        ];
        for (label, closed, was, now, held, want_calls, want_held) in cases {
            let lid = FakeLid::new(*closed);
            let got_held = follow_up(&lid, *was, *now, *held);
            assert_eq!(&lid.calls.borrow()[..], *want_calls, "{label}: calls");
            assert_eq!(got_held, *want_held, "{label}: held_closed");
        }
    }
}
