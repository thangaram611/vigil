# vigil

Keep AI coding agents running while your Mac is allowed to lock and turn its displays off.

> **Status: pre-release.** Phase 1 in progress. No version tag, no Homebrew tap, no GitHub release until the full intended feature set lands. Local-only testing.

## Why

[Amphetamine](https://apps.apple.com/us/app/amphetamine/id937984704) and similar tools are general-purpose and don't know when an AI agent is actively running. Vigil is purpose-built: it watches for the agents you actually use, holds sleep open while they're working, and releases as soon as they're done.

## What it does today

- Watches for the **CLI** processes `claude` (Claude Code), `codex`, `copilot`. The `copilot --acp` worker that [`copilot-companion`](https://github.com/thangaram611/copilot-companion) spawns per Copilot session is the same `copilot` binary and is detected via the same path; the long-lived `node` router daemon is intentionally not detected (it does no agent work itself).
- Watches for the **Codex.app** Electron host (`/Applications/Codex.app/Contents/MacOS/Codex`). Counts toward refcount only while Codex.app is producing rollout writes (idle-but-open is treated as idle). Coverage extends transitively to the OpenAI ChatGPT VS Code extension, which spawns the same kind of `codex` worker outside `/Applications/`. **Claude.app**'s Local Agent Mode is covered by the same `claude` basename match as the CLI (LAM spawns the bundled Claude Code binary, which writes to `~/.claude/projects/`). **VS Code + GitHub Copilot Chat** is covered via the VS Code/Insiders host process plus semantic hash changes in `workspaceStorage/*/chatEditingSessions/*/state.json`; mtime-only idle rewrites are ignored.
- **Activity-aware:** an agent only counts toward sleep prevention when its session storage has been touched within the last 5 minutes (`VIGIL_IDLE_AFTER_SEC=300`). An idle REPL waiting for input or a Codex.app window with no in-flight prompt is treated as idle. Probe is per-agent-type and uses `find -mmin` against `~/.claude/projects/`, `~/.codex/sessions/`, `~/.copilot/session-state/`, honoring provider home overrides (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `COPILOT_HOME`) and Vigil-specific overrides (`VIGIL_CLAUDE_HOME`, `VIGIL_CODEX_HOME`, `VIGIL_COPILOT_HOME`).
- Provides a `vigil run <cmd>` wrapper for explicit invocations (re-aliases your `claudex` cleanly). Wrappers are an explicit user opt-in and hold sleep for the wrapped command's full lifetime, regardless of session activity.
- Holds `pmset disablesleep=1` plus `caffeinate -i` while at least one agent is active. This is the best-effort closed-lid/system-sleep path, but it still lets macOS lock naturally and lets displays sleep.
- Restores your **prior** `SleepDisabled` state on release — does not clobber other tools.
- Reconciles the live engaged state every tick: if `SleepDisabled` is flipped back or the `caffeinate` child exits while agents are still active, vigil reasserts.
- Cuts off automatically on thermal warnings, on low battery while unplugged.
- Runs as a per-user `launchd` LaunchAgent; auto-starts at login.

What vigil **does not** do (yet) — see [`ROADMAP.md`](./ROADMAP.md):

- Detect standalone GitHub Copilot.app beyond the CLI/VS Code surfaces above.
- Linux / Windows support (phase 5).

## Local lock guard (phase 4)

`vigil lock` runs a native helper under `caffeinate -i`, installs an
active-session input tap, and blocks mouse/keyboard/scroll input until the
configured unlock chord is pressed. Display sleep and the native macOS Lock
Screen are still allowed. If macOS locks while the guard is armed, login input
passes through; after login, the Vigil combo is still required before desktop
input is released. This is a local freeze guard, not the macOS login/lock
screen. See
[`docs/macos-lock-and-locked-use.md`](./docs/macos-lock-and-locked-use.md) for
the event-tap boundary and why Codex-style locked use is a separate feature.

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

Vigil's normal hold uses the best-effort closed-lid path. `pmset disablesleep`
writes the same `kIOPMSleepDisabledKey` flag that
Apple's own private power-management SPI uses; there is **no hidden API that
does more**. On Apple Silicon (M-series, macOS Ventura and later), Apple
introduced a hardware-level magnet-sensor sleep that bypasses software
assertions when the lid closes without an external display. In practice
`pmset disablesleep` works most of the time on M-series, but the only
Apple-supported lid-closed workflow is **clamshell mode** (external display +
power + input). See [`docs/apple-silicon-lid-closed.md`](./docs/apple-silicon-lid-closed.md).

If you depend on overnight closed-lid runs, plug into an external display first.

## Install (manual, while pre-release)

```bash
git clone https://github.com/thangaram611/vigil.git ~/Documents/projects/personal/vigil
cd ~/Documents/projects/personal/vigil
./bin/vigil setup
./bin/vigil doctor
```

Use `./bin/vigil setup --dry-run` first if you want to preview the install plan
without changing the system. Add `--verbose` to print the generated plist and
newsyslog file bodies.

`vigil setup` does four things, each prompting only what's strictly needed:

1. Installs a root LaunchDaemon helper at `/Library/LaunchDaemons/com.thangaram.vigil.helper.plist`. The helper owns the privileged `pmset -a disablesleep 0|1` transitions and accepts only `engage`, `release`, and `status` requests through its filesystem queue.
2. Writes `/etc/newsyslog.d/vigil.conf` — rotates `~/Library/Logs/vigil/daemon.log` at 1 MiB, keeps 5 gzipped generations. Standard macOS log-rotation pattern, evaluated hourly by `com.apple.newsyslog`.
3. Creates `~/Library/Application Support/vigil/state/` (mode 0700) and `~/Library/Logs/vigil/`.
4. Installs and bootstraps `~/Library/LaunchAgents/com.thangaram.vigil.plist`.

Inspect the LaunchDaemon and newsyslog entries yourself before approving — `etc/com.thangaram.vigil.helper.plist.in` and `etc/vigil.newsyslog.in` are the templates.

## Usage

```bash
vigil status            # concise service, scan readiness, activity, and power state
vigil status --verbose  # include provider paths and raw power assertion rows
vigil status --json     # machine-readable daemon and power state
vigil log -f            # tail daemon log
vigil run claude …      # wrap a one-off command
vigil lock               # local input-freeze guard (macOS-only)
vigil lock doctor        # verify helper permissions + tap smoke test
vigil lock doctor --prompt  # request missing prompts
vigil doctor            # concise install diagnostics and next action
vigil doctor --verbose  # include install paths and provider roots
vigil doctor --power    # focused pmset/caffeinate/assertion diagnostics
vigil uninstall         # remove helper + newsyslog, plist, restore baseline state
```

`vigil status` reports the daemon's latest scan age. Immediately after
`start`, `reload`, or login, it may show a bounded `pending first scan` /
`expected hold: pending` state while launchd has started the daemon but the
first power transition has not completed. It also summarizes active power
assertions. Use `vigil status --verbose` to print the parsed `pmset -g
assertions` rows; Vigil marks its own `caffeinate` child with `← vigil` so you
can tell whether Vigil or another tool is keeping the Mac awake.

Power policy:

- Vigil always uses the best-effort closed-lid/system-sleep path on macOS:
  `pmset disablesleep=1` plus `caffeinate -i`.
- Vigil never uses a display-awake assertion by default, so display sleep and
  the native macOS Lock Screen remain allowed.

`vigil doctor` and `vigil doctor --power` are readiness checks. Exit `0` means
the required checks passed. Exit `1` means Vigil is not ready for this user:
the output distinguishes `state: not installed` from `state: needs repair`.
Warnings, such as the optional lock helper being absent before using
`vigil lock`, do not fail the command.

To wrap your existing `claudex` alias, edit `~/.zshrc`:

```diff
- alias claudex="claude --dangerously-skip-permissions --chrome --plugin-dir …"
+ alias claudex="vigil run claude --dangerously-skip-permissions --chrome --plugin-dir …"
```

## Safety

- Normal runtime does not execute `sudo`. The user LaunchAgent requests `engage`, `release`, or `status` from the installed root helper.
- The root helper has a narrow command surface and runs only fixed `/usr/bin/pmset -a disablesleep 0|1` argv. Request files are validated for action, type, owner, and permissions before the helper acts.
- `vigil setup` and `vigil uninstall` are the only admin paths. They refuse test mode and refuse environment-overridden privileged install paths before running any `sudo` command.
- `./tests/run.sh` prepends a failing `sudo` guard and sets `VIGIL_TEST_NO_ADMIN=1`, so repeated development test runs cannot prompt for admin access.
- Tradeoff: this removes repeated runtime sudo, but Vigil now owns a persistent privileged component. Treat the helper boundary as a real privilege boundary.
- Thermal and battery cut-offs are conservative by default. Override only via the `VIGIL_FORCE=1` env var on a single invocation.

## Acknowledgements

Direction-setting and design references (with verified facts traced back to source):

- [`CharlonTank/agents-sleep-preventer`](https://github.com/CharlonTank/agents-sleep-preventer) — tick loop, thermal probe, refcount discipline.
- [`hiddenest/awake`](https://github.com/hiddenest/awake) — `caffeinate` lifecycle pattern, session-aware-providers model (informs phases 2-3).
- [`iccir/Fermata`](https://github.com/iccir/Fermata) — confirmed via `Source/AppleSPI.h` and `RestlessEngine.m` that the private SPI uses the same `kIOPMSleepDisabledKey` as `pmset disablesleep`.
- [`segevfiner/keepawake-rs`](https://github.com/segevfiner/keepawake-rs) — cross-OS sleep prevention reference for phase 5.

## License

MIT — see [`LICENSE`](./LICENSE).
