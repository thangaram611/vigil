use super::model::State;
use super::text;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default, Clone)]
pub struct Codex {
    pub id: String,
    pub parent: Option<String>,
    pub turns: BTreeMap<String, i64>,
    calls: BTreeMap<String, String>,
    seen: BTreeSet<String>,
    pub gap: bool,
}
impl Codex {
    pub fn event(&mut self, v: &Value) {
        let p = &v["payload"];
        match (text(v, "type"), text(p, "type")) {
            ("session_meta", _) => {
                self.id = text(p, "id").to_owned();
                let parent = p["parent_thread_id"].as_str().or_else(|| {
                    p["source"]["subagent"]["thread_spawn"]["parent_thread_id"].as_str()
                });
                self.parent = parent.map(str::to_owned);
                if self.id.is_empty() {
                    self.gap = true;
                }
            }
            ("event_msg", "task_started") => {
                let id = text(p, "turn_id");
                if id.is_empty() {
                    self.gap = true;
                    return;
                }
                let key = format!("start:{id}");
                if self.seen.insert(key) {
                    self.turns.insert(
                        id.to_owned(),
                        p["started_at"]
                            .as_i64()
                            .unwrap_or_else(|| super::timestamp(v)),
                    );
                }
            }
            ("event_msg", "task_complete" | "turn_aborted") => {
                let id = text(p, "turn_id");
                if id.is_empty() {
                    self.gap = true;
                } else {
                    self.turns.remove(id);
                    self.calls.retain(|_, t| t != id);
                }
            }
            ("response_item", "function_call" | "custom_tool_call") => {
                let id = text(p, "call_id");
                let turn = text(&p["internal_chat_message_metadata_passthrough"], "turn_id");
                if id.is_empty() {
                    self.gap = true;
                    return;
                }
                let turn = if turn.is_empty() && self.turns.len() == 1 {
                    self.turns.keys().next().unwrap().as_str()
                } else {
                    turn
                };
                if self.turns.contains_key(turn) {
                    self.calls.insert(id.to_owned(), turn.to_owned());
                } else if !self.turns.is_empty() {
                    self.gap = true;
                }
            }
            ("response_item", "function_call_output" | "custom_tool_call_output") => {
                self.calls.remove(text(p, "call_id"));
            }
            _ => {}
        }
    }
    pub fn state(&self) -> State {
        if self.gap || !self.calls.is_empty() {
            State::Unknown
        } else if !self.turns.is_empty() {
            State::Working
        } else {
            State::Idle
        }
    }
    pub fn unfinished(&self) -> bool {
        !self.turns.is_empty()
    }
    pub fn owned_state(&self, start: i64) -> (State, bool) {
        let mut owned = Self {
            turns: self.turns.clone(),
            calls: self.calls.clone(),
            gap: self.gap,
            ..Default::default()
        };
        owned.turns.retain(|_, birth| *birth >= start);
        owned.calls.retain(|_, turn| owned.turns.contains_key(turn));
        (owned.state(), owned.unfinished())
    }
}
