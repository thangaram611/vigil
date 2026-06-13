//! `vigil doctor` — native grouped checklist + three-state resolution (§5.4).
//!
//! Consumes the SAME [`vigil::check::CheckEngine`] snapshot as `status` (for the
//! providers line + the `--power` power readings) and builds the grouped doctor
//! checklist on top of it. Three-state resolution (errs / warns / install_markers):
//!
//!   - `errs==0 && warns==0`  → ready                → exit 0
//!   - `errs==0 && warns>0`   → ready with warnings  → exit 0
//!   - `errs>0  && markers>0` → needs repair         → exit 1
//!   - `errs>0  && markers==0`→ not installed        → exit 1
//!
//! `lock helper` absent is the ONLY `warns++` site (the ready-with-warnings third
//! state). `doctor --power` runs its own `errs` counter → exit 0/1.

use std::ffi::OsString;
use std::path::Path;

use vigil::check::{CheckEngine, CheckMode, StatusSnapshot};
use vigil::config::VigilConfig;

use super::{load_config_or_exit, now_unix};

/// Parsed `doctor` invocation.
struct Opts {
    power: bool,
    verbose: bool,
}

fn usage_die() -> ! {
    anstream::eprintln!("usage: vigil doctor [--power] [--verbose]");
    std::process::exit(super::EX_ERROR);
}

fn parse(args: &[OsString]) -> Opts {
    let mut o = Opts {
        power: false,
        verbose: false,
    };
    for a in args {
        match a.to_str() {
            Some("--power") => o.power = true,
            Some("--verbose") => o.verbose = true,
            _ => usage_die(),
        }
    }
    o
}

/// Entry point for the `Doctor` dispatch arm. Returns `!` (always exits with the
/// three-state code or the `--power` code).
pub fn run(args: Vec<OsString>) -> ! {
    let opts = parse(&args);
    let cfg = load_config_or_exit();
    let now = now_unix();

    if opts.power {
        let report = CheckEngine::run(&cfg, CheckMode::Power, now);
        let code = render_power(&report.snapshot, opts.verbose);
        std::process::exit(code);
    }

    let report = CheckEngine::run(&cfg, CheckMode::Doctor, now);
    let code = render_doctor(&cfg, &report.snapshot, opts.verbose);
    std::process::exit(code);
}

// ── path helpers (read-only; bash $VIGIL_PLIST / $VIGIL_HELPER_PLIST) ─────────

/// `$HOME/Library/LaunchAgents/com.thangaram.vigil.plist` (bash `$VIGIL_PLIST`).
fn user_plist_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!(
        "{home}/Library/LaunchAgents/{}.plist",
        vigil::service::USER_AGENT_LABEL
    )
}

/// `/Library/LaunchDaemons/com.thangaram.vigil.helper.plist` — hardcoded, never
/// overridable (bash `$VIGIL_HELPER_PLIST`).
fn helper_plist_path() -> String {
    format!(
        "/Library/LaunchDaemons/{}.plist",
        vigil::service::HELPER_LABEL
    )
}

/// True iff `command -v <name>` would succeed (a regular executable on PATH, or an
/// absolute executable path). Read-only.
fn command_exists(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if name.contains('/') {
        return is_exec(Path::new(name));
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .filter(|d| !d.is_empty())
        .any(|d| is_exec(&Path::new(d).join(name)))
}

/// True iff `path` is an executable regular file (bash `[[ -x … ]]`).
fn is_executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// `launchctl print gui/{uid}/{label}` succeeds (bash launchd loaded probe).
fn launchd_loaded() -> bool {
    let uid = vigil::config::get_uid();
    let target = format!("gui/{uid}/{}", vigil::service::USER_AGENT_LABEL);
    std::process::Command::new("launchctl")
        .args(["print", &target])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// State-dir permission bits as the bash `stat -f %Lp` (lower 12 bits, octal).
fn dir_mode(path: &str) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_dir())
        .map(|m| m.permissions().mode() & 0o7777)
}

// ── the main grouped doctor (§5.4) ────────────────────────────────────────────

