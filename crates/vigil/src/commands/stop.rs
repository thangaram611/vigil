//! `vigil stop` — boot out the user LaunchAgent (§4.5, §2.2.3).
//!
//! `bootout` → up-to-50×100ms poll until `launchctl print` fails → remove
//! `daemon.tick` → done. Idempotent: a "not loaded" agent prints the soft path
//! and still removes the tick file (best-effort).
//!
//! The 50×100ms poll lives in [`crate::service`]'s `stop_user_agent` (the seam
//! that is unit-tested against a scripted `Launchctl` fake). This command is the
//! thin CLI wrapper that prints the bash-exact strings and routes the exit code.

use std::ffi::OsString;

use vigil::service::{MacosLaunchdInstaller, ServiceInstaller, StopState};

use super::tui::Tui;
use super::{die, interactive, load_config_or_exit};

/// `vigil stop [--yes|--non-interactive]`. Run the bootout-poll and print the
/// bash strings.
pub fn run(args: Vec<OsString>) -> ! {
    let mut yes = false;
    for a in &args {
        match a.to_str() {
            Some("--yes") | Some("--non-interactive") => yes = true,
            _ => die("usage: vigil stop [--yes|--non-interactive]"),
        }
    }
    let cfg = load_config_or_exit();
    let installer = MacosLaunchdInstaller::new();

    // The clack-style UI, bound once to the interactive gate. When false (--yes,
    // piped, CI) the rail is never drawn: `stop` historically printed ONLY the
    // single `launchd:` line, so the plain path below reproduces exactly those
    // bytes. The rail (intro / step header / `✓` / outro) is added ONLY in
    // interactive mode.
    let ui = Tui::new(interactive(yes));
    let interactive = ui.is_interactive();

    // Resolve the stop state up front; only the PRINTING differs by mode.
    let state = match installer.stop_user_agent(&cfg) {
        Ok(s) => s,
        Err(e) => die(&format!("stop failed: {e}")),
    };

    if interactive {
        ui.intro("vigil: stopping");
        ui.rail_space();
        let pb = ui.step("stopping user LaunchAgent", "stopping user LaunchAgent");
        match state {
            StopState::BootedOut => pb.detail(
                &format!("launchd: booted out {}", vigil::service::USER_AGENT_LABEL),
                "",
            ),
            StopState::NotLoaded => pb.detail("launchd: not loaded", ""),
        }
        pb.done("user LaunchAgent stopped");
        ui.outro("vigil: stop complete");
    } else {
        // Byte-frozen plain path — exactly what `stop` always printed.
        match state {
            StopState::BootedOut => {
                anstream::println!("  launchd: booted out {}", vigil::service::USER_AGENT_LABEL);
            }
            StopState::NotLoaded => {
                anstream::println!("  launchd: not loaded");
            }
        }
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    use vigil::service::ServiceError;
    use vigil::service::{
        BOOTOUT_POLL_MAX, Launchctl, MacosLaunchdInstaller, ServiceInstaller, StopState,
    };

    /// Shared observation state so we can inspect the poll after the fake is moved
    /// into the installer.
    #[derive(Default)]
    struct Obs {
        print_calls: usize,
        sleeps: usize,
        booted_out: bool,
    }

    /// A scripted `launchctl` that reports "loaded" for the first `loaded_for`
    /// `print` calls, then "unloaded", so the bootout poll spins until a `print`
    /// returns false. Records every call into a shared `Obs`.
    struct ScriptedLaunchctl {
        obs: Rc<RefCell<Obs>>,
        loaded_for: usize,
    }

    impl Launchctl for ScriptedLaunchctl {
        fn print_ok(&self, _domain: &str, _label: &str) -> bool {
            let mut o = self.obs.borrow_mut();
            let loaded = o.print_calls < self.loaded_for;
            o.print_calls += 1;
            loaded
        }
        fn bootout(&self, _domain: &str, _label: &str) {
            self.obs.borrow_mut().booted_out = true;
        }
        fn bootstrap(&self, _domain: &str, _plist: &Path) -> Result<(), ServiceError> {
            Ok(())
        }
        fn enable(&self, _domain: &str, _label: &str) {}
        fn sleep_poll(&self) {
            self.obs.borrow_mut().sleeps += 1;
        }
    }

    fn test_cfg() -> vigil::config::VigilConfig {
        // Hermetic config: a temp state dir so the tick-file removal is safe.
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("vigil.conf");
        std::fs::write(&conf, "").unwrap();
        let cfg = vigil::config::load(conf.to_str().unwrap(), None).unwrap();
        std::mem::forget(dir);
        cfg
    }

    #[test]
    fn bootout_polls_until_print_fails() {
        // print#0 = initial load check (loaded). bootout. Then poll: print#1..#3
        // loaded (3 sleeps), print#4 unloaded → break. loaded_for = 4.
        let obs = Rc::new(RefCell::new(Obs::default()));
        let lc = ScriptedLaunchctl {
            obs: Rc::clone(&obs),
            loaded_for: 4,
        };
        let installer = MacosLaunchdInstaller::with_launchctl(lc);
        let cfg = test_cfg();
        let state = installer.stop_user_agent(&cfg).unwrap();
        assert_eq!(state, StopState::BootedOut);
        let o = obs.borrow();
        assert!(o.booted_out, "bootout must be called when loaded");
        assert_eq!(o.sleeps, 3, "must poll (sleep) until print fails");
        // 5 prints total: 1 initial + 4 poll iterations (3 loaded + 1 unloaded).
        assert_eq!(o.print_calls, 5);
    }

    #[test]
    fn bootout_poll_is_bounded_at_50() {
        // Always-loaded: the poll must stop after exactly BOOTOUT_POLL_MAX sleeps.
        let obs = Rc::new(RefCell::new(Obs::default()));
        let lc = ScriptedLaunchctl {
            obs: Rc::clone(&obs),
            loaded_for: usize::MAX,
        };
        let installer = MacosLaunchdInstaller::with_launchctl(lc);
        let cfg = test_cfg();
        installer.stop_user_agent(&cfg).unwrap();
        let o = obs.borrow();
        assert_eq!(
            o.sleeps, BOOTOUT_POLL_MAX,
            "always-loaded poll must be bounded at 50×100ms"
        );
        assert_eq!(BOOTOUT_POLL_MAX, 50);
    }

    #[test]
    fn not_loaded_is_idempotent() {
        let obs = Rc::new(RefCell::new(Obs::default()));
        let lc = ScriptedLaunchctl {
            obs: Rc::clone(&obs),
            loaded_for: 0, // never loaded
        };
        let installer = MacosLaunchdInstaller::with_launchctl(lc);
        let cfg = test_cfg();
        let state = installer.stop_user_agent(&cfg).unwrap();
        assert_eq!(state, StopState::NotLoaded);
        let o = obs.borrow();
        assert!(!o.booted_out, "bootout must NOT run when not loaded");
        assert_eq!(o.sleeps, 0, "no poll when not loaded");
    }
}
