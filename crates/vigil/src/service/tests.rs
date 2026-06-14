//! Unit tests for `src/service/` (Phase 5.7 §2.2).
//!
//! Two thrusts:
//!   1. **Byte-stable render** — the rendered user-agent plist, helper plist, and
//!      newsyslog must match the Gate-0 goldens
//!      (`crates/vigil/tests/golden/{user_agent.plist,helper.plist,vigil.newsyslog}`)
//!      byte-for-byte, modulo values that legitimately differ by environment:
//!        - user-agent `ProgramArguments` (bash 1-element `bin/vigil-daemon` →
//!          Rust 2-element `[ bin/vigil, daemon ]`, deliberate per §2.1.1);
//!        - host uid / username (the goldens baked this host's `id -u`/`id -un`;
//!          a render test on a different host templates them from the live `id`,
//!          matching the bash render which calls `id` directly).
//!   2. **Launchctl seam** — the 50×100ms bootout poll loops until `print` fails,
//!      at most 50 times, 100ms each, with NO real launchctl/sudo.

use super::*;
use crate::config::VigilConfig;
use std::cell::RefCell;
use std::path::PathBuf;

const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

/// The fixed sandbox root the Gate-0 goldens were captured under.
const SBX: &str = "/private/tmp/vigil-golden-sbx";

/// Build a `VigilConfig` whose render-relevant fields exactly reproduce the
/// Gate-0 sandbox env (golden README "Common env"). Non-render fields get inert
/// values — only the paths the renderers read matter here.
fn golden_config() -> VigilConfig {
    // The hardcoded root-tree paths (NOT overridden in the golden env, so they
    // fall to their `/Library/Application Support/vigil` defaults).
    let root = "/Library/Application Support/vigil".to_string();
    VigilConfig {
        install_dir: format!("{SBX}/install"),
        state_dir: format!("{SBX}/state"),
        log_dir: format!("{SBX}/logs"),
        config_file: format!("{SBX}/no.conf"),

        active_dir: format!("{SBX}/state/active"),
        baseline_file: format!("{SBX}/state/baseline.json"),
        caffeinate_pidfile: format!("{SBX}/state/caffeinate.pid"),
        daemon_pidfile: format!("{SBX}/state/daemon.pid"),
        daemon_tick_file: format!("{SBX}/state/daemon.tick"),
        lock_file: format!("{SBX}/state/state.lock"),
        vscode_copilot_state_file: format!("{SBX}/state/vscode-copilot-chat.state"),

        // log_file = {log_dir}/daemon.log (newsyslog reads this).
        log_file: format!("{SBX}/logs/daemon.log"),

        root_dir: root.clone(),
        root_bin_dir: format!("{root}/bin"),
        root_helper: format!("{root}/bin/vigil-root-helper"),
        power_helper_dir: format!("{root}/helper"),
        power_request_base: format!("{root}/helper/requests"),
        power_response_base: format!("{root}/helper/responses"),
        // Golden pins these to the literal-`UID` sandbox path for reproducibility.
        power_request_dir: format!("{SBX}/install/helper/requests/UID"),
        power_response_dir: format!("{SBX}/install/helper/responses/UID"),
        power_state_dir: format!("{root}/helper/state"),
        power_log_dir: format!("{root}/helper/logs"),
        power_log_file: format!("{root}/helper/logs/helper.log"),
        power_helper_timeout_secs: 10,

        newsyslog_file: crate::config::NEWSYSLOG_FILE.to_string(),

        tick_secs: 5,
        stale_age_secs: 0,
        stale_cpu_pct: 0.0,
        thermal_cooldown_secs: 0,
        battery_floor_pct: 0,
        start_wait_secs: 6,
        lock_combo: String::new(),
        lock_max_secs: 0,
        lock_helper: format!("{SBX}/install/bin/vigil-lock-helper"),

        claude_home: format!("{SBX}/home/provider/claude"),
        claude_home_auto: false,
        codex_home: format!("{SBX}/home/provider/codex"),
        codex_home_auto: false,
        copilot_home: format!("{SBX}/home/provider/copilot"),
        copilot_home_auto: false,

        vscode_copilot_discover_secs: 0,
        vscode_copilot_recent_mins: 0,

        idle_after_sec: 0,
        thermal_cpu_limit_floor: None,
    }
}

fn read_golden(name: &str) -> String {
    std::fs::read_to_string(PathBuf::from(GOLDEN_DIR).join(name))
        .unwrap_or_else(|e| panic!("read golden {name}: {e}"))
}

