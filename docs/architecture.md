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
   │  ├─ enable: sudo -n pmset disablesleep 1+caffeinate -di│
   │  └─ on full release: restore baseline + kill caffeinate│
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

- **0 → >0**: Snapshot the current `SleepDisabled` value into `state/baseline.json`. Run `sudo -n pmset -a disablesleep 1`. Spawn `caffeinate -di &` and store its PID.
- **>0 → 0**: Read `state/baseline.json`, run `sudo -n pmset -a disablesleep <baseline>`, kill the caffeinate child, delete `baseline.json`.

Stale PID files are GC'd when (a) the PID is dead, (b) the on-disk start_ts doesn't match the live PID's start time (PID reuse), or (c) the file is older than 30s and the PID's CPU is below 0.5%.

## Why `sudo -n` everywhere

The daemon runs under `launchd` with no controlling tty. A plain `sudo` call would block waiting for password input and either hang the daemon or — worse — silently fail. All `pmset` calls go through a `sudo_n_pmset()` helper that uses `sudo -n` and aborts loudly if non-interactive sudo isn't available.

## Why baseline restoration matters

If you have Amphetamine or another tool already holding `disablesleep=1` when vigil engages, the original draft would have set it back to `0` on release — clobbering the other tool's setting. Vigil snapshots the prior value at the first acquire and restores exactly that on the last release.

### Baseline stickiness: `SleepDisabled=1` is captured and re-captured

If `SleepDisabled=1` is the value vigil captures on its first engage — because another tool was already holding it, or a prior vigil crash left it pinned — every subsequent release restores to `1`, and the **next** engage re-captures `1` (since `baseline.json` was cleared on release and the live value is still `1`). The daemon log will then show `captured baseline SleepDisabled=1` on every engage. This is the design working as intended: vigil never lowers a sleep-prevention flag it didn't originally raise.

To reset the baseline back to `0`-state — i.e., to make vigil release all the way to sleep-enabled on the next quiet window — do one of:

- While vigil is **idle** (refcount = 0, no `baseline.json` on disk): `sudo /usr/bin/pmset -a disablesleep 0`. The next vigil engage will then capture `0`, and the next release will go back to `0`.
- `vigil uninstall && vigil setup` — uninstall restores baseline and clears state; setup starts fresh.

If another tool (Amphetamine, an open `caffeinate -di` shell, etc.) is the *reason* `SleepDisabled=1` keeps coming back, vigil cannot fix that — the other tool will re-assert. Quit the other tool first.

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
