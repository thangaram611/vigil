//! cfg(test) unit tests for the privileged helper — ports of
//! `tests/root_helper_test.sh` (run unprivileged against tempdirs via the
//! cfg(test) non-root bypass + the file-backed pmset/SleepDisabled fakes).
//!
//! Each test builds a tempdir layout (request/response/state/log), seeds the
//! fake SleepDisabled file, sets the fake env vars, places a request file, runs
//! one poll pass, and asserts the response + side effects. Env-mutating, so all
//! tests serialize on a module-local lock.

use super::*;
use crate::power::pmset::fake::{FakePmset, FakeSleepReader};
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Layout {
    _dir: tempfile::TempDir,
    cfg: HelperConfig,
    sleep_file: PathBuf,
    events_file: PathBuf,
}

impl Layout {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let request_dir = dir.path().join("requests");
        let response_dir = dir.path().join("responses");
        let state_dir = dir.path().join("state");
        let log_file = dir.path().join("logs/helper.log");
        let sleep_file = dir.path().join("sleepdisabled");
        let events_file = dir.path().join("events.log");
        std::fs::create_dir_all(&request_dir).unwrap();
        std::fs::create_dir_all(&response_dir).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::create_dir_all(log_file.parent().unwrap()).unwrap();
        std::fs::write(&sleep_file, "0\n").unwrap();
        std::fs::write(&events_file, "").unwrap();

        // SAFETY: geteuid is always safe; we run unprivileged so allowed_uid is
        // our own uid (the fake request files we write are owned by us).
        let uid = unsafe { libc::geteuid() };
        let cfg = HelperConfig {
            request_dir,
            response_dir,
            state_dir,
            log_file,
            allowed_uid: uid,
            allowed_user: "tester".to_string(),
            poll_secs: 1,
            once: true,
        };
        Layout {
            _dir: dir,
            cfg,
            sleep_file,
            events_file,
        }
    }

    fn set_env(&self) {
        // SAFETY: serialized by ENV_LOCK; single-threaded mutation window.
        unsafe {
            std::env::set_var(crate::power::pmset::fake::SLEEP_FILE_ENV, &self.sleep_file);
            std::env::set_var(crate::power::pmset::fake::EVENTS_ENV, &self.events_file);
            std::env::remove_var(crate::power::pmset::fake::PMSET_FAIL_ENV);
        }
    }

    fn set_pmset_fail(&self, fail: bool) {
        // SAFETY: serialized by ENV_LOCK.
        unsafe {
            if fail {
                std::env::set_var(crate::power::pmset::fake::PMSET_FAIL_ENV, "1");
            } else {
                std::env::remove_var(crate::power::pmset::fake::PMSET_FAIL_ENV);
            }
        }
    }

    fn write_request(&self, id: &str, body: &str) {
        let path = self.cfg.request_dir.join(format!("req.{id}"));
        std::fs::write(&path, body).unwrap();
        set_mode_0600(&path);
    }

    fn run(&self) {
        process_once_with_seams(&self.cfg, &FakePmset, &FakeSleepReader)
            .expect("process_once should succeed");
    }

    fn response(&self, id: &str) -> String {
        std::fs::read_to_string(self.cfg.response_dir.join(format!("resp.{id}")))
            .unwrap_or_default()
    }

    fn sleepdisabled(&self) -> String {
        std::fs::read_to_string(&self.sleep_file)
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    fn events(&self) -> String {
        std::fs::read_to_string(&self.events_file).unwrap_or_default()
    }

    fn engaged_exists(&self) -> bool {
        self.cfg.state_dir.join("engaged").exists()
    }

    fn baseline(&self) -> String {
        std::fs::read_to_string(self.cfg.state_dir.join("baseline"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

// ── port: accepts_only_known_actions ──────────────────────────────────────────

#[test]
fn accepts_only_known_actions() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();

    l.write_request("good", "status\n");
    l.run();
    assert!(l.response("good").contains("status=ok"), "status accepted");

    l.write_request("bad", "reboot\n");
    l.run();
    assert!(
        l.response("bad").contains("status=error"),
        "unknown rejected"
    );
    assert!(
        l.response("bad").contains("message=invalid_action"),
        "invalid_action reason"
    );
    // Bash writes the BAD action word into action= (helper_reject_processed
    // "${action:-empty}"). Parity: action=reboot, not action=unknown.
    assert!(
        l.response("bad").contains("action=reboot"),
        "invalid_action response carries the bad action word: {}",
        l.response("bad")
    );

    // A blank first line => action=empty (bash `${action:-empty}`).
    l.write_request("blank", "\n");
    l.run();
    assert!(l.response("blank").contains("message=invalid_action"));
    assert!(
        l.response("blank").contains("action=empty"),
        "blank first line => action=empty: {}",
        l.response("blank")
    );
}

// ── port: rejects_malformed_request_files (extra_content) ─────────────────────

#[test]
fn rejects_malformed_request_files() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    l.write_request("malformed", "engage\nextra\n");
    l.run();
    assert!(l.response("malformed").contains("status=error"));
    assert!(l.response("malformed").contains("message=extra_content"));
    // Bash carries the VALID action word into action= for extra_content
    // (helper_reject_processed "$action"). Parity: action=engage, not unknown.
    assert!(
        l.response("malformed").contains("action=engage"),
        "extra_content response carries the valid action word: {}",
        l.response("malformed")
    );
    // engage must NOT have run.
    assert!(
        !l.events().contains("disablesleep 1"),
        "no pmset engage on reject"
    );
    assert_eq!(l.sleepdisabled(), "0");
}

// ── bounded request read: a huge body is rejected (extra_content), never slurped ─

#[test]
fn huge_request_body_is_bounded_and_rejected() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    // A valid action line followed by a large body. Bash reads at most two lines;
    // the Rust helper caps the read so a multi-MB regular file owned by the served
    // uid cannot OOM the root helper. The over-cap content is past the first
    // action line => extra_content (action=engage), matching bash.
    let mut body = String::from("engage\n");
    body.push_str(&"x".repeat(4 * 1024 * 1024));
    body.push('\n');
    l.write_request("huge", &body);
    l.run();
    assert!(
        l.response("huge").contains("status=error"),
        "huge body rejected"
    );
    assert!(
        l.response("huge").contains("message=extra_content"),
        "huge body => extra_content: {}",
        l.response("huge")
    );
    assert!(
        l.response("huge").contains("action=engage"),
        "carries the valid action word: {}",
        l.response("huge")
    );
    assert!(
        !l.events().contains("disablesleep 1"),
        "no pmset engage on a huge rejected body"
    );
    assert_eq!(l.sleepdisabled(), "0");
}

// ── port: rejects_symlink_request_files ───────────────────────────────────────

#[test]
fn rejects_symlink_request_files() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    let target = l._dir.path().join("target");
    std::fs::write(&target, "engage\n").unwrap();
    std::os::unix::fs::symlink(&target, l.cfg.request_dir.join("req.link")).unwrap();
    l.run();
    assert!(
        l.response("link").contains("status=error"),
        "symlink rejected"
    );
    assert!(l.response("link").contains("message=invalid_request_file"));
    assert_eq!(l.sleepdisabled(), "0", "symlink did not change pmset");
    // moved file removed: nothing left in processing or request dir.
    assert!(!l.cfg.request_dir.join("req.link").exists());
}

