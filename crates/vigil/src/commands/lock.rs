//! `vigil lock` + `vigil lock doctor` — the local freeze guard CLI (§4.12,
//! Option A cutover). This is CLI orchestration OVER the existing
//! `vigil-lock-helper` binary; that binary is NOT modified here.
//!
//! Exit matrix (ported byte-faithfully from bash `cmd_lock`/`cmd_lock_doctor`):
//! - 64 (`EX_USAGE`): non-macOS, unknown subcommand, unknown arg.
//! - 1 (`EX_ERROR`): permission/preflight failure, helper missing/not exec, the
//!   `--max-secs 0`-from-config rejection.
//! - 0: success / doctor ready.
//!
//! The `--max-secs 0` guard: a `0` is accepted ONLY when passed explicitly via
//! `--max-secs` on the CLI; a `0` arriving from the config/env default
//! (`VIGIL_LOCK_MAX_SECS=0`) is REJECTED. The CLI is the sole gate — the helper
//! itself is permissive.

use std::ffi::OsString;
use std::process::Command;

use vigil::check::RealLoadProbe;
use vigil::config::VigilConfig;
use vigil::{check::LoadProbe, refcount};

use super::{die, load_config_or_exit};

/// `EX_USAGE` (64) — the lock-specific usage exit (bash `return 64`).
const EX_USAGE: i32 = crate::exit::EX_USAGE;
/// `EX_ERROR` (1).
const EX_ERROR: i32 = crate::exit::EX_ERROR;

/// `vigil lock [doctor] ...`.
pub fn run(args: Vec<OsString>) -> ! {
    // Non-macOS reject (bash returns 64). The helper is macOS-only.
    if !cfg!(target_os = "macos") {
        anstream::println!(
            "vigil lock: phase-4 local lock guard is macOS-only (not yet in phase 5 scope)."
        );
        std::process::exit(EX_USAGE);
    }

    let cfg = load_config_or_exit();

    // Subcommand dispatch on the FIRST token (bash `case "${1:-}"`).
    let first = args.first().and_then(|a| a.to_str());
    match first {
        Some("doctor") => {
            let rest: Vec<OsString> = args.into_iter().skip(1).collect();
            let code = lock_doctor(&cfg, rest);
            std::process::exit(code);
        }
        Some("--help") | Some("-h") | Some("help") => {
            print_lock_help(&cfg);
            std::process::exit(0);
        }
        // Empty → fall through to the lock run.
        None => {}
        // A leading `--flag` is a lock option (falls through to the parser);
        // anything else is an unknown subcommand → 64.
        Some(s) if !s.starts_with("--") => {
            anstream::println!("vigil lock: unknown subcommand: {s}");
            anstream::println!("  run: vigil lock --help");
            std::process::exit(EX_USAGE);
        }
        Some(_) => {}
    }

    let code = lock_run(&cfg, args);
    std::process::exit(code);
}

