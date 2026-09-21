//! The daemon tick file (§2.1.6): the daemon's per-tick state, written here and
//! read back by `status`/`doctor`, `lock`, and `menubar`.
//!
//! Exactly ten `key=value\n` lines in this precise order, no JSON, no quoting,
//! no extra fields, no trailing blank line. Readers ([`parse`] here,
//! `read_tick_fields` in [`crate::check`]) split on the FIRST `=` and take the
//! first match, so there is exactly one field per line. `pid` and `updated_at` MUST be byte-faithful — a wrong `pid`
//! permanently classifies the scan as `pending`/`missing`, and a non-numeric
//! `updated_at` does the same.
//!
//! Written via atomic tmp+rename: write `{tick_file}.{pid}`, then `rename` over
//! `daemon.tick`, so a consumer never reads a half-written file. `engaged` is the
//! value AFTER the act-branch mutates it (POST-action).

use std::io;
use std::path::Path;

use crate::mode::Mode;

/// The ten fields of one tick snapshot. `pid` and `updated_at` are the
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
    /// The manual mode the daemon applied this tick.
    pub mode: Mode,
}

/// `bool -> "0"|"1"` (bash `(( … ))` numeric).
fn b(v: bool) -> u8 {
    v as u8
}

/// Render the ten lines. Exposed so the byte-stability test can assert
/// the exact bytes without touching the filesystem.
pub fn render(t: &TickSnapshot) -> String {
    format!(
        "pid={}\nupdated_at={}\ntick_secs={}\nrefcount_active={}\n\
         desired_hold={}\nengaged={}\nthermal_cut={}\nbattery_cut={}\ncooling={}\nmode={}\n",
        t.pid,
        t.updated_at,
        t.tick_secs,
        t.refcount_active,
        b(t.desired_hold),
        b(t.engaged),
        b(t.thermal_cut),
        b(t.battery_cut),
        b(t.cooling),
        t.mode.as_str(),
    )
}

/// Parse a tick file body. `None` unless every field is present and well-formed:
/// the writer is atomic, so a partial snapshot is never a state to interpret.
pub fn parse(text: &str) -> Option<TickSnapshot> {
    let field = |key: &str| {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .find_map(|(k, v)| (k == key).then_some(v))
    };
    let flag = |key: &str| match field(key)? {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    };
    Some(TickSnapshot {
        pid: field("pid")?.parse().ok()?,
        updated_at: field("updated_at")?.parse().ok()?,
        tick_secs: field("tick_secs")?.parse().ok()?,
        refcount_active: field("refcount_active")?.parse().ok()?,
        desired_hold: flag("desired_hold")?,
        engaged: flag("engaged")?,
        thermal_cut: flag("thermal_cut")?,
        battery_cut: flag("battery_cut")?,
        cooling: flag("cooling")?,
        mode: Mode::parse(field("mode")?)?,
    })
}

/// Read and parse the tick file (`None` if absent or malformed).
pub fn read(tick_file: &Path) -> Option<TickSnapshot> {
    parse(&std::fs::read_to_string(tick_file).ok()?)
}

/// Seconds after which a tick written every `tick_secs` counts as stale.
pub fn stale_after(tick_secs: i64) -> i64 {
    (tick_secs * 2 + 5).max(15)
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
            mode: Mode::Auto,
        }
    }

    #[test]
    fn ten_field_bytes_exact() {
        // Ten key=value lines, key order fixed, no trailing blank.
        let want = "pid=4242\n\
                    updated_at=1700000000\n\
                    tick_secs=5\n\
                    refcount_active=2\n\
                    desired_hold=1\n\
                    engaged=1\n\
                    thermal_cut=0\n\
                    battery_cut=0\n\
                    cooling=0\n\
                    mode=auto\n";
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
            mode: Mode::Off,
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
                    cooling=1\n\
                    mode=off\n";
        assert_eq!(render(&t), want);
    }

    #[test]
    fn parse_round_trips_render() {
        let t = TickSnapshot {
            engaged: false,
            thermal_cut: true,
            mode: Mode::On,
            ..sample()
        };
        assert_eq!(parse(&render(&t)), Some(t));
    }

    #[test]
    fn parse_rejects_partial_or_malformed_snapshots() {
        let full = render(&sample());
        assert!(parse("").is_none());
        assert!(
            parse(&full.replace("mode=auto\n", "")).is_none(),
            "missing field"
        );
        assert!(parse(&full.replace("engaged=1", "engaged=yes")).is_none());
        assert!(parse(&full.replace("pid=4242", "pid=abc")).is_none());
        assert!(parse(&full.replace("mode=auto", "mode=sideways")).is_none());
    }

    #[test]
    fn stale_after_scales_with_tick_and_has_a_floor() {
        assert_eq!(stale_after(5), 15);
        assert_eq!(stale_after(1), 15, "floor");
        assert_eq!(stale_after(20), 45);
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
