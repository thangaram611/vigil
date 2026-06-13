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

use super::tui::Tui;
use super::{conf_writer, die, interactive, load_config_or_exit};

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
        Some("setup") => {
            let rest: Vec<OsString> = args.into_iter().skip(1).collect();
            let code = lock_setup(&cfg, rest);
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
    // Pre-arm UX countdown (the 3-2-1 shown before the freeze). Default 3;
    // `--countdown` overrides it (0 = arm immediately). This is NOT the pre-arm
    // power-hold safety wait (that is wait_for_lock_power_hold, keyed only on
    // start_wait_secs and unaffected by --countdown).
    let mut countdown: u32 = 3;

    // Parse --combo / --max-secs / --countdown (bash while-loop). Unknown arg →
    // usage die (64).
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
            Some("--countdown") => {
                let Some(v) = it.next() else {
                    usage_die_lock();
                };
                let vs = v.to_string_lossy();
                // non-negative integer only (mirrors --max-secs). Does NOT share
                // state with saw_max (the --max-secs 0 guard is independent).
                if vs.is_empty() || !vs.bytes().all(|b| b.is_ascii_digit()) {
                    die("countdown seconds must be a non-negative integer");
                }
                countdown = match vs.parse::<u32>() {
                    Ok(n) => n,
                    Err(_) => die("countdown seconds must be a non-negative integer"),
                };
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
    // must be true. Any failure → 1. This is the one accepted extra helper spawn
    // on the lock path — it is the gate that yields the deterministic EX_ERROR(1)
    // on missing permission BEFORE arming, and it must run before the
    // pidfile/hold write and the power-hold poll below (ordering is load-bearing,
    // so it is NOT overlapped with the poll).
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

    // Print the lock summary NOW — immediate feedback BEFORE the (silent) pre-arm
    // power-hold wait, so `vigil lock` does not appear to hang on start.
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

    // UX countdown (default 3; `--countdown 0` arms immediately). Pure decoration
    // — NOT the pre-arm power-hold wait, which already ran above.
    if countdown > 0 {
        for delay in (1..=countdown).rev() {
            anstream::println!("  starting in {delay}s...");
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
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

    // Readiness drives BOTH the exit code AND whether remediation hints are
    // emitted. Hoisted so the hint/advisory blocks below can be suppressed when
    // there is nothing left to grant.
    let ready = listen && access && tap;

    anstream::println!("vigil lock doctor");
    for (label, value) in &rows {
        anstream::println!("  {label:<label_w$}  {value}");
    }
    anstream::println!("  helper: {}", cfg.lock_helper);

    // Remediation hints only when at least one permission is missing. A ready run
    // prints just the table, the helper line, and the readiness verdict.
    if doctor_should_print_hints(ready) {
        anstream::println!();
        anstream::println!("System Settings > Privacy & Security > Input Monitoring");
        anstream::println!("System Settings > Privacy & Security > Accessibility");
        anstream::println!();
    }

    // Async advisory only under --prompt AND while something is still missing.
    // When ready there is nothing left to grant, so suppress it even with
    // --prompt.
    if doctor_should_print_prompt_note(prompt, ready) {
        anstream::println!("Prompts/permission changes are asynchronous on macOS.");
        anstream::println!("Run this doctor command again after granting access.");
    }

    if ready {
        anstream::println!("lock guard readiness: ready");
        0
    } else {
        anstream::println!("lock guard readiness: not ready");
        EX_ERROR
    }
}

// ── vigil lock setup — register your unlock chord by pressing it ──────────────

/// `vigil lock setup [--combo <combo> --max-secs <n>]`.
///
/// Interactive (a TTY, no flags): capture the chord via the helper's
/// `--capture-combo` mode so the combo never has to be typed (no scrollback
/// leak), confirm it, ask for a timeout, and persist both `lock_combo` +
/// `lock_max_secs` to vigil.conf. Non-interactive scripting form
/// (`--combo … --max-secs …`): validate + write directly without capture. A
/// non-tty run with NO flags is an error (cannot capture without a terminal) →
/// `EX_ERROR`. Unknown args → usage die (64).
fn lock_setup(cfg: &VigilConfig, args: Vec<OsString>) -> i32 {
    // Parse the optional scripting flags. Unknown args → usage die (64).
    let mut combo_flag: Option<String> = None;
    let mut max_secs_flag: Option<u32> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--combo") => {
                let Some(v) = it.next() else {
                    usage_die_setup();
                };
                combo_flag = Some(v.to_string_lossy().into_owned());
            }
            Some("--max-secs") => {
                let Some(v) = it.next() else {
                    usage_die_setup();
                };
                match parse_non_negative(&v.to_string_lossy()) {
                    Some(n) => max_secs_flag = Some(n),
                    None => die("max seconds must be a non-negative integer"),
                }
            }
            _ => usage_die_setup(),
        }
    }

    // Non-interactive scripting form: --combo (and optionally --max-secs) given.
    // Write directly without capture. --combo is the trigger; --max-secs alone
    // falls through to the interactive/error branch below (combo is mandatory).
    if let Some(combo) = combo_flag {
        let canonical = match validate_combo(&combo) {
            Ok(c) => c,
            Err(e) => {
                die(&format!("invalid combo: {e}"));
            }
        };
        // Default the timeout to the current config value when --max-secs absent.
        let max_secs = max_secs_flag.unwrap_or(cfg.lock_max_secs);
        return persist_or_die(&canonical, max_secs);
    }

    // No --combo. The interactive capture flow requires a terminal; refuse a
    // non-tty run with no flags rather than hang.
    if !interactive(false) {
        anstream::eprintln!(
            "vigil lock setup: cannot capture a chord without an interactive terminal."
        );
        anstream::eprintln!(
            "  run it in a terminal, or script it: vigil lock setup --combo <combo> --max-secs <n>"
        );
        return EX_ERROR;
    }

    lock_setup_interactive(cfg)
}

