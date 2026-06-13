//! Per-agent activity model: the pure mtime scan ([`scan`]), the vscode
//! hash-gate ([`vscode`]), and the daemon-facing [`SessionWatcher`] seam.
//!
//! The pure cores (mtime scan, vscode transition) take paths/records and have no
//! env reads, so they are exercised by cargo tests over temp dirs. The live IO
//! wrappers (host probe, recent-file collection, the read/write daemon path)
//! honor the test-seam env vars at the call site.
//!
//! ## notify-debouncer-mini decision
//! We deliberately do NOT pull in `notify-debouncer-mini` for 5.3. The only
//! event-storm concern is the vscode `state.json` rewrite churn, and that is
//! already bounded by the `discover_secs` throttle inside
//! [`vscode::vscode_transition`] (a scan rewrites state at most once per
//! `discover_secs`). Plain `notify` is sufficient; adding a debouncer
//! speculatively is out of scope per the 5.3 spec.

pub mod scan;
pub mod vscode;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// A resilient session-dir watcher seam for the daemon (full wiring is 5.7).
///
/// If the target session dir does not yet exist, it watches the nearest existing
/// ANCESTOR (walking up until an existing dir is found) so it never errors-and-
/// dies on a missing `~/.codex/sessions`. Once the target appears (observed via
/// any event under the ancestor), [`rearm`](Self::rearm) re-points the watch onto
/// the real dir. On a `notify` error the watcher LOGS and the caller degrades to
/// a periodic rescan — it never panics or exits.
#[allow(dead_code)]
pub struct SessionWatcher {
    target: PathBuf,
    watched: Option<PathBuf>,
    watcher: Option<RecommendedWatcher>,
    rx: Receiver<notify::Result<Event>>,
}

#[allow(dead_code)]
impl SessionWatcher {
    /// Create a watcher for `target`. Watches `target` if it exists, else the
    /// nearest existing ancestor. Returns a watcher even if `notify` could not
    /// arm any path (so the daemon can fall back to periodic rescans).
    pub fn new(target: &Path) -> Self {
        let (tx, rx) = channel();
        let watcher = notify::recommended_watcher(move |res| {
            // Best-effort: a closed receiver just drops the event.
            let _ = tx.send(res);
        })
        .ok();

        let mut sw = SessionWatcher {
            target: target.to_path_buf(),
            watched: None,
            watcher,
            rx,
        };
        sw.arm();
        sw
    }

    /// Nearest existing directory at or above `target`.
    fn nearest_existing(target: &Path) -> Option<PathBuf> {
        let mut cur: Option<&Path> = Some(target);
        while let Some(p) = cur {
            if p.is_dir() {
                return Some(p.to_path_buf());
            }
            cur = p.parent();
        }
        None
    }

    /// (Re)arm onto the best available path: the target if present, else the
    /// nearest existing ancestor. Logs and continues on a notify error.
    fn arm(&mut self) {
        let Some(watcher) = self.watcher.as_mut() else {
            return;
        };
        let Some(path) = Self::nearest_existing(&self.target) else {
            // Nothing exists yet (not even root-ish ancestors readable). Caller
            // falls back to periodic rescan.
            return;
        };
        // Avoid re-arming the same path repeatedly.
        if self.watched.as_deref() == Some(path.as_path()) {
            return;
        }
        // Drop the old watch (if any) before arming the new one.
        if let Some(old) = self.watched.take() {
            let _ = watcher.unwatch(&old);
        }
        let mode = if path == self.target {
            RecursiveMode::Recursive
        } else {
            // Watching an ancestor: non-recursive is enough to catch the target
            // dir's creation; we re-arm recursively once the target appears.
            RecursiveMode::NonRecursive
        };
        match watcher.watch(&path, mode) {
            Ok(()) => self.watched = Some(path),
            Err(e) => {
                tracing::warn!("session watcher: failed to watch {}: {e}", path.display());
            }
        }
    }

    /// If we are currently watching an ancestor and the target has since
    /// appeared, re-point the watch onto the real target dir.
    pub fn rearm(&mut self) {
        if self.watched.as_deref() != Some(self.target.as_path()) && self.target.is_dir() {
            self.arm();
        }
    }

    /// The path currently being watched (target or an ancestor), for diagnostics.
    pub fn watched_path(&self) -> Option<&Path> {
        self.watched.as_deref()
    }

    /// Drain pending fs events (non-blocking). Returns the number drained; the
    /// caller treats any event as "rescan now". Re-arms onto the target if it
    /// has appeared.
    pub fn poll(&mut self) -> usize {
        let mut n = 0;
        while let Ok(res) = self.rx.try_recv() {
            match res {
                Ok(_) => n += 1,
                Err(e) => tracing::warn!("session watcher event error: {e}"),
            }
        }
        self.rearm();
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn watches_parent_when_target_missing_then_rearms() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("a");
        std::fs::create_dir_all(&parent).unwrap();
        let target = parent.join("sessions");
        // target does not exist yet.
        let mut sw = SessionWatcher::new(&target);
        // Must have armed onto an existing ancestor (parent or higher).
        assert!(sw.watched_path().is_some(), "should watch an ancestor");
        assert_ne!(
            sw.watched_path().unwrap(),
            target.as_path(),
            "target missing -> watches ancestor, not target"
        );

        // Create the target dir + a file; poll should re-arm onto target.
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("rollout-x.jsonl"), b"x").unwrap();
        // Give the fs-event backend a moment, polling a few times.
        let mut rearmed = false;
        for _ in 0..50 {
            let _ = sw.poll();
            if sw.watched_path() == Some(target.as_path()) {
                rearmed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            rearmed,
            "watcher should re-arm onto the target once it exists"
        );
    }

    #[test]
    fn poll_degrades_cleanly_without_panic() {
        // A watcher on a path with no events should poll to 0 cleanly.
        let tmp = tempfile::tempdir().unwrap();
        let mut sw = SessionWatcher::new(tmp.path());
        assert_eq!(sw.poll(), 0);
    }
}
