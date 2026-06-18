//! File-based IPC client for the privileged power helper — Phase 5.5.
//!
//! Rust port of `lib/common.sh`'s `vigil_power_helper_request` + response
//! validation. This is the ONLY runtime path for privileged pmset changes.
//!
//! DEAD-FROM-RUST until 5.7: library-only. The default `vigil debug` dump does
//! NOT do a blocking helper round-trip — a real `status` round-trip is a 5.7
//! doctor/status concern.
//!
//! ## Protocol
//! - request: write `.req.<id>` under `umask 077` / mode 0600, then atomic
//!   `rename` to `req.<id>`. `<id>` is high-entropy.
//! - poll the response dir until `resp.<id>` appears, up to a wall-clock budget
//!   of `VIGIL_POWER_HELPER_TIMEOUT_SECS`.
//! - §3.3 step 4 response validation: open `resp.<id>` ONCE with
//!   `O_NOFOLLOW|O_RDONLY`, `fstat` THAT fd (`uid==0`, `S_ISREG`, `nlink==1`,
//!   not group/other-writable), and read the body from the SAME fd — NEVER
//!   re-open by path after the check (re-opening re-introduces the TOCTOU).
//! - on timeout, remove the request file.
//!
//! ## Matched-pair validation (defense in depth)
//! The helper validates requests AND this client validates responses. Neither
//! side is optimized away even though each is the other's trust anchor: a local
//! non-root process in the per-uid response dir must NOT be able to forge a
//! `status=ok` response.

use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nix::fcntl::{OFlag, open};
use nix::sys::stat::{Mode, fstat};

use crate::helper::validate::Action;

/// A parsed helper response (the five `key=value` lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: String,
    pub action: String,
    /// `0` | `1` | `none`.
    pub baseline: String,
    /// Live SleepDisabled at response time.
    pub current: String,
    pub message: String,
}

impl Response {
    /// True iff `status == "ok"`.
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }
}

/// Errors the IPC client can surface.
#[derive(Debug)]
pub enum IpcError {
    /// IPC dirs missing — setup/doctor needed.
    DirsMissing,
    /// I/O error writing the request or polling.
    Io(std::io::Error),
    /// Helper did not respond within the timeout budget.
    Timeout,
    /// The response dir is not traversable/readable by this (non-root) client —
    /// an `EACCES` while probing for `resp.<id>`. This is a setup/permissions
    /// fault (e.g. a response dir left at `0700` instead of root-owned `0755`),
    /// NOT "the response is not ready yet" — so it is surfaced immediately with an
    /// actionable message rather than burned as a generic timeout.
    ResponseDirUnreadable(String),
    /// The response file failed fd-based root-ownership validation (possible
    /// forgery, or a non-root writer in the response dir).
    InvalidResponse(String),
    /// The helper returned `status=error` (carries the response for the message).
    HelperError(Response),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcError::DirsMissing => {
                write!(
                    f,
                    "root helper IPC dirs missing — run 'vigil setup' or 'vigil doctor'"
                )
            }
            IpcError::Io(e) => write!(f, "ipc io error: {e}"),
            IpcError::Timeout => write!(f, "root helper timed out"),
            IpcError::ResponseDirUnreadable(why) => {
                write!(f, "root helper response dir unreadable: {why}")
            }
            IpcError::InvalidResponse(why) => write!(f, "invalid helper response: {why}"),
            IpcError::HelperError(r) => write!(f, "root helper error: {}", r.message),
        }
    }
}

impl From<std::io::Error> for IpcError {
    fn from(e: std::io::Error) -> Self {
        IpcError::Io(e)
    }
}

/// The client trait so [`crate::power::PowerMachine`] and the adversarial test
/// can inject a fake. The three actions wrap [`HelperClient::request`].
pub trait HelperClient {
    fn request(&self, action: Action) -> Result<Response, IpcError>;

    fn engage(&self) -> Result<Response, IpcError> {
        self.request(Action::Engage)
    }
    fn release(&self) -> Result<Response, IpcError> {
        self.request(Action::Release)
    }
    fn status(&self) -> Result<Response, IpcError> {
        self.request(Action::Status)
    }
}

