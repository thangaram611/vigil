# Phase 3 — Desktop app detection

> **Status: shipped 2026-05-25** — synthesis archived for the audit trail.
> Detection rules in §5 were validated by the experiment matrix in §4
> before implementation. The harness (`tests/experiments/phase-3/`) is
> retained for phase-3.1 (VS Code + GitHub Copilot Chat — see §5.4 / §7)
> and future editor integrations.

## 1. Scope

Three desktop targets, locked in with the user 2026-05-21:

1. **Claude.app** — Anthropic desktop app (Electron). Local Agent Mode runs
   tasks; an idle-but-open window must not engage sleep prevention.
2. **Codex.app** — OpenAI desktop app (`com.openai.codex`, Electron). Drives a
   long-lived `codex app-server` subprocess regardless of agent activity.
3. **VS Code + GitHub Copilot agent mode** — explicitly chosen over the
   standalone GitHub Copilot.app tech preview. The user's installed editor is
   VS Code Insiders at `/Applications/Visual Studio Code - Insiders.app`, with
   `github.copilot-chat-0.30.0` and `github.copilot-1.354.1728` in
   `~/.vscode/extensions/`. Copilot.app (`github/app`, May 2026 tech preview)
   is not in scope for this phase.

Out of scope: Cursor, Windsurf, opencode, and other editor integrations.

## 2. Why experiment-driven (not "design from research")

The prior sketch encoded assumptions about per-app session-file paths and
recency windows. Phase 2's audit found that one such assumption (a separate
`copilot-companion` agent type) would have been net-negative — phase 1
already covered the worker via `~/.copilot/session-state/`. Repeating
that discipline here:

- **observe** the live processes and on-disk activity each app produces
  across four states (idle, active, post-completion idle, closed);
- **classify** each observation against the existing phase-1 detection
  surface;
- **decide** per app: phase 1 already covers it (audit + test fixture, no
  code change), or a new rule is required (concrete process pattern + activity
  glob + recency window, traced to a logged observation).

Open-source priors (already collected, not load-bearing for the decision):

- `hiddenest/awake`'s providers (`src/session_polling/claude_code.rs`,
  `codex.rs`) use a `live process AND fresh session-file mtime` AND-gate with
  a 15 s recency window. The codex provider additionally uses `lsof -p <pid>`
  to associate an `app-server` pid with its currently-open
  `rollout-*.jsonl` so a long-lived idle daemon doesn't false-positive.
- `openai/codex` confirms desktop, CLI, and `codex exec` all share
  `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl`
  (codex-rs/app-server/README.md).
- Claude desktop's Local Agent Mode spawns the bundled Claude Code binary
  at `~/Library/Application Support/Claude/claude-code/<ver>/claude.app/Contents/MacOS/claude`
  (basename `claude`, not under `/Applications/`) and writes transcripts to
  `~/.claude/projects/<encoded-cwd>/*.jsonl` — the same path phase 1
  already probes.

These priors inform what to *look for* during experiments; they do not
substitute for running the observations.

## 3. Experiment harness

`tests/experiments/phase-3/observe.sh` — single-shot snapshot tool.

```
observe.sh <label>
```

Per invocation, writes one NDJSON line to
`tests/experiments/phase-3/runs/<label>.ndjson` (gitignored) containing:

- `ts` — unix seconds
- `procs` — array of `{pid, command}` rows where `command` matches any
  candidate basename / path substring (see §3.1).
- `mtimes` — map of canonical names → mtime (or null) for each candidate
  session-file directory (§3.2).
- `lsof_open_files` — optional, populated only when a `--lsof <pid>` flag is
  passed; lists file paths the named pid has open under candidate roots.

Reproduction harness: a one-liner that loops `observe.sh <label>` every 5 s
for N ticks, then we annotate the file with the user-visible state at each
range of ticks.

### 3.1 Candidate process patterns

