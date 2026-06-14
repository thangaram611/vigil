//! PID-file refcount + stale GC — the Rust port of `lib/refcount.sh`.
//!
//! The on-disk pidfile body is BYTE-IDENTICAL to bash so the 5.7 cutover reads
//! either side. count/list parse ONLY the filename (the bash refcount tests
//! create empty-body pidfiles), while GC and field extraction read the body.

use std::path::{Path, PathBuf};

use serde_json::Value;

// ── filename model ─────────────────────────────────────────────────────────────

/// Parse a pidfile BASENAME (without `.pid`) into `(name, pid)`.
///
/// bash: `pid=${base##*-}` (after last `-`), `name=${base%-*}` (before last `-`).
/// So `app-vscode-copilot-chat-22222` -> `("app-vscode-copilot-chat", 22222)`,
/// `wrapper-1004` -> `("wrapper", 1004)`. Split on the LAST `-`. Returns None if
/// there is no `-` or the tail is not a u32.
pub fn parse_pidfile_basename(base: &str) -> Option<(String, u32)> {
    let idx = base.rfind('-')?;
    let name = &base[..idx];
    let pid_str = &base[idx + 1..];
    let pid = pid_str.parse::<u32>().ok()?;
    if name.is_empty() {
        return None;
    }
    Some((name.to_string(), pid))
}

/// Whether a pidfile `prefix` (== name) counts given the four activity flags.
///
/// `cli-claude`/`cli-codex`/`cli-copilot` gate on the matching flag; `app-codex`
/// gates on `codex` (shares `~/.codex/sessions`); `app-vscode-copilot-chat` gates
/// on `vscode`; `wrapper` ALWAYS counts; anything else never counts.
pub fn prefix_counts(prefix: &str, claude: bool, codex: bool, copilot: bool, vscode: bool) -> bool {
    match prefix {
        "cli-claude" => claude,
        "cli-codex" => codex,
        "cli-copilot" => copilot,
        "app-codex" => codex,
        "app-vscode-copilot-chat" => vscode,
        "wrapper" => true,
        _ => false,
    }
}

/// The `state` column for `list`: "active" if the prefix is gated on a true flag
/// (wrapper always active), else "idle".
fn prefix_state(
    prefix: &str,
    claude: bool,
    codex: bool,
    copilot: bool,
    vscode: bool,
) -> &'static str {
    let active = match prefix {
        "cli-claude" => claude,
        "cli-codex" => codex,
        "cli-copilot" => copilot,
        "app-codex" => codex,
        "app-vscode-copilot-chat" => vscode,
        "wrapper" => true,
        _ => false,
    };
    if active { "active" } else { "idle" }
}

// ── on-disk body (BYTE-IDENTICAL) ──────────────────────────────────────────────

/// Agent pidfile body, byte-identical to bash `vigil_refcount_touch`:
/// `{"pid":<n>,"comm":"<exe>","start_ts":<unix>,"name":"<name>"}\n`
/// with all `"` chars STRIPPED from `<exe>` (bash `${exe//\"/}` — removed, not
/// escaped). Hand-formatted (NOT serde_json) to preserve byte-identity.
pub fn pidfile_body(name: &str, pid: u32, exe: &str, start_ts: i64) -> String {
    let safe_exe = exe.replace('"', "");
    format!(
        "{{\"pid\":{pid},\"comm\":\"{safe_exe}\",\"start_ts\":{start_ts},\"name\":\"{name}\"}}\n"
    )
}

/// Wrapper pidfile body, byte-identical to bash `vigil_refcount_touch_wrapper`:
/// `{"pid":<n>,"comm":"wrapper","start_ts":<now>,"cmd":"<cmd>"}\n`
/// with all `"` chars stripped from `<cmd>`.
pub fn wrapper_pidfile_body(pid: u32, cmd: &str, now: i64) -> String {
    let safe_cmd = cmd.replace('"', "");
    format!("{{\"pid\":{pid},\"comm\":\"wrapper\",\"start_ts\":{now},\"cmd\":\"{safe_cmd}\"}}\n")
}

