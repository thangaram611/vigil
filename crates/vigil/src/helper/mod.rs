//! Privileged power helper — Rust port of bash `bin/vigil-root-helper` (Phase 5.5).
//!
//! This module owns ALL helper logic; the `[[bin]] vigil-root-helper`'s `main()`
//! is a thin shell over [`parse_args`] / [`require_root`] / [`serve`] /
//! [`process_once`]. Putting the logic here makes every decision
//! cfg(test)-unit-testable without root or real pmset.
//!
//! ## §3.3 hardening (the crux — see [`validate`])
//! - The request file and EVERY dir check on BOTH sides use
//!   `open(O_NOFOLLOW)` + `fstat`-on-fd, never `std::fs::metadata`/`is_file`
//!   (which follow symlinks and drop the fd/nlink guarantee).
//! - Claim-then-validate: a request is `renameat`-moved into the root-owned
//!   processing subdir of the validated state_dir BEFORE it is validated, then
//!   `O_NOFOLLOW`-opened from there.
//! - The response is written `O_WRONLY|O_CREAT|O_EXCL` to a temp, `fchmod 0644`,
//!   then `renameat` relative to the validated response-dir fd so an id-charset
//!   bug cannot escape the dir.
//! - Liveness: on ANY rejection the moved request file is removed AND (when the
//!   id is charset-valid) an error response is written atomically — a rejection
//!   never becomes a client timeout, and the queue never accumulates poison
//!   files.
//!
//! ## Test seams are COMPILE-TIME only
//! The non-root bypass and the pmset/SleepDisabled fakes are behind
//! `cfg(any(test, feature = "helper-test-seam"))`. The shipped binary (no
//! feature) refuses non-root EVEN with `VIGIL_ROOT_HELPER_TESTING=1` in its env
//! (the red-team test proves this). `--allowed-uid` / `--allowed-user` are baked
//! at install time from argv, NEVER derived from request content.

pub mod validate;

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use nix::fcntl::{OFlag, openat, renameat};
use nix::sys::stat::{Mode, fstat};
use nix::unistd::{UnlinkatFlags, unlinkat};

use crate::power::pmset::{PmsetDisableSleep, SleepReader};
use validate::{
    Action, Reason, dir_stat_ok, file_stat_ok, id_from_base, open_nofollow_dir,
    open_nofollow_dir_at, request_is_single_action,
};

/// Fixed name of the processing subdir under the (validated) state dir.
const PROCESSING_DIR: &str = "processing";

/// Hard cap on how many bytes we read from a validated request file. A legitimate
/// request is a single newline-terminated action word (`engage`/`release`/
/// `status`) — at most 8 bytes + a newline. We read at most this many bytes so a
/// huge regular file planted by the served (non-root) uid cannot OOM the ROOT
/// helper on a single poll tick. Anything beyond the cap is content after the
/// first action line, which [`validate::request_is_single_action`] already
/// classifies as `extra_content` — so the cap changes NO accept/reject outcome,
/// it only makes the read total and cheap (bash reads at most two lines too).
const MAX_REQUEST_BYTES: usize = 64;

/// Helper configuration parsed from argv. `allowed_uid`/`allowed_user` are baked
/// at install time and NEVER influenced by request content.
#[derive(Debug, Clone)]
pub struct HelperConfig {
    pub request_dir: PathBuf,
    pub response_dir: PathBuf,
    pub state_dir: PathBuf,
    pub log_file: PathBuf,
    pub allowed_uid: u32,
    pub allowed_user: String,
    pub poll_secs: u64,
    /// `--serve` (false) vs `--once` (true).
    pub once: bool,
}

/// Argument-parse / validation errors. `Display` mirrors the bash `helper_die`
/// messages closely enough for diagnostics.
#[derive(Debug)]
pub enum ArgError {
    Missing(&'static str),
    InvalidUid(String),
    Unknown(String),
    Help,
}

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgError::Missing(what) => write!(f, "missing {what}"),
            ArgError::InvalidUid(v) => write!(f, "missing/invalid --allowed-uid: {v}"),
            ArgError::Unknown(a) => write!(f, "unknown argument: {a}"),
            ArgError::Help => write!(f, "help requested"),
        }
    }
}

