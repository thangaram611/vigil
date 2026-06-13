//! VS Code + GitHub Copilot Chat hash-gate — the Rust port of the
//! `vigil_vscode_copilot_chat_*` machinery in `lib/activity.sh`.
//!
//! Copilot Chat has no per-chat worker process; the signal is a content-hash
//! change of `workspaceStorage/*/chatEditingSessions/*/state.json`. Raw mtime is
//! noisy (VS Code rewrites the file while idle without changing content), so we
//! treat a SEMANTIC hash change as the activity event and cache an `active_until`.
//!
//! Split into a PURE transition core ([`vscode_transition`]) + a thin
//! file-format ([`VscodeState`]) + live IO wrappers (host probe, recent-file
//! collection, the read/write daemon path).
//!
//! ## State-file format — BYTE-IDENTICAL to bash `_vigil_vscode_write_state`
//! ```text
//! active_until\t<n>
//! last_scan\t<n>
//! primed\t<0|1>
//! file\t<sha256>\t<path>      (one per tracked file; scan order then retained order)
//! ```
//! Writer order: `active_until`, `last_scan`, `primed`, then file lines (current
//! scan's files first in discovery order, THEN retained old files), each with a
//! trailing newline. A 5.7 cutover reads either side.

use std::path::Path;

use sha2::{Digest, Sha256};

/// Parsed vscode-copilot state file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VscodeState {
    pub active_until: i64,
    pub last_scan: i64,
    /// stored as `0|1`.
    pub primed: bool,
    /// `(sha256, path)` per tracked file, in file order.
    pub files: Vec<(String, String)>,
}

impl VscodeState {
    /// Parse the TSV state text. Missing/blank lines fall back to defaults
    /// (`active_until=0, last_scan=0, primed=false, files=[]`). Mirrors the
    /// `awk -F '\t' '$1==k'` field extraction + the `file\t<sha>\t<path>` loop.
    pub fn parse(text: &str) -> VscodeState {
        let mut st = VscodeState::default();
        for line in text.lines() {
            let mut it = line.split('\t');
            match it.next() {
                Some("active_until") => {
                    if let Some(v) = it.next() {
                        st.active_until = parse_u_field(v);
                    }
                }
                Some("last_scan") => {
                    if let Some(v) = it.next() {
                        st.last_scan = parse_u_field(v);
                    }
                }
                Some("primed") => {
                    if let Some(v) = it.next() {
                        st.primed = v == "1";
                    }
                }
                Some("file") => {
                    let sha = it.next().unwrap_or("");
                    let path = it.next().unwrap_or("");
                    // bash keeps a file line only when both sha and path non-empty.
                    if !sha.is_empty() && !path.is_empty() {
                        st.files.push((sha.to_string(), path.to_string()));
                    }
                }
                _ => {}
            }
        }
        st
    }

    /// Serialize to the exact byte format above (trailing newline per line).
    pub fn serialize(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("active_until\t{}\n", self.active_until));
        s.push_str(&format!("last_scan\t{}\n", self.last_scan));
        s.push_str(&format!("primed\t{}\n", if self.primed { 1 } else { 0 }));
        for (sha, path) in &self.files {
            s.push_str(&format!("file\t{sha}\t{path}\n"));
        }
        s
    }

    /// `_vigil_vscode_state_hash_for_path`: the stored sha for a path, if any.
    pub fn hash_for_path(&self, path: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(_, p)| p == path)
            .map(|(sha, _)| sha.as_str())
    }
}

/// Parse a numeric state field the way the bash code validates it: only an
/// all-ASCII-digit value is accepted; anything else falls back to 0.
fn parse_u_field(v: &str) -> i64 {
    if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) {
        v.parse::<i64>().unwrap_or(0)
    } else {
        0
    }
}

/// One recent state-file observation: its current content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentFile {
    pub path: String,
    pub sha256: String,
}