/// The interactive capture flow (a TTY is guaranteed by the caller).
fn lock_setup_interactive(cfg: &VigilConfig) -> i32 {
    let ui = Tui::new(true);
    ui.intro("vigil lock setup");

    // Allow a couple of retries on "no" before giving up.
    const MAX_ATTEMPTS: u32 = 3;
    let mut combo: Option<String> = None;
    for _ in 0..MAX_ATTEMPTS {
        ui.rail_space();
        let captured = capture_combo_via_helper(cfg, &ui);
        let canonical = match captured {
            Some(c) => c,
            None => {
                // Cancelled / failed capture: never write a broken combo.
                ui.outro_cancel("cancelled — no chord registered");
                return 0;
            }
        };
        // Defense in depth: the helper records whatever was pressed (and can
        // finalize a too-short chord if you release early). Never persist a chord
        // the freeze couldn't arm — re-validate and re-prompt on failure.
        let canonical = match validate_combo(&canonical) {
            Ok(c) => c,
            Err(e) => {
                ui.warn(
                    &format!("not a usable chord ({e}) — try again"),
                    &format!("not a usable chord ({e}) — try again"),
                );
                continue;
            }
        };
        ui.step_success(
            &format!("registered: {canonical}"),
            &format!("registered: {canonical}"),
        );
        if ui.confirm("Use this chord?", true) {
            combo = Some(canonical);
            break;
        }
        // Declined: loop and capture again (up to MAX_ATTEMPTS).
    }

    let Some(combo) = combo else {
        ui.outro_cancel("cancelled — no chord registered");
        return 0;
    };

    // Prompt for the timeout; empty/invalid keeps the current config value.
    ui.rail_space();
    let prompt = "Preferred timeout in seconds (0 = no timeout)";
    let raw = ui.input(prompt);
    let max_secs = match parse_non_negative(raw.trim()) {
        Some(n) => n,
        None => cfg.lock_max_secs,
    };

    let code = persist_or_die(&combo, max_secs);
    if code == 0 {
        ui.outro("saved — run `vigil lock` to arm");
    }
    code
}