/// Substitute the host-specific uid/user the goldens baked in for the values the
/// live render will produce (the bash render calls `id` directly, so a render
/// test on a different host must do the same — Gate-0 note #4).
fn retemplate_host_identity(golden: &str) -> String {
    // The capture host's values, hardcoded in the goldens.
    let golden_uid = "1993776753";
    let golden_user = "thanga-5521";
    let live_uid = crate::config::get_uid().to_string();
    let live_user = {
        let uid = nix::unistd::Uid::from_raw(crate::config::get_uid());
        nix::unistd::User::from_uid(uid)
            .ok()
            .flatten()
            .map(|u| u.name)
            .expect("live username")
    };
    golden
        .replace(golden_uid, &live_uid)
        .replace(golden_user, &live_user)
}

/// Replace the bash 1-element `ProgramArguments` array (which points at
/// `bin/vigil-daemon`) with the Rust 2-element form (`bin/vigil` + `daemon`), so
/// the rest of the user-agent golden can be asserted byte-for-byte. This encodes
/// the SOLE deliberate user-agent deviation (§2.1.1, Gate-0 note #2).
fn rustify_user_agent_program_args(golden: &str, install_dir: &str) -> String {
    let bash_block = format!(
        "    <array>\n        <string>{install_dir}/bin/vigil-daemon</string>\n    </array>"
    );
    let rust_block = format!(
        "    <array>\n        <string>{install_dir}/bin/vigil</string>\n        <string>daemon</string>\n    </array>"
    );
    assert!(
        golden.contains(&bash_block),
        "golden user_agent.plist must contain the bash ProgramArguments array"
    );
    golden.replace(&bash_block, &rust_block)
}

#[test]
fn user_agent_render_matches_golden() {
    let cfg = golden_config();
    let rendered = render_user_agent(&cfg);
    let expected =
        rustify_user_agent_program_args(&read_golden("user_agent.plist"), &cfg.install_dir);
    assert_eq!(
        rendered, expected,
        "rendered user-agent plist must match the golden byte-for-byte \
         (modulo the §2.1.1 ProgramArguments deviation)"
    );
}

#[test]
fn helper_render_matches_golden() {
    let cfg = golden_config();
    let rendered = render_helper(&cfg).expect("render helper");
    let expected = retemplate_host_identity(&read_golden("helper.plist"));
    assert_eq!(
        rendered, expected,
        "rendered helper plist must match the golden byte-for-byte (uid/user templated)"
    );
}

#[test]
fn newsyslog_render_matches_golden() {
    let cfg = golden_config();
    let rendered = render_newsyslog(&cfg).expect("render newsyslog");
    let expected = retemplate_host_identity(&read_golden("vigil.newsyslog"));
    assert_eq!(
        rendered, expected,
        "rendered newsyslog must match the golden byte-for-byte (user templated)"
    );
}

#[test]
fn user_agent_program_args_is_two_element_rust_form() {
    let cfg = golden_config();
    let m = user_agent_model(&cfg);
    assert_eq!(
        m.program_arguments,
        vec![
            format!("{}/bin/vigil", cfg.install_dir),
            "daemon".to_string()
        ],
        "the Rust LaunchAgent execs the installed vigil binary with `daemon` (§2.1.1)"
    );
}

#[test]
fn helper_program_args_is_frozen_14_element_argv() {
    let cfg = golden_config();
    let m = helper_model(&cfg).expect("helper model");
    // FROZEN order (§2.2.2). uid/user are live (host-dependent); assert structure.
    assert_eq!(m.program_arguments.len(), 14);
    assert_eq!(m.program_arguments[0], cfg.root_helper);
    assert_eq!(m.program_arguments[1], "--serve");
    assert_eq!(m.program_arguments[2], "--request-dir");
    assert_eq!(m.program_arguments[3], cfg.power_request_dir);
    assert_eq!(m.program_arguments[4], "--response-dir");
    assert_eq!(m.program_arguments[5], cfg.power_response_dir);
    assert_eq!(m.program_arguments[6], "--state-dir");
    assert_eq!(m.program_arguments[7], cfg.power_state_dir);
    assert_eq!(m.program_arguments[8], "--log-file");
    assert_eq!(m.program_arguments[9], cfg.power_log_file);
    assert_eq!(m.program_arguments[10], "--allowed-uid");
    assert_eq!(
        m.program_arguments[11],
        crate::config::get_uid().to_string()
    );
    assert_eq!(m.program_arguments[12], "--allowed-user");
    // [13] is the live username.
}

#[test]
fn labels_are_hardcoded_constants() {
    assert_eq!(USER_AGENT_LABEL, "com.thangaram.vigil");
    assert_eq!(HELPER_LABEL, "com.thangaram.vigil.helper");
    let cfg = golden_config();
    assert_eq!(user_agent_model(&cfg).label, "com.thangaram.vigil");
    assert_eq!(
        helper_model(&cfg).unwrap().label,
        "com.thangaram.vigil.helper"
    );
}

