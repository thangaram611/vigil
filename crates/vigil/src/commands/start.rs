//! `vigil start` — bootstrap the user LaunchAgent (§4.4, §2.3.5).
//!
//! Die if the plist is absent (run setup first). Already-loaded → idempotent.
//! Else `bootstrap` + best-effort `enable`, then the bounded first-scan wait
//! (`wait_for_daemon_scan`): poll up to `VIGIL_START_WAIT_SECS` (default 6) for
//! the daemon's first tick to land. A `fresh` scan → `ready`; the service going
//! away mid-wait → `service not running` (exit 1 from the wait, but start still
//! returns 0 in practice — see below); a timeout → `pending`, which is NOT an
//! error. NO `kickstart`.

use std::ffi::OsString;

use vigil::check::{CheckEngine, DaemonScanState, RealLoadProbe};
use vigil::service::{MacosLaunchdInstaller, ServiceError, ServiceInstaller, StartState};

use super::tui::Tui;
use super::{die, interactive, load_config_or_exit};

/// `vigil start [--yes|--non-interactive]`.
pub fn run(args: Vec<OsString>) -> ! {
    let mut yes = false;
    for a in &args {
        match a.to_str() {
            Some("--yes") | Some("--non-interactive") => yes = true,
            _ => die("usage: vigil start [--yes|--non-interactive]"),
        }
    }
    let cfg = load_config_or_exit();
    let installer = MacosLaunchdInstaller::new();

    // The clack-style UI, bound once to the interactive gate. When false (--yes,
    // piped, CI) the rail is never drawn at all: `start` historically printed NO
    // header/intro/outro, only the `launchd:` line + the daemon-scan progress, so
    // the plain path below reproduces exactly those bytes. The rail (intro / step
    // header / `✓` / outro) is added ONLY in interactive mode; the body `launchd:`
    // line is rendered via `Step::detail` whose plain fallback is the verbatim old
    // line.
    let ui = Tui::new(interactive(yes));
    let interactive = ui.is_interactive();

    // Resolve the start state up front; only the PRINTING differs by mode.
    let state = match installer.start_user_agent(&cfg) {
        Ok(s) => s,
        Err(ServiceError::PlistMissing(p)) => {
            die(&format!("plist not found at {p} — run 'vigil setup' first"))
        }
        Err(e) => die(&format!("start failed: {e}")),
    };

    if interactive {
        // Full rail: intro → step → body detail (+ scan wait) → done → outro.
        ui.intro("vigil: starting");
        ui.rail_space();
        let pb = ui.step("starting user LaunchAgent", "starting user LaunchAgent");
        match state {
            StartState::AlreadyLoaded => {
                pb.detail(
                    &format!(
                        "launchd: already loaded (gui/{}/{})",
                        vigil::config::get_uid(),
                        vigil::service::USER_AGENT_LABEL
                    ),
                    "",
                );
                pb.done("user LaunchAgent already running");
            }
            StartState::Bootstrapped => {
                pb.detail(
                    &format!("launchd: bootstrapped {}", vigil::service::USER_AGENT_LABEL),
                    "",
                );
                // `wait_for_daemon_scan` prints its own progress (shared with
                // setup/reload); run it inside the step's suspend region.
                pb.suspend(|| wait_for_daemon_scan(&cfg));
                pb.done("user LaunchAgent started");
            }
        }
        ui.outro("vigil: start complete");
    } else {
        // Byte-frozen plain path — exactly what `start` always printed.
        match state {
            StartState::AlreadyLoaded => {
                anstream::println!(
                    "  launchd: already loaded (gui/{}/{})",
                    vigil::config::get_uid(),
                    vigil::service::USER_AGENT_LABEL
                );
            }
            StartState::Bootstrapped => {
                anstream::println!(
                    "  launchd: bootstrapped {}",
                    vigil::service::USER_AGENT_LABEL
                );
                // Bounded first-scan wait; pending is NOT an error (always exit 0).
                wait_for_daemon_scan(&cfg);
            }
        }
    }
    std::process::exit(0);
}

/// The bounded first-scan wait (§2.3.5, bash `cmd_wait_for_daemon_scan`).
///
/// `wait_secs = VIGIL_START_WAIT_SECS` (non-numeric → 6; `<1` → return at once).
/// Loop `wait_secs*10` ticks at 100ms each. Each tick classify the scan state via
/// the shared [`CheckEngine`] scan-state path:
///   - `fresh` → print `  daemon scan: ready (…)`, return.
///   - service no longer loaded → print `  daemon scan: service not running`,
///     return (start itself still exits 0).
///   - timeout → `  daemon scan: pending (run 'vigil status' for details)`.
///
/// `pending` is explicitly NOT an error.
pub fn wait_for_daemon_scan(cfg: &vigil::config::VigilConfig) {
    let wait_secs = cfg.start_wait_secs; // already parsed (default 6) by config
    if wait_secs < 1 {
        return;
    }
    let max_ticks = wait_secs as u64 * 10;
    let probe = RealLoadProbe;
    for _ in 0..max_ticks {
        let now = super::now_unix();
        let (state, age) = CheckEngine::daemon_scan(cfg, now, &probe);
        if state == DaemonScanState::Fresh {
            match age {
                Some(a) => anstream::println!("  daemon scan: ready ({a}s ago)"),
                None => anstream::println!("  daemon scan: ready"),
            }
            return;
        }
        if state == DaemonScanState::Unloaded {
            anstream::println!("  daemon scan: service not running");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anstream::println!("  daemon scan: pending (run 'vigil status' for details)");
}
