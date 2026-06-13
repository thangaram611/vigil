//! READ-ONLY: this module scans live state and prints. It MUST NOT write,
//! refresh, or GC any pid file or state file. The vscode flag is read by parsing
//! the existing state file (`active_until > now`) without rescanning.
//!
//! `vigil debug` is a native diagnostic dump of the detection data model. It
//! assembles: per-agent session state (pure scan), detected processes (one
//! scoped sysinfo refresh), and the current refcount (directory listing only).
//! No mutation of any kind.

use std::path::Path;

use serde::Serialize;

use crate::activity::scan::{self, Agent, AgentState};
use crate::activity::vscode::VscodeState;
use crate::config::VigilConfig;
use crate::procscan::{self, AgentMatch, ProcScanner};
use crate::refcount;

/// The hidden `vigil debug detect` oracle: pure detect over two ps text blobs,
/// rendered as byte-exact bash TSV rows. Used by `tests/detect_parity_test.sh`.
pub fn detect_oracle_rows(comm_text: &str, cmd_text: &str) -> Vec<String> {
    procscan::detect_all_text(comm_text, cmd_text)
        .iter()
        .map(procscan::agent_match_tsv)
        .collect()
}

/// One agent's session view (read-only).
#[derive(Debug, Serialize)]
pub struct AgentView {
    pub agent: String,
    pub session_dir: String,
    pub exists: bool,
    pub latest_activity_age_secs: Option<i64>,
    pub state: String,
}

/// One detected process row.
#[derive(Debug, Serialize)]
pub struct ProcessView {
    pub pid: u32,
    pub name: String,
    pub exe: String,
    pub args: String,
}

/// Refcount summary (read-only).
#[derive(Debug, Serialize)]
pub struct RefcountView {
    pub total: u32,
    pub filtered: u32,
    pub by_prefix: std::collections::BTreeMap<String, u32>,
}

/// The whole read-only dump model.
#[derive(Debug, Serialize)]
pub struct DebugDump {
    pub now: i64,
    pub agents: Vec<AgentView>,
    pub processes: Vec<ProcessView>,
    pub refcount: RefcountView,
}

fn state_str(s: AgentState) -> &'static str {
    match s {
        AgentState::None => "none",
        AgentState::Active => "active",
        AgentState::Idle => "idle",
    }
}

/// READ-ONLY: read the vscode flag by PARSING the existing state file and
/// comparing `active_until > now`. Does NOT rescan or rewrite (that is the
/// daemon's `chat_is_active`, deliberately not called here).
fn vscode_active_readonly(state_file: &Path, now: i64) -> bool {
    match std::fs::read_to_string(state_file) {
        Ok(text) => VscodeState::parse(&text).active_until > now,
        Err(_) => false,
    }
}

/// Assemble the read-only dump from a resolved config. No side effects: no
/// directory creation, no pid-file write/refresh/GC, no vscode rescan.
pub fn assemble(cfg: &VigilConfig, now: i64) -> DebugDump {
    // 1. Per-agent session state (pure scan over resolved provider homes).
    let agents = [
        (Agent::Claude, &cfg.claude_home),
        (Agent::Codex, &cfg.codex_home),
        (Agent::Copilot, &cfg.copilot_home),
    ];
    let mut agent_views = Vec::new();
    let mut claude_active = false;
    let mut codex_active = false;
    let mut copilot_active = false;
    for (agent, home) in agents {
        let dir = scan::session_dir_from_provider_home(Path::new(home), agent);
        let pattern = agent.pattern();
        let exists = dir.is_dir();
        let st = scan::agent_state(&dir, pattern, cfg.idle_after_sec, now);
        let age = scan::latest_activity_age_secs(&dir, pattern, now);
        match agent {
            Agent::Claude => claude_active = st == AgentState::Active,
            Agent::Codex => codex_active = st == AgentState::Active,
            Agent::Copilot => copilot_active = st == AgentState::Active,
        }
        agent_views.push(AgentView {
            agent: agent.token().to_string(),
            session_dir: dir.to_string_lossy().into_owned(),
            exists,
            latest_activity_age_secs: age,
            state: state_str(st).to_string(),
        });
    }

    // 2. Detected processes — ONE scoped sysinfo refresh.
    let detected: Vec<AgentMatch> = ProcScanner::new().detect();
    let processes: Vec<ProcessView> = detected
        .iter()
        .map(|m| ProcessView {
            pid: m.pid,
            name: m.kind.name().to_string(),
            exe: m.exe.clone(),
            args: m.args.clone(),
        })
        .collect();

    // 3. Refcount — directory listing only (read-only).
    let active_dir = Path::new(&cfg.active_dir);
    let total = refcount::count_total(active_dir);
    // vscode flag is derived READ-ONLY from the existing state file.
    let vscode_active = vscode_active_readonly(Path::new(&cfg.vscode_copilot_state_file), now);
    let filtered = refcount::count(
        active_dir,
        claude_active,
        codex_active,
        copilot_active,
        vscode_active,
    );
    let mut by_prefix = std::collections::BTreeMap::new();
    for e in refcount::read_entries(active_dir) {
        *by_prefix.entry(e.name).or_insert(0u32) += 1;
    }

    DebugDump {
        now,
        agents: agent_views,
        processes,
        refcount: RefcountView {
            total,
            filtered,
            by_prefix,
        },
    }
}

