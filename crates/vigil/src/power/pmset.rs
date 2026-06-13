//! pmset / SleepDisabled side-effect seams — Phase 5.5.
//!
//! Used by BOTH the helper (root side: it actually mutates SleepDisabled) and
//! the read-only power view (client side: read SleepDisabled only).
//!
//! ## Privilege boundary (unchanged from bash)
//! [`MacPmset::set`] runs `/usr/bin/pmset` with a FIXED argv per action
//! (`-a disablesleep 0` or `-a disablesleep 1`) via `std::process::Command`,
//! with `.env_clear()` + a pinned `PATH=/usr/bin:/bin:/usr/sbin:/sbin`. The argv
//! is NEVER built from request content. We do NOT use the
//! `IOPMSetSystemPowerSetting` SPI.
//!
//! ## Test seams are COMPILE-TIME only
//! The fakes ([`FakePmset`] / [`FakeSleepReader`]) live behind
//! `cfg(any(test, feature = "helper-test-seam"))`. They are compiled OUT of the
//! shipped root binary (no feature). The red-team test proves
//! `VIGIL_ROOT_HELPER_TESTING=1` in the env cannot reach them.

/// Sets the `disablesleep` power setting. `set(1)` engages (prevents sleep);
/// `set(0)` releases. Returns `Ok(())` on a successful pmset run; `Err` (with a
/// short reason) when pmset rejects the change.
pub trait PmsetDisableSleep {
    fn set(&self, value: u8) -> Result<(), String>;
}

/// Reads the live `SleepDisabled` value. FAIL-SAFE: returns `0` on any
/// parse/spawn failure (never panics, never reports a stuck `1`).
pub trait SleepReader {
    fn read(&self) -> u8;
}

/// The pinned PATH for the privileged pmset boundary. Identical to the bash
/// helper's `PATH="/usr/bin:/bin:/usr/sbin:/sbin"`.
pub const PINNED_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Parse the `SleepDisabled` field out of `pmset -g` text. FAIL-SAFE to 0 on a
/// missing/non-(0|1) field. Pure so it is unit-testable without spawning.
pub fn parse_sleepdisabled(text: &str) -> u8 {
    // Bash: `awk '/SleepDisabled/ {print $NF}'` then `case 0|1`. Mirror it: the
    // first line containing "SleepDisabled" whose last whitespace token is 0|1.
    for line in text.lines() {
        if line.contains("SleepDisabled")
            && let Some(last) = line.split_whitespace().next_back()
        {
            return match last {
                "1" => 1,
                _ => 0,
            };
        }
    }
    0
}

/// Real macOS pmset transition. FIXED argv, `env_clear()` + pinned PATH.
pub struct MacPmset;

impl PmsetDisableSleep for MacPmset {
    fn set(&self, value: u8) -> Result<(), String> {
        // FIXED argv per action — value is constrained to "0" or "1", never
        // request-derived freeform text.
        let arg = match value {
            0 => "0",
            1 => "1",
            _ => return Err(format!("invalid disablesleep value: {value}")),
        };
        let status = std::process::Command::new("/usr/bin/pmset")
            .args(["-a", "disablesleep", arg])
            .env_clear()
            .env("PATH", PINNED_PATH)
            .status()
            .map_err(|e| format!("pmset spawn failed: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("pmset exited with {status}"))
        }
    }
}

/// Real macOS SleepDisabled reader (`pmset -g`).
pub struct MacSleepReader;

impl SleepReader for MacSleepReader {
    fn read(&self) -> u8 {
        let out = std::process::Command::new("/usr/bin/pmset")
            .arg("-g")
            .env_clear()
            .env("PATH", PINNED_PATH)
            .output();
        match out {
            Ok(o) => parse_sleepdisabled(&String::from_utf8_lossy(&o.stdout)),
            Err(_) => 0,
        }
    }
}

// ── Compile-time test fakes (cfg(test) OR feature = "helper-test-seam") ───────
//
// These back the lib unit tests AND the feature-gated subprocess adversarial
// test. They read/write a file-backed SleepDisabled value and an events log so a
// SUBPROCESS helper can be observed. They are NOT in the shipped binary.

#[cfg(any(test, feature = "helper-test-seam"))]
pub mod fake {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    /// Env var naming the file holding the fake SleepDisabled value (0|1). Mirrors
    /// the bash test's `ROOT_HELPER_SLEEP_FILE`.
    pub const SLEEP_FILE_ENV: &str = "VIGIL_FAKE_SLEEP_FILE";
    /// Env var naming the events log the fake pmset appends to. Mirrors the bash
    /// test's `ROOT_HELPER_EVENTS`.
    pub const EVENTS_ENV: &str = "VIGIL_FAKE_EVENTS";
    /// When this env var is "1", the fake pmset FAILS (mirrors
    /// `ROOT_HELPER_PMSET_FAIL`).
    pub const PMSET_FAIL_ENV: &str = "VIGIL_FAKE_PMSET_FAIL";

    fn sleep_file() -> Option<PathBuf> {
        std::env::var_os(SLEEP_FILE_ENV).map(PathBuf::from)
    }

    fn append_event(line: &str) {
        if let Some(p) = std::env::var_os(EVENTS_ENV)
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    /// File-backed fake pmset. Writes the new SleepDisabled value to the file
    /// named by `VIGIL_FAKE_SLEEP_FILE` and appends `pmset -a disablesleep <v>`
    /// to `VIGIL_FAKE_EVENTS`. Honors `VIGIL_FAKE_PMSET_FAIL=1`.
    pub struct FakePmset;

    impl PmsetDisableSleep for FakePmset {
        fn set(&self, value: u8) -> Result<(), String> {
            let arg = match value {
                0 => "0",
                1 => "1",
                _ => return Err("invalid".to_string()),
            };
            let fail = std::env::var(PMSET_FAIL_ENV)
                .map(|v| v == "1")
                .unwrap_or(false);
            if fail {
                append_event(&format!("pmset fail -a disablesleep {arg}"));
                return Err("pmset fail".to_string());
            }
            append_event(&format!("pmset -a disablesleep {arg}"));
            if let Some(p) = sleep_file() {
                let _ = std::fs::write(p, format!("{arg}\n"));
            }
            Ok(())
        }
    }

    /// File-backed fake SleepDisabled reader. Reads `VIGIL_FAKE_SLEEP_FILE`;
    /// FAIL-SAFE to 0 on missing/corrupt.
    pub struct FakeSleepReader;

    impl SleepReader for FakeSleepReader {
        fn read(&self) -> u8 {
            match sleep_file().and_then(|p| std::fs::read_to_string(p).ok()) {
                Some(s) => match s.trim() {
                    "1" => 1,
                    _ => 0,
                },
                None => 0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sleepdisabled_table() {
        assert_eq!(parse_sleepdisabled(" SleepDisabled\t\t0\n"), 0);
        assert_eq!(parse_sleepdisabled(" SleepDisabled\t\t1\n"), 1);
        // missing
        assert_eq!(parse_sleepdisabled("Some other field 1\n"), 0);
        // non-0|1 last token
        assert_eq!(parse_sleepdisabled("SleepDisabled foo\n"), 0);
        // empty
        assert_eq!(parse_sleepdisabled(""), 0);
    }
}
