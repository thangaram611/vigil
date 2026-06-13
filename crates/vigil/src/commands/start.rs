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

use super::{die, load_config_or_exit};

/// `vigil start` takes no flags.
pub fn run(args: Vec<OsString>) -> ! {
    if !args.is_empty() {
        die("usage: vigil start");
    }
    let cfg = load_config_or_exit();
    let installer = MacosLaunchdInstaller::new();

    match installer.start_user_agent(&cfg) {
        Ok(StartState::AlreadyLoaded) => {
            anstream::println!(
                "  launchd: already loaded (gui/{}/{})",
                vigil::config::get_uid(),
                vigil::service::USER_AGENT_LABEL
            );
            std::process::exit(0);
        }
        Ok(StartState::Bootstrapped) => {
            anstream::println!(
                "  launchd: bootstrapped {}",
                vigil::service::USER_AGENT_LABEL
            );
            // Bounded first-scan wait; pending is NOT an error (always return 0).
            wait_for_daemon_scan(&cfg);
            std::process::exit(0);
        }
        Err(ServiceError::PlistMissing(p)) => {
            die(&format!("plist not found at {p} — run 'vigil setup' first"))
        }
        Err(e) => die(&format!("start failed: {e}")),
    }
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
