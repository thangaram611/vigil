//! `vigil menubar` — print the SwiftBar menu for the menu-bar plugin.
//!
//! Runs every few seconds, so it reads only the daemon tick file and the mode
//! file: no process scan, no `launchctl`, no helper round-trip. The three mode
//! items call back into this same binary (`vigil mode <m>`), then refresh.
//!
//! Output follows the SwiftBar plugin format: the first line is the menu-bar
//! title, `---` separates sections, and `key=value` pairs after ` | ` are line
//! parameters (`sfimage` = SF Symbol, `checked`, `bash`/`paramN`/`terminal`/
//! `refresh` for actions).

use std::fmt::Write as _;

use vigil::daemon::tick::{self, TickSnapshot};
use vigil::mode::{self, Mode};

use super::{load_config_or_exit, now_unix};

/// What the daemon is doing, as the menu bar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// No fresh tick: the daemon is not running or not ticking.
    Down,
    Holding,
    /// Demand exists, but the thermal guard (cut or cooldown) released the hold.
    Thermal,
    /// Demand exists, but the battery floor released the hold.
    Battery,
    /// Demand exists and nothing blocks it; the daemon engages on its next tick.
    Engaging,
    Idle,
}

impl State {
    fn of(tick: Option<&TickSnapshot>, now: i64) -> State {
        let Some(t) = tick else {
            return State::Down;
        };
        if now - t.updated_at > tick::stale_after(t.tick_secs.into()) {
            State::Down
        } else if t.engaged {
            State::Holding
        } else if t
            .mode
            .demand(t.refcount_active.max(u32::from(t.grace_remaining > 0)))
            == 0
        {
            State::Idle
        } else if t.thermal_cut || t.cooling {
            State::Thermal
        } else if t.battery_cut {
            State::Battery
        } else {
            State::Engaging
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            State::Down => "exclamationmark.triangle",
            State::Holding | State::Engaging => "cup.and.saucer.fill",
            State::Thermal => "thermometer.high",
            State::Battery => "battery.25percent",
            State::Idle => "cup.and.saucer",
        }
    }

    fn sentence(self) -> &'static str {
        match self {
            State::Down => "Daemon not running — try 'vigil doctor'",
            State::Holding => "Holding — this Mac will not sleep",
            State::Thermal => "Released — thermal pressure",
            State::Battery => "Released — battery below the floor",
            State::Engaging => "Engaging…",
            State::Idle => "Idle — this Mac may sleep",
        }
    }
}

