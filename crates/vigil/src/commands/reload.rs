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

use super::tui::Tui;
use super::{
    cmd_sync_install, die, interactive, link_vigil_onto_path, load_config_or_exit,
    require_admin_allowed_or_die, user_plist_path,
};

/// `vigil reload [--yes|--non-interactive]`.
pub fn run(args: Vec<OsString>) -> ! {
    let mut yes = false;
    for a in &args {
        match a.to_str() {
            Some("--yes") | Some("--non-interactive") => yes = true,
            _ => die("usage: vigil reload [--yes|--non-interactive]"),
        }
    }

    // reload bounces launchd; gate it behind the admin guard FIRST (§4.8) so a
    // VIGIL_TEST_NO_ADMIN run aborts before any launchctl/install touch.
    require_admin_allowed_or_die();

    let cfg = load_config_or_exit();

    // The clack-style UI, bound once to the interactive gate. When false (--yes,
    // piped, CI) EVERY method falls back to the byte-frozen plain lines below —
    // including the shared install helpers' (`cmd_sync_install` /
    // `link_vigil_onto_path`) detail lines, which route to the exact plain lines
    // reload has always printed.
    let ui = Tui::new(interactive(yes));

    let installer = MacosLaunchdInstaller::new();

    ui.intro("vigil: reloading");

    // 1. Re-sync the install binary (TCC copy-out) BEFORE re-pointing the plist.
    ui.rail_space();
    let pb = ui.step(
        "re-syncing install binary",
        "  1. re-syncing install binary",
    );
    let dev_vigil = match cmd_sync_install(&cfg, ui) {
        Ok(p) => p,
        Err(e) => die(&e),
    };
    // Heal the PATH symlink (creates it if missing; refreshes a stale one).
    link_vigil_onto_path(&dev_vigil, ui);
    pb.done("install binary re-synced");

    // 2. Re-render the plist if present (pick up plist changes — the reason
    //    reload exists). NEVER kickstart.
    ui.rail_space();
    let pb = ui.step(
        "re-rendering LaunchAgent plist",
        "  2. re-rendering LaunchAgent plist",
    );
    let plist = user_plist_path();
    let mut rerendered = false;
    if plist.is_file() {
        if let Err(e) = installer.install_user_agent(&cfg) {
            die(&format!("could not re-render plist: {e}"));
        }
        rerendered = true;
        pb.detail(
            &format!("re-rendered {}", plist.display()),
            &format!("     re-rendered {}", plist.display()),
        );
    } else {
        pb.detail(
            &format!("plist absent ({}) — skipped", plist.display()),
            &format!("     plist absent ({}) — skipped", plist.display()),
        );
    }
    pb.done("LaunchAgent plist re-rendered");

    // 3. Stop (bootout + 50×100ms poll). NEVER kickstart.
    ui.rail_space();
    let pb = ui.step("stopping", "  3. stopping");
    let stop_state = match installer.stop_user_agent(&cfg) {
        Ok(s) => s,
        Err(e) => die(&format!("stop failed: {e}")),
    };
    match stop_state {
        StopState::BootedOut => pb.detail(
            &format!("launchd: booted out {}", vigil::service::USER_AGENT_LABEL),
            &format!("  launchd: booted out {}", vigil::service::USER_AGENT_LABEL),
        ),
        StopState::NotLoaded => pb.detail("launchd: not loaded", "  launchd: not loaded"),
    }
    pb.done("stopped");

    // 4. Start (bootstrap + enable + bounded first-scan wait).
    ui.rail_space();
    let pb = ui.step("starting", "  4. starting");
    let start_state = match installer.start_user_agent(&cfg) {
        Ok(s) => s,
        Err(e) => die(&format!("start failed: {e}")),
    };
    match start_state {
        StartState::AlreadyLoaded => pb.detail(
            &format!(
                "launchd: already loaded (gui/{}/{})",
                vigil::config::get_uid(),
                vigil::service::USER_AGENT_LABEL
            ),
            &format!(
                "  launchd: already loaded (gui/{}/{})",
                vigil::config::get_uid(),
                vigil::service::USER_AGENT_LABEL
            ),
        ),
        StartState::Bootstrapped => {
            pb.detail(
                &format!("launchd: bootstrapped {}", vigil::service::USER_AGENT_LABEL),
                &format!(
                    "  launchd: bootstrapped {}",
                    vigil::service::USER_AGENT_LABEL
                ),
            );
            // wait_for_daemon_scan prints its own progress; suspend the spinner.
            pb.suspend(|| super::start::wait_for_daemon_scan(&cfg));
        }
    }
    pb.done("started");

    // What-changed summary (§4.3 UX). Interactive: rail bottom + dim details;
    // non-interactive: the verbatim plain completion lines.
    let install_change = format!("install binary: re-synced ({}/bin/vigil)", cfg.install_dir);
    let plist_change = if rerendered {
        "LaunchAgent plist: re-rendered + reloaded".to_string()
    } else {
        "LaunchAgent plist: not present (run 'vigil setup' to install)".to_string()
    };
    let launchd_change = "launchd: bounced via bootout/bootstrap (NOT kickstart)".to_string();
    if ui.is_interactive() {
        ui.outro("vigil: reload complete.");
        ui.detail("what changed:", "");
        ui.detail(&format!("  {install_change}"), "");
        ui.detail(&format!("  {plist_change}"), "");
        ui.detail(&format!("  {launchd_change}"), "");
    } else {
        anstream::println!();
        anstream::println!("vigil: reload complete.");
        anstream::println!("  what changed:");
        anstream::println!("    {install_change}");
        anstream::println!("    {plist_change}");
        anstream::println!("    {launchd_change}");
    }
    std::process::exit(0);
}
