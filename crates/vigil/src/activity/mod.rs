//! Session-scoped activity. Collection is read-only; lifecycle reducers are pure.
pub mod claude;
pub mod codex;
mod collector;
pub mod copilot;
pub mod jsonl;
pub mod model;
pub mod scan;
pub mod vscode;
pub use collector::ActivityCollector;
pub use model::{ActivitySnapshot, SessionActivity, State};
fn text<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn timestamp(v: &serde_json::Value) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text(v, "timestamp"))
        .map(|t| t.timestamp())
        .unwrap_or(0)
}