/// Parse argv (excluding argv[0]) into a [`HelperConfig`]. `--allowed-uid` must
/// match `^[0-9]+$` and parse to a `u32` (baked at install time).
pub fn parse_args<I, S>(args: I) -> Result<HelperConfig, ArgError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut request_dir: Option<String> = None;
    let mut response_dir: Option<String> = None;
    let mut state_dir: Option<String> = None;
    let mut log_file: Option<String> = None;
    let mut allowed_uid_raw: Option<String> = None;
    let mut allowed_user: Option<String> = None;
    let mut poll_secs: u64 = 1;
    let mut once = false;

    let mut it = args.into_iter().map(|s| s.into());
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--serve" => once = false,
            "--once" => once = true,
            "--request-dir" => request_dir = it.next(),
            "--response-dir" => response_dir = it.next(),
            "--state-dir" => state_dir = it.next(),
            "--log-file" => log_file = it.next(),
            "--allowed-uid" => allowed_uid_raw = it.next(),
            "--allowed-user" => allowed_user = it.next(),
            "--poll-secs" => {
                poll_secs = it.next().and_then(|v| v.parse().ok()).unwrap_or(1);
            }
            "--help" | "-h" => return Err(ArgError::Help),
            other => return Err(ArgError::Unknown(other.to_string())),
        }
    }

    let allowed_uid_raw = allowed_uid_raw.ok_or(ArgError::Missing("--allowed-uid"))?;
    // ^[0-9]+$ then parse to u32.
    if allowed_uid_raw.is_empty() || !allowed_uid_raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ArgError::InvalidUid(allowed_uid_raw));
    }
    let allowed_uid: u32 = allowed_uid_raw
        .parse()
        .map_err(|_| ArgError::InvalidUid(allowed_uid_raw.clone()))?;

    Ok(HelperConfig {
        request_dir: request_dir
            .ok_or(ArgError::Missing("--request-dir"))?
            .into(),
        response_dir: response_dir
            .ok_or(ArgError::Missing("--response-dir"))?
            .into(),
        state_dir: state_dir.ok_or(ArgError::Missing("--state-dir"))?.into(),
        log_file: log_file.ok_or(ArgError::Missing("--log-file"))?.into(),
        allowed_uid,
        allowed_user: allowed_user.ok_or(ArgError::Missing("--allowed-user"))?,
        poll_secs: poll_secs.max(1),
        once,
    })
}

/// Refuse to run as non-root. COMPILE-TIME bypass only: when built with
/// `cfg(test)` OR the `helper-test-seam` feature this returns Ok regardless
/// (so the lib unit tests + the subprocess adversarial test can run unprivileged
/// against tempdirs). The SHIPPED binary (no feature) has NO bypass — even
/// `VIGIL_ROOT_HELPER_TESTING=1` in the env cannot reach this branch, because
/// the env var is NEVER read here.
pub fn require_root() -> Result<(), String> {
    // COMPILE-TIME seam: under cfg(test) OR the feature, ALWAYS allow.
    #[cfg(any(test, feature = "helper-test-seam"))]
    let allowed = true;
    // SHIPPED binary: require euid 0. SAFETY: geteuid() is always safe.
    #[cfg(not(any(test, feature = "helper-test-seam")))]
    let allowed = unsafe { libc::geteuid() } == 0;

    if allowed {
        Ok(())
    } else {
        Err("must run as root".to_string())
    }
}

/// Best-effort timestamped log line to the helper log file. Re-guards the log
/// dir as a non-symlink (`O_NOFOLLOW` dir open) before writing; refuses a
/// symlinked log dir. Never fails the caller.
fn helper_log(cfg: &HelperConfig, level: &str, msg: &str) {
    let Some(parent) = cfg.log_file.parent() else {
        return;
    };
    // Re-guard the log dir: open it O_NOFOLLOW|O_DIRECTORY; a symlinked dir fails.
    let Some(parent_str) = parent.to_str() else {
        return;
    };
    let Ok(dir_fd) = open_nofollow_dir(parent_str) else {
        return;
    };
    let Some(name) = cfg.log_file.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    // openat the log file relative to the validated dir fd, append mode.
    let fd = openat(
        &dir_fd,
        name,
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_APPEND | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    );
    if let Ok(fd) = fd {
        let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%z");
        let line = format!("{ts} {level} {msg}\n");
        let mut f = std::fs::File::from(fd);
        let _ = f.write_all(line.as_bytes());
    }
}

