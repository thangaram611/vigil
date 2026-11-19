# Vigil Roadmap

**Vigil is a personal macOS tool.** It is not a product: no releases, no Homebrew tap, no cross-OS port. `main` is the working state.

That was decided on 2026-09-20. The category is covered — [`adrafinil`](https://github.com/kageroumado/adrafinil) owns the macOS niche, and Claude Code, Codex, and Copilot CLI all inhibit idle sleep themselves. The evidence is in [`VIABILITY-REVIEW-2026-08-04.md`](./VIABILITY-REVIEW-2026-08-04.md). What Vigil still does that nothing else does for this machine: hookless Copilot CLI and VS Code Copilot Chat detection, restoring the prior `SleepDisabled` value, and the built-in lock guard.

## Shipped

| Phase | Scope |
| --- | --- |
| **1. CLI + wrapper** | Detect `claude`, `codex`, `copilot`. `vigil run <cmd>`. Resident daemon, root LaunchDaemon helper for `pmset`, LaunchAgent, refcount, thermal/battery guards, baseline restore. |
| **2. copilot-companion** | Audited, no code change: the `node` router is excluded by the `node_repl` guard; the bundled `copilot` worker is caught by basename plus `~/.copilot/session-state` activity. |
| **3. Desktop app detection** | Codex.app host, Claude.app Local Agent Mode, and the ChatGPT VS Code extension's bundled `codex` worker. |
| **3.1. VS Code + Copilot Chat** | Host-process anchor plus sha256 content-hash changes in `chatEditingSessions/*/state.json` (raw mtime is unusable — VS Code rewrites the file while idle). |
| **4. Lock guard** | `vigil lock`: CoreGraphics event tap freezes input until an ordered chord. The macOS Lock Screen passes through. |
| **5. Rust rewrite** | One `vigil` binary, a root `vigil-root-helper`, and `vigil-lock-helper`. All shell deleted. Unified `CheckEngine` behind `status` and `doctor`. |
| **6. Manual mode + menu bar** | `vigil mode auto\|on\|off` overrides agent detection; thermal and battery guards still win. A SwiftBar plugin shows state and switches mode from the menu bar. |
| **7. Clamshell test + closed-lid fixes** | Measured on the M5 Pro: `disablesleep` beats the lid sensor on battery. Found and fixed two closed-lid defects — sleep after a release, lock under a hold. [`docs/apple-silicon-lid-closed.md`](./docs/apple-silicon-lid-closed.md). |
| **8. Per-session activity** | Shared native session reducers and process ownership, explicit unknown/uncertain demand, 30-second release grace, acknowledged refresh, schema 2 status/tick, and read-only diagnostics. Coverage limits remain below. |

## Dropped

- **Linux (5.8) and Windows (5.9).** The partial Linux power path was removed; the workspace refuses to compile off macOS.
- **Native menu-bar/tray app.** Replaced by the SwiftBar plugin.
- **The all-phases release gate, v1.0.0, and the Homebrew tap.**
- **Pause on lid close (2026-11-19).** The commute failures were network loss under a hold, not sleep. Claude Code's `CLAUDE_CODE_RETRY_WATCHDOG` waits out the outage and Codex waits for the network by itself, so the hold now lets a closed Mac resume on any network. No pause feature or provider hooks; agent-companion's `docs/MVP_TRACKER.md` decision 11 has the evidence.

## Open

- **Activity coverage extensions.** Disk-only Codex approval waits, cold detached-process ownership, Claude Desktop and unmeasured remote/restart paths remain unknown rather than exact coverage. The implementation does not add private provider APIs, hooks or another daemon. Evidence and limits: [`future/phase-8-per-session-activity.md`](./future/phase-8-per-session-activity.md) and [`docs/fixtures/phase-8/README.md`](./docs/fixtures/phase-8/README.md).
- **New agent surfaces** — only when one is installed here and can be verified live. Sketch: [`future/phase-N-agent-surface-refresh.md`](./future/phase-N-agent-surface-refresh.md).
- **Stuck-hold follow-ups (2026-11-19).** `b0f85cd` stopped counting the agent-companion bridge, `codex-code-mode-host` and Claude's `ugrep` as work. Still open:
  - Confirm a `release — no demand` once every session is idle. Right after the fix, a "cold orphan" Claude session still gave 1 uncertain demand.
  - Any other long-lived MCP server child of a session would still count as work; only the bridge is excluded by name.
  - A running agent-companion job counts only sometimes while its host session is idle: `codex app-server` can, `claude -p` needs a native registration, and `opencode serve` only as a tied descendant. A closed Mac can sleep mid-job.
  - There is no "retrying" state. A session waiting out an outage under the retry watchdog counts as working, holding the Mac for up to the retry window (about 6 h with `CLAUDE_CODE_MAX_RETRIES=600`), bounded by the battery and thermal guards. That is deliberate for now: work resumes on any network.
  - Proposal, not adopted: after the fix, log the detector against vendor sleep assertions for two weeks, and retire the detector if they agree.
- **Conflict detection** against other sleep holders. Sketch: [`future/phase-N-conflict-detection.md`](./future/phase-N-conflict-detection.md).

## Working policy

- **Cargo is the sole test suite.** `tests/` holds only fixtures and an experiment script.
- **Re-plan a sketch before implementing it.**
- **Rust over Swift**, latest crate versions, no pins.
