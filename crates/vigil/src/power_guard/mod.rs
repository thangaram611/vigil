//! Power guard abstraction + the pure thermal cooldown window (Phase 5.4).
//!
//! DEAD-FROM-RUST until 5.7: library-only. Nothing here is wired into a loop.
//!
//! ## Why `power_guard` and NOT `power`
//! `crates/vigil/src/power/` is RESERVED for the 5.5 port of `lib/pmset.sh` (the
//! privileged engage/release/reconcile state machine). Placing this read-only
//! thermal+battery guard there would entangle the two. The dependency is
//! one-directional: 5.5's `power` will DEPEND on `power_guard` (its
//! `recover_startup`/`can_hold` evaluates the 5.4 guard at startup) — no cycle,
//! no name clash.
//!
//! ## What lives here
//! - [`PowerGuard`] — the trait the 5.5 startup recovery consumes. A real
//!   env-driven impl ([`EnvPowerGuard`]) composes [`crate::thermal`] +
//!   [`crate::battery`]; tests can pass a fake.
//! - [`cooldown_state`] — the PURE sliding cooldown window ported verbatim from
//!   `bin/vigil-daemon` (COOLDOWN_UNTIL re-armed every pressure tick).

use serde::Serialize;

use crate::{battery, thermal};

/// The thermal+battery guard abstraction. `can_hold()` is true iff NEITHER the
/// thermal nor the battery guard wants to cut — i.e. it is safe to hold sleep
/// prevention. 5.5's `recover_startup` consumes this.
pub trait PowerGuard {
    /// True iff the thermal guard wants to cut (release sleep prevention).
    fn thermal_cut(&self) -> bool;
    /// True iff the battery guard wants to cut.
    fn battery_cut(&self) -> bool;
    /// True iff it is safe to hold sleep prevention (neither guard cuts).
    fn can_hold(&self) -> bool {
        !self.thermal_cut() && !self.battery_cut()
    }
}

/// Real env-driven guard. Reads `VIGIL_FORCE` + the fixture-or-pmset text via
/// the [`crate::thermal`] / [`crate::battery`] collectors. Honors the SAME env
/// seams as bash (`VIGIL_FORCE`, `VIGIL_THERMAL_FIXTURE`, `VIGIL_BATTERY_FIXTURE`)
/// and the resolved config knobs (`floor` = `VIGIL_THERMAL_CPU_LIMIT_FLOOR`,
/// `battery_floor_pct` = `VIGIL_BATTERY_FLOOR_PCT`).
pub struct EnvPowerGuard {
    /// `VIGIL_THERMAL_CPU_LIMIT_FLOOR` knob (None = unset = parity default).
    pub floor: Option<u32>,
    /// `VIGIL_BATTERY_FLOOR_PCT` (default 20).
    pub battery_floor_pct: u32,
}

impl EnvPowerGuard {
    /// Read `VIGIL_FORCE` the same way bash does: `1` => true, anything else
    /// (including unset) => false.
    fn force() -> bool {
        std::env::var("VIGIL_FORCE")
            .map(|v| v == "1")
            .unwrap_or(false)
    }
}

impl PowerGuard for EnvPowerGuard {
    fn thermal_cut(&self) -> bool {
        // VIGIL_FORCE FIRST, before any subprocess — bash returns "no cut" on
        // force before forking `pmset -g therm` (lib/thermal.sh). Short-circuit
        // the pmset read so a per-tick force never spawns pmset at 5.7.
        if Self::force() {
            return false;
        }
        thermal::live_should_cut(false, &thermal::read_therm_raw(), self.floor)
    }

    fn battery_cut(&self) -> bool {
        // VIGIL_FORCE FIRST, before any subprocess (lib/battery.sh).
        if Self::force() {
            return false;
        }
        battery::live_should_cut(false, &battery::read_ps_raw(), self.battery_floor_pct)
    }
}