/// The uid the root-tree dirs (response/state/log/processing) must be owned by.
///
/// In the SHIPPED binary this is ALWAYS `0` (root) — the dirs are installed
/// root-owned by `vigil setup`. Under the COMPILE-TIME test seam (cfg(test) OR
/// `helper-test-seam`) the dirs are created by the unprivileged test user, so we
/// validate against the running euid instead. This mirrors the bash test, which
/// drives `helper_process_pending` directly and never runs the root-dir checks.
/// It is a COMPILE-TIME relaxation only — `VIGIL_ROOT_HELPER_TESTING=1` in the
/// env cannot reach it.
fn expected_root_uid() -> u32 {
    // COMPILE-TIME seam: under cfg(test) OR the feature the dirs are owned by the
    // unprivileged test user; otherwise the shipped binary requires root (uid 0).
    #[cfg(any(test, feature = "helper-test-seam"))]
    {
        // SAFETY: geteuid is always safe.
        unsafe { libc::geteuid() }
    }
    #[cfg(not(any(test, feature = "helper-test-seam")))]
    {
        0
    }
}

/// Open + validate a ROOT-owned directory (uid==[`expected_root_uid`], S_ISDIR,
/// not group/other writable) via `O_NOFOLLOW|O_DIRECTORY` + fstat. Returns the
/// validated fd. The O_NOFOLLOW open is what rejects a symlinked dir, regardless
/// of the owner relaxation.
fn open_validated_root_dir(path: &Path) -> Result<OwnedFd, String> {
    let path_str = path.to_str().ok_or("non-utf8 path")?;
    let fd = open_nofollow_dir(path_str).map_err(|e| format!("open {path_str}: {e}"))?;
    let st = fstat(&fd).map_err(|e| format!("fstat {path_str}: {e}"))?;
    if !dir_stat_ok(&st, expected_root_uid()) {
        return Err(format!("dir not root-owned/secure: {path_str}"));
    }
    Ok(fd)
}

/// Validate the helper's config dirs at startup: response/state/log root dirs
/// must be root-owned, non-symlink, not group/other-writable. The processing
/// subdir of state_dir must exist (created here) and be root-owned + 0700.
///
/// Returns the validated state-dir fd (used to claim-rename requests into
/// processing) and the validated response-dir fd (used to write the response).
pub struct ValidatedDirs {
    pub state_fd: OwnedFd,
    pub response_fd: OwnedFd,
    /// fd of the processing subdir (root-owned, 0700).
    pub processing_fd: OwnedFd,
}

/// Ensure a directory exists with mode 0700 (best-effort create); does NOT
/// follow symlinks for the validation that follows.
fn ensure_dir_0700(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o700);
        let _ = std::fs::set_permissions(path, perms);
    }
    Ok(())
}

/// Startup validation. Creates the response/state/log dirs + the processing
/// subdir if absent, then fd-validates each as root-owned + non-symlink +
/// not-group/other-writable.
pub fn validate_dirs(cfg: &HelperConfig) -> Result<ValidatedDirs, String> {
    // Create dirs if absent (matches bash `mkdir -p`). The fd-based validation
    // below is what actually enforces ownership/symlink/mode.
    let _ = ensure_dir_0700(&cfg.response_dir);
    let _ = ensure_dir_0700(&cfg.state_dir);
    if let Some(log_parent) = cfg.log_file.parent() {
        let _ = ensure_dir_0700(log_parent);
    }
    let processing = cfg.state_dir.join(PROCESSING_DIR);
    let _ = ensure_dir_0700(&processing);

    // Validate each root dir via fd.
    let response_fd = open_validated_root_dir(&cfg.response_dir)
        .map_err(|e| format!("invalid response directory: {e}"))?;
    let state_fd = open_validated_root_dir(&cfg.state_dir)
        .map_err(|e| format!("invalid state directory: {e}"))?;
    if let Some(log_parent) = cfg.log_file.parent() {
        open_validated_root_dir(log_parent).map_err(|e| format!("invalid log directory: {e}"))?;
    }
    // Processing dir: a root-owned 0700 subdir of the validated state dir. Open
    // it RELATIVE to the validated state fd (so it cannot be a redirect).
    let processing_fd = open_nofollow_dir_at(&state_fd, PROCESSING_DIR)
        .map_err(|e| format!("invalid processing directory: {e}"))?;
    let pst = fstat(&processing_fd).map_err(|e| format!("fstat processing: {e}"))?;
    if !dir_stat_ok(&pst, expected_root_uid()) {
        return Err("processing dir not root-owned/secure".to_string());
    }

    Ok(ValidatedDirs {
        state_fd,
        response_fd,
        processing_fd,
    })
}

