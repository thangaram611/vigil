//! Battery guard — Rust port of `lib/battery.sh` (Phase 5.4).
//!
//! DEAD-FROM-RUST until 5.7: library-only. Nothing here is wired into the live
//! bash daemon.
//!
//! ## TOCTOU fix (invariant)
//! Bash forks `pmset -g ps` TWICE today (`vigil_battery_on_battery` reads it,
//! `vigil_battery_pct` reads it AGAIN), which races the AC/battery state against
//! the percentage. This port COLLAPSES both into ONE snapshot read
//! ([`read_ps_raw`]) parsed once by [`parse_ps`]. The cut DECISION is unchanged,
//! so the parity oracle still agrees — only the double-fork race is removed.
//!
//! ## Parse / decision / collector seam
//! - [`parse_ps`] — pure parse of one `pmset -g ps` snapshot into a
//!   [`BatteryReading`] (AC state + percent).
//! - [`should_cut`] — pure decision (STRICT `<` floor; AC/unknown/empty-pct =>
//!   no cut).
//! - [`live_should_cut`] — thin collector over the single `pmset -g ps` snapshot.

/// Power source as reported by `pmset -g ps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcState {
    /// "AC Power" present in the snapshot.
    Ac,
    /// "Battery Power" present (and not AC).
    Battery,
    /// Neither marker present.
    Unknown,
}

/// Parsed view of one `pmset -g ps` snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatteryReading {
    pub ac: AcState,
    /// First `[0-9]+%` integer match across the whole snapshot. `None` if
    /// unparseable (matches the bash awk, which scans the whole output).
    pub pct: Option<u32>,
}

/// Parse one `pmset -g ps` snapshot.
///
/// - `ac`: contains "AC Power" => `Ac`; else contains "Battery Power" =>
///   `Battery`; else `Unknown`. (Matches the bash `case` order: AC checked
///   first.)
/// - `pct`: first `[0-9]+%` integer match ANYWHERE in the snapshot — identical
///   to the bash `awk 'match($0, /[0-9]+%/)'` which scans line-by-line top to
///   bottom and exits on the first hit. Scanning the whole text left-to-right
///   yields the same first occurrence.
pub fn parse_ps(raw: &str) -> BatteryReading {
    let ac = if raw.contains("AC Power") {
        AcState::Ac
    } else if raw.contains("Battery Power") {
        AcState::Battery
    } else {
        AcState::Unknown
    };
    BatteryReading {
        ac,
        pct: first_pct(raw),
    }
}

/// First `[0-9]+%` integer in `raw`, scanning left to right (matches the bash
/// awk's first-match-then-exit). Returns the integer BEFORE the `%`.
fn first_pct(raw: &str) -> Option<u32> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // A run of digits immediately followed by '%' is a match.
            if i < bytes.len() && bytes[i] == b'%' {
                // SAFETY-free: ASCII digit slice is valid UTF-8.
                return raw[start..i].parse::<u32>().ok();
            }
            // else: keep scanning AFTER this digit run (i already advanced).
        } else {
            i += 1;
        }
    }
    None
}

/// Pure cut decision.
///
/// - `Ac` / `Unknown` => never cut for battery reasons.
/// - `Battery`: `None` pct => no cut (unparseable, fail-safe); else cut iff
///   `pct < floor_pct` (STRICT `<`, so exactly == floor does NOT cut).
pub fn should_cut(r: &BatteryReading, floor_pct: u32) -> bool {
    match r.ac {
        AcState::Ac | AcState::Unknown => false,
        AcState::Battery => match r.pct {
            None => false,
            Some(p) => p < floor_pct,
        },
    }
}

/// UX summary. NOT a parity constraint. `'?'` when pct is `None`.
pub fn battery_summary(r: &BatteryReading, floor_pct: u32) -> String {
    let pct = r
        .pct
        .map(|p| p.to_string())
        .unwrap_or_else(|| "?".to_string());
    match r.ac {
        AcState::Battery => format!("on battery {pct}% (floor {floor_pct}%)"),
        AcState::Ac => format!("AC {pct}%"),
        AcState::Unknown => "unknown".to_string(),
    }
}

/// Live collector (env seam). Parses the single `raw` snapshot and runs the
/// pure decision.
pub fn live_should_cut(raw: &str, floor_pct: u32) -> bool {
    should_cut(&parse_ps(raw), floor_pct)
}