/// READ-ONLY thermal+battery view for the `vigil debug` dump.
///
/// Reads `pmset -g therm` / `pmset -g ps` (or the fixtures) ONCE each, parses
/// them, and exposes the PARSED numeric throttle value + floor knobs + reframed
/// summaries. This is strictly read-only: no pmset transition, no file write, no
/// helper engage/release — it preserves the `vigil debug` read-only contract.
///
/// Force is intentionally NOT applied here: this is a diagnostic VIEW of the raw
/// thermal/battery signal, so the operator sees true pressure even under
/// `VIGIL_FORCE=1`. The cut decision in the oracle subcommands does honor force.
#[derive(Debug, Serialize)]
pub struct PowerView {
    // --- thermal ---
    /// True iff a `thermal warning level = ...` line is present.
    pub thermal_warning_present: bool,
    /// Parsed numeric `CPU_Scheduler_Limit` value (None if absent/non-numeric).
    /// This is the PARSED throttle value, NOT bash's `head -c 100` blob.
    pub cpu_scheduler_limit: Option<u32>,
    /// The `VIGIL_THERMAL_CPU_LIMIT_FLOOR` knob (None = unset = parity default).
    pub thermal_floor: Option<u32>,
    /// `pmset -g therm` produced no output.
    pub thermal_unavailable: bool,
    /// Reframed UX summary (e.g. "paused: thermal pressure ... after cooldown").
    pub thermal_summary: String,
    // --- battery ---
    /// "AC" | "battery" | "unknown".
    pub power_source: String,
    /// Parsed battery percentage (None if unparseable).
    pub battery_pct: Option<u32>,
    /// `VIGIL_BATTERY_FLOOR_PCT`.
    pub battery_floor_pct: u32,
    /// Reframed UX summary, e.g. "on battery 18% (floor 20%)".
    pub battery_summary: String,
}

impl PowerView {
    /// Read + parse both signals once (read-only). Force is NOT applied (this is
    /// a raw-signal diagnostic view).
    pub fn read(thermal_floor: Option<u32>, battery_floor_pct: u32) -> Self {
        let therm = thermal::parse_therm(&thermal::read_therm_raw());
        let batt = battery::parse_ps(&battery::read_ps_raw());
        let power_source = match batt.ac {
            battery::AcState::Ac => "AC",
            battery::AcState::Battery => "battery",
            battery::AcState::Unknown => "unknown",
        }
        .to_string();
        PowerView {
            thermal_warning_present: therm.warning_present,
            cpu_scheduler_limit: therm.cpu_scheduler_limit,
            thermal_floor,
            thermal_unavailable: therm.empty,
            thermal_summary: thermal::thermal_summary(&therm, thermal_floor),
            power_source,
            battery_pct: batt.pct,
            battery_floor_pct,
            battery_summary: battery::battery_summary(&batt, battery_floor_pct),
        }
    }
}

