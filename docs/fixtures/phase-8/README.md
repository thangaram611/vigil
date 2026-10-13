# Phase 8 evidence fixtures

Captured September–October 2026; preserved October 13 at Vigil `5df1f8a`.
These are research inputs for the implementation described in
[the handoff](../../../future/phase-8-per-session-activity.md), not implemented
Vigil behavior. `manifest.json` records source hashes, projection hashes,
versions and cleanup. Do not depend on the original temporary directories.

JSONL files retain selected **native event envelopes**, ordering, timestamps,
correlation IDs and lifecycle fields. Prompts, reasoning, source contents,
command text and unrelated telemetry are removed. Placeholder paths are not
resolvable. Missing fields/records in these projections are not evidence that
those fields/records were absent from the original log. In particular, do not
use a projected fixture's `parentId` chain to prove log completeness. JSON
`*-observations` and `*-summary` files are observer evidence, not provider schemas.
Synthetic mutation tests must identify themselves as synthetic; they are not
additional measured cancellation or restart cases.

| Files | What they establish | Expected assertions in future Rust tests |
| --- | --- | --- |
| `claude-parent-2.1.296.jsonl`, `claude-child-2.1.296.jsonl`, `claude-child-meta.json` | Native async launch, sidechain ownership, normal end and duplicated enqueue/remove notification | Dispatch does not finish child; matching completion closes its invocation once; root question/permission does not erase child |
| `claude-wait-observations.json`, `claude-subagent-summary.json` | Child overlaps parent human waits; 262.798 s of quiet synthesis | No mtime expiry or caffeinate requirement; after child completion, parent wait alone supplies no confirmed work |
| `claude-loop-{pending,cancelled,final}.json` | Live attached background worker's wake and sidecar clear on Esc; raw job state remains working | Pending valid loop supplies demand; cancellation removes it; raw working state does not restore it |
| `copilot-shells-1.0.95.jsonl`, `provider-shells-summary.json` | Async shells survive final answer and abort; interrupted read lacks completion | Separate shell lifetime; actor abort clears its wait without clearing independent shell |
| `copilot-subagents-1.0.95.jsonl` | Child/root interleaving, parent question, child permission, parent abort and surviving child | Use `data.parentToolCallId`; permission joins through `permissionRequest.toolCallId`; task dispatch completion is not subagent completion |
| `codex-terminals.jsonl`, `codex-terminals-summary.json` | Real desktop host's yielded commands outlive normal/aborted turns | Exact turn IDs; independent process ownership; late CommandExecution completion cannot reopen turn |
| `codex-detached.jsonl`, `codex-detached-observations.json` | Dedicated server's user-shell descendant outlives command and host, reparented to PID 1 | Empty terminal list and ended command cannot prove all descendants exited; cold ownership remains uncertain |
| `codex-approval-pending.jsonl`, `codex-approval-finished.jsonl`, `codex-approval-rpc.json` | Dedicated server reports live approval wait, with no corresponding persisted wait event at the captured point | Rollout-only reader cannot claim exact permission-wait classification; never import the RPC observation as a disk field |
| `local-analysis-result.json` | Scoped sysinfo environment access works; current delayed wrapper registration yields DropPidReuse | Add a failing regression for actual start time before changing wrapper registration |

Copilot October 13 checkpoints (UTC):

- 08:26:37.573: child starts, origin `toolu_01HNyzmRyabEvQH931nNaEMY`.
- 08:26:39.712: root `ask_user` starts. Child continues, so session has work.
- 08:26:48.862: child completes. Question remains open until 08:33:11.766
  (use native completion timestamp in the fixture as authority). Wait alone is
  not active work. No `user_input.requested` record was observed.
- 08:33:51.260: second child starts, origin `toolu_011t1qs9Jzh6sm2n7wnNmXLz`.
- 08:33:53.309: child Bash permission requested; root subsequently waits in
  `read_agent`. Work is blocked on the human, not two independent running jobs.
- 08:34:26.913: permission completed; own workload starts at 08:34:26.989.
- 08:34:28.638: parent abort. Workload and child survive.
- 08:35:36.990: workload exits; native Bash completion 08:35:37.007.
- 08:35:39.773: child completes, then native agent-idle notification starts another
  parent turn, ending 08:35:42.195. The original interrupted `read_agent` call has
  no matching completion. A later automatic read has a different call ID.

Child shell marker `COPILOT_AGENT_SESSION_ID=7e9ad0d5-da9b-4dcd-9309-a3ad638159d2`
is neither the root session nor the dispatched agent ID. It was natively set;
its value alone is not a root-session ownership mapping. Shell PID 31547 belonged
to Bash 31546 under the uniquely identified Copilot host 14559.

The two October 13 Codex probes used a dedicated, documented stdio app-server,
not the desktop app's transport. Its execution path for detachment was
`thread/shellCommand`, distinct from the October 12 `exec_command` experiment.
Do not generalize its restart result to every desktop/PTY/remote path.

Fixture verification is offline: parse every line/object, verify hashes in
`manifest.json`, and replay the checkpoints. Large records, truncation, PID
reuse, duplicate notifications and malformed fields can be generated locally
inside Cargo tests. Re-run live provider research only to extend coverage or
resolve an actual schema/semantic change; routine implementation should use
these fixtures and the documented unknown-state behavior.

## Implementation acceptance — October 13, 2026

The Phase 8 readers now replay these fixtures in Cargo tests. Additional
implementation regressions cover partial/oversized/replaced histories, ownership
by host incarnation, mixed old/new work, missing process metadata, witnessed
reparenting, read-only IPC trees, grace and acknowledged refresh. The bounded
live OS fixtures use synthetic provider hosts and real process metadata; they
are distinguished from actual provider measurements.

A disposable foreground Copilot session was also measured against the installed
Vigil build. Its launcher version was 1.0.96; the new native session recorded
producer 1.0.95 with auto-update disabled. It yielded 20 confirmed working samples
and no remaining session after host exit. Scrubbed native events and observations
are in [the implementation evidence](../phase-8-implementation/copilot-smoke-summary.json).
This smoke does not extend the source-only Claude lifecycle, Desktop, remote,
or cold recovery claims. Physical lid/sleep acceptance was not performed.

Final verification passed all required Cargo checks, including the helper seam
suite, 18 core activity regressions and 15 bounded live OS acceptance cases.
A fresh read-only subagent independently passed the core/live cases and found
no remaining actionable blocker. The [final installed acceptance](../phase-8-implementation/installed-acceptance.json)
records matching release/install hashes, schema 2 with a matching daemon
identity, and two concurrent acknowledged refreshes. `doctor` exited 0 with
the expected warning that helper responsiveness is unchecked by read-only
diagnostics.
