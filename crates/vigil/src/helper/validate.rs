//! §3.3 privilege-boundary validation primitives — Phase 5.5.
//!
//! This is the crux of the helper hardening. EVERY check here operates on a
//! file descriptor's `fstat`, never on a path. We NEVER call
//! `std::fs::metadata` / `symlink_metadata` / `Path::is_file` / `Path::is_dir`:
//! - `metadata` FOLLOWS symlinks (a symlinked request file would be opened by
//!   path and read through the link — a silent redirect-of-root-writes bug),
//! - none of them give an fd, so they cannot close the validate→open TOCTOU
//!   window, and a fresh `stat` after `open` still drops the `st_nlink` guarantee
//!   read from the SAME fd the body is read from.
//!
//! ## Pure vs side-effecting split
//! - PURE (no IO): [`id_from_base`], [`request_is_single_action`],
//!   [`file_stat_ok`], [`dir_stat_ok`] — all operate on already-collected inputs
//!   (a filename string, the file bytes, or a `libc::stat` snapshot).
//! - SIDE-EFFECTING: [`open_nofollow_regular`], [`open_nofollow_dir`],
//!   [`fstat_fd`] — the `open(O_NOFOLLOW)` + `fstat` raw primitives. They return
//!   an `OwnedFd` so the caller reads the body from the SAME fd it validated.

use std::os::fd::{AsFd, OwnedFd};

use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::{FileStat, Mode, fstat};

/// Reasons a request can be rejected. The `Display` form is the EXACT
/// `message=<reason>` token written in the error response (byte-identical to the
/// bash helper's reason strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Filename charset / `req.` prefix / `.`|`..` traversal guard failed.
    BadFilename,
    /// The fd-based file validation failed (symlink at open(2), non-regular,
    /// wrong owner, hardlink (nlink != 1), or group/other-writable). The bash
    /// helper collapses all of these into the single `invalid_request_file`
    /// message, so we preserve that token for response parity.
    InvalidRequestFile,
    /// The first action line was not engage|release|status.
    InvalidAction,
    /// Content present after the first newline-terminated action line (including
    /// trailing bytes with no final newline).
    ExtraContent,
}

impl Reason {
    /// The `message=` token written into the response. MUST match the bash
    /// helper exactly.
    pub fn message(self) -> &'static str {
        match self {
            Reason::BadFilename => "bad_filename",
            Reason::InvalidRequestFile => "invalid_request_file",
            Reason::InvalidAction => "invalid_action",
            Reason::ExtraContent => "extra_content",
        }
    }
}

/// The only three actions the helper accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Engage,
    Release,
    Status,
}

impl Action {
    /// The action word as written into the `action=` response field.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Engage => "engage",
            Action::Release => "release",
            Action::Status => "status",
        }
    }
}

