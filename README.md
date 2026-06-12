# vigil

Keep your Mac awake while AI coding agents are working — including with the lid closed, as much as the hardware allows.

> **Status: pre-release.** Phase 1 in progress. No version tag, no Homebrew tap, no GitHub release until the full intended feature set lands. Local-only testing.

## Why

[Amphetamine](https://apps.apple.com/us/app/amphetamine/id937984704) and similar tools are general-purpose and don't know when an AI agent is actively running. Vigil is purpose-built: it watches for the agents you actually use, holds sleep open while they're working, and releases as soon as they're done.

## What it does today

- Watches for the **CLI** processes `claude` (Claude Code), `codex`, `copilot`. The `copilot --acp` worker that [`copilot-companion`](https://github.com/thangaram611/copilot-companion) spawns per Copilot session is the same `copilot` binary and is detected via the same path; the long-lived `node` router daemon is intentionally not detected (it does no agent work itself).
- Watches for the **Codex.app** Electron host (`/Applications/Codex.app/Contents/MacOS/Codex`). Counts toward refcount only while Codex.app is producing rollout writes (idle-but-open is treated as idle). Coverage extends transitively to the OpenAI ChatGPT VS Code extension, which spawns the same kind of `codex` worker outside `/Applications/`. **Claude.app**'s Local Agent Mode is covered by the same `claude` basename match as the CLI (LAM spawns the bundled Claude Code binary, which writes to `~/.claude/projects/`). **VS Code + GitHub Copilot Chat** is covered via the VS Code/Insiders host process plus semantic hash changes in `workspaceStorage/*/chatEditingSessions/*/state.json`; mtime-only idle rewrites are ignored.
- **Activity-aware:** an agent only counts toward sleep prevention when its session storage has been touched within the last 5 minutes (`VIGIL_IDLE_AFTER_SEC=300`). An idle REPL waiting for input or a Codex.app window with no in-flight prompt is treated as idle. Probe is per-agent-type and uses `find -mmin` against `~/.claude/projects/`, `~/.codex/sessions/`, `~/.copilot/session-state/`, honoring provider home overrides (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `COPILOT_HOME`) and Vigil-specific overrides (`VIGIL_CLAUDE_HOME`, `VIGIL_CODEX_HOME`, `VIGIL_COPILOT_HOME`).
- Provides a `vigil run <cmd>` wrapper for explicit invocations (re-aliases your `claudex` cleanly). Wrappers are an explicit user opt-in and hold sleep for the wrapped command's full lifetime, regardless of session activity.
- Holds `pmset disablesleep=1` + `caffeinate -di` while at least one agent is active.
- Restores your **prior** `SleepDisabled` state on release — does not clobber other tools.
- Reconciles the live engaged state every tick: if `SleepDisabled` is flipped back
  or the `caffeinate` child exits while agents are still active, vigil reasserts.
- Cuts off automatically on thermal warnings, on low battery while unplugged.
- Runs as a per-user `launchd` LaunchAgent; auto-starts at login.

What vigil **does not** do (yet) — see [`ROADMAP.md`](./ROADMAP.md):

- Detect standalone GitHub Copilot.app beyond the CLI/VS Code surfaces above.
- Linux / Windows support (phase 5).

## Local lock guard (phase 4)

`vigil lock` runs a native helper that installs an active-session input tap and
blocks mouse/keyboard/scroll input until the configured unlock chord is pressed.
This is a local freeze guard, not the macOS login/lock screen.

- `vigil lock` — arm with config defaults (`VIGIL_LOCK_COMBO`, `VIGIL_LOCK_MAX_SECS`)
- `vigil lock --combo <combo>` — custom unlock chord
- `vigil lock --max-secs <seconds>` — watchdog timeout (`0` means no timeout with explicit CLI override)
- `vigil lock doctor` — print permission + tap readiness (`listen_event_access`, `accessibility_trusted`, `post_event_access`, `tap_create_active_session_ok`)
- `vigil lock doctor --prompt` — request OS permission prompts (if needed)
- `vigil lock --help` — full lock-mode usage

Config examples:

- `VIGIL_LOCK_COMBO` (default `ctrl+alt+shift+cmd+l`)
- `VIGIL_LOCK_MAX_SECS` (default `28800`)
- `VIGIL_LOCK_HELPER` (default `$VIGIL_INSTALL_DIR/bin/vigil-lock-helper`)

Recovery:

- `pkill -TERM vigil-lock-helper`
- `vigil lock --help` prints current command text and expected recovery flow.

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
vigil lock               # local input-freeze guard (macOS-only)
vigil lock doctor        # verify helper permissions + tap smoke test
vigil lock doctor --prompt  # request missing prompts
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
