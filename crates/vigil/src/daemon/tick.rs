//! One deterministic tick parser/writer for daemon policy and all consumers.
//! Version 1 is read as legacy policy: its session counts and refresh identity
//! are unknown. Version 2 acknowledges refresh only after the power action.
use std::collections::BTreeMap;
use std::io;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::mode::Mode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickSnapshot {
    pub version: u32,
    pub pid: u32,
    pub daemon_start: Option<i64>,
    pub updated_at: i64,
    pub tick_secs: u32,
    pub evaluation_sequence: u64,
    /// Legacy policy demand. This is not a session count.
    pub refcount_active: u32,
    pub working: Option<u32>,
    pub waiting: Option<u32>,
    pub idle: Option<u32>,
    pub unknown: Option<u32>,
    pub detected_hosts: Option<u32>,
    pub wrappers: Option<u32>,
    pub uncertain: Option<u32>,
    pub vscode: Option<bool>,
    pub grace_remaining: u32,
    pub refresh_token: Option<String>,
    pub desired_hold: bool,
    /// POST-action engaged state.
    pub engaged: bool,
    pub thermal_cut: bool,
    pub battery_cut: bool,
    pub cooling: bool,
    pub mode: Mode,
}

impl Default for TickSnapshot {
    fn default() -> Self {
        Self {
            version: 2,
            pid: 0,
            daemon_start: None,
            updated_at: 0,
            tick_secs: 5,
            evaluation_sequence: 0,
            refcount_active: 0,
            working: Some(0),
            waiting: Some(0),
            idle: Some(0),
            unknown: Some(0),
            detected_hosts: Some(0),
            wrappers: Some(0),
            uncertain: Some(0),
            vscode: Some(false),
            grace_remaining: 0,
            refresh_token: None,
            desired_hold: false,
            engaged: false,
            thermal_cut: false,
            battery_cut: false,
            cooling: false,
            mode: Mode::Auto,
        }
    }
}

impl TickSnapshot {
    pub fn supports_refresh(&self) -> bool {
        self.version == 2 && self.pid > 0 && self.daemon_start.is_some_and(|v| v > 0)
    }

    pub fn policy_reason(&self) -> &'static str {
        if self.mode == Mode::Off {
            "manual-off"
        } else if self.thermal_cut {
            "thermal-cut"
        } else if self.battery_cut {
            "battery-cut"
        } else if self.cooling {
            "thermal-cooldown"
        } else if self.mode == Mode::On {
            "manual-on"
        } else if self.version == 1 {
            "legacy-policy"
        } else if self.working.unwrap_or(0) > 0 {
            "working-sessions"
        } else if self.uncertain.unwrap_or(0) > 0 {
            "uncertain-activity"
        } else if self.wrappers.unwrap_or(0) > 0 {
            "wrappers"
        } else if self.vscode == Some(true) {
            "vscode-copilot-chat"
        } else if self.grace_remaining > 0 {
            "release-grace"
        } else {
            "no-demand"
        }
    }
}

const LEGACY_KEYS: [&str; 10] = [
    "pid",
    "updated_at",
    "tick_secs",
    "refcount_active",
    "desired_hold",
    "engaged",
    "thermal_cut",
    "battery_cut",
    "cooling",
    "mode",
];
const V2_KEYS: [&str; 23] = [
    "version",
    "pid",
    "daemon_start",
    "updated_at",
    "tick_secs",
    "evaluation_sequence",
    "refcount_active",
    "working",
    "waiting",
    "idle",
    "unknown",
    "detected_hosts",
    "wrappers",
    "uncertain",
    "vscode",
    "grace_remaining",
    "refresh_token",
    "desired_hold",
    "engaged",
    "thermal_cut",
    "battery_cut",
    "cooling",
    "mode",
];

