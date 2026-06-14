//! Thermal cutoff guard — Rust port of `lib/thermal.sh` (Phase 5.4).
//!
//! DEAD-FROM-RUST until 5.7: this is library-only. Nothing here is wired into
//! the live bash daemon; the daemon's tick loop wiring is a 5.7 obligation.
//!
//! ## Parse / decision / collector seam (mirrors `procscan`)
//! - [`parse_therm`] is a pure parser of `pmset -g therm` text into a
//!   [`ThermalReading`]. It applies the SAME anchored per-line test bash uses:
//!   `^[[:space:]]*(CPU_Scheduler_Limit|thermal warning level)[[:space:]]*=`.
//!   The leading `[[:space:]]*` and the `[[:space:]]*` before `=` are
//!   LOAD-BEARING: the informational `Note: No thermal warning level has been
//!   recorded` lines have NO `=` after the keyword, so they MUST NOT match
//!   (a substring match without the `=` anchor is a false-positive bug).
//! - [`should_cut`] is the ONLY decision fn (env-free, parsed inputs + the
//!   optional floor knob). It encodes the parity-default-or-smarter policy.
//! - [`live_should_cut`] is the thin collector: it applies `VIGIL_FORCE` first,
//!   then reads the fixture-or-pmset text and runs `parse_therm` + `should_cut`.
//!
//! ## Floor policy (locked by the 5.4 design note)
//! - Floor `None` (the default / unset): cut iff the reading has ANY matching
//!   `=` line (`any_match`). This is provably byte-for-byte the bash `grep -q`
//!   behavior — see the `parity` unit tests below.
//! - Floor `Some(F)`: cut iff a thermal-warning line is present
//!   (`warning_present`, non-numeric pressure always cuts) OR a NUMERIC
//!   `CPU_Scheduler_Limit` value is below `F` (lower = more throttled). A
//!   numeric `CPU_Scheduler_Limit >= F` with no warning line is the ONLY case
//!   the smarter policy tolerates (minor throttle). This policy has NO bash
//!   counterpart and is asserted only by cargo unit tests, never against bash.

/// Parsed view of `pmset -g therm`, capturing exactly what the bash `=`-anchored
/// regex matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalReading {
    /// At least one matching `thermal warning level = ...` line was present.
    pub warning_present: bool,
    /// Numeric value of a matching `CPU_Scheduler_Limit = <n>` line, if the
    /// value after `=` parsed as an integer. `None` when absent OR non-numeric.
    pub cpu_scheduler_limit: Option<u32>,
    /// At least one anchored line matched (warning OR a CPU_Scheduler_Limit line,
    /// even if its value is non-numeric). This is the bash `grep -q` predicate
    /// and the unset-floor cut decision; kept SEPARATE from
    /// `cpu_scheduler_limit` so a non-numeric Scheduler_Limit value still cuts
    /// in unset mode (would otherwise under-cut vs bash).
    pub any_match: bool,
    /// `pmset -g therm` produced no output (trimmed empty).
    pub empty: bool,
}

/// The two keyword prefixes bash's regex alternation matches.
const KEYWORDS: [&str; 2] = ["CPU_Scheduler_Limit", "thermal warning level"];

/// Test one line against bash's anchor:
/// `^[[:space:]]*(KEYWORD)[[:space:]]*=`. Returns the matched keyword and the
/// trimmed value text after `=` when it matches, else `None`.
fn match_line(line: &str) -> Option<(&'static str, &str)> {
    // ^[[:space:]]* — strip leading ASCII/Unicode whitespace.
    let rest = line.trim_start();
    for kw in KEYWORDS {
        if let Some(after_kw) = rest.strip_prefix(kw) {
            // [[:space:]]* before '=' — strip whitespace, then require '='.
            let after_ws = after_kw.trim_start();
            if let Some(value) = after_ws.strip_prefix('=') {
                return Some((kw, value.trim()));
            }
        }
    }
    None
}

/// Pure parse of `pmset -g therm` text into a [`ThermalReading`].
///
/// Walks each line applying [`match_line`] (bash's exact anchored test). From
/// the matching lines it derives `warning_present`, `cpu_scheduler_limit`
/// (numeric only), and `any_match` (the bash `grep -q` predicate).
pub fn parse_therm(raw: &str) -> ThermalReading {
    let empty = raw.trim().is_empty();
    let mut warning_present = false;
    let mut cpu_scheduler_limit = None;
    let mut any_match = false;

    for line in raw.lines() {
        if let Some((kw, value)) = match_line(line) {
            any_match = true;
            match kw {
                "thermal warning level" => warning_present = true,
                // First numeric Scheduler_Limit wins (don't clobber a parsed
                // value with a later non-numeric line).
                "CPU_Scheduler_Limit" if cpu_scheduler_limit.is_none() => {
                    cpu_scheduler_limit = value.parse::<u32>().ok();
                }
                _ => {}
            }
        }
    }

    ThermalReading {
        warning_present,
        cpu_scheduler_limit,
        any_match,
        empty,
    }
}

