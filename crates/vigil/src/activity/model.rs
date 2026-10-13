use super::scan::Agent;
use serde::Serialize;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Working,
    Waiting,
    Idle,
    Unknown,
}
impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Idle => "idle",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct SessionActivity {
    pub provider: String,
    pub id: String,
    pub state: State,
    pub evidence: String,
    pub reason: String,
    pub foreground_wait: Option<String>,
    pub uncertain_demand: bool,
}
#[derive(Debug, Default, Clone, Serialize)]
pub struct ActivitySnapshot {
    pub sessions: Vec<SessionActivity>,
    pub detected_hosts: u32,
    pub wrappers: u32,
    pub vscode: bool,
    pub uncertain_providers: Vec<String>,
    pub transcript_bytes: usize,
}
impl ActivitySnapshot {
    fn count(&self, s: State) -> u32 {
        self.sessions.iter().filter(|v| v.state == s).count() as u32
    }
    pub fn working(&self) -> u32 {
        self.count(State::Working)
    }
    pub fn waiting(&self) -> u32 {
        self.count(State::Waiting)
    }
    pub fn idle(&self) -> u32 {
        self.count(State::Idle)
    }
    pub fn unknown(&self) -> u32 {
        self.count(State::Unknown)
    }
    pub fn uncertain(&self) -> u32 {
        let mut providers = self.uncertain_providers.clone();
        providers.extend(
            self.sessions
                .iter()
                .filter(|s| s.uncertain_demand)
                .map(|s| s.provider.clone()),
        );
        providers.sort();
        providers.dedup();
        providers.len() as u32
    }
    pub fn demand(&self) -> u32 {
        self.working() + self.uncertain() + self.wrappers + u32::from(self.vscode)
    }
    pub fn provider_state(&self, a: Agent) -> &'static str {
        let sessions: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| s.provider == a.token())
            .collect();
        if sessions.iter().any(|s| s.state == State::Working) {
            "working"
        } else if sessions.iter().any(|s| s.state == State::Unknown)
            || self.uncertain_providers.iter().any(|s| s == a.token())
        {
            "unknown"
        } else if sessions.iter().any(|s| s.state == State::Waiting) {
            "waiting"
        } else if !sessions.is_empty() {
            "idle"
        } else {
            "none"
        }
    }
    pub(crate) fn insert(&mut self, s: SessionActivity) {
        if let Some(old) = self
            .sessions
            .iter_mut()
            .find(|v| v.provider == s.provider && v.id == s.id)
        {
            let rank = |s: State| match s {
                State::Working => 3,
                State::Unknown => 2,
                State::Waiting => 1,
                State::Idle => 0,
            };
            if old.state == State::Unknown {
                old.uncertain_demand |= s.uncertain_demand;
            }
            if old.foreground_wait.is_none() {
                old.foreground_wait = s.foreground_wait.clone();
            }
            if rank(s.state) > rank(old.state) {
                *old = s;
            }
        } else {
            self.sessions.push(s);
        }
    }
}