// ── field extraction (mirror _vigil_pidfile_field, pinned by parser_test) ───────

/// Extract a field from a pidfile JSON body as a STRING.
///
/// The files are real JSON, so we parse with serde_json and look up `key`:
///   - JSON integer -> its canonical integer string (no trailing `.0`)
///   - JSON string  -> the string content
///   - missing key  -> None
///
/// Reproduces the bash `_vigil_pidfile_field` results exactly: `field("start_ts")`
/// returns "1700000000" (NOT the pid), `field("name")` -> "cli-claude",
/// `field("nope")` on `{"pid":1234}` -> None, `field("SleepDisabled")` -> "1".
pub fn field(json_body: &str, key: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_body.trim()).ok()?;
    let val = v.get(key)?;
    match val {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i.to_string())
            } else if let Some(u) = n.as_u64() {
                Some(u.to_string())
            } else {
                // Non-integral number: fall back to its canonical JSON repr.
                Some(n.to_string())
            }
        }
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

// ── directory operations ───────────────────────────────────────────────────────

/// One parsed pidfile entry from the active dir.
#[derive(Debug, Clone)]
pub struct PidEntry {
    pub path: PathBuf,
    pub name: String,
    pub pid: u32,
    pub mtime: i64,
    pub body: String,
}

/// Whole-second mtime for a path, or 0 (mirrors bash `stat -f %m || echo 0`).
fn mtime_secs_or_zero(path: &Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// List `*.pid` in `active_dir` (non-recursive, maxdepth 1), parse each basename
/// and read each body. Entries whose basename does not parse are skipped.
pub fn read_entries(active_dir: &Path) -> Vec<PidEntry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(active_dir) else {
        return out;
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(base) = fname.strip_suffix(".pid") else {
            continue;
        };
        let Some((name, pid)) = parse_pidfile_basename(base) else {
            continue;
        };
        let mtime = mtime_secs_or_zero(&path);
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        out.push(PidEntry {
            path,
            name,
            pid,
            mtime,
            body,
        });
    }
    out
}

/// Activity-filtered count (`vigil_refcount_count`). Missing dir -> 0.
pub fn count(active_dir: &Path, claude: bool, codex: bool, copilot: bool, vscode: bool) -> u32 {
    read_entries(active_dir)
        .iter()
        .filter(|e| prefix_counts(&e.name, claude, codex, copilot, vscode))
        .count() as u32
}

/// Raw count of all `*.pid` (`vigil_refcount_count_total`). Missing dir -> 0.
pub fn count_total(active_dir: &Path) -> u32 {
    read_entries(active_dir).len() as u32
}

/// List rows `<pid>\t<name>\t<age_secs>\t<state>` (`vigil_refcount_list`).
/// `state`: per prefix gated by the matching flag -> "active" else "idle";
/// wrapper -> "active".
pub fn list(
    active_dir: &Path,
    now: i64,
    claude: bool,
    codex: bool,
    copilot: bool,
    vscode: bool,
) -> Vec<(u32, String, i64, &'static str)> {
    read_entries(active_dir)
        .into_iter()
        .map(|e| {
            let state = prefix_state(&e.name, claude, codex, copilot, vscode);
            (e.pid, e.name, now - e.mtime, state)
        })
        .collect()
}

// ── stale GC ───────────────────────────────────────────────────────────────────

/// The GC decision for one pidfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcDecision {
    Keep,
    DropDead,
    DropPidReuse,
    DropIdle,
}

