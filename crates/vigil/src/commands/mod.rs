//! `src/commands/` — the native command ports (Phase 5.7 §4).
//!
//! This module owns the security-sensitive lifecycle + lock commands that 5.7
//! cuts over from bash to Rust: `setup`, `uninstall`, `reload`, `start`, `stop`,
//! and `lock`. Each `run(args)` returns `!` — it either prints + `std::process::
//! exit(code)` (mirroring the bash `die`/exit-code discipline) or, for the lock
//! path, exec-spawns the helper and propagates its status.
//!
//! Exit codes follow the existing `exit.rs` contract: `EX_USAGE = 64` for a
//! usage/argument violation, `EX_ERROR = 1` for an operational failure, `0` on
//! success.
//!
//! The single privileged choke point is `crate::exit::admin_allowed()` (honors
//! `VIGIL_TEST_NO_ADMIN=1`): every sudo / launchctl / root-file touch in setup,
//! uninstall, and reload routes through `require_admin_allowed()` BEFORE any
//! privileged action.

pub mod doctor;
pub mod lock;
pub mod log;
pub mod reload;
pub mod run;
pub mod setup;
pub mod start;
pub mod status;
pub mod stop;
pub mod uninstall;

/// Re-exported so the read-only commands (status/doctor/run/log) reach the
/// operational-failure exit code via `super::EX_ERROR` (they were authored as
/// library modules; the merged tree places `commands` in the binary crate).
pub use crate::exit::EX_ERROR;

use vigil::config::{self, VigilConfig};

/// Resolve the config file path the same way the binary's read-only commands do:
/// `$VIGIL_CONFIG_FILE` or `$HOME/.config/vigil/vigil.conf`.
fn config_file_path() -> String {
    std::env::var("VIGIL_CONFIG_FILE").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        format!("{home}/.config/vigil/vigil.conf")
    })
}

/// Load the fully-resolved config, exiting `EX_USAGE` on a malformed conf (same
/// behavior as `main::load_config_or_exit`). No side effects.
fn load_config_or_exit() -> VigilConfig {
    match config::load(&config_file_path(), None) {
        Ok(c) => c,
        Err(e) => {
            anstream::eprintln!("{e}");
            std::process::exit(crate::exit::EX_USAGE);
        }
    }
}

/// Print a `die`-style message to stderr and exit `EX_ERROR` (mirrors bash
/// `die`, which prints `vigil: <msg>` and exits 1). The bash `die` prefixes
/// `vigil: `; callers pass the bare message.
fn die(msg: &str) -> ! {
    anstream::eprintln!("vigil: {msg}");
    std::process::exit(crate::exit::EX_ERROR);
}

/// Enforce the admin guard or terminate with the exact bash message
/// (`die "admin operation blocked by VIGIL_TEST_NO_ADMIN"`). This is the single
/// choke point every privileged path calls BEFORE touching sudo/launchctl/root
/// files.
fn require_admin_allowed_or_die() {
    if let Err(_msg) = crate::exit::admin_allowed() {
        die("admin operation blocked by VIGIL_TEST_NO_ADMIN");
    }
}

/// The `assert_vigil_tree_path` guard (bash `cmd_assert_vigil_tree_path`,
/// `bin/vigil:82-90`) plus the Q4 hardening delta: reject paths under
/// `~/Documents` (TCC). Returns `Err(message)` on rejection.
///
/// Rules (in bash order):
/// 1. absolute (starts with `/`)
/// 2. no newline / carriage return
/// 3. not `/`
/// 4. not `$HOME`
/// 5. basename == `vigil`
/// 6. NOT under `$HOME/Documents` — NEW hardening (Q4), flagged as a delta, not
///    parity. The copy-out (§4.7) enforces this structurally too; this is a
///    belt-and-suspenders refusal before any privileged write.
fn assert_vigil_tree_path(label: &str, path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err(format!(
            "refusing unsafe {label} path (not absolute): {path}"
        ));
    }
    if path.contains('\n') || path.contains('\r') {
        return Err(format!("refusing unsafe {label} path containing a newline"));
    }
    if path == "/" {
        return Err(format!("refusing unsafe {label} path: /"));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && path == home {
        return Err(format!(
            "refusing unsafe {label} path equal to HOME: {path}"
        ));
    }
    let base = path.rsplit('/').next().unwrap_or("");
    if base != "vigil" {
        return Err(format!(
            "refusing unsafe {label} path (must end in /vigil): {path}"
        ));
    }
    // Q4 hardening delta: not under ~/Documents (TCC). NOT in the bash 5-rule set.
    if !home.is_empty() {
        let documents = format!("{home}/Documents");
        if path == documents || path.starts_with(&format!("{documents}/")) {
            return Err(format!(
                "refusing unsafe {label} path under ~/Documents (TCC): {path}"
            ));
        }
    }
    Ok(())
}

