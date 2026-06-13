//! Thin shim: re-exec the existing bash `bin/vigil` for not-yet-ported
//! subcommands. Uses `execv` so the bash's exit status / fatal signal
//! propagates verbatim — the Rust process is replaced, not waited on.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use crate::exit::EX_ERROR;

/// Resolve the bash `vigil` script path:
///   1. `$VIGIL_BASH_BIN` (explicit override — used by tests)
///   2. `$VIGIL_INSTALL_DIR/bin/vigil`
///   3. repo-relative fallback: <dir-of-current-exe>/../../../bin/vigil
///      (target/debug/vigil -> repo root /bin/vigil), then CWD `bin/vigil`.
fn resolve_bash_bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("VIGIL_BASH_BIN") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Some(dir) = std::env::var_os("VIGIL_INSTALL_DIR") {
        let mut p = PathBuf::from(dir);
        p.push("bin");
        p.push("vigil");
        if p.exists() {
            return Some(p);
        }
    }
    // Repo-relative: target/{debug,release}/vigil -> repo_root/bin/vigil.
    // ancestors() of .../target/debug/vigil yields:
    //   [0] .../target/debug/vigil
    //   [1] .../target/debug
    //   [2] .../target
    //   [3] .../<repo_root>
    if let Ok(exe) = std::env::current_exe()
        && let Some(repo_root) = exe.ancestors().nth(3)
    {
        let cand = repo_root.join("bin").join("vigil");
        if cand.exists() {
            return Some(cand);
        }
    }
    // Last resort: CWD-relative.
    let cwd_rel = PathBuf::from("bin/vigil");
    if cwd_rel.exists() {
        return Some(cwd_rel);
    }
    None
}

/// Re-exec the bash `vigil` with `subcommand` + `rest`. On success this never
/// returns. On failure (binary not found, exec error) it prints to stderr and
/// exits `EX_ERROR`.
pub fn exec_bash(subcommand: &str, rest: &[OsString]) -> ! {
    let Some(bin) = resolve_bash_bin() else {
        anstream::eprintln!("vigil: cannot locate bash vigil (set VIGIL_BASH_BIN)");
        std::process::exit(EX_ERROR);
    };

    let mut cmd = Command::new(&bin);
    cmd.arg(subcommand);
    cmd.args(rest);

    // `exec` only returns on error.
    let err = cmd.exec();
    anstream::eprintln!("vigil: failed to exec {}: {err}", bin.display());
    std::process::exit(EX_ERROR);
}