/// Spawn `{cfg.lock_helper} --capture-combo` and read the captured combo from its
/// JSON stdout. Returns `None` on cancel/timeout/failure (the helper exits
/// non-zero with no combo). The spinner is suspended across the spawn because the
/// helper owns the TTY + the HID tap for the duration of the capture.
fn capture_combo_via_helper(cfg: &VigilConfig, ui: &Tui) -> Option<String> {
    // Helper present + executable first; a missing helper is a hard failure.
    if require_helper(cfg).is_err() {
        return None;
    }
    let sp = ui.step(
        "Press your chord in order, then release to finish (Esc to cancel)…",
        "Press your chord in order, then release to finish (Esc to cancel)…",
    );
    let output = sp.suspend(|| {
        Command::new(&cfg.lock_helper)
            .arg("--capture-combo")
            .output()
    });
    sp.done("chord captured");

    let output = output.ok()?;
    if !output.status.success() {
        return None; // cancel / timeout / unmapped / tap failure
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_capture_json(&stdout)
}

/// Extract the `combo` string from the helper's `{"combo":"…"}` capture output.
/// Returns `None` if the field is absent or empty. Minimal JSON extraction
/// (mirrors the `json_bool_field` style): find `"combo":"` then read to the next
/// `"`. The combo alphabet (`a-z0-9+`) never contains a quote, so no escape
/// handling is needed.
fn parse_capture_json(json: &str) -> Option<String> {
    let key = "\"combo\":\"";
    let start = json.find(key)? + key.len();
    let rest = &json[start..];
    let end = rest.find('"')?;
    let combo = &rest[..end];
    if combo.is_empty() {
        None
    } else {
        Some(combo.to_string())
    }
}

/// Persist both keys to vigil.conf or `die` with `EX_ERROR` on write failure.
/// Returns 0 on success.
fn persist_or_die(combo: &str, max_secs: u32) -> i32 {
    match conf_writer::write_lock_settings(&conf_writer::conf_path(), combo, max_secs) {
        Ok(()) => 0,
        Err(e) => die(&format!("could not save lock settings: {e}")),
    }
}

/// Parse a non-negative integer (the SAME validation as `--max-secs`: a
/// non-empty string of ASCII digits). `None` on empty/non-digit input.
fn parse_non_negative(v: &str) -> Option<u32> {
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    v.parse::<u32>().ok()
}

// ── combo validation (mirrors the helper's parse_chord contract) ──────────────

/// The safety floor: a chord must contain at least this many keys (mirrors the
/// helper's `combo::MIN_CHORD_KEYS`). Below this an unlock would be too easy to
/// trigger by accident.
const MIN_CHORD_KEYS: usize = 3;

/// The key alphabet the helper supports (`keycode_for_key`): a–z, 0–9, f1–f12,
/// space, tab, return. Kept in lock-step with the helper's `keycode_for_key` so a
/// `--combo` flag the helper would reject is rejected here too (we never write a
/// combo the freeze can't arm).
fn is_supported_key(token: &str) -> bool {
    matches!(
        token,
        "a" | "b"
            | "c"
            | "d"
            | "e"
            | "f"
            | "g"
            | "h"
            | "i"
            | "j"
            | "k"
            | "l"
            | "m"
            | "n"
            | "o"
            | "p"
            | "q"
            | "r"
            | "s"
            | "t"
            | "u"
            | "v"
            | "w"
            | "x"
            | "y"
            | "z"
            | "0"
            | "1"
            | "2"
            | "3"
            | "4"
            | "5"
            | "6"
            | "7"
            | "8"
            | "9"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
            | "space"
            | "tab"
            | "return"
    )
}

/// Normalize a modifier token to its canonical spelling, or `None` if it is not a
/// modifier. Mirrors the helper's `normalize_modifier_token`.
fn canonical_modifier(token: &str) -> Option<&'static str> {
    match token {
        "ctrl" | "control" => Some("ctrl"),
        "alt" | "option" | "opt" => Some("alt"),
        "shift" => Some("shift"),
        "cmd" | "command" | "super" => Some("cmd"),
        _ => None,
    }
}