/// Pure GC decision for ONE entry given live probe results. Branch order is
/// LOAD-BEARING (mirrors `vigil_refcount_gc`):
///   (a) `!pid_alive`                                            -> DropDead
///   (b) on-disk start_ts != live start_ts (both Some)          -> DropPidReuse
///   (c) name != "wrapper" && age > stale && cpu < stale_cpu    -> DropIdle
///   else                                                       -> Keep
///
/// (a) and (b) apply to wrappers too; the wrapper carve-out is ONLY for the idle
/// branch (c).
#[allow(clippy::too_many_arguments)]
pub fn gc_decision(
    name: &str,
    age: i64,
    pid_alive: bool,
    on_disk_start: Option<i64>,
    live_start: Option<i64>,
    cpu_pct: Option<f64>,
    stale_age_secs: u32,
    stale_cpu_pct: f64,
) -> GcDecision {
    // (a) dead pid — drop unconditionally (applies to wrapper too).
    if !pid_alive {
        return GcDecision::DropDead;
    }
    // (b) pid reuse — start_ts mismatch (both present). Applies to wrapper too.
    if let (Some(d), Some(l)) = (on_disk_start, live_start)
        && d != l
    {
        return GcDecision::DropPidReuse;
    }
    // (c) idle — old + low cpu. Wrapper is carved out of THIS branch only.
    if name != "wrapper"
        && age > stale_age_secs as i64
        && let Some(cpu) = cpu_pct
        && cpu < stale_cpu_pct
    {
        return GcDecision::DropIdle;
    }
    GcDecision::Keep
}

/// `kill(pid, 0) == 0` — true iff the PID exists (and we may signal it).
fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 performs no signal delivery, only existence /
    // permission checking; always safe to call.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Live GC over `active_dir`: for each entry, gather probes (kill(pid,0),
