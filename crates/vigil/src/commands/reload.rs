//! `vigil reload` — re-sync the install binary + re-render the plist + bounce
//! launchd (§4.3, §2.2.4).
//!
//! `cmd_sync_install` → re-render the plist if present → `stop` (bootout +
//! 50×100ms poll) → `start` (bootstrap + enable + bounded wait) → done.
//!
//! NEVER `kickstart -k`: kickstart restarts the process but launchd keeps the
//! CACHED plist, so plist changes never take effect. reload MUST re-read the
//! plist via bootout/bootstrap (§2.2.4).
//!
//! reload reaches sudo only via `cmd_sync_install` (which is non-privileged) and
//! the launchctl bounce. Per §4.8 the admin guard is enforced FIRST so a
//! `VIGIL_TEST_NO_ADMIN` run aborts before any launchctl/root touch.

use std::ffi::OsString;

use vigil::service::{MacosLaunchdInstaller, ServiceInstaller, StartState, StopState};

use super::{
    cmd_sync_install, die, load_config_or_exit, require_admin_allowed_or_die, user_plist_path,
};

/// `vigil reload` takes no flags.
pub fn run(args: Vec<OsString>) -> ! {
    if !args.is_empty() {
        die("usage: vigil reload");
    }

    // reload bounces launchd; gate it behind the admin guard FIRST (§4.8) so a
    // VIGIL_TEST_NO_ADMIN run aborts before any launchctl/install touch.
    require_admin_allowed_or_die();

    let cfg = load_config_or_exit();

    anstream::println!("vigil: reloading");

    // 1. Re-sync the install binary (TCC copy-out) BEFORE re-pointing the plist.
    anstream::println!();
    anstream::println!("  1. re-syncing install binary");
    if let Err(e) = cmd_sync_install(&cfg) {
        die(&e);
    }

    let installer = MacosLaunchdInstaller::new();

    // 2. Re-render the plist if present (pick up plist changes — the reason
    //    reload exists). NEVER kickstart.
    anstream::println!();
    anstream::println!("  2. re-rendering LaunchAgent plist");
    let plist = user_plist_path();
    let mut rerendered = false;
    if plist.is_file() {
        if let Err(e) = installer.install_user_agent(&cfg) {
            die(&format!("could not re-render plist: {e}"));
        }
        rerendered = true;
        anstream::println!("     re-rendered {}", plist.display());
    } else {
        anstream::println!("     plist absent ({}) — skipped", plist.display());
    }

    // 3. Stop (bootout + 50×100ms poll). NEVER kickstart.
    anstream::println!();
    anstream::println!("  3. stopping");
    let stop_state = match installer.stop_user_agent(&cfg) {
        Ok(s) => s,
        Err(e) => die(&format!("stop failed: {e}")),
    };
    match stop_state {
        StopState::BootedOut => {
            anstream::println!("  launchd: booted out {}", vigil::service::USER_AGENT_LABEL)
        }
        StopState::NotLoaded => anstream::println!("  launchd: not loaded"),
    }

    // 4. Start (bootstrap + enable + bounded first-scan wait).
    anstream::println!();
    anstream::println!("  4. starting");
    let start_state = match installer.start_user_agent(&cfg) {
        Ok(s) => s,
        Err(e) => die(&format!("start failed: {e}")),
    };
    match start_state {
        StartState::AlreadyLoaded => anstream::println!(
            "  launchd: already loaded (gui/{}/{})",
            vigil::config::get_uid(),
            vigil::service::USER_AGENT_LABEL
        ),
        StartState::Bootstrapped => {
            anstream::println!(
                "  launchd: bootstrapped {}",
                vigil::service::USER_AGENT_LABEL
            );
            super::start::wait_for_daemon_scan(&cfg);
        }
    }

    // What-changed summary (§4.3 UX).
    anstream::println!();
    anstream::println!("vigil: reload complete.");
    anstream::println!("  what changed:");
    anstream::println!(
        "    install binary: re-synced ({}/bin/vigil)",
        cfg.install_dir
    );
    if rerendered {
        anstream::println!("    LaunchAgent plist: re-rendered + reloaded");
    } else {
        anstream::println!("    LaunchAgent plist: not present (run 'vigil setup' to install)");
    }
    anstream::println!("    launchd: bounced via bootout/bootstrap (NOT kickstart)");
    std::process::exit(0);
}