#[test]
fn helper_plist_has_no_environment_variables_dict() {
    let cfg = golden_config();
    let rendered = render_helper(&cfg).expect("render helper");
    assert!(
        !rendered.contains("EnvironmentVariables"),
        "the helper LaunchDaemon must NOT carry an EnvironmentVariables dict (asymmetry)"
    );
}

#[test]
fn typed_plists_are_serde_valid_via_plist_crate() {
    // Honor §2.2.2's "model both plists as #[derive(Serialize)] structs" intent:
    // the typed model must be a valid plist the `plist` crate can serialize. (The
    // shipping renderer is byte-stable and golden-driven; this proves the data
    // model itself is sound and round-trippable.)
    let cfg = golden_config();
    let ua = user_agent_model(&cfg);
    let mut buf = Vec::new();
    plist::to_writer_xml(&mut buf, &ua).expect("user-agent plist serializes");
    let parsed: plist::Value = plist::from_bytes(&buf).expect("user-agent plist re-parses");
    let dict = parsed.as_dictionary().expect("dict");
    assert_eq!(
        dict.get("Label").and_then(|v| v.as_string()),
        Some("com.thangaram.vigil")
    );
    assert_eq!(
        dict.get("RunAtLoad").and_then(|v| v.as_boolean()),
        Some(true)
    );
    assert_eq!(
        dict.get("ExitTimeOut").and_then(|v| v.as_signed_integer()),
        Some(60)
    );

    let helper = helper_model(&cfg).expect("helper model");
    let mut hbuf = Vec::new();
    plist::to_writer_xml(&mut hbuf, &helper).expect("helper plist serializes");
    let hparsed: plist::Value = plist::from_bytes(&hbuf).expect("helper plist re-parses");
    let hdict = hparsed.as_dictionary().expect("dict");
    assert_eq!(
        hdict.get("Label").and_then(|v| v.as_string()),
        Some("com.thangaram.vigil.helper")
    );
    assert_eq!(
        hdict.get("ExitTimeOut").and_then(|v| v.as_signed_integer()),
        Some(10)
    );
    assert!(hdict.get("EnvironmentVariables").is_none());
}

#[test]
fn xml_special_chars_are_escaped_by_library_primitive() {
    // A path with all five XML special chars must come out as the plist crate's
    // escaping (& < > " ') — proving we delegate escaping, not hand-roll it.
    let raw = r#"a & b < c > d " e ' f"#;
    let escaped = xesc(raw);
    assert_eq!(escaped, "a &amp; b &lt; c &gt; d &quot; e &apos; f");
}

// ── launchctl seam: the 50×100ms bootout poll ─────────────────────────────────

/// A scripted fake `launchctl`. `print_results` is a queue of booleans consumed
/// one-per-`print_ok` call; `sleeps` and `bootouts` count the poll behavior.
struct FakeLaunchctl {
    print_results: RefCell<std::collections::VecDeque<bool>>,
    print_calls: RefCell<usize>,
    bootout_calls: RefCell<usize>,
    bootstrap_calls: RefCell<usize>,
    enable_calls: RefCell<usize>,
    sleep_calls: RefCell<usize>,
}

impl FakeLaunchctl {
    fn new(print_results: Vec<bool>) -> Self {
        FakeLaunchctl {
            print_results: RefCell::new(print_results.into_iter().collect()),
            print_calls: RefCell::new(0),
            bootout_calls: RefCell::new(0),
            bootstrap_calls: RefCell::new(0),
            enable_calls: RefCell::new(0),
            sleep_calls: RefCell::new(0),
        }
    }
}

impl Launchctl for FakeLaunchctl {
    fn print_ok(&self, _domain: &str, _label: &str) -> bool {
        *self.print_calls.borrow_mut() += 1;
        // After the scripted queue drains, default to "not loaded" (false) so an
        // unbounded poll cannot hang the test.
        self.print_results.borrow_mut().pop_front().unwrap_or(false)
    }
    fn bootout(&self, _domain: &str, _label: &str) {
        *self.bootout_calls.borrow_mut() += 1;
    }
    fn bootstrap(&self, _domain: &str, _plist: &std::path::Path) -> Result<(), ServiceError> {
        *self.bootstrap_calls.borrow_mut() += 1;
        Ok(())
    }
    fn enable(&self, _domain: &str, _label: &str) {
        *self.enable_calls.borrow_mut() += 1;
    }
    fn sleep_poll(&self) {
        // No real sleep — just count (proves the loop calls it without waiting).
        *self.sleep_calls.borrow_mut() += 1;
    }
}

