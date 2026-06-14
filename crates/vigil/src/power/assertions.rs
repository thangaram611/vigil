//! pmset `-g assertions` tri-state parser — Phase 5.7 §2.3.3.
//!
//! READ-ONLY status-render code (NOT privilege-boundary code). Ports the bash
//! `vigil_assertions_summary` (`lib/pmset.sh:226-306`) into Rust so `vigil
//! status [--json]` can surface who is currently holding a power assertion.
//!
//! ## The three output states (Contract 4 §4a)
//! 1. **TSV rows** (≥1 holder) — one [`Assertion`] per `pid(process): … <type>`
//!    holder row; the `vigil` flag is set iff `pid == caffeinate_pid`.
//! 2. **`(none)`** ([`AssertionsSummary::None`]) — empty output / no `Listed by
//!    owning process:` header / a block with zero holder rows and zero
//!    non-matching rows.
//! 3. **`(parse-failed; raw output:)`** ([`AssertionsSummary::ParseFailed`]) — ≥1
//!    non-blank non-informational block row but NONE match the holder shape (the
//!    Apple-changed-the-schema sentinel). Carries the first 10 raw lines.
//!
//! ## Determinism seam
//! [`read_assertions_raw`] honors `VIGIL_ASSERTIONS_FIXTURE` by **presence**
//! (even an empty `""` value), mirroring the bash `_vigil_pmset_assertions`
//! `[[ -n "${VIGIL_ASSERTIONS_FIXTURE+x}" ]]` set-check — so an empty fixture
//! exercises the "pmset returned nothing → (none)" branch hermetically rather
//! than falling through to live `pmset`.
//!
//! ## LC_ALL=C / non-ASCII (Contract 4 §4a)
//! The bash forces the C locale so BSD awk treats the block as bytes (assertion
//! names can carry a Unicode apostrophe). Rust `&str` line iteration is already
//! byte-safe for our literal-anchor matching; we never do locale-dependent
//! character-class work, so the port is faithful without a locale shim.

/// One parsed assertion holder row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub pid: u32,
    pub process: String,
    pub atype: String,
    /// True iff this holder's pid equals our recorded caffeinate pid.
    pub vigil: bool,
}

/// The tri-state result of parsing `pmset -g assertions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionsSummary {
    /// ≥1 holder row parsed.
    Holders(Vec<Assertion>),
    /// No holders to enumerate (empty / no header / empty block / `No …` rows).
    None,
    /// Block had non-blank rows but none matched the holder shape. Carries the
    /// first 10 raw lines (for the `(parse-failed; raw output:)` render).
    ParseFailed { raw_head: Vec<String> },
}

impl AssertionsSummary {
    /// Map to the `power_assertions_state` enum string (Contract 4 §1d, §4a):
    /// holders → `"ok"`, none → `"none"`, parse-failed → `"parse_failed"`.
    pub fn state(&self) -> &'static str {
        match self {
            AssertionsSummary::Holders(_) => "ok",
            AssertionsSummary::None => "none",
            AssertionsSummary::ParseFailed { .. } => "parse_failed",
        }
    }
}