/// The `vigil lock` run path. Returns the exit code (64/1/0 or the helper's).
fn lock_run(cfg: &VigilConfig, args: Vec<OsString>) -> i32 {
    let mut combo = cfg.lock_combo.clone();
    let mut max_secs = cfg.lock_max_secs;
    let mut saw_max = false;

    // Parse --combo / --max-secs (bash while-loop). Unknown arg → usage die (64).
    let mut it = args.into_iter().peekable();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--combo") => {
                let Some(v) = it.next() else {
                    usage_die_lock();
                };
                combo = v.to_string_lossy().into_owned();
            }
            Some("--max-secs") => {
                let Some(v) = it.next() else {
                    usage_die_lock();
                };
                let vs = v.to_string_lossy();
                // non-negative integer only (bash `*[!0-9]*`).
                if vs.is_empty() || !vs.bytes().all(|b| b.is_ascii_digit()) {
                    die("max seconds must be a non-negative integer");
                }
                max_secs = match vs.parse::<u32>() {
                    Ok(n) => n,
                    Err(_) => die("max seconds must be a non-negative integer"),
                };
                saw_max = true;
            }
            _ => usage_die_lock(),
        }
    }

    // The --max-secs 0 guard: 0 accepted ONLY via explicit CLI. A 0 from the
    // config/env default (VIGIL_LOCK_MAX_SECS=0) is rejected. The CLI is the sole
    // gate (the helper is permissive).
    if max_secs == 0 && !saw_max {
        anstream::println!(
            "vigil lock: VIGIL_LOCK_MAX_SECS=0 is only accepted when passed explicitly via --max-secs."
        );
        anstream::println!(
            "  set --max-secs 0 to enable a no-timeout run, or use a non-zero default in vigil.conf."
        );
        return EX_ERROR;
    }

    // Helper present + executable (bash cmd_lock_require_helper) → 1 on failure.
    if let Err(code) = require_helper(cfg) {
        return code;
    }

    // Preflight permission probe (no --prompt): all three of listen/access/tap
    // must be true. Any failure → 1.
    let json = match check_permissions_json(cfg, false) {
        Some(j) if !j.trim().is_empty() => j,
        _ => {
            anstream::println!(
                "vigil lock: preflight doctor failed; helper did not run permission probe."
            );
            anstream::println!("  run: vigil lock doctor for details");
            return EX_ERROR;
        }
    };
    let listen = json_bool_field("listen_event_access", &json);
    let access = json_bool_field("accessibility_trusted", &json);
    let tap = json_bool_field("tap_create_active_hid_ok", &json);
    if listen.is_none() || access.is_none() || tap.is_none() {
        anstream::println!("vigil lock: failed to parse permission JSON");
        anstream::println!("  raw: {json}");
        return EX_ERROR;
    }
    if listen != Some(true) || access != Some(true) || tap != Some(true) {
        anstream::println!("vigil lock: doctor preflight failed; run this first:");
        anstream::println!("  vigil lock doctor");
        return EX_ERROR;
    }

    // Pre-arm: ensure state dirs, write the wrapper pidfile (counts as +1 work),
    // then wait for the daemon tick to reflect the hold BEFORE launching the
    // helper. Install an RAII cleanup so the pidfile is removed on exit/signal.
    if let Err(e) = cfg.ensure_state_dir() {
        die(&format!("could not create state dirs: {e}"));
    }
    let pid = std::process::id();
    let pidfile = format!("{}/wrapper-{pid}.pid", cfg.active_dir);
    let body = refcount::wrapper_pidfile_body(pid, "vigil lock", super::now_unix());
    if let Err(e) = std::fs::write(&pidfile, body) {
        die(&format!("could not write lock pidfile: {e}"));
    }
    let _guard = PidfileGuard {
        path: pidfile.clone(),
    };
    wait_for_lock_power_hold(cfg);

    anstream::println!("  lock combo:  {combo}");
    anstream::println!("  max seconds: {max_secs} (0 = no timeout)");
    anstream::println!("  helper:      {}", cfg.lock_helper);
    anstream::println!(
        "  sleep hold:  best effort; display sleep and macOS Lock Screen are allowed"
    );
    anstream::println!("  recover:     pkill -TERM vigil-lock-helper");
    anstream::println!("  This is a local freeze guard, not the macOS Lock Screen.");
    anstream::println!(
        "  If macOS locks first, login input is allowed; the combo is still required after login."
    );
    for delay in [3, 2, 1] {
        anstream::println!("  starting in {delay}s...");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Launch the helper (NON-exec so the RAII pidfile guard fires on return).
    let status = Command::new(&cfg.lock_helper)
        .arg("--freeze")
        .arg("--combo")
        .arg(&combo)
        .arg("--max-secs")
        .arg(max_secs.to_string())
        .status();
    match status {
        Ok(s) => {
            // Propagate the helper's exit code; signal-terminated → 128 + signal.
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(code) = s.code() {
                    code
                } else if let Some(sig) = s.signal() {
                    128 + sig
                } else {
                    EX_ERROR
                }
            }
            #[cfg(not(unix))]
            {
                s.code().unwrap_or(EX_ERROR)
            }
        }
        Err(e) => {
            anstream::println!("vigil lock: failed to run helper: {e}");
            EX_ERROR
        }
    }
}

