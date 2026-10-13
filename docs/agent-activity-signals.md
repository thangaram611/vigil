# Agent activity signals — phase 8 research

Research date: 2026-09-21. Repository: `main` at `5df1f8a`.

**Research handoff completed October 13; production implementation has not
started.** The [Phase 8 handoff](../future/phase-8-per-session-activity.md) is the
current design authority. [Preserved fixtures](fixtures/phase-8/README.md) make
the measured cases replayable without temporary directories or new model runs.
Dated entries below preserve earlier hypotheses and open gates; the final
October 13 section supersedes their research/approval status. Some investigated
surfaces remain source-only, unavailable or intrinsically unobservable in local
files. Those limits are explicit implementation behavior, not passed tests.

The September 21 pass inspected
source, existing process metadata and existing session files. No agent was
started, interrupted or prompted; no sampler, power experiment, build or test
was run in that pass. Its measurements below are snapshots, not transition-lag
measurements. User-approved measurements began October 9; see the dated results
at the end of this document.

## Confirmed local findings

| Finding | Evidence | Implementation consequence |
| --- | --- | --- |
| Claude counts processes using one provider-wide activity flag | `daemon/mod.rs::activity` calls `scan::is_active`; `refcount/mod.rs::count` applies the same flag to every Claude entry | Classify each live Claude session independently |
| The fallback defaults to five minutes | `activity/scan.rs::window_secs` rounds the configured interval up to whole minutes | Retain this only for explicitly uncertain fallback results |
| ChatGPT's Codex worker is excluded | Two live executables ended in `/ChatGPT.app/Contents/Resources/codex`; `procscan/detect.rs` only carves out the old Codex.app host before excluding `/Applications/` | Add a precise worker-path match; exclude unrelated helpers |
| A fixed 64 KiB tail loses a live turn | During this task, the only `task_started` was 831,776 bytes before EOF; the final 64 KiB contained no turn boundary | Read back to a relevant boundary or reconstruct state, then consume appended records; absence in one chunk is not idle |
| JSONL records can exceed 64 KiB | Recent Codex files contained single records of 143,993, 791,407 and 1,035,169 bytes | Chunk boundaries and partial last lines need separate handling |
| Codex interruptions have their own terminal event | October 12 controlled Stop on CLI `0.162.0-alpha.17.2` persisted `event_msg/turn_aborted`, with `reason: interrupted` and the matching `turn_id`; the app reported `interrupted` | Close the matching turn on abort; separately account for work surviving interruption |
| Codex terminals survive an ended turn | October 12 yielded terminal commands stayed alive for 86 seconds after normal completion and 113 seconds after Stop; each carried its owning `CODEX_THREAD_ID` | Combine turn activity with verified live terminal ownership; an ended turn alone is insufficient |
| Codex terminal exit is recorded after the turn ends | Both commands later appended `item_completed/CommandExecution` with the original thread/turn IDs, logical `process_id`, and exit code 0 without polling | Correlate command lifetime separately from turn lifetime; a logical process ID is not an OS PID |
| Claude waiting can hide live background work | October 12: native question and Read permission dialogs reported `waiting` during background shells and an in-process subagent; caffeinate disappeared before the work ended | Combine independent shell, subagent and loop demand before treating a foreground wait as inactive |
| Copilot tool completion is not shell completion | October 12, CLI 1.0.95: asynchronous `bash` returned successfully before its process completed; final answer and user abort both left the shell running | Track live shell ownership and native shell-completion notifications separately from tool/turn completion |
| Copilot logs mix parent and subagent turns | The latest existing Copilot log contained five start/end pairs, two attributed to a subagent; a child end preceded parent tool completion and parent turn end | Track correlated outstanding work; a child's end must not clear its parent's activity |
| Copilot interruptions also have a terminal event | An existing September 10 log ended with an unmatched `assistant.turn_start` followed by two `abort` events | Handle aborts idempotently |
| Claude's process-start text uses UTC on this machine | `procStart` was `Sun Sep 20 05:41:47 2026`, matching `TZ=UTC ps`; local `ps` showed `11:11:47` | Compare normalized timestamps, not local formatted strings; a mismatch must fall through |
| The live count omits this working Codex task | A later daemon snapshot reported `refcount_active=0`, `engaged=0`, mode `auto`; its active directory contained one Claude pidfile and no Codex pidfile | Detection and activity changes must be verified together |

The one Claude session observed in this pass reported version `2.1.278`,
`kind: interactive`, and `status: idle`. There were no Copilot session files
modified in the preceding 24 hours. Older Copilot evidence validates event
shapes, not current-version transition timing.

## Upstream contracts and their limits

