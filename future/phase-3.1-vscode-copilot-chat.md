# Phase 3.1 — VS Code + GitHub Copilot Chat detection

> **Status: shipped 2026-06-12** — synthesis archived for the audit trail.

## 1. Why this exists

Phase 3 deliberately deferred GitHub Copilot Chat because the observed chat ran
inside VS Code's extension host. There was no distinct `copilot` process to
anchor against, and raw `workspaceStorage/` mtime was too noisy to use as an
activity-only signal.

The 2026-06 research pass changed two assumptions:

- VS Code now documents Copilot Chat as a built-in extension, so installed
  extension directory names are not a stable production signal.
- GitHub and VS Code now document Copilot CLI sessions as a first-class path.
  Those are already covered by Vigil's existing `copilot` process match plus
  `COPILOT_HOME/session-state/**/events.jsonl` activity probe.

So phase 3.1 is now scoped only to **in-process VS Code Copilot Chat activity**,
not Copilot CLI sessions.

## 2. Current known surfaces

### Already covered

Copilot CLI:

- process anchor: `copilot`
- activity root: `${VIGIL_COPILOT_HOME:-${COPILOT_HOME:-~/.copilot}}/session-state`
- activity file: `events.jsonl`

No new code is planned for CLI sessions unless empirical testing proves the
process basename or session-state contract changed.

### Candidate VS Code Chat signals

Empirical phase-3 observations found writes under:

- `~/Library/Application Support/Code/User/workspaceStorage/*/chatEditingSessions/*/state.json`
- `~/Library/Application Support/Code - Insiders/User/workspaceStorage/*/chatEditingSessions/*/state.json`
- `.../workspaceStorage/*/GitHub.copilot-chat/debug-logs/*/models.json`

The candidate production signal is **not** raw workspaceStorage mtime. It is a
scoped file glob under those chat-specific paths, and only if validation shows
the files are quiet during normal editor idle.

## 3. Detection model

Validation rejected the original raw-mtime proposal. The shipped detector uses:

- agent token: `vscode-copilot-chat`
- refcount tag: `app-vscode-copilot-chat`
- host condition: VS Code or VS Code Insiders is running
- activity condition: a matching `state.json` file's content hash changes
  after the detector has been primed

Host condition matters because workspaceStorage files can survive editor exit.
The daemon only counts the activity file if a VS Code host process exists.

Do not add a generic `Code Helper` process match to `detect.sh`; that would
false-positive against any open VS Code window. Instead, add a narrow host
probe that returns true when one of these app hosts is live:

- `/Applications/Visual Studio Code.app/Contents/MacOS/*`
- `/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/*`
- matching per-user or external-volume app-bundle paths with the same
  `*.app/Contents/MacOS/*` suffix and app name.

## 4. Validation matrix

Run the retained phase-3 harness, extended to record the candidate globs above.
Each state needs at least 30 ticks at the daemon cadence.

| State | Result |
| --- | --- |
| VS Code closed | no scoped files modified |
| VS Code idle | no scoped files modified during initial idle sample |
| ordinary workspace change | `state.json` appeared recent, but only because of a prior mtime rewrite |
| Copilot Chat answer | same `state.json` content hash changed; `timeline.currentEpoch` / checkpoint count advanced |
| post-completion idle | `state.json` mtime rewrote again while content hash stayed unchanged |

This is why the implementation is hash-based. Raw mtime would false-positive.

## 5. Implementation closeout

1. Extended `tests/experiments/phase-3/observe.sh` with scoped candidate globs.
2. Added `app-vscode-copilot-chat` host detection in `lib/detect.sh`.
3. Added `vigil_vscode_copilot_chat_is_active` in `lib/activity.sh`.
4. Threaded the virtual agent through daemon refcounting, `vigil status`, and
   `status --json`.
5. Added tests for host detection, helper exclusion, hash-change activity, and
   the refcount gate.

## 6. Non-goals

- No extension-directory matching.
- No raw `workspaceStorage/` mtime matching.
- No `Code Helper` refcount files.
- No support for Cursor, Windsurf, or other editors in this phase.
