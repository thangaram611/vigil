# Vigil

<p align="center">
  <img src="docs/assets/vigil-mark.svg" width="96" height="96" alt="Vigil shield mark">
</p>

<p align="center">
  <strong>Keep AI coding agents running while your Mac can lock, sleep its display, and stay inside battery and thermal guardrails.</strong>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#daily-use">Daily use</a> ·
  <a href="#manual-mode-and-menu-bar">Menu bar</a> ·
  <a href="#agent-coverage">Agent coverage</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#safety-model">Safety model</a> ·
  <a href="./ROADMAP.md">Roadmap</a>
</p>

![Vigil hero image showing a locked laptop, command blocks, and a power shield](docs/assets/vigil-hero.png)

> **Status: personal macOS tool.** Feature-complete for one machine and one workflow. No releases, no Homebrew tap, no Linux or Windows port — see [`ROADMAP.md`](./ROADMAP.md) for why. If you want a packaged app for this, use [`adrafinil`](https://github.com/kageroumado/adrafinil).

## What Vigil Is

Vigil is a small systems utility for a specific workflow: you start an AI coding agent, let the display sleep or lock the Mac, and expect the agent to keep working until it is actually idle.

Generic keep-awake tools hold the machine awake whenever you ask. Vigil watches the agent surfaces you use, counts supported working sessions, reports uncertain evidence, releases after observed work stops, and refuses to keep the machine awake through thermal pressure or a low battery.

The current implementation is a Rust workspace:

- one user-facing `vigil` binary with CLI, setup, install, daemon, status, doctor, config, wrapper, and completion commands;
- one privileged `vigil-root-helper` binary that owns the narrow `pmset` boundary on macOS;
- one native `vigil-lock-helper` binary for the optional local input-freeze guard.

There is no shell daemon and no `bin/` or `lib/` shell tree.

## At A Glance

| Area | Current behavior |
| --- | --- |
| Sleep hold | `pmset disablesleep=1` plus `caffeinate -i` while active work exists. |
| Display policy | No display-awake assertion by default; displays can sleep and macOS can lock. |
| Agent signal | Native per-session state and correlated work, with explicit unknown coverage. |
| Release policy | 30-second grace after observed automatic demand ends; safety guards and manual-off bypass it. |
| Baseline | Restores the prior `SleepDisabled` state instead of clobbering another tool. |
| Runtime privilege | Normal runtime does not execute `sudo`; power transitions go through the installed root helper. |
| Install model | `vigil setup` copies an install snapshot into `~/Library/Application Support/vigil/bin/` for launchd/TCC stability and links the dev build onto `PATH`. |

## Agent Coverage

| Surface | Local activity evidence |
| --- | --- |
| Claude Code CLI | PID/start-validated native session state, correlated child lifecycles and witnessed local commands. |
| Codex CLI and the ChatGPT/Codex desktop worker | Sessions associated with validated workers and open rollouts; turn/tool lifecycles and witnessed local commands. Auxiliary servers are excluded. |
| GitHub Copilot CLI, including `--acp` | Native session/actor events, human waits, child invocations and witnessed local commands. |
| VS Code + GitHub Copilot Chat | Existing host and semantic content-hash gate in `workspaceStorage/*/chatEditingSessions/*/state.json`. |

One session counts once even when it owns several work sources. One host can
own several sessions. `status` reports working, waiting, idle and unknown
sessions separately from detected hosts, wrappers, VS Code and uncertain demand.
A fully observed human wait releases demand only when no independent work remains.

Coverage is bounded by the provider's local evidence. Codex approval state is
not reliably saved in rollouts; an unresolved locally owned turn can remain
unknown and retain demand until its matching end or ownership invalidation.
Recent-file fallback is labeled uncertain and contributes at most once per
provider; `VIGIL_IDLE_AFTER_SEC` applies to that fallback, not verified work.
Quiet verified work has no transcript timeout. A restarted Vigil cannot recover
all detached-command ownership from a marker alone. Claude Desktop and remote
execution environments have not been verified here; app presence or a remote
viewer alone does not prove local work. The [research handoff](./future/phase-8-per-session-activity.md)
and [native fixtures](./docs/fixtures/phase-8/README.md) distinguish measured,
source-only and synthetic coverage.

Supported Claude self-paced loop gaps require background mode (`/bg`) and a
matching native worker/wake record. Pending fixed cron schedules do not hold;
executing invocations use ordinary work evidence. Use manual-on or a wrapper
when automatic coverage is insufficient.

The long-lived `node` router used by companion-style tools is intentionally excluded when its command line carries the `node_repl` token; Vigil watches the actual `copilot` worker instead.

Deferred surfaces, including standalone GitHub Copilot.app and Antigravity/Gemini CLI, are tracked in [`ROADMAP.md`](./ROADMAP.md) and [`future/phase-N-agent-surface-refresh.md`](./future/phase-N-agent-surface-refresh.md).

## Install

```bash
git clone https://github.com/thangaram611/vigil.git
cd vigil

cargo build --release
./target/release/vigil setup --dry-run
./target/release/vigil setup
vigil doctor
```

`vigil setup --dry-run` prints the planned paths and touches nothing. Add `--verbose` to print generated plist and newsyslog bodies before installation.

`vigil setup` performs these steps:

1. Creates user state and log directories.
2. Builds and copies `vigil` plus `vigil-lock-helper` into `~/Library/Application Support/vigil/bin/`.
3. Installs the root LaunchDaemon helper and its per-user IPC directory matrix.
4. Installs `/etc/newsyslog.d/vigil.conf` for daemon log rotation.
5. Installs and bootstraps the user LaunchAgent.
6. Writes the SwiftBar menu-bar plugin, if SwiftBar has a plugin folder configured.

After setup, `vigil` is available on `PATH` through `~/.local/bin/vigil`, a symlink to the dev build. The LaunchAgent still runs the copied install snapshot, which is required because macOS TCC grants are tied to the executable path and signature. Override the link directory with `VIGIL_BIN_LINK_DIR`.

Useful install commands:

```bash
vigil setup --dry-run --verbose
vigil setup --yes
vigil reload
vigil uninstall
```

`vigil uninstall` removes the helper, newsyslog entry, LaunchAgent, menu-bar plugin, install state, and runtime state. Logs under `~/Library/Logs/vigil` are preserved, and the `~/.local/bin/vigil` symlink is left in place so setup can reinstall from the checkout.

## Daily Use

```bash
vigil status
vigil status --verbose
vigil status --json
vigil refresh
vigil doctor
vigil doctor --power
vigil log
vigil log -f
```

Wrap a one-off command when you want an explicit hold for the full child lifetime:

```bash
vigil run claude
vigil run codex
vigil run copilot
```

To wrap an existing alias:

```diff
- alias claudex="claude --dangerously-skip-permissions --chrome --plugin-dir ..."
+ alias claudex="vigil run claude --dangerously-skip-permissions --chrome --plugin-dir ..."
```

`vigil run` keeps the child in the foreground, removes its wrapper pidfile on exit, and propagates the child exit code. A signal-terminated child returns `128 + signal`; command-not-found returns `127`.

![Vigil status terminal preview](docs/assets/vigil-status.svg)

## Manual Mode And Menu Bar

Agent detection decides when to hold. `vigil mode` overrides it:

```bash
vigil mode          # print the current mode
vigil mode on       # hold now, agents or not
vigil mode off      # never hold, even with active agents
vigil mode auto     # back to agent detection (default)
```

The mode persists across restarts and takes effect on the daemon's next tick. Thermal pressure and the battery floor still release the hold in every mode — `on` outranks detection, not the safety guards.

The menu-bar item is a [SwiftBar](https://github.com/swiftbar/SwiftBar) plugin: an icon for the current state, the three modes, and **Refresh now**.

```bash
brew install --cask swiftbar
open -a SwiftBar        # pick a plugin folder on first launch
vigil setup             # writes vigil.5s.sh into that folder
```

The plugin is a two-line shim around `vigil menubar`, which reads only the daemon's tick file and the mode file, so its normal refresh does not scan provider transcripts.

| Icon | Meaning |
| --- | --- |
| cup, filled | Holding sleep prevention |
| cup, outline | Idle — the Mac may sleep |
| thermometer / battery | Hold released by a safety guard |
| warning triangle | Daemon not running or not ticking |
| `ON` / `OFF` beside the icon | A manual mode is set |

## Command Surface

| Command | Use |
| --- | --- |
| `vigil setup` | Install the user LaunchAgent, root helper, log rotation, install snapshot, and PATH link. |
| `vigil start` / `vigil stop` | Bootstrap or boot out the LaunchAgent. |
| `vigil reload` | Rebuild, re-sync install binaries, heal the PATH link, and restart launchd. |
| `vigil status` | Print fresh activity, cached daemon policy and power evidence. JSON schema version 2. |
| `vigil refresh` | Request and acknowledge one daemon evaluation; consume release grace when automatic demand is zero. |
| `vigil doctor` | Diagnose installation readiness. |
| `vigil doctor --power` | Focus on `pmset`, helper, `caffeinate`, and assertion state. |
| `vigil run <cmd>` | Hold sleep prevention for an explicit foreground command. |
| `vigil mode [auto\|on\|off]` | Show or set the manual override. |
| `vigil menubar` | Print the SwiftBar menu. Called by the plugin, not by hand. |
| `vigil lock` | Arm the local input-freeze guard. |
| `vigil lock setup` | Capture or set the ordered unlock chord. |
| `vigil lock doctor` | Check Input Monitoring, Accessibility, and event-tap readiness. |
| `vigil config` / `vigil config --json` | Print the fully resolved configuration. |
| `vigil completions <shell>` | Emit shell completions. |
| `vigil uninstall` | Remove install components and restore the baseline. |

`vigil refresh` has a single 2-second response deadline and never starts an absent
daemon. It reports the matching post-action evaluation and hold reason. Real work
or uncertain demand can keep the hold engaged. Timeout means the outcome is
unknown. Old tick files expose legacy policy with unknown session counts and
cannot acknowledge refresh; run `vigil reload` after updating the source.

Immediately after `start`, `reload`, or login, `vigil status` may briefly report `pending first scan` or `expected hold: pending` while launchd has started the daemon but the first tick has not completed.

## Configuration

Vigil reads strict TOML from `$VIGIL_CONFIG_FILE` or `~/.config/vigil/vigil.conf`. Shell-style `export`, `$VAR`, and `KEY=value` files are rejected clearly.

Every field can also be set through a `VIGIL_<FIELD>` environment variable, which overrides TOML. Inspect the final values with:

```bash
vigil config
vigil config --json
```

Common knobs:

| Setting | Default | Meaning |
| --- | --- | --- |
| `VIGIL_IDLE_AFTER_SEC` | `300` | Recent-file fallback window, rounded up to whole minutes; also the existing VS Code hash-gate window. |
| `VIGIL_TICK_SECS` | `5` | Daemon tick interval. |
| `VIGIL_BATTERY_FLOOR_PCT` | `20` | Battery cutoff while unplugged. |
| `VIGIL_START_WAIT_SECS` | `6` | First-scan and lock pre-arm wait bound. |
| `VIGIL_LOCK_COMBO` | `ctrl+alt+shift+cmd+l` | Default local lock unlock chord. |
| `VIGIL_LOCK_MAX_SECS` | `28800` | Default local lock watchdog timeout. |
| `VIGIL_INSTALL_DIR` | `~/Library/Application Support/vigil` | Install snapshot, helper queue, and runtime tree. |
| `VIGIL_BIN_LINK_DIR` | `~/.local/bin` | Directory where `setup` links the dev build onto `PATH`. |

Provider homes honor the native provider variables (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `COPILOT_HOME`) and Vigil-specific overrides (`VIGIL_CLAUDE_HOME`, `VIGIL_CODEX_HOME`, `VIGIL_COPILOT_HOME`).

## Local Lock Guard

`vigil lock` freezes local mouse, keyboard, and scroll input until an ordered unlock chord is pressed. Display sleep and the native macOS Lock Screen are still allowed.

This is a local input-freeze guard, not a replacement for the macOS login screen. If macOS locks while the guard is armed, login input passes through. After login, the Vigil chord is still required before desktop input is released.

```bash
vigil lock
vigil lock --combo ctrl+alt+l
vigil lock --max-secs 0
vigil lock --countdown 0
vigil lock setup
vigil lock setup --combo ctrl+alt+shift+l --max-secs 3600
vigil lock doctor
vigil lock doctor --prompt
```

The unlock chord is ordered: `ctrl+l+alt` is not the same as `ctrl+alt+l`. It must contain at least three keys and may mix modifiers and regular keys. The overlay shows a generic "Press your unlock chord to continue" hint and never displays the literal combo.

Required macOS grants: **Input Monitoring** and **Accessibility**. Run `vigil lock doctor` before arming.

Recovery command:

```bash
pkill -TERM vigil-lock-helper
```

For the event-tap boundary and locked-use details, read [`docs/macos-lock-and-locked-use.md`](./docs/macos-lock-and-locked-use.md).

## How It Works

![Vigil architecture diagram](docs/assets/vigil-architecture.svg)

Two launchd jobs drive macOS runtime behavior:

- **LaunchAgent `com.thangaram.vigil`** runs `<install_dir>/bin/vigil daemon` as the user. The daemon ticks every `VIGIL_TICK_SECS`, collects process identities once, reduces per-session evidence, removes invalid pidfiles, evaluates thermal and battery cutoffs, and decides whether a hold should be engaged.
- **LaunchDaemon `com.thangaram.vigil.helper`** runs `vigil-root-helper --serve` as root. It accepts only `engage`, `release`, and `status` through a per-uid filesystem request/response queue. The helper validates request files by file descriptor and runs only fixed `/usr/bin/pmset -a disablesleep 0|1` argv.

The daemon also owns a `caffeinate -i` child while active work exists. It never uses a display-awake assertion by default, so macOS can still turn displays off and show the Lock Screen.

`status`, `doctor` and `debug` share read-only activity collection. They read
provider/state files and launchd state without writing, GC or helper requests.
Helper installation and IPC permissions are checked; responsiveness is explicitly
**unchecked**, a warning rather than a failed health probe. `doctor` and
`doctor --power` exit 0 when required static checks pass; concrete installation
faults remain errors. Status JSON separates fresh `activity` from `cached_policy`,
including cache age and daemon identity, so an old policy is never a fresh session count.

Deep dives:

- [`docs/architecture.md`](./docs/architecture.md) — daemon, helper, IPC, refcount, baseline, status/doctor internals.
- [`docs/apple-silicon-lid-closed.md`](./docs/apple-silicon-lid-closed.md) — why closed-lid behavior on Apple Silicon is best effort.
- [`docs/testing.md`](./docs/testing.md) — test conventions, especially bounded CPU hog safety.

## Safety Model

Vigil is intentionally conservative:

- Normal runtime does not execute `sudo`.
- The daemon never builds a privileged command from request content.
- The root helper accepts only `engage`, `release`, and `status`.
- Request and response files are validated with `O_NOFOLLOW` plus fd-based metadata checks.
- The helper runs a fixed `pmset` argv with a cleared environment and pinned `PATH`.
- Thermal pressure releases the hold.
- Low battery while unplugged releases the hold.
- A release with the lid closed is followed by `pmset sleepnow`. macOS only sleeps on the lid *closing*, so without this a release would leave a closed Mac running.
- A lid closing under a hold is followed by `pmset displaysleepnow`, so the Mac locks. Without it a held, closed Mac opens straight to the desktop.
- Baseline restoration avoids clobbering another tool's `SleepDisabled` state.
- Admin paths refuse test mode (`VIGIL_TEST_NO_ADMIN`) and refuse environment-overridden privileged install paths.

The tradeoff is explicit: Vigil avoids repeated runtime sudo prompts by owning a persistent privileged component. Treat the helper queue as a real privilege boundary.

## Apple Silicon Lid-Closed Caveat

Vigil uses the strongest public macOS path available for this workflow: `pmset disablesleep` plus `caffeinate -i`. Measured on an M5 Pro running macOS 26.7, on battery with no external display, it held for a 20-minute closed-lid run; the numbers are in [`docs/apple-silicon-lid-closed.md`](./docs/apple-silicon-lid-closed.md).

On Apple Silicon, macOS can still enter hardware-level magnet-sensor sleep when the lid closes without an external display. In practice the `pmset disablesleep` path works much of the time, but the only Apple-supported closed-lid workflow is clamshell mode: external display, power, and input.

If you depend on overnight closed-lid runs, use clamshell mode. See [`docs/apple-silicon-lid-closed.md`](./docs/apple-silicon-lid-closed.md).

## Roadmap

Vigil is done growing sideways. Linux, Windows, a native tray app, and public releases were dropped on 2026-09-20; session activity now has explicit coverage limits, and new agent surfaces remain subject to local verification. See [`ROADMAP.md`](./ROADMAP.md).

## Development

Default verification for real changes:

```bash
cargo fmt --check
cargo build
cargo clippy --workspace --all-targets -- -D warnings
cargo test
```

Use the helper test seam only when explicitly testing the helper subprocess boundary:

```bash
cargo test --features helper-test-seam
```

Tests that spawn CPU-burning or long-lived children must use `BoundedCpuHog` or an equivalent self-bounded process-group guard. Do not hand-roll unbounded `while :; do :; done` children. See [`docs/testing.md`](./docs/testing.md).

## Acknowledgements

Direction-setting and design references:

- [`CharlonTank/agents-sleep-preventer`](https://github.com/CharlonTank/agents-sleep-preventer) — tick loop, thermal probe, refcount discipline.
- [`hiddenest/awake`](https://github.com/hiddenest/awake) — `caffeinate` lifecycle pattern, session-aware providers model.
- [`iccir/Fermata`](https://github.com/iccir/Fermata) — confirmed via `Source/AppleSPI.h` and `RestlessEngine.m` that the private SPI uses the same `kIOPMSleepDisabledKey` as `pmset disablesleep`.

## License

MIT — see [`LICENSE`](./LICENSE).
