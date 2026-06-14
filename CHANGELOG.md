# Changelog

This is a pre-release, single-user project: there are no version tags or public
releases yet. Everything below is the shipped-but-unreleased history at a high
level. Per-commit detail lives in `git log`; per-phase scope and closeouts live
in [`ROADMAP.md`](./ROADMAP.md) and the `future/phase-*.md` plans.

## [Unreleased]

### Detection, wrapper, and power core (phases 1–4)

- **Phase 1 — CLI + wrapper + power boundary.** Detects the `claude`, `codex`,
  and `copilot` CLIs by process basename, holds sleep prevention via a
  refcount over per-agent pidfiles, and exposes `vigil run <cmd> [args...]` to
  hold the assertion while a wrapped command runs. A resident daemon, a per-user
  LaunchAgent, and a root LaunchDaemon power helper drive `pmset disablesleep`
  through a validated file-IPC queue; thermal/battery guards and baseline-state
  restore round out the loop.
- **Phase 2 — copilot-companion (audited, no code change).** Confirmed the
  companion's `copilot --acp` worker is the real `copilot` CLI and is already
  covered by the existing process + session-activity probes.
- **Phase 3 — desktop-app detection.** Adds `app-codex` (the Codex.app host,
  carved out before the `/Applications/` exclusion), Claude.app Local Agent Mode
  coverage (transitive, via the bundled `claude` basename), and the VS Code
  OpenAI ChatGPT extension's bundled `codex` worker.
- **Phase 3.1 — VS Code + GitHub Copilot Chat.** Adds the
  `app-vscode-copilot-chat` host anchor (VS Code / Insiders) and a semantic
  content-hash activity gate over
  `workspaceStorage/*/chatEditingSessions/*/state.json` — raw mtime is ignored
  because VS Code rewrites the file while idle.
- **Phase 4 — lock guard.** Adds `vigil lock` (and `vigil lock doctor`) plus the
  native `vigil-lock-helper`, a macOS event tap that freezes input until a
  configured key combo unlocks. The default combo-unlock path is a local freeze
  guard, not the OS Lock Screen.

### Phase 5 — full Rust rewrite (5.1–5.7)

The Bash implementation has been fully replaced by a single Rust workspace; all
`bin/*.sh` and `lib/*.sh` are deleted. The remaining cross-OS slices (5.8 Linux,
5.9 Windows) are planned in [`future/phase-5.8-5.9-cross-os.md`](./future/phase-5.8-5.9-cross-os.md).

- **Self-contained binary.** One shipped `vigil` binary (the daemon is a hidden
  `vigil daemon` subcommand) plus a separate root `vigil-root-helper`. `vigil
  setup` builds and copies both into `~/Library/Application Support/vigil/bin/`
  (the install snapshot the LaunchAgent execs — mandatory for the TCC grant,
  which is tied to the binary path) and symlinks the dev build onto PATH at
  `~/.local/bin/vigil` (overridable via `VIGIL_BIN_LINK_DIR`).
- **Daemon + launchd service.** The resident tick loop runs under the per-user
  LaunchAgent `com.thangaram.vigil`; the privileged power helper runs as the
  long-lived root LaunchDaemon `com.thangaram.vigil.helper` (`--serve`). They
  communicate only through a per-uid, fd-validated file-IPC queue
  (`engage`/`release`/`status`); the daemon never runs `pmset` itself. The
  user-idle hold is `caffeinate -i` (no display assertion), so macOS can lock and
  displays can sleep while agent work continues.
- **Unified CheckEngine.** One read-only engine builds both the `vigil status`
  snapshot (`--json` / `--verbose`) and the three-state `vigil doctor` checklist
  (`--power` / `--verbose`), so status and doctor share one source of truth.

### Phase 5.6 — native lock overlay + CF migration + ordered-chord unlock

- **objc2 / CoreFoundation migration.** The lock helper moved off
  `core-graphics 0.25` to `objc2` / `objc2-core-foundation` /
  `objc2-core-graphics` for the event tap, run loop, and overlay window.
- **Opaque centered overlay.** A hand-rolled borderless `NSWindow` (above dock
  and menu bar) is fully opaque and shows a generic "press your unlock chord"
  hint — the literal combo is never rendered on screen.
- **Ordered-chord unlock.** Chords are now an ordered key sequence (press order
  is significant, floor of 3 keys, any mix). `vigil lock setup` captures the
  chord by pressing it (anchor = first key down; finalized on anchor release),
  and unlock fires only on an exact in-order match.

### UX overhaul (shipped)

- **clack-style TUI** for the interactive `setup` / `uninstall` / lock flows.
- **PATH-managed install:** `setup` symlinks `vigil` onto PATH (never clobbers a
  real file at the target), prints a PATH hint when needed, and `uninstall`
  leaves the symlink and logs in place; `reload` heals the symlink in place.
- **Faster `status` / `doctor`:** the helper-liveness probe is capped at 2s
  (vs. the daemon's full 10s power-helper timeout), so a dead or slow helper no
  longer blocks the status/doctor paint.