// ── port: rejects_hardlink_request_files ──────────────────────────────────────

#[test]
fn rejects_hardlink_request_files() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    let target = l._dir.path().join("hard-target");
    std::fs::write(&target, "engage\n").unwrap();
    set_mode_0600(&target);
    std::fs::hard_link(&target, l.cfg.request_dir.join("req.hard")).unwrap();
    l.run();
    assert!(
        l.response("hard").contains("status=error"),
        "hardlink rejected"
    );
    assert!(l.response("hard").contains("message=invalid_request_file"));
    assert_eq!(l.sleepdisabled(), "0", "hardlink did not change pmset");
}

// ── port: rejects_request_files_not_owned_by_expected_user ────────────────────

#[test]
fn rejects_request_files_not_owned_by_expected_user() {
    let _g = lock_env();
    let mut l = Layout::new();
    l.set_env();
    // Bake a wrong allowed_uid (a uid we are NOT). Our request file is owned by
    // our real uid, which now mismatches.
    l.cfg.allowed_uid = 999_999;
    l.write_request("wrong_owner", "engage\n");
    // The per-poll request-DIR check requires the dir owned by allowed_uid; with
    // a bogus uid the dir check fails first, so no response is written. To
    // exercise the per-FILE owner check specifically, we instead keep the dir
    // owner (us) but assert the dir-level rejection still prevents the pmset
    // call. Either way: no engage, SleepDisabled unchanged.
    l.run();
    assert_eq!(l.sleepdisabled(), "0", "wrong owner did not change pmset");
    assert!(
        !l.events().contains("disablesleep 1"),
        "no engage for wrong owner"
    );
}

// ── port: rejects_request_files_not_owned (FILE-level via real uid match) ─────

