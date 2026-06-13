//! Red-team integration test — proves the test seam is COMPILE-TIME, not a
//! runtime env flag.
//!
//! This file is meaningful ONLY when built WITHOUT `helper-test-seam` (the
//! default `cargo test`, and meaningful in release profile too). It spawns the
//! shipped-shape `vigil-root-helper` binary with `VIGIL_ROOT_HELPER_TESTING=1`
//! in its environment and asserts it STILL refuses to run as non-root. Since the
//! test runner is non-root and the non-root bypass is compiled OUT (no feature),
//! the helper must exit non-zero and never touch power state.
//!
//! When built WITH the feature, the bypass IS compiled in, so this assertion
//! would not hold — therefore the whole file is `#[cfg(not(feature =
//! "helper-test-seam"))]`.

#![cfg(not(feature = "helper-test-seam"))]

use std::process::Command;

/// The shipped binary must refuse non-root EVEN with VIGIL_ROOT_HELPER_TESTING=1
/// in its env. The env var is never read by the binary; the seam is the
/// (absent) compile-time feature.
#[test]
fn red_team_release_profile_refuses_non_root() {
    // If the test runner happens to be root (e.g. CI as root), this test is not
    // meaningful — skip rather than falsely pass. SAFETY: geteuid is safe.
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        eprintln!("skipping red-team test: running as root");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let request = dir.path().join("requests");
    let response = dir.path().join("responses");
    let state = dir.path().join("state");
    let log = dir.path().join("logs/helper.log");
    std::fs::create_dir_all(&request).unwrap();
    std::fs::create_dir_all(&response).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_vigil-root-helper"))
        .args([
            "--once",
            "--request-dir",
            request.to_str().unwrap(),
            "--response-dir",
            response.to_str().unwrap(),
            "--state-dir",
            state.to_str().unwrap(),
            "--log-file",
            log.to_str().unwrap(),
            // A red-team would try to impersonate another user. The allowed-uid
            // is baked from argv here; the point is the ROOT refusal fires first.
            "--allowed-uid",
            "0",
            "--allowed-user",
            "root",
        ])
        // The runtime env var that USED to flip the bash seam. It must NOT flip
        // the Rust seam (compiled out).
        .env("VIGIL_ROOT_HELPER_TESTING", "1")
        .env("VIGIL_ROOT_HELPER_LIB_ONLY", "1")
        .env("VIGIL_TEST_NO_ADMIN", "1")
        .output()
        .expect("spawn vigil-root-helper");

    assert!(
        !output.status.success(),
        "shipped helper must refuse non-root even with VIGIL_ROOT_HELPER_TESTING=1; \
         exit status was {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must run as root"),
        "expected root-refusal message, got: {stderr}"
    );
}

/// The install-time-fixed --allowed-uid is taken from validated argv, never from
/// request content. We assert the binary REJECTS a non-numeric --allowed-uid at
/// parse time (a usage error), proving the uid is argv-derived and validated.
#[test]
fn allowed_uid_is_argv_validated_not_request_derived() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_vigil-root-helper"))
        .args([
            "--once",
            "--request-dir",
            dir.path().to_str().unwrap(),
            "--response-dir",
            dir.path().to_str().unwrap(),
            "--state-dir",
            dir.path().to_str().unwrap(),
            "--log-file",
            dir.path().join("x.log").to_str().unwrap(),
            "--allowed-uid",
            "not-a-number",
            "--allowed-user",
            "u",
        ])
        .env("VIGIL_ROOT_HELPER_TESTING", "1")
        .output()
        .expect("spawn vigil-root-helper");

    assert!(
        !output.status.success(),
        "non-numeric --allowed-uid must be a usage error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("allowed-uid"),
        "error must mention allowed-uid, got: {stderr}"
    );
}
