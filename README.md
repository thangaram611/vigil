# vigil

Keep AI coding agents running while your Mac is allowed to lock and turn its displays off.

> **Status: pre-release.** macOS is feature-complete through the Rust rewrite (phases 1–5.7 plus the UX overhaul are shipped); remaining work is Linux / Windows — see [`ROADMAP.md`](./ROADMAP.md). No version tag, no Homebrew tap, no GitHub release yet. Local-only testing.
> **Next track:** Linux first, then Windows. Antigravity/Gemini CLI detection is documented but deferred until those tools can be fresh-installed and verified locally.

## Why

[Amphetamine](https://apps.apple.com/us/app/amphetamine/id937984704) and similar tools are general-purpose and don't know when an AI agent is actively running. Vigil is purpose-built: it watches for the agents you actually use, holds sleep open while they're working, and releases as soon as they're done.

Vigil is a single self-contained Rust binary. The resident daemon, every CLI subcommand, and the install logic all live in one `vigil` executable; a separate root binary (`vigil-root-helper`) owns the privileged power transitions, and a separate native binary (`vigil-lock-helper`) owns the input-freeze guard. There is no shell daemon and no `lib/*.sh` — all bash was removed in the 5.7 cutover.

## What it does today

- Watches for the **CLI** processes `claude` (Claude Code), `codex`, `copilot`, matched by basename. The `copilot --acp` worker that [`copilot-companion`](https://github.com/thangaram611/copilot-companion) spawns per Copilot session is the same `copilot` binary and is detected via the same path; the long-lived `node` router daemon is intentionally not detected (its command line carries a `node_repl` token that vigil excludes).
- Watches for the **Codex.app** Electron host (`.../Codex.app/Contents/MacOS/Codex`), carved out before the `/Applications/` exclusion so a Codex.app under `/Applications/` still matches. It counts toward refcount only while Codex.app is producing rollout writes (idle-but-open is treated as idle). Coverage extends transitively to the OpenAI ChatGPT VS Code extension, which spawns the same kind of `codex` worker outside `/Applications/`. **Claude.app**'s Local Agent Mode is covered by the same `claude` basename match as the CLI (LAM spawns the bundled Claude Code binary, which writes to `~/.claude/projects/`). **VS Code + GitHub Copilot Chat** is covered via the VS Code/Insiders host process plus semantic content-hash changes in `workspaceStorage/*/chatEditingSessions/*/state.json`; mtime-only idle rewrites are ignored.
- **Activity-aware:** an agent only counts toward sleep prevention when its session storage has been touched within the last 5 minutes (`VIGIL_IDLE_AFTER_SEC=300`). An idle REPL waiting for input or a Codex.app window with no in-flight prompt is treated as idle. The probe is per-agent-type and uses `find -mmin` whole-minute semantics against `~/.claude/projects/`, `~/.codex/sessions/`, `~/.copilot/session-state/`, honoring provider home overrides (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `COPILOT_HOME`) and Vigil-specific overrides (`VIGIL_CLAUDE_HOME`, `VIGIL_CODEX_HOME`, `VIGIL_COPILOT_HOME`).
- Provides a `vigil run <cmd>` wrapper for explicit invocations (re-aliases your `claudex` cleanly). Wrappers are an explicit user opt-in and hold sleep for the wrapped command's full lifetime, regardless of session activity.
- Holds `pmset disablesleep=1` plus `caffeinate -i` while at least one agent is active. This is the best-effort closed-lid/system-sleep path, but it still lets macOS lock naturally and lets displays sleep.
- Restores your **prior** `SleepDisabled` state on release — does not clobber other tools.
- Reconciles the live engaged state every tick: if `SleepDisabled` is flipped back or the `caffeinate` child exits while agents are still active, vigil reasserts.
- Cuts off automatically on thermal warnings, and on low battery while unplugged.
- Runs as a per-user `launchd` LaunchAgent; auto-starts at login.

What vigil **does not** do (yet) — see [`ROADMAP.md`](./ROADMAP.md):

- Detect standalone GitHub Copilot.app beyond the CLI/VS Code surfaces above.
- Detect Antigravity CLI / legacy Gemini CLI.
- Linux / Windows support.

## Architecture

Two launchd jobs run two modes of the same Rust workspace, and they talk only through a per-uid file-based IPC queue:

- **LaunchAgent `com.thangaram.vigil`** (per-user) runs the resident daemon as `<install_dir>/bin/vigil daemon` — the install snapshot of the `vigil` binary plus a hidden `daemon` subcommand. It ticks every 5s (`VIGIL_TICK_SECS`), detects agents, GCs stale refcount pidfiles, evaluates activity + thermal + battery cutoffs, and decides whether to hold.
- **LaunchDaemon `com.thangaram.vigil.helper`** (root) runs `vigil-root-helper --serve` as a long-lived poll loop. It is the only thing that mutates `SleepDisabled`, via a fixed `/usr/bin/pmset -a disablesleep 0|1` argv. It accepts exactly three actions — `engage`, `release`, `status` — through a filesystem request/response queue, validating every request by file descriptor (owner, mode, regular-file, single-link) to close TOCTOU and symlink-redirect.

The install snapshot under `~/Library/Application Support/vigil` (not a binary in `~/Documents`) is mandatory for TCC: macOS ties the granted automation/full-disk authorization to a specific binary path and signature, so the LaunchAgent must exec the copied binary rather than the repo build.

## Local lock guard

`vigil lock` runs a native helper (`vigil-lock-helper`) under the daemon's sleep hold, installs a CoreGraphics HID event tap, and blocks mouse/keyboard/scroll input until the configured unlock chord is pressed. Display sleep and the native macOS Lock Screen are still allowed. If macOS locks while the guard is armed, login input passes through; after login, the Vigil combo is still required before desktop input is released. This is a local freeze guard, not the macOS login/lock screen. See [`docs/macos-lock-and-locked-use.md`](./docs/macos-lock-and-locked-use.md) for the event-tap boundary and why Codex-style locked use is a separate feature.

The unlock chord is an **ordered** sequence (press order is significant: `ctrl+l+alt` is not `ctrl+alt+l`), at least 3 keys, any mix of modifiers and regular keys. While the guard is armed a fully-opaque centered overlay covers the screen; it shows a generic "Press your unlock chord to continue" hint and never displays the literal combo, so an onlooker can't read it off the screen.

- `vigil lock` — arm with config defaults (`VIGIL_LOCK_COMBO`, `VIGIL_LOCK_MAX_SECS`)
- `vigil lock --combo <combo>` — custom unlock chord for this run
- `vigil lock --max-secs <seconds>` — watchdog timeout (`0` means no timeout, but only when passed explicitly on the CLI)
- `vigil lock --countdown <seconds>` — pre-arm 3-2-1 countdown (default `3`; `0` arms immediately). Does not affect the power-hold wait.
- `vigil lock setup` — capture a new chord by pressing it (interactive), or `vigil lock setup --combo <combo> [--max-secs <seconds>]` to write directly; persists `lock_combo`/`lock_max_secs` to `vigil.conf`
- `vigil lock doctor` — print permission + tap readiness (`listen_event_access`, `accessibility_trusted`, `tap_create_active_hid_ok`; `post_event_access` is informational only)
- `vigil lock doctor --prompt` — request OS permission prompts (if needed)
- `vigil lock --help` — full lock-mode usage

Required macOS grants: **Input Monitoring** and **Accessibility** (System Settings > Privacy & Security). Run `vigil lock doctor` to verify them before arming.

Config examples:

- `VIGIL_LOCK_COMBO` (default `ctrl+alt+shift+cmd+l`)
- `VIGIL_LOCK_MAX_SECS` (default `28800`)
- `VIGIL_LOCK_HELPER` (default `$VIGIL_INSTALL_DIR/bin/vigil-lock-helper`)

Recovery:

- `pkill -TERM vigil-lock-helper`
- `vigil lock --help` prints current command text and the expected recovery flow.

## The Apple Silicon lid-closed caveat

Vigil's normal hold uses the best-effort closed-lid path. `pmset disablesleep` writes the same `kIOPMSleepDisabledKey` flag that Apple's own private power-management SPI uses; there is **no hidden API that does more**. On Apple Silicon (M-series, macOS Ventura and later), Apple introduced a hardware-level magnet-sensor sleep that bypasses software assertions when the lid closes without an external display. In practice `pmset disablesleep` works most of the time on M-series, but the only Apple-supported lid-closed workflow is **clamshell mode** (external display + power + input). See [`docs/apple-silicon-lid-closed.md`](./docs/apple-silicon-lid-closed.md).

If you depend on overnight closed-lid runs, plug into an external display first.

## Install (manual, while pre-release)

Vigil is a Rust workspace — there is no `bin/` directory and no shell entrypoint. Build the workspace, then run `setup`, which copies the install binaries into place and symlinks the dev build onto your `PATH`.

```bash
git clone https://github.com/thangaram611/vigil.git ~/Documents/projects/personal/vigil
cd ~/Documents/projects/personal/vigil
cargo build --release
./target/release/vigil setup     # builds + copies binaries, symlinks ~/.local/bin/vigil onto PATH
vigil doctor                     # now resolvable on PATH (ensure ~/.local/bin is on $PATH)
```

Before running `setup`, the binary is at `./target/release/vigil`. After `setup`, `vigil` is on your `PATH` via `~/.local/bin/vigil` (a symlink pointing back at the repo's `target/release/vigil`, so `vigil reload`/`vigil setup` can rebuild from the checkout). The link directory is overridable with `VIGIL_BIN_LINK_DIR`. A real (non-symlink) file already at `~/.local/bin/vigil` is never clobbered, and a PATH hint is printed if `~/.local/bin` isn't on `$PATH`.

Use `./target/release/vigil setup --dry-run` first if you want to preview the install plan without changing the system — it touches nothing and prints the path table. Add `--verbose` to also print the generated plist and newsyslog file bodies. You can also pass `--yes`/`--non-interactive` to skip the confirm prompt.

`vigil setup` does the following, each prompting only what's strictly needed:

1. Creates `~/Library/Application Support/vigil/state/` (mode 0700) and `~/Library/Logs/vigil/`.
2. Builds `vigil` + `vigil-lock-helper` (release) and copies them into `~/Library/Application Support/vigil/bin/` — the TCC-safe install snapshot — then symlinks the dev build onto your `PATH`.
3. Installs the root LaunchDaemon helper at `/Library/LaunchDaemons/com.thangaram.vigil.helper.plist`, sets up the per-uid IPC directory ownership matrix, and copies `vigil-root-helper` into `/Library/Application Support/vigil/bin/`. The helper owns the privileged `pmset -a disablesleep 0|1` transitions and accepts only `engage`, `release`, and `status` requests through its filesystem queue. (Any legacy `/etc/sudoers.d/vigil` is removed.)
4. Writes `/etc/newsyslog.d/vigil.conf` — rotates `~/Library/Logs/vigil/daemon.log` at 1 MiB, keeps 5 gzipped generations. Standard macOS log-rotation pattern.
5. Installs and bootstraps `~/Library/LaunchAgents/com.thangaram.vigil.plist`, whose `ProgramArguments` point at the installed snapshot (`<install>/bin/vigil daemon`).

Inspect the LaunchDaemon and newsyslog entries yourself before approving — `etc/com.thangaram.vigil.helper.plist.in` and `etc/vigil.newsyslog.in` are the templates.

## Usage

```bash
vigil status            # concise service, scan readiness, activity, and power state
vigil status --verbose  # include provider paths and raw power assertion rows
vigil status --json     # machine-readable daemon and power state
vigil log               # cat the daemon log (last 2000 lines)
vigil log -f            # tail -f the daemon log
vigil run claude …      # wrap a one-off command; holds sleep for its lifetime
vigil lock              # local input-freeze guard (macOS-only)
vigil lock setup        # capture/set the unlock chord and timeout
vigil lock doctor       # verify helper permissions + tap readiness
vigil lock doctor --prompt  # request missing permission prompts
vigil doctor            # concise install diagnostics and next action
vigil doctor --verbose  # include install paths and provider roots
vigil doctor --power    # focused pmset/caffeinate/assertion diagnostics
vigil config            # show the fully-resolved configuration
vigil config --json     # machine-readable resolved config
vigil completions <shell>   # print a shell completion script to stdout
vigil start             # bootstrap the LaunchAgent
vigil stop              # boot out the LaunchAgent
vigil reload            # rebuild + re-sync install binaries, heal PATH symlink, restart launchd
vigil uninstall         # remove helper + newsyslog + plist, restore baseline, wipe state
```

`vigil status` reports the daemon's latest scan age. Immediately after `start`, `reload`, or login, it may show a bounded `pending first scan` / `expected hold: pending` state while launchd has started the daemon but the first power transition has not completed. It also summarizes active power assertions. Use `vigil status --verbose` to print the parsed `pmset -g assertions` rows; Vigil marks its own `caffeinate` child with `← vigil` so you can tell whether Vigil or another tool is keeping the Mac awake. Both `status` and `doctor` cap their helper-liveness probe at 2s so a dead or slow helper can't block the paint.

Power policy:

- Vigil always uses the best-effort closed-lid/system-sleep path on macOS: `pmset disablesleep=1` plus `caffeinate -i`.
- Vigil never uses a display-awake assertion by default, so display sleep and the native macOS Lock Screen remain allowed.

`vigil doctor` and `vigil doctor --power` are readiness checks. Exit `0` means the required checks passed. Exit `1` means Vigil is not ready for this user: the output distinguishes `state: not installed` from `state: needs repair`. Warnings, such as the optional lock helper being absent before using `vigil lock`, do not fail the command.

`vigil uninstall` stops the LaunchAgent, restores your prior `SleepDisabled` baseline, removes the helper plist + newsyslog + LaunchAgent, and wipes `~/Library/Application Support/vigil`. Logs under `~/Library/Logs/vigil` are preserved, and the `~/.local/bin/vigil` PATH symlink is left in place (its target survives uninstall, so `vigil setup` can reinstall).

To wrap your existing `claudex` alias, edit `~/.zshrc`:

```diff
- alias claudex="claude --dangerously-skip-permissions --chrome --plugin-dir …"
+ alias claudex="vigil run claude --dangerously-skip-permissions --chrome --plugin-dir …"
```

`vigil run` runs the child as a foreground subprocess and propagates its exit code (signal-terminated → `128 + signal`; command-not-found → `127`).

## Configuration

The config file is `$VIGIL_CONFIG_FILE` or `~/.config/vigil/vigil.conf`, parsed as strict TOML (shell-style `export`/`$VAR`/`KEY=value` files are rejected with a clear error). Any field is also settable as a `VIGIL_<FIELD>` environment variable, which overrides the TOML value. `vigil config` prints the fully-resolved configuration; `vigil config --json` emits a sorted machine-readable form. Commonly-tuned knobs:

- `VIGIL_IDLE_AFTER_SEC` (default `300`) — activity idle window
- `VIGIL_TICK_SECS` (default `5`) — daemon tick interval
- `VIGIL_BATTERY_FLOOR_PCT` (default `20`) — battery cutoff while unplugged
- `VIGIL_START_WAIT_SECS` (default `6`) — bounds the first-scan wait after start/reload/setup and the lock pre-arm power-hold wait
- `VIGIL_LOCK_COMBO` / `VIGIL_LOCK_MAX_SECS` / `VIGIL_LOCK_HELPER` — lock guard defaults
- `VIGIL_INSTALL_DIR` (default `~/Library/Application Support/vigil`), `VIGIL_BIN_LINK_DIR` (default `~/.local/bin`)

## Safety

- Normal runtime does not execute `sudo`. The user LaunchAgent requests `engage`, `release`, or `status` from the installed root helper through a filesystem queue.
- The root helper has a narrow command surface and runs only fixed `/usr/bin/pmset -a disablesleep 0|1` argv (with `env_clear()` and a pinned `PATH`); the argv is never built from request content. Request files are validated by file descriptor for action, type, owner, and permissions before the helper acts.
- `vigil setup` and `vigil uninstall` (and `reload`'s launchctl bounce) are the only admin paths. They refuse test mode (`VIGIL_TEST_NO_ADMIN`) and refuse environment-overridden privileged install paths before running any `sudo` command.
- Tradeoff: this removes repeated runtime sudo, but Vigil now owns a persistent privileged component. Treat the helper boundary as a real privilege boundary.
- Thermal and battery cut-offs are conservative by default and always enforced — there is no override. The daemon releases sleep prevention when the kernel reports thermal pressure or the battery falls below the floor, so Vigil never holds the machine awake while it is overheating or running low.

## Acknowledgements

Direction-setting and design references (with verified facts traced back to source):

- [`CharlonTank/agents-sleep-preventer`](https://github.com/CharlonTank/agents-sleep-preventer) — tick loop, thermal probe, refcount discipline.
- [`hiddenest/awake`](https://github.com/hiddenest/awake) — `caffeinate` lifecycle pattern, session-aware-providers model.
- [`iccir/Fermata`](https://github.com/iccir/Fermata) — confirmed via `Source/AppleSPI.h` and `RestlessEngine.m` that the private SPI uses the same `kIOPMSleepDisabledKey` as `pmset disablesleep`.
- [`segevfiner/keepawake-rs`](https://github.com/segevfiner/keepawake-rs) — cross-OS sleep prevention reference for Linux/Windows.

## License

MIT — see [`LICENSE`](./LICENSE).
