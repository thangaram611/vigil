use super::{model::State, text};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default, Clone)]
pub struct Child {
    pub tool: String,
    pub birth: i64,
    pub generation: bool,
    pub calls: BTreeSet<String>,
    pub finished: bool,
    pub gap: bool,
    pub ambiguous_invocation: bool,
}
#[derive(Default, Clone)]
pub struct Claude {
    pub children: BTreeMap<String, Child>,
    seen: BTreeSet<(String, String)>,
    pub gap: bool,
}
fn tag<'a>(content: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    content.split_once(&open)?.1.split_once(&close).map(|v| v.0)
}
impl Claude {
    pub fn event(&mut self, v: &Value) {
        let id = text(v, "uuid");
        if !id.is_empty()
            && !self
                .seen
                .insert((text(v, "agentId").to_owned(), id.to_owned()))
        {
            return;
        }
        if text(v, "type") == "queue-operation" {
            let content = text(v, "content");
            if !content.starts_with("<task-notification>") {
                return;
            }
            if let (Some(agent), Some(tool), Some(status)) = (
                tag(content, "task-id"),
                tag(content, "tool-use-id"),
                tag(content, "status"),
            ) && ["completed", "failed", "killed"].contains(&status)
                && let Some(child) = self.children.get_mut(agent)
            {
                // A reused invocation without a measured run correlation must
                // not let an old notification finish the new work.
                if child.ambiguous_invocation {
                    child.gap = true;
                    return;
                }
                if child.tool == tool {
                    child.finished = true;
                    child.generation = false;
                    child.calls.clear();
                } else {
                    self.gap = true;
                }
            }
            return;
        }
        let result = &v["toolUseResult"];
        let ambiguous_run = result.get("runId").is_some_and(|run| !run.is_null());
        if result["isAsync"] == true && text(result, "status") != "async_launched" {
            self.gap = true;
        }
        if result["isAsync"] == true && text(result, "status") == "async_launched" {
            let agent = text(result, "agentId");
            let tool = v["message"]["content"]
                .as_array()
                .and_then(|a| a.iter().find(|c| text(c, "type") == "tool_result"))
                .map(|c| text(c, "tool_use_id"))
                .unwrap_or("");
            if agent.is_empty() || tool.is_empty() {
                self.gap = true;
                return;
            }
            match self.children.get(agent) {
                Some(c) if c.tool == tool => {
                    // Exact event duplicates were handled above. Another
                    // launch reusing the same invocation IDs is ambiguous.
                    let child = self.children.get_mut(agent).unwrap();
                    child.finished = false;
                    child.gap = true;
                    child.ambiguous_invocation = true;
                    child.birth = child.birth.max(super::timestamp(v));
                }
                _ => {
                    self.children.insert(
                        agent.to_owned(),
                        Child {
                            tool: tool.to_owned(),
                            birth: super::timestamp(v),
                            generation: true,
                            gap: ambiguous_run,
                            ambiguous_invocation: ambiguous_run,
                            ..Default::default()
                        },
                    );
                }
            }
        }
        let agent = text(v, "agentId");
        if agent.is_empty() {
            return;
        }
        let Some(child) = self.children.get_mut(agent) else {
            self.gap = true;
            return;
        };
        if child.finished {
            return;
        }
        if let Some(content) = v["message"]["content"].as_array() {
            for c in content {
                match text(c, "type") {
                    "tool_use" => {
                        let id = text(c, "id");
                        if id.is_empty() {
                            child.gap = true;
                        } else {
                            child.calls.insert(id.to_owned());
                            child.generation = true;
                        }
                    }
                    "tool_result" => {
                        child.calls.remove(text(c, "tool_use_id"));
                    }
                    _ => {}
                }
            }
        }
        if text(&v["message"], "stop_reason") == "end_turn" {
            child.generation = false;
        }
    }
    pub fn reset_child(&mut self, agent: &str) {
        self.seen.retain(|(actor, _)| actor != agent);
        if let Some(child) = self.children.get_mut(agent) {
            child.calls.clear();
            child.gap = child.ambiguous_invocation;
            child.generation = !child.finished;
        }
    }
    pub fn mark_child_gap(&mut self, agent: &str) {
        if let Some(child) = self.children.get_mut(agent) {
            child.gap = true;
        }
    }
    pub fn child_state(&self, foreground_wait: bool) -> State {
        if self
            .children
            .values()
            .any(|c| !c.finished && !c.gap && c.generation && c.calls.is_empty())
        {
            return State::Working;
        }
        if self.gap || self.children.values().any(|c| !c.finished && c.gap) {
            return State::Unknown;
        }
        if self
            .children
            .values()
            .any(|c| !c.finished && (c.generation || !c.calls.is_empty() || c.gap))
        {
            if foreground_wait {
                State::Unknown
            } else {
                State::Working
            }
        } else {
            State::Idle
        }
    }
    pub fn oldest_unfinished(&self) -> Option<i64> {
        self.children
            .values()
            .filter(|c| !c.finished && (c.generation || !c.calls.is_empty() || c.gap))
            .map(|c| c.birth)
            .min()
    }
    pub fn owned_state(
        &self,
        wait: bool,
        start: i64,
        incomplete: &BTreeSet<String>,
    ) -> (State, bool) {
        let mut owned = Self {
            children: self.children.clone(),
            gap: self.gap,
            ..Default::default()
        };
        owned.children.retain(|_, child| child.birth >= start);
        for (agent, child) in &mut owned.children {
            if incomplete.contains(agent) {
                child.gap = true;
            }
        }
        (owned.child_state(wait), owned.unfinished())
    }
    pub fn unfinished(&self) -> bool {
        self.children
            .values()
            .any(|c| !c.finished && (c.generation || !c.calls.is_empty() || c.gap))
    }
}
/// Both live-job wake signals must describe the same self-paced chain.
pub fn loop_state(session: &Value, job: &Value, wake: Option<&Value>) -> State {
    if text(session, "kind") != "bg" {
        return State::Idle;
    }
    if text(session, "sessionId") != text(job, "sessionId") {
        return State::Unknown;
    }
    let at = job["inFlight"]["wake"]["at"].as_i64();
    match (at, wake) {
        (None, None) => State::Idle,
        (Some(at), Some(w))
            if w["scheduledFor"].as_i64() == Some(at)
                && !text(w, "toolUseId").is_empty()
                && w["chainStartedAt"].as_i64().is_some_and(|chain| {
                    chrono::NaiveDateTime::parse_from_str(
                        text(session, "procStart"),
                        "%a %b %e %H:%M:%S %Y",
                    )
                    .ok()
                    .is_some_and(|start| chain >= start.and_utc().timestamp() * 1000)
                }) =>
        {
            State::Working
        }
        _ => State::Unknown,
    }
}
