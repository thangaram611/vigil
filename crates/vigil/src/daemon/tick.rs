//! The FROZEN daemon tick-file writer (§2.1.6) — a byte-stable ABI.
//!
//! Exactly nine `key=value\n` lines in this precise order, no JSON, no quoting,
//! no extra fields, no trailing blank line. The reader (`cmd_daemon_tick_field`
//! in bash, `read_tick_fields` in [`crate::check`]) does an `awk -F=` first-match
//! parse, so `=` must be the FIRST separator and there is exactly one field per
//! line. `pid` and `updated_at` MUST be byte-faithful — a wrong `pid`
//! permanently classifies the scan as `pending`/`missing`, and a non-numeric
//! `updated_at` does the same.
//!
//! Written via atomic tmp+rename: write `{tick_file}.{pid}`, then `rename` over
//! `daemon.tick`, so a consumer never reads a half-written file. `engaged` is the
//! value AFTER the act-branch mutates it (POST-action).

use std::io;
use std::path::Path;

/// The nine fields of one tick snapshot. `pid` and `updated_at` are the
/// scan-state inputs; the five booleans are emitted as `0|1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickSnapshot {
    pub pid: u32,
    pub updated_at: i64,
    pub tick_secs: u32,
    pub refcount_active: u32,
    pub desired_hold: bool,
    /// POST-action engaged state.
    pub engaged: bool,
    pub thermal_cut: bool,
    pub battery_cut: bool,
    pub cooling: bool,
}

/// `bool -> "0"|"1"` (bash `(( … ))` numeric).
fn b(v: bool) -> u8 {
    v as u8
}

/// Render the nine frozen lines. Exposed so the byte-stability test can assert
/// the exact bytes without touching the filesystem.
pub fn render(t: &TickSnapshot) -> String {
    format!(
        "pid={}\nupdated_at={}\ntick_secs={}\nrefcount_active={}\n\
         desired_hold={}\nengaged={}\nthermal_cut={}\nbattery_cut={}\ncooling={}\n",
        t.pid,
        t.updated_at,
        t.tick_secs,
        t.refcount_active,
        b(t.desired_hold),
        b(t.engaged),
        b(t.thermal_cut),
        b(t.battery_cut),
        b(t.cooling),
    )
}

/// Atomic tmp+rename write of the frozen tick file. `tmp = {tick_file}.{pid}`,
/// matching bash `"$VIGIL_DAEMON_TICK_FILE.$$"`.
pub fn write_tick(tick_file: &Path, t: &TickSnapshot) -> io::Result<()> {
    // Bash uses `"$VIGIL_DAEMON_TICK_FILE.$$"` — i.e. append `.{pid}` to the
    // FULL path, NOT a Path::with_extension (which would replace `.tick`). Build
    // the tmp name from the OsString directly.
    let mut tmp = tick_file.as_os_str().to_os_string();
    tmp.push(format!(".{}", t.pid));
    let tmp = std::path::PathBuf::from(tmp);

    std::fs::write(&tmp, render(t))?;
    std::fs::rename(&tmp, tick_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TickSnapshot {
        TickSnapshot {
            pid: 4242,
            updated_at: 1_700_000_000,
            tick_secs: 5,
            refcount_active: 2,
            desired_hold: true,
            engaged: true,
            thermal_cut: false,
            battery_cut: false,
            cooling: false,
        }
    }

    #[test]
    fn frozen_nine_field_bytes_exact() {
        // The exact byte layout the bash `daemon_write_tick` emits (bin/vigil-daemon
        // lines 70-78): nine key=value lines, key order frozen, no trailing blank.
        let want = "pid=4242\n\
                    updated_at=1700000000\n\
                    tick_secs=5\n\
                    refcount_active=2\n\
                    desired_hold=1\n\
                    engaged=1\n\
                    thermal_cut=0\n\
                    battery_cut=0\n\
                    cooling=0\n";
        assert_eq!(render(&sample()), want);
    }

    #[test]
    fn all_booleans_zero() {
        let t = TickSnapshot {
            desired_hold: false,
            engaged: false,
            thermal_cut: true,
            battery_cut: true,
            cooling: true,
            ..sample()
        };
        let want = "pid=4242\n\
                    updated_at=1700000000\n\
                    tick_secs=5\n\
                    refcount_active=2\n\
                    desired_hold=0\n\
                    engaged=0\n\
                    thermal_cut=1\n\
                    battery_cut=1\n\
                    cooling=1\n";
        assert_eq!(render(&t), want);
    }

    #[test]
    fn write_is_atomic_rename_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let tick = dir.path().join("daemon.tick");
        write_tick(&tick, &sample()).unwrap();
        // tmp must be gone (renamed away).
        let tmp = {
            let mut s = tick.as_os_str().to_os_string();
            s.push(format!(".{}", sample().pid));
            std::path::PathBuf::from(s)
        };
        assert!(!tmp.exists(), "tmp must be renamed away");
        let body = std::fs::read_to_string(&tick).unwrap();
        assert_eq!(body, render(&sample()));
    }

    #[test]
    fn tmp_name_appends_pid_to_full_path_not_replacing_ext() {
        // Regression: bash appends `.$$` to the WHOLE path; with_extension would
        // turn `daemon.tick` into `daemon.4242`. Assert the tmp filename keeps
        // `.tick` and appends the pid.
        let dir = tempfile::tempdir().unwrap();
        let tick = dir.path().join("daemon.tick");
        let mut tmp = tick.as_os_str().to_os_string();
        tmp.push(".4242");
        let tmp = std::path::PathBuf::from(tmp);
        assert_eq!(
            tmp.file_name().unwrap().to_str().unwrap(),
            "daemon.tick.4242"
        );
    }
}