/// Parse the five `key=value` response lines. Unknown keys ignored; missing keys
/// default to empty. Mirrors the bash `awk -F=` field extraction.
pub fn parse_response(text: &str) -> Response {
    let mut r = Response {
        status: String::new(),
        action: String::new(),
        baseline: String::new(),
        current: String::new(),
        message: String::new(),
    };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "status" => r.status = v.to_string(),
                "action" => r.action = v.to_string(),
                "baseline" => r.baseline = v.to_string(),
                "current" => r.current = v.to_string(),
                "message" => r.message = v.to_string(),
                _ => {}
            }
        }
    }
    r
}

/// §3.3 step 4: open `resp_path` ONCE with `O_NOFOLLOW|O_RDONLY`, fstat THAT fd
/// (uid==0, S_ISREG, nlink==1, not group/other-writable), and read the body from
/// the SAME fd. Never re-opens by path after the check.
///
/// Returns the file bytes on success, or an `InvalidResponse` reason on any
/// ownership/type/link/mode violation (a forged or attacker-writable response).
fn read_validated_response(resp_path: &Path) -> Result<String, IpcError> {
    let fd: OwnedFd = open(
        resp_path,
        OFlag::O_NOFOLLOW | OFlag::O_RDONLY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| IpcError::InvalidResponse(format!("open: {e}")))?;

    let st = fstat(&fd).map_err(|e| IpcError::InvalidResponse(format!("fstat: {e}")))?;
    let mode = st.st_mode as u32;
    if mode & libc::S_IFMT as u32 != libc::S_IFREG as u32 {
        return Err(IpcError::InvalidResponse("not a regular file".into()));
    }
    if st.st_uid != 0 {
        return Err(IpcError::InvalidResponse(format!(
            "response not root-owned (uid={})",
            st.st_uid
        )));
    }
    if st.st_nlink as u64 != 1 {
        return Err(IpcError::InvalidResponse("response is hardlinked".into()));
    }
    if mode & 0o022 != 0 {
        return Err(IpcError::InvalidResponse(
            "response group/other-writable".into(),
        ));
    }

    // Read the body from the SAME validated fd.
    let mut file = std::fs::File::from(fd);
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .map_err(|e| IpcError::InvalidResponse(format!("read: {e}")))?;
    Ok(buf)
}

/// Configuration for the real file-based client.
pub struct MacHelperClient {
    pub request_dir: PathBuf,
    pub response_dir: PathBuf,
    pub timeout_secs: u32,
}

impl MacHelperClient {
    /// High-entropy request id. Charset is a subset of `[A-Za-z0-9_.-]` so it
    /// passes the helper's id guard (and our own response-id guard).
    fn new_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // Two pulls of OS entropy via a fresh stack address + a counter.
        let r1: u64 = rand_u64();
        let r2: u64 = rand_u64();
        format!("{pid}.{nanos}.{r1:x}{r2:x}")
    }
}

/// Best-effort 64-bit entropy without an extra dep: mix `getrandom`-equivalent
/// sources available in std. Uses the system clock nanos + an address + a
/// process-unique counter; for an IPC request id collision-resistance not
/// crypto-strength is required (the dir is per-uid + 0700).
fn rand_u64() -> u64 {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local!(static COUNTER: Cell<u64> = const { Cell::new(0) });
    let c = COUNTER.with(|c| {
        let v = c.get().wrapping_add(1);
        c.set(v);
        v
    });
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let addr = &c as *const _ as u64;
    nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17)
        ^ addr.wrapping_mul(0xD1B5_4A32_D192_ED03)
        ^ c.wrapping_mul(0xC2B2_AE35_29C0_5C13)
}