/// The standard helper-plist path (bash `VIGIL_HELPER_PLIST`). Hardcoded — the
/// label is never overridable. Re-exported from the config allowlist constant.
fn helper_plist_path() -> &'static str {
    config::HELPER_PLIST_FILE
}

/// The legacy sudoers file path (bash `VIGIL_LEGACY_SUDOERS_FILE`). Hardcoded.
const LEGACY_SUDOERS_FILE: &str = config::LEGACY_SUDOERS_FILE;

/// The user LaunchAgent plist path (`$HOME/Library/LaunchAgents/
/// com.thangaram.vigil.plist`). Hardcoded label.
fn user_plist_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", vigil::service::USER_AGENT_LABEL))
}

/// Current epoch seconds (the command layer's clock; bash `vigil_now_unix`).
fn now_unix() -> i64 {
    chrono::Local::now().timestamp()
}

// ── cmd_sync_install — the TCC copy-out (§4.7, bash cmd_sync_install) ──────────

/// Resolve the source repo root: `$VIGIL_REPO_ROOT` (tests / explicit), else
/// `target/{debug,release}/vigil` → repo root (3 ancestors up, like the shim).
fn resolve_repo_root() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("VIGIL_REPO_ROOT") {
        let p = std::path::PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(repo_root) = exe.ancestors().nth(3)
    {
        return Some(repo_root.to_path_buf());
    }
    None
}

/// `cargo build --release` the two shipped binaries (`vigil`, `vigil-lock-helper`)
/// and copy them into `{install}/bin` BEFORE the plist points at them — the TCC
/// copy-out-of-Documents (§4.7, bash `cmd_sync_install`). Source lives under the
/// repo (possibly `~/Documents`); dest is `~/Library/Application Support/vigil`,
/// OUTSIDE Documents, so launchd execs it without TCC consent.
///
/// The Rust port copies the single `vigil` binary (which IS the daemon via the
/// hidden `vigil daemon` subcommand) + `vigil-lock-helper`; there is no
/// `bin/vigil-daemon` or `lib/*.sh` to copy.
///
/// Non-privileged (no sudo) — but it builds + writes the install dir, so it runs
/// only AFTER the admin guard in setup/reload. Returns `Err(message)` on failure.
fn cmd_sync_install(cfg: &VigilConfig) -> Result<(), String> {
    let install_bin = format!("{}/bin", cfg.install_dir);
    let install_lib = format!("{}/lib", cfg.install_dir);
    std::fs::create_dir_all(&install_bin)
        .map_err(|e| format!("could not create {install_bin}: {e}"))?;
    // `lib/` retained for layout parity (the bash install dir has bin + lib);
    // the Rust port writes no files into it.
    let _ = std::fs::create_dir_all(&install_lib);

    let repo_root =
        resolve_repo_root().ok_or_else(|| "could not resolve repo root for sync".to_string())?;
    let manifest = repo_root.join("Cargo.toml");

    // Build BOTH shipped binaries in one release build of the workspace.
    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--quiet")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--bin")
        .arg("vigil")
        .status()
        .map_err(|e| format!("could not run cargo build: {e}"))?;
    if !status.success() {
        return Err("failed to build vigil (see above)".to_string());
    }

    let target_dir = cargo_target_dir(&manifest)
        .unwrap_or_else(|| repo_root.join("target").to_string_lossy().into_owned());

    // Install the `vigil` binary.
    let vigil_src = format!("{target_dir}/release/vigil");
    let vigil_dst = format!("{install_bin}/vigil");
    install_binary(&vigil_src, &vigil_dst, 0o755)
        .map_err(|e| format!("failed to install vigil: {e}"))?;
    anstream::println!("  daemon: installed to {vigil_dst}");

    // Darwin-only: build + install the lock helper.
    if cfg!(target_os = "macos") {
        let helper_manifest = repo_root.join("native/vigil-lock-helper/Cargo.toml");
        let status = std::process::Command::new("cargo")
            .arg("build")
            .arg("--quiet")
            .arg("--release")
            .arg("--manifest-path")
            .arg(&helper_manifest)
            .status()
            .map_err(|e| format!("could not run cargo build (lock helper): {e}"))?;
        if !status.success() {
            return Err("failed to build vigil-lock-helper (see above)".to_string());
        }
        let helper_target =
            cargo_target_dir(&helper_manifest).unwrap_or_else(|| target_dir.clone());
        let helper_src = format!("{helper_target}/release/vigil-lock-helper");
        let helper_dst = format!("{install_bin}/vigil-lock-helper");
        install_binary(&helper_src, &helper_dst, 0o755)
            .map_err(|e| format!("failed to install vigil-lock-helper: {e}"))?;
        anstream::println!("  lock helper: installed to {helper_dst}");
    }

    Ok(())
}