/// start_ts and cpu via the sysinfo `System`) and remove files whose
/// decision != Keep.
///
/// `start_time()` (sysinfo, unix seconds) == bash `ps -o lstart=` -> epoch at
/// whole-second granularity; both feed the reuse-equality check, which only
/// needs change detection.
///
/// CPU contract: sysinfo computes `cpu_usage()` from a time diff between two
/// refreshes, so the FIRST `with_cpu()` refresh of any process yields `0.0`
/// (sysinfo `system.rs`: "a process needs to be refreshed **twice**"). A single
/// refresh would make every busy agent read `0.0 < stale_cpu_pct` and get
/// DropIdle'd — inverting bash, which reads the real `ps -o %cpu=` decayed
/// average. So `gc` performs the required two `with_cpu()` refreshes itself,
/// spaced by [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`], BEFORE reading cpu. This
/// makes `gc` correct on any `System`, including a freshly-constructed one (the
/// procscan path uses a different `System` instance, so we cannot rely on a
/// prior cpu refresh having happened on this one).
pub fn gc(
    active_dir: &Path,
    sys: &mut sysinfo::System,
    stale_age_secs: u32,
    stale_cpu_pct: f64,
    now: i64,
) {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate};
    // Read the active dir FIRST and bail before paying any scan cost when there
    // is nothing to GC (the common idle case). gc() runs every daemon tick, and
    // each non-empty pass walks the whole process table TWICE with a mandatory
    // ~200ms sleep wedged between — so skipping it when empty removes the single
    // heaviest per-tick cost on an idle machine. (Kept ProcessesToUpdate::All:
    // narrowing the refresh to specific pids on this long-lived `System` would
    // let stale pids from prior ticks accumulate and feed a reused-pid a wrong
    // start_time/cpu into gc_decision.)
    let entries = read_entries(active_dir);
    if entries.is_empty() {
        return;
    }
    // Refresh scoped to cpu + (implicit) start_time for the probe. Two refreshes
    // spaced by MINIMUM_CPU_UPDATE_INTERVAL are REQUIRED for non-zero cpu_usage().
    let kind = ProcessRefreshKind::nothing().with_cpu();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);

    for e in entries {
        let alive = pid_alive(e.pid);
        let proc_ = sys.process(Pid::from(e.pid as usize));
        let live_start = proc_.map(|p| p.start_time() as i64);
        let cpu = proc_.map(|p| p.cpu_usage() as f64);
        let on_disk_start = field(&e.body, "start_ts").and_then(|s| s.parse::<i64>().ok());
        let age = now - e.mtime;
        let decision = gc_decision(
            &e.name,
            age,
            alive,
            on_disk_start,
            live_start,
            cpu,
            stale_age_secs,
            stale_cpu_pct,
        );
        if decision != GcDecision::Keep {
            let _ = std::fs::remove_file(&e.path);
            tracing::debug!(
                "gc {:?} pid={} name={} age={}s",
                decision,
                e.pid,
                e.name,
                age
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basename_last_dash() {
        assert_eq!(
            parse_pidfile_basename("app-vscode-copilot-chat-22222"),
            Some(("app-vscode-copilot-chat".into(), 22222))
        );
        assert_eq!(
            parse_pidfile_basename("wrapper-1004"),
            Some(("wrapper".into(), 1004))
        );
        assert_eq!(parse_pidfile_basename("nodash"), None);
    }

    #[test]
    fn body_byte_shape() {
        assert_eq!(
            pidfile_body("cli-claude", 1234, "claude", 1700000000),
            "{\"pid\":1234,\"comm\":\"claude\",\"start_ts\":1700000000,\"name\":\"cli-claude\"}\n"
        );
        // quote stripping: exe a"b -> ab
        assert_eq!(
            pidfile_body("cli-claude", 1, "a\"b", 5),
            "{\"pid\":1,\"comm\":\"ab\",\"start_ts\":5,\"name\":\"cli-claude\"}\n"
        );
    }

    #[test]
    fn field_extraction_parser_test_cases() {
        let body = r#"{"pid":1234,"comm":"claude","start_ts":1700000000,"name":"cli-claude"}"#;
        assert_eq!(field(body, "pid").as_deref(), Some("1234"));
        assert_eq!(field(body, "start_ts").as_deref(), Some("1700000000"));
        assert_eq!(field(body, "name").as_deref(), Some("cli-claude"));
        assert_eq!(field(r#"{"pid":1234}"#, "nope"), None);
        assert_eq!(
            field(
                r#"{"SleepDisabled":1,"captured_at":1700000000}"#,
                "SleepDisabled"
            )
            .as_deref(),
            Some("1")
        );
    }

    #[test]
    fn gc_branch_order() {
        // dead beats everything (even wrapper).
        assert_eq!(
            gc_decision("wrapper", 9999, false, Some(1), Some(2), Some(0.0), 30, 0.5),
            GcDecision::DropDead
        );
        // reuse beats idle (even wrapper).
        assert_eq!(
            gc_decision("wrapper", 9999, true, Some(1), Some(2), Some(0.0), 30, 0.5),
            GcDecision::DropPidReuse
        );
        // wrapper carve-out from idle: old + low cpu but alive + matching start.
        assert_eq!(
            gc_decision("wrapper", 9999, true, Some(1), Some(1), Some(0.0), 30, 0.5),
            GcDecision::Keep
        );
        // cli-claude idle: old + low cpu -> drop.
        assert_eq!(
            gc_decision(
                "cli-claude",
                9999,
                true,
                Some(1),
                Some(1),
                Some(0.0),
                30,
                0.5
            ),
            GcDecision::DropIdle
        );
        // cli-claude busy: cpu >= threshold -> keep.
        assert_eq!(
            gc_decision(
                "cli-claude",
                9999,
                true,
                Some(1),
                Some(1),
                Some(0.9),
                30,
                0.5
            ),
            GcDecision::Keep
        );
    }

    /// Backdate a file's mtime by `secs` seconds (so `gc` sees it as aged).
    fn backdate_mtime(path: &Path, secs: i64) {
        let m = std::fs::metadata(path).unwrap();
        let mtime = m
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - secs;
        let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let tv = libc::timeval {
            tv_sec: mtime as libc::time_t,
            tv_usec: 0,
        };
        let times = [tv, tv]; // [atime, mtime]
        // SAFETY: cpath is a valid NUL-terminated path; times points at 2 timevals.
        let rc = unsafe { libc::utimes(cpath.as_ptr(), times.as_ptr()) };
        assert_eq!(rc, 0, "utimes must succeed");
    }

    /// REGRESSION (finding 1, high/sysinfo-notify): the LIVE `gc()` must meet
    /// sysinfo's two-refresh cpu contract before reading `cpu_usage()`. A single
    /// refresh yields `cpu==0.0` for every process, so a busy agent whose pidfile
    /// has aged past `stale_age_secs` would read `0.0 < stale_cpu_pct` and get
    /// DropIdle'd — inverting bash (which reads the real `ps -o %cpu=`). This test
    /// spawns a CPU-burning child, writes an aged `cli-claude` pidfile for it, and
    /// asserts `gc()` KEEPS the file (alive + matching start_ts + high cpu).
    ///
    /// Robustness: `gc`'s cpu probe is a single ~`MINIMUM_CPU_UPDATE_INTERVAL`
    /// (~200ms) sample, and under heavy machine load (load avg > cores) the
    /// scheduler can starve even a tight spin-loop below `stale_cpu_pct` for one
    /// such window — a flake unrelated to the contract under test. We therefore
    /// re-sample with a bounded retry: a correct double-refresh reads the child's
    /// true (high) usage on at least one attempt, while the single-refresh
    /// regression this guards against reads `0.0` on EVERY attempt, so it still
    /// fails deterministically. (Production is unaffected by the starvation case:
    /// the idle branch only fires on pidfiles older than `stale_age_secs`, and the
    /// daemon re-touches every live matched agent's pidfile each 5s tick, so a busy
    /// agent never reaches a 30s-stale pidfile there.)
    #[test]
    fn gc_keeps_busy_agent_with_aged_pidfile() {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};

        // A busy-loop child: burns ~100% of a core. `BoundedCpuHog` makes it
        // leak-proof — it self-terminates after 60s even if orphaned, leads its
        // own process group, and is SIGKILL'd + reaped by its `Drop` on panic OR
        // normal exit (so a failed assertion below cannot leave it spinning).
        // 60s dwarfs the test's worst case (10 retries × ~200ms), so the
        // self-bound never ends the workload under test.
        let hog = crate::testutil::BoundedCpuHog::spawn(60);
        let pid = hog.pid();

        // Read the child's live start_ts via the same sysinfo path gc uses, so
        // the pid-reuse branch (b) cannot fire on a start_ts mismatch.
        let mut sys = sysinfo::System::new();
        let kind = ProcessRefreshKind::nothing().with_cpu();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        let live_start = sys
            .process(sysinfo::Pid::from(pid as usize))
            .map(|p| p.start_time() as i64)
            .expect("child must be visible to sysinfo");

        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join(format!("cli-claude-{pid}.pid"));

        // stale_age_secs=30, stale_cpu_pct=0.5 (the documented defaults). The
        // child is burning ~100% cpu, so the idle branch (c) must NOT fire — but
        // re-sample to ride out a single scheduler-starved cpu window under load.
        let mut kept = false;
        for _ in 0..10 {
            // Recreate the aged pidfile (a starved sample may have dropped it).
            std::fs::write(
                &pidfile,
                pidfile_body("cli-claude", pid, "claude", live_start),
            )
            .expect("write pidfile");
            // Age the pidfile well past the default stale window (30s).
            backdate_mtime(&pidfile, 600);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            gc(dir.path(), &mut sys, 30, 0.5, now);
            if pidfile.exists() {
                kept = true;
                break;
            }
        }

        // `hog` is dropped here (or on an assertion panic below) — its `Drop`
        // SIGKILLs the process group and reaps it; no manual kill needed.
        assert!(
            kept,
            "gc must KEEP an aged pidfile for a busy (high-cpu) live agent across \
             repeated samples; a single-refresh cpu probe would read 0.0 every \
             time and wrongly DropIdle it"
        );
    }
}
