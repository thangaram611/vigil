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
