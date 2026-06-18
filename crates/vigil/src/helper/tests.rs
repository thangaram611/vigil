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

// ── dir-owner check passes => well-formed request yields status=ok ───────────

#[test]
fn owned_request_dir_accepts_request() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    // allowed_uid == dir owner (us), so the per-poll request-DIR owner check
    // passes and a well-formed request is accepted (status=ok). This bounds the
    // wrong-owner case in rejects_request_files_not_owned_by_expected_user to the
    // dir-level guard. The FILE-level owner branch (a wrong st_uid) cannot be
    // exercised unprivileged — we cannot chown to another uid — so it is covered
    // by the pure validate::file_stat_ok_rejects_wrong_owner predicate test.
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

// ── §3.3: symlinked root dir rejected at open(O_NOFOLLOW|O_DIRECTORY) ──────────

#[test]
fn symlinked_root_dir_rejected() {
    let _g = lock_env();
    // Each row replaces ONE validated root dir with a symlink to a sibling real
    // dir and asserts validate_dirs rejects it with that dir's error substring.
    // O_NOFOLLOW|O_DIRECTORY on the final component fails the open. A fresh
    // Layout per row keeps each row isolated (validate_dirs checks response
    // before state, so a stale symlink from a prior row would mask the row under
    // test); BOTH original error substrings stay asserted, one per row.
    struct Row {
        label: &'static str,
        real_name: &'static str,
        // returns the validated root dir to replace with a symlink.
        target: fn(&Layout) -> std::path::PathBuf,
        panic_msg: &'static str,
        substr: &'static str,
    }
    let cases: &[Row] = &[
        Row {
            label: "state",
            real_name: "real_state",
            target: |l| l.cfg.state_dir.clone(),
            panic_msg: "symlinked state dir should be rejected",
            substr: "state directory",
        },
        Row {
            label: "response",
            real_name: "real_resp",
            target: |l| l.cfg.response_dir.clone(),
            panic_msg: "symlinked response dir should be rejected",
            substr: "response directory",
        },
    ];
    for row in cases {
        let l = Layout::new();
        l.set_env();
        let dir = (row.target)(&l);
        let real = l._dir.path().join(row.real_name);
        std::fs::create_dir_all(&real).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&real, &dir).unwrap();
        let err = match validate_dirs(&l.cfg) {
            Ok(_) => panic!("{}", row.panic_msg),
            Err(e) => e,
        };
        assert!(
            err.contains(row.substr),
            "symlinked {} dir rejected: {err}",
            row.label
        );
    }
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

// ── cold-start idle release: a release with NO prior engage is a no-op ─────────

#[test]
fn cold_start_idle_release_is_noop() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();

    // Never engaged. Externally set SleepDisabled=1; a COLD release (no engage
    // ever ran) must report ok WITHOUT clobbering SleepDisabled and WITHOUT a
    // pmset transition.
    std::fs::write(&l.sleep_file, "1\n").unwrap();
    l.write_request("cold_release", "release\n");
    l.run();

    let resp = l.response("cold_release");
    assert!(resp.contains("status=ok"), "cold idle release ok: {resp}");
    assert!(
        resp.contains("action=release"),
        "echoes the release action: {resp}"
    );
    assert_eq!(
        l.sleepdisabled(),
        "1",
        "cold idle release does not clobber external SleepDisabled"
    );
    assert!(!l.engaged_exists(), "never engaged => no engaged marker");
    // No pmset transition issued by a cold release.
    assert!(
        !l.events().contains("disablesleep"),
        "cold idle release issues no pmset call: {}",
        l.events()
    );
}

// ── §4.10 dir-mode convergence: response/log stay daemon-traversable ──────────

#[test]
fn validate_dirs_self_heals_response_to_0755_and_keeps_state_0700() {
    use std::os::unix::fs::PermissionsExt;
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();

    // Reproduce the live failure an OLDER helper produced: response (and log) dir
    // tightened to 0700 (daemon can no longer traverse → every round-trip times
    // out), and the state dir loosened to 0755. validate_dirs runs on EVERY poll
    // (via process_once_with_seams), so it must CONVERGE each dir to setup's
    // §4.10 mode — repairing the response/log dirs back to a daemon-traversable
    // 0755 and keeping state root-only 0700.
    let log_dir = l.cfg.log_file.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&l.cfg.response_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&log_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&l.cfg.state_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    l.run();

    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode(&l.cfg.response_dir),
        0o755,
        "response dir self-heals to daemon-traversable 0755"
    );
    assert_eq!(mode(&log_dir), 0o755, "log dir converges to 0755");
    assert_eq!(
        mode(&l.cfg.state_dir),
        0o700,
        "state dir converges to root-only 0700"
    );
    assert_eq!(
        mode(&l.cfg.state_dir.join(PROCESSING_DIR)),
        0o700,
        "processing subdir stays 0700"
    );
}

// ── stale response GC: root reclaims abandoned/consumed resp.* files ──────────

#[test]
fn stale_response_sweep_removes_old_keeps_fresh_and_spares_non_resp() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    let dirs = validate_dirs(&l.cfg).expect("dirs validate");

    // The response dir is root-owned, so the non-root daemon can NEVER unlink its
    // own consumed responses — the helper is the only reclaimer. Plant a finished
    // resp, an in-flight temp, and a non-resp control file.
    std::fs::write(l.cfg.response_dir.join("resp.fresh"), "status=ok\n").unwrap();
    std::fs::write(l.cfg.response_dir.join(".resp.tmp.123"), "x").unwrap();
    std::fs::write(l.cfg.response_dir.join("keepme"), "x").unwrap();

    // age>=0 sweeps everything resp-shaped; the control file is spared.
    cleanup_stale_responses(&dirs, &l.cfg, 0);
    assert!(
        !l.cfg.response_dir.join("resp.fresh").exists(),
        "resp.* reclaimed at age>=0"
    );
    assert!(
        !l.cfg.response_dir.join(".resp.tmp.123").exists(),
        ".resp.* temp reclaimed at age>=0"
    );
    assert!(
        l.cfg.response_dir.join("keepme").exists(),
        "a non-resp file is NEVER swept"
    );

    // With the production floor, a just-written resp SURVIVES — never reclaimed
    // out from under a client still polling for it.
    std::fs::write(l.cfg.response_dir.join("resp.live"), "status=ok\n").unwrap();
    cleanup_stale_responses(&dirs, &l.cfg, STALE_RESPONSE_SECS);
    assert!(
        l.cfg.response_dir.join("resp.live").exists(),
        "fresh resp survives the {STALE_RESPONSE_SECS}s sweep"
    );
}

// ── status response echoes action=status (and message=ok) ─────────────────────

#[test]
fn status_response_echoes_action_status() {
    let _g = lock_env();
    let l = Layout::new();
    l.set_env();
    l.write_request("st", "status\n");
    l.run();
    let resp = l.response("st");
    assert!(resp.contains("status=ok"), "status => ok: {resp}");
    assert!(
        resp.contains("action=status"),
        "status response echoes action=status: {resp}"
    );
    assert!(
        resp.contains("message=ok"),
        "status carries message=ok: {resp}"
    );
    // a pure status query never engages pmset.
    assert!(
        !l.events().contains("disablesleep 1"),
        "status does not engage: {}",
        l.events()
    );
    assert_eq!(l.sleepdisabled(), "0");
}