/// Render the full grouped checklist + three-state resolution. Returns the exit
/// code (0 ready/ready-with-warnings, 1 needs-repair/not-installed).
fn render_doctor(cfg: &VigilConfig, snap: &StatusSnapshot, verbose: bool) -> i32 {
    let mut errs = 0u32;
    let mut warns = 0u32;
    let mut install_markers = 0u32;

    anstream::println!("vigil doctor");
    anstream::println!();

    // platform
    anstream::println!("  platform");
    let arch = arch_string();
    anstream::println!("    cpu arch:      {arch}");
    if arch == "arm64" {
        anstream::println!(
            "    note:          Apple Silicon closed-lid sleep prevention is best-effort"
        );
        anstream::println!("                   See docs/apple-silicon-lid-closed.md");
    }

    // dependencies
    anstream::println!();
    anstream::println!("  dependencies");
    if command_exists("caffeinate") {
        anstream::println!("    caffeinate:    ok");
    } else {
        anstream::println!("    caffeinate:    missing (expected /usr/bin/caffeinate)");
        errs += 1;
    }
    if command_exists("pmset") {
        anstream::println!("    pmset:         ok");
    } else {
        anstream::println!("    pmset:         missing");
        errs += 1;
    }

    // privileged helper
    anstream::println!();
    anstream::println!("  privileged helper");
    anstream::println!("    power mode:    best-effort");
    if is_executable(&cfg.root_helper) {
        anstream::println!("    binary:        ok");
    } else {
        anstream::println!("    binary:        missing (run vigil setup)");
        errs += 1;
    }
    if Path::new(&helper_plist_path()).is_file() {
        anstream::println!("    LaunchDaemon:  ok");
    } else {
        anstream::println!("    LaunchDaemon:  missing (run vigil setup)");
        errs += 1;
    }
    if snap.power_helper_ok {
        anstream::println!("    IPC:           ok");
    } else {
        anstream::println!("    IPC:           fail (helper unavailable)");
        errs += 1;
    }
    if Path::new(&cfg.newsyslog_file).is_file() {
        anstream::println!("    log rotation:  ok");
    } else {
        anstream::println!("    log rotation:  missing (run vigil setup)");
        errs += 1;
    }

    // user agent
    anstream::println!();
    anstream::println!("  user agent");
    if Path::new(&user_plist_path()).is_file() {
        install_markers += 1;
        anstream::println!("    LaunchAgent:   ok");
    } else {
        anstream::println!("    LaunchAgent:   missing (run vigil setup)");
        errs += 1;
    }
    // The Rust daemon IS the installed `vigil` binary (`vigil daemon`); the bash
    // doctor checked `bin/vigil-daemon`. Post-5.7 the install copies `vigil`.
    let daemon_bin = format!("{}/bin/vigil", cfg.install_dir);
    if is_executable(&daemon_bin) {
        install_markers += 1;
        anstream::println!("    daemon:        ok");
    } else {
        anstream::println!("    daemon:        missing (run vigil setup)");
        errs += 1;
    }
    // lock helper absent is the ONLY warns++ site (ready-with-warnings).
    let lock_helper = format!("{}/bin/vigil-lock-helper", cfg.install_dir);
    if is_executable(&lock_helper) {
        anstream::println!("    lock helper:   ok");
    } else {
        anstream::println!(
            "    lock helper:   missing (run vigil setup/reload before using vigil lock)"
        );
        warns += 1;
    }
    if launchd_loaded() {
        anstream::println!("    launchd:       loaded");
    } else {
        anstream::println!(
            "    launchd:       not loaded (run vigil start if intentionally stopped)"
        );
    }
    if let Some(mode) = dir_mode(&cfg.state_dir) {
        install_markers += 1;
        anstream::println!("    state dir:     ok (mode {mode:o})");
    } else {
        anstream::println!("    state dir:     missing");
        errs += 1;
    }
    if Path::new(&cfg.log_dir).is_dir() {
        anstream::println!("    log dir:       ok");
    } else {
        anstream::println!("    log dir:       missing");
        errs += 1;
    }

    // providers
    anstream::println!();
    anstream::println!("  providers");
    anstream::println!("    agents:        {}", agents_line(snap));

    if verbose {
        anstream::println!();
        anstream::println!("  paths");
        anstream::println!("    root helper:   {}", cfg.root_helper);
        anstream::println!("    LaunchDaemon:  {}", helper_plist_path());
        anstream::println!("    newsyslog:     {}", cfg.newsyslog_file);
        anstream::println!("    LaunchAgent:   {}", user_plist_path());
        anstream::println!("    install dir:   {}", cfg.install_dir);
        anstream::println!("    state dir:     {}", cfg.state_dir);
        anstream::println!("    log dir:       {}", cfg.log_dir);
        anstream::println!();
        anstream::println!("  provider roots:");
        render_provider_roots(snap);
    } else {
        anstream::println!();
        anstream::println!(
            "  detail: use 'vigil doctor --verbose' for install paths and provider roots"
        );
    }

    // three-state resolution.
    anstream::println!();
    let state = if errs == 0 {
        if warns > 0 {
            "ready with warnings"
        } else {
            "ready"
        }
    } else if install_markers > 0 {
        "needs repair"
    } else {
        "not installed"
    };
    anstream::println!("state:  {state}");

    if errs == 0 {
        if warns > 0 {
            anstream::println!("result: required checks passed with {warns} warning(s)");
        } else {
            anstream::println!("result: all checks passed");
        }
        anstream::println!("next:   vigil status");
        0
    } else if state == "not installed" {
        anstream::println!("result: setup required ({errs} required check(s) missing)");
        anstream::println!("next:   vigil setup");
        1
    } else {
        anstream::println!("result: {errs} required check(s) failed");
        anstream::println!("next:   vigil setup");
        1
    }
}

