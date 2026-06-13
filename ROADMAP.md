# Vigil Roadmap

Phases ship locally, in order. **No version tag, no GitHub release, no Homebrew tap until every phase has shipped and stabilized.** Each phase from 2 onward gets a *detailed plan written into its `future/phase-N-*.md` file before implementation starts* — the sketches below are direction-setting only.

| Phase | Scope | Status |
| --- | --- | --- |
| **1. CLI + wrapper** | Detect `claude`, `codex`, `copilot` CLIs. `vigil run <cmd>` wrapper. Bash daemon, root LaunchDaemon helper for pmset transitions, LaunchAgent, refcount, thermal/battery guards, baseline-state restore. | **in progress** |
| **2. copilot-companion integration** | End-to-end re-evaluation of the companion daemon. Session-aware via mtime on `~/.claude/copilot-companion/threads/*.json`. | **audited — no code change needed** ([closeout](./future/phase-2-copilot-companion.md)) |
| **3. Desktop app detection** | Session-aware Codex.app + Claude.app LAM + VS Code OpenAI ChatGPT extension agent mode. VS Code + GitHub Copilot Chat deferred to phase 3.1. | **shipped — closeout** ([`future/phase-3-desktop-apps.md`](./future/phase-3-desktop-apps.md)) |
| **3.1. VS Code + GitHub Copilot Chat detection** | In-process JavaScript chat: VS Code/Insiders host-process anchor plus semantic hash changes in `workspaceStorage/*/chatEditingSessions/*/state.json`. Raw mtime was rejected because VS Code rewrites the file while idle. Copilot CLI sessions are already covered by the `copilot` process + `${COPILOT_HOME:-~/.copilot}/session-state` probe. | **shipped — closeout** ([`future/phase-3.1-vscode-copilot-chat.md`](./future/phase-3.1-vscode-copilot-chat.md)) |
| **4. Lock feature** | `vigil lock` freezes the active macOS session until a configured key combo. Native Rust helper with an active session CGEventTap; real macOS Lock Screen integration is not part of the default combo-unlock path. | **shipped — phase-4 lock helper** ([`future/phase-4-lock-feature.md`](./future/phase-4-lock-feature.md)) |
| **5. Full Rust rewrite + UX overhaul + cross-OS** | Strangle the Bash into a single Rust `vigil` binary (+ root helper, + the existing lock helper) one **vertical slice** at a time — each slice bundles the Rust port, its UX overhaul, and its security hardening (not separate passes). Sub-phases 5.1–5.7 reach **macOS parity first** (CLI/output substrate, config/logging, detection core, smarter thermal policy, the privileged power boundary, lock overlay window, daemon + service + a unified check engine); 5.8 Linux (logind via `zbus`) and 5.9 Windows (`SetThreadExecutionState`) fill platform-trait impls behind already-stable seams. | **planned — umbrella plan** ([`future/phase-5-rust-rewrite.md`](./future/phase-5-rust-rewrite.md)); supersedes the old [`phase-5-cross-os.md`](./future/phase-5-cross-os.md) sketch |
| **6. Native UI surfaces (deferred)** | Menu-bar / tray status item, optional full GUI app, and the `doctor`/`status` command **merge** (Phase 5.7 already unifies the underlying `CheckEngine`, so the merge is then trivial). Explicitly out of the Phase 5 rewrite. | deferred — not yet planned |

## Working policy

- **No release until all phases ship and stabilize locally.** Then v1.0.0 + Homebrew tap.
- **Re-plan every deferred phase before implementing.** Edit the corresponding `future/phase-N-*.md` from a sketch into a real plan before starting work.
- **Bash for v1–v4, Rust from v5.** Phases 1–4 are Bash plus a native lock helper; **Phase 5 is the full cutover to Rust** via the vertical-slice strangler (see [`future/phase-5-rust-rewrite.md`](./future/phase-5-rust-rewrite.md)). Rust over Swift to avoid Apple lock-in.
- **No interim semver.** Master branch is the working state until v1.0.0.
