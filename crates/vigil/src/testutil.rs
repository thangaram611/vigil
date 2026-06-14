//! Test-only utilities shared across the crate's in-crate (`#[cfg(test)]`) unit
//! tests. Compiled ONLY under `cfg(test)`.

/// A CPU-burning child process for stress tests that is structurally impossible
/// to leak as a runaway hog. Three independent backstops, each covering a leak
/// path the others cannot:
///
/// 1. **Self-bounded** — the child kills its own process tree after `bound_secs`
///    via a backgrounded `sleep N; kill -9 $$`. This is the load-bearing
///    defense: it survives even a `SIGKILL` of the *test runner* (a hard kill
///    skips Rust's `Drop`, so RAII alone cannot help there). An interrupted
///    `cargo test` therefore cannot leave the hog spinning past the bound.
/// 2. **Own process group** (`process_group(0)`) — the busy shell leads its own
///    group, so [`Drop`] can reap the whole subtree (the busy shell *and* its
///    `sleep` watchdog) with a single negated-pid `kill`.
/// 3. **RAII `Drop`** — `SIGKILL`s the group and reaps it on panic OR normal
///    scope exit. `std::process::Child` does neither on its own (dropping a
///    `Child` neither kills nor waits), so without this a panicking assertion
///    between spawn and an explicit `kill()` would orphan the child.
///
/// The hot loop stays a bare `while :; do :; done`, so the child pins ~100% of a
/// core *identically* to a naive busy loop — the `sleep`/`kill` watchdog runs in
/// a backgrounded subshell and adds no per-iteration work, so the duty cycle a
/// CPU probe observes is unchanged.
///
/// Motivation: an interrupted de-flake harness once orphaned a dozen such shells
/// to `launchd` and overheated the machine. This makes that leak impossible by
/// construction. See `docs/testing.md`.
pub(crate) struct BoundedCpuHog {
    child: std::process::Child,
}

impl BoundedCpuHog {
    /// Spawn a ~100%-CPU child that self-terminates after `bound_secs` even if
    /// orphaned. `bound_secs` should be generously larger than the test's
    /// worst-case runtime so the self-kill never fires during a legitimate (even
    /// slow / heavily-loaded) run — it exists purely to cap an *orphan*'s
    /// lifetime, never to end the workload under test.
    ///
    /// stdio is inherited (not nulled) so a test runner's leak detection can
    /// still observe the child while it lives.
    pub(crate) fn spawn(bound_secs: u32) -> Self {
        #[cfg(unix)]
        use std::os::unix::process::CommandExt;
        use std::process::Command;

        // `$$` is the parent shell's pid even inside the `( … )` subshell (POSIX),
        // so the watchdog kills the shell running the busy loop. `kill -9` is used
        // so no inherited trap can keep it alive.
        let script = format!("(sleep {bound_secs}; kill -9 $$) & while :; do :; done");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(script);
        // Lead our own process group so `Drop` can SIGKILL the whole subtree via
        // a negated-pid kill. This pairing is INSEPARABLE from the negated-pid
        // kill in `Drop`: without `process_group(0)` the child shares the
        // runner's group and `-pid` would signal the test runner itself.
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd.spawn().expect("spawn bounded cpu hog");
        Self { child }
    }

    /// The child's pid. Because the child leads its own process group
    /// (`process_group(0)`), this is also its process-group id.
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for BoundedCpuHog {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: a plain libc `kill(2)`. The negated pid is the id of the group
        // this child leads (see `spawn`'s `process_group(0)`), so this signals
        // exactly our subtree and nothing else. ESRCH (group already gone, e.g.
        // the self-bound elapsed) is ignored.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        // Reap the (now-dead) leader: `std::process::Child` does not wait on drop.
        let _ = self.child.wait();
    }
}