| App | Candidate `command=` substrings |
| --- | --- |
| Claude.app | `/Applications/Claude.app/`; `Library/Application Support/Claude/claude-code/`; `/.local/bin/claude`; `/.local/share/claude/` |
| Codex.app  | `/Applications/Codex.app/`; `codex app-server`; `codex_chronicle` |
| VS Code    | `/Applications/Visual Studio Code`; `Code Helper`; `copilot-language-server`; `copilot-chat`; `node` argv containing `vscode-server` |

These are the matching surfaces we test against, not the proposed
production rules — those are decided in §5.

### 3.2 Candidate activity-file directories

| App | Roots probed for newest-mtime |
| --- | --- |
| Claude.app | `~/.claude/projects/`; `~/Library/Application Support/Claude/claude-code/`; `~/Library/Application Support/Claude/claude-code-sessions/`; `~/Library/Application Support/Claude/local-agent-mode-sessions/` |
| Codex.app  | `~/.codex/sessions/`; `~/.codex/session_index.jsonl`; `~/.codex/state_5.sqlite` |
| VS Code    | `~/.vscode/extensions/github.copilot*/`; `~/Library/Application Support/Code - Insiders/User/workspaceStorage/`; `~/Library/Application Support/Code - Insiders/User/globalStorage/github.copilot*/`; `~/Library/Application Support/Code - Insiders/logs/` |

The harness records mtimes for ALL of these per snapshot — we determine
post-hoc which ones actually move during active work.

## 4. Experiment matrix

For each app, four states × ≥30 ticks (5 s tick → ~2.5 min) per state.
Tick cadence matches the daemon's; resolution is acceptable for a >15 s
recency window.

### 4.1 Codex.app

| State | Setup |
| --- | --- |
| E1 idle | Codex.app open, no prompt in flight, foreground window irrelevant. |
| E2 active | Submit a prompt that produces a 30-90 s response. |
| E3 post-idle | Hold for 6+ min after E2 completes, until any candidate mtime ages past 5 min. |
| E4 closed | Quit Codex.app (`⌘Q`). Sample 30 s. |

### 4.2 Claude.app

| State | Setup |
| --- | --- |
| C1 idle (no LAM) | Claude.app open, browser-style chat only, no Local Agent Mode. |
| C2 LAM active | Start a Local Agent Mode session, give it a 30-90 s task involving file edits. |
| C3 post-idle | After C2, hold 6+ min. |
| C4 closed | Quit. |

### 4.3 VS Code + GitHub Copilot

| State | Setup |
| --- | --- |
| V1 idle | VS Code Insiders open in a folder with the Copilot Chat extension loaded; no Copilot interaction. |
| V2 chat | Open Copilot Chat, ask a question that produces a streamed answer. |
| V3 agent | Use Copilot agent mode (if the extension version supports `@workspace` agent edits) to run a 30-90 s task. |
| V4 post-idle | Hold 6+ min after V2/V3. |
| V5 closed | Quit VS Code. |

### 4.4 What we record per state

For each state, the harness produces a `<app>-<state>.ndjson` file. After
the run we annotate at least:

- which processes appeared **during active** that did not appear in idle;
- which mtimes advanced during active and stopped advancing after
  completion;
- whether any state advanced mtimes without a process appearing
  (false-active risk for activity-only detection);
- whether the process(es) survived state transitions (true for long-lived
  app-server / language-server processes — that's the whole reason we
  AND-gate against activity).

## 5. Synthesis — observations and per-app decisions

Experiments were run 2026-05-25. Raw NDJSON observation logs are in
`tests/experiments/phase-3/runs/` (gitignored). Key timestamps tagged
inline per app.

### 5.0 Cross-cutting discovery — phase 1 parsing bug

`lib/detect.sh:35-37` splits a `ps -axww -o command=` line into `exe + args`
by taking everything up to the **first whitespace** as the executable path:

```
exe="${command_line%% *}"
base="${exe##*/}"
```

This silently misparses any executable whose path contains a literal
space. The Claude.app experiment surfaced the case:
`/Users/thanga-5521/Library/Application Support/Claude/claude-code/<ver>/claude.app/Contents/MacOS/claude`
parses to `exe=/Users/thanga-5521/Library/Application`, `base=Application`,
which never matches the `claude|codex|copilot` case. Replay:

