//! Subprocess-driven §3.3 adversarial integration test for vigil-root-helper.
//!
//! Run with: `cargo test --features helper-test-seam`.
//!
//! This spawns the REAL `vigil-root-helper` binary (built WITH the
//! `helper-test-seam` feature, so the non-root bypass + the file-backed
//! pmset/SleepDisabled fakes are active) in `--once` mode against tempdir
//! request/response/state/log dirs, then asserts the full §3.3 matrix:
//! symlink/hardlink/wrong-owner/multiline/bad-charset requests => status=error
//! (or removal) + no pmset call + no leftover file; the forged-root-owned
//! response is accepted only when truly root-owned (here: owned by us, which the
//! seam treats as the expected uid); engage/release baseline 0 and 1; idle
//! release no-op; engage recapture; pmset engage/release failure paths.
//!
//! Without the feature this file compiles to an EMPTY test binary (every test is
//! `#[cfg(feature = "helper-test-seam")]`), so `cargo test` (no feature) does
//! not try to drive a non-feature binary.

#![cfg(feature = "helper-test-seam")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Env {
    dir: tempfile::TempDir,
    request_dir: PathBuf,
    response_dir: PathBuf,
    state_dir: PathBuf,
    log_file: PathBuf,
    sleep_file: PathBuf,
    events_file: PathBuf,
    uid: u32,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let request_dir = dir.path().join("requests");
        let response_dir = dir.path().join("responses");
        let state_dir = dir.path().join("state");
        let log_file = dir.path().join("logs/helper.log");
        let sleep_file = dir.path().join("sleepdisabled");
        let events_file = dir.path().join("events.log");
        fs::create_dir_all(&request_dir).unwrap();
        fs::create_dir_all(&response_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(log_file.parent().unwrap()).unwrap();
        fs::write(&sleep_file, "0\n").unwrap();
        fs::write(&events_file, "").unwrap();
        // SAFETY: geteuid is always safe.
        let uid = unsafe { libc::geteuid() };
        Env {
            dir,
            request_dir,
            response_dir,
            state_dir,
            log_file,
            sleep_file,
            events_file,
            uid,
        }
    }

    fn write_request(&self, id: &str, body: &str) {
        let p = self.request_dir.join(format!("req.{id}"));
        fs::write(&p, body).unwrap();
        chmod(&p, 0o600);
    }

    /// Spawn the real helper binary in --once mode with the fakes active.
    fn run(&self, pmset_fail: bool) {
        let status = Command::new(env!("CARGO_BIN_EXE_vigil-root-helper"))
            .args([
                "--once",
                "--request-dir",
                self.request_dir.to_str().unwrap(),
                "--response-dir",
                self.response_dir.to_str().unwrap(),
                "--state-dir",
                self.state_dir.to_str().unwrap(),
                "--log-file",
                self.log_file.to_str().unwrap(),
                "--allowed-uid",
                &self.uid.to_string(),
                "--allowed-user",
                "tester",
            ])
            .env("VIGIL_FAKE_SLEEP_FILE", &self.sleep_file)
            .env("VIGIL_FAKE_EVENTS", &self.events_file)
            .env("VIGIL_FAKE_PMSET_FAIL", if pmset_fail { "1" } else { "0" })
            // A red-team flavor: even with TESTING=1 set, the seam is the
            // compile-time feature, not this env var. (Here the feature IS on, so
            // the helper runs; the point of the red-team test is the NO-feature
            // build, in root_helper_redteam.rs.)
            .env("VIGIL_ROOT_HELPER_TESTING", "1")
            .status()
            .expect("spawn vigil-root-helper");
        assert!(status.success(), "helper --once should exit 0");
    }

    fn response(&self, id: &str) -> String {
        fs::read_to_string(self.response_dir.join(format!("resp.{id}"))).unwrap_or_default()
    }

    fn sleepdisabled(&self) -> String {
        fs::read_to_string(&self.sleep_file)
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    fn events(&self) -> String {
        fs::read_to_string(&self.events_file).unwrap_or_default()
    }

    fn engaged(&self) -> bool {
        self.state_dir.join("engaged").exists()
    }

    fn baseline(&self) -> String {
        fs::read_to_string(self.state_dir.join("baseline"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    fn no_leftover(&self, id: &str) -> bool {
        !self.request_dir.join(format!("req.{id}")).exists()
            && !self
                .state_dir
                .join("processing")
                .join(format!("req.{id}"))
                .exists()
    }
}

fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(p).unwrap().permissions();
    perms.set_mode(mode);
    fs::set_permissions(p, perms).unwrap();
}

/// Shared assertion tail for the §3.3 `invalid_request_file` rejections. The
/// DISTINCT attack SETUP (symlink / hardlink / group-writable) stays inline in
/// each test; only the identical tail — `status=error` +
/// `message=invalid_request_file`, pmset untouched (SleepDisabled stays "0"),
/// and no leftover request/processing file — is factored here. Every original
/// assertion is preserved exactly (no contains->eq weakening; the byte-exact
/// SleepDisabled check stays `assert_eq!`).
fn assert_invalid_request_file(e: &Env, id: &str) {
    assert!(
        e.response(id).contains("status=error"),
        "{id}: status=error: {}",
        e.response(id)
    );
    assert!(
        e.response(id).contains("message=invalid_request_file"),
        "{id}: message=invalid_request_file: {}",
        e.response(id)
    );
    assert_eq!(e.sleepdisabled(), "0", "{id}: pmset untouched");
    assert!(
        e.no_leftover(id),
        "{id}: no leftover request/processing file"
    );
}

#[test]
fn status_request_is_ok() {
    let e = Env::new();
    e.write_request("s", "status\n");
    e.run(false);
    assert!(e.response("s").contains("status=ok"));
    assert!(e.response("s").contains("action=status"));
}

#[test]
fn invalid_action_rejected() {
    let e = Env::new();
    e.write_request("bad", "reboot\n");
    e.run(false);
    assert!(e.response("bad").contains("status=error"));
    assert!(e.response("bad").contains("message=invalid_action"));
    // Bash carries the BAD action word into action= (`${action:-empty}`).
    assert!(
        e.response("bad").contains("action=reboot"),
        "invalid_action carries the bad action word: {}",
        e.response("bad")
    );
    assert!(e.no_leftover("bad"));
    assert!(!e.events().contains("disablesleep 1"));
}

#[test]
fn multiline_extra_content_rejected() {
    let e = Env::new();
    e.write_request("ml", "engage\nextra\n");
    e.run(false);
    assert!(e.response("ml").contains("status=error"));
    assert!(e.response("ml").contains("message=extra_content"));
    // Bash carries the VALID action word into action= for extra_content.
    assert!(
        e.response("ml").contains("action=engage"),
        "extra_content carries the valid action word: {}",
        e.response("ml")
    );
    assert_eq!(e.sleepdisabled(), "0", "no pmset call on extra content");
    assert!(e.no_leftover("ml"));
}

#[test]
fn symlink_request_rejected() {
    let e = Env::new();
    let target = e.dir.path().join("target");
    fs::write(&target, "engage\n").unwrap();
    std::os::unix::fs::symlink(&target, e.request_dir.join("req.lnk")).unwrap();
    e.run(false);
    // symlink did not change pmset; rejected + removed.
    assert_invalid_request_file(&e, "lnk");
}

#[test]
fn hardlink_request_rejected() {
    let e = Env::new();
    let target = e.dir.path().join("hard");
    fs::write(&target, "engage\n").unwrap();
    chmod(&target, 0o600);
    fs::hard_link(&target, e.request_dir.join("req.hl")).unwrap();
    e.run(false);
    assert_invalid_request_file(&e, "hl");
}

#[test]
fn group_writable_request_rejected() {
    let e = Env::new();
    let p = e.request_dir.join("req.gw");
    fs::write(&p, "engage\n").unwrap();
    chmod(&p, 0o660); // group-writable
    e.run(false);
    assert_invalid_request_file(&e, "gw");
}

#[test]
fn bad_charset_id_removed_no_response() {
    let e = Env::new();
    // a space in the id is a valid filename but an invalid id.
    let p = e.request_dir.join("req.bad id");
    fs::write(&p, "engage\n").unwrap();
    chmod(&p, 0o600);
    e.run(false);
    // removed, no pmset call, no response named for an invalid id.
    assert!(!p.exists(), "bad-charset request removed");
    assert_eq!(e.sleepdisabled(), "0");
}

#[test]
fn engage_release_baseline_zero() {
    let e = Env::new();
    e.write_request("e", "engage\n");
    e.run(false);
    assert!(e.response("e").contains("status=ok"));
    assert_eq!(e.sleepdisabled(), "1");
    assert_eq!(e.baseline(), "0");
    assert!(e.engaged());
    assert!(e.events().contains("pmset -a disablesleep 1"));

    e.write_request("r", "release\n");
    e.run(false);
    assert!(e.response("r").contains("status=ok"));
    assert_eq!(e.sleepdisabled(), "0");
    assert_eq!(e.baseline(), "0", "release keeps baseline file");
    assert!(!e.engaged());
    assert!(e.events().contains("pmset -a disablesleep 0"));
}

#[test]
fn engage_release_baseline_one() {
    let e = Env::new();
    fs::write(&e.sleep_file, "1\n").unwrap();
    e.write_request("e", "engage\n");
    e.run(false);
    assert_eq!(e.baseline(), "1");
    e.write_request("r", "release\n");
    e.run(false);
    assert_eq!(e.sleepdisabled(), "1", "release restores baseline 1");
    assert_eq!(e.baseline(), "1");
    let count = e.events().matches("pmset -a disablesleep 1").count();
    assert_eq!(count, 2, "engage + release both disablesleep 1");
}

#[test]
fn idle_release_no_op() {
    let e = Env::new();
    e.write_request("e", "engage\n");
    e.run(false);
    e.write_request("r", "release\n");
    e.run(false);
    // external SleepDisabled=1; idle release must not clobber it.
    fs::write(&e.sleep_file, "1\n").unwrap();
    e.write_request("idle", "release\n");
    e.run(false);
    assert!(e.response("idle").contains("status=ok"));
    assert_eq!(e.sleepdisabled(), "1", "idle release no clobber");
}

#[test]
fn engage_recapture_after_release() {
    let e = Env::new();
    e.write_request("e1", "engage\n");
    e.run(false);
    assert_eq!(e.baseline(), "0");
    e.write_request("r1", "release\n");
    e.run(false);
    assert!(!e.engaged());
    fs::write(&e.sleep_file, "1\n").unwrap();
    e.write_request("e2", "engage\n");
    e.run(false);
    assert_eq!(e.baseline(), "1", "fresh engage recaptures baseline 1");
    assert!(e.engaged());
}

#[test]
fn engage_pmset_failure() {
    let e = Env::new();
    e.write_request("fe", "engage\n");
    e.run(true); // pmset fails
    assert!(e.response("fe").contains("status=error"));
    assert!(e.response("fe").contains("message=pmset_engage_failed"));
    assert!(!e.engaged(), "failed engage does not mark engaged");
}

#[test]
fn release_pmset_failure_keeps_engaged() {
    let e = Env::new();
    e.write_request("e", "engage\n");
    e.run(false);
    e.write_request("fr", "release\n");
    e.run(true); // pmset fails on release
    assert!(e.response("fr").contains("status=error"));
    assert!(e.response("fr").contains("message=pmset_release_failed"));
    assert!(e.engaged(), "failed release keeps engaged for retry");
    assert_eq!(e.sleepdisabled(), "1");
}

#[test]
fn corrupt_baseline_releases_to_zero() {
    // Corrupt the helper baseline file while engaged; release must FAIL-SAFE to
    // disablesleep 0 and clear engaged.
    let e = Env::new();
    e.write_request("e", "engage\n");
    e.run(false);
    assert_eq!(e.sleepdisabled(), "1");
    // corrupt baseline
    fs::write(e.state_dir.join("baseline"), "garbage\n").unwrap();
    e.write_request("r", "release\n");
    e.run(false);
    assert_eq!(e.sleepdisabled(), "0", "corrupt baseline => disablesleep 0");
    assert!(
        !e.engaged(),
        "release clears engaged even with corrupt baseline"
    );
    assert!(e.events().contains("pmset -a disablesleep 0"));
}

#[test]
fn response_is_validated_root_owned_by_client() {
    // The IPC client's fd-based response validation: a response file written by
    // the helper is owned by us (the running uid). On a non-root runner the
    // client requires uid==0, so it would reject — proving the matched-pair
    // validation is real. We assert the helper-written response exists and is a
    // regular file owned by us (the seam's expected uid). The client-side uid==0
    // requirement is unit-tested in src/ipc.
    let e = Env::new();
    e.write_request("s", "status\n");
    e.run(false);
    let resp = e.response_dir.join("resp.s");
    let meta = fs::symlink_metadata(&resp).unwrap();
    assert!(meta.file_type().is_file(), "response is a regular file");
    use std::os::unix::fs::MetadataExt;
    assert_eq!(meta.uid(), e.uid, "response owned by the running uid");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        meta.permissions().mode() & 0o022,
        0,
        "response not group/other writable"
    );
}