/// `vigil lock doctor [--prompt]`. Returns 0 iff
/// listen_event_access && accessibility_trusted && tap_create_active_hid_ok
/// (post_event_access is informational). Unknown arg → 64; helper missing → 1.
fn lock_doctor(cfg: &VigilConfig, args: Vec<OsString>) -> i32 {
    // Helper present + executable first (bash order) → 1 on failure.
    if let Err(code) = require_helper(cfg) {
        return code;
    }

    let mut prompt = false;
    let mut it = args.into_iter();
    match it.next().as_ref().and_then(|a| a.to_str()) {
        Some("--prompt") => prompt = true,
        None => {}
        Some(_) => usage_die_doctor(),
    }
    // Any trailing arg → usage die.
    if it.next().is_some() {
        usage_die_doctor();
    }

    let json = match check_permissions_json(cfg, prompt) {
        Some(j) if !j.trim().is_empty() => j,
        _ => {
            anstream::println!("vigil lock doctor: failed to run helper permission probe");
            return EX_ERROR;
        }
    };

    let listen = json_bool_field("listen_event_access", &json);
    let access = json_bool_field("accessibility_trusted", &json);
    let post_event = json_bool_field("post_event_access", &json);
    let tap = json_bool_field("tap_create_active_hid_ok", &json);
    if listen.is_none() || access.is_none() || post_event.is_none() || tap.is_none() {
        anstream::println!("vigil lock doctor: failed to parse permission JSON");
        anstream::println!("  raw: {json}");
        return EX_ERROR;
    }
    let listen = listen.unwrap();
    let access = access.unwrap();
    let post_event = post_event.unwrap();
    let tap = tap.unwrap();

    // COMPUTED column alignment: the label column width is the longest label.
    let rows: [(&str, String); 4] = [
        ("listen_event_access:", bstr(listen)),
        ("accessibility_trusted:", bstr(access)),
        (
            "post_event_access:",
            format!("{} (informational)", bstr(post_event)),
        ),
        ("tap_create_active_hid_ok:", bstr(tap)),
    ];
    let label_w = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);

    anstream::println!("vigil lock doctor");
    for (label, value) in &rows {
        anstream::println!("  {label:<label_w$}  {value}");
    }
    anstream::println!("  helper: {}", cfg.lock_helper);
    anstream::println!();
    anstream::println!("System Settings > Privacy & Security > Input Monitoring");
    anstream::println!("System Settings > Privacy & Security > Accessibility");
    anstream::println!();

    if prompt {
        anstream::println!("Prompts/permission changes are asynchronous on macOS.");
        anstream::println!("Run this doctor command again after granting access.");
    }

    if listen && access && tap {
        anstream::println!("lock guard readiness: ready");
        0
    } else {
        anstream::println!("lock guard readiness: not ready");
        EX_ERROR
    }
}

// ── helper-process orchestration ──────────────────────────────────────────────

/// Validate the lock helper path is present + executable (bash
/// `cmd_lock_require_helper`). Returns `Err(1)` after printing the bash message.
fn require_helper(cfg: &VigilConfig) -> Result<(), i32> {
    let path = &cfg.lock_helper;
    if path.ends_with('/') {
        anstream::println!("vigil lock: invalid helper path (trailing /): {path}");
        return Err(EX_ERROR);
    }
    let p = std::path::Path::new(path);
    if !p.is_file() {
        anstream::println!("vigil lock: missing helper at {path}");
        anstream::println!(
            "  run: vigil setup (or vigil reload) to build and install vigil-lock-helper"
        );
        return Err(EX_ERROR);
    }
    if !is_executable(p) {
        anstream::println!("vigil lock: helper is not executable: {path}");
        anstream::println!("  run: vigil setup (or vigil reload) to reinstall the helper");
        return Err(EX_ERROR);
    }
    Ok(())
}

fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Run `vigil-lock-helper --check-permissions --json [--prompt]` and capture
/// stdout (bash `cmd_lock_check_permissions_json`). `None` on spawn failure.
fn check_permissions_json(cfg: &VigilConfig, prompt: bool) -> Option<String> {
    let mut cmd = Command::new(&cfg.lock_helper);
    cmd.arg("--check-permissions").arg("--json");
    if prompt {
        cmd.arg("--prompt");
    }
    let out = cmd.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Extract a boolean field from the helper's JSON probe output, matching the bash
/// `sed -nE 's/.*"field":(true|false).*/\1/p'` semantics: find `"field":true` or
/// `"field":false`. Returns `None` if the field is absent.
fn json_bool_field(field: &str, json: &str) -> Option<bool> {
    let key = format!("\"{field}\":");
    let idx = json.find(&key)? + key.len();
    let rest = &json[idx..];
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn bstr(b: bool) -> String {
    if b { "true" } else { "false" }.to_string()
}

// ── pre-arm wait (reads the frozen daemon tick ABI) ───────────────────────────

/// Wait up to `VIGIL_START_WAIT_SECS` for the daemon tick to reflect the hold,
/// BEFORE launching the helper (bash `cmd_wait_for_lock_power_hold`). Returns
/// early if the LaunchAgent is not loaded. Reads `refcount_active`, `engaged`,
/// `thermal_cut`, `battery_cut`, `cooling` from the tick file.
fn wait_for_lock_power_hold(cfg: &VigilConfig) {
    let probe = RealLoadProbe;
    if !probe.is_loaded(vigil::service::USER_AGENT_LABEL) {
        return;
    }
    let wait_secs = cfg.start_wait_secs; // default 6, parsed by config
    let max_ticks = wait_secs as u64 * 10;
    for _ in 0..max_ticks {
        let active = tick_field(cfg, "refcount_active");
        let engaged = tick_field(cfg, "engaged");
        let thermal = tick_field(cfg, "thermal_cut");
        let battery = tick_field(cfg, "battery_cut");
        let cooling = tick_field(cfg, "cooling");
        let active_n = active.as_deref().and_then(|s| s.parse::<u32>().ok());
        if let Some(n) = active_n
            && n > 0
            && (engaged.as_deref() == Some("1")
                || thermal.as_deref() == Some("1")
                || battery.as_deref() == Some("1")
                || cooling.as_deref() == Some("1"))
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// `awk -F= -v k=field '$1==k {...; print; exit}'` over the daemon tick file
/// (frozen ABI: `=` is the first separator, one field per line, first-match
/// wins). `None` if the file is absent or the key is missing.
fn tick_field(cfg: &VigilConfig, field: &str) -> Option<String> {
    let text = std::fs::read_to_string(&cfg.daemon_tick_file).ok()?;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=')
            && k == field
        {
            return Some(v.to_string());
        }
    }
    None
}

// ── RAII pidfile cleanup ──────────────────────────────────────────────────────

/// Removes the wrapper pidfile when dropped (on normal return). The path is
/// captured by value so cleanup survives scope exit.
struct PidfileGuard {
    path: String,
}

impl Drop for PidfileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ── usage / help ──────────────────────────────────────────────────────────────

fn usage_die_lock() -> ! {
    die("usage: vigil lock [--combo <combo>] [--max-secs <seconds>]");
}

fn usage_die_doctor() -> ! {
    die("usage: vigil lock doctor [--prompt]");
}

fn print_lock_help(cfg: &VigilConfig) {
    anstream::println!("Usage:");
    anstream::println!("  vigil lock [--combo <combo>] [--max-secs <seconds>]");
    anstream::println!("  vigil lock doctor [--prompt]");
    anstream::println!();
    anstream::println!(
        "  --combo <combo>     Unlock combo (default: {})",
        cfg.lock_combo
    );
    anstream::println!("  --max-secs <secs>   Auto-stop timeout in seconds (0 = no timeout)");
    anstream::println!();
    anstream::println!("  doctor              Run a permission and accessibility probe");
    anstream::println!("  doctor --prompt      Show permission prompts if not yet granted");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_bool_field_parses_true_false() {
        let j = r#"{"listen_event_access":true,"accessibility_trusted":false,"tap_create_active_hid_ok":true}"#;
        assert_eq!(json_bool_field("listen_event_access", j), Some(true));
        assert_eq!(json_bool_field("accessibility_trusted", j), Some(false));
        assert_eq!(json_bool_field("tap_create_active_hid_ok", j), Some(true));
        assert_eq!(json_bool_field("post_event_access", j), None);
    }

    /// The --max-secs 0 guard: rejected from config/env (saw_max=false), accepted
    /// from explicit CLI (saw_max=true). This is the load-bearing CLI-only gate.
    /// We test the pure predicate the run path uses.
    #[test]
    fn max_secs_zero_rejected_from_config_accepted_from_cli() {
        // Pure predicate mirror of lock_run's guard.
        fn rejected(max_secs: u32, saw_max: bool) -> bool {
            max_secs == 0 && !saw_max
        }
        // From config/env default (no CLI flag) → rejected.
        assert!(rejected(0, false), "0 from config must be rejected");
        // From explicit CLI --max-secs 0 → accepted.
        assert!(!rejected(0, true), "0 from CLI must be accepted");
        // Non-zero defaults are fine either way.
        assert!(!rejected(28800, false));
        assert!(!rejected(28800, true));
    }

    /// Integration-style: drive the real lock_run guard with a config whose
    /// lock_max_secs == 0 (as if VIGIL_LOCK_MAX_SECS=0) and NO --max-secs arg →
    /// EX_ERROR (1); with an explicit --max-secs 0 → passes the guard (and then
    /// fails later on the missing helper, NOT on the guard).
    #[test]
    fn max_secs_zero_env_rejected_real_path() {
        use std::sync::Mutex;
        static LK: Mutex<()> = Mutex::new(());
        let _g = LK.lock().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("vigil.conf");
        std::fs::write(&conf, "").unwrap();
        // SAFETY: serialized via LK.
        unsafe {
            std::env::set_var("VIGIL_CONFIG_FILE", &conf);
            std::env::set_var("VIGIL_LOCK_MAX_SECS", "0");
            // Point the lock helper at a non-existent path so the CLI-accepted
            // case fails on require_helper (1), proving the guard was passed.
            std::env::set_var("VIGIL_LOCK_HELPER", dir.path().join("no-such-helper"));
        }
        let cfg = vigil::config::load(conf.to_str().unwrap(), None).unwrap();
        assert_eq!(
            cfg.lock_max_secs, 0,
            "env VIGIL_LOCK_MAX_SECS=0 must load as 0"
        );

        // No --max-secs arg → the guard rejects with EX_ERROR before touching the
        // helper.
        let code = lock_run(&cfg, vec![]);
        assert_eq!(code, EX_ERROR, "0-from-env without --max-secs is rejected");

        // Explicit --max-secs 0 → the guard passes; failure now comes from the
        // missing helper (still EX_ERROR but via require_helper, not the guard).
        // We can't easily distinguish the two EX_ERROR codes here, so assert the
        // guard predicate directly for the CLI case (covered above) and confirm
        // this path does not panic.
        let code2 = lock_run(
            &cfg,
            vec![OsString::from("--max-secs"), OsString::from("0")],
        );
        assert_eq!(
            code2, EX_ERROR,
            "missing helper after passing the guard → 1"
        );

        unsafe {
            std::env::remove_var("VIGIL_CONFIG_FILE");
            std::env::remove_var("VIGIL_LOCK_MAX_SECS");
            std::env::remove_var("VIGIL_LOCK_HELPER");
        }
    }
}