// ── doctor --power (§5.4; its own errs counter) ───────────────────────────────

/// Render the focused power-path diagnostics. Returns 0 (all passed) or 1.
fn render_power(snap: &StatusSnapshot, verbose: bool) -> i32 {
    let mut errs = 0u32;
    anstream::println!("vigil power doctor");
    anstream::println!();
    let arch = arch_string();
    anstream::println!("  cpu arch:           {arch}");
    if arch == "arm64" {
        anstream::println!(
            "                      Apple Silicon: closed-lid operation is best-effort unless clamshell requirements are met."
        );
    }
    match resolve_command("pmset") {
        Some(p) => anstream::println!("  pmset:              {p}"),
        None => {
            anstream::println!("  pmset:              MISSING");
            errs += 1;
        }
    }
    match resolve_command("caffeinate") {
        Some(p) => anstream::println!("  caffeinate:         {p}"),
        None => {
            anstream::println!("  caffeinate:         MISSING");
            errs += 1;
        }
    }
    anstream::println!("  power hold mode:    best-effort");
    anstream::println!("  display sleep:      allowed (pmset disablesleep + caffeinate -i, no -d)");
    anstream::println!("  pmset disablesleep: {}", snap.pmset_disablesleep);
    let baseline = match snap.baseline {
        Some(v) => v.to_string(),
        None => "-".to_string(),
    };
    anstream::println!("  baseline:           {baseline}");
    let caff = match snap.caffeinate_pid {
        Some(p) => p.to_string(),
        None => "-".to_string(),
    };
    anstream::println!("  caffeinate pid:     {caff}");
    anstream::println!(
        "  caffeinate alive:   {}",
        if snap.caffeinate_alive { "yes" } else { "no" }
    );
    if snap.power_helper_ok {
        anstream::println!("  root helper:        ok");
    } else {
        anstream::println!("  root helper:        FAIL");
        errs += 1;
    }
    anstream::println!("  thermal:            {}", snap.thermal);
    anstream::println!("  battery:            {}", snap.battery);
    anstream::println!("  power assertions:   {}", power_assertions_summary(snap));

    if verbose {
        anstream::println!();
        anstream::println!("  provider roots:");
        render_provider_roots_indent2(snap);
        anstream::println!();
        anstream::println!("  power assertions:");
        render_assertion_rows(snap);
    } else {
        anstream::println!();
        anstream::println!(
            "  detail: use 'vigil doctor --power --verbose' for provider paths and assertion rows"
        );
    }

    anstream::println!();
    if errs == 0 {
        anstream::println!("result: power path checks passed");
        return 0;
    }
    anstream::println!("result: {errs} power path check(s) failed");
    anstream::println!("next:   vigil setup");
    1
}

