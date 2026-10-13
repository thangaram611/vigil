//! `vigil refresh` — force an identity-bound evaluation and consume grace.
use std::path::{Path, PathBuf};
use vigil::daemon::refresh;

use super::{die, load_config_or_exit};

pub fn run() -> ! {
    let cfg = load_config_or_exit();
    let mut allowed = vec![PathBuf::from(&cfg.install_dir).join("bin/vigil")];
    if let Ok(exe) = std::env::current_exe() {
        allowed.push(exe);
    }
    match refresh::request_refresh(
        Path::new(&cfg.state_dir),
        Path::new(&cfg.daemon_tick_file),
        Path::new(&cfg.daemon_pidfile),
        &allowed,
    ) {
        Ok(tick) => {
            let state = if tick.engaged { "Holding" } else { "Released" };
            anstream::println!(
                "{state} — {} ({} working, {} waiting, {} idle, {} unknown; uncertain demand {})",
                tick.policy_reason(),
                tick.working
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                tick.waiting
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                tick.idle
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                tick.unknown
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                tick.uncertain
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".into())
            );
            std::process::exit(0);
        }
        Err(error) => die(&error),
    }
}