/// Strip the `req.` prefix and validate the request id charset.
///
/// The id charset `^[A-Za-z0-9_.-]+$` is the ONLY traversal guard for the
/// response path, so we ALSO reject the bare `.` and `..` ids explicitly (they
/// pass the charset but would escape/alias the response dir). Returns the id on
/// success, `None` on any violation.
pub fn id_from_base(base: &str) -> Option<String> {
    let id = base.strip_prefix("req.")?;
    if id.is_empty() {
        return None;
    }
    // Reject `.` and `..` — they pass the charset but are traversal/aliasing.
    if id == "." || id == ".." {
        return None;
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    {
        return None;
    }
    Some(id.to_string())
}

/// Validate a response id used by the IPC CLIENT to construct `resp.<id>`. Same
/// charset + `.`/`..` guard as [`id_from_base`] but the input is the raw id (no
/// `req.` prefix). Returns true iff safe to use in `resp.<id>`.
pub fn id_charset_ok(id: &str) -> bool {
    if id.is_empty() || id == "." || id == ".." {
        return false;
    }
    id.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

/// A request rejected by [`request_is_single_action`], carrying both the
/// [`Reason`] and the attempted first-line action WORD for the response
/// `action=` field (bash parity — see [`request_is_single_action`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedRequest {
    pub reason: Reason,
    /// The `action=` token to write into the error response. Mirrors bash
    /// `helper_reject_processed`: the bad first-line word (or `"empty"` for a
    /// blank/empty first line) for [`Reason::InvalidAction`]; the VALID action
    /// word for [`Reason::ExtraContent`].
    pub action_word: String,
}

/// Parse the WHOLE validated request body: the first newline-terminated line
/// must be exactly `engage`|`release`|`status`, and there must be NO content
/// after it.
///
/// Rejection cases (all => an error response, no pmset call):
/// - empty body, or a first line that is not one of the three actions =>
///   [`Reason::InvalidAction`].
/// - any byte after the first `\n` (including a trailing line with no final
///   `\n`) => [`Reason::ExtraContent`].
///
/// On rejection the returned [`RejectedRequest`] carries the attempted first-line
/// action word for the response `action=` field, byte-for-byte as bash writes it:
/// the bad/blank word (`"empty"` for an empty first line) for `invalid_action`,
/// and the valid action word for `extra_content`.
///
/// This mirrors the bash helper's `read action; read extra; [[ -n "$extra" ]]`
/// but is stricter and total: it reads ALL bytes (the bash helper reads only the
/// first two lines, but a third+ line is also extra content here — and the bash
/// helper's `extra` already catches any second-line content).
pub fn request_is_single_action(bytes: &[u8]) -> Result<Action, RejectedRequest> {
    // Split on the FIRST newline. Everything after it is "extra".
    let (first, rest) = match bytes.iter().position(|&b| b == b'\n') {
        Some(nl) => (&bytes[..nl], &bytes[nl + 1..]),
        // No newline at all: the entire body is the "first line" and there is
        // trailing content with no final newline. The bash helper's first
        // `read` would still capture it as the action, but the contract is a
        // newline-TERMINATED action line, so a no-final-newline body is extra
        // content. We classify on the action first: if the (whole) body is a
        // valid action token but unterminated, it's extra_content; otherwise
        // invalid_action.
        None => (bytes, &b""[..]),
    };

    let action = match first {
        b"engage" => Action::Engage,
        b"release" => Action::Release,
        b"status" => Action::Status,
        _ => {
            // Bash writes `action="${action:-empty}"`: the bad first-line word,
            // or `empty` for a blank/empty first line. Mirror that token.
            let word = String::from_utf8_lossy(first);
            let action_word = if word.is_empty() {
                "empty".to_string()
            } else {
                word.into_owned()
            };
            return Err(RejectedRequest {
                reason: Reason::InvalidAction,
                action_word,
            });
        }
    };

    // A valid action token. Now: was the line newline-terminated, and is there
    // anything after it?
    let terminated = bytes.get(first.len()) == Some(&b'\n');
    if !terminated {
        // Valid action word but no terminating newline => extra_content
        // (trailing content without a newline is rejected). Bash carries the
        // VALID action word into the response.
        return Err(RejectedRequest {
            reason: Reason::ExtraContent,
            action_word: action.as_str().to_string(),
        });
    }
    if !rest.is_empty() {
        return Err(RejectedRequest {
            reason: Reason::ExtraContent,
            action_word: action.as_str().to_string(),
        });
    }
    Ok(action)
}

/// PURE predicate over a `libc::stat` snapshot of a REGULAR request file fd.
///
/// Checks, in the bash helper's order:
/// 1. `S_ISREG` (a fifo/dir/etc. that survived O_NOFOLLOW is still rejected),
/// 2. `st_uid == allowed_uid` (owner),
/// 3. `st_nlink == 1` (hardlink guard — MUST be read from the same fd),
/// 4. not group- or other-writable (`mode & 0o022 == 0`).
///
/// All four collapse to [`Reason::InvalidRequestFile`] (bash parity).
pub fn file_stat_ok(st: &FileStat, allowed_uid: u32) -> Result<(), Reason> {
    let mode = st.st_mode as u32;
    if mode & libc::S_IFMT as u32 != libc::S_IFREG as u32 {
        return Err(Reason::InvalidRequestFile);
    }
    if st.st_uid != allowed_uid {
        return Err(Reason::InvalidRequestFile);
    }
    if st.st_nlink as u64 != 1 {
        return Err(Reason::InvalidRequestFile);
    }
    if mode & 0o022 != 0 {
        return Err(Reason::InvalidRequestFile);
    }
    Ok(())
}

/// PURE predicate over a `libc::stat` snapshot of a DIRECTORY fd.
///
/// Checks: `S_ISDIR`, `st_uid == expected_uid`, not group/other-writable.
/// Returns `true` iff all hold. Used for BOTH the per-poll request-dir
/// re-check (`expected_uid = allowed_uid`) and the startup root-dir checks
/// (`expected_uid = 0`).
pub fn dir_stat_ok(st: &FileStat, expected_uid: u32) -> bool {
    let mode = st.st_mode as u32;
    if mode & libc::S_IFMT as u32 != libc::S_IFDIR as u32 {
        return false;
    }
    if st.st_uid != expected_uid {
        return false;
    }
    if mode & 0o022 != 0 {
        return false;
    }
    true
}

/// `fstat` an open fd into a `FileStat`. Thin wrapper so callers never reach for
/// a path-based stat.
pub fn fstat_fd<Fd: AsFd>(fd: Fd) -> nix::Result<FileStat> {
    fstat(fd)
}

/// Open `name` RELATIVE to `dirfd` with `O_NOFOLLOW|O_RDONLY|O_CLOEXEC`.
///
/// A symlink AT the final component fails open(2) with `ELOOP` — that is the
/// symlink rejection. The caller then `fstat`s the returned fd and checks
/// [`file_stat_ok`]. The body is read from the SAME fd (closing TOCTOU).
pub fn open_nofollow_regular<Fd: AsFd>(dirfd: Fd, name: &str) -> nix::Result<OwnedFd> {
    openat(
        dirfd,
        name,
        OFlag::O_NOFOLLOW | OFlag::O_RDONLY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
}

/// Open an ABSOLUTE directory path with `O_NOFOLLOW|O_DIRECTORY|O_RDONLY|
/// O_CLOEXEC`. A symlinked dir fails (O_NOFOLLOW on the final component); a
/// non-dir fails (O_DIRECTORY). The caller `fstat`s + [`dir_stat_ok`]s the fd.
pub fn open_nofollow_dir(path: &str) -> nix::Result<OwnedFd> {
    open(
        path,
        OFlag::O_NOFOLLOW | OFlag::O_DIRECTORY | OFlag::O_RDONLY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
}

/// Open `name` RELATIVE to `dirfd` as a directory with the same flags as
/// [`open_nofollow_dir`]. Used for the processing dir (a fixed subdir of the
/// validated state dir).
pub fn open_nofollow_dir_at<Fd: AsFd>(dirfd: Fd, name: &str) -> nix::Result<OwnedFd> {
    openat(
        dirfd,
        name,
        OFlag::O_NOFOLLOW | OFlag::O_DIRECTORY | OFlag::O_RDONLY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_from_base_strips_and_validates() {
        assert_eq!(id_from_base("req.abc123"), Some("abc123".to_string()));
        assert_eq!(id_from_base("req.a.b-c_d"), Some("a.b-c_d".to_string()));
        // missing req. prefix
        assert_eq!(id_from_base("foo.abc"), None);
        // empty id
        assert_eq!(id_from_base("req."), None);
        // traversal: `.` and `..`
        assert_eq!(id_from_base("req.."), None); // id == "."
        assert_eq!(id_from_base("req..."), None); // id == ".."
        // bad charset
        assert_eq!(id_from_base("req.a/b"), None);
        assert_eq!(id_from_base("req.a b"), None);
        assert_eq!(id_from_base("req.a;b"), None);
    }

    #[test]
    fn id_charset_guard() {
        assert!(id_charset_ok("abc.123-x_y"));
        assert!(!id_charset_ok(""));
        assert!(!id_charset_ok("."));
        assert!(!id_charset_ok(".."));
        assert!(!id_charset_ok("a/b"));
        assert!(!id_charset_ok("a b"));
    }

    #[test]
    fn single_action_line_accepts_each_action() {
        assert_eq!(request_is_single_action(b"engage\n"), Ok(Action::Engage));
        assert_eq!(request_is_single_action(b"release\n"), Ok(Action::Release));
        assert_eq!(request_is_single_action(b"status\n"), Ok(Action::Status));
    }

    fn reject(reason: Reason, action_word: &str) -> Result<Action, RejectedRequest> {
        Err(RejectedRequest {
            reason,
            action_word: action_word.to_string(),
        })
    }

    #[test]
    fn single_action_rejects_extra_content() {
        // a second line — carries the VALID action word (bash parity).
        assert_eq!(
            request_is_single_action(b"engage\nextra\n"),
            reject(Reason::ExtraContent, "engage")
        );
        // trailing content without a final newline
        assert_eq!(
            request_is_single_action(b"release\nx"),
            reject(Reason::ExtraContent, "release")
        );
        // a trailing blank line is still extra content (matches bash: a second
        // read returning empty would not flag, BUT here the byte after \n is
        // another \n => non-empty rest). Keep strict.
        assert_eq!(
            request_is_single_action(b"status\n\n"),
            reject(Reason::ExtraContent, "status")
        );
    }

    #[test]
    fn single_action_rejects_unterminated_action_word() {
        // valid action word but no terminating newline => extra_content, carrying
        // the valid action word.
        assert_eq!(
            request_is_single_action(b"engage"),
            reject(Reason::ExtraContent, "engage")
        );
    }

    #[test]
    fn single_action_rejects_invalid_action() {
        // The bad first-line word is carried into the response action= field.
        assert_eq!(
            request_is_single_action(b"reboot\n"),
            reject(Reason::InvalidAction, "reboot")
        );
        // A blank/empty first line => action=empty (bash `${action:-empty}`).
        assert_eq!(
            request_is_single_action(b"\n"),
            reject(Reason::InvalidAction, "empty")
        );
        assert_eq!(
            request_is_single_action(b""),
            reject(Reason::InvalidAction, "empty")
        );
    }

    // file_stat_ok / dir_stat_ok over a hand-built libc::stat snapshot.
    fn blank_stat() -> FileStat {
        // SAFETY: zeroed libc::stat is a valid all-fields-zero snapshot we then
        // populate explicitly for the predicate under test.
        unsafe { std::mem::zeroed() }
    }

    #[test]
    fn file_stat_ok_accepts_regular_owned_unwritable() {
        let mut st = blank_stat();
        st.st_mode = (libc::S_IFREG | 0o600) as _;
        st.st_uid = 501;
        st.st_nlink = 1;
        assert_eq!(file_stat_ok(&st, 501), Ok(()));
    }

    #[test]
    fn file_stat_ok_rejects_non_regular() {
        let mut st = blank_stat();
        st.st_mode = (libc::S_IFDIR | 0o600) as _;
        st.st_uid = 501;
        st.st_nlink = 1;
        assert_eq!(file_stat_ok(&st, 501), Err(Reason::InvalidRequestFile));
    }

    #[test]
    fn file_stat_ok_rejects_wrong_owner() {
        let mut st = blank_stat();
        st.st_mode = (libc::S_IFREG | 0o600) as _;
        st.st_uid = 999;
        st.st_nlink = 1;
        assert_eq!(file_stat_ok(&st, 501), Err(Reason::InvalidRequestFile));
    }

    #[test]
    fn file_stat_ok_rejects_hardlink() {
        let mut st = blank_stat();
        st.st_mode = (libc::S_IFREG | 0o600) as _;
        st.st_uid = 501;
        st.st_nlink = 2;
        assert_eq!(file_stat_ok(&st, 501), Err(Reason::InvalidRequestFile));
    }

    #[test]
    fn file_stat_ok_rejects_group_or_other_writable() {
        for bad in [0o620u32, 0o602, 0o666, 0o060, 0o006] {
            let mut st = blank_stat();
            st.st_mode = (libc::S_IFREG as u32 | bad) as _;
            st.st_uid = 501;
            st.st_nlink = 1;
            assert_eq!(
                file_stat_ok(&st, 501),
                Err(Reason::InvalidRequestFile),
                "mode {bad:o} must reject"
            );
        }
    }

    #[test]
    fn dir_stat_ok_checks() {
        let mut st = blank_stat();
        st.st_mode = (libc::S_IFDIR | 0o700) as _;
        st.st_uid = 0;
        assert!(dir_stat_ok(&st, 0));
        // wrong owner
        assert!(!dir_stat_ok(&st, 501));
        // not a dir
        let mut f = blank_stat();
        f.st_mode = (libc::S_IFREG | 0o700) as _;
        f.st_uid = 0;
        assert!(!dir_stat_ok(&f, 0));
        // group writable
        let mut w = blank_stat();
        w.st_mode = (libc::S_IFDIR | 0o770) as _;
        w.st_uid = 0;
        assert!(!dir_stat_ok(&w, 0));
    }
}
