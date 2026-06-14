# Testing conventions

## Never spawn an unbounded CPU hog

Some tests legitimately need a child that burns ~100% of a core (e.g.
`refcount::gc_keeps_busy_agent_with_aged_pidfile`, which proves a busy agent is
*not* garbage-collected). Such a child is dangerous: if the test panics, or the
test **runner** is hard-killed (an interrupted `cargo test`, a CI timeout), the
child is reparented to `launchd` and keeps spinning forever.

This is not hypothetical. An interrupted de-flake harness once left a dozen
`while :; do :; done` shells pinned at 100% CPU after its cleanup `kill` never
ran — driving load average into the 90s and overheating the machine.

**Rule:** any test/dev child that burns CPU or runs long-lived MUST use
[`crate::testutil::BoundedCpuHog`](../crates/vigil/src/testutil.rs) (or an
equivalent with the same three guarantees). Do **not** hand-roll a raw
`Command::new("sh").arg("-c").arg("while :; do :; done")`.

`BoundedCpuHog` layers three independent defenses, each covering a leak path the
others cannot:

1. **Self-bounded** — the child kills its own process tree after `bound_secs`
   (`sleep N; kill -9 $$`). This is the load-bearing defense: it is the *only*
   one that survives a `SIGKILL` of the test runner, because a hard kill skips
   Rust's `Drop`. Choose `bound_secs` generously larger than the test's
   worst-case runtime so the self-kill never ends the workload under test — it
   only caps an *orphan*'s lifetime.
2. **Own process group** (`process_group(0)`) — so the whole subtree (the busy
   shell **and** its `sleep` watchdog) is reapable with one negated-pid `kill`.
3. **RAII `Drop`** — SIGKILLs the group and reaps it on panic OR normal scope
   exit. `std::process::Child` does neither on its own: dropping a `Child`
   neither kills nor waits.

The hot loop stays a bare `while :; do :; done`, so the child pins ~100% of a
core identically to a naive busy loop (the watchdog runs in a backgrounded
subshell and adds no per-iteration work).

### Don't reach for `timeout(1)`

`timeout` / `gtimeout` are **not** present on a stock macOS install (they ship
with GNU coreutils). A test that relies on them either fails to run or, worse,
falls back to an unbounded child. The POSIX `sleep N; kill -9 $$` watchdog inside
`BoundedCpuHog` needs nothing beyond `/bin/sh`.