/// Pure sliding thermal-cooldown window. Ported verbatim from `bin/vigil-daemon`
/// (the COOLDOWN_UNTIL re-arm + `now < COOLDOWN_UNTIL` check).
///
/// - `now` / `prev_until` are epoch seconds.
/// - On `thermal_pressure`, the window is re-armed to `now + cooldown_secs`
///   (sliding: re-armed every pressure tick). Otherwise `prev_until` is kept.
/// - `cooling = now < new_until`.
///
/// `cooldown_secs` is INDEPENDENT of the tick cadence (the window is wall-clock).
/// The initial caller state `prev_until = 0` reproduces the daemon's
/// `COOLDOWN_UNTIL=0`. Library-only; the tick-loop wiring is a 5.7 obligation.
pub fn cooldown_state(
    now: i64,
    thermal_pressure: bool,
    prev_until: i64,
    cooldown_secs: u32,
) -> (i64, bool) {
    let new_until = if thermal_pressure {
        now + cooldown_secs as i64
    } else {
        prev_until
    };
    let cooling = now < new_until;
    (new_until, cooling)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── cooldown_state value table (ports daemon lines 64/151/153) ────────────

    #[test]
    fn cooldown_arms_on_pressure() {
        // (now=100, pressure=true, prev=0, secs=60) => (160, true)
        assert_eq!(cooldown_state(100, true, 0, 60), (160, true));
    }

    #[test]
    fn cooldown_expires_after_window() {
        // (now=170, pressure=false, prev=160, secs=60) => (160, false)
        assert_eq!(cooldown_state(170, false, 160, 60), (160, false));
    }

    #[test]
    fn cooldown_still_cooling_inside_window() {
        // (now=150, pressure=false, prev=160, secs=60) => (160, true)
        assert_eq!(cooldown_state(150, false, 160, 60), (160, true));
    }

    #[test]
    fn cooldown_initial_no_pressure() {
        // initial prev=0, no pressure => (0, false) — matches COOLDOWN_UNTIL=0.
        assert_eq!(cooldown_state(100, false, 0, 60), (0, false));
    }

    #[test]
    fn cooldown_re_arms_sliding_window() {
        // A later pressure tick re-arms from the NEW now, extending the window.
        let (until1, _) = cooldown_state(100, true, 0, 60); // 160
        let (until2, cooling2) = cooldown_state(150, true, until1, 60); // 210
        assert_eq!(until2, 210);
        assert!(cooling2);
    }

    #[test]
    fn cooldown_boundary_now_equals_until_not_cooling() {
        // now == until => now < until is false => not cooling (strict).
        assert_eq!(cooldown_state(160, false, 160, 60), (160, false));
    }

    // ── PowerGuard trait composition over a fake ──────────────────────────────

    struct FakeGuard {
        thermal: bool,
        battery: bool,
    }
    impl PowerGuard for FakeGuard {
        fn thermal_cut(&self) -> bool {
            self.thermal
        }
        fn battery_cut(&self) -> bool {
            self.battery
        }
    }

    #[test]
    fn can_hold_composition() {
        assert!(
            FakeGuard {
                thermal: false,
                battery: false
            }
            .can_hold()
        );
        assert!(
            !FakeGuard {
                thermal: true,
                battery: false
            }
            .can_hold()
        );
        assert!(
            !FakeGuard {
                thermal: false,
                battery: true
            }
            .can_hold()
        );
        assert!(
            !FakeGuard {
                thermal: true,
                battery: true
            }
            .can_hold()
        );
    }

    // ── EnvPowerGuard over fixtures + VIGIL_FORCE ─────────────────────────────
    //
    // Env-mutating: serialize against other env tests in this binary via a
    // module-local lock. (Other modules use their own locks; cross-binary env
    // races are avoided because cargo runs unit tests of one crate in one
    // process — the config module's ENV_LOCK is a separate static, so we keep a
    // local lock here for the env vars THIS test mutates.)
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn env_guard_reads_fixtures() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("VIGIL_THERMAL_FIXTURE", "thermal warning level = warning");
            std::env::set_var(
                "VIGIL_BATTERY_FIXTURE",
                "Now drawing from 'Battery Power' 5%; discharging",
            );
            std::env::remove_var("VIGIL_FORCE");
        }
        let g = EnvPowerGuard {
            floor: None,
            battery_floor_pct: 20,
        };
        assert!(g.thermal_cut(), "warning fixture => thermal cut");
        assert!(g.battery_cut(), "battery 5% < 20 => battery cut");
        assert!(!g.can_hold());
        unsafe {
            std::env::remove_var("VIGIL_THERMAL_FIXTURE");
            std::env::remove_var("VIGIL_BATTERY_FIXTURE");
        }
    }

    #[test]
    fn env_guard_force_overrides() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("VIGIL_THERMAL_FIXTURE", "thermal warning level = critical");
            std::env::set_var(
                "VIGIL_BATTERY_FIXTURE",
                "Now drawing from 'Battery Power' 1%; discharging",
            );
            std::env::set_var("VIGIL_FORCE", "1");
        }
        let g = EnvPowerGuard {
            floor: None,
            battery_floor_pct: 20,
        };
        assert!(!g.thermal_cut(), "force overrides thermal");
        assert!(!g.battery_cut(), "force overrides battery");
        assert!(g.can_hold(), "force => can hold");
        unsafe {
            std::env::remove_var("VIGIL_THERMAL_FIXTURE");
            std::env::remove_var("VIGIL_BATTERY_FIXTURE");
            std::env::remove_var("VIGIL_FORCE");
        }
    }
}
