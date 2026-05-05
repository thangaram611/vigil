# Phase 2 — copilot-companion integration

> **Status: SKETCH ONLY.** This file must be replaced with a detailed plan before phase 2 implementation begins.

## Why deferred

The companion's node ACP daemon (`copilot-acp-daemon.mjs`) runs continuously, so raw process detection would keep the Mac awake 24/7. Need session-aware logic — but the actual thread-state lifecycle hasn't been audited end-to-end yet. The mtime-window choice depends on whether thread JSON files get touched on every poll or only on state changes.

## Direction

Borrow `hiddenest/awake`'s session-provider model:

- Match the long-lived node daemon by `argv[1]` containing `copilot-acp-daemon.mjs` (or the canonical companion path).
- Mark *active* iff at least one file under `~/.claude/copilot-companion/threads/*.json` has mtime within an `ACTIVE_SESSION_WINDOW_SECS` (15-30s, TBD after the audit).
- Otherwise treat the daemon as idle even though it is running.

## Open questions for the replan

- Does the companion update thread JSON on every poll iteration, or only on state changes? Determines the mtime window.
- Is there a richer signal (e.g. `tracked_until` in the thread JSON) that's more accurate than mtime?
- Does the `~/.claude/copilot-companion/threads/` path stay stable across companion versions?
- Should this also flag *absence of expected polls* (companion stuck) as not-active? Probably yes — only count when there's evidence of forward progress.

## When this phase begins

Replace this file with a concrete plan: the exact file glob, the exact field/timestamp signal, the exact window, the test fixtures, and the expected behavior on a live Copilot job vs. a stale daemon vs. a finished job.