/// Pure cut decision over a parsed reading and the optional floor knob.
///
/// - `floor == None` (unset / parity default): cut iff `any_match` — identical
///   to bash `grep -q`.
/// - `floor == Some(F)`: cut iff `warning_present` (non-numeric pressure always
///   cuts) OR a numeric `CPU_Scheduler_Limit < F`.
pub fn should_cut(r: &ThermalReading, floor: Option<u32>) -> bool {
    match floor {
        None => r.any_match,
        Some(f) => r.warning_present || r.cpu_scheduler_limit.is_some_and(|v| v < f),
    }
}

/// UX-only reframed summary for the read-only debug surface. NOT a parity
/// constraint — the cross-engine oracle checks the cut DECISION only, so this
/// string is free to diverge from bash's `head -c 100` blob. It surfaces the
/// PARSED numeric throttle value and the floor knob rather than the raw blob.
pub fn thermal_summary(r: &ThermalReading, floor: Option<u32>) -> String {
    if r.empty {
        return "unavailable".to_string();
    }
    if should_cut(r, floor) {
        let mut detail = String::new();
        if r.warning_present {
            detail.push_str("thermal warning present");
        }
        if let Some(v) = r.cpu_scheduler_limit {
            if !detail.is_empty() {
                detail.push_str("; ");
            }
            detail.push_str(&format!("CPU scheduler limit {v}%"));
        }
        if detail.is_empty() {
            detail.push_str("thermal pressure");
        }
        match floor {
            Some(f) => format!(
                "paused: thermal pressure from running agents ({detail}, floor {f}%); \
                 will resume after cooldown"
            ),
            None => format!(
                "paused: thermal pressure from running agents ({detail}); \
                 will resume after cooldown"
            ),
        }
    } else {
        "ok".to_string()
    }
}

/// Live collector (env seam). Honors `VIGIL_FORCE` first (force => no cut),
/// exactly like bash, then parses `raw` (the fixture-or-pmset text) and runs the
/// pure decision. `raw` is supplied by the caller so the read of `pmset -g
/// therm` vs `VIGIL_THERMAL_FIXTURE` is a single point of IO.
pub fn live_should_cut(force: bool, raw: &str, floor: Option<u32>) -> bool {
    if force {
        return false;
    }
    let reading = parse_therm(raw);
    should_cut(&reading, floor)
}

