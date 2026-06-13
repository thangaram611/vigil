//! `vigil run <cmd> [args...]` — the NON-exec sleep-prevention wrapper
//! (Phase 5.7 §4.6, Contract 3 §1).
//!
//! The wrapper writes a `wrapper-{pid}.pid` refcount file (which the daemon's
//! `refcount::count` treats as an unconditional +1), then runs the child as a
//! foreground SUBPROCESS — never `execv`. Exec would replace this process and the
//! pidfile would never be cleaned; the whole point of the wrapper is that the
//! cleanup runs after the child exits. An RAII guard + INT/TERM/HUP handler delete
//! the pidfile on every exit path, with the path captured BY VALUE so cleanup
//! survives the function returning.
//!
//! Exit-status propagation is shell-faithful: a normal child exit propagates its
//! code; a signal-terminated child propagates `128 + signal`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use super::{load_config_or_exit, now_unix};

/// Print the usage line to stderr and exit 1 (bash `die`).
fn usage_die() -> ! {
    anstream::eprintln!("usage: vigil run <cmd> [args...]");
    std::process::exit(super::EX_ERROR);
}

/// RAII cleanup for the wrapper pidfile. The path is owned (captured by value) so
/// `Drop` fires correctly however the function unwinds/returns. `remove_file` is
/// idempotent (a missing file is fine), so a signal-handler removal followed by a
/// `Drop` removal is harmless.
struct PidfileGuard {
    path: PathBuf,
}

impl PidfileGuard {
    fn remove(&self) {
        // Best-effort, idempotent (mirrors bash `rm -f`).
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for PidfileGuard {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Entry point for the `Run` dispatch arm. Returns `!` (always exits).
pub fn run(args: Vec<OsString>) -> ! {
    // 1. Zero args → die (bash `[[ $# -eq 0 ]] && die`).
    if args.is_empty() {
        usage_die();
    }

    let cfg = load_config_or_exit();
    // 2. ensure_state_dir (bash `vigil_ensure_dirs`): creates state/active/log.
    if let Err(e) = cfg.ensure_state_dir() {
        anstream::eprintln!("vigil: run: could not create state directories: {e}");
        std::process::exit(super::EX_ERROR);
    }

    let pid = std::process::id();
    let now = now_unix();

    // 3. cmd_str = args joined by a single space (bash `"$*"`). args[0] is the
    //    program, the rest are its arguments; the summary is the whole line.
    let cmd_str = args
        .iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");

    // 4. Write the wrapper pidfile (byte-identical to bash
    //    `vigil_refcount_touch_wrapper`).
    let pidfile = PathBuf::from(&cfg.active_dir).join(format!("wrapper-{pid}.pid"));
    let body = vigil::refcount::wrapper_pidfile_body(pid, &cmd_str, now);
    if let Err(e) = std::fs::write(&pidfile, body) {
        anstream::eprintln!("vigil: run: could not write wrapper pidfile: {e}");
        std::process::exit(super::EX_ERROR);
    }

    // 5. Install the RAII guard (path BY VALUE) + the INT/TERM/HUP handler. The
    //    signal handler only flips an AtomicBool and removes the pidfile through a
    //    captured clone (remove_file is async-signal-imperfect but practically
    //    safe here, and the Drop guard is the authoritative cleanup on the normal
    //    path). bash traps EXIT INT TERM HUP — we mirror INT/TERM/HUP as signals
    //    and EXIT via Drop.
    let guard = PidfileGuard {
        path: pidfile.clone(),
    };
    install_signal_cleanup(&pidfile);

    // 6. Spawn the child as a foreground SUBPROCESS — NOT exec. The first arg is
    //    the program; the rest are its argv.
    let program = &args[0];
    let child_args = &args[1..];
    let mut command = Command::new(program);
    command.args(child_args);

    let status = match command.spawn().and_then(|mut child| child.wait()) {
        Ok(s) => s,
        Err(e) => {
            // Spawn/wait failure (e.g. command-not-found). Clean up explicitly and
            // exit 127 (shell convention for "command not found"). The guard would
            // also fire on the implicit drop at exit; remove here for clarity.
            anstream::eprintln!("vigil: run: {}: {e}", program.to_string_lossy());
            guard.remove();
            std::process::exit(127);
        }
    };

    // 7. Cleanup MUST run before exit (this is why we did NOT exec). The Drop guard
    //    fires on scope exit; we also remove explicitly so the ordering is obvious.
    guard.remove();

    // Propagate the child's exit status (normal → code; signal → 128 + signal).
    std::process::exit(propagate_status(&status));
}

/// Translate a child `ExitStatus` into the process exit code, shell-faithfully:
/// a normal exit propagates its code; a signal-terminated child propagates
/// `128 + signal` (e.g. SIGTERM(15) → 143, SIGINT(2) → 130).
fn propagate_status(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    // No code and no signal should be unreachable on unix; default to a generic
    // failure rather than 0.
    super::EX_ERROR
}

/// Install INT/TERM/HUP handlers that remove the wrapper pidfile and exit with the
/// shell-convention `128 + signal`. The pidfile path is captured BY VALUE inside
/// the handler (via a leaked 'static buffer) so cleanup survives the wrapper's own
/// scope — matching the bash trap that bakes `$pidfile` into the trap string.
///
/// Because the wrapper must clean up AND exit with the right `128 + signal` code,
/// the handler does the minimal async-signal-safe work directly: `unlink(2)` the
/// pidfile then `_exit(2)`. The RAII [`PidfileGuard`] remains the authoritative
/// cleanup on the normal (no-signal) path.
fn install_signal_cleanup(pidfile: &std::path::Path) {
    // Leak the path string so the &'static reference is valid in the handler.
    let path_owned: &'static [u8] = Box::leak(
        pidfile
            .as_os_str()
            .to_string_lossy()
            .into_owned()
            .into_bytes()
            .into_boxed_slice(),
    );

    for &sig in &[
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        // SAFETY: the handler does only async-signal-safe work: `unlink(2)` on a
        // 'static NUL-bounded path buffer and `_exit(2)`. No allocation, no Rust
        // unwinding, no locks. The path buffer is leaked, so it lives forever.
        let res = unsafe {
            signal_hook::low_level::register(sig, move || {
                unlink_then_exit(path_owned, sig);
            })
        };
        if res.is_err() {
            // If we cannot install a handler the RAII guard still covers the
            // normal path; a missed signal only risks a stale pidfile, which the
            // daemon GC reaps. Do not abort the run for this.
        }
    }
}

/// Async-signal-safe cleanup: `unlink` the pidfile then `_exit(128 + sig)`. Uses
/// only libc primitives (no allocation, no Rust runtime). The path must be a
/// 'static byte slice; we copy it into a small stack buffer and NUL-terminate.
fn unlink_then_exit(path: &'static [u8], sig: i32) -> ! {
    // PATH_MAX is 1024 on macOS; copy into a fixed stack buffer + NUL terminator.
    const MAX: usize = 1024;
    let mut buf = [0u8; MAX + 1];
    let n = path.len().min(MAX);
    buf[..n].copy_from_slice(&path[..n]);
    buf[n] = 0;
    // SAFETY: buf is NUL-terminated; unlink/_exit are async-signal-safe.
    unsafe {
        libc::unlink(buf.as_ptr() as *const libc::c_char);
        libc::_exit(128 + sig);
    }
}
