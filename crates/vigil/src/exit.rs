//! Exit-code discipline + the test-mode admin guard skeleton.
//!
//! EVERY future privileged path MUST call `admin_allowed()` before touching a
//! privileged resource (sudo, launchctl, root-owned files). This is the single
//! choke point the security model depends on; no later slice may add a
//! privileged path that bypasses it.

/// sysexits.h EX_USAGE — bad invocation (unknown command/subcommand/arg).
pub const EX_USAGE: i32 = 64;
/// Generic operational failure (mirrors bash `die` exiting 1).
pub const EX_ERROR: i32 = 1;

/// Env var that hard-disables every admin operation (test seam, parity with the
/// bash `cmd_require_admin_allowed`). When set to "1", admin paths must abort.
pub const ENV_TEST_NO_ADMIN: &str = "VIGIL_TEST_NO_ADMIN";

/// Returns `Ok(())` if admin operations are permitted, or an `Err(message)`
/// describing why they are blocked. Future admin paths call this and, on `Err`,
/// print the message to stderr and exit `EX_ERROR`.
///
/// In 5.1 there is NO privileged code; this is the skeleton all later admin
/// paths route through. It honors `VIGIL_TEST_NO_ADMIN=1` as a hard block.
pub fn admin_allowed() -> Result<(), String> {
    match std::env::var(ENV_TEST_NO_ADMIN) {
        Ok(v) if v == "1" => Err(format!("admin operation blocked by {ENV_TEST_NO_ADMIN}")),
        _ => Ok(()),
    }
}

/// Convenience used by future admin commands: enforce the guard or terminate the
/// process with a clear message and a non-zero exit code. Lands now so the
/// abort entrypoint is uniform from slice 1.
#[allow(dead_code)]
pub fn require_admin_allowed() {
    if let Err(msg) = admin_allowed() {
        anstream::eprintln!("{msg}");
        std::process::exit(EX_ERROR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize the env-mutating tests: cargo runs tests on multiple threads and
    // process-wide env mutation is not thread-safe in edition 2024.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn admin_blocked_when_env_set() {
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized via ENV_LOCK; set then clear immediately.
        unsafe { std::env::set_var(ENV_TEST_NO_ADMIN, "1") };
        let r = admin_allowed();
        unsafe { std::env::remove_var(ENV_TEST_NO_ADMIN) };
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("VIGIL_TEST_NO_ADMIN"));
    }

    #[test]
    fn admin_allowed_by_default() {
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized via ENV_LOCK; ensure the var is unset.
        unsafe { std::env::remove_var(ENV_TEST_NO_ADMIN) };
        assert!(admin_allowed().is_ok());
    }

    #[test]
    fn exit_constants() {
        assert_eq!(EX_USAGE, 64);
        assert_eq!(EX_ERROR, 1);
    }
}