/// PURE transition. Mirrors `vigil_vscode_copilot_chat_is_active` EXACTLY.
///
/// Given `prior` state, the CURRENT recent files (already hashed), `now`,
/// `idle_after_sec`, and `discover_secs`, compute the new state and whether the
/// host is currently active.
///
/// - **discover throttle**: if `now - last_scan < clamp(discover_secs, >=5)` →
///   return cached `(None, active_until > now)` WITHOUT rescanning/rewriting.
/// - **primed-first-run suppression**: a change only counts when `primed==true`
///   AND an old hash existed AND `old != new`. The first scan records hashes,
///   sets `primed=1`, does NOT set `active_until`.
/// - **mtime-only rewrite** (unchanged hash) → no change → stays idle.
/// - **retain** old hashes for paths NOT in the current recent set.
/// - **on change**: `active_until = now + idle_after_sec`.
/// - always rewrite state with `primed=1, last_scan=now` after a real scan.
///
/// Returns `(new_state, is_active)`. `new_state == None` means throttled
/// (caller must NOT write).
pub fn vscode_transition(
    prior: &VscodeState,
    current: &[RecentFile],
    now: i64,
    idle_after_sec: u32,
    discover_secs: u32,
) -> (Option<VscodeState>, bool) {
    // Clamp discover_secs to a floor of 5 (bash `(( discover_secs < 5 )) && =5`).
    let discover = discover_secs.max(5) as i64;

    // Throttle: return cached without rescanning.
    if now - prior.last_scan < discover {
        return (None, prior.active_until > now);
    }

    let mut changed = false;
    let mut files: Vec<(String, String)> = Vec::new();
    let mut current_paths: Vec<&str> = Vec::new();

    for rf in current {
        // A change counts only when primed AND an old hash existed AND differs.
        if prior.primed
            && let Some(old) = prior.hash_for_path(&rf.path)
            && old != rf.sha256
        {
            changed = true;
        }
        current_paths.push(rf.path.as_str());
        files.push((rf.sha256.clone(), rf.path.clone()));
    }

    // Retain hashes for prior-tracked paths not present in the current scan, so
    // a later mtime-only rewrite of an aged-out file does not falsely signal.
    for (old_sha, old_path) in &prior.files {
        if !current_paths.contains(&old_path.as_str()) {
            files.push((old_sha.clone(), old_path.clone()));
        }
    }

    let mut active_until = prior.active_until;
    if changed {
        active_until = now + idle_after_sec as i64;
    }

    let new_state = VscodeState {
        active_until,
        last_scan: now,
        primed: true,
        files,
    };
    let is_active = active_until > now;
    (Some(new_state), is_active)
}

/// SHA-256 of a byte slice, lower-hex (equal to `shasum -a 256 | awk '{print $1}'`).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// SHA-256 of a file's bytes, lower-hex; None if unreadable.
pub fn sha256_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha256_hex(&bytes))
}

