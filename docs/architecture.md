# Architecture

## Phase 1 (current)

```
                ┌─────────────────────────────────────┐
                │ launchd LaunchAgent (KeepAlive)     │
                │   com.thangaram.vigil               │
                └──────────────┬──────────────────────┘
                               │ runs
                               ▼
   ┌────────────────────────────────────────────────────────┐
   │ vigil-daemon (bash, 5s tick)                           │
   │  ├─ ps -axww -o pid= -o command=                       │
   │  ├─ match: claude / codex / copilot CLIs only          │
   │  ├─ refcount via PID files in state dir                │
   │  ├─ thermal: pmset -g therm                            │
   │  ├─ battery: pmset -g ps                               │
   │  ├─ on first acquire: snapshot baseline SleepDisabled  │
   │  ├─ helper engage: pmset disablesleep 1+caffeinate -di │
   │  └─ on full release: restore baseline + kill caffeinate│
   └────────────────────────────────────────────────────────┘
                               │
                    engage/release/status request files
                               ▼
   ┌────────────────────────────────────────────────────────┐
   │ root LaunchDaemon: com.thangaram.vigil.helper          │
   │  ├─ validates request file type, owner, mode, content   │
   │  ├─ accepts only: engage / release / status             │
   │  └─ runs fixed /usr/bin/pmset -a disablesleep 0|1 argv  │
   └────────────────────────────────────────────────────────┘
                               ▲
              writes PID files │
                               │
   ┌───────────────────────────┴──────────────────────┐
   │ vigil run <cmd>                                   │
   │   - write PID file                                │
   │   - "$@" (foreground; no exec — trap fires)       │
   │   - trap EXIT cleans PID file                     │
   └───────────────────────────────────────────────────┘
```

## Signal flow