fn count(value: Option<u32>) -> String {
    value
        .map(|n| n.to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Render the whole menu. `requested` is the mode file (so a click shows at
/// once, before the daemon's next tick); `exe` is this binary, for the actions.
fn render(tick: Option<&TickSnapshot>, requested: Mode, now: i64, exe: &str) -> String {
    let state = State::of(tick, now);
    let mut o = String::new();

    // Title: the state symbol, plus the word for a manual mode so a forgotten
    // override stays visible.
    let word = match requested {
        Mode::Auto => "",
        Mode::On => "ON",
        Mode::Off => "OFF",
    };
    let _ = writeln!(o, "{word} | sfimage={}", state.symbol());
    o.push_str("---\n");
    let _ = writeln!(o, "{}", state.sentence());
    if let Some(t) = tick.filter(|_| state != State::Down) {
        if let (Some(working), Some(waiting), Some(idle), Some(unknown)) =
            (t.working, t.waiting, t.idle, t.unknown)
        {
            let _ = writeln!(
                o,
                "Sessions: {working} working, {waiting} waiting, {idle} idle, {unknown} unknown"
            );
            let _ = writeln!(
                o,
                "Detected hosts: {} · wrappers: {} · uncertain demand: {}",
                count(t.detected_hosts),
                count(t.wrappers),
                count(t.uncertain)
            );
            if t.vscode == Some(true) {
                o.push_str("VS Code Copilot Chat demand\n");
            }
        } else {
            o.push_str("Session counts unknown — legacy tick\n");
        }
        let _ = writeln!(o, "Policy: {}", t.policy_reason());
        if t.grace_remaining > 0 {
            let _ = writeln!(o, "Release grace: {}s remaining", t.grace_remaining);
        }
        let _ = writeln!(
            o,
            "Cached evaluation: {}s ago",
            now.saturating_sub(t.updated_at).max(0)
        );
    }
    o.push_str("---\n");
    for (label, m) in [
        ("Auto — follow agents", Mode::Auto),
        ("Keep awake", Mode::On),
        ("Never hold", Mode::Off),
    ] {
        let _ = writeln!(
            o,
            "{label} | checked={} bash=\"{exe}\" param0=mode param1={} terminal=false refresh=true",
            m == requested,
            m.as_str(),
        );
    }
    o.push_str("---\n");
    let _ = writeln!(
        o,
        "Refresh now | bash=\"{exe}\" param0=refresh terminal=false refresh=true"
    );
    let _ = writeln!(o, "Status… | bash=\"{exe}\" param0=status terminal=true");
    let _ = writeln!(
        o,
        "Log… | bash=\"{exe}\" param0=log param1=-f terminal=true"
    );
    o
}

pub fn run() -> ! {
    let cfg = load_config_or_exit();
    let tick = tick::read(std::path::Path::new(&cfg.daemon_tick_file));
    let requested = mode::read(std::path::Path::new(&cfg.mode_file));
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "vigil".to_string());
    // Plain print: SwiftBar parses this, so no ANSI.
    print!("{}", render(tick.as_ref(), requested, now_unix(), &exe));
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    fn tick() -> TickSnapshot {
        TickSnapshot {
            pid: 4242,
            updated_at: NOW - 3,
            tick_secs: 5,
            refcount_active: 2,
            desired_hold: true,
            engaged: true,
            thermal_cut: false,
            battery_cut: false,
            cooling: false,
            mode: Mode::Auto,
            working: Some(2),
            ..Default::default()
        }
    }

    #[test]
    fn state_table() {
        let idle = TickSnapshot {
            refcount_active: 0,
            working: Some(0),
            desired_hold: false,
            engaged: false,
            ..tick()
        };
        let released = TickSnapshot {
            engaged: false,
            ..tick()
        };
        let cases: &[(&str, Option<TickSnapshot>, State)] = &[
            ("no tick file", None, State::Down),
            (
                "stale tick",
                Some(TickSnapshot {
                    updated_at: NOW - 16,
                    ..tick()
                }),
                State::Down,
            ),
            ("engaged", Some(tick()), State::Holding),
            ("no agents", Some(idle.clone()), State::Idle),
            (
                "mode off hides active agents",
                Some(TickSnapshot {
                    mode: Mode::Off,
                    ..released.clone()
                }),
                State::Idle,
            ),
            (
                "mode on with no agents, not yet engaged",
                Some(TickSnapshot {
                    mode: Mode::On,
                    ..idle.clone()
                }),
                State::Engaging,
            ),
            (
                "thermal cut",
                Some(TickSnapshot {
                    thermal_cut: true,
                    ..released.clone()
                }),
                State::Thermal,
            ),
            (
                "cooldown",
                Some(TickSnapshot {
                    cooling: true,
                    ..released.clone()
                }),
                State::Thermal,
            ),
            (
                "battery cut",
                Some(TickSnapshot {
                    battery_cut: true,
                    ..released.clone()
                }),
                State::Battery,
            ),
            (
                "thermal outranks battery",
                Some(TickSnapshot {
                    thermal_cut: true,
                    battery_cut: true,
                    ..released.clone()
                }),
                State::Thermal,
            ),
            ("demand, nothing blocking", Some(released), State::Engaging),
        ];
        for (label, t, want) in cases {
            assert_eq!(State::of(t.as_ref(), NOW), *want, "{label}");
        }
    }

    #[test]
    fn menu_bytes_exact_holding_auto() {
        let want = " | sfimage=cup.and.saucer.fill\n\
            ---\n\
            Holding — this Mac will not sleep\n\
            Sessions: 2 working, 0 waiting, 0 idle, 0 unknown\n\
            Detected hosts: 0 · wrappers: 0 · uncertain demand: 0\n\
            Policy: working-sessions\n\
            Cached evaluation: 3s ago\n\
            ---\n\
            Auto — follow agents | checked=true bash=\"/a b/vigil\" param0=mode param1=auto terminal=false refresh=true\n\
            Keep awake | checked=false bash=\"/a b/vigil\" param0=mode param1=on terminal=false refresh=true\n\
            Never hold | checked=false bash=\"/a b/vigil\" param0=mode param1=off terminal=false refresh=true\n\
            ---\n\
            Refresh now | bash=\"/a b/vigil\" param0=refresh terminal=false refresh=true\n\
            Status… | bash=\"/a b/vigil\" param0=status terminal=true\n\
            Log… | bash=\"/a b/vigil\" param0=log param1=-f terminal=true\n";
        assert_eq!(render(Some(&tick()), Mode::Auto, NOW, "/a b/vigil"), want);
    }

    #[test]
    fn requested_mode_shows_before_the_daemon_applies_it() {
        // The tick still says auto; the mode file already says on.
        let menu = render(Some(&tick()), Mode::On, NOW, "vigil");
        assert!(menu.starts_with("ON | sfimage="), "{menu}");
        assert!(menu.contains("Keep awake | checked=true"), "{menu}");
        assert!(
            menu.contains("Auto — follow agents | checked=false"),
            "{menu}"
        );
    }

    #[test]
    fn daemon_down_hides_the_agent_count() {
        let menu = render(None, Mode::Off, NOW, "vigil");
        assert!(menu.starts_with("OFF | sfimage=exclamationmark.triangle\n"));
        assert!(!menu.contains("Sessions:"), "{menu}");
    }
    #[test]
    fn grace_remains_visible_without_fabricating_working_sessions() {
        let t = TickSnapshot {
            working: Some(0),
            refcount_active: 0,
            grace_remaining: 12,
            ..tick()
        };
        let menu = render(Some(&t), Mode::Auto, NOW, "vigil");
        assert!(menu.contains("Sessions: 0 working"));
        assert!(menu.contains("Policy: release-grace"));
        assert!(menu.contains("Release grace: 12s remaining"));
        assert!(menu.contains("Refresh now | bash=\"vigil\" param0=refresh"));
    }

    #[test]
    fn legacy_count_is_not_presented_as_session_count() {
        let t = TickSnapshot {
            version: 1,
            working: None,
            waiting: None,
            idle: None,
            unknown: None,
            ..tick()
        };
        let menu = render(Some(&t), Mode::Auto, NOW, "vigil");
        assert!(menu.contains("Session counts unknown — legacy tick"));
        assert!(!menu.contains("Sessions:"));
    }
}