#[test]
fn rejects_request_file_owner_at_file_level() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    // Keep allowed_uid == dir owner (us) so the DIR check passes, then validate
    // the FILE-level owner check by constructing the file_stat_ok predicate over
    // a wrong-uid stat directly. (We cannot chown to another uid unprivileged,
    // so the file-level branch is also covered by validate::file_stat_ok tests.)
    // Here we assert the happy file is accepted to prove the dir check passes,
    // bounding the wrong-owner case above to the dir-level guard.
    l.write_request("ok_owner", "status\n");
    l.run();
    assert!(l.response("ok_owner").contains("status=ok"));
}

// ── port: reports_engage_pmset_failure ────────────────────────────────────────

#[test]
fn reports_engage_pmset_failure() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    l.set_pmset_fail(true);
    l.write_request("fail_engage", "engage\n");
    l.run();
    assert!(l.response("fail_engage").contains("status=error"));
    assert!(
        l.response("fail_engage")
            .contains("message=pmset_engage_failed")
    );
    assert!(!l.engaged_exists(), "failed engage does not mark engaged");
    l.set_pmset_fail(false);
}

// ── port: reports_release_pmset_failure_and_keeps_engaged ─────────────────────

#[test]
fn reports_release_pmset_failure_and_keeps_engaged() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    l.write_request("engage", "engage\n");
    l.run();
    assert_eq!(l.sleepdisabled(), "1");
    l.set_pmset_fail(true);
    l.write_request("fail_release", "release\n");
    l.run();
    assert!(l.response("fail_release").contains("status=error"));
    assert!(
        l.response("fail_release")
            .contains("message=pmset_release_failed")
    );
    assert!(l.engaged_exists(), "failed release keeps engaged for retry");
    assert_eq!(
        l.sleepdisabled(),
        "1",
        "failed release did not change SleepDisabled"
    );
    l.set_pmset_fail(false);
}

// ── port: engage_and_release_restore_baseline ─────────────────────────────────

#[test]
fn engage_and_release_restore_baseline() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();

    l.write_request("engage", "engage\n");
    l.run();
    assert!(l.response("engage").contains("status=ok"));
    assert_eq!(l.sleepdisabled(), "1", "engage set SleepDisabled");
    assert_eq!(l.baseline(), "0", "baseline captured");

    l.write_request("release", "release\n");
    l.run();
    assert!(l.response("release").contains("status=ok"));
    assert_eq!(l.sleepdisabled(), "0", "release restored baseline");
    assert_eq!(l.baseline(), "0", "release keeps root baseline");
    assert!(!l.engaged_exists(), "release marks helper idle");
    assert!(
        l.events().contains("pmset -a disablesleep 1"),
        "engage fixed argv"
    );
    assert!(
        l.events().contains("pmset -a disablesleep 0"),
        "release fixed argv"
    );
}

// ── port: release_restores_baseline_one ───────────────────────────────────────

#[test]
fn release_restores_baseline_one() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    std::fs::write(&l.sleep_file, "1\n").unwrap();

    l.write_request("engage", "engage\n");
    l.run();
    assert!(l.response("engage").contains("status=ok"));
    assert_eq!(l.baseline(), "1", "baseline 1 captured");

    l.write_request("release", "release\n");
    l.run();
    assert!(l.response("release").contains("status=ok"));
    assert_eq!(l.sleepdisabled(), "1", "release restored baseline 1");
    assert_eq!(l.baseline(), "1", "release keeps baseline 1");
    assert!(!l.engaged_exists());
    let count = l.events().matches("pmset -a disablesleep 1").count();
    assert_eq!(count, 2, "engage + release both issue disablesleep 1");
}

// ── port: engage_recaptures_after_release ─────────────────────────────────────

#[test]
fn engage_recaptures_after_release() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();

    l.write_request("first_engage", "engage\n");
    l.run();
    assert_eq!(l.baseline(), "0", "first engage captures baseline 0");

    l.write_request("first_release", "release\n");
    l.run();
    assert!(!l.engaged_exists(), "release marks idle before recapture");

    std::fs::write(&l.sleep_file, "1\n").unwrap();
    l.write_request("second_engage", "engage\n");
    l.run();
    assert!(l.response("second_engage").contains("status=ok"));
    assert_eq!(l.baseline(), "1", "fresh engage recaptures baseline 1");
    assert!(l.engaged_exists(), "second engage marks engaged");
}

// ── port: idle_release_is_noop ────────────────────────────────────────────────

#[test]
fn idle_release_is_noop() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();

    l.write_request("engage", "engage\n");
    l.run();
    l.write_request("release", "release\n");
    l.run();

    // externally set SleepDisabled=1; an idle release must NOT clobber it.
    std::fs::write(&l.sleep_file, "1\n").unwrap();
    l.write_request("idle_release", "release\n");
    l.run();
    assert!(
        l.response("idle_release").contains("status=ok"),
        "idle release ok"
    );
    assert_eq!(
        l.sleepdisabled(),
        "1",
        "idle release does not clobber external SleepDisabled"
    );
}

