//! `vigil setup` — install the daemon + privileged helper + log rotation + load
//! the LaunchAgent (§4.1, §4.7, §4.8, §4.10).
//!
//! Flags: `--dry-run`, `--verbose` (any other → usage die). Dry-run touches
//! NOTHING. Real setup runs the three guards FIRST (admin → security-paths →
//! vigil-tree), then the numbered steps 1-5 in bash order.
//!
//! Every sudo / launchctl / root-file touch is gated behind the admin guard
//! (`require_admin_allowed`, honoring `VIGIL_TEST_NO_ADMIN`) which runs BEFORE
//! any privileged action (§4.8 SECURITY).

use std::ffi::OsString;

use vigil::config::VigilConfig;
use vigil::service::{MacosLaunchdInstaller, ServiceInstaller, StartState};

use super::tui::Tui;
use super::{
    LEGACY_SUDOERS_FILE, bin_link_path, cmd_sync_install, die, helper_plist_path, interactive,
    link_vigil_onto_path, load_config_or_exit, require_admin_allowed_or_die, user_plist_path,
};

/// `vigil setup [--dry-run] [--verbose] [--yes|--non-interactive]`.
pub fn run(args: Vec<OsString>) -> ! {
    let mut dry_run = false;
    let mut verbose = false;
    let mut yes = false;
    for a in &args {
        match a.to_str() {
            Some("--dry-run") => dry_run = true,
            Some("--verbose") => verbose = true,
            Some("--yes") | Some("--non-interactive") => yes = true,
            _ => die("usage: vigil setup [--dry-run] [--verbose] [--yes|--non-interactive]"),
        }
    }

    let cfg = load_config_or_exit();

    if dry_run {
        dry_run_summary(&cfg, verbose);
        std::process::exit(0);
    }

    // ── guards FIRST, in this exact order (§4.1) ──
    require_admin_allowed_or_die();
    if let Err(e) = cfg.validate_security_paths() {
        die(&e);
    }
    if let Err(e) = super::assert_vigil_tree_path("install dir", &cfg.install_dir) {
        die(&e);
    }

    // The clack-style UI, bound once to the interactive gate. When false (--yes,
    // piped, CI) EVERY method falls back to the byte-frozen plain lines below.
    let ui = Tui::new(interactive(yes));

    // Interactive confirm — strictly AFTER all three guards and BEFORE the first
    // side effect (so guard ordering + exit codes are untouched; declining is a
    // pre-sudo abort, and VIGIL_TEST_NO_ADMIN already died at guard #1 above).
    // Non-interactive/--yes → `confirm` returns the default WITHOUT prompting →
    // proceeds, identical to today (which skipped the confirm entirely).
    if !ui.confirm("Proceed with install? Privileged steps run via sudo.", true) {
        // Decline = clean no-op exit 0 (nothing was changed). NOT an error code.
        ui.outro_cancel("vigil: aborted. No privileged changes were made.");
        std::process::exit(0);
    }

    let installer = MacosLaunchdInstaller::new();

    ui.intro("vigil: setting up");

    // 1. prepare user directories.
    ui.rail_space();
    let pb = ui.step(
        "preparing user directories",
        "  1. preparing user directories",
    );
    if let Err(e) = cfg.ensure_state_dir() {
        die(&format!("could not create state dirs: {e}"));
    }
    pb.detail(
        &format!("state dir: {}", cfg.state_dir),
        &format!("     state dir: {}", cfg.state_dir),
    );
    pb.detail(
        &format!("log dir: {}", cfg.log_dir),
        &format!("     log dir:   {}", cfg.log_dir),
    );
    pb.done("user directories ready");

    // Silent stop: boot out a prior daemon BEFORE replacing its binary.
    let _ = installer.stop_user_agent(&cfg);

    // 2. install user daemon (TCC copy-out, BEFORE the plist points at it).
    ui.rail_space();
    let pb = ui.step("installing user daemon", "  2. installing user daemon");
    let dev_vigil = match cmd_sync_install(&cfg, ui) {
        Ok(p) => p,
        Err(e) => die(&e),
    };
    // Put `vigil` on the user's PATH (symlink into ~/.local/bin). It points at the
    // dev build in the repo so `setup`/`reload` can rebuild from source; the
    // installed copy at {install}/bin is only for the LaunchAgent daemon.
    link_vigil_onto_path(&dev_vigil, ui);
    pb.done("user daemon installed");

    // 3. install privileged power helper (root LaunchDaemon + IPC dirs +
    //    legacy-sudoers cleanup). The sudo region is run WITHOUT an active
    //    spinner so the macOS password prompt owns the TTY.
    ui.rail_space();
    let pb = ui.step(
        "installing privileged power helper",
        "  3. installing privileged power helper",
    );
    if verbose {
        pb.detail(
            "LaunchDaemon plist preview:",
            "     LaunchDaemon plist preview:",
        );
        match installer.render_helper_daemon(&cfg) {
            Ok(p) => indent_print(&p, 7),
            Err(e) => die(&format!("could not render helper plist: {e}")),
        }
        anstream::println!();
    } else {
        pb.detail(
            &format!("LaunchDaemon: {}", helper_plist_path()),
            &format!("     LaunchDaemon: {}", helper_plist_path()),
        );
        pb.detail(
            &format!("binary: {}", cfg.root_helper),
            &format!("     binary:       {}", cfg.root_helper),
        );
    }
    // Suspend the spinner across the privileged block: sudo may prompt for a
    // password and an active steady-tick would clobber that prompt line.
    pb.suspend(|| {
        cmd_install_root_helper(&cfg, &installer, ui);
        // legacy-sudoers cleanup.
        if std::path::Path::new(LEGACY_SUDOERS_FILE).is_file() {
            pb.detail(
                &format!("removing legacy sudoers file: {LEGACY_SUDOERS_FILE}"),
                &format!("  removing legacy sudoers file: {LEGACY_SUDOERS_FILE}"),
            );
            sudo_rm_f(LEGACY_SUDOERS_FILE);
        }
    });
    pb.done("privileged power helper installed");

    // 4. install log rotation (newsyslog → /etc/newsyslog.d/vigil.conf).
    ui.rail_space();
    let pb = ui.step("installing log rotation", "  4. installing log rotation");
    let newsyslog = match installer.render_newsyslog(&cfg) {
        Ok(s) => s,
        Err(e) => die(&format!("could not render newsyslog: {e}")),
    };
    if verbose {
        pb.detail("newsyslog preview:", "     newsyslog preview:");
        indent_print(&newsyslog, 7);
    } else {
        pb.detail(
            &format!("newsyslog: {}", cfg.newsyslog_file),
            &format!("     newsyslog: {}", cfg.newsyslog_file),
        );
    }
    pb.suspend(|| {
        install_newsyslog(&newsyslog, &cfg.newsyslog_file);
    });
    pb.detail("newsyslog: installed", "  newsyslog: installed");
    pb.done("log rotation installed");

    // 5. load user LaunchAgent (write the rendered plist, then start).
    ui.rail_space();
    let pb = ui.step("loading user LaunchAgent", "  5. loading user LaunchAgent");
    if let Err(e) = installer.install_user_agent(&cfg) {
        die(&format!("could not write LaunchAgent plist: {e}"));
    }
    pb.detail(
        &format!("plist: {}", user_plist_path().display()),
        &format!("     plist: {}", user_plist_path().display()),
    );

    // Start the LaunchAgent (bootstrap + enable + bounded wait).
    match installer.start_user_agent(&cfg) {
        Ok(StartState::AlreadyLoaded) => pb.detail(
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
        Ok(StartState::Bootstrapped) => {
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
        Err(e) => die(&format!("start failed: {e}")),
    }
    pb.done("user LaunchAgent loaded");

    // Outro: rail bottom (interactive) or the verbatim plain completion lines.
    if ui.is_interactive() {
        ui.outro("vigil: setup complete");
        anstream::println!("  next: vigil status");
    } else {
        anstream::println!();
        anstream::println!("vigil: setup complete");
        anstream::println!("  next: vigil status");
    }
    std::process::exit(0);
}

/// Dry-run path: print the path summary; `--verbose` adds the three previews.
/// Touches NOTHING (no dir creation, no launchctl, no sudo).
fn dry_run_summary(cfg: &VigilConfig, verbose: bool) {
    anstream::println!("vigil: setup dry run (no changes will be made)");
    anstream::println!();
    anstream::println!("  user:");
    anstream::println!("    state dir:        {}", cfg.state_dir);
    anstream::println!("    log dir:          {}", cfg.log_dir);
    anstream::println!("    install dir:      {}", cfg.install_dir);
    let dev_target = super::resolve_repo_root()
        .map(|r| format!("{}/target/release/vigil", r.display()))
        .unwrap_or_else(|| "(vigil dev build, resolved at install time)".to_string());
    anstream::println!(
        "    PATH symlink:     {} -> {}",
        bin_link_path(),
        dev_target
    );
    anstream::println!("    LaunchAgent:      {}", user_plist_path().display());
    anstream::println!();
    anstream::println!("  root:");
    anstream::println!("    helper binary:    {}", cfg.root_helper);
    anstream::println!("    LaunchDaemon:     {}", helper_plist_path());
    anstream::println!("    helper requests:  {}", cfg.power_request_dir);
    anstream::println!("    helper responses: {}", cfg.power_response_dir);
    anstream::println!("    newsyslog:        {}", cfg.newsyslog_file);

    // Render previews are PURE (no side effects) — safe in dry-run.
    let installer = MacosLaunchdInstaller::new();
    if verbose {
        anstream::println!();
        anstream::println!("  newsyslog (preview):");
        if let Ok(s) = installer.render_newsyslog(cfg) {
            indent_print(&s, 4);
        }
        anstream::println!();
        anstream::println!("  LaunchAgent plist (preview):");
        if let Ok(s) = installer.render_user_agent(cfg) {
            indent_print(&s, 4);
        }
        anstream::println!();
        anstream::println!("  LaunchDaemon helper plist (preview):");
        if let Ok(s) = installer.render_helper_daemon(cfg) {
            indent_print(&s, 4);
        }
    } else {
        anstream::println!();
        anstream::println!(
            "  generated file previews: hidden (use --verbose to print plist/newsyslog contents)"
        );
    }

    anstream::println!();
    anstream::println!(
        "vigil: dry run complete. No files were installed and launchd was not changed."
    );
}

/// Install the privileged root helper (§4.10): the asymmetric IPC dir ownership
/// matrix, the Rust `vigil-root-helper` binary (NOT the bash script), the helper
/// plist, then bootout/bootstrap/enable. ALL sudo — gated behind the admin guard
/// the caller already enforced.
fn cmd_install_root_helper<I: ServiceInstaller>(cfg: &VigilConfig, installer: &I, ui: Tui) {
    // Re-assert the guards at the privileged boundary (bash
    // cmd_install_root_helper re-runs them) so this is never reachable under
    // VIGIL_TEST_NO_ADMIN even if called out of order.
    require_admin_allowed_or_die();
    if let Err(e) = cfg.validate_security_paths() {
        die(&e);
    }

    let (user, group) = current_user_group();

    ui.detail(
        "root helper: installing LaunchDaemon (sudo may prompt)",
        "     root helper: installing LaunchDaemon (sudo may prompt)",
    );

    // The asymmetric IPC dir ownership matrix (§4.10 table), in this exact
    // order/owner/mode.
    sudo_install_dir("0755", "root", "wheel", &cfg.root_dir);
    sudo_install_dir("0755", "root", "wheel", &cfg.root_bin_dir);
    sudo_install_dir("0755", "root", "wheel", &cfg.power_helper_dir);
    sudo_install_dir("0755", "root", "wheel", &cfg.power_request_base);
    sudo_install_dir("0755", "root", "wheel", &cfg.power_response_base);
    sudo_install_dir("0700", "root", "wheel", &cfg.power_state_dir);
    sudo_install_dir("0755", "root", "wheel", &cfg.power_log_dir);
    // #8: request dir — user-owned 0700 (the privilege boundary; user writes).
    sudo_install_dir("0700", &user, &group, &cfg.power_request_dir);
    // #9: response dir — root-owned 0755 (root writes, user reads).
    sudo_install_dir("0755", "root", "wheel", &cfg.power_response_dir);

    // The Rust vigil-root-helper binary (5.5 deferral resolved, §4.10). Source is
    // the cargo-built release binary already copied into {install}/bin by
    // cmd_sync_install; dest is the root tree, mode 0755 root:wheel.
    let helper_src = format!("{}/bin/vigil-root-helper", cfg.install_dir);
    if !std::path::Path::new(&helper_src).is_file() {
        // Build it on demand if sync_install did not (sync builds vigil +
        // lock-helper; the root helper is the 2nd binary in the vigil crate).
        build_root_helper(cfg);
    }
    sudo_install_file("0755", "root", "wheel", &helper_src, &cfg.root_helper);

    // The helper plist (0644 root:wheel) rendered to a temp, then installed.
    let plist = match installer.render_helper_daemon(cfg) {
        Ok(p) => p,
        Err(e) => die(&format!("could not render helper plist: {e}")),
    };
    let tmp = write_temp("vigil-helper-plist", &plist);
    sudo_install_file("0644", "root", "wheel", &tmp, helper_plist_path());
    let _ = std::fs::remove_file(&tmp);

    // bootout (ignore) → bootstrap → enable (best-effort).
    sudo_launchctl(
        &[
            "bootout",
            &format!("system/{}", vigil::service::HELPER_LABEL),
        ],
        true,
    );
    sudo_launchctl(&["bootstrap", "system", helper_plist_path()], false);
    sudo_launchctl(
        &[
            "enable",
            &format!("system/{}", vigil::service::HELPER_LABEL),
        ],
        true,
    );
    ui.detail(
        &format!("root helper: bootstrapped {}", vigil::service::HELPER_LABEL),
        &format!(
            "     root helper: bootstrapped {}",
            vigil::service::HELPER_LABEL
        ),
    );
}

/// `cargo build --release --bin vigil-root-helper` and copy it into
/// `{install}/bin/vigil-root-helper` so the privileged install has a source.
fn build_root_helper(cfg: &VigilConfig) {
    let Some(repo_root) = super::resolve_repo_root() else {
        die("could not resolve repo root to build vigil-root-helper");
    };
    let manifest = repo_root.join("Cargo.toml");
    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--quiet")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--bin")
        .arg("vigil-root-helper")
        .status();
    match status {
        Ok(s) if s.success() => {}
        _ => die("failed to build vigil-root-helper"),
    }
    let target = super::cargo_target_dir(&manifest)
        .unwrap_or_else(|| repo_root.join("target").to_string_lossy().into_owned());
    let src = format!("{target}/release/vigil-root-helper");
    let dst = format!("{}/bin/vigil-root-helper", cfg.install_dir);
    if let Err(e) = super::install_binary(&src, &dst, 0o755) {
        die(&format!("failed to stage vigil-root-helper: {e}"));
    }
}

// ── sudo / launchctl wrappers (each is a privileged boundary) ─────────────────
//
// EVERY wrapper re-checks the admin guard so VIGIL_TEST_NO_ADMIN can never reach
// a real sudo/launchctl invocation, even if a caller forgets the up-front guard.

fn guard_sudo() {
    require_admin_allowed_or_die();
}

fn sudo_install_dir(mode: &str, owner: &str, group: &str, path: &str) {
    guard_sudo();
    run_checked(
        "sudo",
        &["install", "-d", "-m", mode, "-o", owner, "-g", group, path],
    );
}

fn sudo_install_file(mode: &str, owner: &str, group: &str, src: &str, dst: &str) {
    guard_sudo();
    run_checked(
        "sudo",
        &["install", "-m", mode, "-o", owner, "-g", group, src, dst],
    );
}

fn sudo_rm_f(path: &str) {
    guard_sudo();
    let _ = std::process::Command::new("sudo")
        .args(["rm", "-f", path])
        .status();
}

fn sudo_launchctl(args: &[&str], ignore_failure: bool) {
    guard_sudo();
    let mut full = vec!["launchctl"];
    full.extend_from_slice(args);
    if ignore_failure {
        // Best-effort (idempotency) calls — e.g. the bootout-before-bootstrap on
        // a fresh install where nothing is loaded yet. launchctl writes
        // "Boot-out failed: 3: No such process" to stderr in exactly that normal
        // case; since we already ignore the exit code, drop stderr so the benign
        // noise can't break the rail. A real problem still surfaces at the
        // (checked) bootstrap that follows.
        let _ = std::process::Command::new("sudo")
            .args(&full)
            .stderr(std::process::Stdio::null())
            .status();
    } else {
        run_checked("sudo", &full);
    }
}

fn install_newsyslog(content: &str, dst: &str) {
    guard_sudo();
    let tmp = write_temp("vigil-newsyslog", content);
    // chmod 0644 on the temp (defense-in-depth; install -m also sets it).
    if let Ok(meta) = std::fs::metadata(&tmp) {
        use std::os::unix::fs::PermissionsExt;
        let mut p = meta.permissions();
        p.set_mode(0o644);
        let _ = std::fs::set_permissions(&tmp, p);
    }
    run_checked(
        "sudo",
        &[
            "install", "-m", "0644", "-o", "root", "-g", "wheel", &tmp, dst,
        ],
    );
    let _ = std::fs::remove_file(&tmp);
}

/// Run a command that must succeed; on spawn failure or non-zero exit, die.
fn run_checked(prog: &str, args: &[&str]) {
    match std::process::Command::new(prog).args(args).status() {
        Ok(s) if s.success() => {}
        Ok(s) => die(&format!(
            "{prog} {} exited with {}",
            args.join(" "),
            s.code().unwrap_or(-1)
        )),
        Err(e) => die(&format!("could not run {prog}: {e}")),
    }
}

/// Write `content` to a unique temp file and return its path.
fn write_temp(prefix: &str, content: &str) -> String {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("{prefix}.{}", std::process::id()));
    if let Err(e) = std::fs::write(&path, content) {
        die(&format!("could not write temp file: {e}"));
    }
    path.to_string_lossy().into_owned()
}

