//! `vigil status` — native render on the unified CheckEngine (Phase 5.7 §5.1–5.3).
//!
//! Three surfaces from ONE [`vigil::check::CheckEngine`] run:
//!   - `--json` → the byte-stable [`StatusSnapshot::to_json`] emitter (the only
//!     allowed diff vs the bash golden is the prepended `"version": 1,` key).
//!   - default text → service / activity / power blocks + the `expected_hold`
//!     sub-state + the suppressible `--verbose` hint.
//!   - `--verbose` → the above plus provider-root rows and the raw assertion rows.
//!
//! Exit is ALWAYS 0 on the non-usage path; only a flag violation dies with the
//! usage message and exit 1 (status NEVER returns 64 — that is top-level dispatch).

use std::ffi::OsString;

use vigil::check::{CheckEngine, CheckMode, DaemonScanState, StatusSnapshot, TriState};

use super::{load_config_or_exit, now_unix};

/// Parsed `status` invocation mode.
enum Mode {
    Json,
    Text { verbose: bool },
}

/// Print the usage line to stderr and exit 1 (bash `die`). Status usage errors are
/// operational failures (exit 1), distinct from the top-level clap unknown-command
/// path (exit 64).
fn usage_die() -> ! {
    anstream::eprintln!("usage: vigil status [--json|--verbose]");
    std::process::exit(super::EX_ERROR);
}

/// Parse the (already clap-collected) trailing args into a [`Mode`]. The grammar
/// is bash-faithful: `--json` OR `--verbose` OR nothing; anything else dies.
fn parse(args: &[OsString]) -> Mode {
    match args.len() {
        0 => Mode::Text { verbose: false },
        1 => match args[0].to_str() {
            Some("--json") => Mode::Json,
            Some("--verbose") => Mode::Text { verbose: true },
            _ => usage_die(),
        },
        _ => usage_die(),
    }
}

/// Entry point for the `Status` dispatch arm. Returns `!` (always exits).
pub fn run(args: Vec<OsString>) -> ! {
    let mode = parse(&args);
    let cfg = load_config_or_exit();
    let now = now_unix();
    let report = CheckEngine::run(&cfg, CheckMode::Status, now);
    let snap = &report.snapshot;

    match mode {
        Mode::Json => {
            // Byte-stable schema (reuse the frozen emitter verbatim — plain print,
            // no ANSI: machine output that must match the golden byte-for-byte).
            print!("{}", snap.to_json());
        }
        Mode::Text { verbose } => {
            render_text(snap, verbose);
        }
    }
    std::process::exit(0);
}

// ── text render (byte-faithful to bash `cmd_status`) ──────────────────────────

/// The daemon-scan human text for each of the six states (bash
/// `cmd_daemon_scan_info` column 3). `age` is the `daemon_scan_age_secs` (only set
/// for stale/fresh).
fn scan_text(state: DaemonScanState, age: Option<i64>) -> String {
    match state {
        DaemonScanState::Unloaded => "not running".to_string(),
        DaemonScanState::Starting => "starting (pid pending)".to_string(),
        DaemonScanState::Pending => "pending first scan".to_string(),
        DaemonScanState::Missing => "scan snapshot missing (run 'vigil reload')".to_string(),
        DaemonScanState::Stale => format!("stale ({}s ago)", age.unwrap_or(0)),
        DaemonScanState::Fresh => format!("{}s ago", age.unwrap_or(0)),
    }
}

/// `loaded`→`yes`/`no` (bash literal text).
fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// The per-agent `agents:` line (bash `cmd_agent_states_line_from_flags`).
fn agents_line(snap: &StatusSnapshot) -> String {
    let t = |s: TriState| s.as_str();
    format!(
        "claude={}  codex={}  copilot={}  vscode_copilot_chat={}",
        t(snap.agent_claude),
        t(snap.agent_codex),
        t(snap.agent_copilot),
        t(snap.agent_vscode_copilot_chat),
    )
}

/// The `assertions:` summary line (bash `cmd_power_assertions_summary_line`):
/// `none` | `parse failed` | `<N> active` | `<N> active (<M> vigil)`.
fn assertions_summary_line(snap: &StatusSnapshot) -> String {
    match snap.power_assertions_state.as_str() {
        "none" => "none".to_string(),
        "parse_failed" => "parse failed".to_string(),
        _ => {
            let total = snap.power_assertions.len();
            if total == 0 {
                "none".to_string()
            } else {
                let vigil = snap.power_assertions.iter().filter(|a| a.vigil).count();
                if vigil > 0 {
                    format!("{total} active ({vigil} vigil)")
                } else {
                    format!("{total} active")
                }
            }
        }
    }
}

/// The labeled power-hold summary line (bash `cmd_power_hold_summary_line`):
/// `hold=…  mode=best-effort  disablesleep=N  baseline=X  caffeinate=yes/no [pid=N]`.
fn power_hold_line(snap: &StatusSnapshot) -> String {
    // The summary `hold` gate matches bash `vigil_pmset_hold_engaged`:
    // caffeinate alive, OR (baseline file present AND disablesleep==1).
    let engaged =
        snap.caffeinate_alive || (snap.baseline.is_some() && snap.pmset_disablesleep == 1);
    let hold = if engaged { "engaged" } else { "released" };
    let baseline = match snap.baseline {
        Some(v) => v.to_string(),
        None => "-".to_string(),
    };
    let caff_alive = yes_no(snap.caffeinate_alive);
    let mut line = format!(
        "hold={}  mode={}  disablesleep={}  baseline={}  caffeinate={}",
        hold, snap.power_hold_mode, snap.pmset_disablesleep, baseline, caff_alive,
    );
    if let Some(pid) = snap.caffeinate_pid {
        line.push_str(&format!(" pid={pid}"));
    }
    line
}

