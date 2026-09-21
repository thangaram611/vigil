# Phase 8 — Per-session activity: "working", not "open"

> **Status: PLAN, ready to execute.** Researched 2026-09-21 on this machine
> (Claude Code 2.1.278, Codex inside ChatGPT.app, Copilot CLI 1.0.86). Step 0 is
> measurement; do it first — two classifications below are still unmeasured.

## The problem

The menu said **"Holding — Active agents: 2"** when one Claude Code session had
just finished and the other had been idle for two days. "Active" should mean
*an agent is working right now*, not *an agent is open*.

Three defects, all confirmed in code and on the live machine:

1. **One flag per provider, one count per process.** `scan::is_active` is true if
   *any* `*.jsonl` under `~/.claude/projects/` changed recently, and
   `refcount::count` then counts *every* `claude` pidfile. One working session
   makes every open session "active". Two processes → "2".
2. **A five-minute tail.** `window_secs` rounds `VIGIL_IDLE_AFTER_SEC` (300) up to
   whole minutes, so a finished turn holds the Mac for up to five more minutes.
3. **Codex is not detected at all.** Codex now runs as
   `/Applications/ChatGPT.app/Contents/Resources/codex app-server`. The
   `/Applications/` exclusion drops it and the `Codex.app` carve-out does not
   match, so Vigil has no pidfile for it: Codex work gets **no hold**.

## Why not just tighten the transcript-mtime window

