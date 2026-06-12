# Phase 3.1 — VS Code + GitHub Copilot Chat detection

> **Status: DETAILED PLAN.** Do not implement until the empirical validation
> matrix in §4 passes on the user's current VS Code / Copilot build.

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

## 3. Proposed detection model

If validation passes, add a new activity-only virtual agent:

- agent token: `vscode-copilot-chat`
- refcount tag: `app-vscode-copilot-chat`
- host condition: VS Code or VS Code Insiders is running
- activity condition: newest matching chat file under workspaceStorage is
  newer than `VIGIL_IDLE_AFTER_SEC`

Host condition matters because workspaceStorage files can survive editor exit.
The daemon should only count the activity file if a VS Code host process exists.

Do not add a generic `Code Helper` process match to `detect.sh`; that would
false-positive against any open VS Code window. Instead, add a narrow host
probe function that returns true when one of these app hosts is live:

- `/Applications/Visual Studio Code.app/Contents/MacOS/Electron`
- `/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Electron`
- matching per-user or external-volume app-bundle paths with the same
  `*.app/Contents/MacOS/Electron` suffix and app name.

## 4. Validation matrix

Run the retained phase-3 harness, extended to record the candidate globs above.
Each state needs at least 30 ticks at the daemon cadence.

| State | Expected result |
| --- | --- |
| VS Code closed | no host, no count even if old chat files exist |
| VS Code idle | host present, candidate chat mtimes stay older than idle window |
| ordinary editing | host present, candidate chat mtimes do not advance |
| Copilot Chat answer | candidate `chatEditingSessions` or debug-log mtime advances |
| Copilot agent/edit session | candidate mtime advances until the edit run completes |
| post-completion idle | candidate mtime ages past idle window and count drops |

Validation must be repeated for stable VS Code and VS Code Insiders if both are
installed. If only Insiders is installed, document that in the closeout.

## 5. Implementation steps after validation

1. Extend `tests/experiments/phase-3/observe.sh` to record the candidate globs.
2. Capture and annotate the validation runs.
3. Add `vigil_vscode_host_running` in a new small shell module or in
   `activity.sh` if the implementation stays compact.
4. Add a scoped `vigil_vscode_copilot_chat_is_active` probe.
5. Thread the virtual agent through `bin/vigil-daemon`, `vigil status`, and
   `status --json`.
6. Add fixture tests for host-running true/false and activity-file recency.
7. Update README, ROADMAP, and CHANGELOG.

## 6. Non-goals

- No extension-directory matching.
- No raw `workspaceStorage/` mtime matching.
- No `Code Helper` refcount files.
- No support for Cursor, Windsurf, or other editors in this phase.