/// Read the raw `pmset -g therm` text using the SAME seam bash uses: if
/// `VIGIL_THERMAL_FIXTURE` is set to a NON-EMPTY value, return it verbatim; else
/// run `pmset -g therm` (stderr discarded), returning empty string on any
/// spawn/read failure (fail-safe: empty => no cut).
///
/// Bash's seam is `[[ -n "${VIGIL_THERMAL_FIXTURE:-}" ]]`, which treats a
/// SET-BUT-EMPTY env var as UNSET and falls through to live pmset. We mirror that
/// `-n`/`:-` quirk (non-empty only) so an empty fixture is hermetic against the
/// bash parity oracle rather than diverging (Rust empty-fixture => no cut while
/// bash falls through to a possibly-pressured live pmset).
pub fn read_therm_raw() -> String {
    if let Ok(fixture) = std::env::var("VIGIL_THERMAL_FIXTURE")
        && !fixture.is_empty()
    {
        return fixture;
    }
    std::process::Command::new("pmset")
        .args(["-g", "therm"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// The outcome of a thermal read, distinguishing "pmset ran and produced this
/// text" from "pmset could not be read at all". The fail-OPEN `read_therm_raw`
/// above collapses both into an empty string (no cut); this is the fail-CLOSED
/// view the daemon's HOLD decision uses so a sustained keep-awake is never held
/// while blind to thermal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThermRead {
    /// The `VIGIL_THERMAL_FIXTURE` value, or the stdout of a `pmset -g therm`
    /// that actually ran (exit 0). Parse it normally.
    Text(String),
    /// `pmset -g therm` could not be read — it failed to spawn (e.g. a
    /// fork-starved, overloaded machine — exactly when thermal pressure is most
    /// likely) or exited non-zero. The daemon treats this as a cut (fail closed).
    /// NOTE: an empty stdout from a *successful* pmset is `Text("")`, NOT
    /// `Unavailable` — a healthy `pmset -g therm` always prints something, so we
    /// reserve `Unavailable` for a genuine read failure and never cut on a
    /// quirky-but-working pmset.
    Unavailable,
}

/// Read thermal state as a [`ThermRead`]. Honors the same `VIGIL_THERMAL_FIXTURE`
/// seam as [`read_therm_raw`] (non-empty fixture wins). On the live path, a
/// spawn failure or a non-success exit yields [`ThermRead::Unavailable`].
pub fn read_therm() -> ThermRead {
    if let Ok(fixture) = std::env::var("VIGIL_THERMAL_FIXTURE")
        && !fixture.is_empty()
    {
        return ThermRead::Text(fixture);
    }
    match std::process::Command::new("pmset")
        .args(["-g", "therm"])
        .output()
    {
        Ok(o) if o.status.success() => {
            ThermRead::Text(String::from_utf8_lossy(&o.stdout).into_owned())
        }
        _ => ThermRead::Unavailable,
    }
}

/// PURE fail-closed cut decision over a [`ThermRead`]: an unreadable thermal
/// state cuts; readable text decides via the existing [`should_cut`] policy.
pub fn decide_cut(read: &ThermRead, floor: Option<u32>) -> bool {
    match read {
        ThermRead::Unavailable => true,
        ThermRead::Text(raw) => should_cut(&parse_therm(raw), floor),
    }
}

/// The daemon's fail-CLOSED thermal cut for a HOLD decision. `force` short-
/// circuits to no-cut WITHOUT reading pmset (preserving the no-fork-under-force
/// contract); otherwise reads thermal state and applies [`decide_cut`], so a
/// genuine read failure cuts the hold rather than silently sustaining it.
pub fn cut_thermal_failclosed(force: bool, floor: Option<u32>) -> bool {
    !force && decide_cut(&read_therm(), floor)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ports of tests/thermal_test.sh (default floor = None = parity) ────────

    #[test]
    fn no_warning_does_not_cut() {
        // test_no_warning_does_not_cut — the "No ..." informational lines have
        // no '=' so the anchor never matches.
        let fixture = "Note: No thermal warning level has been recorded\n\
                       Note: No performance warning level has been recorded\n\
                       Note: No CPU power status has been recorded";
        let r = parse_therm(fixture);
        assert!(!r.any_match, "no anchored line should match");
        assert!(!should_cut(&r, None), "must NOT cut");
    }

    #[test]
    fn thermal_warning_cuts() {
        // test_thermal_warning_cuts
        let r = parse_therm("thermal warning level = warning");
        assert!(r.warning_present);
        assert!(r.any_match);
        assert!(should_cut(&r, None), "explicit warning must cut");
    }

    #[test]
    fn cpu_scheduler_limit_cuts() {
        // test_cpu_scheduler_limit_cuts — only the Scheduler_Limit line matches
        // (CPU_Available_CPUs is NOT in the keyword alternation).
        let fixture = "CPU_Scheduler_Limit = 50\nCPU_Available_CPUs = 4";
        let r = parse_therm(fixture);
        assert_eq!(r.cpu_scheduler_limit, Some(50));
        assert!(r.any_match);
        assert!(!r.warning_present);
        assert!(
            should_cut(&r, None),
            "scheduler limit presence must cut (unset)"
        );
    }

    #[test]
    fn force_overrides() {
        // test_force_overrides — force checked BEFORE any parse.
        assert!(!live_should_cut(
            true,
            "thermal warning level = critical",
            None
        ));
    }

    #[test]
    fn fail_closed_unavailable_cuts() {
        // A genuine read failure cuts in BOTH floor modes (the never-blind-to-heat
        // invariant): an unreadable thermal state must never let a hold persist.
        assert!(decide_cut(&ThermRead::Unavailable, None));
        assert!(decide_cut(&ThermRead::Unavailable, Some(80)));
    }

    #[test]
    fn fail_closed_text_decides_normally() {
        // Readable text decides via the existing should_cut policy (unchanged).
        assert!(!decide_cut(
            &ThermRead::Text("Note: No thermal warning level has been recorded".into()),
            None
        ));
        assert!(decide_cut(
            &ThermRead::Text("thermal warning level = warning".into()),
            None
        ));
        // An empty stdout from a *successful* pmset is Text("") => no cut (NOT
        // treated as Unavailable), so we never false-cut on a quirky pmset.
        assert!(!decide_cut(&ThermRead::Text(String::new()), None));
    }

    #[test]
    fn fail_closed_force_never_reads_or_cuts() {
        // force short-circuits to no-cut WITHOUT reading pmset (no fork), so a
        // forced hold is never released by a (possibly failing) live read.
        assert!(!cut_thermal_failclosed(true, None));
        assert!(!cut_thermal_failclosed(true, Some(80)));
    }

    #[test]
    fn empty_output_does_not_cut() {
        // test_empty_output_does_not_cut
        let r = parse_therm("");
        assert!(r.empty);
        assert!(!r.any_match);
        assert!(!should_cut(&r, None));
        // whitespace-only is also empty/no-cut.
        let r2 = parse_therm("   \n\t\n");
        assert!(r2.empty);
        assert!(!should_cut(&r2, None));
    }

    // ── parity: floor=None == bash grep -q on every fixture class ─────────────

    #[test]
    fn unset_floor_matches_bash_grep_semantics() {
        // any_match <=> "at least one anchored line" <=> bash grep -q success.
        let cases: &[(&str, bool)] = &[
            ("Note: No thermal warning level has been recorded", false),
            ("thermal warning level = warning", true),
            ("CPU_Scheduler_Limit = 50\nCPU_Available_CPUs = 4", true),
            ("", false),
            ("   ", false),
            // CPU_Available_CPUs alone is NOT a keyword.
            ("CPU_Available_CPUs = 4", false),
        ];
        for (fixture, want) in cases {
            let r = parse_therm(fixture);
            assert_eq!(
                should_cut(&r, None),
                *want,
                "unset-floor cut for {fixture:?} should be {want}"
            );
        }
    }

    // ── hand-rolled anchor whitespace handling (bash [[:space:]]* allowances) ─

    #[test]
    fn anchor_tolerates_whitespace_padding() {
        // '  CPU_Scheduler_Limit  = 50' — leading WS + WS before '='.
        let r = parse_therm("  CPU_Scheduler_Limit  = 50");
        assert_eq!(r.cpu_scheduler_limit, Some(50));
        assert!(r.any_match);
        assert!(should_cut(&r, None));
        // tab padding too.
        let r2 = parse_therm("\tthermal warning level\t= warning");
        assert!(r2.warning_present);
        assert!(should_cut(&r2, None));
    }

    #[test]
    fn keyword_without_equals_does_not_match() {
        // The keyword appears but there's no '=' — must NOT match (the bash
        // false-positive guard). e.g. an informational line that mentions the
        // keyword in prose.
        let r = parse_therm("Note: No CPU_Scheduler_Limit has been recorded");
        assert!(!r.any_match, "no '=' after keyword => no match");
        assert!(!should_cut(&r, None));
    }

    // ── SET-floor smarter policy (NO bash counterpart; cargo-only) ────────────

    #[test]
    fn set_floor_cuts_below_floor() {
        // CPU_Scheduler_Limit=50, floor=80 => 50 < 80 => CUT.
        let r = parse_therm("CPU_Scheduler_Limit = 50");
        assert!(should_cut(&r, Some(80)));
    }

    #[test]
    fn set_floor_tolerates_minor_throttle() {
        // CPU_Scheduler_Limit=90, floor=80, no warning => 90 >= 80 => NO cut.
        let r = parse_therm("CPU_Scheduler_Limit = 90");
        assert!(!should_cut(&r, Some(80)));
    }

    #[test]
    fn set_floor_warning_always_cuts() {
        // floor set, Scheduler_Limit tolerable BUT a warning line is present =>
        // warning always cuts.
        let r = parse_therm("CPU_Scheduler_Limit = 90\nthermal warning level = warning");
        assert!(r.warning_present);
        assert!(should_cut(&r, Some(80)));
    }

    #[test]
    fn set_floor_nonnumeric_scheduler_limit_does_not_cut_alone() {
        // floor set, Scheduler_Limit value non-numeric, no warning. In SET mode
        // a non-numeric limit (cpu_scheduler_limit == None) does NOT cut on its
        // own (no warning, no numeric < F). NOTE: in UNSET mode any_match still
        // cuts (parity), asserted separately below.
        let r = parse_therm("CPU_Scheduler_Limit = n/a");
        assert_eq!(r.cpu_scheduler_limit, None);
        assert!(r.any_match, "anchored line still matched (any_match)");
        assert!(
            !should_cut(&r, Some(80)),
            "no warning, no numeric < F => no cut"
        );
        assert!(
            should_cut(&r, None),
            "but UNSET mode cuts on any_match (parity)"
        );
    }

    #[test]
    fn set_floor_boundary_equal_does_not_cut() {
        // value == floor => NOT below floor => NO cut (strict '<').
        let r = parse_therm("CPU_Scheduler_Limit = 80");
        assert!(!should_cut(&r, Some(80)));
    }

    // ── summary (UX, not parity) ──────────────────────────────────────────────

    #[test]
    fn summary_strings() {
        assert_eq!(thermal_summary(&parse_therm(""), None), "unavailable");
        assert_eq!(
            thermal_summary(&parse_therm("CPU_Available_CPUs = 4"), None),
            "ok"
        );
        let cut = thermal_summary(&parse_therm("CPU_Scheduler_Limit = 50"), None);
        assert!(cut.contains("paused"), "cut summary reframed: {cut}");
        assert!(cut.contains("50%"), "exposes parsed numeric value: {cut}");
    }
}