/// Render the dump PLUS the read-only power view. `json` -> a single
/// serde_json object combining the dump and the power view; else the three dump
/// table sections followed by a Power section.
///
/// READ-ONLY: the power view was produced by reading `pmset -g therm`/`-g ps`
/// (or fixtures) and parsing — no transition, no write. This preserves the
/// `vigil debug` read-only contract.
pub fn render_with_power(dump: &DebugDump, power: &crate::power_guard::PowerView, json: bool) {
    if json {
        // Combine dump + power into one object so the JSON dump stays a single
        // top-level value.
        #[derive(Serialize)]
        struct Combined<'a> {
            #[serde(flatten)]
            dump: &'a DebugDump,
            power: &'a crate::power_guard::PowerView,
        }
        let combined = Combined { dump, power };
        if let Err(e) = crate::output::print_json(&combined) {
            anstream::eprintln!("vigil: debug --json: {e}");
        }
        return;
    }

    // Non-JSON: the existing three sections, then a Power section.
    render(dump, false);
    render_power_section(power);
}

/// Render just the read-only Power section (thermal + battery) as a table.
fn render_power_section(power: &crate::power_guard::PowerView) {
    anstream::println!("Power");
    let mut t = crate::output::table(&["KEY", "VALUE"]);
    let thermal_floor = power
        .thermal_floor
        .map(|v| format!("{v}%"))
        .unwrap_or_else(|| "unset".to_string());
    let cpu_limit = power
        .cpu_scheduler_limit
        .map(|v| format!("{v}%"))
        .unwrap_or_else(|| "-".to_string());
    let batt_pct = power
        .battery_pct
        .map(|v| format!("{v}%"))
        .unwrap_or_else(|| "?".to_string());
    t.add_row(["thermal.unavailable", bool_str(power.thermal_unavailable)]);
    t.add_row([
        "thermal.warning_present",
        bool_str(power.thermal_warning_present),
    ]);
    t.add_row(["thermal.cpu_scheduler_limit", cpu_limit.as_str()]);
    t.add_row(["thermal.floor", thermal_floor.as_str()]);
    t.add_row(["thermal.summary", power.thermal_summary.as_str()]);
    t.add_row(["battery.power_source", power.power_source.as_str()]);
    t.add_row(["battery.pct", batt_pct.as_str()]);
    t.add_row([
        "battery.floor",
        format!("{}%", power.battery_floor_pct).as_str(),
    ]);
    t.add_row(["battery.summary", power.battery_summary.as_str()]);
    anstream::println!("{t}");
}

fn bool_str(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// Render the dump to stdout. `json` -> a single serde_json object; else three
/// table sections.
pub fn render(dump: &DebugDump, json: bool) {
    if json {
        if let Err(e) = crate::output::print_json(dump) {
            anstream::eprintln!("vigil: debug --json: {e}");
        }
        return;
    }

    // Agents section.
    anstream::println!("Agents");
    let mut t = crate::output::table(&["AGENT", "SESSION_DIR", "EXISTS", "AGE_SECS", "STATE"]);
    for a in &dump.agents {
        let age = a
            .latest_activity_age_secs
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());
        t.add_row([
            a.agent.as_str(),
            a.session_dir.as_str(),
            if a.exists { "yes" } else { "no" },
            age.as_str(),
            a.state.as_str(),
        ]);
    }
    anstream::println!("{t}");

    // Processes section.
    anstream::println!("Processes");
    let mut t = crate::output::table(&["PID", "NAME", "EXE", "ARGS"]);
    for p in &dump.processes {
        t.add_row([
            p.pid.to_string().as_str(),
            p.name.as_str(),
            p.exe.as_str(),
            p.args.as_str(),
        ]);
    }
    anstream::println!("{t}");

    // Refcount section.
    anstream::println!("Refcount");
    let mut t = crate::output::table(&["KEY", "VALUE"]);
    t.add_row(["total", dump.refcount.total.to_string().as_str()]);
    t.add_row(["filtered", dump.refcount.filtered.to_string().as_str()]);
    for (k, v) in &dump.refcount.by_prefix {
        t.add_row([format!("by_prefix.{k}").as_str(), v.to_string().as_str()]);
    }
    anstream::println!("{t}");
}
