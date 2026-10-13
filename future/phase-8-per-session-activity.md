# Phase 8 — Per-session activity implementation handoff

> **Research handoff, October 13, 2026; implemented locally the same day.**
> Research baseline: `main` at `5df1f8a`. This document preserves the accepted
> contract. Current behavior is in [the README](../README.md), with
> [implementation acceptance](../docs/fixtures/phase-8/README.md#implementation-acceptance--october-13-2026).
> The [evidence fixtures](../docs/fixtures/phase-8/README.md) preserve scrubbed
> native records, observations, provenance hashes and expected outcomes.
> [The research log](../docs/agent-activity-signals.md) retains dated detail;
> its older proposals are historical and are superseded by this handoff.

## 1. Objective, authorization and readiness

Count **sessions that need this Mac awake**, independently of how many agent
processes are open. Combine foreground work, background commands, subagents and
supported self-paced loops. Explain why Vigil holds or releases. An ended turn,
a human prompt or another child's completion must not erase independent work.

The user requested research before coding, approved disposable experiments, then
on October 13 authorized analysis/testing without further permission and asked
for this complete handoff for implementation in the next session. Keep real
sessions intact. The research task does not authorize shipping a claim of exact
coverage where the native surface cannot provide it.

**Implementation can start with the contract below. Universal exact detection
is not achievable from the currently measured local files.** In particular,
Codex approval waits are live API state absent from the sampled rollout, and
cold discovery cannot reliably attribute every detached process. These are
observability limits, not work to solve with a timeout or another identical
experiment. The implementation must expose them as uncertain coverage. Full
coverage would need an explicitly designed provider integration; do not sneak
private app connections, settings hooks or a second daemon into this phase.

Research disposition: every remaining surface was investigated; several have
source/documentation evidence only. Claude's new live probe was blocked by its
provider safeguard before tools ran. Claude Desktop was not installed. Neither
is a passed experiment. Do not repeat completed probes merely to begin coding.
Unmeasured claims remain unverified until a future coverage extension provides
new evidence.

## 2. Decisions and success criteria

Already accepted by the user:

- A supported Claude self-paced loop keeps the Mac awake between iterations
  until stopped. **Automatic loop-gap tracking requires background mode.**
  Ordinary interactive turns remain covered; interactive loop gaps need `/bg`
  or a manual hold.
- Research and probes use disposable sessions; existing sessions are not test
  subjects. Implementation belongs to the next session.

Design choices proposed by this handoff, not earlier user votes:

- A **fixed-interval cron schedule waiting for its next fire does not itself
  hold**. Its executing invocation is ordinary work. Otherwise an hourly/daily
  schedule could keep the Mac awake indefinitely. Use an explicit manual hold
  if unattended local schedule delivery must be guaranteed. Do not conflate
  fixed cron with the accepted self-paced-loop policy.
- Keep independently verified local process work through host exit while its
  `(PID, start time)` and established ownership still match. Cold discovery
  without that witness is unknown. Do not persist a second ownership database.
- A local orchestrator executing remote tools still needs this Mac awake.
  Merely viewing a session hosted entirely elsewhere supplies no local demand.
  Unproven remote topology is unknown; the word “remote” alone decides nothing.
- Include the narrowly related wrapper identity and read-only diagnostic fixes
  in integration, with separate failing reproductions. They are described below;
  this research pass has not repaired them.

Success means:

1. Two Claude sessions, one busy and one waiting, yield one working session.
   Multiple working Codex sessions in one host count separately. One session
   with multiple work sources counts once.
2. A fully observed wait with no other work releases demand. A parent wait or
   Stop with independently running work retains demand. A child blocked on
   permission is not automatically treated as running because it was launched.
3. Quiet, verified work never expires merely because no transcript was written.
   The measured Claude child synthesized silently for 262.798 seconds.
4. Unsupported, missing, partial or contradictory evidence becomes **unknown**,
   never a fabricated exact idle/working count. Unknown and its power policy
   are visible separately.
5. Actual activity, uncertain demand and the fixed 30-second release grace are
   separate. Refresh consumes grace; a later ordinary tick cannot resurrect it.
6. Daemon, recovery and read-only diagnostics share collectors/reducers. Thermal,
   battery, cooldown and manual-off still outrank automatic demand.

No cross-OS, packaging or release work. Preserve root-helper fixed actions and
both closed-lid follow-ups. Keep VS Code Copilot Chat's existing hash gate and
`vigil run` / `vigil lock` wrapper semantics.

## 3. Evidence ledger — do not repeat these measurements

Native timestamps are UTC; the original narrative sometimes presents IST
(UTC+05:30). Observation delay is **file-event to sampler**, not UI action to
file, and is not a provider latency guarantee.

| Surface/version | Established result | Consequence / evidence |
| --- | --- | --- |
| Claude 2.1.295/2.1.296 foreground | `busy`, `shell`, `idle`, and native question/Read-permission/ExitPlanMode `waiting` observed; UTC `procStart` | Validate process identity; status describes foreground only |
| Claude 2.1.296 shell + human wait | Quiet background commands continue through question/permission while own caffeinate disappears | `provider-shells-summary.json`; combine independent ownership |
| Claude 2.1.296 subagent + parent wait, Oct 12 | One child runs 368.910 s; 193 question and 138 permission samples overlap it; 162/108 respectively lack caffeinate | Parent/child/meta fixtures; no mtime/caffeine cutoff |
| Same child completion | Child `end_turn` 10:48:27.296; parent native enqueue 10:48:27.370; queue removal repeats notification 10:49:28.222 while permission is answered | Close matching invocation once; notification arrives before parent unblocks |
| Claude interactive self-paced loop | Esc clears UI pending loop but leaves no replayable cancellation record | Cannot infer current loop from historical ScheduleWakeup calls |
| Claude background loop, attached | Pending `inFlight.wake` and `.pending-wakeup` agree; both clear after Esc; raw job `state` remains `working`; 112 samples past due time stay clear | `claude-loop-*`; raw job state is insufficient |
| Codex desktop, CLI 0.162.0-alpha.17.2, app 26.1007.30912 | Real normal completion and user Stop persist exact turn IDs; independent sessions share app-server PID 5148 | Correlate sessions and turns, not one provider/host flag |
| Same desktop, yielded local terminals | Shells survive final answer by 86.025 s and Stop by 112.842 s; native `CODEX_THREAD_ID` matches owner; later CommandExecution exit records name original turn | `codex-terminals*`; logical IDs 8395/86423 are not OS PIDs 95562/97891 |
| Codex dedicated app-server, Oct 13 | `thread/shellCommand` detached descendant is reparented to PID 1, retains native thread marker, outlives completed command and host replacement; terminal lists empty before/after resume | `codex-detached*`; user-shell path, not proof for every desktop/PTY execution path |
| Codex dedicated app-server approval, Oct 13 | Live `thread/read` gives `activeFlags: [waitingOnApproval]`; pending rollout has tool call but no corresponding wait event. Declined once; turn finishes, file not created | `codex-approval-*`; rollout alone cannot promise exact human-wait release |
| Copilot CLI 1.0.95, Oct 12 | Async Bash survives final and user abort; shell-completion notification may start another turn; interrupted `read_bash` never completes | `copilot-shells-*`; turn/tool/shell lifetimes differ |
| Copilot 1.0.95, Oct 13 normal child + question | Child 08:26:37.573–08:26:48.862; root ask_user starts 08:26:39.712 and stays open after child ends | `copilot-subagents-*`; actor is `data.parentToolCallId`, not global parentId |
| Same version, child permission + parent abort | Child Bash permission waits 08:33:53.309–08:34:26.913; root waits in read_agent. Parent abort 08:34:28.638; workload exits 08:35:36.990, child completes 08:35:39.773 | Permission joins via toolCallId; root abort preserves child; interrupted read_agent never completes |
| Copilot child ownership | Native shell session marker differs from both root session UUID and task agent UUID | Join native actor records and unique validated host; do not require child marker == root or trust marker alone |
| Rust/sysinfo 0.39.3 | Actual scoped environment read, process start and parent lookup succeeded in a disposable Rust program linked to installed dependencies | `local-analysis-result.json`; no new dependency needed |
| Existing wrapper registration | Pure reproduction using current library returns DropPidReuse for live wrapper registered at 1010 with actual start 1000 | Fix registration identity; keep reuse protection |

All October 12 completed probes were cleaned up. October 13: both CLI controllers
exited normally; recorder stopped after 732 samples without sample errors; owned
Bash/workload processes were gone; all three dedicated Codex servers exited 0,
both Codex threads were archived, and detached workload finished under its own
100-second alarm. Known probe PIDs were checked absent. No Vigil reload,
LaunchAgent replacement or live power change occurred.

### Remaining-surface conclusions and confidence

| Investigated surface | Evidence level | Implementation rule |
| --- | --- | --- |
| Claude cancel/failure/resume | Installed 2.1.296 source has completed/failed/killed notifications, optional runId, metadata stoppedByUser, and repeated notifications for resumed task IDs. New live setup was provider-blocked | Parse recognized correlated terminals, never globally finish an agent ID; unsupported/ambiguous invocation transitions remain unknown. Synthetic tests do not make this live-verified |
| Claude child permission / nested surviving work | Official docs route child prompts to parent; installed code allows completion while owned background work remains. Exact on-disk attribution not established | Whole-session `waiting` plus an unresolved child is not enough to prove which actor runs. Preserve independent verified work; mark unresolved actor state unknown |
| Claude fixed cron | Official schedule contract and installed scheduler branches inspected; no new CronCreate live test | Do not hold for pending fixed schedule. Do not treat generic `session_cron` count as self-paced loop. Actual execution uses ordinary evidence |
| Claude detached background job / worker restart | Documented terminal-independent supervisor; source checks live PID/procStart/zombie and persists wake sidecar. Attached pending/clear is measured; detached/restart behavior not live-verified | Attachment process is not owner. Validate current worker and matching wake; after changed worker identity require fresh evidence, never resurrect from stale sidecar |
| Claude Desktop Local / Cloud / SSH | Official documentation distinguishes execution host; no actual Desktop bundle found in `/Applications` or `~/Applications` (URL-handler app is not Desktop) | Reuse Claude reader only if native worker identity/schema is validated; app presence alone unknown. Cloud viewer alone has no local work |
| Off-machine Codex execution | Public remote/app-server architecture, installed schema; no external host tested | Local host/orchestrator governs local demand. Do not label untested remote execution covered |
| Codex approval/user-input waits | Approval API vs disk mismatch directly measured; installed schema also lists waitingOnUserInput | No exact disk-only wait guarantee. Active unresolved tool with unavailable live wait state is unknown; don't invent a saved activeFlags field |
| Cold orphan discovery / restarted Vigil | Detached marker survives, but original ancestry/host may be gone | Without earlier validated witness, unknown. Existing transcript alone does not restore current work |

Official references: [Claude subagents](https://code.claude.com/docs/en/sub-agents),
[scheduled tasks](https://code.claude.com/docs/en/scheduled-tasks),
[Desktop execution environments](https://code.claude.com/docs/en/desktop),
[Codex app-server protocol](https://learn.chatgpt.com/docs/app-server).
The dated research log records installed-source offsets and qualifications.
Current scheduling docs describe CronCreate restoration; installed source also
contains flag-dependent in-memory/durable branches. Do not assume docs describe
every enabled feature on this machine, or use restoration as evidence of work.

## 4. Minimal model and ownership contract

Keep this inside `activity`, with small Claude/Codex/Copilot readers and shared
JSONL buffering/aggregation. Keep provider semantics in their readers. No generic
workflow engine, database, blanket environment capture or duplicate full scan.

Conceptual types (names may follow local conventions):

- `ProcessIdentity { pid, start_time }` plus collected parent, executable and uid.
- `SessionKey { provider, native_id }`; a host can own several sessions.
- `SessionActivity { key, state, evidence, reason, foreground_wait }`.
  `state = working | waiting | idle | unknown`. Evidence distinguishes direct
  native state, correlated lifecycle, verified local process and uncertain data.
- Provider-private outstanding turns/calls/child invocations and wait dependencies.
  Shells/children are work belonging to a session, not extra counted sessions.
- `ActivitySnapshot`: session counts, raw detected hosts, wrapper demand,
  VS Code demand and explicitly uncertain demand. Do not subtract hosts from
  working sessions to infer idle sessions.

Aggregation precedence: **verified running work → working; unresolved evidence
that could affect remaining work → unknown; all unresolved work human-blocked →
waiting; fully resolved and no pending work → idle**. A session may show a
foreground-wait detail while classified working. A native loop gap is deliberate
scheduled demand, displayed as such rather than model computation.

An open turn proves an unfinished lifecycle, not universally active computation.
For Codex, generation with no unresolved tool/approval ambiguity can count as
working; an outstanding opaque tool call without execution/wait evidence is
unknown. For Claude, an async child with an outstanding call that may own the
reported permission wait is unknown unless attribution resolves it. This avoids
turning the successful parent-wait experiment into an unsupported universal rule.

### Process collection

Extend `ProcScanner`, which owns process collection today. Reuse its PID/start
snapshot for detection, liveness, parentage and ownership, including GC where
possible; do not keep the existing separate full GC scan and add another.

1. Match precise executable **and role**, same user and valid non-zombie identity.
   Auxiliary servers, supervisors, attach clients and caffeinate aren't sessions.
2. Join native session files/open rollout descriptors/in-use records to that
   identity. Read paths from validated metadata, not transcript prose.
3. For candidate owned commands only, read the needed native marker:
   `CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`, `COPILOT_AGENT_SESSION_ID`.
   A small temporary `sysinfo::System` scoped to candidate PIDs may extract and
   discard environments: the long-lived System otherwise retains every variable.
   Store only the selected marker. Never log full environments or credentials.
4. Nested providers inherit launcher markers. Prefer the nearest validated
   owning provider; reconcile its native session/child evidence. A marker is
   corroboration, not an identity or “busy” bit. Long-lived MCP/helper processes
   are not automatically active commands just because they inherited it.
5. Recheck identities after collection. PID reuse, inaccessible env, races,
   conflicting hosts or partial joins give unknown. Canonicalize path aliases
   such as `/var` versus `/private/var`; compare normalized UTC process starts.
6. Cache a validated workload's own identity/session association while alive.
   Reparenting or host exit does not erase it. Drop on verified exit/reuse;
   a temporary probe failure is unknown, not exit. At cold startup a marker
   alone cannot recreate this witness. Never revive old turns under a new host
   merely because that host opened a historical rollout.

Codex open-file association is read-only evidence of which host loaded a
rollout, not whether its old last turn is executing. Use a bounded targeted
`lsof` collection initially (measured on the real desktop host); timeout or
permission failure is unknown. Do not invoke provider list commands that mutate
state, attach private transports, or spawn an app-server as an observer.
`~/.codex/process_manager/chat_processes.json` never tracked the measured
terminals and must not become a source of truth.

### Unknown and power demand

Exact counts include only identified sessions with supported evidence. Never
multiply provider fallback by detected PIDs. Existing `VIGIL_IDLE_AFTER_SEC`
(default 300, currently rounded to whole minutes) remains only for recent-file
fallback, at most one uncertain demand per provider where some evidence is
unresolved. Explain why it applies. Resolved idle/waiting sessions alone cannot
activate provider fallback, even if their files are recent.

An unfinished, currently owned **native turn** with ambiguous wait state retains
uncertain demand until its matching completion/abort or ownership invalidation;
do not expire it after 30 minutes. It is displayed as unknown/unfinished-turn,
not as an exact working session. This intentionally may hold through an
unobservable Codex permission wait. A stale/unowned transcript gets only bounded
fallback if otherwise eligible, never this indefinite hold.

Similarly, a well-correlated live-host child invocation with missing terminal
or ambiguous wait keeps uncertain demand; contradictory native state marks the
uncertainty explicitly. Stop cannot clear independent work without its own
terminal/exit evidence. Do not promise automatic release for the unverified
Claude child-cancellation paths. Manual-off remains an immediate escape.

`refresh` consumes grace and reevaluates evidence; it does not override real work
or uncertain native-lifecycle demand. Display that reason if a refresh keeps the
Mac awake. Missing observation is not proof of idle.

## 5. Provider reducers

### Claude

Validate `sessions/<pid>.json`: PID, sessionId, UTC procStart and supported kind /
status. `busy|shell` supplies foreground demand; `waiting` or `idle` only
resolves foreground state. Unknown values stay unknown. Collect independently
owned commands and children before whole-session aggregation.

Correlate native Agent dispatch/result (`isAsync`, `status: async_launched`,
`agentId`, tool-use ID), metadata, and sidechain transcript. Native interactive
defaults can choose background execution without `run_in_background` in the
arguments. Do not look only for that argument. Follow output-file symlinks only
after validating the session/agent and expected provider root.

Child `end_turn` closes the current child generation; it does not close its
independently owned commands/children. Correlated native `queue-operation`
notifications with completed/failed/killed terminate the matching invocation,
subject to surviving work. Enqueue/remove duplicates are idempotent. Parse
native envelopes/IDs, never arbitrary `<task-notification>` prose in user text.
A resumed invocation reopens work using a new native run/start boundary; the
same agent ID may finish repeatedly. A stale `stoppedByUser` flag alone does not
cancel a newer run. Unsupported resumption attribution is unknown.

The measured AskUserQuestion call was persisted only after the dialog was
answered. Direct status is essential. Pending tool calls are not proof that
execution began; child-owned waits need positive attribution or remain unknown.

For a background self-paced loop, join **current worker identity**, session/job
IDs, `inFlight.wake.at` and `.pending-wakeup.scheduledFor` (milliseconds), with
sidecar toolUseId/chainStartedAt. Both signals clearing means no pending loop.
Disagreement/partial write/changed worker is unknown. Generic `session_cron`,
job `working`, transcript mtime and a past ScheduleWakeup call are insufficient.
No fixed timeout should end a valid self-paced chain merely for being overdue.
A stale sidecar with no live owner creates no confirmed loop demand.

Own caffeinate may support uncertain fallback when direct evidence is missing.
Its roughly 30-second linger must not override resolved idle/waiting, and its
absence proves nothing about independent work.

### Codex

Track exact turn IDs in `task_started`, `task_complete`, `turn_aborted`; make
duplicates harmless. A child's/other session's end cannot clear parent work.
Track late `item_completed` / `CommandExecution` against original thread/turn
and logical process ID; never substitute that logical ID for an OS PID.

Combine verified owned local processes independently. Completion/abort closes
only the matching foreground turn; a completed shell command also does not
prove detached descendants exited. Preserve witnessed descendant ownership
through host exit, but expose cold recovery ambiguity as above.

Live `ThreadStatus.activeFlags` belongs to a live app-server interface, not a
persisted rollout. This phase does not connect to the desktop's private server.
Use the explicit unknown/unfinished-turn behavior for unresolved native tools;
never claim transcript-only exact approval/question state. Do not parse opaque
JavaScript tool code to infer whether an internal command is waiting.

Add an executable-and-role carve-out before the generic `/Applications/`
exclusion for the measured worker:

```text
/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex
```

Retain older known layouts with fixtures. Exclude `exec-server`, helper/version
invocations, and the GUI process itself from session counts. A rollout header's
creation version is not necessarily the current host version.

### Copilot CLI

Replay in order using actor = root or **`data.parentToolCallId`**. `parentId` is
an interleaved event chain, not actor ownership. Numeric `turnId` repeats, even
inside one interaction; key an invocation by its start-event identity within
actor/interaction, not the number alone. Terminal events lacking interactionId
close only the compatible actor's outstanding invocation; ambiguity is unknown.

Track subagent start/completed/failed by originating task toolCallId. A task's
successful dispatch completion does not finish its child. Permission.requested
contains `permissionRequest.toolCallId`, which joins to the tool's actor even
when the permission event lacks parentToolCallId; completion uses requestId.
A root `ask_user` tool start is wait evidence until its completion/abort. This
CLI did not emit persisted `user_input.requested` for that question, so do not
require an SDK event that wasn't observed.

`read_agent(wait=true)` is a dependency on that child, not independent busy
work when the child itself is human-blocked. Likewise `read_bash` depends on
its command. Implement only these native wait relationships, not a general
workflow graph. Root abort clears that actor's interrupted calls/dependencies;
it does not clear child work. Interrupted reads may never have tool completions.

For async Bash, completion of dispatch differs from shell exit. Correlate native
shell ID with `system.notification.kind.type: shell_completed` and verified
local process evidence. Native completion/agent-idle notifications may start
another root turn after abort. No persisted session.idle appeared in these runs.
Subagent shell markers can identify internal sessions; unique live CLI ownership
plus child/tool evidence must map them to the root, with ambiguity exposed.

## 6. JSONL, recovery and performance

Use `BufRead` chunks and complete-record parsing, not a fixed 64 KiB tail or a
24-hour eligibility cutoff. Measured starts were 831,776 bytes before EOF and
single records exceeded 1 MiB. Incremental daemon state holds file identity,
offset, partial bytes and compact correlated lifecycle state, not prompt text.

On discovery rebuild from available complete history. On replace/truncate,
invalidate and rebuild. Hold a partial final line for the next append. Unknown
required schemas, malformed lifecycle records, missing history or a resource
budget limit mark reconstruction incomplete; they cannot clear outstanding work.
Unrelated model telemetry/compaction prose is ignored. Compaction is not an
implicit completion. Deduplicate by native event ID or stable file/offset key
where the format has none, and by semantic terminal identity across queue repeats.

Start with private implementation limits: 64 KiB read chunks, 8 MiB per complete
record, and 16 MiB of total transcript reads per evaluation, shared fairly across
sessions. These are design limits, not measured provider limits or new user
configuration. An oversized required record makes reconstruction unknown; drain
to the next newline without retaining it and keep that gap explicit. A budgeted
initial replay continues on later ticks without claiming completion. Add >1 MiB
and over-limit synthetic records and an incremental long-history check. Unchanged
files should read zero transcript bytes after stat, and a normal tick must not
reread all historical transcripts. Adjust these private limits only if the
implementation's measured resource behavior warrants it.
Don't promise a 2-second refresh when cold reconstruction exceeds its budget:
acknowledge an evaluation that honestly reports incomplete/unknown, or timeout.

Recovery uses the same collector/reducers before deciding whether to retain a
leftover power baseline. Do not restore old grace or transfer unclosed turns
from an earlier host incarnation. A newly loaded old rollout is not proof its
previous incomplete turn resumed. Cache ownership in memory only in this phase.

## 7. Power policy, grace and refresh

Automatic demand combines working sessions, explicit uncertain demand and the
VS Code gate. Wrappers/manual-on remain separate. Preserve existing guard,
recovery, power release ordering and closed-lid behavior.

Use a monotonic **30-second grace** when observed automatic demand transitions
from nonzero to zero. No startup grace from stale disk. Manual-off, safety cuts
and cooldown release immediately and consume old grace. New work can establish
a new cycle; clearing a guard alone cannot restore an old grace hold.

Add `vigil refresh` and menu “Refresh now”. Force one immediate collection /
evaluation; consume grace. Keep holding if verified or declared uncertain demand
remains. Report the evaluated result and its reason, not merely “signal sent”.

Minimal request protocol in the existing user state directory:

1. Client has a single 2-second total deadline, including serialization. Acquire
   a nonblocking/retrying advisory lock; use a unique token and expected daemon
   PID/start identity in an atomic request file. No global lock held by daemon.
2. Validate daemon executable/role, PID/start and supported tick protocol before
   SIGUSR1. Signal is a wakeup only. Recheck identity to narrow PID-reuse races;
   daemon also validates request target. Never start an absent daemon.
3. Daemon captures request token before evaluation and writes that exact token
   into the **post-action** tick with its own identity and evaluation sequence.
   A request arriving during evaluation stays pending for another pass.
4. Client succeeds only on matching token AND daemon identity; ordinary ticks
   and older forced ticks cannot acknowledge it. Release serialization lock.
   On timeout, report outcome unknown; don't claim success or restore grace.
5. Treat legacy tick, daemon replacement, stale/malformed request and concurrent
   callers explicitly. Keep at most the single serialized request slot; use
   atomic rename and user-only file permissions.

Normal transition detection is one configured tick plus collection time
(default tick 5 s). Grace release target is 30 s after **observed** loss of
certain/uncertain demand plus one tick/action delay. No UI-to-release upper bound
has been measured; don't invent one. Refresh's deadline is a response contract,
not permission to block indefinitely in lsof/parsing/helper IPC.

## 8. Integration map, schema and nearby fixes

| Area | Change | Required check |
| --- | --- | --- |
| `activity/mod.rs`, new provider readers and JSONL reader | Shared identities, work reduction, aggregation; retain `scan.rs` only as labeled fallback | Native fixtures plus malformed/duplicate/interleaved histories |
| `procscan/mod.rs`, `procscan/detect.rs` | One collection owner; process identity/parent/uid and scoped marker extraction; exact Codex worker role | Nested hosts, shared host, reuse, inaccessible metadata, helper exclusions |
| `refcount/mod.rs` | Keep pidfile lifecycle and wrappers; remove provider booleans as session-count authority; don't use low CPU to disprove verified work | Two sessions/one host; wrapper coexistence; silent work |
| `daemon/mod.rs` activity/recover/run_loop | Consume shared snapshot; incremental cache; grace and forced evaluation | Startup and tick parity; fake clock and fake power controller |
| `check/mod.rs`, `debug/mod.rs` | Replace duplicated classifiers / live_active_match_count; read-only collection | Same evidence yields same classification; no writes/GC/agent commands |
| `daemon/tick.rs`, `check/json.rs` | Versioned activity/policy reporting and refresh acknowledgement | Update every reader and golden together |
| `commands/status.rs`, `doctor.rs`, `menubar.rs`, `lock.rs`, `start.rs` | Honest counts/unknown/hold reason; cheap menu tick read; preserve lock/start scan semantics | CLI snapshots, old tick compatibility, menu action |
| `main.rs`, `commands/mod.rs`, new `commands/refresh.rs` | CLI dispatch and acknowledged refresh | Absent/replaced daemon, concurrent clients, timeout |
| README, architecture docs, ROADMAP, CHANGELOG | Coverage and actual shipped behavior | No universal exact-wait/remote/restart claim |

Use **status JSON version 2**. Preserve useful existing power/install fields;
replace misleading refcount active/total session ratios with explicit session
counts, detected hosts, wrappers, VS Code demand, uncertain demand/reason and
policy hold reasons. `status --verbose` can show per-session identity, evidence,
coverage and foreground wait. Avoid exposing prompts/commands/environment data.

Use **tick schema version 2**, retaining atomic write. Include daemon start
identity, evaluation sequence, working/waiting/idle/unknown counts, wrapper and
uncertain demand, grace remaining, refresh token, existing mode/guard/desired/
post-action engaged fields. Define deterministic key order with one writer and
one parser; remove CheckEngine's divergent tick interpretation. Legacy ten-field
tick can be read only as legacy policy with unknown session counts and refresh
unsupported. Menu reads tick only, no transcript scans. Cached policy age is
visible and cannot be presented as fresh activity.

Nearby fix A — wrapper identity: `commands/run.rs:76` and `commands/lock.rs:194`
pass current time to `wrapper_pidfile_body`; `gc_decision` compares it with the
actual process start, also for wrappers. Reproduction is saved. Store the same
actual start-time source used by GC; retain PID-reuse protection. If identity
cannot be read, fail registration clearly rather than manufacture current time
as process birth. Reuse scanner identity helper; no wrapper-only exemption.

Nearby fix B — read-only diagnostics: `CheckEngine::snapshot` currently calls
`helper.status()` (`check/mod.rs:548`). The client writes/renames a request and
the helper writes response/logs. Power is unchanged, but files are written.
Remove this round trip from status/doctor/debug. Report helper installation,
launchd/process identity, IPC directory permissions and existing validated
health evidence read-only; distinguish “present” from “responsive verified”.
Absent active probe means responsiveness unknown, not false success. Unknown
responsiveness is a warning (exit 0 when static installation checks pass),
including in `doctor --power`; concrete missing/invalid installation or observed
power faults remain errors (exit 1). Report unchecked responsiveness explicitly,
never label it verified. Keep an
explicit IPC health check in already-mutating operational paths where needed;
no new helper action or privileged command. Tests must include IPC paths in
immutability checks, not silently exclude them. Update doctor exit/warning
semantics and golden fields accordingly; run helper-test-seam if this boundary
contract changes.

## 9. Ordered implementation and checks

Work on `main`, no feature branch. Minimum Rust change per step; no production
code was added by the research task. Do not change native lock chord rules.

1. **Baseline and regressions.** Recheck working tree and HEAD; preserve handoff
   files. October 13 baseline fmt/build/clippy/test all passed. Run the baseline
   again for the new implementation session, not provider experiments. Add
   failing reproductions for provider-wide miscount, shared host, wait + child,
   and the delayed wrapper identity bug before fixing them.
2. **Collection/model.** Add identity and activity results; consolidate process
   collection ownership. Test same-user identities, nested providers, reparented
   witnessed commands, cold orphan unknown, missing env and host replacement.
3. **Readers.** Implement Claude, Codex and Copilot reducers against preserved
   fixtures. Verify checkpoint states, unknown gaps, multiple outstanding calls,
   actor-scoped abort, repeated numeric turn IDs, resume generations, native
   notification duplicates and tool dispatch vs command exit. Generate synthetic
   malformed/oversized/partial/truncated/replaced histories. Source-only Claude
   terminals must remain labeled unverified coverage, not measured assertions.
4. **Integrate all consumers.** Replace duplicated provider booleans in daemon,
   recover, CheckEngine and debug. Repair wrapper registration and remove
   diagnostic IPC writes. Test filesystem immutability including IPC and provider
   roots, read-only parity, VS Code preservation and power safety invariants.
5. **Policy and interfaces.** Add grace/refresh with fake monotonic time, signals
   and daemon identities. Test active→wait→grace→release, refresh during grace,
   repeated refresh, no resurrection, guards/manual-off, real work surviving
   refresh, same-second ticks, in-flight request race, concurrent callers, reused
   PID, replaced/absent daemon, unsupported protocol and timeout. Update schema,
   status/menu/doctor/lock/start and goldens together.
6. **Required verification.** Run exactly the repo's applicable suite:

   ```sh
   export PATH="$HOME/.cargo/bin:$PATH"
   cargo fmt --check
   cargo build
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test
   cargo test --features helper-test-seam # if helper boundary is touched
   ```

   No CPU-burning test unless it uses `BoundedCpuHog` or the documented equivalent
   with self-deadline, private process group and cleanup/reap guarantees. Check
   unchanged transcript files aren't reread and that budgets expose unknown.
7. **Install acceptance after implementation.** `vigil reload`, then `vigil doctor`:
   LaunchAgent runs `~/Library/Application Support/vigil/bin`, not target/debug.
   Validate shipped classification against fixture-driven seams and one concise
   disposable smoke covering changed integration; don't repeat the research
   matrix without a regression/version reason. Use fake power seams for the
   automated suite. Any physical lid/sleep acceptance must be coordinated with
   the user so unrelated work isn't interrupted. Verify cleanup and docs.

Completion requires consistent consumers, green applicable checks, honest
coverage diagnostics, and the installed snapshot verified when deployment is
requested. A green build alone does not prove the running LaunchAgent changed.

## 10. Next-session starting instruction

Implement this handoff incrementally, beginning with the failing count/ownership
fixtures and shared activity model. Reuse `docs/fixtures/phase-8`; do not rerun
provider probes by default. Respect the documented unknown-state policy and
confidence distinctions. Do not claim full Claude child lifecycle, Desktop,
off-machine execution, cold detached recovery or disk-only Codex human-wait
coverage. If full exact coverage becomes a hard requirement, design the missing
provider integration explicitly before changing that claim. No such integration
is established by the current evidence.
