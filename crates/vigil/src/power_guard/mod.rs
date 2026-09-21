//! The read-only thermal+battery view for `vigil debug`, and the pure thermal
//! cooldown window the daemon applies each tick.

use serde::Serialize;

use crate::{battery, thermal};

/// READ-ONLY thermal+battery view for the `vigil debug` dump.
///
/// Reads `pmset -g therm` / `pmset -g ps` (or the fixtures) ONCE each, parses
/// them, and exposes the PARSED numeric throttle value + floor knobs + reframed
/// summaries. This is strictly read-only: no pmset transition, no file write, no
/// helper engage/release — it preserves the `vigil debug` read-only contract.
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
    /// Read + parse both signals once (read-only): a raw-signal diagnostic view.
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
    fn cooldown_state_value_table() {
        // Pure (now, pressure, prev_until, secs) -> (until, cooling). One row per
        // former single-assert test, including the strict now==until boundary. The
        // stateful sliding-window re-arm (which chains two calls) stays separate.
        let cases: &[(i64, bool, i64, u32, i64, bool, &str)] = &[
            (100, true, 0, 60, 160, true, "arms on pressure"),
            (170, false, 160, 60, 160, false, "expires after window"),
            (
                150,
                false,
                160,
                60,
                160,
                true,
                "still cooling inside window",
            ),
            (
                100,
                false,
                0,
                60,
                0,
                false,
                "initial no pressure (COOLDOWN_UNTIL=0)",
            ),
            (
                160,
                false,
                160,
                60,
                160,
                false,
                "now==until => not cooling (strict)",
            ),
        ];
        for (now, pressure, prev, secs, until, cooling, label) in cases {
            assert_eq!(
                cooldown_state(*now, *pressure, *prev, *secs),
                (*until, *cooling),
                "{label}"
            );
        }
    }

    #[test]
    fn cooldown_re_arms_sliding_window() {
        // A later pressure tick re-arms from the NEW now, extending the window.
        let (until1, _) = cooldown_state(100, true, 0, 60); // 160
        let (until2, cooling2) = cooldown_state(150, true, until1, 60); // 210
        assert_eq!(until2, 210);
        assert!(cooling2);
    }
}