/// Open + fd-validate the request DIR for the per-poll ownership re-check. The
/// request dir is owned by the ALLOWED_UID (the per-uid request dir), NOT root.
/// Returns the validated request-dir fd.
fn open_validated_request_dir(cfg: &HelperConfig) -> Result<OwnedFd, String> {
    let path = cfg.request_dir.to_str().ok_or("non-utf8 request dir")?;
    let fd = open_nofollow_dir(path).map_err(|e| format!("open request dir {path}: {e}"))?;
    let st = fstat(&fd).map_err(|e| format!("fstat request dir: {e}"))?;
    if !dir_stat_ok(&st, cfg.allowed_uid) {
        return Err(format!(
            "request dir not owned by allowed uid {}",
            cfg.allowed_uid
        ));
    }
    Ok(fd)
}

/// Read the live SleepDisabled via the seam.
fn read_sleepdisabled<S: SleepReader>(sleep: &S) -> u8 {
    sleep.read()
}

// ── baseline + engaged state files ────────────────────────────────────────────

fn baseline_file(cfg: &HelperConfig) -> PathBuf {
    cfg.state_dir.join("baseline")
}
fn engaged_file(cfg: &HelperConfig) -> PathBuf {
    cfg.state_dir.join("engaged")
}

fn is_engaged(cfg: &HelperConfig) -> bool {
    engaged_file(cfg).exists()
}

fn mark_engaged(cfg: &HelperConfig) -> std::io::Result<()> {
    let f = engaged_file(cfg);
    std::fs::write(&f, "1\n")?;
    set_mode_0600(&f);
    Ok(())
}

fn mark_released(cfg: &HelperConfig) {
    let _ = std::fs::remove_file(engaged_file(cfg));
}

fn set_mode_0600(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

/// Read the helper-side baseline file ("0"|"1"|none). For the response field.
fn current_baseline(cfg: &HelperConfig) -> String {
    let f = baseline_file(cfg);
    if let Ok(s) = std::fs::read_to_string(&f) {
        match s.trim() {
            "0" => return "0".to_string(),
            "1" => return "1".to_string(),
            _ => {}
        }
    }
    "none".to_string()
}

/// Capture the baseline for an engage IDEMPOTENTLY: if already engaged AND a
/// baseline file exists, keep it; else read SleepDisabled and write it (0600).
fn capture_baseline_for_engage<S: SleepReader>(cfg: &HelperConfig, sleep: &S) {
    let f = baseline_file(cfg);
    if is_engaged(cfg) && f.exists() {
        return;
    }
    let prior = read_sleepdisabled(sleep);
    let _ = std::fs::write(&f, format!("{prior}\n"));
    set_mode_0600(&f);
}

// ── action handlers ────────────────────────────────────────────────────────────

/// engage: capture-baseline-for-engage → pmset disablesleep 1 → mark engaged.
/// On pmset failure: Err (the caller emits `pmset_engage_failed`), engaged NOT
/// marked.
fn action_engage<P: PmsetDisableSleep, S: SleepReader>(
    cfg: &HelperConfig,
    pmset: &P,
    sleep: &S,
) -> Result<(), &'static str> {
    capture_baseline_for_engage(cfg, sleep);
    pmset.set(1).map_err(|_| "pmset_engage_failed")?;
    mark_engaged(cfg).map_err(|_| "pmset_engage_failed")?;
    Ok(())
}

