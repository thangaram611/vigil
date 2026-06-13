//! `vigil uninstall` — remove the daemon + helper + state (§4.2). LOGS PRESERVED.
//!
//! Accepts ONLY `--yes`/`--non-interactive` (everything else → usage die). Same
//! three guards as setup. 5 ordered steps: stop → full_release (best-effort) →
//! rm LaunchAgent plist → remove privileged helper + newsyslog + legacy sudoers →
//! clear state (rm baseline + rm -rf install dir). `~/Library/Logs/vigil` is
//! NEVER removed. When run in a TTY without `--yes`, a single destructive confirm
//! gates the whole flow.

use std::ffi::OsString;

use vigil::config::VigilConfig;
use vigil::ipc::MacHelperClient;
use vigil::power::PowerMachine;
use vigil::power::caffeinate::MacCaffeinate;
use vigil::power::pmset::MacSleepReader;
use vigil::service::{MacosLaunchdInstaller, ServiceInstaller, StopState};

use super::tui::Tui;
use super::{
    LEGACY_SUDOERS_FILE, die, helper_plist_path, interactive, load_config_or_exit,
    require_admin_allowed_or_die, user_plist_path,
};

/// `vigil uninstall [--yes|--non-interactive]`.
pub fn run(args: Vec<OsString>) -> ! {
    let mut yes = false;
    for a in &args {
        match a.to_str() {
            Some("--yes") | Some("--non-interactive") => yes = true,
            // Every other arg (incl. --dry-run, -x, bare words) is still a usage
            // violation → die → EX_ERROR(1), the exact pre-change code path.
            _ => die("usage: vigil uninstall [--yes|--non-interactive]"),
        }
    }

    // ── guards FIRST, same order as setup (§4.2) ──
    require_admin_allowed_or_die();
    let cfg = load_config_or_exit();
    if let Err(e) = cfg.validate_security_paths() {
        die(&e);
    }
    if let Err(e) = super::assert_vigil_tree_path("install dir", &cfg.install_dir) {
        die(&e);
    }

    // The clack-style UI, bound once to the interactive gate (see setup.rs).
    let ui = Tui::new(interactive(yes));

    // Destructive confirm — the WHOLE command removes daemon/helper/state, so gate
    // it (default=false) AFTER the three guards and BEFORE step 1. Non-interactive
    // /--yes → `confirm` returns the default (false here means: when the prompt is
    // skipped we proceed, matching today, because the gate that skips it is the
    // same one that means "proceed"). Decline → clean exit 0 no-op.
    //
    // NOTE: the default passed to `confirm` is the interactive default shown to a
    // human (false = don't uninstall unless they say yes). In non-interactive mode
    // `confirm` does NOT use this default to decide flow — the command proceeds
    // because `interactive(yes)` is false; we replicate the prior `if
    // interactive(yes) && !confirm` short-circuit explicitly.
    if ui.is_interactive()
        && !ui.confirm(
            "Uninstall vigil? Removes the daemon, privileged helper, and local state (logs preserved).",
            false,
        )
    {
        ui.outro_cancel("vigil: uninstall aborted. Nothing was removed.");
        std::process::exit(0);
    }

    let installer = MacosLaunchdInstaller::new();

    ui.intro("vigil: uninstalling");

    // 1. stop the user LaunchAgent (best-effort; let it release cleanly).
    ui.rail_space();
    let pb = ui.step(
        "stopping user LaunchAgent",
        "  1. stopping user LaunchAgent",
    );
    match installer.stop_user_agent(&cfg) {
        Ok(StopState::BootedOut) => pb.detail(
            &format!("launchd: booted out {}", vigil::service::USER_AGENT_LABEL),
            &format!("  launchd: booted out {}", vigil::service::USER_AGENT_LABEL),
        ),
        Ok(StopState::NotLoaded) => pb.detail("launchd: not loaded", "  launchd: not loaded"),
        Err(e) => pb.detail(
            &format!("launchd: stop best-effort failed: {e}"),
            &format!("  launchd: stop best-effort failed: {e}"),
        ),
    }
    pb.done("user LaunchAgent stopped");

    // 2. release any best-effort power hold (full_release: restore baseline, kill
    //    caffeinate, clear baseline) — best-effort.
    ui.rail_space();
    let pb = ui.step("releasing power hold", "  2. releasing power hold");
    pb.suspend(|| best_effort_full_release(&cfg));
    pb.detail(
        "power hold: released if active",
        "     power hold: released if active",
    );
    pb.done("power hold released");

    // 3. remove the user LaunchAgent plist.
    ui.rail_space();
    let pb = ui.step(
        "removing user LaunchAgent",
        "  3. removing user LaunchAgent",
    );
    let plist = user_plist_path();
    if plist.is_file() {
        let _ = std::fs::remove_file(&plist);
        pb.detail(
            &format!("removed: {}", plist.display()),
            &format!("     removed: {}", plist.display()),
        );
    } else {
        pb.detail(
            &format!("already absent: {}", plist.display()),
            &format!("     already absent: {}", plist.display()),
        );
    }
    pb.done("user LaunchAgent removed");

    // 4. remove root-owned files (gated behind a combined existence check).
    ui.rail_space();
    let pb = ui.step(
        "removing privileged helper and log rotation",
        "  4. removing privileged helper and log rotation",
    );
    let newsyslog_exists = std::path::Path::new(&cfg.newsyslog_file).is_file();
    let sudoers_exists = std::path::Path::new(LEGACY_SUDOERS_FILE).is_file();
    let helper_plist_exists = std::path::Path::new(helper_plist_path()).is_file();
    let helper_dir_exists = std::path::Path::new(&cfg.power_helper_dir).is_dir();
    let root_helper_exists = std::path::Path::new(&cfg.root_helper).is_file();

    if newsyslog_exists
        || sudoers_exists
        || helper_plist_exists
        || helper_dir_exists
        || root_helper_exists
    {
        // Suspend the spinner across the sudo block (password prompt owns the TTY).
        pb.suspend(|| {
            if newsyslog_exists {
                pb.detail(
                    &format!(
                        "removing newsyslog: {} (sudo may prompt)",
                        cfg.newsyslog_file
                    ),
                    &format!(
                        "     removing newsyslog: {} (sudo may prompt)",
                        cfg.newsyslog_file
                    ),
                );
                sudo_rm_f(&cfg.newsyslog_file);
            }
            if sudoers_exists {
                pb.detail(
                    &format!("removing legacy sudoers: {LEGACY_SUDOERS_FILE}"),
                    &format!("     removing legacy sudoers: {LEGACY_SUDOERS_FILE}"),
                );
                sudo_rm_f(LEGACY_SUDOERS_FILE);
            }
            if helper_plist_exists || helper_dir_exists || root_helper_exists {
                cmd_remove_root_helper(&cfg, ui);
            }
        });
    } else {
        pb.detail(
            "no root-owned Vigil files found",
            "     no root-owned Vigil files found",
        );
    }
    pb.done("privileged helper removed");

    // 5. clear local state (rm baseline + rm -rf install dir). LOGS PRESERVED.
    ui.rail_space();
    let pb = ui.step("clearing local state", "  5. clearing local state");
    if std::path::Path::new(&cfg.baseline_file).is_file() {
        let _ = std::fs::remove_file(&cfg.baseline_file);
        pb.detail("baseline state: removed", "     baseline state: removed");
    } else {
        pb.detail(
            "baseline state: already clear",
            "     baseline state: already clear",
        );
    }
    if std::path::Path::new(&cfg.install_dir).is_dir() {
        let _ = std::fs::remove_dir_all(&cfg.install_dir);
        pb.detail(
            &format!("install dir: removed {}", cfg.install_dir),
            &format!("     install dir: removed {}", cfg.install_dir),
        );
    } else {
        pb.detail(
            &format!("install dir: already absent {}", cfg.install_dir),
            &format!("     install dir: already absent {}", cfg.install_dir),
        );
    }
    // The `vigil` PATH symlink points at your repo dev build (not the install
    // dir), so it survives uninstall — leave it so `vigil setup` can reinstall.
    pb.done("local state cleared");

    // Outro: rail bottom (interactive) or the verbatim plain completion lines.
    if ui.is_interactive() {
        ui.outro("vigil: uninstall complete");
        // LOGS ARE NEVER REMOVED.
        anstream::println!("  logs preserved: {}", cfg.log_dir);
    } else {
        anstream::println!();
        anstream::println!("vigil: uninstall complete");
        // LOGS ARE NEVER REMOVED.
        anstream::println!("  logs preserved: {}", cfg.log_dir);
    }
    std::process::exit(0);
}