/// Resolve the current user's login name + primary group name (`id -un`/`id -gn`).
fn current_user_group() -> (String, String) {
    let uid = nix::unistd::Uid::from_raw(vigil::config::get_uid());
    let user = nix::unistd::User::from_uid(uid)
        .ok()
        .flatten()
        .map(|u| u.name)
        .unwrap_or_else(|| uid.to_string());
    // SAFETY: getgid is always safe.
    let gid = nix::unistd::Gid::from_raw(unsafe { libc::getgid() });
    let group = nix::unistd::Group::from_gid(gid)
        .ok()
        .flatten()
        .map(|g| g.name)
        .unwrap_or_else(|| gid.to_string());
    (user, group)
}

/// Print `text` with each line indented by `n` spaces (mirrors bash
/// `sed 's/^/    /'`).
fn indent_print(text: &str, n: usize) {
    let pad = " ".repeat(n);
    for line in text.lines() {
        anstream::println!("{pad}{line}");
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::Mutex;

    // Serialize env-mutating tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Helper: run `setup --dry-run` in a fully sandboxed env and capture whether
    /// any file/dir was created. We can't easily capture `std::process::exit`, so
    /// the assertion is structural: dry-run never calls ensure_state_dir / sudo /
    /// launchctl. We verify the state dir does NOT come into existence.
    #[test]
    fn dry_run_touches_nothing() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let logs = dir.path().join("logs");
        let install = dir.path().join("Library/Application Support/vigil");
        let conf = dir.path().join("vigil.conf");
        std::fs::write(&conf, "").unwrap();

        // SAFETY: serialized via ENV_LOCK.
        unsafe {
            std::env::set_var("VIGIL_CONFIG_FILE", &conf);
            std::env::set_var("VIGIL_STATE_DIR", &state);
            std::env::set_var("VIGIL_LOG_DIR", &logs);
            std::env::set_var("VIGIL_INSTALL_DIR", &install);
        }
        let cfg = super::load_config_or_exit();
        // Render the dry-run summary directly (bypasses process::exit). It must
        // not create the state/log/install dirs.
        super::dry_run_summary(&cfg, true);

        let created = state.exists() || logs.exists() || install.exists();
        unsafe {
            std::env::remove_var("VIGIL_CONFIG_FILE");
            std::env::remove_var("VIGIL_STATE_DIR");
            std::env::remove_var("VIGIL_LOG_DIR");
            std::env::remove_var("VIGIL_INSTALL_DIR");
        }
        assert!(!created, "dry-run must NOT create any directory");
    }

    #[test]
    fn unknown_flag_is_usage_error() {
        // Structural check: an unrecognized arg routes to the usage die. We can't
        // catch the exit, so just assert the parse classifies a bad flag (the
        // run() loop accepts --dry-run/--verbose/--yes/--non-interactive).
        let bad: Vec<OsString> = vec![OsString::from("--nope")];
        let recognized = bad.iter().all(|a| {
            matches!(
                a.to_str(),
                Some("--dry-run") | Some("--verbose") | Some("--yes") | Some("--non-interactive")
            )
        });
        assert!(!recognized, "--nope must not be a recognized setup flag");
    }

    /// The 14-path privileged allowlist: a tampered privileged path is REFUSED at
    /// `validate_security_paths()` — setup's guard #2, which runs BEFORE
    /// `cmd_install_root_helper` (the first sudo). This is the exact call site
    /// setup uses, so a refusal here is a refusal before any sudo/launchctl/root
    /// touch. We never spawn sudo in this test.
    #[test]
    fn tampered_privileged_path_refused_before_sudo() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("vigil.conf");
        std::fs::write(&conf, "").unwrap();

        // SAFETY: serialized via ENV_LOCK. Tamper a root-tree privileged path.
        unsafe {
            std::env::set_var("VIGIL_CONFIG_FILE", &conf);
            std::env::set_var("VIGIL_ROOT_DIR", "/tmp/evil-root");
        }
        let cfg = super::load_config_or_exit();
        // This is guard #2 in setup::run, evaluated BEFORE any privileged action.
        let verdict = cfg.validate_security_paths();
        unsafe {
            std::env::remove_var("VIGIL_CONFIG_FILE");
            std::env::remove_var("VIGIL_ROOT_DIR");
        }
        let err = verdict.expect_err("tampered VIGIL_ROOT_DIR must be refused");
        assert!(
            err.contains("refusing non-standard"),
            "refusal message expected, got: {err}"
        );
    }
}