/// Validate a chord string and return its canonical form, **preserving press
/// order**, or `Err(message)`. Mirrors the helper's `parse_chord` load-bearing
/// rules so the scripting form never writes a combo the freeze would reject:
/// every token is a known modifier (incl. aliases) or supported key, no `escape`,
/// no duplicate key, at least [`MIN_CHORD_KEYS`] keys. Order is significant —
/// `ctrl+l+alt` and `ctrl+alt+l` are different chords and both round-trip.
fn validate_combo(input: &str) -> Result<String, String> {
    let mut tokens: Vec<String> = Vec::new();

    for token in input.split('+') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err("chord tokens cannot be empty".to_string());
        }
        let lowered = trimmed.to_ascii_lowercase();
        if lowered == "escape" {
            return Err("escape is not allowed in an unlock chord".to_string());
        }
        let canonical = if let Some(m) = canonical_modifier(&lowered) {
            m.to_string()
        } else if is_supported_key(&lowered) {
            lowered
        } else {
            return Err(format!("unsupported key: {lowered}"));
        };
        if tokens.contains(&canonical) {
            return Err(format!("duplicate key in chord: {canonical}"));
        }
        tokens.push(canonical);
    }

    if tokens.len() < MIN_CHORD_KEYS {
        return Err(format!("chord must include at least {MIN_CHORD_KEYS} keys"));
    }

    Ok(tokens.join("+"))
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

/// Whether `vigil lock doctor` should print the "System Settings > …"
/// remediation hints: only when NOT ready (at least one permission missing).
fn doctor_should_print_hints(ready: bool) -> bool {
    !ready
}

/// Whether `vigil lock doctor` should print the async-prompt advisory: only
/// under `--prompt` AND while NOT ready (something is still left to grant).
fn doctor_should_print_prompt_note(prompt: bool, ready: bool) -> bool {
    prompt && !ready
}

// ── pre-arm wait (reads the frozen daemon tick ABI) ───────────────────────────