```
$ vigil_detect_line 87838 "/Users/.../Application Support/Claude/.../MacOS/claude --output-format ..."
(blank — no match)
```

The bug is also present for any `Codex Helper` path under
`/Applications/Codex.app/Contents/Frameworks/Codex Helper.app/...` — but
there the existing `/Applications/*` exclusion fires for the wrong reason
(the misparsed `exe` still starts with `/Applications/`), so the
`test_excludes_helpers_and_node_repl` assertion in
`tests/detect_test.sh:26-32` passes by accident. The Claude.app LAM path
has no such accidental coverage.

**Fix required for phase 3** (carries phase 1 hardening as a side effect):
two-pass `ps` so basename matching uses `ps -o comm=` (executable basename,
space-safe) and path-based exclusions use the full `ps -o command=` line as
substring matches. Both columns are joined by pid in a single awk pipeline
— two `ps` invocations per tick total, regardless of match count.

### 5.1 Codex.app — **needs new code**

**Experiment:** `tests/experiments/phase-3/runs/codex-active-2026-05-25.ndjson`
(362 ticks, 65.7 min). Tags: start=1779690559, codex-pause=1779690700,
codex-resume=1779694075, codex-done=1779694511.

**Process signature** — Codex.app keeps **22 stable PIDs** across the
entire run (active + 47 min idle + second active). The persistent
inventory includes:

- one main `/Applications/Codex.app/Contents/MacOS/Codex` (Electron host)
- 4 × `codex app-server …` workers under
  `/Applications/Codex.app/Contents/Resources/codex`
- 3 × `node_repl` workers under `/Applications/Codex.app/Contents/Resources/`
- Electron helpers (GPU / Renderer / utility)

**Process presence alone is meaningless** — none of these processes
correlate with agent activity. Detection requires AND-gating with an
activity signal.

**Activity signature** — `~/.codex/sessions/**/rollout-*.jsonl` mtime
advances every ~3–14 s during an active prompt (ages stayed under 10 s
in both active windows) and stops advancing within a tick of the agent's
final response. During the 47-min idle gap, the longest stretch of
"sessions newest >300 s old" was 2820 s (282 ticks) — i.e., the
rollout-file probe is silent during true idle.

**Noise filters** — `~/.codex/logs_2.sqlite-wal` (telemetry) and
`~/Library/Application Support/Codex/{Cookies,DIPS-wal,sentry/*}`
advance every few seconds throughout idle. Probing `~/.codex/sessions/`
specifically (which phase 1 already does for the `codex` CLI agent)
avoids both noise sources.

**Detection rule (decision):**

| field | value |
| --- | --- |
| host process pattern | `comm` matches the suffix-glob `*/Codex.app/Contents/MacOS/Codex` — covers `/Applications/...` (canonical), `~/Applications/...` (per-user install), and `/Volumes/<external>/...` (sideloaded). `Codex Helper` and the other Electron helpers have basename "Codex Helper" (with literal space) and do NOT match the suffix. |
| activity probe | reuse phase-1 `vigil_agent_is_active codex` (probes `~/.codex/sessions/**/rollout-*.jsonl`, window `VIGIL_IDLE_AFTER_SEC=300`) |
| refcount tag | `app-codex` — exactly one PID file per running Codex.app instance |
| gating | `app-codex` PID files contribute to refcount iff `codex_active=1`, same shape as `cli-codex` (intentional reuse: both share the `~/.codex/sessions/` write path, so a single probe correctly answers both "is the CLI doing work" and "is Codex.app doing work") |

### 5.2 Claude.app LAM — **needs parsing-bug fix only (audit pattern)**

**Experiment:** `tests/experiments/phase-3/runs/claude-lam-2026-05-25.ndjson`
(24 ticks, 4.55 min). Tags: start=1779694788, claude-start=1779695038,
claude-done=1779695075. Synthetic task: write 5 fun facts about
kangaroos to `/tmp/vigil-phase3-test.md`.