/// Host running? `ps`-match on the Code main process.
///
/// Honors `VIGIL_VSCODE_PS_FIXTURE`: if the env var is SET (even to ""), use its
/// value as the ps text; else collect live process command lines via sysinfo.
/// Match: text contains `/Visual Studio Code.app/Contents/MacOS/` or
/// `/Visual Studio Code - Insiders.app/Contents/MacOS/`.
pub fn host_running(ps_text_override: Option<&str>) -> bool {
    let text = match ps_text_override {
        Some(t) => t.to_string(),
        // bash `_vigil_vscode_ps`: when `VIGIL_VSCODE_PS_FIXTURE` is SET (even to
        // ""), its value is the ps text; only when UNSET do we scan live. `var_os`
        // gives the `${VAR+set}` "present regardless of value" semantics exactly.
        None => match std::env::var_os("VIGIL_VSCODE_PS_FIXTURE") {
            Some(fixture) => fixture.to_string_lossy().into_owned(),
            None => {
                // Live: reconstruct `ps -axww -o command=` from sysinfo.
                let mut scanner = crate::procscan::ProcScanner::new();
                scanner
                    .collect()
                    .into_iter()
                    .map(|r| r.cmdline)
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        },
    };
    text.contains("/Visual Studio Code.app/Contents/MacOS/")
        || text.contains("/Visual Studio Code - Insiders.app/Contents/MacOS/")
}

/// The two workspaceStorage roots under a home dir.
fn workspace_roots(home: &Path) -> [std::path::PathBuf; 2] {
    [
        home.join("Library/Application Support/Code/User/workspaceStorage"),
        home.join("Library/Application Support/Code - Insiders/User/workspaceStorage"),
    ]
}

/// Collect recent `state.json` files under the two workspaceStorage roots,
/// recursive, matching `*/chatEditingSessions/*/state.json`, modified within
/// `recent_mins`, then sha256 each. Mirrors
/// `vigil_vscode_copilot_recent_state_files` (`find -maxdepth 6 -mmin -N`).
pub fn recent_state_files(home: &Path, recent_mins: u32, now: i64) -> Vec<RecentFile> {
    // recent_mins clamps to floor 1; non-numeric -> 10 (handled by config layer).
    let mins = recent_mins.max(1) as i64;
    let window = mins * 60;
    let mut out = Vec::new();
    for root in workspace_roots(home) {
        if !root.is_dir() {
            continue;
        }
        // maxdepth 6 from the root; the fixture file sits at depth 4.
        for entry in walkdir::WalkDir::new(&root)
            .max_depth(6)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.file_name().to_str() != Some("state.json") {
                continue;
            }
            // path glob `*/chatEditingSessions/*/state.json`: components relative
            // to root must be `<one>/chatEditingSessions/<one>/state.json`. bash
            // `find -path` lets `*` cross slashes, so it would also match deeper
            // nestings within maxdepth 6; VS Code only ever produces this exact
            // 4-component layout (`<hash>/chatEditingSessions/<session>/state.json`),
            // so we match it exactly rather than emulate cross-slash globbing.
            let Ok(rel) = entry.path().strip_prefix(&root) else {
                continue;
            };
            let comps: Vec<&str> = rel
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            if comps.len() != 4 || comps[1] != "chatEditingSessions" || comps[3] != "state.json" {
                continue;
            }
            // recent within `recent_mins` (whole-second mtime, strict <).
            let Some(m) = mtime_secs(entry.path()) else {
                continue;
            };
            if (now - m) >= window {
                continue;
            }
            if let Some(sha) = sha256_file(entry.path()) {
                out.push(RecentFile {
                    path: entry.path().to_string_lossy().into_owned(),
                    sha256: sha,
                });
            }
        }
    }
    out
}

/// Whole-second mtime (seconds since epoch) for a path, or None.
fn mtime_secs(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let dur = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(dur.as_secs() as i64)
}

