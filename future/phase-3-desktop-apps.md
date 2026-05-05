# Phase 3 — Desktop app detection

> **Status: SKETCH ONLY.** Replace with a detailed plan before implementation.

## Why deferred

Treating Claude.app / Codex.app / Copilot.app as "active" whenever the desktop app is open is wrong — keeping the Mac awake all day for an open-but-idle window is the bug we're avoiding, not a feature. Needs session-aware logic borrowed from `hiddenest/awake`.

## Direction

Per-provider session-state checks:

- **Claude.app** — correlate the `Claude` main process (`/Applications/Claude.app/Contents/MacOS/Claude`) with recent updates to Claude Code session files in `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`.
- **Codex.app** — the bundled CLI at `/Applications/Codex.app/Contents/Resources/codex` running *without* `app-server` in argv is the actual agent execution. Correlate with Codex's rollout file convention (TBD; needs source audit of Codex CLI v0.125+).
- **Copilot.app** — TBD; depends on what GitHub ships.

Default: **opt-in only.** Off until the user explicitly enables in `~/.config/vigil/vigil.conf`. Off with a clear warning that this can have false positives until session-aware logic is robust.

## Open questions

- Where exactly does Claude.app store session state on disk? Does it write to `~/.claude/projects/`, or to a separate desktop-app path?
- What's Codex desktop's rollout-file path and update cadence?
- What window length is correct? `awake` uses 15s; we may need different per provider.
- Should desktop providers be one boolean (`detect_desktop=true`), or per-app (`detect_claude_app=true`, `detect_codex_app=true`)? Per-app is more granular but more config surface.

## When this phase begins

Replace this file with a concrete plan: per-app session file globs + recency windows + test fixtures with both active and idle states.