/// Read the raw `pmset -g assertions` snapshot using the bash PRESENCE seam: if
/// `VIGIL_ASSERTIONS_FIXTURE` is SET (even to `""`), return its value verbatim;
/// else run `pmset -g assertions` (stderr discarded), empty on any failure.
///
/// Note the deliberate divergence from the `VIGIL_THERMAL_FIXTURE` /
/// `VIGIL_BATTERY_FIXTURE` seams (which use `-n`, non-empty only): assertions
/// uses **presence** so an empty fixture is honored as "no assertions", matching
/// the bash `_vigil_pmset_assertions`.
pub fn read_assertions_raw() -> String {
    if let Some(fixture) = std::env::var_os("VIGIL_ASSERTIONS_FIXTURE") {
        return fixture.to_string_lossy().into_owned();
    }
    match std::process::Command::new("pmset")
        .args(["-g", "assertions"])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

/// Parse a `pmset -g assertions` blob plus our caffeinate pid into the tri-state
/// summary. Pure: no IO, fully unit-testable.
///
/// `our_pid` is the recorded caffeinate pid (None when no pidfile); a holder
/// whose pid matches gets the `vigil` flag.
pub fn parse_assertions(raw: &str, our_pid: Option<u32>) -> AssertionsSummary {
    // bash: `[[ -z "$raw" ]] → (none)`.
    if raw.is_empty() {
        return AssertionsSummary::None;
    }

    // Header absent → no holders to enumerate.
    if !raw
        .lines()
        .any(|l| l.starts_with("Listed by owning process:"))
    {
        return AssertionsSummary::None;
    }

    // Slice out the block: start AFTER the "Listed by owning process:" header,
    // end at "No new entries" / a fresh "Assertion status" section / EOF
    // (`flag` awk gate). Tolerates intra-block blank lines.
    let mut in_block = false;
    let mut matched: Vec<Assertion> = Vec::new();
    let mut non_matching = 0usize;

    for line in raw.lines() {
        if line.starts_with("Listed by owning process:") {
            in_block = true;
            continue;
        }
        if line.starts_with("No new entries") || line.starts_with("Assertion status") {
            in_block = false;
            // Do NOT `continue` past further lines outside the block; the awk
            // simply clears the flag and keeps scanning for a new section.
            continue;
        }
        if !in_block {
            continue;
        }

        // ── inside the block ──────────────────────────────────────────────
        // Skip fully-blank rows (bash `[[ -z "${line// /}" ]]` — whitespace
        // collapses to empty).
        if line.trim().is_empty() {
            continue;
        }
        let trimmed = trim_leading_ws(line);
        // Informational "No …" rows (e.g. "No assertions.") — skip silently.
        if trimmed.starts_with("No ") {
            continue;
        }
        // Deeply-indented (≥4 leading whitespace) = continuation of the previous
        // holder row (Details:, Timeout will fire …) — skip silently.
        if leading_ws_count(line) >= 4 {
            continue;
        }

        // Candidate holder row: must match the pid/process/type shape.
        if let Some(a) = parse_holder_row(line, our_pid) {
            matched.push(a);
        } else {
            non_matching += 1;
        }
    }

    if !matched.is_empty() {
        return AssertionsSummary::Holders(matched);
    }
    if non_matching > 0 {
        let raw_head: Vec<String> = raw.lines().take(10).map(|l| l.to_string()).collect();
        return AssertionsSummary::ParseFailed { raw_head };
    }
    AssertionsSummary::None
}

/// Number of leading ASCII-whitespace bytes (matches bash `[[:space:]]`).
fn leading_ws_count(line: &str) -> usize {
    line.bytes().take_while(|b| b.is_ascii_whitespace()).count()
}

/// Strip leading whitespace (bash `"${line#"${line%%[![:space:]]*}"}"`).
fn trim_leading_ws(line: &str) -> &str {
    line.trim_start_matches(|c: char| c.is_ascii_whitespace())
}

/// Match the holder regex `^\s*pid\s+([0-9]+)\(([^)]+)\):.*\]\s+[0-9:]+\s+([A-Za-z]+)`
/// (bash `pid_re`). Returns the parsed [`Assertion`] iff the row matches.
fn parse_holder_row(line: &str, our_pid: Option<u32>) -> Option<Assertion> {
    let rest = trim_leading_ws(line);
    // `pid` keyword followed by ≥1 whitespace.
    let rest = rest.strip_prefix("pid")?;
    if rest.is_empty() || !rest.as_bytes()[0].is_ascii_whitespace() {
        return None;
    }
    let rest = trim_leading_ws(rest);

    // `([0-9]+)` — the holder pid.
    let pid_end = rest.find(|c: char| !c.is_ascii_digit())?;
    if pid_end == 0 {
        return None;
    }
    let pid: u32 = rest[..pid_end].parse().ok()?;
    let rest = &rest[pid_end..];

    // `\(([^)]+)\)` — `(process)`.
    let rest = rest.strip_prefix('(')?;
    let close = rest.find(')')?;
    let process = &rest[..close];
    if process.is_empty() {
        return None;
    }
    let rest = &rest[close + 1..];

    // `:` immediately after the `)`.
    let rest = rest.strip_prefix(':')?;

    // `.*\]` — skip up to and including the FIRST `]` (the assertion id bracket).
    let bracket = rest.find(']')?;
    let rest = &rest[bracket + 1..];

    // `\s+[0-9:]+\s+` — ≥1 whitespace, a `[0-9:]+` timestamp, ≥1 whitespace.
    let rest = trim_leading_ws(rest);
    let ts_end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == ':'))
        .unwrap_or(rest.len());
    if ts_end == 0 {
        return None; // no timestamp token
    }
    let after_ts = &rest[ts_end..];
    // Require at least one whitespace separating the timestamp from the type.
    if after_ts.is_empty() || !after_ts.as_bytes()[0].is_ascii_whitespace() {
        return None;
    }
    let rest = trim_leading_ws(after_ts);

    // `([A-Za-z]+)` — the assertion type (leading alpha run).
    let atype_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    if atype_end == 0 {
        return None;
    }
    let atype = &rest[..atype_end];

    let vigil = our_pid == Some(pid);
    Some(Assertion {
        pid,
        process: process.to_string(),
        atype: atype.to_string(),
        vigil,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── The 8 ported `tests/assertions_test.sh` cases (Phase 5.7 §5.5, GAP #1) ──

    #[test]
    fn pure_none_golden_cases() {
        // The four pure-equality `parse_assertions(...) == None` golden cases
        // (mirror of tests/assertions_test.sh). Each fixture stays byte-identical
        // and individually labeled; this is a golden/parity set.
        let cases: &[(&str, &str)] = &[
            // test_empty_output_is_none — VIGIL_ASSERTIONS_FIXTURE="" → (none).
            ("empty_output_is_none", ""),
            // test_header_only_is_none — no "Listed by owning process:" block.
            (
                "header_only_is_none",
                "Assertion status system-wide:\n   PreventUserIdleSystemSleep    0\n   UserIsActive                  0",
            ),
            // test_block_present_but_empty_is_none — header but zero holder rows.
            (
                "block_present_but_empty_is_none",
                "Assertion status system-wide:\n   PreventUserIdleSystemSleep    0\nListed by owning process:\nNo new entries",
            ),
            // test_no_assertions_literal_is_none — "No assertions." informational
            // → (none), NOT parse-failed.
            (
                "no_assertions_literal_is_none",
                "Assertion status system-wide:\nListed by owning process:\n   No assertions.\nNo new entries",
            ),
        ];
        for (label, fixture) in cases {
            assert_eq!(
                parse_assertions(fixture, None),
                AssertionsSummary::None,
                "{label}"
            );
        }
    }

    #[test]
    fn single_assertion_holder_is_tsv() {
        // test_single_assertion_holder_is_tsv.
        let fixture = "Assertion status system-wide:\n   PreventUserIdleSystemSleep    1\nListed by owning process:\n  pid 200(loginwindow): [0x000049930006d2eb] 00:00:00 PreventUserIdleSystemSleep named: \"com.apple.loginwindow.assertion\"\nNo new entries";
        let got = parse_assertions(fixture, None);
        assert_eq!(
            got,
            AssertionsSummary::Holders(vec![Assertion {
                pid: 200,
                process: "loginwindow".to_string(),
                atype: "PreventUserIdleSystemSleep".to_string(),
                vigil: false,
            }])
        );
    }

    #[test]
    fn multi_holder_with_continuation_lines() {
        // test_multi_holder_with_continuation_lines — 3 holders; Details:/Timeout
        // continuation rows (≥4 indent) filtered.
        let fixture = "Assertion status system-wide:\n   PreventUserIdleSystemSleep    1\n   PreventUserIdleDisplaySleep   1\nListed by owning process:\n  pid 200(loginwindow): [0x000049930006d2eb] 00:00:00 PreventUserIdleSystemSleep named: \"com.apple.loginwindow.assertion\"\n    Details: blah blah continuation\n    Timeout will fire in 60 seconds Action=TimeoutActionRelease\n  pid 41(coreaudiod): [0x000049930006d2ec] 00:12:34 PreventUserIdleSystemSleep named: \"com.apple.audio.AudioServiceForApp\"\n  pid 9999(caffeinate): [0x000049930006d2ed] 00:00:01 PreventUserIdleDisplaySleep named: \"caffeinate command-line tool\"\nNo new entries";
        let got = parse_assertions(fixture, None);
        let AssertionsSummary::Holders(holders) = got else {
            panic!("expected holders, got {got:?}");
        };
        assert_eq!(holders.len(), 3, "three holders → three rows");
        assert_eq!(
            holders[0],
            Assertion {
                pid: 200,
                process: "loginwindow".to_string(),
                atype: "PreventUserIdleSystemSleep".to_string(),
                vigil: false
            }
        );
        assert_eq!(
            holders[1],
            Assertion {
                pid: 41,
                process: "coreaudiod".to_string(),
                atype: "PreventUserIdleSystemSleep".to_string(),
                vigil: false
            }
        );
        assert_eq!(
            holders[2],
            Assertion {
                pid: 9999,
                process: "caffeinate".to_string(),
                atype: "PreventUserIdleDisplaySleep".to_string(),
                vigil: false
            }
        );
        // Continuation lines must NOT have produced rows.
        assert!(!holders.iter().any(|a| a.process == "Details"));
    }

    #[test]
    fn our_caffeinate_pid_is_tagged() {
        // test_our_caffeinate_pid_is_tagged — pid 9999 == our caffeinate → vigil.
        let fixture = "Assertion status system-wide:\n   PreventUserIdleDisplaySleep   1\nListed by owning process:\n  pid 9999(caffeinate): [0x000049930006d2ed] 00:00:01 PreventUserIdleDisplaySleep named: \"caffeinate command-line tool\"\n  pid 200(loginwindow): [0x000049930006d2eb] 00:00:00 PreventUserIdleSystemSleep named: \"com.apple.loginwindow.assertion\"\nNo new entries";
        let got = parse_assertions(fixture, Some(9999));
        let AssertionsSummary::Holders(holders) = got else {
            panic!("expected holders, got {got:?}");
        };
        let vigil_row = holders.iter().find(|a| a.pid == 9999).unwrap();
        let other_row = holders.iter().find(|a| a.pid == 200).unwrap();
        assert!(vigil_row.vigil, "our caffeinate pid should be tagged");
        assert!(!other_row.vigil, "non-vigil pid should NOT be tagged");
    }

    #[test]
    fn malformed_block_is_parse_failed() {
        // test_malformed_block_is_parse_failed — pid-looking rows that miss the
        // shape (no bracket, no timestamp) → parse-failed + raw head.
        let fixture = "Assertion status system-wide:\n   PreventUserIdleSystemSleep    1\nListed by owning process:\n  pid_owner=200 name=loginwindow type=PreventUserIdleSystemSleep\n  pid_owner=41  name=coreaudiod  type=PreventUserIdleSystemSleep";
        let got = parse_assertions(fixture, None);
        let AssertionsSummary::ParseFailed { raw_head } = got else {
            panic!("expected parse-failed, got {got:?}");
        };
        assert!(
            raw_head.iter().any(|l| l.contains("pid_owner=200")),
            "raw output (first 10 lines) should include the malformed rows"
        );
        assert_eq!(parse_assertions(fixture, None).state(), "parse_failed");
    }

    // ── State-mapping + edge coverage ──────────────────────────────────────

    #[test]
    fn state_strings_map_correctly() {
        assert_eq!(AssertionsSummary::None.state(), "none");
        assert_eq!(
            AssertionsSummary::Holders(vec![Assertion {
                pid: 1,
                process: "x".into(),
                atype: "Y".into(),
                vigil: false
            }])
            .state(),
            "ok"
        );
        assert_eq!(
            AssertionsSummary::ParseFailed { raw_head: vec![] }.state(),
            "parse_failed"
        );
    }

    #[test]
    fn raw_head_caps_at_ten_lines() {
        let mut fixture = String::from("Listed by owning process:\n");
        for i in 0..20 {
            fixture.push_str(&format!("  broken row {i}\n"));
        }
        let AssertionsSummary::ParseFailed { raw_head } = parse_assertions(&fixture, None) else {
            panic!("expected parse-failed");
        };
        assert_eq!(raw_head.len(), 10, "raw head must cap at 10 lines");
    }

    #[test]
    fn non_ascii_process_name_parses() {
        // LC_ALL=C parity — a Unicode apostrophe in the process name must not
        // break the byte-anchored parse.
        let fixture = "Listed by owning process:\n  pid 77(John\u{2019}s Mouse): [0x0a] 00:00:01 PreventUserIdleDisplaySleep named: \"x\"\nNo new entries";
        let AssertionsSummary::Holders(h) = parse_assertions(fixture, None) else {
            panic!("expected holders");
        };
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].process, "John\u{2019}s Mouse");
    }
}