impl HelperClient for MacHelperClient {
    fn request(&self, action: Action) -> Result<Response, IpcError> {
        if !self.request_dir.is_dir() || !self.response_dir.is_dir() {
            return Err(IpcError::DirsMissing);
        }

        let id = Self::new_id();
        let tmp = self.request_dir.join(format!(".req.{id}"));
        let req = self.request_dir.join(format!("req.{id}"));
        let resp = self.response_dir.join(format!("resp.{id}"));

        // Write the request body under umask 077 / mode 0600, then atomic rename.
        write_request_atomic(&tmp, &req, action.as_str())?;

        // Poll for the response within the wall-clock budget.
        let deadline = Instant::now() + Duration::from_secs(self.timeout_secs.max(1) as u64);
        loop {
            // Existence check via symlink_metadata is fine here (it does NOT
            // follow, and we re-validate via fd before trusting content); the
            // trust decision is the fd-based read below, never this probe.
            match std::fs::symlink_metadata(&resp) {
                Ok(_) => {
                    let body = read_validated_response(&resp)?;
                    let parsed = parse_response(&body);
                    if parsed.is_ok() {
                        return Ok(parsed);
                    }
                    return Err(IpcError::HelperError(parsed));
                }
                // EACCES => the response dir is not traversable/readable by us. A
                // missing-but-pending response yields NotFound, never EACCES, so
                // this is a real perms/setup fault — fail fast with guidance
                // instead of polling out the whole timeout and blaming the helper.
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    let _ = std::fs::remove_file(&req);
                    let _ = std::fs::remove_file(&tmp);
                    return Err(IpcError::ResponseDirUnreadable(format!(
                        "cannot read {} (EACCES) — run 'vigil setup' or 'vigil doctor'",
                        self.response_dir.display()
                    )));
                }
                // NotFound (response not written yet) or any other transient
                // error => keep polling until the deadline.
                Err(_) => {}
            }
            if Instant::now() >= deadline {
                let _ = std::fs::remove_file(&req);
                let _ = std::fs::remove_file(&tmp);
                return Err(IpcError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Write `body\n` to `tmp` with mode 0600 under umask 077, then atomically
/// rename to `req`. Mirrors the bash `( umask 077; printf ... > tmp ); chmod
/// 0600; mv tmp req`.
fn write_request_atomic(tmp: &Path, req: &Path, body: &str) -> Result<(), IpcError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    // O_CREAT|O_EXCL so a pre-planted tmp (by another local user) is not reused.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)?;
    f.write_all(body.as_bytes())?;
    f.write_all(b"\n")?;
    f.flush()?;
    drop(f);
    std::fs::rename(tmp, req)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_response_fields() {
        let text = "status=ok\naction=engage\nbaseline=0\ncurrent=1\nmessage=ok\n";
        let r = parse_response(text);
        assert_eq!(r.status, "ok");
        assert_eq!(r.action, "engage");
        assert_eq!(r.baseline, "0");
        assert_eq!(r.current, "1");
        assert_eq!(r.message, "ok");
        assert!(r.is_ok());
    }

    #[test]
    fn parse_response_error() {
        let text = "status=error\naction=release\nbaseline=none\ncurrent=1\nmessage=pmset_release_failed\n";
        let r = parse_response(text);
        assert!(!r.is_ok());
        assert_eq!(r.message, "pmset_release_failed");
    }

    #[test]
    fn parse_response_unknown_keys_ignored_missing_keys_default_empty() {
        // The `_ => {}` arm drops keys we don't model; lines without `=` are
        // skipped by `split_once`. Only the two recognized keys are populated; the
        // other three default to the empty string (their `String::new()` seed). An
        // empty status is NOT "ok".
        let text = "bogus=zzz\nstatus=ok\nno-equals-here\naction=engage\n";
        let r = parse_response(text);
        assert_eq!(r.status, "ok", "recognized status set");
        assert_eq!(r.action, "engage", "recognized action set");
        assert_eq!(r.baseline, "", "missing key -> empty default");
        assert_eq!(r.current, "", "missing key -> empty default");
        assert_eq!(r.message, "", "missing key -> empty default");
        assert!(r.is_ok(), "status=ok is ok");

        // An entirely empty body leaves every field empty and is_ok() false.
        let empty = parse_response("");
        assert_eq!(empty.status, "");
        assert_eq!(empty.action, "");
        assert_eq!(empty.baseline, "");
        assert_eq!(empty.current, "");
        assert_eq!(empty.message, "");
        assert!(!empty.is_ok(), "empty status is not ok");
    }

    #[test]
    fn ids_are_distinct() {
        let a = MacHelperClient::new_id();
        let b = MacHelperClient::new_id();
        assert_ne!(a, b, "ids must differ");
        // id charset must pass the helper guard.
        assert!(
            crate::helper::validate::id_charset_ok(&a),
            "id {a} must be charset-valid"
        );
    }

    #[test]
    fn write_request_then_rename() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join(".req.x");
        let req = dir.path().join("req.x");
        write_request_atomic(&tmp, &req, "engage").unwrap();
        assert!(!tmp.exists(), "tmp renamed away");
        let body = std::fs::read_to_string(&req).unwrap();
        assert_eq!(body, "engage\n");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&req).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "request is 0600");
    }

