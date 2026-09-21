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

## Dropped

- **Linux (5.8) and Windows (5.9).** The partial Linux power path was removed; the workspace refuses to compile off macOS.
- **Native menu-bar/tray app.** Replaced by the SwiftBar plugin.
- **The all-phases release gate, v1.0.0, and the Homebrew tap.**

## Open

- **Clamshell test.** Does `pmset disablesleep` beat the lid sensor on this M5 Pro, on battery, with no external display? Protocol and result: [`docs/apple-silicon-lid-closed.md`](./docs/apple-silicon-lid-closed.md).
- **New agent surfaces** — only when one is installed here and can be verified live. Sketch: [`future/phase-N-agent-surface-refresh.md`](./future/phase-N-agent-surface-refresh.md).
- **Conflict detection** against other sleep holders. Sketch: [`future/phase-N-conflict-detection.md`](./future/phase-N-conflict-detection.md).

## Working policy

- **Cargo is the sole test suite.** `tests/` holds only fixtures and an experiment script.
- **Re-plan a sketch before implementing it.**
- **Rust over Swift**, latest crate versions, no pins.