That was the first idea, and the measurement says no. With a background shell
task running after the turn ended, this session's transcript went silent — age
climbing to 84 s and counting — while real work continued. A short mtime window
would have released the hold mid-work. This is the exact failure adrafinil
shipped ([#7](https://github.com/kageroumado/adrafinil/issues/7): "Mac sleeps
while agent is waiting for a background workflow"). Transcript mtime is a proxy;
every agent here exposes something better.

## What each agent actually exposes

### Claude Code — a per-process status file

`~/.claude/sessions/<pid>.json`, rewritten on every state change:

```json
{ "pid": 28778, "sessionId": "bb2bf3cc-…", "cwd": "…/vigil",
  "procStart": "Sun Sep 20 05:41:47 2026", "version": "2.1.278",
  "status": "busy", "statusUpdatedAt": 1789975149938 }
```

Measured on this machine:

| Situation | `status` | `waitingFor` | own `caffeinate -i` child | transcript |
| --- | --- | --- | --- | --- |
| mid-turn | `busy` | — | yes | fresh |
| turn ended, **background shell still running** | **`shell`** | — | **yes** | **silent, 84 s+** |
| idle, asked a question two days ago | `waiting` | `input needed` | no | stale |

- Documented values ([agent view](https://code.claude.com/docs/en/agent-view)):
  `status` ∈ `busy` / `waiting` / `idle`; `waitingFor` ∈ `permission prompt`,
  `input needed`, `sandbox request`, `worker request`, `dialog open`. The binary
  also contains `goal proposal`, and `shell` is undocumented — so **unknown values
  must be handled, not assumed away**.
- The supported reader is `claude agents --json --all`: same fields, but 0.59 s
  per call, and it spawns a `claude` process that Vigil's own detector would
  count. Right for a `doctor` contract check; wrong for a 5-second tick.
- Hooks cannot replace this: the hook payload has `session_id` and
  `transcript_path` but **no pid**, so a hook cannot say *which process* is busy.
  Vigil stays hookless.
- `pid` + `procStart` give a pid-reuse guard for free.

### Codex — turn boundaries in the rollout

`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` brackets every turn with
`event_msg/task_started` … `event_msg/task_complete`. A session is working iff
its last boundary record is `task_started`.

### Copilot CLI — turn boundaries in the event log

`~/.copilot/session-state/<id>/events.jsonl` has `assistant.turn_start` /
`assistant.turn_end`, plus `tool.execution_start|complete` and
`subagent.started|completed`. Same rule.

### VS Code + Copilot Chat

Unchanged. The content-hash gate already measures real change, not file churn.

## Design

**Activity becomes a property of a session, decided by the agent's own signal,
with today's mtime scan kept only as the fallback.**

Claude Code, per pidfile, first rule that answers wins:

1. `sessions/<pid>.json` parses, `pid` matches, and `procStart` matches the
   process start time → **active iff `status` ∈ {`busy`, `shell`}**; `waiting` and
   `idle` are not active.
2. File missing, unparseable, mismatched, or `status` unrecognised → **active iff
   the process has a `caffeinate -i` child**. Free: the daemon already walks the
   process table every tick.
3. Neither available → today's provider-wide mtime window.

Codex and Copilot, per provider: tail-read (last 64 KB) each session file
modified in the last 24 h; a session is working iff its last boundary record is
a start. Count the working sessions, and require at least one live process of
that provider. A **stall guard** — a start record older than 30 minutes with no
writes since — reads as not working, so a crashed turn cannot pin the Mac awake.

Then:

- `refcount::count` takes a per-pidfile predicate instead of four provider bools.
  `refcount_active` in the tick file keeps its meaning (things holding the Mac
  awake) but is now sessions, not processes.
- The 300 s tail is replaced by a **fixed 30 s grace** that only debounces
  back-to-back turns. `VIGIL_IDLE_AFTER_SEC` remains, governing the fallback
  only. No new knob unless Step 0 shows 30 s is wrong.
- Detection: carve `…/ChatGPT.app/Contents/Resources/codex` out before the
  `/Applications/` exclusion, as `app-codex`.
- `vigil status` and the menu show what is working — `claude: 1 working,
  1 waiting` — instead of a bare number.
- `vigil doctor` gains an on-demand contract check: direct file read versus
  `claude agents --json --all`, plus the Claude Code version. If Anthropic changes
  the file, doctor says so and rule 2 carries the load meanwhile.

## Steps, each with its check

0. **Measure the unknowns.** Use a *detached* sampler (`nohup … & disown`) — a
   tracked background task would itself flip the status to `shell`. Cases:
   background subagent after the turn ends; `/loop`; a permission prompt (run one
   session without `--dangerously-skip-permissions`); `AskUserQuestion`; plan-mode
   exit; Claude.app Local Agent Mode (does it write `sessions/<pid>.json`?); a
   `kind: background` session. For Codex: a live turn in ChatGPT.app, and the
   record an interrupted turn writes. For Copilot: one live turn.
   *Check:* `docs/agent-activity-signals.md` has a row per case and no
   "unknown" for any state Vigil must classify.
1. **Failing reproduction.** Two `claude` pidfiles, fixture session files — one
   `busy`, one `waiting` → count must be 1. Today it is 2.
2. **`activity/claude_sessions.rs`:** parse, classify, guards. Table test:
   every known status, unknown status, missing file, bad JSON, pid mismatch,
   `procStart` mismatch → each lands on the intended rule.
3. **Rewire `refcount::count`, the daemon tick, and `CheckEngine`.** *Check:*
   step 1 passes; the existing suite stays green.
4. **Codex and Copilot tail parsers**, with fixtures captured from this machine
   and scrubbed. *Check:* start-last → working; complete-last → idle; stalled
   start → idle; a truncated last line is ignored, not fatal.
5. **ChatGPT.app carve-out.** *Check:* a detect test, and `vigil debug` lists the
   live `codex app-server`.
6. **Grace window, status/menu text, doctor contract check.**
7. **Live verification.** Two sessions, one idle → menu says 1. Finish the turn →
   0 within ~35 s and `SleepDisabled` returns to `No`. Start a background shell
   and end the turn → stays 1 until it exits. Run a Codex turn in ChatGPT.app →
   held. Re-run the clamshell `release` arm → still sleeps.
8. **Docs:** README agent-coverage table, `docs/architecture.md`, ROADMAP, CHANGELOG.

## Risks

- **`sessions/<pid>.json` is not a documented interface.** Mitigated by the rule
  ladder, the doctor contract check, and treating unknown statuses as "ask the
  next rule". It is already depended on by several public tools
  ([claude-busy-monitor](https://github.com/pbauermeister/claude-busy-monitor),
  claude-status, ccmon), and
  [claude-code#94620](https://github.com/anthropics/claude-code/issues/94620)
  asks Anthropic to bless it.
- **Status lag.** Unmeasured; the 30 s grace absorbs it. Step 0 records the
  worst case.
- **Releasing sooner means sleeping sooner.** With the closed-lid fix, a release
  under a closed lid now sleeps the Mac within seconds. That is intended, and it
  is why Step 0's background-subagent case must be measured before shipping: a
  wrong "idle" there is a lost overnight run.

## Decisions needed before executing

1. **Does `waiting` on a *permission prompt* release the hold?** Recommended:
   yes. Nothing progresses until you answer, and it matches "actively working".
   adrafinil reached the same conclusion
   ([#20](https://github.com/kageroumado/adrafinil/issues/20)).
2. **Is a 30-second grace right?** It trades a slightly earlier sleep against
   flapping between quick back-to-back turns.
