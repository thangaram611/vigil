# Phase 2 — copilot-companion integration

> **Status: AUDITED — no code change needed.** Phase 1's existing
> `copilot` CLI handling already covers copilot-companion correctly.
> This file is retained as the audit trail.

## What we thought we needed

The original sketch borrowed `hiddenest/awake`'s session-provider model:
detect the companion's long-lived node ACP daemon
(`copilot-acp-daemon.mjs`) and gate it on per-thread mtime inside
`~/.claude/copilot-companion/threads/`. That sketch assumed the
*daemon* is what does the work and therefore needed its own detection.

## What we actually have

Architecture trace (from
[`copilot-companion/scripts/copilot-acp-daemon.mjs`](https://github.com/thangaram611/copilot-companion)):

- `node /…/copilot-companion/scripts/copilot-acp-daemon.mjs` is the
  long-lived router. It does **no agent work itself**. It listens on
  `/tmp/copilot-acp.sock`, owns the Copilot ACP session map, and
  shuttles JSON-RPC between the bridge and the worker.
- Per Copilot session it spawns a child via
  `spawn(COPILOT_BIN, COPILOT_FLAGS)` (`copilot-acp-daemon.mjs:33-46,
  211-223`). `COPILOT_BIN` resolves via `command -v copilot` →
  `/opt/homebrew/bin/copilot` (a Node-SEA Mach-O binary). Flags:
  `--acp --model … --reasoning-effort … --no-ask-user
  --allow-all-tools --allow-all-paths --allow-all-urls --experimental`.
- The worker writes session events to
  `~/.copilot/session-state/<uuid>/events.jsonl` regardless of `--acp`
  mode (confirmed on disk and via live observation).

Phase 1's existing pieces line up exactly:

1. `lib/detect.sh:33-50` matches any basename `copilot` →
   `cli-copilot`. The companion-spawned `copilot --acp` worker is one
   such process. The companion's `node` daemon (basename `node`) is
   correctly **not** matched.
2. `lib/activity.sh:43-56` (`vigil_agent_is_active copilot`) probes
   `~/.copilot/session-state/**/events.jsonl` for mtime within
   `VIGIL_IDLE_AFTER_SEC` (default 300s). The companion worker writes
   to that exact path.
3. `lib/refcount.sh:71-87` only contributes the worker to the engaged
   refcount when the activity flag is set — so a heartbeat-extended
   idle worker (companion keeps it alive between prompts via
   `HOST_LIVENESS_TTL_MS = 30 min`) correctly drops out of the
   refcount 5 minutes after the last `events.jsonl` write.

## Live verification (2026-05-21)

`./tests/run.sh` → `pass=61 fail=0` (60 pre-existing + 1 new targeted
`test_picks_up_copilot_companion_acp_worker`).

Live job dispatched against the running companion daemon:

- Initial state: `copilot=idle`, refcount 3 (claude only). Companion
  router (pid 2802) running but no worker child.
- A 1-token job triggered the daemon to spawn worker pid 86946:
  `/opt/homebrew/bin/copilot --acp --model gpt-5.5 …`. Parent pid 2802
  is the node router; pid 86946 is the worker.
- A fresh
  `~/.copilot/session-state/216c41aa-e58f-4b7d-b769-c8e6288957b6/events.jsonl`
  appeared and grew during the prompt.
- Mid-job `vigil status`:
  ```
  refcount: 4 active / 4 total   (idle window 5m)
  agents:   claude=active  codex=idle  copilot=active
  active matches:
    pid=86946  name=cli-copilot  age=6s  active
  ```

One harmless race observed: on the first tick after the worker spawned
but *before* it wrote to `events.jsonl`, the worker's PID file existed
but `copilot=idle`. The next tick (5s later) flipped to `active`. At
most one tick of "running but not yet counted" delay. Acceptable; no
fix needed.

## Why we don't watch
`~/.claude/copilot-companion/threads/` or `jobs/`

Those directories carry per-job metadata (retention, terminal status,
session id mappings). They're *not* the authoritative in-flight signal:

- Job files are atomically replaced at start and at terminal — between
  those two writes (which can be many minutes for a long prompt), the
  mtime is stale even though the worker is actively running. mtime
  alone would false-idle a long worker.
- The right signal is `terminalAt:null` in the JSON body, but that
  requires JSON parsing.
- The worker's `events.jsonl` mtime is updated continuously while the
  prompt is being processed — a strictly better signal, and it's what
  phase 1 already probes.

## What this means for later phases

- **Phase 3 (desktop apps):** validates the same model. Phase 3's
  per-app probes can mirror `vigil_agent_is_active`'s shape.
- **Phase 5 (Rust port):** no separate `copilot-companion` agent type
  is needed in the rewrite. One `copilot` agent (basename match +
  per-agent session-storage path) covers both standalone CLI and
  companion-spawned workers.
- **Phase N (conflict detection):** orthogonal.

## Test coverage anchor

The fixture
`tests/fixtures/ps-axww-snapshot.txt:409` captures a live
`/opt/homebrew/bin/copilot --acp …` worker line.
`tests/detect_test.sh::test_picks_up_copilot_companion_acp_worker`
pins the contract: that PID maps to `cli-copilot` with `--acp` and the
full resolved binary path preserved in the TSV row. If a future
refactor breaks this, that test fails — keeping the audit alive.
