# Vigil Roadmap

Phases ship locally, in order. **No version tag, no GitHub release, no Homebrew tap until every phase has shipped and stabilized.** Each phase from 2 onward gets a *detailed plan written into its `future/phase-N-*.md` file before implementation starts* — the sketches below are direction-setting only.

| Phase | Scope | Status |
| --- | --- | --- |
| **1. CLI + wrapper** | Detect `claude`, `codex`, `copilot` CLIs. `vigil run <cmd>` wrapper. Bash daemon, sudoers.d, LaunchAgent, refcount, thermal/battery guards, baseline-state restore. | **in progress** |
| **2. copilot-companion integration** | End-to-end re-evaluation of the companion daemon. Session-aware via mtime on `~/.claude/copilot-companion/threads/*.json`. | **audited — no code change needed** ([closeout](./future/phase-2-copilot-companion.md)) |
| **3. Desktop app detection** | Session-aware Codex.app + Claude.app LAM + VS Code OpenAI ChatGPT extension agent mode. VS Code + GitHub Copilot Chat deferred to phase 3.1. | **shipped — closeout** ([`future/phase-3-desktop-apps.md`](./future/phase-3-desktop-apps.md)) |
| **3.1. VS Code + GitHub Copilot Chat detection** | In-process JavaScript chat: VS Code/Insiders host-process anchor plus semantic hash changes in `workspaceStorage/*/chatEditingSessions/*/state.json`. Raw mtime was rejected because VS Code rewrites the file while idle. Copilot CLI sessions are already covered by the `copilot` process + `${COPILOT_HOME:-~/.copilot}/session-state` probe. | **shipped — closeout** ([`future/phase-3.1-vscode-copilot-chat.md`](./future/phase-3.1-vscode-copilot-chat.md)) |
| **4. Lock feature** | `vigil lock` freezes the active macOS session until a configured key combo. Native Rust helper with an active CGEventTap; real macOS Lock Screen integration is not part of the default combo-unlock path. | planned — [`future/phase-4-lock-feature.md`](./future/phase-4-lock-feature.md) |
| **5. Cross-OS port** | Full Rust rewrite. Linux (D-Bus systemd-logind / ScreenSaver) + Windows (`SetThreadExecutionState` / `LockWorkStation`). Bash phase 1 stays the macOS reference. | sketched — [`future/phase-5-cross-os.md`](./future/phase-5-cross-os.md) |

## Working policy

- **No release until all phases ship and stabilize locally.** Then v1.0.0 + Homebrew tap.
- **Re-plan every deferred phase before implementing.** Edit the corresponding `future/phase-N-*.md` from a sketch into a real plan before starting work.
- **Bash for v1, Rust for v4-5.** Native code only enters when there's no shell-out path (key capture, lock screen, cross-OS APIs). Rust over Swift to avoid Apple lock-in.
- **No interim semver.** Master branch is the working state until v1.0.0.