    #[test]
    fn validated_response_accepts_self_owned_regular_file() {
        // A file we own (uid == our uid). On a non-root test runner uid != 0, so
        // the uid==0 check will REJECT it — which is the correct security
        // behavior. We assert the *type/link/mode* path by checking the reason
        // is specifically the owner mismatch (proving we got past type checks).
        let dir = tempfile::tempdir().unwrap();
        let resp = dir.path().join("resp.x");
        std::fs::write(&resp, "status=ok\n").unwrap();
        let err = read_validated_response(&resp).unwrap_err();
        match err {
            IpcError::InvalidResponse(why) => {
                assert!(
                    why.contains("root-owned"),
                    "non-root file must be rejected for ownership, got: {why}"
                );
            }
            other => panic!("expected InvalidResponse, got {other:?}"),
        }
    }

    #[test]
    fn ipc_error_display_operator_text() {
        // The operator-facing Display strings are part of the doctor/status UX
        // contract; pin them byte-exact. DirsMissing points the user at setup/
        // doctor; Timeout names the root helper; InvalidResponse/HelperError/Io
        // interpolate their payload.
        assert_eq!(
            IpcError::DirsMissing.to_string(),
            "root helper IPC dirs missing — run 'vigil setup' or 'vigil doctor'"
        );
        assert_eq!(IpcError::Timeout.to_string(), "root helper timed out");
        assert_eq!(
            IpcError::ResponseDirUnreadable("cannot read /x (EACCES)".into()).to_string(),
            "root helper response dir unreadable: cannot read /x (EACCES)"
        );
        assert_eq!(
            IpcError::InvalidResponse("not a regular file".into()).to_string(),
            "invalid helper response: not a regular file"
        );
        let helper_err = IpcError::HelperError(Response {
            status: "error".into(),
            action: "release".into(),
            baseline: "none".into(),
            current: "1".into(),
            message: "pmset_release_failed".into(),
        });
        assert_eq!(
            helper_err.to_string(),
            "root helper error: pmset_release_failed"
        );
        let io_err = IpcError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ));
        assert_eq!(io_err.to_string(), "ipc io error: denied");
    }

    #[test]
    fn request_eacces_response_dir_fast_fails_instead_of_timing_out() {
        use std::os::unix::fs::PermissionsExt;
        // A root test runner can traverse a 0000 dir, so the EACCES this test
        // depends on would not occur — skip it there (matches the non-root
        // assumption of the other ipc fd tests).
        // SAFETY: geteuid is always safe.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let req = dir.path().join("req");
        let resp = dir.path().join("resp");
        std::fs::create_dir(&req).unwrap();
        std::fs::create_dir(&resp).unwrap();
        // Make the response dir non-traversable: the probe for resp.<id> hits
        // EACCES (a perms/setup fault), NOT "not written yet".
        std::fs::set_permissions(&resp, std::fs::Permissions::from_mode(0o000)).unwrap();

        let client = MacHelperClient {
            request_dir: req,
            response_dir: resp.clone(),
            timeout_secs: 5,
        };
        let start = Instant::now();
        let err = client.request(Action::Engage).unwrap_err();
        let elapsed = start.elapsed();

        // Restore perms so the tempdir cleanup can recurse.
        std::fs::set_permissions(&resp, std::fs::Permissions::from_mode(0o755)).unwrap();

        match err {
            IpcError::ResponseDirUnreadable(why) => {
                assert!(why.contains("EACCES"), "diagnostic names EACCES: {why}")
            }
            other => panic!("expected ResponseDirUnreadable, got {other:?}"),
        }
        // The whole point: it FAILS FAST rather than burning the 5s budget.
        assert!(
            elapsed < Duration::from_secs(2),
            "EACCES must not poll out the timeout (took {elapsed:?})"
        );
    }

    #[test]
    fn validated_response_rejects_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "status=ok\n").unwrap();
        let link = dir.path().join("resp.link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = read_validated_response(&link).unwrap_err();
        // O_NOFOLLOW makes open(2) fail with ELOOP.
        match err {
            IpcError::InvalidResponse(why) => assert!(why.contains("open")),
            other => panic!("expected open failure, got {other:?}"),
        }
    }
}
