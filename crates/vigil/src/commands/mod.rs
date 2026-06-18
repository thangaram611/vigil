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

pub mod conf_writer;
pub mod doctor;
pub mod lock;
pub mod log;
pub mod reload;
pub mod run;
pub mod setup;
pub mod start;
pub mod status;
pub mod stop;
pub mod tui;
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

// ── interactive TUI seam (setup / uninstall) ──────────────────────────────────
//
// The crux of the elegant-TUI slice: a SINGLE gate decides whether `tui::Tui`
// renders the clack rail (dialoguer confirm + static `◇`/`✓` steps + styled
// glyphs), or falls back to the EXACT plain `anstream::println!` path that
// scripts/CI/golden-oracle have always seen. When the gate is false the `Tui`
// touches NONE of the new crates — so NO_COLOR / --color / the byte-frozen
// dry-run output are unaffected, and a piped/CI run can never hang on a prompt.
//
// `tui::Tui::new(interactive(yes))` is constructed once per command and threaded
// through every step (see `commands::tui`).

use std::io::IsTerminal;

/// True iff we should render the interactive TUI (confirm prompts + spinners).
///
/// FALSE when `--yes`/`--non-interactive` was passed (`yes == true`) OR either of
/// stdin/stdout is not a terminal. In that case callers proceed with defaults and
/// emit today's plain lines verbatim. The `yes` short-circuit is first so a
/// non-interactive flag always wins regardless of TTY state.
pub(super) fn interactive(yes: bool) -> bool {
    !yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
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

// ── PATH symlink for the `vigil` CLI ──────────────────────────────────────────
//
// `setup`/`reload` rebuild `vigil` from the repo, so the `vigil` on the user's
// PATH must be able to find the repo. It therefore points at the DEV build
// (`<repo>/target/release/vigil`, inside the checkout) — exactly where the old
// bash symlink pointed — NOT at the installed snapshot in `{install_dir}/bin`
// (which has no repo above it and could never run `reload`). The installed copy
// exists only for the LaunchAgent daemon (a TCC-stable path). `setup`/`reload`
// (re)create this PATH symlink; `uninstall` leaves it, because its target (your
// repo checkout) survives an uninstall and you may want to reinstall.
//
// A real (non-symlink) file at the link path is NEVER clobbered.

/// The directory the `vigil` CLI is symlinked into so it lands on `PATH`.
/// Override with `VIGIL_BIN_LINK_DIR`; default `~/.local/bin`.
fn bin_link_dir() -> String {
    match std::env::var("VIGIL_BIN_LINK_DIR") {
        Ok(d) if !d.is_empty() => d,
        _ => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            format!("{home}/.local/bin")
        }
    }
}

/// The PATH symlink for the `vigil` CLI: `{bin_link_dir}/vigil`.
fn bin_link_path() -> String {
    format!("{}/vigil", bin_link_dir())
}

/// True if `dir` is a component of the current `PATH`.
fn dir_on_path(dir: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|p| p == dir)
}

/// (Re)create the managed symlink `link` → `target` under `dir`. A stale symlink
/// is refreshed; an existing NON-symlink file is left untouched (so we never
/// clobber an unrelated binary). Returns `(linked, note)` — `linked` is true iff
/// the symlink now points at `target`.
fn refresh_managed_symlink(dir: &str, link: &str, target: &str) -> (bool, String) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        return (false, format!("skipped (could not create {dir}: {e})"));
    }
    match std::fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let _ = std::fs::remove_file(link);
        }
        Ok(_) => {
            return (
                false,
                format!("{link} already exists and is not a symlink — left as-is"),
            );
        }
        Err(_) => {} // absent
    }
    match std::os::unix::fs::symlink(target, link) {
        Ok(()) => (true, format!("{link} -> {target}")),
        Err(e) => (false, format!("skipped ({e})")),
    }
}

