//! `vigil status` — shared read-only activity, cached policy and power evidence.
//! Exit is always 0 except for a usage violation.

use std::ffi::OsString;

use vigil::activity::scan::Agent;
use vigil::check::{CheckEngine, CheckMode, DaemonScanState, StatusSnapshot};

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
    // Interactive-only spinner; piped machine output contains only JSON.
    let interactive = super::interactive(false);
    let report = super::tui::with_activity(interactive, "Gathering status…", || {
        CheckEngine::run(&cfg, CheckMode::Status, now)
    });
    let snap = &report.snapshot;

    match mode {
        Mode::Json => {
            // Deterministic schema, plain JSON without ANSI.
            print!("{}", snap.to_json());
        }
        Mode::Text { verbose } => {
            render_text(snap, verbose);
        }
    }
    std::process::exit(0);
}

// Text rendering.

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
    format!(
        "claude={}  codex={}  copilot={}  vscode_copilot_chat={}",
        snap.activity.provider_state(Agent::Claude),
        snap.activity.provider_state(Agent::Codex),
        snap.activity.provider_state(Agent::Copilot),
        snap.agent_vscode_copilot_chat,
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
    let hold = if snap.hold_engaged {
        "engaged"
    } else {
        "released"
    };
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

/// The `expected_hold` sub-state (§5.3) — why there is demand but no hold, in
/// priority order. `cut_*` are LIVE decisions (the snapshot carries
/// `cut_thermal`/`cut_battery` from live `should_cut`).
fn expected_hold(snap: &StatusSnapshot) -> Option<&'static str> {
    if snap.hold_engaged {
        return None;
    }
    let work = snap.activity.demand();
    if snap.mode.demand(work) == 0 {
        // Plain idle needs no explanation; `off` suppressing real work does.
        return (work > 0).then_some("off by manual mode ('vigil mode auto' to resume)");
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
    } else {
        "pending (daemon/helper transition in progress)"
    })
}

/// Render live activity and label the age and identity of cached daemon policy.
fn render_text(snap: &StatusSnapshot, verbose: bool) {
    let daemon_pid = match snap.daemon_pid {
        Some(p) => p.to_string(),
        None => "-".to_string(),
    };
    let scan = scan_text(snap.daemon_scan_state, snap.daemon_scan_age_secs);
    let helper = if snap.power_helper.launchd_loaded {
        "loaded"
    } else {
        "not loaded"
    };

    anstream::println!("vigil status");
    anstream::println!();
    anstream::println!("  service");
    anstream::println!("    launchd:      {}", yes_no(snap.launchd_loaded));
    anstream::println!("    daemon pid:    {daemon_pid}");
    anstream::println!("    scan:          {scan}");
    anstream::println!("    root helper:   {helper}; responsiveness unchecked");
    anstream::println!();
    anstream::println!("  activity");
    anstream::println!(
        "    sessions:      {} working / {} waiting / {} idle / {} unknown",
        snap.activity.working(),
        snap.activity.waiting(),
        snap.activity.idle(),
        snap.activity.unknown(),
    );
    anstream::println!(
        "    demand:        {} wrappers / {} uncertain / vscode={}",
        snap.activity.wrappers,
        snap.activity.uncertain(),
        yes_no(snap.activity.vscode),
    );
    anstream::println!(
        "    hosts:         {} detected",
        snap.activity.detected_hosts
    );
    if let Some(policy) = &snap.cached_policy {
        anstream::println!(
            "    cached policy: {} ({}s ago; identity {})",
            policy.policy_reason(),
            snap.cached_policy_age_secs.unwrap_or(0),
            if snap.cached_policy_identity_matches {
                "matches"
            } else {
                "unverified"
            },
        );
        if policy.version == 1 {
            anstream::println!(
                "                   legacy policy; session counts unavailable; refresh unsupported"
            );
        }
    }
    anstream::println!("    agents:        {}", agents_line(snap));
    match snap.mode {
        vigil::mode::Mode::Auto => {}
        vigil::mode::Mode::On => {
            anstream::println!("    manual mode:   on (holding regardless of agents)")
        }
        vigil::mode::Mode::Off => anstream::println!("    manual mode:   off (never holding)"),
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
        anstream::println!("  sessions:");
        for session in &snap.activity.sessions {
            anstream::println!(
                "    {} {}: {} ({}, {}){}",
                session.provider,
                session.id,
                session.state.as_str(),
                session.evidence,
                session.reason,
                session
                    .foreground_wait
                    .as_deref()
                    .map(|wait| format!("; foreground wait={wait}"))
                    .unwrap_or_default(),
            );
        }
        anstream::println!();
        anstream::println!("  provider roots:");
        // bash `cmd_provider_roots_text | sed 's/^/    /'`: two lines per agent,
        // base-indented then shifted 4 more.
        for (name, p, state) in [
            (
                "claude",
                &snap.provider_claude,
                snap.activity.provider_state(Agent::Claude),
            ),
            (
                "codex",
                &snap.provider_codex,
                snap.activity.provider_state(Agent::Codex),
            ),
            (
                "copilot",
                &snap.provider_copilot,
                snap.activity.provider_state(Agent::Copilot),
            ),
        ] {
            anstream::println!("      {name:<7} home={}", p.home);
            anstream::println!(
                "              session={} exists={} state={}",
                p.session_dir,
                yes_no(p.exists),
                state,
            );
        }
        anstream::println!();
        anstream::println!("  power assertions:");
        render_assertion_rows(snap);
    } else {
        anstream::println!();
        anstream::println!(
            "  detail: use 'vigil status --verbose' for session evidence, provider paths and assertion rows"
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