- [Claude agent view](https://code.claude.com/docs/en/agent-view) documents
  `claude agents --json` as the supported reader. Background `state: working`
  includes autonomous waits between steps; it is distinct from process
  `status: busy`. The proposed `busy|shell` rule therefore needs measurements
  for `/loop`, background agents and supervisor-hosted sessions.
- The same documentation says `claude agents` can start a fresh supervisor
  after it stops. October 12 source analysis additionally found conditional
  stale-registry deletion in the JSON reader path. That is sufficient to rule
  it out of read-only diagnostics; whether the JSON-only path starts a supervisor
  was not established. See the version-specific analysis below.
- [Codex's protocol source](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs)
  defines start, completion and abort events. It explicitly says interrupting
  a task does not terminate background terminal processes. Turn completion and
  cancellation cannot alone prove that all owned background work stopped.
  This upstream branch is corroborating evidence, not the installed binary.
- [GitHub's agent-loop documentation](https://docs.github.com/en/copilot/how-tos/copilot-sdk/features/agent-loop)
  defines a start/end pair per model call and its tools. One user request can
  contain several such pairs. `session.idle` ends the whole loop but is
  ephemeral: it is not saved to `events.jsonl`.
- [GitHub's event reference](https://docs.github.com/en/copilot/how-tos/copilot-sdk/features/streaming-events)
  documents subagent attribution, persisted `abort`, and ephemeral permission
  and user-input events. The October 12 CLI 1.0.95 measurement nevertheless
  persisted `permission.requested` and `permission.completed` in `events.jsonl`.
  Treat persistence as version/surface-specific: the older SDK documentation
  does not establish what this CLI writes. User-question persistence was not
  measured, and no `session.idle` was observed in the completed probe.

## Necessary changes to the proposed design

These are proposals to resolve the findings, not implemented behavior.

1. **One shared activity model.** The daemon, `CheckEngine`, crash recovery and
   `debug` currently duplicate provider-wide classification. Move classification
   into `activity`; pass in process identity, session evidence and time. Keep
   collection and the daemon's state writes separate from read-only consumers.
   Expose whether a result came from direct evidence or fallback.
2. **Count sessions separately from process anchors.** A boolean predicate over
   pidfiles cannot represent several active Codex sessions hosted by one
   app-server. Keep pidfiles for process liveness and explicit `vigil run`
   wrappers; aggregate unique working sessions once per provider. Preserve the
   VS Code hash gate. Do not multiply session counts by the number of hosts.
3. **Parse complete event histories where required.** Use bounded read buffers,
   not a fixed total tail limit that discards relevant history. Codex can search
   backward for a boundary; interleaved Copilot work needs correlated state.
   Cache offsets/state for unchanged files in the daemon, handle replacement
   and truncation, and retain incomplete trailing bytes for the next pass.
4. **Separate activity from grace.** Report actual working/waiting counts while
   showing a grace hold as grace. Apply the 30-second grace only to automatic
   activity demand; thermal, battery and manual-off still release immediately.
   Refresh must clear the existing grace so an ordinary tick cannot re-engage
   solely from the same old activity. New work starts a new grace cycle.
5. **Acknowledge refresh explicitly.** `updated_at` is whole seconds, and an
   ordinary tick can advance it before the refresh is handled. A refresh
   generation written after a forced evaluation gives the CLI a meaningful
   acknowledgement. Matching two saved PIDs is insufficient after PID reuse:
   validate the live daemon identity before signaling. Keep the two-second
   caller deadline, and report a timeout honestly if evaluation is still busy.
6. **Be precise about uncertainty.** A live provider process does not prove
   ownership of every unfinished historical session. The 30-minute no-write
   guard bounds stale holds but can also expire legitimate silent work. A
   missing caffeinate child is not necessarily authoritative idle evidence.
   Measure these cases before treating either heuristic as an exact signal.
7. **Keep supported background loops awake between iterations.** On October 12,
   the user accepted background mode as a requirement for automatic self-paced
   loop tracking in Phase 8. Live measurement found that background job wakeup
   fields persist during idle gaps and clear after Esc. Validate their ownership
   before treating them as activity; raw session idle cannot override a pending
   supported loop. Ordinary terminal turns remain in scope, but ordinary terminal
   loop gaps require `/bg` or a manual hold. Document this limit explicitly.
   Replaying transcript scheduling calls cannot track interactive Esc cancellation.
   Fixed-interval cron tasks remain a separate, unmeasured case.
8. **Keep Codex terminal activity separate from turn activity.** The October 12
   experiment verified that yielded local `exec_command` processes survive both
   final answers and Stop. Both processes exposed the exact owning
   `CODEX_THREAD_ID` and remained children of the verified app-server until exit.
   Combine outstanding turns with live owned terminal work, counting a session
   once. Preserve PID/start and host checks; do not use an environment tag alone
   to classify arbitrary processes. Command completion can arrive after the
   turn's terminal event and must not reopen that turn. Detached descendants,
   host restart and remote execution remain outside this measurement.
9. **A foreground wait must not clear independent background work.** Claude
   2.1.296 reports `waiting` for both a question and a permission dialog even
   while a background shell runs; its own caffeinate child can disappear too.
   The tested processes carry native `CLAUDE_CODE_SESSION_ID`, and Copilot
   shells carry `COPILOT_AGENT_SESSION_ID`. Validate each marker against the
   appropriate live host and PID/start identity. Nested tool hosts can inherit
   another provider's environment, so a tag alone must not assign ownership.
   Both combined shell and subagent paths are now measured. The subagent is
   in-process: native asynchronous dispatch, child transcript and a matching
   queued completion notification supply lifecycle evidence that shell PID
   tracking cannot. Cancellation/failure/resumption still need validation.
10. **Replay Copilot events with their actual lifecycle boundaries.** In CLI
    1.0.95, asynchronous `bash` completion means dispatch finished, whereas
    `system.notification.kind.type: shell_completed` records shell exit.
    An interrupted `read_bash` had no matching tool-completion record through
    shutdown. Abort must close interrupted foreground accounting without
    erasing the still-live shell. Numeric turn IDs restart, including within
    one interaction after a background completion; they are not globally unique.

## Step 0 measurement matrix

Previously reported observations below come from the phase plan; they were not
reproduced in this pass. Every pending row must be measured, or its surface
explicitly deferred with the user, before the implementation claims coverage.
The user chose further research on October 12; the unmeasured surfaces below
have not been deferred.

| Case | Evidence available now | Still needed |
| --- | --- | --- |
| Claude foreground turn | October 9: `busy` throughout a foreground command; comparison session stayed idle | UI-to-status lag was not captured |
| Claude idle | October 9: independent idle sessions and return to idle after work | UI-to-status lag was not captured |
| Claude background shell after final answer | October 9: `shell` throughout the command, including 25 seconds without its own caffeinate child | Measured for the bounded shell case |
| Claude background subagent after final answer | October 9: parent stayed `busy` after its final reply. October 12: one subagent continued through parent question and permission waits, finishing while the permission dialog remained open | Normal dispatch/completion measured; cancellation, failure and resumption remain open |
| Claude `/loop` between iterations | October 9: self-paced loop reports `idle`; native wakeup and explicit stop succeeded. October 12: an attached background job clears both persisted wakeup signals after Esc; user accepted background-only automatic tracking | Ordinary interactive loop gaps explicitly outside Phase 8 automatic coverage; fixed-interval cron remains unmeasured |
| Claude permission prompt | October 12: `waiting` overlapped both a background shell and an in-process subagent; the latter had 108 overlapping samples without caffeinate and completed with the dialog still open | Normal combined cases measured; a subagent itself requesting permission remains open |
| Claude AskUserQuestion | October 12: `waiting` overlapped both a shell and an in-process subagent; the latter had 162 overlapping samples without caffeinate | Combined normal case measured; question was answered before the same child proceeded through the permission case |
| Claude plan-mode exit | October 9: native ExitPlanMode reports `waiting` / `permission prompt`; cancellation returns to idle | Measured for cancellation of the native plan dialog |
| Claude `kind: background` | October 12: verified new session/job identity after `/bg`; attached background loop exposes pending wakeup and clears it after Esc; no scheduled turn follows | Measured for an attached background loop; restart and detached cancellation behavior are not covered |
| Claude.app Local Agent Mode | October 12: no Claude Desktop bundle in `/Applications` or `~/Applications`; only a Claude Code URL handler was found | Locate an installed surface or agree to defer it |
| Codex turn in ChatGPT.app | October 12: controlled normal completion persisted matching start/complete IDs; the probe became idle while the research task stayed active under the same app-server | Measured for turn completion and per-session independence; UI-to-file lag was not captured |
| Codex interruption | October 12: user Stop persisted `turn_aborted`; a yielded terminal survived for another 113 seconds, retained the owning thread ID, then recorded its exit against the interrupted turn | Measured for the local yielded-terminal path; detached descendants, host restart and remote execution are not covered |
| Codex background work after final answer | October 12: a yielded terminal survived for 86 seconds after the final answer; native thread ownership and eventual command completion were verified | Measured for the local yielded-terminal path; production owner filtering and PID-reuse handling still need implementation checks |
| Copilot foreground/tool/subagent turn | October 12, CLI 1.0.95: live model/tool sequence, final answer, asynchronous shell survival and native shell exit recorded; older logs contain subagent interleaving | Main-session/tool/shell path measured; current-version subagent interleaving remains unmeasured |
| Copilot interrupted turn | October 12: double Escape during `read_bash` persisted `abort: user_abort`; shell remained alive for another 39 seconds, retained ownership, then exited normally | Measured for an asynchronous local shell interrupted during its output wait |

## First experiment — approved and started 2026-10-09

Run a detached, read-only metadata sampler, once per second, with a hard
10-minute lifetime. It must run outside Claude's tracked tools so the sampler
does not itself turn Claude's status into `shell`. Record only timestamps,
process identity and parentage, selected session-state fields, and caffeinate
presence/flags. Do not record prompt text, tool arguments or environment values.
Write observations to a temporary directory; retain only scrubbed evidence in
the repository. No daemon reload, mode change or power command is part of this
experiment.

The user drives one normal Claude terminal session through idle, a short turn,
a bounded background shell and a bounded background subagent, allowing each to
finish. Observe another idle session concurrently to establish independence.
Both background jobs must have their own hard deadline and cleanup. Stop after
this round, review the observations together, then arrange the waiting/loop
cases and Codex/Copilot rounds. Live clamshell testing is a separate later step.

## Implementation checks after measurement

- Capture a green Cargo baseline before refactoring. Add the failing two-Claude
  reproduction before fixing the count.
- Table-test Claude statuses, PID/start identity, UTC parsing and every fallback.
- Replay scrubbed event fixtures: long turns beyond 64 KiB, long JSONL records,
  partial writes, aborts, interleaved subagents, replaced files, stale starts,
  multiple sessions under one host, and no live host.
- Drive grace, refresh acknowledgement, same-second refresh, foreign/reused PID,
  timeout, manual modes and safety cuts through seams without live power effects.
- Verify status, debug and doctor preserve read-only behavior and agree with the
  daemon's classifier. Verify the menu reports waiting and grace truthfully.
- Run the repository's required Cargo checks. After authorized installation,
  `vigil reload` and `vigil doctor`, then complete the user-assisted live checks.

## 2026-10-09 measurements

Versions refreshed before sampling: Claude Code `2.1.295`, Copilot CLI `1.0.94`,
Codex CLI `0.162.0-alpha.17.2`. Codex now runs from
`/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex`;
the September path alone will not cover the current install.

The detached sampler records process/session metadata once per second and
automatically stops after 600 seconds. It captures no prompts or tool arguments
and does not change Vigil or power settings. Recording began at 15:31:06 IST.

### Foreground shell, with a second idle session

The user ran `sleep 15` in the foreground with a 30-second tool timeout, then
let Claude return its final answer and remain idle. Through 332 samples there
were no sampler errors. The observed sequence was:

| Time (IST) | Exercised session | Own caffeinate child | Comparison session |
| --- | --- | --- | --- |
| 15:31:06 | idle | absent | idle |
| 15:35:27 | busy | absent | idle |
| 15:35:28 | busy | `-i` present | idle |
| 15:35:52 | idle | `-i` still present | idle |
| 15:36:22 | idle | absent | idle |

The `sleep` process was visible from 15:35:35 through 15:35:49. This verifies
independent per-session status and coverage of the foreground command. It does
not measure the delay from UI submission or final-answer rendering to the
status update, because those UI timestamps were not captured.

**Fallback consequence:** Claude's own caffeinate child lingered for about 30
seconds after its status became idle. Prefer valid direct status over this
child; presence alone cannot distinguish working from Claude's own release
delay. The background shell case follows below; the subagent case remains pending.

### Background shell after the foreground answer

The user ran `sleep 60` with `run_in_background=true` and a 90-second tool
timeout, and reported that Claude had answered `BACKGROUND STARTED`.

| Time (IST) | Exercised session | Background sleep | Own caffeinate child | Comparison session |
| --- | --- | --- | --- | --- |
| 15:38:05 | busy | not yet visible | `-i` present | idle |
| 15:38:15 | busy | running | `-i` present | idle |
| 15:38:20 | shell | running | `-i` present | idle |
| 15:38:50 | shell | running | absent | idle |
| 15:39:15 | busy | exited | new `-i` child | idle |
| 15:39:22 | idle | exited | new `-i` child still present | idle |
| 15:39:52 | idle | exited | absent | idle |

The command's disappearance coincided with a return to `busy`, consistent with
Claude processing its completion notification; the sampler did not capture
message contents. Another caffeinate process belonged to a different parent
and was not counted as Claude's own child.

**Result:** `shell` correctly retained the working classification throughout
the background command. Claude's own caffeinate child disappeared about 30
seconds after entering `shell`, while the command continued for another 25
seconds. The September observation of a caffeinate child accompanying `shell`
does not establish that the child persists for the whole command. Positive
child evidence may support fallback, but an absent child must not override a
valid `shell` status or be treated as conclusive idle evidence. Background
subagent behavior remains unmeasured.

### Background subagent — first attempt inconclusive

For this case the user opened a new `vigil-activity-test` Claude conversation in
an empty temporary directory, keeping both existing conversations idle. A new
600-second metadata recorder began at 15:45:42 IST. The test asked one background
subagent to run a foreground `sleep 60` with a 90-second tool timeout.

The throwaway session became busy at 15:47:14 and idle at 15:47:43. No sleep
process appeared. The user's screenshot showed that the subagent was dispatched
but its command was rejected with `Blocked: standalone sleep 60`; it finished
without executing the sleep. This is not a successful sample of sustained
background work and does not establish whether the parent stays classified as
working while its subagent is running. Both comparison sessions remained idle.

The next attempt used genuine read-only source analysis, with no synthetic
delay and no change to the harness's command restrictions.

### Background subagent — read-only analysis succeeded

The same throwaway session dispatched one background subagent to trace Vigil's
activity classification using only file-reading and search tools. The subagent
made 19 `Read` calls. A targeted inspection of the throwaway transcript's event
timestamps, tool names and stop reasons correlated its lifecycle with the
recorder; no prompt bodies or tool arguments were copied into the evidence.

| Time (IST) | Event | Parent status | Own caffeinate child |
| --- | --- | --- | --- |
| 15:50:37 | Parent begins the turn | busy | `-i` present |
| 15:50:47 | Background subagent dispatched | busy | `-i` present |
| 15:50:50.886 | Parent replies `SUBAGENT STARTED`, with `end_turn` | busy | `-i` present |
| 15:51:24.291 | Subagent makes its last observed `Read` call | busy | `-i` present |
| 15:53:05.433 | Subagent records its final answer, with `end_turn` | busy | `-i` present |
| 15:53:37.119 | Parent records its final completion response | busy until the following observation | `-i` present |
| 15:53:37.413 | Recorder observes the parent returned to idle | idle | `-i` still present |
| 15:54:07.410 | Recorder observes its caffeinate child gone | idle | absent |

**Result:** direct `busy` status covered the entire observed background-subagent
lifetime, including over two minutes after the parent's initial final reply and
the subagent's quiet interval after its last file read. Both original Claude
sessions remained idle throughout. The parent returned to idle after handling
completion, followed by the same roughly 30-second caffeinate tail.

This validates `busy` for the measured interactive-session background Agent
case on Claude Code 2.1.295. It does not establish `/loop`, permission/question
waits, plan-mode exit, supervisor-hosted jobs or other agent versions. No
classifier code or installed Vigil state was changed. The second recorder was
stopped after the completed case rather than extended beyond its 600-second cap.

### AskUserQuestion — waiting and resumption succeeded

The user approved a waiting-state measurement round in the same throwaway
session. A new 600-second recorder began at 16:04:50 IST. The prompt explicitly
described the experiment and requested a real `AskUserQuestion` dialog, followed
by `WAIT TEST COMPLETE` only after the user selected an answer.

| Time (IST) | Observation | Own caffeinate child |
| --- | --- | --- |
| 16:10:04.928 | Parent is busy | `-i` present |
| 16:10:07.592 | Transcript records the native `AskUserQuestion` call | Not a process sample |
| 16:10:07.925 | Recorder observes `waiting`, `waitingFor: input needed` | `-i` present |
| 16:10:37.905 | Still `waiting`, `waitingFor: input needed` | absent |
| 16:10:45 | User has confirmed the actual dialog is open and unanswered | absent in the preceding sample |

Both original sessions remained idle. This confirms the direct waiting signal
for a real unanswered question on Claude Code 2.1.295. The timestamps correlate
the tool call with observed state, but do not measure the dialog's rendering
time. Resumption after the user's answer was then checked as follows.

After the user selected Continue, the tool result was recorded at 16:12:07.566
IST. The recorder saw `busy` at 16:12:07.964, then `idle` at 16:12:08.953. The
transcript recorded the expected `WAIT TEST COMPLETE` response at 16:12:08.729.
Both comparison sessions stayed idle. This completes the waiting/resumption
case; a native unanswered question is distinguishable from active work.

### Permission prompt — setup correction

Before this case, inspection of the throwaway process's arguments found
`--dangerously-skip-permissions`. User settings separately defaulted to `auto`.
A prompt alone cannot reliably request native approval under that launch mode.
The installed CLI lists `manual` as an accepted permission mode.

The next session uses the explicit executable path, `--permission-mode manual`,
and a temporary settings file containing `permissions.ask: ["Read"]`. A harmless
text file in the scratch directory provides the read target. This only adds an
approval requirement to the test session; global settings and the user's other
sessions are unchanged. Claude's [permission documentation](https://code.claude.com/docs/en/permissions)
confirms that ask rules take precedence over allow rules, including for Read.

### Permission prompt — waiting and resumption succeeded

The user opened `vigil-permission-test` using the prepared command. Live process
arguments confirmed `--permission-mode manual`, the temporary settings path,
and absence of the bypass flag. A new 600-second recorder began at 16:20:32 IST.
The user requested exactly one native Read of the harmless probe file and
confirmed that its actual approval dialog was open and unanswered.

| Time (IST) | Test-session status | Own caffeinate child |
| --- | --- | --- |
| 16:20:32.262 | idle | absent |
| 16:21:26.282 | busy | `-i` present |
| 16:21:29.266 | `waiting`, `waitingFor: permission prompt` | `-i` present |
| 16:21:59.252 | `waiting`, `waitingFor: permission prompt` | absent |
| 16:22:03.276 | `waiting`, `waitingFor: permission prompt` | absent |

The two original sessions and the earlier throwaway all remained idle during
this recording. No sampler errors were observed through 92 samples. Direct
state distinguishes a native permission wait from the prior question's
`input needed` reason; both are `waiting`.

After the user approved the single read, its tool result was recorded at
16:23:18.274 IST. The recorder observed `busy` at 16:23:19.280 and `idle` at
16:23:20.279. The expected `PERMISSION TEST COMPLETE` response was recorded at
16:23:19.607. Claude's own caffeinate child was absent by 16:23:50.249. All three
comparison sessions stayed idle. This completes the native permission-dialog
case without changing global permission settings.

### ExitPlanMode — approval wait and cancellation succeeded

The user entered plan mode in the same manual-approval throwaway session and
requested a minimal test proposal, explicitly to be cancelled without
implementation. Claude wrote its normal plan artifact and called ExitPlanMode.

| Time (IST) | Event | Session status | Own caffeinate child |
| --- | --- | --- | --- |
| 16:26:26.253 | Planning turn is running | busy | `-i` present |
| 16:26:34.514 | Native plan artifact write succeeds | busy | `-i` present |
| 16:26:36.030 | Transcript records ExitPlanMode | Tool event, not a process sample | — |
| 16:26:36.279 | Plan approval is pending | `waiting`, `permission prompt` | `-i` present |
| 16:27:06.269 | Approval remains pending | `waiting`, `permission prompt` | absent |
| 16:28:20.302 | User cancels; ExitPlanMode returns a rejection | Tool event, not a process sample | — |
| 16:28:21.306 | Recorder observes cancellation complete | idle | absent |

Both original sessions and the earlier throwaway stayed idle. Neither proposed
probe-file location existed after cancellation, and no implementation write
appeared in the transcript. The native plan artifact alone was created. This
version labels the plan dialog `permission prompt`, not a separate plan-specific
waiting reason. The recorder was stopped after cancellation.

### Self-paced /loop — scheduled gap, resumption and explicit stop succeeded

The user returned the same throwaway session to manual mode. A fresh recorder
began at 16:36:22 IST. The explicit experiment prompt requested two iterations:
output a marker and arm native ScheduleWakeup, then stop the loop and output a
completion marker on the second iteration. It prohibited other work. The
transcript confirms the session had exited plan mode.

| Time (IST) | Event | Session status | Own caffeinate child |
| --- | --- | --- | --- |
| 16:37:40.811 | First iteration is running | busy | `-i` present |
| 16:37:46.094 | ScheduleWakeup succeeds; requested delay 60 seconds, tool reports next wakeup in 74 seconds at 16:39:00 | Tool result, not a process sample | — |
| 16:37:46.789 | First iteration ends with wakeup pending | idle | `-i` present |
| 16:38:16.775 | Scheduled gap continues | idle | absent |
| 16:39:00.178 | Transcript records `system/scheduled_task_fire`, with `taskKind: loop` and `cronKind: loop` | Event, not a process sample | — |
| 16:39:00.810 | Second iteration is running | busy | `-i` present |
| 16:39:06.036 | ScheduleWakeup with `stop: true` succeeds; result confirms the dynamic loop ended | Tool result, not a process sample | — |
| 16:39:07.718 | Assistant replies `LOOP 2 DONE`, with `end_turn` | Event, not a process sample | — |
| 16:39:07.816 | Completion observed | idle | `-i` present |
| 16:39:37.789 | Post-completion caffeinate tail ends | idle | absent |

All three comparison sessions stayed idle. The recorder stopped at 16:40:19
after 237 samples, with no sampling errors; its process was confirmed gone.
The transcript contains no CronCreate, Monitor, shell command or agent launch
for this case. The native stop result confirms this dynamic loop ended.

**Finding:** `busy|shell` and caffeinate alone cannot distinguish an idle
interactive session from an active self-paced loop between iterations. The
pending gap lasted about 74 seconds, with no own caffeinate child for about
44 seconds. A 30-second grace would not bridge it. Inspection during the gap
found no loop or wakeup field in this session's status JSON. The transcript
does retain the successful scheduling call, the scheduled-fire event and the
successful explicit stop, but that does not yet establish reliable detection
of Esc cancellation, resumption, or every way a loop can end.

**User decision, October 9:** keep the Mac awake until an active self-paced
loop stops, including its scheduled gaps. This changes the proposed direct
`idle` rule. It does not decide whether every future fixed-interval cron job
should retain a hold. The next measurement should establish how Esc cancellation
is exposed before choosing a loop-state reader.

### October 12 recovery — Esc cancellation remains unmeasured

The follow-up recorder started on October 9 at 16:44:48 IST and stopped at its
600-second deadline, 16:54:48, after 600 samples. The target stayed `idle`
throughout; the recorded throwaway transcript contains no new scheduling call
after this recorder started. This run therefore provides no Esc-cancellation
evidence.

On October 12 the user reported a laptop restart and Claude Code update. A
read-only check confirmed installed Claude Code `2.1.296`; the prior test-session
and recorder processes were absent. Earlier successful measurements remain
evidence for `2.1.295`, not verification of the new version. Resume the approved
cancellation experiment in a fresh throwaway session with a fresh process
identity and bounded recorder; no recorder was restarted during this check.

### October 12 Esc cancellation — succeeds without a persisted cancellation event

The user opened a fresh manual-mode throwaway session on Claude Code `2.1.296`.
Its PID and UTC process start matched its session file. A new recorder began
at 11:02:05 IST. The explicit test prompt armed one native ScheduleWakeup with
a requested 180-second delay and a stop-on-wakeup fallback. The user confirmed
the waiting indicator, then pressed Esc when asked and reported it cancelled.

| Time (IST) | Observation |
| --- | --- |
| 11:02:29.272 | Test session is busy, with its own caffeinate child |
| 11:02:45.946 | ScheduleWakeup succeeds; tool reports a wakeup at 11:06:00, in 194 seconds |
| 11:02:46.269 | Session returns to idle while the wakeup is pending |
| 11:03:05.807 | Pending-state snapshot captures the transcript and session-file metadata |
| 11:03:16.258 | Own caffeinate child is absent |
| 11:03:58.203 | Snapshot after the user reports Esc cancellation: both files remain unchanged |
| 11:06:00–11:06:54.282 | All 55 samples at or after the scheduled time report idle; no scheduled-fire event or fallback response is appended |

The transcript stayed at 340,162 bytes with exactly the same modification
timestamp before and after cancellation, including after the scheduled time.
The session JSON retained its size, modification timestamp, `updatedAt`,
`statusUpdatedAt`, and `idle` status. It contains no loop-state field. The
project's `scheduled_tasks.json` was absent in both snapshots. Its remaining
`scheduled_tasks.lock` contains owner identity and acquisition time; its presence
does not establish a pending wakeup. No debug log existed for this test session.

The recorder encountered three two-second process-snapshot timeouts at
11:04:53, 11:04:55 and 11:04:57. It recovered before the scheduled wakeup;
there were no errors in the 55 post-deadline samples. The unchanged transcript
separately confirms that no scheduled turn was recorded. The recorder was
stopped after the final snapshot, and its process was confirmed gone.

**Result:** the observed Esc cancellation succeeded, but neither the direct
session file nor transcript exposes the transition. A classifier that remembers
a successful ScheduleWakeup until it sees a native `stop: true` call would
keep this cancelled session active incorrectly. These sources alone cannot
implement the agreed loop policy accurately. A timeout would be a heuristic,
not proof of cancellation.

The current [agent-view contract](https://code.claude.com/docs/en/agent-view#read-session-state-from-a-script)
describes `state: working` across autonomous waits, but documents `state` for
background sessions. This test remained interactive. That contract therefore
does not establish an interactive loop-state reader. Investigate an additional
read-only signal before choosing an implementation or requesting another live
experiment; no supervisor or CLI session-state reader was started in this test.

### October 12 static analysis — persisted loop state is gated to background jobs

Read-only inspection of the installed Claude Code 2.1.296 executable's embedded
JavaScript explains the observed cancellation gap. This is implementation
evidence for this binary, not a supported external API or a live background-job
measurement. No extracted code was executed, and no CLI state reader or private
IPC request was issued.

Binary: `/opt/homebrew/Caskroom/claude-code@latest/2.1.296/claude`,
240,664,432 bytes; SHA-256
`c9b5341637becbd423ddffc5b254afb645682a3868cb708bbc6cc0e7bb419937`.
Offsets below are decimal byte offsets into that binary. Short function names
are module-scoped and are not stable across releases.

| Source path | Evidence | Implication |
| --- | --- | --- |
| In-memory scheduler; `w4` near 186859379, `N8n` at 203376560 | ScheduleWakeup inserts a loop into session cron state. User abort removes loop tasks and emits a pending-wakeup change; the idle Esc handler invokes this cancellation path | The cancellation need not append a transcript event, matching the live observation |
| Sidecar writer module at 202132813 | `qHr` subscribes to pending-wakeup changes. `FNt` writes `.pending-wakeup` with `toolUseId`, `scheduledFor`, and `chainStartedAt`, or removes it when the event clears | Candidate pending/cancelled signal, subject to the writer's eligibility gates |
| Background gate near 189830179 | `bd()` requires background session kind or a background takeover; `dO()` obtains that job directory. The writer has another mode-dependent suppression check | An ordinary interactive session does not receive this sidecar; absence alone is not universal idle evidence |
| In-flight snapshot near 195146000; job watcher near 218841046 | Pending cron adds `session_cron` to `inFlight.kinds`; a loop adds `wake.at`, `reason`, and `fires`. The job watcher subscribes to changes and updates persisted job state conditionally | Inspect only selected metadata during a background test; write timing and cancellation behavior still need measurement |
| JSON listing at 225718849 | Background entries include `state`; ordinary interactive entries expose status but no loop-state field. Raw `shell` is normalized to `busy` | Running the CLI does not fill the ordinary interactive loop gap |
| Registry reader at 194434802; cleanup at 194434603 | The JSON listing calls `CJ`, which can invoke `V` for stale records when registry sweeping is permitted. `V` deletes the record and performs associated cleanup | The supported listing is not a strictly read-only replacement for direct file reads |

The [documented JSON interface](https://code.claude.com/docs/en/agent-view#read-session-state-from-a-script)
also scopes `state` to background jobs and includes autonomous waits in working
state. Upstream recommends this interface over parsing job files. Direct files
remain research candidates with a compatibility risk, not a chosen production
contract. No suitable documented read-only signal for ordinary interactive
Esc cancellation has been established.

### Background loop cancellation — approved and measured October 12

Use only the existing throwaway `vigil-loop-cancel-test`, after revalidating its
session ID and process start. The user runs `/bg` there, then attaches to that
same session. This can start Claude's normal shared supervisor and create its
job metadata; the user approved this experiment on October 12. Do not send a new task
to an unrelated session or stop the shared supervisor during cleanup.

Start a fresh detached recorder only after approval. Give it a hard ten-minute
deadline and an explicit stop file. Record the target process identity,
session/job association, selected status and in-flight fields, sidecar presence
and scheduling timestamps, and own caffeinate children. Exclude prompts,
credentials and IPC contents. Read the target files directly; do not invoke
`claude agents --json` from the recorder.

Once attachment and the recorder are confirmed, use this explicit prompt:

```text
This is a bounded Vigil activity-detection experiment in a throwaway session.
Do not do project work, edit files, run shell commands, or launch agents.
Use the native ScheduleWakeup tool, not CronCreate or a simulated wait.
On this first turn, reply BG CANCEL TEST ARMED, then schedule one wakeup with
delaySeconds 180. Its prompt must say: "This is the fallback turn of the Vigil
background cancellation test. Call ScheduleWakeup with stop true, reply exactly
BG CANCEL TEST FALLBACK STOPPED, and do nothing else."
After scheduling, end this turn without another tool call. I will press Esc
while the pending-wakeup indicator is visible. If the named native tool is
unavailable, reply BG CANCEL TEST UNAVAILABLE and stop; do not substitute tools.
```

Capture a pending-state snapshot, ask the user to press Esc while attached,
capture cancellation, and observe beyond the actual scheduled time. Success
requires a persisted pending signal that clears after Esc, a valid owner
identity throughout, and no scheduled turn after cancellation. The bounded
fallback stops the loop if cancellation does not occur. Stop the recorder and
ask the user to stop only this throwaway job after collecting the final snapshot.
No Vigil reload or power changes are part of this test.

This experiment establishes background-job coverage only. The user subsequently
accepted that boundary for Phase 8 automatic self-paced loop tracking; see the
October 12 scope decision below.

Recorder started at 11:40:09 IST on October 12, with a hard ten-minute deadline.
The initial sample verified the throwaway's PID 12624 and UTC process start,
session ID `fc66a35d-c474-4737-959f-9217348512d2`, Claude version 2.1.296,
interactive kind and idle status. No background job existed at that point.
Temporary evidence directory: `vigil-bg-loop-20261012-0udxzb5l` under the macOS
temporary directory; recorder PID 6584. Backgrounding and cancellation are not
yet measured.

At 11:45:19 IST, `/bg` opened a dialog (`waiting` / `dialog open`) and created
job `6cd06110`. Backgrounding changed the session ID to
`6cd06110-9982-47de-b25e-1d013e5e6fc4`; the original process disappeared.
The initial recorder matched the original session ID, so its empty samples
after this handoff are a collection gap, not evidence of idle or absent work.
It was stopped at 11:47:12 after 423 samples; its process was confirmed gone.

Direct inspection verified worker PID 13179 against its UTC start time, job ID,
name and throwaway cwd. Its parent is a Claude PTY host under the new supervisor.
Attachment PID 13915 has `attach` and the exact job ID in its arguments. A fresh
recorder, PID 17230, began at 11:48:10 IST with a ten-minute deadline and both
the new session ID and job ID as selectors. Temporary evidence directory:
`vigil-bg-loop-attached-20261012-1rtm3sbh` under the macOS temporary directory.
Its first sample reports raw session `kind: bg`, `status: idle`; job state
`working`, `tempo: blocked`, zero in-flight tasks, no pending-wakeup sidecar.
The new session's expected transcript path does not yet exist. These are
baseline observations before the loop prompt, not cancellation results.

### Background loop cancellation result — persisted signals clear on Esc

The attached throwaway used native ScheduleWakeup on Claude Code 2.1.296.
The transcript confirms exactly one scheduling call in this experiment, with
`delaySeconds: 180`; the tool returned a wakeup at 11:54:00 IST, in 210 seconds.
No shell command, CronCreate or subagent call appeared in this test turn.

| Time (IST) | Observation |
| --- | --- |
| 11:50:22.089 | Worker reports busy; own caffeinate child present |
| 11:50:29.898 | Native scheduling succeeds |
| 11:50:30.083 | Worker reports idle; job has one `session_cron` task and `inFlight.wake.at`; `.pending-wakeup` contains the same scheduled timestamp |
| 11:50:51.958 | Pending-state snapshot confirms both signals and the worker identity |
| 11:51:00.088 | Own caffeinate child is absent while the wakeup remains pending |
| 11:51:26.294 | Job-state update after Esc removes `wake` and `session_cron`, with zero tasks |
| 11:51:27.078 | First sample with cleared in-flight state and no `.pending-wakeup` file |
| 11:51:56.533 | Snapshot after the user reports cancellation confirms both signals are clear |
| 11:54:00.105–11:55:51.103 | All 112 samples at or after the scheduled time remain idle, with zero tasks and no pending wakeup |

The recorder stopped at 11:55:52.032 after 462 samples, with no sampling errors;
its process was confirmed gone. The worker's PID and UTC process-start identity
matched throughout. Every sample after the observed cancellation retained the
cleared state. No scheduled-fire event, assistant response, tool call or fallback
stop turn appeared after cancellation.

The raw session file did not change after cancellation. The transcript did
receive a `system/away_summary` record at 11:53:33.756; its recap still described
the cancelled wakeup as pending. This was not a scheduled turn or a reliable
lifecycle signal. Record types matter: transcript growth alone does not prove
work resumed, and prose summaries must not override direct scheduler evidence.

The job's top-level `state` stayed `working` and `tempo` stayed `idle` even after
the loop was cancelled. Those values alone cannot establish activity. In this
case the useful transition was in `inFlight` and the sidecar, while the worker
remained alive. Their live ownership must still be checked; a leftover file is
not proof that a worker can run.

**Result:** the attached background case has an observed read-only pending/clear
signal. This is evidence for the installed version and tested path, not a stable
upstream file contract. Ordinary interactive sessions still lack the equivalent
cancellation signal; the user subsequently accepted background-only automatic
loop tracking, as recorded below.

**Cleanup verified:** after the user stopped the throwaway, its job state became
`stopped` at 12:17:39.175 IST, with no in-flight state or pending-wakeup file.
A read-only check at 12:18:00 confirmed that its worker, attachment process and
both recorders were absent, and no session file remained for this job. The
experiment is complete; no shared-supervisor stop or Vigil power change was
performed.

### October 12 scope decision — automatic loops require background mode

The user explicitly approved requiring background mode for automatic loop
tracking in Phase 8 and proceeding to the next research item. Supported
self-paced background loops retain a hold through their scheduled gaps until
they stop. Ordinary terminal turns remain in scope; ordinary terminal loop
gaps require `/bg` or a manual hold. Coverage documentation must state this
limitation, and the classifier must not claim exact interactive loop lifecycle
tracking from transcript replay. Fixed-interval cron policy is still undecided.

### October 12 Codex preparation — existing evidence, no new experiment started

The installed binary reports `codex-cli 0.162.0-alpha.17.2`. The host bundle is
ChatGPT.app version `26.1007.30912` (build `20156`, identifier `com.openai.codex`).
Its worker executable remains
`/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex`.
The source's old Codex.app carve-out still does not cover that path before the
`/Applications/` exclusion. The separate `exec-server` process must not be
mistaken for an additional active session.

A read-only open-file check associated this research task's rollout with the
live app-server PID 5148, started at 05:20:08 UTC. That same process held four
rollout files open at the observation point. This demonstrates why process
counts cannot substitute for session counts; an open file establishes a host
association, not that every associated session is working.

This task's existing rollout contains matching `task_started` / `task_complete`
turn IDs, including the immediately preceding turn. It has no `turn_aborted`
record yet. Its session header retains CLI version `0.155.0-alpha.9.2` from
creation on September 21, despite the current binary being newer. Do not use a
resumed session's creation-time version as proof of its current runtime version.
Current records also contain `event_msg/item_completed` with thread/turn IDs;
`CommandExecution` items include a logical `process_id`, status and exit code.
That schema alone does not prove how a still-running terminal is represented.

The [current app-server documentation](https://learn.chatgpt.com/docs/app-server)
defines live thread-status notifications, interrupted turn completion and an
experimental background-terminal listing. These are protocol interfaces, not
guarantees about persisted JSONL. The observed app-server has no explicit
`--listen` argument; the documented default is stdio. Starting another server
would not establish access to this process's loaded sessions. No server was
started, no RPC request was issued, and no app transport or settings were changed.

### Codex completion and interruption — approved October 12

The user approved creating a disposable local Codex task and running this
experiment. Keep this research conversation intact. Use a projectless task and
no project files, shell commands, subprocesses or delegated agents for this round.
First establish the disposable task's identity and its owning app-server; only
then start a detached metadata recorder with a hard ten-minute deadline.

Initial task prompt:

```text
This is a disposable task for a controlled Vigil activity-detection experiment.
Do not inspect or edit files, run commands, create goals, or launch agents.
For this setup turn, reply exactly CODEX PROBE READY and wait for the next prompt.
```

Normal completion prompt:

```text
This is the normal-completion case of the Vigil experiment.
Call the clock.sleep tool once with duration_ms 20000.
When it returns, reply exactly CODEX NORMAL DONE.
Do not use any other tool or do other work.
If clock.sleep is unavailable, reply CODEX PROBE UNAVAILABLE and stop;
do not substitute another tool or a shell command.
```

Interruption prompt, only after normal completion has been captured:

```text
This is the interruption case of the Vigil experiment.
Say CODEX INTERRUPT ARMED, then call clock.sleep once with duration_ms 60000.
I will press Stop while that tool is waiting.
If the wait finishes first, reply exactly CODEX INTERRUPT FALLBACK DONE.
Do not use any other tool or do other work.
If clock.sleep is unavailable, reply CODEX PROBE UNAVAILABLE and stop;
do not substitute another tool or a shell command.
```

The user operates Stop in the disposable task while the observer stays in this
research task. Record only process identity, rollout identity and file metadata,
turn-boundary types/IDs/timestamps, and observation times. Check that normal
completion closes its own turn; interruption yields a persisted terminal event;
the observer's activity does not keep the disposable task classified as working;
and both sessions are counted independently despite sharing a host. If the wait
finishes before Stop, report the interruption case as unmeasured and coordinate
another attempt within the approved scope rather than inferring an abort.

Stop the recorder and archive the disposable task after collecting the result.
No Vigil reload or power changes are included. This round does not establish
background-terminal survival or its ownership signal; that remains a separate
bounded experiment after the turn lifecycle is understood.

### Codex first run — completion measured, interruption window missed

Created the projectless task `Vigil Codex activity probe`, thread
`01a12eed-ca89-7ca1-8dbb-0f0072f32939`. Its creation-time CLI version is
`0.162.0-alpha.17.2`, matching the installed binary. The setup response was
`CODEX PROBE READY`. A direct open-file check verified that app-server PID 5148
held the probe's rollout, alongside the research task's rollout.

The recorder began at 12:37:07 IST with a ten-minute deadline. It consumed both
rollouts incrementally, retained incomplete trailing bytes, and recorded only
selected event/process metadata. Temporary evidence directory:
`vigil-codex-turns-20261012-9nmlskms` under the macOS temporary directory.

| Case | Persisted event (IST) | First observed sample (IST) |
| --- | --- | --- |
| Normal turn starts | 12:37:13.841 `task_started` | 12:37:13.929 |
| Normal turn completes | 12:37:41.692 `task_complete` | 12:37:41.931 |
| Intended interruption turn starts | 12:38:39.547 `task_started` | 12:38:39.933 |
| Wait expires; fallback completes | 12:39:47.589 `task_complete` | 12:39:47.933 |

The normal probe used the requested 20-second clock wait and returned
`CODEX NORMAL DONE`; its start and completion carried the same turn ID.
After completion its outstanding-turn set was empty while this research task
remained active on the same host. These timestamps establish observed file-event
latency in this run, not UI-to-file latency or a general maximum.

The 60-second interruption attempt finished before the user could locate Stop.
The app reported `completed` with `CODEX INTERRUPT FALLBACK DONE`; the rollout
contains a matching completion and no abort for that turn. This is not an
interruption result. The user's screenshot showed the already-completed reply,
so no active Stop control was expected at that point.

The recorder was stopped at 12:43:34.906 after 387 samples and its process was
confirmed gone. There were no sampling or JSON parse errors; the host identity
matched throughout. The throwaway task remains idle for an approved retry.
Before restarting, explain the Stop control and wait for user readiness; do
not consume the interruption window while the user is still locating it.

Read-only inspection of the installed app's composer code confirmed that the
submit control has a `stop` state, an accessible label `Stop`, and an `onStop`
handler while a response is in progress. The supplied screenshot is an idle
state, so it cannot establish the running control's appearance. Automated native
UI inspection of this app was unavailable; no alternate UI-control route was used.

### Codex interruption retry — measured October 12

After the user confirmed readiness, a fresh recorder started at 12:50:50 IST
with the same two rollout identities and a ten-minute deadline. It verified
the app-server's PID/start identity and an idle probe before the retry was sent.
Temporary evidence directory: `vigil-codex-interrupt-retry-20261012-lisatxiu`.
The retry again requested one 60-second `clock.sleep`, preceded by
`CODEX INTERRUPT ARMED`, with no other tool or work. The user pressed Stop.

| Boundary | Persisted event (IST) | First observed sample (IST) |
| --- | --- | --- |
| Retry starts | 12:51:03.189 `task_started` | 12:51:04.160 |
| User interrupts | 12:51:21.734 `turn_aborted` | 12:51:22.162 |

Both records carry turn ID `01a12efc-83d0-7ac1-b236-8ae98b18faff`.
The abort has `reason: interrupted`; the app independently reported the turn
as `interrupted`, lasting 18,545 ms, and the thread as `idle`. The rollout
contains the clock wait's call at 12:51:09.485, before the abort. This verifies
interruption during the tool wait rather than merely before dispatch.

All 58 samples from the first observed abort through 12:52:19.164 had no
outstanding probe turn while the research task remained active. App-server
PID 5148 and its start identity matched throughout. The abort was observed
0.428 seconds after its persisted timestamp; this is file-event observation
latency, not a measured delay from clicking Stop.

The recorder stopped at 12:52:20.164 after 90 samples, with zero sampling or
JSON parse errors; its PID was confirmed absent. The disposable task was
archived successfully. Normal completion and user interruption are now
measured on the installed version. This round launched no terminal work,
so it does not establish terminal survival, ownership or exit tracking after
a turn ends. No production code, Vigil installation or power state changed.

### Codex terminal survival and ownership — measured October 12

The user approved this separate experiment. A new projectless task,
`Vigil Codex terminal probe` (`01a12f00-ab75-7cf3-84bc-52a10969528d`), ran on
CLI `0.162.0-alpha.17.2`. A read-only open-file check verified its rollout was
held by app-server PID 5148, with the same start identity as the previous round.
The earlier probe remained archived.

Each case ran one prewritten Python workload through `exec_command` with
`yield_time_ms=1000` and `login=false`. The command used `exec` and remained
the tool's foreground process; there was no shell `&`, `nohup` or manual
detachment. After the tool yielded a session ID, the normal case returned a
final answer immediately. The interruption case announced readiness and waited
in `clock.sleep`; the user pressed Stop during that wait. Neither case polled
or terminated the terminal.

The workloads were quiet, with normal durations of 90 and 120 seconds and an
independent 150-second alarm. Each had its own process group, spawned no
children, and was covered by observer cleanup that checked process identity
before killing a still-live group. Start/exit receipts were experiment ground
truth only, not a proposed Vigil input. A detached recorder with a ten-minute
deadline sampled both rollouts and selected process metadata once per second.
Temporary evidence directory: `vigil-codex-terminals-20261012-9iu1pebn`.

| Case | OS PID / logical tool session ID | Turn ends (IST) | Workload exits (IST) | Survival after turn end |
| --- | --- | --- | --- | --- |
| Normal final answer | 95562 / 8395 | 12:58:23.932 `task_complete` | 12:59:49.957 | 86.025 s |
| User Stop | 97891 / 86423 | 13:00:27.977 `turn_aborted` | 13:02:20.819 | 112.842 s |

Normal turn ID: `01a12f03-1749-7af1-8a50-b7fb04073d79`.
Interrupted turn ID: `01a12f04-f022-7931-99d0-9299780921b6`.
The app reported the probe idle after each turn. All 86 normal-case and 112
interruption-case samples with a live workload after turn end had no outstanding
probe turn, while the observer task remained active under the same app-server.
The workload parent and host start identity remained unchanged until exit.

**Ownership:** a read-only process inspection extracted only the known
`CODEX_THREAD_ID` field from each experiment-owned workload. Both values matched
the probe thread exactly, including after Stop. The workload did not set this
field. No other environment values were emitted or saved. Process parentage
alone reaches the shared app-server and therefore cannot identify a session;
the native thread marker supplies the missing per-session association in this
tested launch path. Its absence or a failed identity check must remain unknown,
not positive proof of idle. No claim is made that every process carrying this
marker is terminal work.

**Exit:** without polling or a follow-up probe turn, both exits appended
`event_msg/item_completed` with a `CommandExecution` item. These contained the
original thread ID and turn ID, `source: unified_exec_startup`,
`status: completed`, exit code 0, and logical `process_id` values `8395` and
`86423`. Those IDs match the earlier tool responses' `session_id`; neither is
the OS PID. The normal exit record was persisted at 12:59:49.960 and observed
at 12:59:50.897. The interrupted case was persisted at 13:02:20.828 and observed
at 13:02:20.903. Each process was absent in that same first exit sample.
These are file/process observations, not UI latency guarantees.

**Other candidates:** the existing
`~/.codex/process_manager/chat_processes.json` file has fields for thread, turn,
logical process ID and OS PID, but it never acquired an entry for this probe or
changed during the experiment. It is not a usable current activity source for
this path. The [official app-server API](https://learn.chatgpt.com/docs/app-server)
also exposes experimental per-thread terminal listing, but that is a live
protocol interface. No private app transport was connected and no second
app-server was started to obtain these measurements.

**Local implementation analysis:** `ProcScanner` currently refreshes command
and executable metadata only. The installed `sysinfo` 0.39.3 dependency already
has scoped environment refresh and `Process::environ()` on macOS, implemented
through `KERN_PROCARGS2`. That is a candidate way to read the one ownership field
within the existing scanner; it was inspected in source, not exercised through
a new Rust build in this experiment. Candidate-process selection, short-lived
process races, PID reuse and missing ownership evidence still need explicit
implementation rules and checks.

**Cleanup:** the recorder stopped at 13:02:47.889 after 274 samples. There were
no sampling, registry-read or JSON parse errors; the host identity matched
throughout. Both workloads exited normally, and both PIDs and process groups
were verified absent. The recorder PID was also absent. The throwaway task was
archived successfully. No production code, build, Vigil reload or power change
was part of this round. This establishes ordinary local yielded terminals;
detached descendants, host restart, PTY-specific behavior and remote execution
were not exercised.

### Claude combined waits and Copilot lifecycle — measured October 12

The user approved both rounds in throwaway sessions. Installed versions were
Claude Code 2.1.296 and Copilot CLI 1.0.95. Each used a separate empty temporary
workspace and a new explicit session UUID; existing conversations were neither
resumed nor prompted. Interactive CLIs ran in dedicated PTYs with fifteen-minute
controller and child deadlines. The observer operated the native dialogs and
Copilot's double-Escape interruption directly through those PTYs.

The four workloads each ran for 90 seconds, with a separate 150-second alarm,
their own process group, no child processes, and observer cleanup. Native
background tool modes launched them; no shell `&` or manual detachment was used.
The observer's ten-minute metadata recorder ran outside both agents' tools.
No production code, Cargo build, Vigil reload or power command was involved.
Evidence directory: `vigil-provider-probes-retry-20261012-rxyt3dl2`.

#### Setup correction and cleanup

The first Claude throwaway's terminal mishandled a pasted prompt, and its next
request failed with an API error stating that tool definitions exceeded
4,194,304 bytes. No workload started. That throwaway and its 80-sample recorder
were stopped. The controller unnecessarily attempted to signal the already-exited
child's group and received `PermissionError`; a process scan confirmed both the
child and group were absent. The temporary controller was corrected to signal
only while its owned child remained unreaped.

A new Claude session used `--strict-mcp-config` with an empty MCP configuration,
retaining native Bash, AskUserQuestion and Read. That session completed both
cases. This isolates the successful measurement from the initial tool-payload
failure; it does not attribute that failure to a particular plugin. Manual
permission mode and a temporary `permissions.ask: ["Read"]` rule produced real
approval dialogs. Only individual test actions were approved; no global
permission default was changed. Bracketed paste delivered the subsequent
prompts correctly.

#### Claude: waiting and live background shells coexist

Session `aea3d553-12da-4951-af95-a72ccf775793`, PID 3405, reported
`kind: interactive`. Each case first started native background Bash work, then
opened its foreground dialog. The observer left that dialog unanswered through
the workload's natural exit, then answered or approved it.

| Observation (IST) | Question case | Read permission case |
| --- | --- | --- |
| Workload starts | 15:22:32.246, PID 4570 | 15:25:48.751, PID 7163 |
| First observed foreground wait | 15:22:36.880, `input needed` | 15:25:56.896, `permission prompt` |
| Own caffeinate child first absent during the wait | 15:23:06.905 | 15:26:26.901 |
| Workload exits naturally | 15:24:02.250 | 15:27:18.756 |
| First observed busy state after answering/approval | 15:24:25.911 | 15:27:39.913 |
| First observed idle state after final answer | 15:24:28.881 | 15:27:42.899 |

There were 86 samples with the question open and its workload alive, and 82
with the permission dialog open and its workload alive. Of these, 56 and 52
respectively had no own caffeinate child. Both cases still reported `waiting`
after workload exit until the foreground dialog was answered. They ended with
the requested completion markers and returned to idle.

Both workload processes carried native `CLAUDE_CODE_SESSION_ID` matching this
session and remained children of the live Claude process. The question-case
stdout/stderr descriptors pointed to Claude's native session-specific task
output file. This supports ownership for the measured Bash path; the experiment's
own receipt files are ground truth, not production inputs. Only matching
session-field names and selected process metadata were retained, not credential
values. The nested Claude process also inherited Codex environment keys from
the observer's launch context, reinforcing that a provider tag without host
validation is insufficient.

**Consequence:** the original unconditional `waiting -> inactive` rule is wrong
when other owned work is running. Human waiting alone does not create demand;
the independent background work does. The simultaneous background-subagent
case was not exercised, so these results must not be generalized to all
in-process background work.

#### Copilot: dispatch, turns and shell lifetime are distinct

Session `faf568cd-e5b4-4379-a381-71a4714f53d6`, PID 4575, used native `bash`
with `mode: async`. Its tool result supplied a shell ID immediately. In the
normal case, the model then returned the requested final answer. In the
interruption case, it called `read_bash` with `delay: 60` for the yielded shell;
the observer pressed Escape twice in quick succession during that wait.

| Event (IST) | Normal case | Interruption case |
| --- | --- | --- |
| Shell starts | 15:25:02.179, PID 6721, shell ID `0` | 15:28:16.021, PID 8231, shell ID `1` |
| Final answer / abort | 15:25:04.809 `COPILOT NORMAL PROBE DONE` | 15:29:06.757 `abort`, `reason: user_abort` |
| Process exits naturally | 15:26:32.184 | 15:29:46.024 |
| Persisted shell-completion notification | 15:26:32.217 | 15:29:46.056 |
| Process survival after final answer / abort | 87.375 s | 39.267 s |

Both processes carried native `COPILOT_AGENT_SESSION_ID` matching the probe and
remained children of the same live Copilot PID. Ownership was observed again
after abort. Each persisted exit was a structured `system.notification` with
`kind.type: shell_completed`, the matching `shellId`, and `exitCode: 0`.
The CLI followed each notification with a `read_bash` call at `delay: 0` and a
fresh assistant turn. This automatic follow-up occurred even after user abort;
the observer did not submit another prompt to request the read or continuation.

Important distinctions from the original parser proposal:

- `tool.execution_complete` for asynchronous Bash was already present at
  dispatch. It did not mean the shell had exited.
- The interrupted `read_bash` had no matching `tool.execution_complete` through
  session shutdown. Foreground cancellation needs its own accounting rule;
  outstanding-tool counts cannot simply wait forever for a completion record.
- `assistant.turn_end` preceded the abort event in this case. A subsequent
  shell-exit notification started new work, so abort is not a permanent
  session-idle marker.
- Numeric `turnId` restarted at `0` for new user interactions and again for the
  shell-completion follow-up within the same `interactionId`. Event ordering
  and scope matter; numeric turn ID alone is not a unique work identity.
- Four `permission.requested` and four `permission.completed` events were
  persisted for real path and command approvals. The earlier broad claim that
  Copilot permission events are always ephemeral does not hold for this CLI.
  No `session.idle` record appeared. User-question persistence was not tested.
- The session had native `inuse.4575.lock` and `inuse.4575.hold` files while
  open; both were gone after normal CLI exit. Their existence alone is not a
  replacement for validating the live process identity.

Current-version subagent interleaving was not exercised in this round; the
older evidence for it remains a separate coverage consideration.

#### Verified cleanup

All four workloads completed naturally. The successful recorder stopped at
15:30:57.912 after 558 samples, with zero sampling or JSON parse errors. Both
CLIs exited through `/exit` with status 0. A final process scan found none of
the four workloads, their groups, either CLI, either PTY controller, or the
recorder. Claude's session-status file and Copilot's in-use files were absent.
The throwaway transcripts and temporary evidence remain available for review.

### Claude waiting with an in-process background subagent — October 12

The user approved this experiment in a throwaway session, followed by plan
reconciliation. Claude Code 2.1.296 ran in an empty temporary workspace containing
copies of Vigil source for a finite read-only analysis. Existing sessions were
not resumed or prompted. Session `94291bcb-f77e-4e69-b676-2722f208e40a`, PID 22288,
used manual permissions, native Agent/Read/Grep/Glob/AskUserQuestion tools and the
same empty strict MCP configuration as the successful preceding probe. A
session-only ask rule covered one harmless permission fixture. No global
permission mode was changed, and the observer approved only that single Read.

The dedicated PTY controller and CLI had independent fifteen-minute deadlines;
the detached metadata recorder had a fourteen-minute deadline. Exactly one
background `general-purpose` subagent performed real source analysis with a
35-tool-call cap: 27 Read, seven Grep and one Glob. It made no edits, shell calls,
artificial delays or nested agents. The observer first left a native question
open, answered it after establishing sustained overlap, then requested a native
Read permission dialog while the **same** child was still running. This avoided
starting a second analysis agent. The permission dialog stayed open through
child completion.

Evidence directory: `vigil-subagent-waits-20261012-lad96ggn`; its `summary.json`
contains the reduced lifecycle, sample counts and cleanup evidence. Times below
are IST (UTC+05:30); native records and the recorder use UTC.

| Observation | Time (IST) | Evidence |
| --- | --- | --- |
| Agent dispatch returns | 16:12:18.386 | Native `isAsync: true`, `status: async_launched`, agent ID `aecf9ba24458d94db` |
| Parent question wait first observed | 16:12:22.537 | `waiting` / `input needed` |
| Own caffeinate first absent during question | 16:12:53.522 | Child continued file reads; question stayed open |
| Last child tool call | 16:14:04.498 | Grep, the 35th tool call; synthesis continued afterward |
| After answering question | 16:15:35.532 | Parent returns to busy; child still running |
| Parent permission wait first observed | 16:16:09.537 | `waiting` / `permission prompt` |
| Own caffeinate first absent during permission | 16:16:39.532 | Child still running in the same Claude process |
| Child final answer | 16:18:27.296 | Child transcript `assistant`, `stop_reason: end_turn` |
| Native completion queued in parent | 16:18:27.370 | `queue-operation: enqueue`, matching task/tool IDs, `status: completed` |
| Permission approved; notification consumed | 16:19:28.222 | Matching queue removal, `reason: absorbed_mid_turn` |
| Parent returns to idle | 16:19:32.529 | After `PERMISSION WAIT RELEASED` |

The child's measured dispatch-to-final lifetime was **368.910 seconds**. There
were 193 question-wait samples during that lifetime, 162 without an own
caffeinate child. The permission case contributed another 138 overlapping
samples, 108 without caffeinate. There were then 61 permission-wait samples
after child completion, before approval. Foreground status therefore did not
change when independent background demand ended.

The last tool call preceded the final answer by 262.798 seconds. The UI continued
showing child generation during the gap, and the final record arrived while the
permission remained unanswered. A short transcript-mtime window cannot stand
in for this lifecycle. No separate long-lived subagent process appeared in the
one-second process samples; subprocess ownership alone cannot cover this case.

#### Read-only lifecycle evidence

The parent Agent tool result supplied `agentId` and `outputFile`. The persisted
Agent arguments did **not** include `run_in_background`, despite the prompt
requesting it: current interactive defaults selected background execution.
The native result and child `.meta.json` establish what actually happened;
metadata contained `requestShape: background`, `requestNonInteractive: true`,
`spawnDepth: 1` and the matching `toolUseId`.

The output path was a symlink to
`<project>/<sessionId>/subagents/agent-<agentId>.jsonl`. Child records carried the
same parent session ID, agent ID and `isSidechain: true`. The metadata/transcript
remained after completion, so mere file existence is not running evidence.
These paths and background defaults are corroborated by the
[official subagent documentation](https://code.claude.com/docs/en/sub-agents).

The parent's queued task notification recorded completion **before** the human
answered the permission prompt. Its task ID and originating tool-use ID matched
dispatch. Queue removal later repeated that notification; a reducer must treat
it idempotently. Completion content also states that a resumed child can notify
again using the same task ID; that is a native hint, not a measured resume case.
Use invocation/event boundaries rather than permanently marking an agent ID
finished. Correlate native envelope fields, not prose in arbitrary user messages.

The AskUserQuestion call was only observed in persisted parent transcript
records after its dialog was answered. Sampling direct status and independent
child evidence is therefore necessary during the open wait. Neither the
parent's foreground status, the caffeinate child, nor the parent transcript's
last completed tool can alone describe the whole session.

**Result:** normal background-subagent activity and completion are established
across both parent wait types. Track that activity independently, then release
its demand on the matching terminal record even if the foreground remains
waiting. Subagent cancellation/failure/resumption, nested surviving work and a
permission requested by the child itself were not exercised. This result does
not close those lifecycle gaps or the remaining provider research.

#### Cleanup and repository boundary

The CLI exited normally through `/exit` at 16:20:07.062 with status 0. The recorder
stopped at 16:20:07.457 after 537 samples, with zero sampling/JSON parse errors
and zero host identity mismatches while the host was present. The controller,
CLI, recorder and CLI process group were verified absent; the native session
status file was removed. The copied source was checked against the repository.
No production code, build, Vigil reload, power change or commit was made.

### October 12 plan reconciliation and remaining work

The user explicitly chose **research the remaining surfaces before
implementation**, rather than defer Claude Desktop, fixed cron, detached/remote/
restarted work or unverified Copilot subagent behavior. The revised
[Phase 8 plan](../future/phase-8-per-session-activity.md) removes the obsolete
waiting-is-idle ladder, fixed 64 KiB tail, 30-minute silent-turn expiry,
per-pid session counting and timestamp-only refresh acknowledgement. It defines
shared classification, independent work lifetimes, labeled uncertainty,
versioned output changes, grace consumption and the research/implementation
sequence. Current-version Copilot subagent and wait interleaving is next.

Two nearby source findings surfaced during the read-only subagent analysis and
were inspected locally afterward. Neither was changed or live-reproduced:

- **Wrapper identity mismatch:** `wrapper_pidfile_body` stores creation-time
  `now` as `start_ts`; both `vigil run` and `vigil lock` pass the time they write
  the file. GC compares it with the process's actual start and applies the
  mismatch rule to wrappers. If those seconds differ, GC drops a live wrapper.
  The lock path writes only after its helper permission probe, making the
  distinction material. Before preserving wrapper behavior through Phase 8,
  add a failing delayed-registration fixture and propose writing the actual
  process start identity; do not remove PID-reuse protection as a shortcut.
- **Literal read-only contract versus existing IPC:** `CheckEngine::snapshot`
  calls `helper.status()`. The current IPC client creates/renames a request file,
  causing helper response and log writes even for Status. The helper's Status
  action itself does not alter power state. This is a filesystem-write mismatch
  with the stated read-only command invariant, not evidence of a power mutation.
  Resolve the diagnostic health-probe contract before claiming a no-write test
  passes; any helper-boundary change requires its dedicated seam suite and a
  separately agreed repair scope.

### October 13 — final research round and implementation handoff

The user authorized analysis and testing without additional permission and
requested a self-contained plan for the next session. That supersedes the
older ask-and-pause instructions above. This round completed feasible disposable
measurements, investigated remaining surfaces through installed source and
primary documentation, and preserved evidence in `docs/fixtures/phase-8`.
No production code was changed. The final handoff explicitly separates measured,
source-only, unobservable and unavailable coverage; it does not declare every
provider surface live-verified.

#### Copilot child/root interleaving and permission ownership

CLI 1.0.95 ran in a disposable directory with eight copied Rust files, manual
permissions and built-in MCPs disabled. The CLI and its PTY controller each had
900-second deadlines; the external recorder had an 840-second deadline. Folder
trust and the later command were approved once, not saved globally. Existing
sessions were untouched.

Setup reported native `task(mode: background)`, `read_agent`, `ask_user` and no
dedicated stop-subagent tool. One background general-purpose child analyzed the
copied files. It used nine calls (one Glob, eight View), exceeding the requested
eight-call cap by one; the prompt cap was not enforced by the runtime. No shell,
network, edits or nested child was requested or observed in that case.

Native timeline (UTC):

| Time | Record / observation |
| --- | --- |
| 08:26:37.573 | `subagent.started`, origin task call `toolu_01HNyzmRyabEvQH931nNaEMY` |
| 08:26:37.574 | Task dispatch `tool.execution_complete`; child is still working |
| 08:26:37.606 | Child `assistant.turn_start`, `data.parentToolCallId` = origin |
| 08:26:39.712 | Root `ask_user` starts while child runs |
| 08:26:43.100–.114 | Eight child View calls start/complete while root question remains open |
| 08:26:48.862 | `subagent.completed`, nine tool calls; human question still open |
| 08:33:11 | Human answers Continue; exact native completion time retained in fixture summary |

Both actors' turns/tools interleave in one `events.jsonl`. `data.parentToolCallId`
establishes child attribution; top-level `parentId` follows the global event
chain and cannot do that job. Child dispatch carries agent ID in the tool result
telemetry; `subagent.completed` refers to the originating toolCallId. No
`user_input.requested`, `user_input.completed` or `session.idle` was persisted
for this question. Native `ask_user` execution start/completion is usable here.

Second case: a single background child ran one prewritten Python workload with
native synchronous Bash. It slept for 70 seconds, wrote only its own receipt,
had a 110-second self-alarm and spawned no children. Bash had its own process
group, and controller cleanup was bounded. The parent's `read_agent(wait=true)`
was interrupted with the normal double-Esc control while the child ran.

| Time (UTC) | Record / observation |
| --- | --- |
| 08:33:51.260 | Child starts; task origin `toolu_011t1qs9Jzh6sm2n7wnNmXLz` |
| 08:33:53.234 | Child Bash tool starts, attributed by parentToolCallId |
| 08:33:53.309 | Permission requested for that Bash tool; workload receipt absent |
| 08:33:53.674 | Root `read_agent` starts; UI shows child permission and parent waiting |
| 08:34:26.913 | Permission completed after one-time Yes |
| 08:34:26.989 | Workload starts, PID 31547, Bash parent 31546 under CLI 14559 |
| 08:34:28.638 | Root `abort(reason: user_abort)`; UI waits for background agents |
| 08:35:36.990 | Workload exits normally, 68.352 seconds after parent abort |
| 08:35:37.007 | Child Bash completion, success and exit code 0 |
| 08:35:39.773 | Child completion, 71.135 seconds after parent abort |
| 08:35:39.790 | Native agent-idle notification; automatic read uses a new call ID |
| 08:35:42.195 | Notification-triggered root turn ends |

The permission request has no actor field but contains
`permissionRequest.toolCallId`; joining that to the child's tool start resolves
ownership. The root's interrupted read_agent has **no matching completion**
through shutdown. A generic outstanding-call counter would pin the session.
A parent waiting on a child that is itself waiting for permission is not two
running jobs; native wait dependencies matter.

The shell's **native** `COPILOT_AGENT_SESSION_ID` was
`7e9ad0d5-da9b-4dcd-9309-a3ad638159d2`, different from root session
`a03fcec8-b72f-4ce1-a30e-97ebce6a664f` and task agent
`13b5aca9-df07-442c-b9cb-31592e9c09eb`. The workload did not set it. Native root /
child logs and unique validated host ancestry are needed; marker equality alone
would miss this command. No full environment was recorded.

The native Bash API uses `initial_wait`, not the requested wall-time timeout;
the independent Python alarm provided the hard bound. The first child cap and
this timeout distinction are preserved so later readers don't mistake prompt
intent for executed behavior.

#### Codex detached process and host replacement

The installed CLI's generated JSON schema and
[documented app-server protocol](https://learn.chatgpt.com/docs/app-server)
provided a supported stdio interface. A dedicated server was started for this
experiment; it did not connect to the desktop app or observe the real host.
No custom CODEX_HOME, model override, private transport or account credential
inspection was used. A fresh thread ran `thread/shellCommand` with an explicit
15-second command timeout. No model turn was needed for this user-shell path.

The command forked exactly one detached child, which closed standard descriptors,
created its own session/process group, retained a 100-second alarm, and recorded
its own identity every five seconds for 80 seconds. The command and turn completed
at 08:35:53.148/.152 UTC, while child PID 32190 lived on under launchd PID 1 with
native `CODEX_THREAD_ID=01a13467-6104-7ef0-b259-077e7eb29b14`.

`thread/backgroundTerminals/list` was empty. The original disposable server
32145 was terminated normally, and a new disposable server 32241 resumed the
same thread. Its terminal list was also empty; the child remained alive. Both
servers exited 0 and the thread was archived. Child completed at 08:37:13.187.
This proves an empty terminal list/command completion/host exit is insufficient
for this launch path. It does not prove every desktop unified-exec or PTY path
has identical shutdown semantics, nor does an inherited marker alone establish
cold ownership after Vigil itself restarts.

#### Codex human approval is not fully reconstructable from the rollout

A separate fresh thread on the dedicated server used a read-only sandbox and
native on-request approval. It was asked to create one harmless fixture file.
The native shell requested approval; the observer held it for three seconds,
read current thread state and captured the pending rollout, then declined. It
used no workaround and created no file. Server exited 0; thread was archived.

At 08:42:40.041 UTC, live `item/commandExecution/requestApproval` identified the
thread/turn/item. Live `thread/read` reported `active` with
`activeFlags: [waitingOnApproval]`. The pending on-disk rollout contained an
unfinished task and a `custom_tool_call`, but **no persisted corresponding wait
record**. After decline the flag cleared and the turn completed at 08:42:46.204.
The generated installed schema also defines `waitingOnUserInput`; that second
flag was source-inspected, not separately exercised.

This disproves universal exact human-wait inference from task_started/task_complete
alone. A private desktop connection or an unrelated second server cannot repair
that observer gap. The handoff keeps ambiguous currently-owned unfinished work
as explicitly uncertain demand, so it may hold through such a wait; it does not
claim exact waiting release. Full exactness would require a separate supported
integration. This is a product capability limit, not a reason to keep rerunning
the same permission experiment.

#### Claude remaining lifecycle and scheduling research

Claude Code 2.1.296's new throwaway setup was rejected by Fable 5.1's provider
safeguard before any experimental tool ran. The runtime retried once and offered
to switch models; the observer chose to stay and stop, then exited. No model
switch or prompt workaround was attempted. Thus this round did **not** measure
child cancel/failure/resume, child permission, fixed cron or detached worker
restart. The earlier normal completion and background-loop measurements remain
valid evidence for their stated versions and paths.

Read-only installed-source analysis supplied narrower facts. Binary SHA-256:
`c9b5341637becbd423ddffc5b254afb645682a3868cb708bbc6cc0e7bb419937`.
Exact ranges and individual hashes are in `claude-source-findings.json`; no raw
proprietary source is copied into the repository.

- At byte 199458705, the notification builder accepts completed/failed/killed,
  optional runId, and repeated notification for a task ID. It explicitly handles
  completion with owned background work still running. It cannot justify closing
  every work item when one child reports a result.
- Around 199461000, cancellation updates in-memory state and aborts the child;
  the code also handles orphan shell work. Around 199776800, metadata writing
  supports `stoppedByUser`, parentAgentId, toolUseId and background request shape.
  These branches do not prove every terminal status is promptly persisted in
  every UI mode. A stale flag cannot close a resumed invocation.
- Around 194961000, scheduling has in-memory and durable project-file branches;
  durable ownership includes session, PID/start and project. Around 195146013,
  `session_cron` contributes to inFlight tasks, but `wake` is derived specifically
  from loop-kind work. Generic scheduled-task count is not active model work.
- Around 202134025, `.pending-wakeup` has a bounded schema with toolUseId,
  scheduledFor and chainStartedAt, and serialized write/removal in the background
  session path. Installed liveness checks include PID, procStart and zombie.
  Attachment is not the worker; stale sidecars cannot prove restart recovery.

[Current scheduling documentation](https://code.claude.com/docs/en/scheduled-tasks)
describes CronCreate restoration on resume, with exclusions, and terminal-
independent background sessions. Installed source retains flag-dependent storage
branches. Neither proves the enabled restoration branch here. The plan therefore
uses current owned work, not historical schedule existence, to establish demand.
The fixed-cron gap policy is a **new recommendation in the handoff**, not a user
answer to the earlier self-paced-loop question.

[Subagent documentation](https://code.claude.com/docs/en/sub-agents) routes
background permission prompts to the parent. The measured generic root status
is not enough to attribute every such prompt; ambiguous child activity is unknown.
[Desktop documentation](https://code.claude.com/docs/en/desktop) distinguishes
Local, Cloud and SSH execution. No actual Claude Desktop bundle was installed
in standard app directories; the existing Claude Code URL Handler is only a
handler. Desktop internals and actual off-machine execution were not tested.

#### Local implementation checks, preservation and cleanup

The existing repo's `cargo fmt --check`, `cargo build`, workspace all-target
clippy with warnings denied, and `cargo test` all passed on October 13. No helper
boundary was changed, so helper-test-seam was not run in this research pass.
These checks establish a baseline, not Phase 8 implementation correctness.

A disposable Rust program linked to current Vigil/sysinfo rlibs successfully
read one selected environment marker with a scoped process refresh and obtained
process start/parent metadata. It printed only that selected synthetic marker's
success, not other environment values. Environment access is now exercised,
not merely proposed from dependency source. The handoff accounts for sysinfo
retaining full environments: use short-lived scoped extraction and keep only
needed ownership tags.

The same program reproduced the wrapper defect through actual library functions:
registration time 1010 with true start 1000 makes `gc_decision` return
`DropPidReuse` for a live wrapper; intended behavior after repair is Keep. The
helper diagnostic write path was independently source-confirmed, without issuing
a live helper request: CheckEngine calls helper.status; IPC writes a request and
the helper writes response/logs. Both have a bounded proposed repair in the plan.

Today both CLI controllers exited 0; the recorder stopped after 732 samples with
no top-level sampling errors. Saved prior summaries retain parser/identity checks
for earlier rounds. Copilot Bash/workload, CLI/controllers/recorder, detached
Codex workload and all three own app-servers were checked absent. The native
Copilot session ended normally, and both disposable Codex threads were archived.
Copied source was not modified by either probe. No live power, installed Vigil,
real session or production Rust file was changed.

The durable fixture bundle contains native lifecycle projections, prior reduced
summaries, the pending Codex approval rollout plus separate live API evidence,
source offsets/hashes, versions, cleanup and future replay assertions. The
implementation plan is now self-contained: do not reread old chat turns or rerun
completed experiments as a prerequisite. Preserve the explicit confidence and
coverage limits when implementing and documenting the feature.