/// Symlink the freshly-built dev `vigil` (`target`) onto `PATH` (best-effort; a
/// failure here never fails the install). Prints a hint if `bin_link_dir` is not
/// on `PATH`.
fn link_vigil_onto_path(target: &str, ui: tui::Tui) {
    let dir = bin_link_dir();
    let (linked, note) = refresh_managed_symlink(&dir, &bin_link_path(), target);
    ui.detail(
        &format!("PATH symlink: {note}"),
        &format!("     PATH symlink: {note}"),
    );
    if linked && !dir_on_path(&dir) {
        let hint = format!(
            "{dir} is not on your PATH — add it: echo 'export PATH=\"{dir}:$PATH\"' >> ~/.zshrc"
        );
        ui.detail(&format!("note: {hint}"), &format!("     note: {hint}"));
    }
}

// ── cmd_sync_install — the TCC copy-out (§4.7, bash cmd_sync_install) ──────────

/// Resolve the source repo root: `$VIGIL_REPO_ROOT` (tests / explicit), else
/// derive it from the executable. The dev build lives at
/// `<repo>/target/{debug,release}/vigil`; we **canonicalize** the exe first (so a
/// PATH symlink like `~/.local/bin/vigil` is followed to the real build path —
/// `current_exe()` does NOT resolve symlinks on macOS) and walk up to the first
/// ancestor that looks like the vigil workspace root (`Cargo.toml` +
/// `native/vigil-lock-helper/`). Returns `None` from an installed copy with no
/// repo above it — callers then require `$VIGIL_REPO_ROOT`.
fn resolve_repo_root() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("VIGIL_REPO_ROOT") {
        let p = std::path::PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.ancestors()
        .skip(1)
        .find(|a| a.join("Cargo.toml").is_file() && a.join("native/vigil-lock-helper").is_dir())
        .map(|a| a.to_path_buf())
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
/// only AFTER the admin guard in setup/reload. On success returns the path to the
/// freshly-built dev `vigil` binary (for the PATH symlink); `Err(message)` on
/// failure.
fn cmd_sync_install(cfg: &VigilConfig, ui: tui::Tui) -> Result<String, String> {
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

    // Build BOTH shipped binaries of the vigil crate (`vigil` + the privileged
    // `vigil-root-helper`) in one release build of the workspace. Staging the
    // helper here too is what makes a RE-`setup` deploy the CURRENT helper:
    // `cmd_install_root_helper` reuses `{install}/bin/vigil-root-helper` when it
    // exists, so without this it would re-install a stale staged copy and any
    // helper fix would silently fail to ship.
    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--quiet")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--bin")
        .arg("vigil")
        .arg("--bin")
        .arg("vigil-root-helper")
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
    ui.detail(
        &format!("daemon: installed to {vigil_dst}"),
        &format!("  daemon: installed to {vigil_dst}"),
    );

    // Stage the freshly-built privileged helper alongside `vigil` so a re-`setup`
    // always ships the current binary (cmd_install_root_helper sudo-installs this
    // staged copy into the root tree). Atomic temp+rename, so it cleanly replaces
    // an older staged helper.
    let helper_src = format!("{target_dir}/release/vigil-root-helper");
    let helper_dst = format!("{install_bin}/vigil-root-helper");
    install_binary(&helper_src, &helper_dst, 0o755)
        .map_err(|e| format!("failed to install vigil-root-helper: {e}"))?;
    ui.detail(
        &format!("root helper: staged to {helper_dst}"),
        &format!("  root helper: staged to {helper_dst}"),
    );

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
        ui.detail(
            &format!("lock helper: installed to {helper_dst}"),
            &format!("  lock helper: installed to {helper_dst}"),
        );
    }

    Ok(vigil_src)
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

    /// The non-interactive guarantee: `--yes`/`--non-interactive` (yes == true)
    /// short-circuits BEFORE any TTY check, so the interactive TUI is NEVER
    /// rendered regardless of whether stdin/stdout happen to be terminals. This
    /// locks the "CI/scripts must never hang and stay byte-identical" contract.
    #[test]
    fn interactive_is_false_when_yes() {
        assert!(
            !interactive(true),
            "--yes must always force the non-interactive (plain) path"
        );
        // The `Tui` built from this gate must select the inert (plain) path: a
        // non-interactive spinner is inert and confirm returns the default
        // without prompting (caller keeps its plain println lines, never hangs).
        let ui = super::tui::Tui::new(interactive(true));
        assert!(
            !ui.is_interactive(),
            "--yes must always force the non-interactive Tui"
        );
        assert!(
            ui.confirm("anything", true),
            "--yes confirm must return the default without prompting"
        );
    }

    /// Table-driven guard coverage for `assert_vigil_tree_path` — one row per
    /// bash rule plus the Q4 ~/Documents hardening delta. HOME is pinned to
    /// `/Users/x` (save/restore) so the HOME-equality (rule #4) and ~/Documents
    /// (rule #6) branches are deterministic.
    #[test]
    fn vigil_tree_path_guard_rules() {
        // SAFETY: single-threaded unit test; HOME set/restored locally.
        let saved = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", "/Users/x") };

        // (label, path, expect_ok)
        let cases: &[(&str, &str, bool)] = &[
            // rule 1: must be absolute.
            ("non-absolute", "relative/vigil", false),
            // rule 2: no newline / carriage return.
            ("newline", "/opt/vi\ngil/vigil", false),
            // rule 3: not `/`.
            ("root", "/", false),
            // rule 4: not `$HOME` (HOME pinned to /Users/x above).
            ("equals HOME", "/Users/x", false),
            // rule 5: basename must be `vigil`.
            ("non-vigil basename", "/opt/notvigil", false),
            // rule 6 (Q4 delta): not under ~/Documents (TCC).
            ("under ~/Documents", "/Users/x/Documents/vigil", false),
            // accepts a standard install path.
            (
                "standard install dir",
                "/Users/x/Library/Application Support/vigil",
                true,
            ),
        ];
        for (label, path, expect_ok) in cases {
            let r = assert_vigil_tree_path("install dir", path);
            assert_eq!(
                r.is_ok(),
                *expect_ok,
                "{label}: {path} expected ok={expect_ok}, got {r:?}"
            );
        }

        match saved {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn helper_plist_is_hardcoded_label() {
        assert_eq!(
            helper_plist_path(),
            "/Library/LaunchDaemons/com.thangaram.vigil.helper.plist"
        );
    }

    #[test]
    fn legacy_sudoers_is_hardcoded() {
        assert_eq!(LEGACY_SUDOERS_FILE, "/etc/sudoers.d/vigil");
    }

    #[test]
    fn managed_symlink_creates_and_refreshes() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let link = bin.join("vigil");
        let target = dir.path().join("repo/target/release/vigil");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"#!/bin/true\n").unwrap();
        let (dir_s, link_s, target_s) = (
            bin.to_str().unwrap(),
            link.to_str().unwrap(),
            target.to_str().unwrap(),
        );

        // Create.
        let (linked, _) = refresh_managed_symlink(dir_s, link_s, target_s);
        assert!(linked);
        assert_eq!(std::fs::read_link(&link).unwrap(), target);

        // A stale symlink (e.g. pointing at an old build) is refreshed in place.
        let stale = dir.path().join("old/vigil");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&stale, &link).unwrap();
        let (relinked, _) = refresh_managed_symlink(dir_s, link_s, target_s);
        assert!(relinked);
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
    }

    #[test]
    fn managed_symlink_never_clobbers_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let link = bin.join("vigil");
        std::fs::write(&link, b"a real user binary").unwrap();

        let (linked, note) =
            refresh_managed_symlink(bin.to_str().unwrap(), link.to_str().unwrap(), "/x/vigil");
        assert!(!linked, "must not replace a non-symlink file");
        assert!(note.contains("not a symlink"));
        // The real file is intact.
        assert_eq!(std::fs::read(&link).unwrap(), b"a real user binary");
    }
}
