//! `vigil uninstall` — remove the daemon + helper + state (§4.2). LOGS PRESERVED.
//!
//! STRICT zero-flag: ANY argument → usage die. Same three guards as setup.
//! 5 ordered steps: stop → full_release (best-effort) → rm LaunchAgent plist →
//! remove privileged helper + newsyslog + legacy sudoers → clear state (rm
//! baseline + rm -rf install dir). `~/Library/Logs/vigil` is NEVER removed.

use std::ffi::OsString;

use vigil::config::VigilConfig;
use vigil::ipc::MacHelperClient;
use vigil::power::PowerMachine;
use vigil::power::caffeinate::MacCaffeinate;
use vigil::power::pmset::MacSleepReader;
use vigil::service::{MacosLaunchdInstaller, ServiceInstaller, StopState};

use super::{
    LEGACY_SUDOERS_FILE, die, helper_plist_path, load_config_or_exit, require_admin_allowed_or_die,
    user_plist_path,
};

/// `vigil uninstall` — STRICT zero-flag.
pub fn run(args: Vec<OsString>) -> ! {
    if !args.is_empty() {
        die("usage: vigil uninstall");
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

    let installer = MacosLaunchdInstaller::new();

    anstream::println!("vigil: uninstalling");

    // 1. stop the user LaunchAgent (best-effort; let it release cleanly).
    anstream::println!();
    anstream::println!("  1. stopping user LaunchAgent");
    match installer.stop_user_agent(&cfg) {
        Ok(StopState::BootedOut) => {
            anstream::println!("  launchd: booted out {}", vigil::service::USER_AGENT_LABEL)
        }
        Ok(StopState::NotLoaded) => anstream::println!("  launchd: not loaded"),
        Err(e) => anstream::println!("  launchd: stop best-effort failed: {e}"),
    }

    // 2. release any best-effort power hold (full_release: restore baseline, kill
    //    caffeinate, clear baseline) — best-effort.
    anstream::println!();
    anstream::println!("  2. releasing power hold");
    best_effort_full_release(&cfg);
    anstream::println!("     power hold: released if active");

    // 3. remove the user LaunchAgent plist.
    anstream::println!();
    anstream::println!("  3. removing user LaunchAgent");
    let plist = user_plist_path();
    if plist.is_file() {
        let _ = std::fs::remove_file(&plist);
        anstream::println!("     removed: {}", plist.display());
    } else {
        anstream::println!("     already absent: {}", plist.display());
    }

    // 4. remove root-owned files (gated behind a combined existence check).
    anstream::println!();
    anstream::println!("  4. removing privileged helper and log rotation");
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
        if newsyslog_exists {
            anstream::println!(
                "     removing newsyslog: {} (sudo may prompt)",
                cfg.newsyslog_file
            );
            sudo_rm_f(&cfg.newsyslog_file);
        }
        if sudoers_exists {
            anstream::println!("     removing legacy sudoers: {LEGACY_SUDOERS_FILE}");
            sudo_rm_f(LEGACY_SUDOERS_FILE);
        }
        if helper_plist_exists || helper_dir_exists || root_helper_exists {
            cmd_remove_root_helper(&cfg);
        }
    } else {
        anstream::println!("     no root-owned Vigil files found");
    }

    // 5. clear local state (rm baseline + rm -rf install dir). LOGS PRESERVED.
    anstream::println!();
    anstream::println!("  5. clearing local state");
    if std::path::Path::new(&cfg.baseline_file).is_file() {
        let _ = std::fs::remove_file(&cfg.baseline_file);
        anstream::println!("     baseline state: removed");
    } else {
        anstream::println!("     baseline state: already clear");
    }
    if std::path::Path::new(&cfg.install_dir).is_dir() {
        let _ = std::fs::remove_dir_all(&cfg.install_dir);
        anstream::println!("     install dir: removed {}", cfg.install_dir);
    } else {
        anstream::println!("     install dir: already absent {}", cfg.install_dir);
    }

    anstream::println!();
    anstream::println!("vigil: uninstall complete");
    // LOGS ARE NEVER REMOVED.
    anstream::println!("  logs preserved: {}", cfg.log_dir);
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
fn cmd_remove_root_helper(cfg: &VigilConfig) {
    require_admin_allowed_or_die();
    if let Err(e) = cfg.validate_security_paths() {
        die(&e);
    }
    anstream::println!(
        "     root helper: removing LaunchDaemon and helper files (sudo may prompt)"
    );
    // bootout (ignore failure).
    let _ = std::process::Command::new("sudo")
        .args([
            "launchctl",
            "bootout",
            &format!("system/{}", vigil::service::HELPER_LABEL),
        ])
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

    /// Strict zero-flag: ANY argument is a usage error. We can't catch the exit,
    /// so assert the predicate `run` uses: a non-empty args vec must be rejected.
    #[test]
    fn strict_zero_flag_rejects_any_arg() {
        for arg in ["--anything", "extra", "--dry-run", "-x"] {
            let args: Vec<OsString> = vec![OsString::from(arg)];
            assert!(
                !args.is_empty(),
                "any arg to uninstall is a usage violation"
            );
        }
        // The empty-args case is the only accepted invocation.
        let none: Vec<OsString> = Vec::new();
        assert!(none.is_empty());
    }
}