// ── shared render helpers ─────────────────────────────────────────────────────

fn arch_string() -> String {
    std::process::Command::new("uname")
        .arg("-m")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::consts::ARCH.to_string())
}

/// `command -v <name>` resolved path (bash power-doctor prints the path).
fn resolve_command(name: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let is_exec = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    let path = std::env::var("PATH").ok()?;
    path.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| is_exec(p))
        .map(|p| p.display().to_string())
}

fn agents_line(snap: &StatusSnapshot) -> String {
    format!(
        "claude={}  codex={}  copilot={}  vscode_copilot_chat={}",
        snap.agent_claude.as_str(),
        snap.agent_codex.as_str(),
        snap.agent_copilot.as_str(),
        snap.agent_vscode_copilot_chat.as_str(),
    )
}

/// The power-doctor `power assertions:` inline summary (bash
/// `cmd_power_assertions_summary_line`).
fn power_assertions_summary(snap: &StatusSnapshot) -> String {
    match snap.power_assertions_state.as_str() {
        "none" => "none".to_string(),
        "parse_failed" => "parse failed".to_string(),
        _ => {
            let total = snap.power_assertions.len();
            if total == 0 {
                "none".to_string()
            } else {
                let vigil = snap.power_assertions.iter().filter(|a| a.vigil).count();
                if vigil > 0 {
                    format!("{total} active ({vigil} vigil)")
                } else {
                    format!("{total} active")
                }
            }
        }
    }
}

/// Provider-root rows for the main doctor verbose section (bash
/// `cmd_provider_roots_text | sed 's/^/    /'` → 4-space base shift).
fn render_provider_roots(snap: &StatusSnapshot) {
    for (name, p, state) in [
        ("claude", &snap.provider_claude, snap.agent_claude),
        ("codex", &snap.provider_codex, snap.agent_codex),
        ("copilot", &snap.provider_copilot, snap.agent_copilot),
    ] {
        anstream::println!("      {name:<7} home={}", p.home);
        anstream::println!(
            "              session={} exists={} state={}",
            p.session_dir,
            if p.exists { "yes" } else { "no" },
            state.as_str(),
        );
    }
}

/// Provider-root rows for the power-doctor verbose section (bash
/// `cmd_provider_roots_text | sed 's/^/  /'` → 2-space base shift).
fn render_provider_roots_indent2(snap: &StatusSnapshot) {
    for (name, p, state) in [
        ("claude", &snap.provider_claude, snap.agent_claude),
        ("codex", &snap.provider_codex, snap.agent_codex),
        ("copilot", &snap.provider_copilot, snap.agent_copilot),
    ] {
        anstream::println!("    {name:<7} home={}", p.home);
        anstream::println!(
            "            session={} exists={} state={}",
            p.session_dir,
            if p.exists { "yes" } else { "no" },
            state.as_str(),
        );
    }
}

/// Verbose assertion rows (bash: `(none)` / `(parse-failed; …)` pass through
/// 4-indented; holder rows become TSV-formatted lines).
fn render_assertion_rows(snap: &StatusSnapshot) {
    match snap.power_assertions_state.as_str() {
        "none" => anstream::println!("    (none)"),
        "parse_failed" => anstream::println!("    (parse-failed; raw output:)"),
        _ => {
            for a in &snap.power_assertions {
                let marker = if a.vigil { "← vigil" } else { "" };
                anstream::println!(
                    "    pid={:<7} {:<28} {:<32} {}",
                    a.pid,
                    a.process,
                    a.atype,
                    marker
                );
            }
        }
    }
}