/// release: if NOT engaged → no-op (do not clobber externally-set SleepDisabled).
/// If engaged → target = baseline (corrupt/missing => 0, FAIL-SAFE) → pmset
/// disablesleep <target> → mark released (KEEP baseline file). On pmset failure:
/// Err (`pmset_release_failed`), KEEP engaged for retry.
fn action_release<P: PmsetDisableSleep>(cfg: &HelperConfig, pmset: &P) -> Result<(), &'static str> {
    if !is_engaged(cfg) {
        // idle release: no pmset transition, must NOT clobber SleepDisabled.
        return Ok(());
    }
    let target: u8 = match current_baseline(cfg).as_str() {
        "0" => 0,
        "1" => 1,
        _ => 0, // FAIL-SAFE
    };
    pmset.set(target).map_err(|_| "pmset_release_failed")?;
    mark_released(cfg);
    Ok(())
}

// ── response write (openat/renameat relative to validated response-dir fd) ─────

/// Write the response atomically into the validated response dir. Creates a temp
/// `O_WRONLY|O_CREAT|O_EXCL`, `fchmod 0644`, writes the five `key=value` lines,
/// then `renameat(response_fd, tmp, response_fd, resp.<id>)` so an id-charset bug
/// cannot escape the dir.
#[allow(clippy::too_many_arguments)]
fn write_response<S: SleepReader>(
    response_fd: &OwnedFd,
    cfg: &HelperConfig,
    sleep: &S,
    id: &str,
    status: &str,
    action: &str,
    message: &str,
) -> std::io::Result<()> {
    let current = read_sleepdisabled(sleep);
    let baseline = current_baseline(cfg);
    let body = format!(
        "status={status}\naction={action}\nbaseline={baseline}\ncurrent={current}\nmessage={message}\n"
    );

    let tmp_name = format!(".resp.{id}.{}", std::process::id());
    let final_name = format!("resp.{id}");

    // O_WRONLY|O_CREAT|O_EXCL temp relative to the validated response fd.
    let tmp_fd = openat(
        response_fd,
        tmp_name.as_str(),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o644),
    )
    .map_err(std::io::Error::other)?;

    // fchmod 0644 (umask may have masked the open mode).
    let _ = nix::sys::stat::fchmod(&tmp_fd, Mode::from_bits_truncate(0o644));

    {
        let mut f = std::fs::File::from(tmp_fd);
        f.write_all(body.as_bytes())?;
        f.flush()?;
    }

    // renameat(tmp -> resp.<id>) relative to the validated response fd.
    renameat(
        response_fd,
        tmp_name.as_str(),
        response_fd,
        final_name.as_str(),
    )
    .map_err(std::io::Error::other)?;
    Ok(())
}

// ── claim-then-validate request processing ─────────────────────────────────────

/// Result of processing one request. The carried reason/message is surfaced via
/// `Display` so callers (and tests) can assert the outcome; the fields are
/// intentionally part of the public-ish shape even though the current
/// `process_pending` only counts.
#[derive(Debug)]
enum Processed {
    Ok,
    Rejected(Reason),
    PmsetFailed(&'static str),
}

impl Processed {
    /// True iff the request was handled with `status=ok`.
    #[allow(dead_code)]
    fn is_ok(&self) -> bool {
        matches!(self, Processed::Ok)
    }