/// Render in a fixed key order. `unknown` is reserved for legacy/missing data.
pub fn render(t: &TickSnapshot) -> String {
    let policy = format!(
        "desired_hold={}\nengaged={}\nthermal_cut={}\nbattery_cut={}\ncooling={}\nmode={}\n",
        t.desired_hold as u8,
        t.engaged as u8,
        t.thermal_cut as u8,
        t.battery_cut as u8,
        t.cooling as u8,
        t.mode.as_str()
    );
    if t.version == 1 {
        return format!(
            "pid={}\nupdated_at={}\ntick_secs={}\nrefcount_active={}\n{policy}",
            t.pid, t.updated_at, t.tick_secs, t.refcount_active
        );
    }
    let value = |v: Option<u32>| {
        v.map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    format!(
        "version=2\npid={}\ndaemon_start={}\nupdated_at={}\ntick_secs={}\nevaluation_sequence={}\nrefcount_active={}\nworking={}\nwaiting={}\nidle={}\nunknown={}\ndetected_hosts={}\nwrappers={}\nuncertain={}\nvscode={}\ngrace_remaining={}\nrefresh_token={}\n{policy}",
        t.pid,
        t.daemon_start
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unknown".into()),
        t.updated_at,
        t.tick_secs,
        t.evaluation_sequence,
        t.refcount_active,
        value(t.working),
        value(t.waiting),
        value(t.idle),
        value(t.unknown),
        value(t.detected_hosts),
        value(t.wrappers),
        value(t.uncertain),
        value(t.vscode.map(u32::from)),
        t.grace_remaining,
        t.refresh_token.as_deref().unwrap_or("-")
    )
}

/// Require a complete supported schema and reject duplicate keys. Atomic writes
/// mean partial files cannot be interpreted as evaluated policy.
pub fn parse(text: &str) -> Option<TickSnapshot> {
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=')?;
        if fields.insert(key, value).is_some() {
            return None;
        }
    }
    let version = match fields.get("version") {
        None => 1,
        Some(&"2") => 2,
        _ => return None,
    };
    let keys: &[&str] = if version == 1 { &LEGACY_KEYS } else { &V2_KEYS };
    if fields.len() != keys.len() || keys.iter().any(|key| !fields.contains_key(key)) {
        return None;
    }
    let field = |key: &str| fields.get(key).copied();
    let flag = |key: &str| match field(key)? {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    };
    let mut t = TickSnapshot {
        version,
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
        working: None,
        waiting: None,
        idle: None,
        unknown: None,
        detected_hosts: None,
        wrappers: None,
        uncertain: None,
        vscode: None,
        ..Default::default()
    };
    if version == 2 {
        t.daemon_start = match field("daemon_start")? {
            "unknown" => None,
            s => Some(s.parse().ok()?),
        };
        t.evaluation_sequence = field("evaluation_sequence")?.parse().ok()?;
        let count = |key: &str| match field(key)? {
            "unknown" => Some(None),
            s => Some(Some(s.parse::<u32>().ok()?)),
        };
        t.working = count("working")?;
        t.waiting = count("waiting")?;
        t.idle = count("idle")?;
        t.unknown = count("unknown")?;
        t.detected_hosts = count("detected_hosts")?;
        t.wrappers = count("wrappers")?;
        t.uncertain = count("uncertain")?;
        t.vscode = match field("vscode")? {
            "unknown" => None,
            "0" => Some(false),
            "1" => Some(true),
            _ => return None,
        };
        t.grace_remaining = field("grace_remaining")?.parse().ok()?;
        t.refresh_token = match field("refresh_token")? {
            "-" => None,
            token if crate::daemon::refresh::valid_token(token) => Some(token.into()),
            _ => return None,
        };
    }
    Some(t)
}

pub fn read(tick_file: &Path) -> Option<TickSnapshot> {
    parse(&std::fs::read_to_string(tick_file).ok()?)
}

pub fn stale_after(tick_secs: i64) -> i64 {
    (tick_secs.saturating_mul(2).saturating_add(5)).max(15)
}

pub fn write_tick(tick_file: &Path, t: &TickSnapshot) -> io::Result<()> {
    let mut tmp = tick_file.as_os_str().to_os_string();
    tmp.push(format!(".{}", t.pid));
    let tmp = std::path::PathBuf::from(tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)?;
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(render(t).as_bytes())?;
    std::fs::rename(&tmp, tick_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> TickSnapshot {
        TickSnapshot {
            pid: 4242,
            daemon_start: Some(1_699_999_999),
            updated_at: 1_700_000_000,
            evaluation_sequence: 7,
            refcount_active: 2,
            working: Some(2),
            desired_hold: true,
            engaged: true,
            ..Default::default()
        }
    }

    #[test]
    fn v2_bytes_exact_and_round_trip() {
        let want = "version=2\npid=4242\ndaemon_start=1699999999\nupdated_at=1700000000\ntick_secs=5\nevaluation_sequence=7\nrefcount_active=2\nworking=2\nwaiting=0\nidle=0\nunknown=0\ndetected_hosts=0\nwrappers=0\nuncertain=0\nvscode=0\ngrace_remaining=0\nrefresh_token=-\ndesired_hold=1\nengaged=1\nthermal_cut=0\nbattery_cut=0\ncooling=0\nmode=auto\n";
        assert_eq!(render(&sample()), want);
        assert_eq!(parse(want), Some(sample()));
    }

    #[test]
    fn legacy_policy_reads_with_unknown_counts_and_no_refresh() {
        let body = "pid=4242\nupdated_at=1700000000\ntick_secs=5\nrefcount_active=2\ndesired_hold=1\nengaged=1\nthermal_cut=0\nbattery_cut=0\ncooling=0\nmode=auto\n";
        let t = parse(body).unwrap();
        assert_eq!(t.version, 1);
        assert_eq!(t.working, None);
        assert_eq!(t.unknown, None);
        assert!(!t.supports_refresh());
        assert_eq!(render(&t), body);
    }

    #[test]
    fn rejects_duplicates_partial_and_unsupported_versions() {
        let full = render(&sample());
        for bad in [
            full.replace("version=2", "version=3"),
            full.replace("mode=auto\n", ""),
            full.replace("engaged=1", "engaged=yes"),
            full.replace("pid=4242", "pid=abc"),
            format!("{full}engaged=0\n"),
            full.replace("refresh_token=-", "refresh_token=a=b"),
        ] {
            assert!(parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn refresh_token_and_same_second_evaluation_survive_round_trip() {
        let t = TickSnapshot {
            refresh_token: Some("abcd1234".into()),
            evaluation_sequence: 8,
            ..sample()
        };
        assert_eq!(parse(&render(&t)), Some(t));
    }

    #[test]
    fn stale_after_scales_and_has_floor() {
        assert_eq!(stale_after(5), 15);
        assert_eq!(stale_after(1), 15);
        assert_eq!(stale_after(20), 45);
    }

    #[test]
    fn writes_atomically_with_private_permissions_and_appended_pid_tmp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.tick");
        write_tick(&path, &sample()).unwrap();
        assert_eq!(read(&path), Some(sample()));
        assert!(!dir.path().join("daemon.tick.4242").exists());
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