/// Resolve a manifest's cargo `target_directory` via `cargo metadata` (bash
/// `cmd_cargo_target_dir`). Returns `None` on any failure (caller falls back to
/// `<repo>/target`).
fn cargo_target_dir(manifest: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("cargo")
        .arg("metadata")
        .arg("--format-version")
        .arg("1")
        .arg("--no-deps")
        .arg("--manifest-path")
        .arg(manifest)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    // Minimal extraction without a JSON dep: find "target_directory":"...".
    let needle = "\"target_directory\":\"";
    let start = text.find(needle)? + needle.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Copy `src` → `dst` with `mode` (atomic-ish: write to a temp then rename), like
/// `install -m`.
fn install_binary(src: &str, dst: &str, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if !std::path::Path::new(src).is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("built binary not found at {src}"),
        ));
    }
    let tmp = format!("{dst}.tmp.{}", std::process::id());
    std::fs::copy(src, &tmp)?;
    let mut perms = std::fs::metadata(&tmp)?.permissions();
    perms.set_mode(mode);
    std::fs::set_permissions(&tmp, perms)?;
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vigil_tree_path_rejects_non_absolute() {
        assert!(assert_vigil_tree_path("install dir", "relative/vigil").is_err());
    }

    #[test]
    fn vigil_tree_path_rejects_root() {
        assert!(assert_vigil_tree_path("install dir", "/").is_err());
    }

    #[test]
    fn vigil_tree_path_rejects_non_vigil_basename() {
        assert!(assert_vigil_tree_path("install dir", "/opt/notvigil").is_err());
    }

    #[test]
    fn vigil_tree_path_rejects_newline() {
        assert!(assert_vigil_tree_path("install dir", "/opt/vi\ngil/vigil").is_err());
    }

    #[test]
    fn vigil_tree_path_accepts_standard() {
        assert!(
            assert_vigil_tree_path("install dir", "/Users/x/Library/Application Support/vigil")
                .is_ok()
        );
    }

    #[test]
    fn helper_plist_is_hardcoded_label() {
        assert_eq!(
            helper_plist_path(),
            "/Library/LaunchDaemons/com.thangaram.vigil.helper.plist"
        );
    }

    #[test]
    fn vigil_tree_path_rejects_under_documents() {
        // SAFETY: single-threaded unit test; HOME set/restored locally.
        let saved = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", "/Users/x") };
        let r = assert_vigil_tree_path("install dir", "/Users/x/Documents/vigil");
        match saved {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        assert!(r.is_err(), "must reject ~/Documents path (Q4 hardening)");
    }

    #[test]
    fn legacy_sudoers_is_hardcoded() {
        assert_eq!(LEGACY_SUDOERS_FILE, "/etc/sudoers.d/vigil");
    }
}