    /// The reason/message token, when this is a non-ok outcome.
    #[allow(dead_code)]
    fn detail(&self) -> Option<&str> {
        match self {
            Processed::Ok => None,
            Processed::Rejected(r) => Some(r.message()),
            Processed::PmsetFailed(m) => Some(m),
        }
    }
}

/// Read the body from an already-validated fd, BOUNDED to [`MAX_REQUEST_BYTES`]
/// (plus one sentinel byte to detect overflow). A request is a single short
/// action line; reading the whole file would let the served uid OOM the root
/// helper with a multi-GB regular file. We read at most `MAX_REQUEST_BYTES + 1`
/// bytes: the first line of any legitimate request fits in the cap, and anything
/// past it is content [`validate::request_is_single_action`] already rejects as
/// `extra_content` — so the bound is transparent to accept/reject outcomes.
fn read_all_from_fd(fd: &OwnedFd) -> std::io::Result<Vec<u8>> {
    // Dup the fd into a File without consuming the OwnedFd (we still hold it).
    // SAFETY: as_raw_fd is valid for the lifetime of `fd`; we read then drop the
    // borrowed File without closing the underlying fd (use into-from carefully).
    let raw = fd.as_raw_fd();
    // SAFETY: dup the fd so the File we build owns its own descriptor and
    // dropping it does not close `fd`.
    let dup = unsafe { libc::dup(raw) };
    if dup < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `dup` is a fresh owned descriptor.
    let owned = unsafe { OwnedFd::from_raw_fd(dup) };
    let f = std::fs::File::from(owned);
    // Read at most MAX_REQUEST_BYTES + 1 (the +1 sentinel makes any over-cap file
    // present > MAX_REQUEST_BYTES bytes, which carries content past the first
    // action line into the parser => extra_content, matching bash).
    let mut buf = Vec::new();
    f.take(MAX_REQUEST_BYTES as u64 + 1).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Process ONE request file named `base` found in the request dir.
///
/// Claim-then-validate:
/// 1. id charset guard ([`id_from_base`]); bad => remove from request dir, no
///    response (no valid id to name one).
/// 2. `renameat` the file from the request dir into the root-owned processing
///    dir (claims it; an attacker can no longer swap it).
/// 3. `O_NOFOLLOW`-open the MOVED file, fstat the fd ([`file_stat_ok`]).
/// 4. read the whole body from the SAME fd; single-action-line parse.
/// 5. dispatch engage/release/status.
/// 6. write the response; ALWAYS remove the moved file.
///
/// On ANY rejection (steps 3–5) the moved file is removed AND an error response
/// is written (the id is charset-valid by step 1) — never a client timeout.
#[allow(clippy::too_many_arguments)]
fn process_request<P: PmsetDisableSleep, S: SleepReader>(
    cfg: &HelperConfig,
    dirs: &ValidatedDirs,
    request_dir_fd: &OwnedFd,
    pmset: &P,
    sleep: &S,
    base: &str,
) -> Processed {
    // Step 1: id charset guard. Bad filename => remove from the request dir and
    // bail (no valid id to name a response).
    let Some(id) = id_from_base(base) else {
        let _ = unlinkat(request_dir_fd, base, UnlinkatFlags::NoRemoveDir);
        helper_log(
            cfg,
            "WARN",
            &format!("reject request base={base} reason=bad_filename"),
        );
        return Processed::Rejected(Reason::BadFilename);
    };

    // Step 2: claim by renameat into the root-owned processing dir.
    let moved_name = format!("{base}.{}", std::process::id());
    if renameat(
        request_dir_fd,
        base,
        &dirs.processing_fd,
        moved_name.as_str(),
    )
    .is_err()
    {
        // Could not claim (already gone / race). Nothing to do.
        return Processed::Rejected(Reason::InvalidRequestFile);
    }

    // Helper closure to finalize a rejection: remove the moved file + write an
    // error response. `action_word` is the `action=` token written into the
    // response — mirrors bash `helper_reject_processed`: `unknown` for the
    // file-validation rejections (invalid_request_file/bad_filename), and the
    // attempted/valid action word for the content rejections.
    let reject = |reason: Reason, action_word: &str| -> Processed {
        let _ = unlinkat(
            &dirs.processing_fd,
            moved_name.as_str(),
            UnlinkatFlags::NoRemoveDir,
        );
        let _ = write_response(
            &dirs.response_fd,
            cfg,
            sleep,
            &id,
            "error",
            action_word,
            reason.message(),
        );
        helper_log(
            cfg,
            "WARN",
            &format!(
                "reject request id={id} action={action_word} reason={}",
                reason.message()
            ),
        );
        Processed::Rejected(reason)
    };

    // Step 3: O_NOFOLLOW open the MOVED file + fstat the fd.
    let fd = match validate::open_nofollow_regular(&dirs.processing_fd, moved_name.as_str()) {
        Ok(fd) => fd,
        // open(2) fails on a symlink (ELOOP) — that IS the symlink rejection.
        Err(_) => return reject(Reason::InvalidRequestFile, "unknown"),
    };
    let st = match fstat(&fd) {
        Ok(st) => st,
        Err(_) => return reject(Reason::InvalidRequestFile, "unknown"),
    };
    if let Err(reason) = file_stat_ok(&st, cfg.allowed_uid) {
        return reject(reason, "unknown");
    }

    // Step 4: read the WHOLE body from the SAME fd; single-action-line parse.
    let body = match read_all_from_fd(&fd) {
        Ok(b) => b,
        Err(_) => return reject(Reason::InvalidRequestFile, "unknown"),
    };
    let action = match request_is_single_action(&body) {
        Ok(a) => a,
        // The rejection carries the attempted action word for the response
        // action= field (bash parity).
        Err(rej) => return reject(rej.reason, &rej.action_word),
    };

    // Step 5: dispatch. The action is now trusted (validated id + fd + content).
    let (status, message): (&str, &str) = match action {
        Action::Engage => match action_engage(cfg, pmset, sleep) {
            Ok(()) => ("ok", "ok"),
            Err(m) => ("error", m),
        },
        Action::Release => match action_release(cfg, pmset) {
            Ok(()) => ("ok", "ok"),
            Err(m) => ("error", m),
        },
        Action::Status => ("ok", "ok"),
    };

    // Step 6: write the response, then ALWAYS remove the moved file.
    let _ = write_response(
        &dirs.response_fd,
        cfg,
        sleep,
        &id,
        status,
        action.as_str(),
        message,
    );
    let _ = unlinkat(
        &dirs.processing_fd,
        moved_name.as_str(),
        UnlinkatFlags::NoRemoveDir,
    );
    let action_str = action.as_str();
    helper_log(
        cfg,
        "INFO",
        &format!(
            "request id={id} uid={} user={} action={action_str} result={status} message={message}",
            cfg.allowed_uid, cfg.allowed_user
        ),
    );

    if status == "ok" {
        Processed::Ok
    } else if let Action::Status = action {
        Processed::Ok
    } else {
        Processed::PmsetFailed(message)
    }
}

/// List the `req.*` basenames currently in the request dir (a directory read).
/// Orphaned `.<x>` temp files and non-`req.` entries are ignored.
fn list_request_bases(cfg: &HelperConfig) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&cfg.request_dir) {
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with("req.")
            {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    out
}

/// Clean up orphaned files from the processing dir left by a crashed prior
/// instance (KeepAlive restart). Each is root-owned (the dir is root-owned +
/// 0700, validated at startup), so removing them is safe; we use unlinkat
/// relative to the validated processing fd.
fn cleanup_orphaned_processing(dirs: &ValidatedDirs, cfg: &HelperConfig) {
    // Read the processing dir via its path (the fd validated it is root-owned).
    let processing = cfg.state_dir.join(PROCESSING_DIR);
    if let Ok(rd) = std::fs::read_dir(&processing) {
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                let _ = unlinkat(&dirs.processing_fd, name, UnlinkatFlags::NoRemoveDir);
            }
        }
    }
}