**Process signature** — when LAM kicked in, Claude.app spawned **pid 87838**:

```
/Users/thanga-5521/Library/Application Support/Claude/claude-code/2.1.142/claude.app/Contents/MacOS/claude --output-format stream-json --verbose --input-format stream-json --effort xhigh --model claude-…
```

- basename: `claude` ✓ (matches `claude|codex|copilot` case)
- exe path: **not** under `/Applications/*` ✓ (exclusion does not fire)
- path **contains a literal space** ("Application Support") ✗ — currently
  misparsed by `detect.sh` (see §5.0)

The disclaimer helper (pid 87837 at
`/Applications/Claude.app/Contents/Helpers/disclaimer`) is correctly
excluded by the existing `*/Helper*` pattern.

**Activity signature** — `~/.claude/projects/<encoded-cwd>/*.jsonl`
mtime advanced steadily through the LAM session (21 distinct values
across the run). This is the path phase-1's `vigil_agent_is_active claude`
already probes.

**Once §5.0's parsing bug is fixed, phase 1 detects Claude.app LAM
end-to-end with no Claude-app-specific code.** The bundled CC process
becomes a `cli-claude` row; the activity probe fires on the new jsonl
writes; refcount increments naturally.

**Decision:** audit pattern, no new agent type. Pin the contract with a
new fixture row capturing the LAM bundled-CC command line (path with
spaces) and a `test_picks_up_claude_app_lam_worker` assertion.

### 5.3 VS Code + OpenAI ChatGPT extension agent mode — **already covered (audit pattern)**

**Experiment:** `tests/experiments/phase-3/runs/vscode-copilot-2026-05-25.ndjson`
(16 ticks). Tags: start=1779695361, vscode-start=1779695518,
vscode-done=1779695568.

**Process signature** — submitting an agent task in the OpenAI ChatGPT
VS Code extension spawned:

```
/Users/thanga-5521/.vscode-insiders/extensions/openai.chatgpt-26.5519.32039-darwin-arm64/bin/macos-aarch64/codex app-server --analytics-default-enabled
```

- basename: `codex` ✓
- exe path: not under `/Applications/*`, no spaces ✓ — current
  `detect.sh` parses correctly, replay yields `cli-codex`.

**Activity signature** — writes to `~/.codex/sessions/.../rollout-*.jsonl`,
same path as Codex.app and the codex CLI. Phase-1's `codex` probe
already covers it.

**Decision:** audit pattern. Pin the contract with a fixture row of the
ChatGPT-extension-spawned codex worker and a
`test_picks_up_vscode_chatgpt_extension_codex_worker` assertion.

### 5.4 VS Code + GitHub Copilot Chat (Sonnet model) — **out of scope**

**Experiment:** `tests/experiments/phase-3/runs/vscode-chat-sonnet-2026-05-25.ndjson`
(22 ticks). Tags: start=1779695874, vsc-start=1779696001,
vsc-done=1779696127. User noted an interruption + auto-restart mid-session.

