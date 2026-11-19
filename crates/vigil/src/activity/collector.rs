//! Read-only collection shared by daemon, recovery, status and debug.
use super::{
    claude, codex, copilot, jsonl,
    model::*,
    scan::{self, Agent},
    text,
};
use crate::config::VigilConfig;
use crate::procscan::{AgentKind, ProcRecord, ProcScanner, detect_line};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Default)]
struct History {
    parent_revision: u64,
    reader: jsonl::Reader,
    claude: claude::Claude,
    codex: codex::Codex,
    copilot: copilot::Copilot,
}
impl History {
    fn read(&mut self, path: &Path, provider: Agent, budget: &mut usize) -> usize {
        let Self {
            reader,
            claude,
            codex,
            copilot,
            ..
        } = self;
        let reset = std::cell::Cell::new(false);
        let result = reader.read(
            path,
            budget,
            || reset.set(true),
            |v| {
                if reset.replace(false) {
                    *claude = Default::default();
                    *codex = Default::default();
                    *copilot = Default::default();
                }
                match provider {
                    Agent::Claude => claude.event(&v),
                    Agent::Codex => codex.event(&v),
                    Agent::Copilot => copilot.event(&v),
                }
            },
        );
        if reset.get() {
            *claude = Default::default();
            *codex = Default::default();
            *copilot = Default::default();
        }
        match result {
            Ok(n) => n,
            Err(_) => {
                reader.gap = true;
                0
            }
        }
    }
    fn incomplete(&self) -> bool {
        self.reader.gap || !self.reader.caught_up
    }
}
#[derive(Default)]
pub struct ActivityCollector {
    histories: BTreeMap<PathBuf, History>,
    witnesses: BTreeMap<(u32, i64), (Agent, String)>,
    parents: BTreeMap<String, String>,
    associated_paths: BTreeMap<(u32, i64), Vec<PathBuf>>,
}
fn provider(r: &ProcRecord) -> Option<Agent> {
    if !r.same_user || r.zombie || r.start_time <= 0 {
        return None;
    }
    match detect_line(r.pid, &r.exe, &r.cmdline)?.kind {
        AgentKind::CliClaude => Some(Agent::Claude),
        AgentKind::CliCodex | AgentKind::AppCodex => Some(Agent::Codex),
        AgentKind::CliCopilot => Some(Agent::Copilot),
        _ => None,
    }
}
fn native_json(path: &Path) -> Option<Value> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let m = file.metadata().ok()?;
    if !m.is_file() || m.len() > 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn safe_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 200
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}
fn owned_path(path: &Path, root: &Path) -> bool {
    canonical(path).starts_with(canonical(root))
}
fn proc_start(v: &Value) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(text(v, "procStart"), "%a %b %e %H:%M:%S %Y")
        .ok()
        .map(|t| t.and_utc().timestamp())
}
/// One bounded read-only open-file query. Failure is not an empty association.
fn open_files(pid: u32) -> Result<Vec<PathBuf>, ()> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/sbin/lsof")
        .args(["-n", "-P", "-a", "-p", &pid.to_string(), "-F", "n"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    // Drain concurrently so a full pipe cannot block lsof before exit.
    let mut stdout = child.stdout.take().ok_or(())?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .by_ref()
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes);
        (result, bytes)
    });
    let deadline = Instant::now() + Duration::from_millis(350);
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let (result, bytes) = reader.join().map_err(|_| ())?;
    if !ok || result.is_err() || bytes.len() > 2 * 1024 * 1024 {
        return Err(());
    }
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|s| s.strip_prefix('n'))
        .map(PathBuf::from)
        .collect())
}
fn nearest_owner<'a>(
    r: &ProcRecord,
    records: &'a BTreeMap<u32, &ProcRecord>,
) -> Option<&'a ProcRecord> {
    let mut next = r.parent;
    let mut visited = BTreeSet::new();
    while let Some(pid) = next {
        if !visited.insert(pid) {
            return None;
        }
        let p = *records.get(&pid)?;
        if provider(p).is_some() {
            return Some(p);
        }
        next = p.parent;
    }
    None
}
fn ancestor_witness(
    r: &ProcRecord,
    records: &BTreeMap<u32, &ProcRecord>,
    witnesses: &BTreeMap<(u32, i64), (Agent, String)>,
) -> Option<(Agent, String)> {
    let mut next = r.parent;
    let mut visited = BTreeSet::new();
    while let Some(pid) = next {
        if !visited.insert(pid) {
            return None;
        }
        let p = *records.get(&pid)?;
        if provider(p).is_some() {
            return None;
        }
        if let Some(owner) = witnesses.get(&(p.pid, p.start_time)) {
            return Some(owner.clone());
        }
        next = p.parent;
    }
    None
}
fn companion_bridge(argv: &[OsString]) -> bool {
    let script = match argv {
        [_, script] => script,
        [_, flag, loader, script]
            if flag == "--import"
                && Path::new(loader).ends_with("hooks/launch-agent-bridge.mjs") =>
        {
            script
        }
        _ => return false,
    };
    let script = Path::new(script);
    script.ends_with("bridge-server/server.mjs")
        && script.iter().any(|part| part == "agent-companion")
}
fn command_candidate(r: &ProcRecord, argv: &[OsString]) -> bool {
    let name = crate::procscan::detect::process_name(&r.exe, &r.cmdline);
    // These processes are provider infrastructure, not work. Other processes
    // still require a native marker, validated nearest host and identity.
    ![
        "caffeinate",
        "codex",
        "codex-code-mode-host",
        "claude",
        "copilot",
        "vigil",
        "vigil-root-helper",
        "vigil-lock-helper",
    ]
    .contains(&name)
        && !(name == "node" && companion_bridge(argv))
        && ![
            "node_repl",
            "exec-server",
            "mcp-server",
            "language-server",
            "/Helper",
            "crashpad",
        ]
        .iter()
        .any(|s| r.cmdline.contains(s))
}
impl ActivityCollector {
    pub fn collect(
        &mut self,
        cfg: &VigilConfig,
        scanner: &mut ProcScanner,
        records: &[ProcRecord],
        now: i64,
    ) -> ActivitySnapshot {
        self.collect_with_files(cfg, scanner, records, now, open_files)
    }
    fn collect_with_files(
        &mut self,
        cfg: &VigilConfig,
        scanner: &mut ProcScanner,
        records: &[ProcRecord],
        now: i64,
        mut files: impl FnMut(u32) -> Result<Vec<PathBuf>, ()>,
    ) -> ActivitySnapshot {
        let mut snap = ActivitySnapshot::default();
        let by_pid: BTreeMap<_, _> = records.iter().map(|r| (r.pid, r)).collect();
        let hosts: Vec<_> = records
            .iter()
            .filter(|r| provider(r).is_some())
            .filter(|r| {
                // The occupied background worker has the same role as a spare.
                // A native registry entry distinguishes it; other read failures
                // remain eligible for explicit uncertainty.
                !(provider(r) == Some(Agent::Claude)
                    && crate::procscan::detect::process_role(&r.exe, &r.cmdline)
                        .split_whitespace()
                        .next()
                        == Some("bg-spare")
                    && std::fs::metadata(
                        Path::new(&cfg.claude_home)
                            .join("sessions")
                            .join(format!("{}.json", r.pid)),
                    )
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound))
            })
            .collect();
        snap.detected_hosts = hosts.len() as u32;
        snap.wrappers = crate::refcount::read_entries(Path::new(&cfg.active_dir))
            .iter()
            .filter(|e| {
                e.name == "wrapper"
                    && by_pid.get(&e.pid).is_some_and(|r| {
                        r.same_user
                            && !r.zombie
                            && crate::refcount::field(&e.body, "start_ts")
                                .and_then(|s| s.parse::<i64>().ok())
                                == Some(r.start_time)
                    })
            })
            .count() as u32;
        let host_budget = jsonl::EVALUATION_BYTES / hosts.len().max(1);
        let mut observed_paths = BTreeSet::new();
        let mut unidentified_providers = BTreeSet::new();
        let mut host_sessions: BTreeMap<u32, Vec<(Agent, String)>> = BTreeMap::new();
        for host in &hosts {
            let mut budget = host_budget;
            let mut association_unavailable = false;
            let agent = provider(host).unwrap();
            let root = Path::new(match agent {
                Agent::Claude => &cfg.claude_home,
                Agent::Codex => &cfg.codex_home,
                Agent::Copilot => &cfg.copilot_home,
            });
            let mut paths = Vec::new();
            let mut native = None;
            if agent == Agent::Claude {
                native = native_json(&root.join("sessions").join(format!("{}.json", host.pid)));
                if let Some(v) = &native {
                    let id = text(v, "sessionId");
                    if v["pid"].as_u64() != Some(host.pid as u64)
                        || proc_start(v) != Some(host.start_time)
                        || !safe_id(id)
                        || !["interactive", "bg"].contains(&text(v, "kind"))
                    {
                        native = None;
                    } else {
                        let project = text(v, "cwd")
                            .chars()
                            .map(|c| {
                                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                                    c
                                } else {
                                    '-'
                                }
                            })
                            .collect::<String>();
                        let p = root
                            .join("projects")
                            .join(project)
                            .join(format!("{id}.jsonl"));
                        if p.is_file() {
                            paths.push(p);
                        } else if let Ok(projects) = std::fs::read_dir(root.join("projects")) {
                            for project in projects.flatten() {
                                let p = project.path().join(format!("{id}.jsonl"));
                                if p.is_file() && owned_path(&p, root) {
                                    paths.push(p);
                                }
                            }
                        }
                    }
                }
            } else if agent == Agent::Copilot {
                let sessions = root.join("session-state");
                if let Ok(dirs) = std::fs::read_dir(sessions) {
                    for dir in dirs.flatten() {
                        let p = dir.path();
                        if p.join(format!("inuse.{}.lock", host.pid)).exists()
                            || p.join(format!("inuse.{}.hold", host.pid)).exists()
                        {
                            paths.push(p.join("events.jsonl"));
                        }
                    }
                }
            }
            if paths.is_empty() && native.is_none() || agent == Agent::Codex {
                match files(host.pid) {
                    Ok(files) => paths.extend(files.into_iter().filter(|p| {
                        owned_path(p, root)
                            && match agent {
                                Agent::Claude => {
                                    p.extension().is_some_and(|e| e == "jsonl")
                                        && !p.to_string_lossy().contains("/subagents/")
                                }
                                Agent::Codex => {
                                    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                                        n.starts_with("rollout-") && n.ends_with(".jsonl")
                                    })
                                }
                                Agent::Copilot => {
                                    p.file_name().is_some_and(|n| n == "events.jsonl")
                                }
                            }
                    })),
                    Err(_) => {
                        association_unavailable = true;
                        if let Some(previous) =
                            self.associated_paths.get(&(host.pid, host.start_time))
                        {
                            paths.extend(previous.iter().cloned());
                        }
                        unidentified_providers.insert(agent);
                    }
                }
            }
            paths.sort();
            paths.dedup();
            self.associated_paths
                .insert((host.pid, host.start_time), paths.clone());
            let recent_evidence = paths
                .iter()
                .any(|path| scan::recent_file(path, cfg.idle_after_sec, now));
            if agent == Agent::Claude
                && paths.is_empty()
                && let Some(v) = native.as_ref()
            {
                self.claude_session(&mut snap, root, v, None, &mut budget, &mut observed_paths);
            }
            let path_budget = budget / paths.len().max(1);
            for path in paths {
                let path = canonical(&path);
                observed_paths.insert(path.clone());
                let h = self.histories.entry(path.clone()).or_default();
                let mut available = path_budget.min(budget);
                let bytes = h.read(&path, agent, &mut available);
                budget -= bytes;
                snap.transcript_bytes += bytes;
                if agent == Agent::Claude {
                    if let Some(v) = native.as_ref() {
                        self.claude_session(
                            &mut snap,
                            root,
                            v,
                            Some(&path),
                            &mut budget,
                            &mut observed_paths,
                        );
                    } else {
                        if scan::recent_file(&path, cfg.idle_after_sec, now) {
                            snap.uncertain_providers.push(agent.token().to_owned());
                        }
                    }
                    continue;
                }
                let (id, owned_state, unfinished) = match agent {
                    Agent::Codex => {
                        let (state, work) = h.codex.owned_state(host.start_time);
                        (h.codex.id.clone(), state, work)
                    }
                    Agent::Copilot => {
                        let (state, work) = h.copilot.owned_state(host.start_time);
                        (h.copilot.id.clone(), state, work)
                    }
                    _ => unreachable!(),
                };
                let mut state = owned_state;
                if !safe_id(&id) {
                    if scan::recent_file(&path, cfg.idle_after_sec, now) {
                        snap.uncertain_providers.push(agent.token().to_owned());
                    }
                    continue;
                }
                if agent == Agent::Codex
                    && let Some(parent) = &h.codex.parent
                    && safe_id(parent)
                {
                    self.parents.insert(id.clone(), parent.clone());
                }
                // Old unfinished turns loaded by a new host do not regain ownership.
                let stale = match agent {
                    Agent::Codex => h.codex.turns.values().any(|start| *start < host.start_time),
                    Agent::Copilot => h
                        .copilot
                        .oldest_outstanding()
                        .is_some_and(|start| start < host.start_time),
                    _ => false,
                };
                if h.incomplete()
                    || association_unavailable
                    || (stale && matches!(owned_state, State::Idle | State::Waiting))
                {
                    state = State::Unknown;
                }
                if state == State::Unknown
                    && (!unfinished || matches!(owned_state, State::Idle | State::Waiting))
                    && scan::recent_file(&path, cfg.idle_after_sec, now)
                {
                    snap.uncertain_providers.push(agent.token().to_owned());
                }
                host_sessions
                    .entry(host.pid)
                    .or_default()
                    .push((agent, id.clone()));
                snap.insert(SessionActivity {
                    provider: agent.token().to_owned(),
                    id,
                    state,
                    evidence: "correlated-lifecycle".into(),
                    reason: if association_unavailable {
                        "host association probe unavailable"
                    } else if stale && state == State::Unknown {
                        "host changed; old turn ownership unverified"
                    } else if h.incomplete() {
                        "history incomplete"
                    } else if state == State::Unknown {
                        "unfinished tool; human wait unavailable"
                    } else {
                        "native session lifecycle"
                    }
                    .into(),
                    foreground_wait: None,
                    uncertain_demand: state == State::Unknown
                        && unfinished
                        && (owned_state == State::Unknown
                            || h.incomplete()
                            || association_unavailable),
                });
            }
            if agent == Agent::Claude
                && native.as_ref().is_some_and(|v| {
                    snap.sessions.iter().any(|s| {
                        s.provider == "claude"
                            && s.id == text(v, "sessionId")
                            && s.state == State::Unknown
                            && !s.uncertain_demand
                    })
                })
                && recent_evidence
            {
                snap.uncertain_providers.push("claude".into());
            }
            if agent == Agent::Claude
                && let Some(v) = native.as_ref()
            {
                host_sessions
                    .entry(host.pid)
                    .or_default()
                    .push((agent, text(v, "sessionId").to_owned()));
            }
            if !scanner.identity_matches(host.pid, host.start_time)
                && let Some(sessions) = host_sessions.get(&host.pid)
            {
                for (_, id) in sessions {
                    if let Some(s) = snap
                        .sessions
                        .iter_mut()
                        .find(|s| s.id == *id && s.provider == agent.token())
                    {
                        s.state = State::Unknown;
                        s.uncertain_demand = false;
                        s.reason = "host identity changed during collection".into();
                    }
                }
            }
            if !host_sessions.contains_key(&host.pid) {
                unidentified_providers.insert(agent);
            }
        }
        // Host order must not let an unidentified host borrow recency from a
        // resolved sibling. Classified unknowns already use their own evidence.
        for agent in unidentified_providers {
            let root = Path::new(match agent {
                Agent::Claude => &cfg.claude_home,
                Agent::Codex => &cfg.codex_home,
                Agent::Copilot => &cfg.copilot_home,
            });
            if scan::is_active_where(
                &scan::session_dir_from_provider_home(root, agent),
                agent.pattern(),
                cfg.idle_after_sec,
                now,
                |path| {
                    let path = canonical(path);
                    // A child transcript belongs to its observed Claude session even
                    // before the bounded reader reaches the launch in the parent.
                    if agent == Agent::Claude
                        && let Some(dir) = path.parent()
                        && dir.ends_with("subagents")
                        && let Some(session) = dir.parent()
                        && observed_paths.contains(&session.with_extension("jsonl"))
                    {
                        return false;
                    }
                    !observed_paths.contains(&path)
                },
            ) {
                snap.uncertain_providers.push(agent.token().to_owned());
            }
        }
        // Witnesses are in memory only. An absent identity must be verified as
        // exited/reused; a failed process probe is uncertainty, never idle.
        let mut witness_unknown = BTreeSet::new();
        self.witnesses.retain(|(pid, start), _| {
            if let Some(r) = by_pid.get(pid) {
                return r.start_time == *start && !r.zombie;
            }
            if scanner.identity_matches(*pid, *start) {
                return true;
            }
            if crate::refcount::pid_alive(*pid) {
                witness_unknown.insert((*pid, *start));
                true
            } else {
                false
            }
        });
        let candidates: Vec<_> = records
            .iter()
            .filter(|r| {
                r.same_user
                    && !r.zombie
                    && command_candidate(r, scanner.argv(r.pid))
                    && (nearest_owner(r, &by_pid).is_some()
                        || ancestor_witness(r, &by_pid, &self.witnesses).is_some()
                        || r.parent == Some(1))
            })
            .map(|r| r.pid)
            .collect();
        let markers = scanner.session_markers(&candidates);
        for pid in candidates {
            let r = by_pid[&pid];
            if let Some(owner) = ancestor_witness(r, &by_pid, &self.witnesses)
                && scanner.identity_matches(r.pid, r.start_time)
            {
                self.witnesses.insert((r.pid, r.start_time), owner);
                continue;
            }
            let Some(owner) = nearest_owner(r, &by_pid) else {
                if let Some(values) = markers.get(&pid) {
                    for (agent, id) in values {
                        if safe_id(id) && !self.witnesses.contains_key(&(r.pid, r.start_time)) {
                            snap.insert(SessionActivity {
                                provider: agent.token().into(),
                                id: id.clone(),
                                state: State::Unknown,
                                evidence: "unattributed-local-process".into(),
                                reason: "cold orphan lacks validated ownership witness".into(),
                                foreground_wait: None,
                                uncertain_demand: false,
                            });
                        }
                    }
                }
                continue;
            };
            let agent = provider(owner).unwrap();
            let Some(sessions) = host_sessions.get(&owner.pid) else {
                continue;
            };
            let marker = markers.get(&pid).and_then(|m| m.get(&agent));
            let matched =
                marker.and_then(|id| sessions.iter().find(|(a, s)| *a == agent && s == id));
            // Copilot children use internal markers; require a unique validated
            // host session plus outstanding native work before admitting it.
            let matched = matched.or_else(|| {
                if agent == Agent::Copilot
                    && marker.is_some()
                    && sessions.len() == 1
                    && snap
                        .sessions
                        .iter()
                        .any(|s| s.id == sessions[0].1 && s.state == State::Working)
                {
                    sessions.first()
                } else {
                    None
                }
            });
            let identity_valid = scanner.identity_matches(r.pid, r.start_time);
            if matched.is_none() || !identity_valid {
                for (_, id) in sessions {
                    if let Some(s) = snap
                        .sessions
                        .iter_mut()
                        .find(|s| s.id == *id && s.provider == agent.token())
                        && s.state != State::Working
                    {
                        s.state = State::Unknown;
                        s.reason = if marker.is_none() {
                            "owned descendant marker unavailable"
                        } else {
                            "descendant session attribution ambiguous"
                        }
                        .into();
                    }
                }
                if self
                    .associated_paths
                    .get(&(owner.pid, owner.start_time))
                    .is_some_and(|paths| {
                        paths
                            .iter()
                            .any(|path| scan::recent_file(path, cfg.idle_after_sec, now))
                    })
                {
                    snap.uncertain_providers.push(agent.token().into());
                }
            }
            if let Some((agent, id)) = matched
                && identity_valid
            {
                self.witnesses
                    .insert((r.pid, r.start_time), (*agent, id.clone()));
            }
        }
        for (identity, (agent, id)) in &self.witnesses {
            let unavailable = witness_unknown.contains(identity);
            snap.insert(SessionActivity {
                provider: agent.token().into(),
                id: id.clone(),
                state: if unavailable {
                    State::Unknown
                } else {
                    State::Working
                },
                evidence: "verified-local-process".into(),
                reason: if unavailable {
                    "owned command probe unavailable"
                } else {
                    "owned command alive"
                }
                .into(),
                foreground_wait: snap
                    .sessions
                    .iter()
                    .find(|s| s.id == *id && s.provider == agent.token())
                    .and_then(|s| s.foreground_wait.clone()),
                uncertain_demand: unavailable,
            });
        }
        let sessions = std::mem::take(&mut snap.sessions);
        for mut session in sessions {
            if session.provider == "codex" {
                let mut seen = BTreeSet::new();
                while let Some(parent) = self.parents.get(&session.id) {
                    if !seen.insert(session.id.clone()) {
                        session.state = State::Unknown;
                        session.reason = "child ownership cycle".into();
                        break;
                    }
                    session.id = parent.clone();
                }
            }
            snap.insert(session);
        }
        self.histories.retain(|p, _| observed_paths.contains(p));
        self.associated_paths.retain(|(pid, start), _| {
            by_pid
                .get(pid)
                .is_some_and(|r| r.start_time == *start && !r.zombie)
        });
        snap.sessions
            .sort_by(|a, b| (&a.provider, &a.id).cmp(&(&b.provider, &b.id)));
        snap.uncertain_providers.sort();
        snap.uncertain_providers.dedup();
        snap
    }
    fn claude_session(
        &mut self,
        snap: &mut ActivitySnapshot,
        root: &Path,
        v: &Value,
        path: Option<&Path>,
        budget: &mut usize,
        observed: &mut BTreeSet<PathBuf>,
    ) {
        let id = text(v, "sessionId");
        let status = text(v, "status");
        let wait = status == "waiting";
        let mut state = match status {
            "busy" | "shell" => State::Working,
            "waiting" => State::Waiting,
            "idle" => State::Idle,
            _ => State::Unknown,
        };
        let mut unresolved = false;
        let mut stale_child = false;
        let mut incomplete = path.is_none();
        if path.is_none() && state != State::Working {
            state = State::Unknown;
        }

        if let Some(path) = path {
            let children: Vec<_> = self.histories[path]
                .claude
                .children
                .keys()
                .cloned()
                .collect();
            let child_budget = *budget / children.len().max(1);
            let mut incomplete_children = BTreeSet::new();
            for agent in children {
                if !safe_id(&agent) {
                    incomplete = true;
                    continue;
                }
                let child_path = path
                    .with_extension("")
                    .join("subagents")
                    .join(format!("agent-{agent}.jsonl"));
                if child_path.exists() && owned_path(&child_path, root) {
                    let child_path = canonical(&child_path);
                    observed.insert(child_path.clone());
                    let mut child = self.histories.remove(&child_path).unwrap_or_default();
                    // Sidechain events must reduce against the parent's invocation.
                    let parent = self.histories.get_mut(path).unwrap();
                    if child.parent_revision != parent.reader.revision {
                        child.reader = Default::default();
                        child.parent_revision = parent.reader.revision;
                    }
                    let reset = std::cell::Cell::new(false);
                    let mut available = child_budget.min(*budget);
                    let n = child.reader.read(
                        &child_path,
                        &mut available,
                        || reset.set(true),
                        |event| {
                            if reset.replace(false) {
                                parent.claude.reset_child(&agent);
                            }
                            if text(&event, "sessionId") == id && text(&event, "agentId") == agent {
                                parent.claude.event(&event);
                            } else {
                                parent.claude.mark_child_gap(&agent);
                            }
                        },
                    );
                    if reset.get() {
                        incomplete_children.insert(agent.clone());
                    }
                    let bytes = n.unwrap_or(0);
                    *budget -= bytes;
                    snap.transcript_bytes += bytes;
                    if child.incomplete() {
                        incomplete_children.insert(agent.clone());
                    }
                    self.histories.insert(child_path, child);
                } else if self.histories[path]
                    .claude
                    .children
                    .get(&agent)
                    .is_some_and(|c| !c.finished)
                {
                    incomplete_children.insert(agent.clone());
                }
            }
            let h = &self.histories[path];
            let (child_state, owned_unfinished) = h.claude.owned_state(
                wait,
                proc_start(v).unwrap_or(i64::MAX),
                &incomplete_children,
            );
            stale_child = h
                .claude
                .oldest_unfinished()
                .is_some_and(|start| proc_start(v).is_none_or(|host_start| start < host_start));
            unresolved = owned_unfinished && (child_state == State::Unknown || h.incomplete());
            incomplete |= h.incomplete();
            if state != State::Working {
                if child_state == State::Working && !h.incomplete() {
                    state = State::Working;
                } else if child_state == State::Unknown || incomplete || stale_child {
                    state = State::Unknown;
                }
            }
        }
        let mut reason = "validated foreground state";
        if text(v, "kind") == "bg" {
            let job_id = text(v, "jobId");
            if safe_id(job_id) {
                let job_path = root.join("jobs").join(job_id);
                let job = native_json(&job_path.join("state.json"));
                let wake = native_json(&job_path.join(".pending-wakeup"));
                let wake_unreadable = job_path.join(".pending-wakeup").exists() && wake.is_none();
                let loop_state = job
                    .as_ref()
                    .map(|j| claude::loop_state(v, j, wake.as_ref()))
                    .unwrap_or(State::Unknown);
                let loop_state = if wake_unreadable {
                    State::Unknown
                } else {
                    loop_state
                };
                if loop_state == State::Working {
                    state = State::Working;
                    reason = "supported self-paced loop gap";
                } else if loop_state == State::Unknown && state != State::Working {
                    state = State::Unknown;
                    reason = "background wake ownership incomplete";
                }
            } else if state != State::Working {
                state = State::Unknown;
                reason = "background job identity missing";
            }
        }
        snap.insert(SessionActivity {
            provider: "claude".into(),
            id: id.into(),
            state,
            evidence: "native-session-state".into(),
            reason: if stale_child && state == State::Unknown {
                "host changed; old child ownership unverified"
            } else if incomplete {
                "history incomplete"
            } else if unresolved && state == State::Unknown {
                "child wait attribution unresolved"
            } else {
                reason
            }
            .into(),
            foreground_wait: wait.then(|| text(v, "waitingFor").to_owned()),
            uncertain_demand: state == State::Unknown && unresolved,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_infrastructure_is_not_a_workload_candidate() {
        let bridge = "/plugin cache/agent-companion/0.2.0/bridge-server/server.mjs";
        for (exe, argv, workload) in [
            ("node", vec!["node", bridge], false),
            (
                "node",
                vec![
                    "node",
                    "--import",
                    "/plugin cache/agent-companion/0.2.0/hooks/launch-agent-bridge.mjs",
                    bridge,
                ],
                false,
            ),
            (
                "codex-code-mode-host",
                vec!["codex-code-mode-host", "--listen", "stdio"],
                false,
            ),
            ("node", vec!["node", "build.mjs", bridge], true),
            (
                "node",
                vec!["node", "/project/bridge-server/server.mjs"],
                true,
            ),
            ("claude", vec!["ugrep", "-G", "pattern"], true),
        ] {
            let record = ProcRecord {
                pid: 42,
                parent: Some(41),
                start_time: 1,
                same_user: true,
                zombie: false,
                exe: exe.into(),
                cmdline: argv.join(" "),
            };
            let argv: Vec<_> = argv.into_iter().map(OsString::from).collect();
            assert_eq!(
                command_candidate(&record, &argv),
                workload,
                "{}",
                record.cmdline
            );
        }
    }

    #[test]
    fn synthetic_descriptor_failure_keeps_owned_turn_uncertain_without_recent_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg =
            crate::config::load(tmp.path().join("absent").to_str().unwrap(), None).unwrap();
        cfg.codex_home = tmp.path().to_string_lossy().into_owned();
        cfg.active_dir = tmp.path().join("active").to_string_lossy().into_owned();
        let now = chrono::Utc::now().timestamp();
        let pid = std::process::id();
        let start = ProcScanner::process_start(pid).unwrap();
        let records = [ProcRecord {
            pid,
            start_time: start,
            parent: None,
            same_user: true,
            zombie: false,
            exe: "/fixture/codex".into(),
            cmdline: "/fixture/codex app-server".into(),
        }];
        let path = tmp.path().join("rollout-native.jsonl");
        std::fs::write(&path,format!("{}\n{}\n",json!({"type":"session_meta","payload":{"id":"native-session"}}),json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"turn","started_at":now}}))).unwrap();
        let mtime = std::time::UNIX_EPOCH + Duration::from_secs((now - 3600) as u64);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(mtime))
            .unwrap();
        let mut collector = ActivityCollector::default();
        let mut scanner = ProcScanner::new();
        let first = collector.collect_with_files(&cfg, &mut scanner, &records, now, |_| {
            Ok(vec![path.clone()])
        });
        assert_eq!(first.working(), 1);
        let next = collector.collect_with_files(&cfg, &mut scanner, &records, now, |_| Err(()));
        assert_eq!(next.working(), 0);
        assert_eq!(next.unknown(), 1);
        assert_eq!(next.uncertain(), 1);
        assert_eq!(next.transcript_bytes, 0);
        let unloaded =
            collector.collect_with_files(&cfg, &mut scanner, &records, now, |_| Ok(Vec::new()));
        assert_eq!(unloaded.demand(), 0);
    }
}