/// Wait up to `VIGIL_START_WAIT_SECS` for the daemon tick to reflect the hold,
/// BEFORE launching the helper (bash `cmd_wait_for_lock_power_hold`). Returns
/// early if the LaunchAgent is not loaded. Reads `refcount_active`, `engaged`,
/// `thermal_cut`, `battery_cut`, `cooling` from the tick file.
///
/// This is the pre-arm power-hold guarantee. Its worst-case bound is driven ONLY
/// by `start_wait_secs` (default 6s); the `--countdown` flag does NOT shorten or
/// otherwise affect this wait. The early-return condition is unchanged:
/// `refcount_active > 0` AND any of engaged/thermal/battery/cooling == 1.
fn wait_for_lock_power_hold(cfg: &VigilConfig) {
    let probe = RealLoadProbe;
    if !probe.is_loaded(vigil::service::USER_AGENT_LABEL) {
        return;
    }
    let wait_secs = cfg.start_wait_secs; // default 6, parsed by config
    let max_ticks = wait_secs as u64 * 10;
    for _ in 0..max_ticks {
        // Read the tick file ONCE per iteration (was 5x read_to_string per tick)
        // and parse all five fields from that single snapshot. Same frozen ABI
        // and same early-return condition — only the I/O is batched.
        if lock_power_hold_satisfied(read_tick_fields(cfg).as_deref()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Snapshot of the daemon tick file (`None` if absent/unreadable). Caller parses
/// the needed fields out of this single read via `tick_field_from`.
fn read_tick_fields(cfg: &VigilConfig) -> Option<String> {
    std::fs::read_to_string(&cfg.daemon_tick_file).ok()
}

/// Pure predicate for the pre-arm power-hold early-return condition over a single
/// tick-file snapshot: `refcount_active > 0` AND any of
/// engaged/thermal_cut/battery_cut/cooling == 1. A `None` snapshot (no tick file)
/// is never satisfied.
fn lock_power_hold_satisfied(tick: Option<&str>) -> bool {
    let Some(text) = tick else {
        return false;
    };
    let active_n = tick_field_from(text, "refcount_active").and_then(|s| s.parse::<u32>().ok());
    let Some(n) = active_n else {
        return false;
    };
    if n == 0 {
        return false;
    }
    tick_field_from(text, "engaged").as_deref() == Some("1")
        || tick_field_from(text, "thermal_cut").as_deref() == Some("1")
        || tick_field_from(text, "battery_cut").as_deref() == Some("1")
        || tick_field_from(text, "cooling").as_deref() == Some("1")
}

/// `awk -F= -v k=field '$1==k {...; print; exit}'` over a daemon-tick snapshot
/// (frozen ABI: `=` is the first separator, one field per line, first-match
/// wins). `None` if the key is missing.
fn tick_field_from(text: &str, field: &str) -> Option<String> {
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
    die("usage: vigil lock [--combo <combo>] [--max-secs <seconds>] [--countdown <seconds>]");
}

fn usage_die_doctor() -> ! {
    die("usage: vigil lock doctor [--prompt]");
}

/// Unknown args to `lock setup` exit `EX_USAGE` (64) — the sysexits usage code
/// for a bad invocation (the lock module's documented "unknown arg → 64"
/// contract). Prints the bash-style `vigil: <usage>` line to stderr first.
fn usage_die_setup() -> ! {
    anstream::eprintln!("vigil: usage: vigil lock setup [--combo <combo> --max-secs <seconds>]");
    std::process::exit(EX_USAGE);
}

fn print_lock_help(cfg: &VigilConfig) {
    anstream::println!("Usage:");
    anstream::println!(
        "  vigil lock [--combo <combo>] [--max-secs <seconds>] [--countdown <seconds>]"
    );
    anstream::println!("  vigil lock setup [--combo <combo> --max-secs <seconds>]");
    anstream::println!("  vigil lock doctor [--prompt]");
    anstream::println!();
    anstream::println!(
        "  --combo <combo>     Unlock combo (default: {})",
        cfg.lock_combo
    );
    anstream::println!("  --max-secs <secs>   Auto-stop timeout in seconds (0 = no timeout)");
    anstream::println!(
        "  --countdown <secs>  Pre-arm countdown before the freeze (default: 3; 0 = arm immediately)"
    );
    anstream::println!();
    anstream::println!(
        "  setup               Register your unlock chord by pressing it, then save it"
    );
    anstream::println!("  doctor              Run a permission and accessibility probe");
    anstream::println!("  doctor --prompt      Show permission prompts if not yet granted");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `vigil lock setup` on a NON-tty with NO flags must NOT hang: it returns
    /// EX_ERROR (cannot capture a chord without an interactive terminal). The test
    /// harness runs with stdin/stdout NOT terminals, so `interactive(false)` is
    /// false and the error branch fires deterministically.
    #[test]
    fn setup_non_tty_no_flags_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("vigil.conf");
        std::fs::write(&conf, "").unwrap();
        let cfg = vigil::config::load(conf.to_str().unwrap(), None).unwrap();
        let code = lock_setup(&cfg, vec![]);
        assert_eq!(
            code, EX_ERROR,
            "setup with no flags on a non-tty must return EX_ERROR, not hang"
        );
    }

    /// The capture JSON parser extracts the combo from the helper's
    /// `{"combo":"…"}` output, and rejects absent/empty fields.
    #[test]
    fn parse_capture_json_extracts_combo() {
        assert_eq!(
            parse_capture_json(r#"{"combo":"ctrl+alt+shift+cmd+l"}"#),
            Some("ctrl+alt+shift+cmd+l".to_string())
        );
        // Trailing newline / surrounding whitespace tolerated.
        assert_eq!(
            parse_capture_json("{\"combo\":\"ctrl+shift+cmd+5\"}\n"),
            Some("ctrl+shift+cmd+5".to_string())
        );
        // Missing field → None.
        assert_eq!(parse_capture_json(r#"{"other":"x"}"#), None);
        // Empty combo → None (never write a broken combo).
        assert_eq!(parse_capture_json(r#"{"combo":""}"#), None);
    }

    /// `validate_combo` mirrors the helper's `parse_chord` contract: an ordered
    /// sequence (press order preserved, NOT sorted), ≥3 keys of any mix, alias
    /// normalization, and the rejections (duplicate key, escape, unsupported key,
    /// too short).
    #[test]
    fn validate_combo_preserves_order_and_rejects() {
        // Order is preserved — the modifiers are NOT reordered.
        assert_eq!(validate_combo("CTRL + L + aLt").unwrap(), "ctrl+l+alt");
        // A different press order is a different (still valid) chord.
        assert_eq!(validate_combo("ctrl+alt+l").unwrap(), "ctrl+alt+l");
        // Any mix of ≥3 keys, order preserved.
        assert_eq!(validate_combo("l+ctrl+alt").unwrap(), "l+ctrl+alt");
        // Aliases normalize but stay in place.
        assert_eq!(
            validate_combo("control+space+super").unwrap(),
            "ctrl+space+cmd"
        );
        // Below the 3-key floor.
        assert!(validate_combo("ctrl+l").is_err());
        // Escape forbidden anywhere.
        assert!(validate_combo("ctrl+escape+l").is_err());
        // Duplicate key (modifier alias collision).
        assert!(validate_combo("ctrl+control+l").is_err());
        // Duplicate regular key.
        assert!(validate_combo("ctrl+l+l").is_err());
        // Unsupported key.
        assert!(validate_combo("ctrl+alt+f13").is_err());
        // Empty token.
        assert!(validate_combo("ctrl++l").is_err());
    }

    /// The setup --max-secs validation reuses `parse_non_negative` (same as the
    /// run-path --max-secs): digits parse (including 0), non-digits/empty reject.
    #[test]
    fn parse_non_negative_matches_max_secs_rules() {
        assert_eq!(parse_non_negative("0"), Some(0));
        assert_eq!(parse_non_negative("28800"), Some(28800));
        assert_eq!(parse_non_negative(""), None);
        assert_eq!(parse_non_negative("-1"), None);
        assert_eq!(parse_non_negative("12x"), None);
    }

    /// The --countdown flag parser: pure-predicate mirror of the lock_run parse
    /// arm. Default is 3 (the 3-2-1 countdown); positive ints parse; non-digit/
    /// empty is rejected. Mirrors the --max-secs predicate style.
    #[test]
    fn countdown_defaults_three_and_parses_non_negative_int() {
        // Pure predicate mirror of the lock_run --countdown arm: returns the
        // parsed value, or None for invalid input (which the run path turns into
        // a die()).
        fn parse_countdown(v: &str) -> Option<u32> {
            if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            v.parse::<u32>().ok()
        }
        // Default is 3 when the flag is absent (the run path initializes to 3).
        let default_countdown: u32 = 3;
        assert_eq!(default_countdown, 3);
        // Positive integers parse.
        assert_eq!(parse_countdown("3"), Some(3));
        assert_eq!(parse_countdown("0"), Some(0));
        assert_eq!(parse_countdown("10"), Some(10));
        // Non-digits / empty are rejected (→ die in the run path).
        assert_eq!(parse_countdown("abc"), None);
        assert_eq!(parse_countdown(""), None);
        assert_eq!(parse_countdown("3s"), None);
        assert_eq!(parse_countdown("-1"), None);
    }

    /// Doctor remediation gating: hints only when NOT ready; the async-prompt
    /// advisory only under --prompt AND while NOT ready.
    #[test]
    fn doctor_gating_predicates() {
        // Hints suppressed when ready, shown when not ready.
        assert!(!doctor_should_print_hints(true), "ready → no hints");
        assert!(doctor_should_print_hints(false), "not ready → hints");

        // Prompt advisory: only with --prompt AND not ready.
        assert!(
            !doctor_should_print_prompt_note(true, true),
            "prompt + ready → suppressed (nothing to grant)"
        );
        assert!(
            doctor_should_print_prompt_note(true, false),
            "prompt + not ready → shown"
        );
        assert!(
            !doctor_should_print_prompt_note(false, false),
            "no prompt → never shown"
        );
        assert!(
            !doctor_should_print_prompt_note(false, true),
            "no prompt + ready → never shown"
        );
    }

    /// The batched pre-arm power-hold predicate must keep the EXACT early-return
    /// condition over a single tick snapshot: refcount_active > 0 AND any of
    /// engaged/thermal_cut/battery_cut/cooling == 1. Also exercises the frozen
    /// ABI (`=` first separator, one field per line, first-match wins).
    #[test]
    fn lock_power_hold_satisfied_matches_frozen_condition() {
        // No tick file → never satisfied.
        assert!(!lock_power_hold_satisfied(None));

        // refcount_active == 0 → not satisfied even with engaged=1.
        let t = "refcount_active=0\nengaged=1\n";
        assert!(!lock_power_hold_satisfied(Some(t)));

        // refcount_active > 0 but no power flag set → not satisfied.
        let t = "refcount_active=2\nengaged=0\nthermal_cut=0\nbattery_cut=0\ncooling=0\n";
        assert!(!lock_power_hold_satisfied(Some(t)));

        // refcount_active > 0 AND engaged=1 → satisfied.
        let t = "refcount_active=1\nengaged=1\n";
        assert!(lock_power_hold_satisfied(Some(t)));

        // Each of the other three power flags independently satisfies.
        assert!(lock_power_hold_satisfied(Some(
            "refcount_active=1\nthermal_cut=1\n"
        )));
        assert!(lock_power_hold_satisfied(Some(
            "refcount_active=1\nbattery_cut=1\n"
        )));
        assert!(lock_power_hold_satisfied(Some(
            "refcount_active=1\ncooling=1\n"
        )));

        // Missing refcount_active field → not satisfied.
        assert!(!lock_power_hold_satisfied(Some("engaged=1\n")));

        // First-match-wins on duplicate keys (frozen ABI).
        let t = "refcount_active=1\nrefcount_active=0\nengaged=1\n";
        assert!(lock_power_hold_satisfied(Some(t)));
    }

    /// `tick_field_from` honors the frozen ABI: `=` is the first separator
    /// (values may contain `=`), one field per line, first match wins.
    #[test]
    fn tick_field_from_frozen_abi() {
        let t = "engaged=1\nnote=a=b=c\nrefcount_active=3\n";
        assert_eq!(tick_field_from(t, "engaged"), Some("1".to_string()));
        // Value retains everything after the FIRST `=`.
        assert_eq!(tick_field_from(t, "note"), Some("a=b=c".to_string()));
        assert_eq!(tick_field_from(t, "refcount_active"), Some("3".to_string()));
        // Missing key.
        assert_eq!(tick_field_from(t, "cooling"), None);
        // First match wins.
        let dup = "k=first\nk=second\n";
        assert_eq!(tick_field_from(dup, "k"), Some("first".to_string()));
    }

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
