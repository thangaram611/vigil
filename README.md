# vigil

Keep your Mac awake while AI coding agents are working — including with the lid closed, as much as the hardware allows.

> **Status: pre-release.** Phase 1 in progress. No version tag, no Homebrew tap, no GitHub release until the full intended feature set lands. Local-only testing.

## Why

[Amphetamine](https://apps.apple.com/us/app/amphetamine/id937984704) and similar tools are general-purpose and don't know when an AI agent is actively running. Vigil is purpose-built: it watches for the agents you actually use, holds sleep open while they're working, and releases as soon as they're done.

## What it does today (phase 1)

- Watches for the **CLI** processes `claude` (Claude Code), `codex`, `copilot`.
- **Activity-aware:** a CLI agent only counts toward sleep prevention when its session storage has been touched within the last 5 minutes (`VIGIL_IDLE_AFTER_SEC=300`). An idle REPL waiting for input is treated as idle. Probe is per-agent-type and uses `find -mmin` against `~/.claude/projects/`, `~/.codex/sessions/`, `~/.copilot/session-state/`.
- Provides a `vigil run <cmd>` wrapper for explicit invocations (re-aliases your `claudex` cleanly). Wrappers are an explicit user opt-in and hold sleep for the wrapped command's full lifetime, regardless of session activity.
- Holds `pmset disablesleep=1` + `caffeinate -di` while at least one agent is active.
- Restores your **prior** `SleepDisabled` state on release — does not clobber other tools.
- Reconciles the live engaged state every tick: if `SleepDisabled` is flipped back
  or the `caffeinate` child exits while agents are still active, vigil reasserts.
- Cuts off automatically on thermal warnings, on low battery while unplugged.
- Runs as a per-user `launchd` LaunchAgent; auto-starts at login.

What phase 1 **does not** do (yet) — see [`ROADMAP.md`](./ROADMAP.md):

- Detect Claude.app / Codex.app / Copilot.app desktop apps (deferred to phase 3, needs session-aware logic to avoid false positives when the app is merely open).
- Detect copilot-companion's background poller (deferred to phase 2).
- Lock the laptop with a key-combo unlock (phase 4).
- Linux / Windows support (phase 5).

## The Apple Silicon lid-closed caveat

`pmset disablesleep` writes the same `kIOPMSleepDisabledKey` flag that Apple's own private power-management SPI uses; there is **no hidden API that does more**. On Apple Silicon (M-series, macOS Ventura and later), Apple introduced a hardware-level magnet-sensor sleep that bypasses software assertions when the lid closes without an external display. In practice `pmset disablesleep` works most of the time on M-series, but the only Apple-supported lid-closed workflow is **clamshell mode** (external display + power + input). See [`docs/apple-silicon-lid-closed.md`](./docs/apple-silicon-lid-closed.md).

If you depend on overnight closed-lid runs, plug into an external display first.

## Install (manual, while pre-release)

```bash
git clone https://github.com/thangaram611/vigil.git ~/Documents/projects/personal/vigil
cd ~/Documents/projects/personal/vigil
./bin/vigil setup
./bin/vigil doctor
```

Use `./bin/vigil setup --dry-run` first if you want to preview every install
path and root-owned file without changing the system.

`vigil setup` does four things, each prompting only what's strictly needed:

1. Writes `/etc/sudoers.d/vigil` (validated with `visudo -c` first) — exact-argv `NOPASSWD` only for `pmset -a disablesleep 0` and `pmset -a disablesleep 1`.
2. Writes `/etc/newsyslog.d/vigil.conf` — rotates `~/Library/Logs/vigil/daemon.log` at 1 MiB, keeps 5 gzipped generations. Standard macOS log-rotation pattern, evaluated hourly by `com.apple.newsyslog`.
3. Creates `~/Library/Application Support/vigil/state/` (mode 0700) and `~/Library/Logs/vigil/`.
4. Installs and bootstraps `~/Library/LaunchAgents/com.thangaram.vigil.plist`.

Inspect the sudoers and newsyslog entries yourself before approving — `etc/vigil.sudoers.in` and `etc/vigil.newsyslog.in` are the templates.

## Usage

```bash
vigil status            # daemon state, refcount, pmset state, baseline, thermal, battery, power assertions
vigil status --json     # same state, machine-readable for agents/scripts
vigil log -f            # tail daemon log
vigil run claude …      # wrap a one-off command
vigil doctor            # diagnose install
vigil doctor --power    # focused pmset/caffeinate/assertion diagnostics
vigil uninstall         # remove sudoers + newsyslog, plist, restore baseline state
```

`vigil status` includes a `power assertions:` block — a parsed view of `pmset -g assertions` that marks our own caffeinate child with `← vigil` so you can tell at a glance whether vigil or some other tool is the reason your Mac isn't sleeping.

To wrap your existing `claudex` alias, edit `~/.zshrc`:

```diff
- alias claudex="claude --dangerously-skip-permissions --chrome --plugin-dir …"
+ alias claudex="vigil run claude --dangerously-skip-permissions --chrome --plugin-dir …"
```

## Safety

- Sudoers rule is **exact-argv**: `pmset -a disablesleep 0` and `pmset -a disablesleep 1` only. Nothing else.
- Daemon never invokes plain `sudo`; always `sudo -n` and aborts loudly if non-interactive sudo isn't wired up.
- Thermal and battery cut-offs are conservative by default. Override only via the `VIGIL_FORCE=1` env var on a single invocation.

## Acknowledgements

Direction-setting and design references (with verified facts traced back to source):

- [`CharlonTank/agents-sleep-preventer`](https://github.com/CharlonTank/agents-sleep-preventer) — tick loop, thermal probe, refcount discipline.
- [`hiddenest/awake`](https://github.com/hiddenest/awake) — `caffeinate` lifecycle pattern, session-aware-providers model (informs phases 2-3).
- [`iccir/Fermata`](https://github.com/iccir/Fermata) — confirmed via `Source/AppleSPI.h` and `RestlessEngine.m` that the private SPI uses the same `kIOPMSleepDisabledKey` as `pmset disablesleep`.
- [`segevfiner/keepawake-rs`](https://github.com/segevfiner/keepawake-rs) — cross-OS sleep prevention reference for phase 5.

## License

MIT — see [`LICENSE`](./LICENSE).