/// One poll pass: re-validate the request DIR ownership (fd-based), then process
/// every pending `req.*`. Returns the number of requests processed.
pub fn process_pending<P: PmsetDisableSleep, S: SleepReader>(
    cfg: &HelperConfig,
    dirs: &ValidatedDirs,
    pmset: &P,
    sleep: &S,
) -> usize {
    // Per-poll request-DIR ownership re-check (fd-based, uid==ALLOWED_UID).
    let request_dir_fd = match open_validated_request_dir(cfg) {
        Ok(fd) => fd,
        Err(e) => {
            helper_log(cfg, "ERROR", &format!("request dir rejected: {e}"));
            return 0;
        }
    };

    let mut count = 0;
    for base in list_request_bases(cfg) {
        let _ = process_request(cfg, dirs, &request_dir_fd, pmset, sleep, &base);
        count += 1;
    }
    count
}

/// Run one poll pass against the real seams (`MacPmset` / `MacSleepReader`), with
/// startup dir validation + orphan cleanup. Used by `--once` and by each loop
/// iteration of `--serve`.
pub fn process_once_with_seams<P: PmsetDisableSleep, S: SleepReader>(
    cfg: &HelperConfig,
    pmset: &P,
    sleep: &S,
) -> Result<usize, String> {
    let dirs = validate_dirs(cfg)?;
    cleanup_orphaned_processing(&dirs, cfg);
    Ok(process_pending(cfg, &dirs, pmset, sleep))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
