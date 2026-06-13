//! PURE per-agent session-mtime activity scan — the Rust port of the
//! `vigil_agent_*` functions in `lib/activity.sh`.
//!
//! IO happens only through paths passed in by the caller; there is no env read.
//! The `find -mmin -N` whole-minute round-up is reproduced exactly (see §6 of
//! the 5.3 spec and the formula on [`is_active`]).

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// The three CLI agents with a session-dir activity signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Copilot,
}

impl Agent {
    /// `"claude" | "codex" | "copilot"`.
    pub fn token(&self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Copilot => "copilot",
        }
    }

    /// The session subdir appended under the provider home:
    /// claude -> `projects`, codex -> `sessions`, copilot -> `session-state`.
    pub fn session_subdir(&self) -> &'static str {
        match self {
            Agent::Claude => "projects",
            Agent::Codex => "sessions",
            Agent::Copilot => "session-state",
        }
    }

    /// The whole-name glob for the agent's per-turn file.
    pub fn pattern(&self) -> AgentGlob {
        match self {
            Agent::Claude => AgentGlob::Suffix(".jsonl"),
            Agent::Codex => AgentGlob::PrefixSuffix("rollout-", ".jsonl"),
            Agent::Copilot => AgentGlob::Exact("events.jsonl"),
        }
    }
}

/// A tiny whole-name glob matcher (so the pure scan fn takes no shell string).
///
/// claude: `Suffix(".jsonl")`; codex: `PrefixSuffix("rollout-", ".jsonl")`;
/// copilot: `Exact("events.jsonl")`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentGlob {
    Exact(&'static str),
    Suffix(&'static str),
    PrefixSuffix(&'static str, &'static str),
}

impl AgentGlob {
    pub fn matches(&self, file_name: &str) -> bool {
        match self {
            AgentGlob::Exact(s) => file_name == *s,
            AgentGlob::Suffix(s) => file_name.ends_with(s),
            AgentGlob::PrefixSuffix(p, s) => file_name.starts_with(p) && file_name.ends_with(s),
        }
    }
}

/// home_override form (the tests' 2nd arg): `<home>/.claude/projects` etc.
///
/// Mirrors `vigil_session_dir_for_agent <agent> <home>`: appends `.<token>`
/// (with the leading dot) then `/<subdir>`.
pub fn session_dir_from_home(agent: Agent, home_override: &Path) -> PathBuf {
    home_override
        .join(format!(".{}", agent.token()))
        .join(agent.session_subdir())
}

/// resolved-provider-home form (daemon): `VigilConfig.<provider>_home` is
/// already `.../.claude`, so this just joins the subdir.
pub fn session_dir_from_provider_home(provider_home: &Path, agent: Agent) -> PathBuf {
    provider_home.join(agent.session_subdir())
}

/// The `find -mmin -N` window, in whole seconds. See [`is_active`].
fn window_secs(idle_after_sec: u32) -> i64 {
    // mins = ceil(secs / 60), floored at 1 (BSD `find -mmin` is whole-minute).
    let secs = idle_after_sec as i64;
    let mut mins = (secs + 59) / 60;
    if mins < 1 {
        mins = 1;
    }
    mins * 60
}

/// Whole-second mtime (st_mtime truncated to seconds) for a path, or None.
///
/// Uses whole seconds — NOT nanosecond precision — to match BSD `find -mmin`
/// granularity.
fn mtime_secs(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let dur = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(dur.as_secs() as i64)
}

/// True iff any file under `dir` whose name matches `pattern` (recursive, like
/// `find -type f -name PAT`) was modified within the idle window measured from
/// `now`. Missing/unreadable/empty dir -> false.
///
/// ```text
///   secs        = idle_after_sec
///   mins        = max(1, ceil(secs / 60))           // ceil to whole minutes, floor 1
///   window_secs = mins * 60
///   active iff exists matching file with (now - mtime_secs) < window_secs   // STRICT <
/// ```
///
/// We use STRICT `<` (the BSD `find -mmin -N` selects files strictly newer than
/// N minutes). Short-circuits on the first matching recent file (mirrors
/// `find ... -print -quit`). Fully recursive (no maxdepth), mirroring `find`.
pub fn is_active(dir: &Path, pattern: AgentGlob, idle_after_sec: u32, now: i64) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let window = window_secs(idle_after_sec);
    for entry in WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str() else {
            continue;
        };
        if !pattern.matches(name) {
            continue;
        }
        if let Some(m) = mtime_secs(entry.path())
            && (now - m) < window
        {
            return true; // short-circuit, like find -print -quit
        }
    }
    false
}

/// Newest matching file mtime (unix secs) under `dir`, or None.
pub fn latest_activity_mtime(dir: &Path, pattern: AgentGlob) -> Option<i64> {
    if !dir.is_dir() {
        return None;
    }
    let mut latest: Option<i64> = None;
    for entry in WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str() else {
            continue;
        };
        if !pattern.matches(name) {
            continue;
        }
        if let Some(m) = mtime_secs(entry.path()) {
            latest = Some(latest.map_or(m, |l| l.max(m)));
        }
    }
    latest
}

/// `now - latest_mtime`, or None when no matching file (diagnostics).
pub fn latest_activity_age_secs(dir: &Path, pattern: AgentGlob, now: i64) -> Option<i64> {
    latest_activity_mtime(dir, pattern).map(|m| now - m)
}

/// Tri-state for status: None = dir absent; else Active/Idle from [`is_active`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    None,
    Active,
    Idle,
}

/// `vigil_agent_state`: `none` if dir absent, else active/idle.
pub fn agent_state(dir: &Path, pattern: AgentGlob, idle_after_sec: u32, now: i64) -> AgentState {
    if !dir.is_dir() {
        return AgentState::None;
    }
    if is_active(dir, pattern, idle_after_sec, now) {
        AgentState::Active
    } else {
        AgentState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_rounds_up_to_whole_minutes() {
        assert_eq!(window_secs(300), 300); // 5 min
        assert_eq!(window_secs(1), 60); // ceil(1/60)=1 -> 60
        assert_eq!(window_secs(0), 60); // floored at 1
        assert_eq!(window_secs(61), 120); // ceil(61/60)=2 -> 120
    }

    #[test]
    fn glob_matchers() {
        assert!(Agent::Claude.pattern().matches("abc.jsonl"));
        assert!(!Agent::Claude.pattern().matches("abc.txt"));
        assert!(Agent::Codex.pattern().matches("rollout-x.jsonl"));
        assert!(!Agent::Codex.pattern().matches("other-x.jsonl"));
        assert!(Agent::Copilot.pattern().matches("events.jsonl"));
        assert!(!Agent::Copilot.pattern().matches("notes.txt"));
    }
}
