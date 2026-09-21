//! The manual hold mode: the user's override of agent detection.
//!
//! `vigil mode` (and the menu bar through it) writes one word to the mode file;
//! the daemon reads it every tick. An absent file means [`Mode::Auto`], so a
//! fresh install and `vigil mode auto` are the same state.
//!
//! A mode outranks detection, never the safety guards: it only changes the
//! demand fed to [`crate::daemon::desired_hold`], which still refuses to hold
//! through a thermal or battery cut.

use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Mode {
    /// Hold while agents are active (the default).
    #[default]
    Auto,
    /// Hold regardless of agents.
    On,
    /// Never hold.
    Off,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::On => "on",
            Mode::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "auto" => Some(Mode::Auto),
            "on" => Some(Mode::On),
            "off" => Some(Mode::Off),
            _ => None,
        }
    }

    /// The hold demand this mode yields from `active` detected agents: the
    /// count the daemon's decide/act/recover paths then treat as "work exists".
    pub fn demand(self, active: u32) -> u32 {
        match self {
            Mode::Auto => active,
            Mode::On => active.max(1),
            Mode::Off => 0,
        }
    }
}

/// The mode in `mode_file`. Absent, unreadable, or unrecognized content is
/// [`Mode::Auto`]: a damaged override must never pin the machine awake.
pub fn read(mode_file: &Path) -> Mode {
    std::fs::read_to_string(mode_file)
        .ok()
        .and_then(|s| Mode::parse(&s))
        .unwrap_or_default()
}

/// Persist `mode`. `Auto` removes the file; `On`/`Off` are written via
/// tmp+rename so the daemon never reads a half-written word.
pub fn write(mode_file: &Path, mode: Mode) -> io::Result<()> {
    if mode == Mode::Auto {
        return match std::fs::remove_file(mode_file) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let mut tmp = mode_file.as_os_str().to_os_string();
    tmp.push(format!(".{}", std::process::id()));
    std::fs::write(&tmp, format!("{}\n", mode.as_str()))?;
    std::fs::rename(&tmp, mode_file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_as_str_round_trip() {
        for m in [Mode::Auto, Mode::On, Mode::Off] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(
            Mode::parse("on\n"),
            Some(Mode::On),
            "file content is trimmed"
        );
        assert_eq!(Mode::parse("ON"), None);
        assert_eq!(Mode::parse(""), None);
    }

    #[test]
    fn demand_overrides_detection_only() {
        assert_eq!(Mode::Auto.demand(0), 0);
        assert_eq!(Mode::Auto.demand(3), 3);
        assert_eq!(Mode::On.demand(0), 1, "on holds with no agents");
        assert_eq!(Mode::On.demand(3), 3);
        assert_eq!(Mode::Off.demand(3), 0, "off ignores active agents");
    }

    #[test]
    fn read_defaults_to_auto() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("mode");
        assert_eq!(read(&f), Mode::Auto, "absent");
        std::fs::write(&f, "garbage\n").unwrap();
        assert_eq!(read(&f), Mode::Auto, "unrecognized content");
    }

    #[test]
    fn write_round_trips_and_auto_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("mode");
        write(&f, Mode::On).unwrap();
        assert_eq!(read(&f), Mode::On);
        write(&f, Mode::Off).unwrap();
        assert_eq!(read(&f), Mode::Off);
        write(&f, Mode::Auto).unwrap();
        assert!(!f.exists(), "auto is the absent file");
        write(&f, Mode::Auto).unwrap();
        let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 0, "no tmp files left behind");
    }
}