Two signals, one source of truth (the daemon's refcount):

1. **Process polling** — daemon scans `ps -axww -o pid= -o command=` every 5 seconds, applies hard exclusions then matches CLI basenames. Each match writes/touches `~/Library/Application Support/vigil/state/active/<name>-<pid>.pid`.

2. **Wrapper PID files** — `vigil run <cmd>` writes a `wrapper-<pid>.pid` file with metadata before exec'ing the command (in foreground, not via shell `exec`), and removes it via `trap EXIT`.

The daemon counts files in `state/active/`. Transition rules:

- **0 → >0**: Snapshot the current `SleepDisabled` value into `state/baseline.json`. Submit an `engage` request to the root helper, which captures its own root-owned baseline if needed and runs `/usr/bin/pmset -a disablesleep 1`. Spawn `caffeinate -di &` and store its PID.
- **>0 steady state**: Verify every tick that `SleepDisabled=1` and the recorded
  `caffeinate` child is still alive. If either drifted, reassert immediately.
- **>0 → 0**: Submit a `release` request to the root helper, which restores its captured baseline with `/usr/bin/pmset -a disablesleep <baseline>` while marked engaged. Idle release requests are no-ops, so a stale retained helper baseline cannot clobber a later third-party sleep setting. Kill the caffeinate child and delete the user-visible `baseline.json`.

Stale PID files are GC'd when (a) the PID is dead, (b) the on-disk start_ts doesn't match the live PID's start time (PID reuse), or (c) the file is older than 30s and the PID's CPU is below 0.5%.

## Why a root helper instead of runtime sudo

The daemon runs under user-domain `launchd` with no controlling tty. Runtime `sudo` either needs a brittle `NOPASSWD` rule or risks blocking with no password prompt. Vigil now installs one root LaunchDaemon helper during `vigil setup`; normal runtime does not execute `sudo`.

The helper boundary is intentionally narrow:

- request actions are only `engage`, `release`, and `status`;
- the helper rejects symlinks, non-regular files, unexpected owners, group/other-writable request files, unknown actions, and extra request content;
- request files with multiple hard links are rejected, and the request directory itself must be owned by the configured user and not group/other writable;
- helper response, state, and log directories must be root-owned and not group/other writable; the user daemon also rejects helper response files that are not regular, root-owned files;
- the helper runs only fixed `/usr/bin/pmset -a disablesleep 1` and `/usr/bin/pmset -a disablesleep 0|1` argv;
- the helper tracks active engagement separately from the retained baseline, so each fresh engage re-captures current system state and idle releases do not run `pmset`;
- setup/uninstall may still prompt for admin access to install, bootstrap, bootout, or remove root-owned files.

This reduces noisy repeated sudo execution but increases responsibility: Vigil owns a persistent privileged component, so the request queue is treated as a real privilege boundary. Development tests reinforce that boundary by setting `VIGIL_TEST_NO_ADMIN=1` and putting a failing `sudo` shim first in `PATH`; test runs must not exercise admin paths.

## Why baseline restoration matters

If you have Amphetamine or another tool already holding `disablesleep=1` when vigil engages, the original draft would have set it back to `0` on release — clobbering the other tool's setting. Vigil snapshots the prior value at the first acquire and restores exactly that on the last release.

### Baseline stickiness: `SleepDisabled=1` is captured and re-captured

If `SleepDisabled=1` is the value vigil captures on its first engage — because another tool was already holding it, or a prior vigil crash left it pinned — every subsequent release restores to `1`, and the **next** engage re-captures `1` (since `baseline.json` was cleared on release and the live value is still `1`). The daemon log will then show `captured baseline SleepDisabled=1` on every engage. This is the design working as intended: vigil never lowers a sleep-prevention flag it didn't originally raise.

To reset the baseline back to `0`-state — i.e., to make vigil release all the way to sleep-enabled on the next quiet window — do one of:

- While vigil is **idle** (refcount = 0, no `baseline.json` on disk): release the other sleep-prevention tool, or intentionally reset `SleepDisabled` as an admin outside Vigil. The next vigil engage will then capture `0`, and the next release will go back to `0`.
- `vigil uninstall && vigil setup` — uninstall restores baseline and clears state; setup starts fresh.

If another tool (Amphetamine, an open `caffeinate -di` shell, etc.) is the *reason* `SleepDisabled=1` keeps coming back, vigil cannot fix that — the other tool will re-assert. Quit the other tool first.

## Crash recovery

If the daemon restarts and finds `baseline.json`, it refreshes live process
evidence before deciding what to do. If active agent or wrapper refs still
exist and thermal/battery guards allow holding sleep, vigil keeps the captured
baseline and reasserts `SleepDisabled=1` plus `caffeinate -di`. If no active
work remains, it restores the captured baseline and clears the stale state.

## Why `caffeinate -di` and not `-dimsu`

Reading `man caffeinate`:

- `-d`: prevent display sleep. Useful.
- `-i`: prevent system idle sleep. Useful.
- `-m`: prevent disk idle sleep. Cosmetic — doesn't affect agent runtime.
- `-s`: prevent system sleep. **Only effective on AC power**; no-op on battery.
- `-u`: declare user active. **Requires `-t <timeout>`** to be useful — without it, the assertion times out in 5 seconds.

For a daemon that doesn't have an enclosing command lifetime, `-d` and `-i` are the only flags that do real work. `pmset disablesleep` covers the closed-lid / system-sleep half. The original draft's `-dimsu` was misleading.

## Why per-user state and log paths

`/tmp/vigil/...` was the original draft. Two problems:

1. `/tmp` (i.e. `/private/tmp`) is periodically cleaned and may be empty after long uptime; symlink attacks are easier to mount in shared `/tmp`.
2. **launchd opens log files before exec'ing the program.** If the log directory doesn't exist when launchd tries to open the log files, the daemon fails to start with no useful error. Putting log paths under a directory that the installer creates first is the only reliable pattern.

So:

- State: `~/Library/Application Support/vigil/state/` (mode 0700, created by `vigil setup`).
- Logs: `~/Library/Logs/vigil/` (created by `vigil setup`).

## Why the daemon binary is COPIED out of the source repo

Found by smoke test: macOS TCC (Transparency, Consent, Control) **denies user-domain launchd permission to execute scripts under `~/Documents/`**. The first run of `vigil setup` pointed the LaunchAgent at `~/Documents/projects/personal/vigil/bin/vigil-daemon`, and launchd recorded `last exit code = 126` with stderr `"Operation not permitted"` for every spawn. Granting Terminal full disk access doesn't carry over to launchd.

The fix is the standard macOS LaunchAgent pattern: install the daemon out of the protected directory. `vigil setup` (and `vigil reload`) copy `bin/vigil-daemon` and `lib/*.sh` into `$VIGIL_INSTALL_DIR` (default `~/Library/Application Support/vigil/`), and the rendered plist points there.

Trade-off: edits to the source repo do not auto-propagate to the running daemon. `vigil reload` re-syncs and bounces launchd in one shot — no sudo needed.

A symlink in the install dir back to the source tree would NOT have helped, because TCC denies launchd from following symlinks into `~/Documents/` just as it denies direct execution.
