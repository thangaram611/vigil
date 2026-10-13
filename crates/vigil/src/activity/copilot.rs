use super::{model::State, text};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default, Clone)]
struct Actor {
    turn: Option<(String, String)>,
    birth: i64,
    child: bool,
    calls: BTreeMap<String, Call>,
}
#[derive(Clone)]
struct Call {
    name: String,
    target: String,
    blocked: bool,
    asynchronous: bool,
    birth: i64,
}
#[derive(Default, Clone)]
pub struct Copilot {
    pub id: String,
    actors: BTreeMap<String, Actor>,
    permissions: BTreeMap<String, String>,
    agents: BTreeMap<String, String>,
    shells: BTreeSet<String>,
    seen: BTreeSet<String>,
    pub gap: bool,
    shell_birth: BTreeMap<String, i64>,
}
impl Copilot {
    pub fn event(&mut self, v: &Value) {
        let id = text(v, "id");
        if !id.is_empty() && !self.seen.insert(id.to_owned()) {
            return;
        }
        let d = &v["data"];
        let actor = text(d, "parentToolCallId").to_owned();
        match text(v, "type") {
            "session.start" => {
                self.id = text(d, "sessionId").to_owned();
                if self.id.is_empty() {
                    self.gap = true;
                }
            }
            "assistant.turn_start" => {
                let a = self.actors.entry(actor).or_default();
                if a.turn.is_some() {
                    self.gap = true;
                }
                a.birth = super::timestamp(v);
                a.turn = Some((text(d, "turnId").to_owned(), id.to_owned()));
            }
            "assistant.turn_end" => {
                if let Some(a) = self.actors.get_mut(&actor) {
                    if a.turn.as_ref().is_some_and(|t| t.0 == text(d, "turnId")) {
                        a.turn = None;
                    } else {
                        self.gap = true;
                    }
                } else {
                    self.gap = true;
                }
            }
            "tool.execution_start" => {
                let call = text(d, "toolCallId");
                if call.is_empty() {
                    self.gap = true;
                    return;
                }
                let name = text(d, "toolName");
                let args = &d["arguments"];
                let target = if name == "read_agent" {
                    text(args, "agent_id")
                } else {
                    text(args, "shellId")
                };
                self.actors.entry(actor).or_default().calls.insert(
                    call.to_owned(),
                    Call {
                        birth: super::timestamp(v),
                        name: name.to_owned(),
                        target: target.to_owned(),
                        blocked: name == "ask_user",
                        asynchronous: text(args, "mode") == "async",
                    },
                );
            }
            "tool.execution_complete" => {
                let call = text(d, "toolCallId");
                let mut removed = None;
                for a in self.actors.values_mut() {
                    if let Some(c) = a.calls.remove(call) {
                        removed = Some(c);
                        break;
                    }
                }
                if let Some(c) = removed {
                    if c.asynchronous && d["success"] == true {
                        let output = text(&d["result"], "content");
                        if let Some(shell) = output
                            .strip_prefix("<command started in background with shellId: ")
                            .and_then(|s| s.strip_suffix('>'))
                        {
                            self.shells.insert(shell.to_owned());
                            self.shell_birth.insert(shell.to_owned(), c.birth);
                        } else {
                            self.gap = true;
                        }
                    }
                    if c.name == "task" {
                        let native = text(&d["toolTelemetry"]["restrictedProperties"], "agent_id");
                        if !native.is_empty() {
                            self.agents.insert(native.to_owned(), call.to_owned());
                        }
                    }
                }
            }
            "subagent.started" => {
                let call = text(d, "toolCallId");
                if call.is_empty() {
                    self.gap = true;
                } else {
                    let actor = self.actors.entry(call.to_owned()).or_default();
                    actor.child = true;
                    actor.birth = super::timestamp(v);
                }
            }
            "subagent.completed" | "subagent.failed" => {
                let call = text(d, "toolCallId");
                if let Some(a) = self.actors.get_mut(call) {
                    a.child = false;
                    a.turn = None;
                    a.calls.clear();
                } else {
                    self.gap = true;
                }
            }
            "permission.requested" => {
                let call = text(&d["permissionRequest"], "toolCallId");
                let request = text(d, "requestId");
                if let Some(c) = self.actors.values_mut().find_map(|a| a.calls.get_mut(call)) {
                    c.blocked = true;
                    self.permissions.insert(request.to_owned(), call.to_owned());
                } else {
                    self.gap = true;
                }
            }
            "permission.completed" => {
                if let Some(call) = self.permissions.remove(text(d, "requestId")) {
                    if let Some(c) = self
                        .actors
                        .values_mut()
                        .find_map(|a| a.calls.get_mut(&call))
                    {
                        c.blocked = c.name == "ask_user";
                    }
                } else {
                    self.gap = true;
                }
            }
            "abort" => {
                if let Some(a) = self.actors.get_mut(&actor) {
                    a.turn = None;
                    a.calls.clear();
                }
                self.permissions
                    .retain(|_, c| self.actors.values().any(|a| a.calls.contains_key(c)));
            }
            "system.notification" if text(&d["kind"], "type") == "shell_completed" => {
                self.shells.remove(text(&d["kind"], "shellId"));
                self.shell_birth.remove(text(&d["kind"], "shellId"));
            }
            "session.shutdown" => {
                for a in self.actors.values_mut() {
                    a.turn = None;
                    a.calls.clear();
                    a.child = false;
                }
            }
            _ => {}
        }
    }
    fn actor_state(&self, key: &str, depth: usize) -> State {
        if depth > self.actors.len() {
            return State::Unknown;
        }
        let Some(a) = self.actors.get(key) else {
            return State::Unknown;
        };
        let mut waiting = false;
        let mut running = false;
        for c in a.calls.values() {
            if c.blocked {
                waiting = true;
            } else if c.name == "read_agent" {
                let target = self
                    .agents
                    .get(&c.target)
                    .map(String::as_str)
                    .unwrap_or(&c.target);
                match self.actor_state(target, depth + 1) {
                    State::Working => running = true,
                    State::Unknown => return State::Unknown,
                    _ => waiting = true,
                }
            } else if c.name == "read_bash" {
                if self.shells.contains(&c.target) {
                    running = true;
                } else {
                    waiting = true;
                }
            } else {
                running = true;
            }
        }
        if running {
            State::Working
        } else if waiting {
            State::Waiting
        } else if a.turn.is_some() || a.child {
            State::Working
        } else {
            State::Idle
        }
    }
    pub fn state(&self) -> State {
        if !self.shells.is_empty() {
            return State::Working;
        }
        let states: Vec<_> = self.actors.keys().map(|k| self.actor_state(k, 0)).collect();
        if states.contains(&State::Working) {
            State::Working
        } else if self.gap || states.contains(&State::Unknown) {
            State::Unknown
        } else if states.contains(&State::Waiting) {
            State::Waiting
        } else {
            State::Idle
        }
    }
    pub fn oldest_outstanding(&self) -> Option<i64> {
        self.actors
            .values()
            .flat_map(|a| {
                let mut starts = Vec::new();
                if a.turn.is_some() || a.child {
                    starts.push(a.birth);
                }
                starts.extend(a.calls.values().map(|c| c.birth));
                starts
            })
            .chain(self.shell_birth.values().copied())
            .min()
    }
    pub fn owned_state(&self, start: i64) -> (State, bool) {
        let mut owned = Self {
            actors: self.actors.clone(),
            agents: self.agents.clone(),
            shells: self.shells.clone(),
            shell_birth: self.shell_birth.clone(),
            gap: self.gap,
            ..Default::default()
        };
        for actor in owned.actors.values_mut() {
            if actor.birth < start {
                actor.turn = None;
                actor.child = false;
            }
            actor.calls.retain(|_, call| call.birth >= start);
        }
        owned.shells.retain(|shell| {
            owned
                .shell_birth
                .get(shell)
                .is_some_and(|birth| *birth >= start)
        });
        (owned.state(), owned.unfinished())
    }
    pub fn unfinished(&self) -> bool {
        self.actors
            .values()
            .any(|a| a.turn.is_some() || a.child || !a.calls.is_empty())
            || !self.shells.is_empty()
    }
}