**Process signature** — no chat-specific process. The Copilot Chat
extension lives **inside VS Code's extension host** (a `Code Helper
(Plugin)` Node.js process that is always running while VS Code is open
and serves many extensions). The single new process observed during
this run, pid 31322, was the OpenAI ChatGPT extension's `codex app-server`
spawning for unrelated background work — not a GitHub Copilot Chat
worker.

**Activity signature** — `~/.codex/sessions/` did **not** advance during
the active window (the chat went through GitHub Copilot Chat, not the
OpenAI extension's agent path). VS Code workspaceStorage advanced
(9 mtime jumps, 119 s of mtime delta) — chat session state lives at
`~/Library/Application Support/Code - Insiders/User/workspaceStorage/<hash>/chatEditingSessions/<id>/state.json`
and `~/Library/Application Support/Code - Insiders/User/workspaceStorage/<hash>/GitHub.copilot-chat/debug-logs/<id>/models.json`,
both of which the experiment confirmed are written during chat. But
workspaceStorage is also bumped by file history, settings sync,
language-server caches, and many other extensions throughout normal
editor use — it is **not a clean activity signal** without further
filtering (the path-glob narrowing to `chatEditingSessions/*/state.json`
would help; per-hash workspace traversal is the cost).

**Decision:** **defer to a follow-up phase.** Rationale:

1. In-process JavaScript inside the extension host means there is no
   distinct process to anchor a host-process AND-gate against. The model
   we're using everywhere else (host process + activity probe) does not
   fit.
2. The activity-only signal is workspaceStorage, which is too noisy in
   its raw form. A scoped probe
   (`workspaceStorage/*/chatEditingSessions/*/state.json` and
   `workspaceStorage/*/GitHub.copilot-chat/debug-logs/*/models.json`)
   could work but needs a separate validation cycle: verify the file
   names are stable across extension versions, and confirm they aren't
   touched outside an active chat (e.g. by extension startup).
3. The empirical user workflow that drove this experiment was
   "Sonnet chat", not an agent task that runs for tens of seconds — the
   typical Copilot Chat response completes in <30 s, so even if vigil
   missed it the practical impact would be small (laptop sleeps after
   the chat completes; user is already at the keyboard so user-active
   sleep prevention is in play anyway).
4. Phase 3's other wins (Codex.app + Claude.app LAM detection + the
   parsing-bug fix) are already net-positive without this signal.

**Tracked in §7 as a phase-3.1 follow-up.**

### 5.5 Summary

| App / mode | Decision | New code? |
| --- | --- | --- |
| Codex.app | new `app-codex` host detection + activity gate | yes |
| Claude.app LAM | covered by phase 1 once §5.0 parsing bug is fixed | parsing fix + fixture |
| VS Code + OpenAI ChatGPT (agent) | covered by phase 1 as `cli-codex` | fixture only |
| VS Code + GitHub Copilot Chat (Sonnet) | deferred to phase 3.1 | none |

### 5.6 Non-goals (explicit)

- per-tick `lsof` calls (phase 5 portability cost — `lsof` is macOS/Linux
  only with substantially different output shape; the Windows port has no
  equivalent). The Codex.app host-process AND-gate makes `lsof` unnecessary
  here.
- per-app opt-in flags. Every detection rule in §5.1–5.3 is empirically
  validated, so they ship always-on per user direction.

## 6. Alignment with later phases

- **Phase 4 (lock feature)** — orthogonal. No interaction with detection.
- **Phase 5 (Rust cross-OS)** — every rule produced here is constrained to
  `process-name match + file-mtime check`. Both are portable; both already
  have known Rust crates (`sysinfo`, `walkdir`). Skipping `lsof` keeps the
  port mechanical.
- **Phase N (conflict detection)** — orthogonal. Conflict detection consumes
  the refcount transitions, not the per-agent identity.

## 7. Out of scope / follow-ups

- **Phase 3.1 — VS Code + GitHub Copilot Chat detection.** See §5.4.
  Needs a scoped activity probe under
  `~/Library/Application Support/Code{,- Insiders}/User/workspaceStorage/*/chatEditingSessions/`
  and `…/workspaceStorage/*/GitHub.copilot-chat/debug-logs/`, plus
  empirical validation that those paths are quiet outside an active
  chat and stable across extension versions.
- Standalone Copilot.app (`github/app`) tech preview. Becomes its own audit
  cycle when the user installs and chooses to enable it.
- Cursor, Windsurf, opencode. The harness in §3 is reusable; the per-app
  experiment matrix is the only thing that needs to be added per editor.

## 8. When this phase ships

1. §5 synthesis is filled in with traced evidence per app.
2. Per-app code changes (if any) land with new fixture rows.
3. `tests/run.sh` passes.
4. README + CHANGELOG + ROADMAP updated.
5. This file's status header flips from "experiment-driven plan" to
   "shipped — synthesis archived for the audit trail."
