//! Live process scan via `sysinfo` + the pure detection core (`detect`).
//!
//! The pure classifier and pid-keyed ps-join live in [`detect`]; they are fed
//! text/records and have no IO. This module owns the ONE long-lived
//! `sysinfo::System` that the daemon refreshes per tick (5.7) and that
//! `vigil debug` refreshes exactly once (5.3).
//!
//! ## sysinfo -> (comm, cmdline) parity mapping
//! - `exe` (= bash `comm`, from `ps -o comm=`): [`sysinfo::Process::exe`] (full
//!   path, space-safe). For PATH-style invocations where sysinfo has no exe
//!   path, fall back to [`sysinfo::Process::name`] (the bare basename — matches
//!   `ps -o comm=` printing the basename for PATH invocations). Both via
//!   `to_string_lossy()`.
//! - `cmdline` (= bash `command_line`, from `ps -o command=`):
//!   `cmd().iter().map(to_string_lossy).join(" ")` — reconstructs `ps -o
//!   command=` for the common case.
//!
//! ## same-user `cmd()` restriction is PARITY, not a regression
//! On macOS sysinfo can only read full `cmd()`/`exe()` for processes owned by
//! the same euid. The daemon (and the bash `ps -axww`) run as the user, and
//! cross-user agent processes are out of scope for both: session dirs and pid
//! files are per-user under a 0700 state dir, so the bash daemon also only
//! refcounts the invoking user's agents. The pure layer is unaffected (it is
//! fed text/records), so the parity oracle over fixtures does not exercise this.
//!
//! ## one System for the daemon lifetime
//! [`ProcScanner`] holds a single `System` and refreshes SCOPED to cmd+exe via
//! `refresh_processes_specifics`/`ProcessRefreshKind` — never `refresh_all`, so
//! we never pay full-system refresh cost per tick.

pub mod detect;

pub use detect::{
    AgentKind, AgentMatch, agent_match_tsv, detect_all_text, detect_line, parse_ps_pid_keyed,
    ps_join,
};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// One live process record: the sysinfo analogue of a joined ps row.
#[derive(Debug, Clone)]
pub struct ProcRecord {
    pub pid: u32,
    /// = bash `comm` (exe path or bare basename).
    pub exe: String,
    /// = bash `command_line` (full argv, space-joined).
    pub cmdline: String,
}

/// A long-lived `sysinfo::System`, created ONCE for the daemon lifetime (never
/// per-tick).
pub struct ProcScanner {
    sys: System,
}

impl Default for ProcScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcScanner {
    /// Create an empty `System`. No refresh happens until [`collect`](Self::collect).
    pub fn new() -> Self {
        ProcScanner { sys: System::new() }
    }

    /// Refresh ONLY processes' cmd+exe (scoped), then collect [`ProcRecord`]s.
    ///
    /// Scope: `ProcessRefreshKind::nothing().with_cmd(Always).with_exe(Always)`
    /// — NOT cpu/memory/etc.
    pub fn collect(&mut self) -> Vec<ProcRecord> {
        let kind = ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always);
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, kind);

        let mut out = Vec::new();
        for (pid, proc_) in self.sys.processes() {
            // exe = comm: prefer the full exe path; fall back to the bare name
            // (PATH-style invocations have no exe path, matching `ps -o comm=`).
            let exe = match proc_.exe() {
                Some(p) => p.to_string_lossy().into_owned(),
                None => proc_.name().to_string_lossy().into_owned(),
            };
            // cmdline = command_line: argv joined by single spaces.
            let cmdline = if proc_.cmd().is_empty() {
                exe.clone()
            } else {
                proc_
                    .cmd()
                    .iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            out.push(ProcRecord {
                pid: pid.as_u32(),
                exe,
                cmdline,
            });
        }
        out
    }

    /// Live detect: [`collect`](Self::collect) then [`detect_line`] over each record.
    pub fn detect(&mut self) -> Vec<AgentMatch> {
        self.collect()
            .into_iter()
            .filter_map(|r| detect_line(r.pid, &r.exe, &r.cmdline))
            .collect()
    }

    /// Start time (unix secs) for `pid` from the ALREADY-REFRESHED `System`, or
    /// `None` if the pid is not currently visible. This reads cached process
    /// metadata populated by the most recent [`collect`](Self::collect) /
    /// [`detect`](Self::detect) refresh — sysinfo fills `start_time()` on any
    /// process refresh, so no extra refresh scope is needed.
    ///
    /// The daemon writes this into each agent pidfile's `start_ts` so the GC
    /// pid-reuse branch (on-disk `start_ts` vs live `start_time()`) compares two
    /// values from the SAME clock source and never false-positives on a
    /// long-lived agent. Mirrors bash's `vigil_pid_start_ts` feeding both the
    /// `vigil_refcount_touch` write AND the `vigil_refcount_gc` reuse probe.
    pub fn start_time(&self, pid: u32) -> Option<i64> {
        use sysinfo::Pid;
        self.sys
            .process(Pid::from(pid as usize))
            .map(|p| p.start_time() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_collects_self() {
        // Smoke test: our own process must appear with a non-empty exe.
        let mut s = ProcScanner::new();
        let recs = s.collect();
        assert!(!recs.is_empty(), "should collect at least this process");
        let me = std::process::id();
        assert!(
            recs.iter().any(|r| r.pid == me),
            "collect must include the current pid"
        );
    }
}