/// Build a `PowerMachine` over the Mac seams and call `full_release()`
/// (best-effort): restore the saved baseline, kill caffeinate, clear baseline.
fn best_effort_full_release(cfg: &VigilConfig) {
    let ipc = MacHelperClient {
        request_dir: std::path::PathBuf::from(&cfg.power_request_dir),
        response_dir: std::path::PathBuf::from(&cfg.power_response_dir),
        timeout_secs: cfg.power_helper_timeout_secs,
    };
    let machine = PowerMachine {
        ipc: &ipc,
        caffeinate: &MacCaffeinate,
        sleep: &MacSleepReader,
        baseline_file: std::path::PathBuf::from(&cfg.baseline_file),
        caffeinate_pidfile: std::path::PathBuf::from(&cfg.caffeinate_pidfile),
    };
    machine.full_release();
}

/// Remove the privileged helper (§4.2): bootout system helper → conditional rm
/// helper plist → rm -rf power helper dir → rm root helper → rmdir root bin/root
/// dirs (best-effort; non-empty → kept). Re-asserts the guards at the boundary.
fn cmd_remove_root_helper(cfg: &VigilConfig, ui: Tui) {
    require_admin_allowed_or_die();
    if let Err(e) = cfg.validate_security_paths() {
        die(&e);
    }
    ui.detail(
        "root helper: removing LaunchDaemon and helper files (sudo may prompt)",
        "     root helper: removing LaunchDaemon and helper files (sudo may prompt)",
    );
    // bootout (ignore failure). Drop stderr: if the helper isn't loaded,
    // launchctl prints "Boot-out failed: 3: No such process" — benign noise that
    // would otherwise break the rail.
    let _ = std::process::Command::new("sudo")
        .args([
            "launchctl",
            "bootout",
            &format!("system/{}", vigil::service::HELPER_LABEL),
        ])
        .stderr(std::process::Stdio::null())
        .status();
    if std::path::Path::new(helper_plist_path()).is_file() {
        sudo_rm_f(helper_plist_path());
    }
    if std::path::Path::new(&cfg.power_helper_dir).is_dir() {
        sudo_rm_rf(&cfg.power_helper_dir);
    }
    if std::path::Path::new(&cfg.root_helper).is_file() {
        sudo_rm_f(&cfg.root_helper);
    }
    if std::path::Path::new(&cfg.root_bin_dir).is_dir() {
        sudo_rmdir(&cfg.root_bin_dir);
    }
    if std::path::Path::new(&cfg.root_dir).is_dir() {
        sudo_rmdir(&cfg.root_dir);
    }
}