#[test]
fn stop_polls_until_print_fails_then_boots_out() {
    // print sequence: [true (loaded gate), true, true, false] →
    //   gate print = loaded → bootout, then poll: print=true, sleep; print=true,
    //   sleep; print=false → break. Exactly 2 sleeps, 1 bootout.
    let cfg = golden_config();
    let fake = FakeLaunchctl::new(vec![true, true, true, false]);
    let installer = MacosLaunchdInstaller::with_launchctl(fake);
    let state = installer.stop_user_agent(&cfg).expect("stop");
    assert_eq!(state, StopState::BootedOut);
    assert_eq!(*installer.launchctl.bootout_calls.borrow(), 1);
    assert_eq!(*installer.launchctl.sleep_calls.borrow(), 2);
}

#[test]
fn stop_poll_is_bounded_at_exactly_50() {
    // Gate print = true (loaded). Then `print` ALWAYS true → the poll must stop at
    // exactly BOOTOUT_POLL_MAX iterations (50 sleeps), never unbounded.
    let cfg = golden_config();
    let mut results = vec![true]; // gate
    results.extend(std::iter::repeat_n(true, 200)); // always loaded
    let fake = FakeLaunchctl::new(results);
    let installer = MacosLaunchdInstaller::with_launchctl(fake);
    let state = installer.stop_user_agent(&cfg).expect("stop");
    assert_eq!(state, StopState::BootedOut);
    assert_eq!(
        *installer.launchctl.sleep_calls.borrow(),
        BOOTOUT_POLL_MAX,
        "the bootout poll must be bounded at exactly 50 × 100ms"
    );
    assert_eq!(BOOTOUT_POLL_MAX, 50);
}

#[test]
fn stop_when_not_loaded_is_idempotent_noop() {
    // Gate print = false → not loaded; no bootout, no poll.
    let cfg = golden_config();
    let fake = FakeLaunchctl::new(vec![false]);
    let installer = MacosLaunchdInstaller::with_launchctl(fake);
    let state = installer.stop_user_agent(&cfg).expect("stop");
    assert_eq!(state, StopState::NotLoaded);
    assert_eq!(*installer.launchctl.bootout_calls.borrow(), 0);
    assert_eq!(*installer.launchctl.sleep_calls.borrow(), 0);
}

#[test]
fn start_already_loaded_is_idempotent() {
    // Write a plist file so the missing-plist guard passes, then gate print=true.
    let tmp = tempfile::tempdir().unwrap();
    let agents = tmp.path().join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join(format!("{USER_AGENT_LABEL}.plist")), "x").unwrap();
    let _home = EnvGuard::set("HOME", tmp.path().to_str().unwrap());

    let cfg = golden_config();
    let fake = FakeLaunchctl::new(vec![true]); // already loaded
    let installer = MacosLaunchdInstaller::with_launchctl(fake);
    let state = installer.start_user_agent(&cfg).expect("start");
    assert_eq!(state, StartState::AlreadyLoaded);
    assert_eq!(*installer.launchctl.bootstrap_calls.borrow(), 0);
}

#[test]
fn start_bootstraps_and_enables_when_unloaded() {
    let tmp = tempfile::tempdir().unwrap();
    let agents = tmp.path().join("Library/LaunchAgents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join(format!("{USER_AGENT_LABEL}.plist")), "x").unwrap();
    let _home = EnvGuard::set("HOME", tmp.path().to_str().unwrap());

    let cfg = golden_config();
    let fake = FakeLaunchctl::new(vec![false]); // not loaded
    let installer = MacosLaunchdInstaller::with_launchctl(fake);
    let state = installer.start_user_agent(&cfg).expect("start");
    assert_eq!(state, StartState::Bootstrapped);
    assert_eq!(*installer.launchctl.bootstrap_calls.borrow(), 1);
    assert_eq!(*installer.launchctl.enable_calls.borrow(), 1);
}

#[test]
fn start_errors_when_plist_missing() {
    let tmp = tempfile::tempdir().unwrap();
    // No plist written under HOME/Library/LaunchAgents.
    let _home = EnvGuard::set("HOME", tmp.path().to_str().unwrap());
    let cfg = golden_config();
    let fake = FakeLaunchctl::new(vec![]);
    let installer = MacosLaunchdInstaller::with_launchctl(fake);
    let err = installer.start_user_agent(&cfg).unwrap_err();
    assert!(matches!(err, ServiceError::PlistMissing(_)));
    // print must NOT have been called (the guard fires first).
    assert_eq!(*installer.launchctl.print_calls.borrow(), 0);
}

/// Serialize + scope HOME mutation (process-wide env is not thread-safe in
/// edition 2024). Restores the prior value on drop.
struct EnvGuard {
    key: String,
    prev: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl EnvGuard {
    fn set(key: &str, value: &str) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var(key).ok();
        // SAFETY: serialized via ENV_LOCK; restored on drop.
        unsafe { std::env::set_var(key, value) };
        EnvGuard {
            key: key.to_string(),
            prev,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: still holding ENV_LOCK.
        match &self.prev {
            Some(v) => unsafe { std::env::set_var(&self.key, v) },
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}
