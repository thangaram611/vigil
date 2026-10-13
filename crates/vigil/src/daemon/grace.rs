//! Monotonic release grace. Evidence demand and this policy hold stay separate.
use std::time::Duration;

const RELEASE_GRACE: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
pub struct Grace {
    previous_demand: bool,
    armed: bool,
    until: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraceDecision {
    pub automatic_hold: bool,
    pub remaining_secs: u32,
}

impl Grace {
    /// `now` is elapsed monotonic time. `consume` is manual-off, a safety
    /// cut/cooldown, or refresh with no automatic demand. Refresh during ongoing
    /// work consumes no future grace. Pass actual demand, before manual policy.
    pub fn evaluate(&mut self, now: Duration, automatic: bool, consume: bool) -> GraceDecision {
        if consume {
            self.until = None;
            self.armed = false;
        }
        if automatic {
            // Only new evidence can re-arm a consumed cycle. Clearing a guard
            // with the same work still present must not restore an old grace.
            if !self.previous_demand && !consume {
                self.armed = true;
            }
            self.until = None;
        } else if self.previous_demand {
            if self.armed && !consume {
                self.until = Some(now + RELEASE_GRACE);
            }
            self.armed = false;
        }
        self.previous_demand = automatic;
        let remaining = self
            .until
            .map(|until| until.saturating_sub(now))
            .unwrap_or_default();
        if remaining.is_zero() {
            self.until = None;
        }
        let remaining_secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() != 0);
        GraceDecision {
            automatic_hold: automatic || remaining_secs > 0,
            remaining_secs: remaining_secs as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    #[test]
    fn startup_has_no_grace_and_observed_loss_has_exactly_thirty_seconds() {
        let mut g = Grace::default();
        assert_eq!(g.evaluate(at(0), false, false).remaining_secs, 0);
        assert!(g.evaluate(at(2), true, false).automatic_hold);
        assert_eq!(g.evaluate(at(5), false, false).remaining_secs, 30);
        assert_eq!(g.evaluate(at(34), false, false).remaining_secs, 1);
        assert_eq!(
            g.evaluate(at(35), false, false),
            GraceDecision {
                automatic_hold: false,
                remaining_secs: 0
            }
        );
        assert!(!g.evaluate(at(100), false, false).automatic_hold);
    }

    #[test]
    fn refresh_consumes_grace_and_later_ticks_cannot_restore_it() {
        let mut g = Grace::default();
        g.evaluate(at(0), true, false);
        g.evaluate(at(1), false, false);
        assert!(!g.evaluate(at(2), false, true).automatic_hold);
        assert!(!g.evaluate(at(3), false, false).automatic_hold);
        assert!(!g.evaluate(at(4), false, true).automatic_hold);
        g.evaluate(at(5), true, false);
        assert_eq!(g.evaluate(at(6), false, false).remaining_secs, 30);
    }

    #[test]
    fn consumed_cycle_preserves_work_without_resurrecting_old_grace() {
        let mut g = Grace::default();
        g.evaluate(at(0), true, false);
        assert!(g.evaluate(at(1), true, true).automatic_hold);
        assert!(g.evaluate(at(2), true, false).automatic_hold);
        assert!(!g.evaluate(at(3), false, false).automatic_hold);
    }

    #[test]
    fn guards_and_manual_off_cannot_resurrect_old_grace() {
        let mut g = Grace::default();
        g.evaluate(at(0), true, false);
        g.evaluate(at(1), true, true);
        g.evaluate(at(2), true, false);
        assert!(!g.evaluate(at(3), false, false).automatic_hold);
        g.evaluate(at(4), true, false);
        assert_eq!(g.evaluate(at(5), false, false).remaining_secs, 30);
        assert!(!g.evaluate(at(6), false, true).automatic_hold);
        assert!(!g.evaluate(at(7), false, false).automatic_hold);
    }

    #[test]
    fn fractional_remaining_seconds_round_up_and_same_time_refresh_wins() {
        let mut g = Grace::default();
        g.evaluate(at(0), true, false);
        g.evaluate(at(1), false, false);
        assert_eq!(
            g.evaluate(Duration::from_millis(30_999), false, false)
                .remaining_secs,
            1
        );
        assert!(
            !g.evaluate(Duration::from_millis(30_999), false, true)
                .automatic_hold
        );
    }
    #[test]
    fn refresh_during_real_work_does_not_consume_future_grace() {
        let mut g = Grace::default();
        g.evaluate(at(0), true, false);
        // Caller consumes refresh only when actual automatic demand is zero.
        assert!(g.evaluate(at(1), true, false).automatic_hold);
        assert_eq!(g.evaluate(at(2), false, false).remaining_secs, 30);
    }
}