/// The `expected_hold` sub-state (§5.3) — computed only when work exists but no
/// hold is engaged, priority-ordered exactly as bash. `cut_*` are LIVE decisions
/// (the snapshot carries `cut_thermal`/`cut_battery` from live `should_cut`).
fn expected_hold(snap: &StatusSnapshot) -> Option<&'static str> {
    // The expected-hold gate matches bash: disablesleep==1 OR caffeinate alive
    // (NOT baseline-gated, distinct from the summary-line hold above).
    let hold_engaged = snap.pmset_disablesleep == 1 || snap.caffeinate_alive;
    let work = snap.refcount_active > 0 || snap.pending_active_matches > 0;
    if !work || hold_engaged {
        return None;
    }
    Some(if snap.cut_thermal {
        "blocked by thermal cutoff"
    } else if snap.cut_battery {
        "blocked by battery floor"
    } else if !snap.launchd_loaded {
        "pending (LaunchAgent is not loaded)"
    } else if matches!(
        snap.daemon_scan_state,
        DaemonScanState::Starting | DaemonScanState::Pending
    ) {
        "pending (daemon first scan has not completed)"
    } else if matches!(
        snap.daemon_scan_state,
        DaemonScanState::Stale | DaemonScanState::Missing
    ) {
        "pending (daemon scan is unavailable; try 'vigil reload')"
    } else if snap.pending_active_matches > 0 {
        "pending (live matches are waiting for the next daemon scan)"
    } else {
        "pending (daemon/helper transition in progress)"
    })
}

/// `launchctl print system/{helper_label}` → loaded / not loaded (bash
/// `cmd_helper_launchd_status`). Read-only probe; not a privilege-boundary path.
fn helper_launchd_status() -> &'static str {
    let target = format!("system/{}", vigil::service::HELPER_LABEL);
    let ok = std::process::Command::new("launchctl")
        .args(["print", &target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok { "loaded" } else { "not loaded" }
}

/// Render the full plain/verbose status text (byte-faithful to bash `cmd_status`).
/// Uses anstream println so `--color` stripping is honored; the operational
/// strings + spacing are identical to the captured golden.
fn render_text(snap: &StatusSnapshot, verbose: bool) {
    let daemon_pid = match snap.daemon_pid {
        Some(p) => p.to_string(),
        None => "-".to_string(),
    };
    let scan = scan_text(snap.daemon_scan_state, snap.daemon_scan_age_secs);
    let helper = helper_launchd_status();

    anstream::println!("vigil status");
    anstream::println!();
    anstream::println!("  service");
    anstream::println!("    launchd:      {}", yes_no(snap.launchd_loaded));
    anstream::println!("    daemon pid:    {daemon_pid}");
    anstream::println!("    scan:          {scan}");
    anstream::println!("    root helper:   {helper}");
    anstream::println!();
    anstream::println!("  activity");
    anstream::println!(
        "    refcount:      {} active / {} total (idle window {}m)",
        snap.refcount_active,
        snap.refcount_total,
        snap.idle_window_minutes,
    );
    anstream::println!("    agents:        {}", agents_line(snap));
    if snap.pending_active_matches > 0 {
        anstream::println!(
            "    pending scan:  {} live match(es) not counted yet",
            snap.pending_active_matches,
        );
    }
    anstream::println!();
    anstream::println!("  power");
    anstream::println!("    {}", power_hold_line(snap));
    if let Some(eh) = expected_hold(snap) {
        anstream::println!("    expected hold: {eh}");
    }
    anstream::println!("    thermal:       {}", snap.thermal);
    anstream::println!("    battery:       {}", snap.battery);
    anstream::println!("    assertions:    {}", assertions_summary_line(snap));

    if verbose {
        anstream::println!();
        anstream::println!("  provider roots:");
        // bash `cmd_provider_roots_text | sed 's/^/    /'`: two lines per agent,
        // base-indented then shifted 4 more.
        for (name, p, state) in [
            ("claude", &snap.provider_claude, snap.agent_claude),
            ("codex", &snap.provider_codex, snap.agent_codex),
            ("copilot", &snap.provider_copilot, snap.agent_copilot),
        ] {
            anstream::println!("      {name:<7} home={}", p.home);
            anstream::println!(
                "              session={} exists={} state={}",
                p.session_dir,
                yes_no(p.exists),
                state.as_str(),
            );
        }
        anstream::println!();
        anstream::println!("  power assertions:");
        render_assertion_rows(snap);
    } else {
        anstream::println!();
        anstream::println!(
            "  detail: use 'vigil status --verbose' for provider paths and assertion rows"
        );
    }
}

/// Render the verbose assertion rows (bash: `(none)` / `(parse-failed; …)`
/// pass through 4-indented; holder rows become TSV-formatted lines).
fn render_assertion_rows(snap: &StatusSnapshot) {
    match snap.power_assertions_state.as_str() {
        "none" => anstream::println!("    (none)"),
        "parse_failed" => anstream::println!("    (parse-failed; raw output:)"),
        _ => {
            for a in &snap.power_assertions {
                let marker = if a.vigil { "← vigil" } else { "" };
                anstream::println!(
                    "    pid={:<7} {:<28} {:<32} {}",
                    a.pid,
                    a.process,
                    a.atype,
                    marker,
                );
            }
        }
    }
}