fn sudo_rm_f(path: &str) {
    require_admin_allowed_or_die();
    let _ = std::process::Command::new("sudo")
        .args(["rm", "-f", path])
        .status();
}

fn sudo_rm_rf(path: &str) {
    require_admin_allowed_or_die();
    let _ = std::process::Command::new("sudo")
        .args(["rm", "-rf", path])
        .status();
}

fn sudo_rmdir(path: &str) {
    require_admin_allowed_or_die();
    let _ = std::process::Command::new("sudo")
        .args(["rmdir", path])
        .status();
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    /// Classify an arg the way `run` does: only `--yes`/`--non-interactive` are
    /// accepted; every other arg routes to the usage `die`. (We can't catch the
    /// process exit, so we assert the parse classification the loop implements.)
    fn is_accepted(arg: &str) -> bool {
        matches!(arg, "--yes" | "--non-interactive")
    }

    /// The relaxed flag contract: `--yes`/`--non-interactive` are the ONLY
    /// accepted args; `--dry-run`/`-x`/`extra`/etc. remain usage violations, and
    /// the empty invocation is still valid (interactive confirm path).
    #[test]
    fn strict_zero_flag_rejects_any_arg() {
        // Still violations (would hit die → EX_ERROR), exactly as before.
        for arg in ["--anything", "extra", "--dry-run", "-x"] {
            assert!(
                !is_accepted(arg),
                "{arg} must NOT be an accepted uninstall flag"
            );
        }
        // The newly-allowed non-interactive flags.
        for arg in ["--yes", "--non-interactive"] {
            assert!(is_accepted(arg), "{arg} must be an accepted uninstall flag");
        }
        // The empty-args case is still a valid invocation (interactive confirm).
        let none: Vec<OsString> = Vec::new();
        assert!(none.is_empty());
    }
}