/// Read the raw `pmset -g ps` snapshot ONCE using the bash seam: if
/// `VIGIL_BATTERY_FIXTURE` is set to a NON-EMPTY value, return it verbatim; else
/// run `pmset -g ps`. Empty on any failure (fail-safe: empty => Unknown => no
/// cut).
///
/// Bash's seam is `[[ -n "${VIGIL_BATTERY_FIXTURE:-}" ]]`, which treats a
/// SET-BUT-EMPTY env var as UNSET and falls through to live pmset. We mirror that
/// `-n`/`:-` quirk (non-empty only) so an empty fixture stays hermetic against the
/// bash parity oracle.
pub fn read_ps_raw() -> String {
    if let Ok(fixture) = std::env::var("VIGIL_BATTERY_FIXTURE")
        && !fixture.is_empty()
    {
        return fixture;
    }
    std::process::Command::new("pmset")
        .args(["-g", "ps"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures byte-identical in shape to tests/battery_test.sh.
    const AC_100: &str = "Now drawing from 'AC Power'\n \
        -InternalBattery-0 (id=1)\t100%; charged; 0:00 remaining present: true";
    const BATTERY_5: &str = "Now drawing from 'Battery Power'\n \
        -InternalBattery-0 (id=1)\t5%; discharging; 0:30 remaining present: true";
    const BATTERY_50: &str = "Now drawing from 'Battery Power'\n \
        -InternalBattery-0 (id=1)\t50%; discharging; 5:00 remaining present: true";

    // ── ports of tests/battery_test.sh (default floor 20) ─────────────────────

    #[test]
    fn live_should_cut_over_fixtures() {
        // One live_should_cut decision per fixture (floor 20): on-AC and a healthy
        // 50% never cut; a 5% discharge cuts. (ports test_ac_does_not_cut /
        // test_battery_low_cuts / test_battery_50_does_not_cut.)
        let cases: &[(&str, bool, &str)] = &[
            (AC_100, false, "AC 100% => no cut"),
            (BATTERY_5, true, "battery 5% < 20 => cut"),
            (BATTERY_50, false, "battery 50% >= 20 => no cut"),
        ];
        for (fixture, want, label) in cases {
            assert_eq!(live_should_cut(fixture, 20), *want, "{label}");
        }
    }

    #[test]
    fn pct_parser() {
        // test_pct_parser: AC fixture => 100, 5% fixture => 5.
        assert_eq!(parse_ps(AC_100).pct, Some(100));
        assert_eq!(parse_ps(BATTERY_5).pct, Some(5));
        assert_eq!(parse_ps(BATTERY_50).pct, Some(50));
    }

    #[test]
    fn battery_summary_labels_source() {
        // battery_summary surfaces the power-source word for each fixture
        // (ports test_ac_summary_says_ac / test_battery_summary_says_battery).
        let cases: &[(&str, &str)] = &[(AC_100, "AC"), (BATTERY_50, "battery")];
        for (fixture, want) in cases {
            let s = battery_summary(&parse_ps(fixture), 20);
            assert!(
                s.contains(want),
                "summary for {fixture:?} should contain {want:?}: {s}"
            );
        }
    }

    // ── boundary + edge cases ─────────────────────────────────────────────────

    #[test]
    fn boundary_exactly_floor_does_not_cut() {
        // Exactly 20% with floor 20 => NO cut (STRICT '<').
        let r = BatteryReading {
            ac: AcState::Battery,
            pct: Some(20),
        };
        assert!(!should_cut(&r, 20), "20 < 20 is false => no cut");
        // one below floor cuts.
        let r19 = BatteryReading {
            ac: AcState::Battery,
            pct: Some(19),
        };
        assert!(should_cut(&r19, 20));
    }

    #[test]
    fn unknown_source_does_not_cut() {
        let r = parse_ps("some unrelated text with no power marker 9%");
        assert_eq!(r.ac, AcState::Unknown);
        assert!(!should_cut(&r, 20), "unknown source => no cut");
    }

    #[test]
    fn on_battery_empty_pct_does_not_cut() {
        // Battery Power present but no parseable percentage.
        let r =
            parse_ps("Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1) present: true");
        assert_eq!(r.ac, AcState::Battery);
        assert_eq!(r.pct, None);
        assert!(!should_cut(&r, 20), "empty pct => no cut (fail-safe)");
    }

    #[test]
    fn ac_marker_wins_over_battery_marker() {
        // bash case checks "AC Power" first; if both markers appear AC wins.
        let r = parse_ps("AC Power ... Battery Power ... 80%");
        assert_eq!(r.ac, AcState::Ac);
    }

    #[test]
    fn first_pct_scans_whole_snapshot() {
        // first /[0-9]+%/ anywhere — even if it precedes the battery line.
        assert_eq!(first_pct("foo 7% bar 99%"), Some(7));
        // a bare number without '%' is skipped; the next '%'-suffixed run wins.
        assert_eq!(first_pct("id=12345 then 42%"), Some(42));
        assert_eq!(first_pct("no percent here"), None);
    }
}
