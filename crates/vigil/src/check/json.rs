//! `status --json` emitter — Phase 5.7 §5.1/§5.2.
//!
//! HAND-EMITS the object in the FROZEN key order. Does NOT derive `Serialize`
//! (whose field order/escaping could drift). The output (MINUS the prepended
//! `"version": 1,` first line) is byte-identical to the captured bash golden
//! `tests/golden/status_clean.json`.

use std::fmt::Write as _;

use super::StatusSnapshot;
use crate::power::assertions::Assertion;

/// The schema version (decision Q2). Prepended as the new FIRST `--json` key with
/// a trailing comma. Bump on any key add/remove/reorder.
pub const STATUS_JSON_VERSION: u32 = 1;

/// Escape a string with the bash `vigil_json_escape` semantics: `\` → `\\`,
/// `"` → `\"`, TAB → `\t`, CR → `\r`, LF → `\n` — IN THAT ORDER (the `\`-first
/// order matters so the inserted backslashes are not re-escaped). No other
/// control chars are touched (the operational strings are short ASCII).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

/// `Option<T: Display>` → `null` or the value. Used for nullable numeric keys.
fn opt_num<T: std::fmt::Display>(v: Option<T>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "null".to_string(),
    }
}

/// Render the `agents` sub-object (§5.2) — fixed 4 keys, closed enums (no escape).
fn agents_obj(s: &StatusSnapshot) -> String {
    format!(
        "{{\"claude\":\"{}\",\"codex\":\"{}\",\"copilot\":\"{}\",\"vscode_copilot_chat\":\"{}\"}}",
        s.agent_claude.as_str(),
        s.agent_codex.as_str(),
        s.agent_copilot.as_str(),
        s.agent_vscode_copilot_chat.as_str(),
    )
}

/// Render the `provider_roots` sub-object (§5.1 key 10) — claude/codex/copilot in
/// order, each `{home,session_dir,exists,latest_activity_age_secs}`.
fn provider_roots_obj(s: &StatusSnapshot) -> String {
    let one = |name: &str, p: &super::ProviderRoot| {
        format!(
            "\"{}\":{{\"home\":\"{}\",\"session_dir\":\"{}\",\"exists\":{},\"latest_activity_age_secs\":{}}}",
            name,
            json_escape(&p.home),
            json_escape(&p.session_dir),
            p.exists,
            opt_num(p.latest_activity_age_secs),
        )
    };
    format!(
        "{{{},{},{}}}",
        one("claude", &s.provider_claude),
        one("codex", &s.provider_codex),
        one("copilot", &s.provider_copilot),
    )
}

/// Render the `power_assertions` array (§2.3.3). `[]` whenever the state ≠ `ok`
/// (the snapshot already empties the vec in that case).
fn assertions_arr(holders: &[Assertion]) -> String {
    let mut out = String::from("[");
    for (i, a) in holders.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"pid\":{},\"process\":\"{}\",\"type\":\"{}\",\"vigil\":{}}}",
            a.pid,
            json_escape(&a.process),
            json_escape(&a.atype),
            a.vigil,
        );
    }
    out.push(']');
    out
}

impl StatusSnapshot {
    /// Hand-emit the FROZEN `--json` object (§5.1). Opens `{\n`, closes `}\n`,
    /// two-space indent on every key, every key but the last (`power_assertions`)
    /// trailing-comma. Prepends `"version": 1,` as the first key.
    pub fn to_json(&self) -> String {
        let mut o = String::new();
        o.push_str("{\n");
        let _ = writeln!(o, "  \"version\": {STATUS_JSON_VERSION},");
        let _ = writeln!(o, "  \"launchd_loaded\": {},", self.launchd_loaded);
        let _ = writeln!(o, "  \"daemon_pid\": {},", opt_num(self.daemon_pid));
        let _ = writeln!(
            o,
            "  \"daemon_scan_state\": \"{}\",",
            json_escape(self.daemon_scan_state.as_str())
        );
        let _ = writeln!(
            o,
            "  \"daemon_scan_age_secs\": {},",
            opt_num(self.daemon_scan_age_secs)
        );
        let _ = writeln!(o, "  \"refcount_active\": {},", self.refcount_active);
        let _ = writeln!(o, "  \"refcount_total\": {},", self.refcount_total);
        let _ = writeln!(
            o,
            "  \"pending_active_matches\": {},",
            self.pending_active_matches
        );
        let _ = writeln!(
            o,
            "  \"idle_window_minutes\": {},",
            self.idle_window_minutes
        );
        let _ = writeln!(o, "  \"agents\": {},", agents_obj(self));
        let _ = writeln!(o, "  \"provider_roots\": {},", provider_roots_obj(self));
        let _ = writeln!(
            o,
            "  \"power_hold_mode\": \"{}\",",
            json_escape(&self.power_hold_mode)
        );
        let _ = writeln!(o, "  \"pmset_disablesleep\": {},", self.pmset_disablesleep);
        let _ = writeln!(o, "  \"baseline\": {},", opt_num(self.baseline));
        let _ = writeln!(o, "  \"caffeinate_pid\": {},", opt_num(self.caffeinate_pid));
        let _ = writeln!(o, "  \"caffeinate_alive\": {},", self.caffeinate_alive);
        let _ = writeln!(o, "  \"thermal\": \"{}\",", json_escape(&self.thermal));
        let _ = writeln!(o, "  \"battery\": \"{}\",", json_escape(&self.battery));
        let _ = writeln!(o, "  \"power_helper_ok\": {},", self.power_helper_ok);
        let _ = writeln!(
            o,
            "  \"power_assertions_state\": \"{}\",",
            json_escape(&self.power_assertions_state)
        );
        // Last key — NO trailing comma.
        let _ = writeln!(
            o,
            "  \"power_assertions\": {}",
            assertions_arr(&self.power_assertions)
        );
        o.push_str("}\n");
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escape_bash_semantics() {
        // Backslash-first order: an input `\` must yield exactly `\\`, never
        // double-processed.
        assert_eq!(json_escape("a\\b"), "a\\\\b");
        assert_eq!(json_escape("a\"b"), "a\\\"b");
        assert_eq!(json_escape("a\tb"), "a\\tb");
        assert_eq!(json_escape("a\rb"), "a\\rb");
        assert_eq!(json_escape("a\nb"), "a\\nb");
        // Combined, order-sensitive.
        assert_eq!(json_escape("\\\t"), "\\\\\\t");
        // Plain ASCII untouched.
        assert_eq!(json_escape("ok 90%"), "ok 90%");
    }

    #[test]
    fn opt_num_renders_null() {
        assert_eq!(opt_num::<u32>(None), "null");
        assert_eq!(opt_num(Some(42u32)), "42");
        assert_eq!(opt_num(Some(-3i64)), "-3");
    }
}