/// Full live probe: read the state file, gate on `host_running`, run
/// [`vscode_transition`], and write the new state UNLESS throttled. Returns
/// `is_active`.
///
/// READ/WRITE — this is the daemon path. NOT used by `vigil debug` (read-only).
#[allow(clippy::too_many_arguments)]
pub fn chat_is_active(
    home: &Path,
    state_file: &Path,
    now: i64,
    idle_after_sec: u32,
    discover_secs: u32,
    recent_mins: u32,
    ps_override: Option<&str>,
) -> bool {
    if !host_running(ps_override) {
        return false;
    }
    let prior = match std::fs::read_to_string(state_file) {
        Ok(text) => VscodeState::parse(&text),
        Err(_) => VscodeState::default(),
    };

    // Pre-throttle check mirrors bash: when throttled we DON'T scan files.
    let discover = discover_secs.max(5) as i64;
    if now - prior.last_scan < discover {
        return prior.active_until > now;
    }

    let current = recent_state_files(home, recent_mins, now);
    let (new_state, is_active) =
        vscode_transition(&prior, &current, now, idle_after_sec, discover_secs);
    if let Some(st) = new_state {
        if let Some(parent) = state_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(state_file, st.serialize());
    }
    is_active
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serializes the two env-mutating tests in this binary (cargo runs tests in
    // parallel threads); only this test touches VIGIL_VSCODE_PS_FIXTURE.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn host_running_honors_ps_fixture_env() {
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: env access serialized by ENV_LOCK; the var is restored before
        // the guard drops, and no other test in this binary reads it.
        unsafe {
            std::env::set_var(
                "VIGIL_VSCODE_PS_FIXTURE",
                "/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Code - Insiders",
            );
        }
        assert!(
            host_running(None),
            "fixture env with a VS Code main path must signal host running"
        );
        // SET-but-non-matching must NOT fall through to a live scan (bash: env
        // SET wins over ps), so an unrelated process line yields not-running.
        unsafe { std::env::set_var("VIGIL_VSCODE_PS_FIXTURE", "/usr/bin/other-proc --flag") };
        assert!(
            !host_running(None),
            "fixture env without a VS Code main path must not signal host running"
        );
        unsafe { std::env::remove_var("VIGIL_VSCODE_PS_FIXTURE") };
    }

    #[test]
    fn parse_roundtrip_byte_identical() {
        let s = VscodeState {
            active_until: 100,
            last_scan: 50,
            primed: true,
            files: vec![("deadbeef".into(), "/x/state.json".into())],
        };
        let text = s.serialize();
        assert_eq!(
            text,
            "active_until\t100\nlast_scan\t50\nprimed\t1\nfile\tdeadbeef\t/x/state.json\n"
        );
        assert_eq!(VscodeState::parse(&text), s);
    }

    #[test]
    fn first_run_primes_without_active() {
        let prior = VscodeState::default();
        let current = vec![RecentFile {
            path: "/x".into(),
            sha256: "h1".into(),
        }];
        // last_scan=0, now large enough to pass throttle.
        let (new, active) = vscode_transition(&prior, &current, 1000, 300, 5);
        assert!(!active, "first run must not count active");
        let new = new.unwrap();
        assert!(new.primed);
        assert_eq!(new.active_until, 0);
    }

    #[test]
    fn hash_change_while_primed_counts() {
        let prior = VscodeState {
            active_until: 0,
            last_scan: 0,
            primed: true,
            files: vec![("h1".into(), "/x".into())],
        };
        let current = vec![RecentFile {
            path: "/x".into(),
            sha256: "h2".into(),
        }];
        let (new, active) = vscode_transition(&prior, &current, 1000, 300, 5);
        assert!(active);
        assert_eq!(new.unwrap().active_until, 1300);
    }

    #[test]
    fn unchanged_hash_stays_idle() {
        let prior = VscodeState {
            active_until: 0,
            last_scan: 0,
            primed: true,
            files: vec![("h1".into(), "/x".into())],
        };
        let current = vec![RecentFile {
            path: "/x".into(),
            sha256: "h1".into(),
        }];
        let (_, active) = vscode_transition(&prior, &current, 1000, 300, 5);
        assert!(!active);
    }

    #[test]
    fn throttle_returns_cached_without_rewrite() {
        let prior = VscodeState {
            active_until: 2000,
            last_scan: 999,
            primed: true,
            files: vec![],
        };
        // now - last_scan = 1 < 5 -> throttled.
        let (new, active) = vscode_transition(&prior, &[], 1000, 300, 5);
        assert!(new.is_none(), "throttled must not produce a new state");
        assert!(active, "cached active_until(2000) > now(1000)");
    }

    #[test]
    fn retains_aged_out_hash() {
        // prior tracked /x; current scan is empty (aged out).
        let prior = VscodeState {
            active_until: 0,
            last_scan: 0,
            primed: true,
            files: vec![("h1".into(), "/x".into())],
        };
        let (new, _) = vscode_transition(&prior, &[], 1000, 300, 5);
        let new = new.unwrap();
        assert_eq!(
            new.hash_for_path("/x"),
            Some("h1"),
            "aged-out hash retained"
        );
    }
}