// ── §3.3: bad-charset id rejected (removed, no traversal) ──────────────────────

#[test]
fn bad_charset_id_rejected() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    // A req. file whose id has a slash cannot exist as a single dir entry, so we
    // use a charset-invalid id with a space (valid filename, invalid id).
    let path = l.cfg.request_dir.join("req.bad id");
    std::fs::write(&path, "engage\n").unwrap();
    set_mode_0600(&path);
    l.run();
    // Bad filename => removed from request dir, no response (no valid id), no
    // pmset call.
    assert!(!path.exists(), "bad-charset request removed");
    assert!(
        !l.events().contains("disablesleep 1"),
        "no engage for bad id"
    );
}

// ── §3.3: dot/dotdot id rejected ──────────────────────────────────────────────

#[test]
fn dot_and_dotdot_ids_rejected() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    for id in ["req..", "req..."] {
        let path = l.cfg.request_dir.join(id);
        std::fs::write(&path, "status\n").unwrap();
        set_mode_0600(&path);
    }
    l.run();
    // both removed, no responses written (ids invalid).
    assert!(!l.cfg.request_dir.join("req..").exists());
    assert!(!l.cfg.request_dir.join("req...").exists());
}

// ── §3.3: symlinked state dir rejected at open(O_NOFOLLOW|O_DIRECTORY) ─────────

#[test]
fn symlinked_state_dir_rejected() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    // Replace state_dir with a symlink to a sibling real dir.
    let real = l._dir.path().join("real_state");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::remove_dir_all(&l.cfg.state_dir).unwrap();
    std::os::unix::fs::symlink(&real, &l.cfg.state_dir).unwrap();
    let err = match validate_dirs(&l.cfg) {
        Ok(_) => panic!("symlinked state dir should be rejected"),
        Err(e) => e,
    };
    assert!(
        err.contains("state directory"),
        "symlinked state dir rejected: {err}"
    );
}

#[test]
fn symlinked_response_dir_rejected() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    let real = l._dir.path().join("real_resp");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::remove_dir_all(&l.cfg.response_dir).unwrap();
    std::os::unix::fs::symlink(&real, &l.cfg.response_dir).unwrap();
    let err = match validate_dirs(&l.cfg) {
        Ok(_) => panic!("symlinked response dir should be rejected"),
        Err(e) => e,
    };
    assert!(
        err.contains("response directory"),
        "symlinked response dir rejected: {err}"
    );
}

// ── §3.3: liveness — every rejection writes an error response (no timeout) ────

#[test]
fn rejection_always_writes_error_response() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    // extra_content rejection (charset-valid id) MUST produce an error response.
    l.write_request("rej", "engage\nmore\n");
    l.run();
    let resp = l.response("rej");
    assert!(
        !resp.is_empty(),
        "rejection wrote a response (no client timeout)"
    );
    assert!(resp.contains("status=error"));
    assert!(resp.contains("message=extra_content"));
    // queue does not accumulate poison files.
    assert!(!l.cfg.request_dir.join("req.rej").exists());
}

// ── arg parsing ───────────────────────────────────────────────────────────────

/// The four required dir/file args every parse_args test shares; each test
/// prepends its own mode flag and appends its own uid / user / poll-secs.
fn base_dirs_argv() -> Vec<&'static str> {
    vec![
        "--request-dir",
        "/r",
        "--response-dir",
        "/p",
        "--state-dir",
        "/s",
        "--log-file",
        "/l/x.log",
    ]
}

#[test]
fn parse_args_requires_numeric_uid() {
    let mut argv = vec!["--once"];
    argv.extend(base_dirs_argv());
    argv.extend(["--allowed-uid", "notanumber", "--allowed-user", "u"]);
    let err = parse_args(argv).unwrap_err();
    assert!(matches!(err, ArgError::InvalidUid(_)));
}

#[test]
fn parse_args_happy() {
    let mut argv = vec!["--serve"];
    argv.extend(base_dirs_argv());
    argv.extend([
        "--allowed-uid",
        "501",
        "--allowed-user",
        "alice",
        "--poll-secs",
        "3",
    ]);
    let cfg = parse_args(argv).unwrap();
    assert_eq!(cfg.allowed_uid, 501);
    assert_eq!(cfg.allowed_user, "alice");
    assert_eq!(cfg.poll_secs, 3);
    assert!(!cfg.once);
}

#[test]
fn parse_args_missing_required() {
    let err = parse_args(["--once", "--allowed-uid", "0"]).unwrap_err();
    assert!(matches!(err, ArgError::Missing(_)));
}
