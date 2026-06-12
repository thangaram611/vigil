# Changelog

> Empty until phase 1 ships. The first entry will be v0.1.0-pre or similar — likely no public release until phase 5.

## [Unreleased]

### Phase 1 (in progress)

- Initial scaffold + roadmap.
- Phase 1 hardening (non-roadmap): newsyslog.d log rotation, `power assertions:` block in `vigil status`, plist `ExitTimeOut`/`ThrottleInterval`, baseline-stickiness docs, fixed `VIGIL_LOG_FILE` init-order so `vigil.conf` overrides of `VIGIL_LOG_DIR` are honored.

### Phase 4 (lock feature)

- Added a native Rust helper (`native/vigil-lock-helper`) that uses an active macOS
  session event tap (`CGEventTapLocation::Session`, head insert, default options) to
  drop input until a configured combo unlocks.
- Added helper CLI:
  - `--check-permissions --json` (optional `--prompt`)
  - `--freeze --combo <combo> --max-secs <seconds>`
  - hidden `--debug-sleep-in-callback-ms` for manual fail-open testing
- Added `vigil lock`, `vigil lock --help`, and `vigil lock doctor [--prompt]`.
- Added Bash integration for non-interactive preflight gating, helper install/reinstall
  in setup/reload, and docs/config keys:
  - `VIGIL_LOCK_COMBO`
  - `VIGIL_LOCK_MAX_SECS`
  - `VIGIL_LOCK_HELPER`
- Documented recovery and TCC guidance; lock remains a local freeze guard (not the OS lock screen).

### Phase 2 (audited — no code change needed)

- Audited copilot-companion's runtime architecture. The companion's `copilot-acp-daemon.mjs` is a long-lived router that spawns a `copilot --acp` worker per session; that worker is the real `copilot` CLI binary and writes session events to `~/.copilot/session-state/<uuid>/events.jsonl`. Phase 1's existing process match (`detect.sh`) + activity probe (`activity.sh`) already cover it correctly. Verified live: companion worker spawns → vigil's refcount tracks it → `copilot=active` while events.jsonl is written → release after 5 min of no writes.
- Added `tests/detect_test.sh::test_picks_up_copilot_companion_acp_worker` to pin the contract that the fixture's `--acp` worker line maps to `cli-copilot` with the resolved binary path and `--acp` marker preserved in the TSV row.
- Replaced `future/phase-2-copilot-companion.md` sketch with an audit closeout document.

### Phase 3 (desktop-app detection — experiment-driven)

- **Detection rewrite to be space-safe.** `lib/detect.sh` now uses two `ps` columns per tick (`-o comm=` for the executable path, `-o command=` for the full argv) joined by pid in awk. The pre-phase-3 implementation split `ps -o command=` output on the first whitespace to derive an exe path, which silently misparses any executable whose path contains a literal space — notably Claude.app's bundled Claude Code at `~/Library/Application Support/Claude/claude-code/<ver>/claude.app/Contents/MacOS/claude`. The space-safe rewrite landed alongside the phase-3 additions because phase-3's Claude.app coverage depended on it. Path-based exclusions now substring-match the full command line (no behavioral change for the prior-fixture cases, plus correct coverage of the previously-misparsed ones).
- **Codex.app main process detected as `app-codex`.** New refcount tag for the Codex.app Electron host (`/Applications/Codex.app/Contents/MacOS/Codex`). Activity-gated on the existing `codex_active` probe (`~/.codex/sessions/**/rollout-*.jsonl` within `VIGIL_IDLE_AFTER_SEC`), so an idle-but-open Codex.app does not hold sleep. One PID file per running Codex.app instance regardless of how many `codex app-server` workers it spawns under `/Applications/Codex.app/Contents/Resources/codex` (those remain excluded by the `/Applications/*` rule). Carved out before the exclusion fires.
- **Claude.app Local Agent Mode coverage.** Empirically validated 2026-05-25 (`future/phase-3-desktop-apps.md` §5.2). The bundled CC binary's basename is `claude` and its path is NOT under `/Applications/*`, so after the parsing fix it's naturally matched as `cli-claude`. New fixture row + `tests/detect_test.sh::test_picks_up_claude_app_lam_bundled_cc` pin the contract that the spaced-path executable produces a `cli-claude` row with the full path preserved in the exe column.
- **VS Code OpenAI ChatGPT extension agent mode coverage.** Empirically validated 2026-05-25 (`future/phase-3-desktop-apps.md` §5.3). The extension's bundled `codex app-server` at `~/.vscode-insiders/extensions/openai.chatgpt-*/bin/<arch>/codex` is already matched as `cli-codex` by phase 1 (basename match, path outside `/Applications/*`, no spaces) and writes to the same `~/.codex/sessions/` path the CLI uses. New fixture row + `tests/detect_test.sh::test_picks_up_vscode_chatgpt_extension_codex_worker` pin the contract.
- **VS Code + GitHub Copilot Chat (Sonnet model) — deferred to phase 3.1.** Empirically the in-process JavaScript chat runs inside VS Code's extension host (no distinct process to anchor against) and writes only to `~/Library/Application Support/Code{,- Insiders}/User/workspaceStorage/<hash>/chatEditingSessions/*/state.json`, which is too noisy in its raw form to use directly. The follow-up needs a scoped activity probe under that exact glob plus version-stability checks; see `future/phase-3-desktop-apps.md` §5.4 / §7 for the open work.
- **Refcount + status output** unchanged in shape; `app-codex` rows surface naturally in `vigil status` and `vigil status --json`.
- **Test fixtures:**
  - `tests/fixtures/ps-axww-snapshot.txt` grew two rows (Claude.app LAM bundled CC at pid 87838, VS Code ChatGPT extension codex worker at pid 31322).
  - New paired fixture `tests/fixtures/ps-axww-comm-snapshot.txt` mirrors what live `ps -o comm=` returns. Regenerated from the command fixture via `tests/fixtures/gen_comm_from_command.py` (kept in-repo for reproducibility — the `.app/Contents/MacOS/` heuristic is documented there).
- **Experiment harness retained** under `tests/experiments/phase-3/` (runs dir gitignored). Reusable for phase-3.1 (GitHub Copilot Chat) and future editor integrations.

### Phase 3.1 (VS Code + GitHub Copilot Chat)

- Added VS Code / VS Code Insiders host detection as `app-vscode-copilot-chat`; Code Helper and extension-host processes are intentionally not matched.
- Added hash-based Copilot Chat activity detection under `workspaceStorage/*/chatEditingSessions/*/state.json`. Live validation showed raw mtime is noisy because VS Code rewrites `state.json` while idle without changing content; Vigil therefore treats semantic file-content hash changes as activity events and caches an `active_until` window.
- Kept GitHub Copilot CLI coverage on the existing `copilot` process + `${COPILOT_HOME:-~/.copilot}/session-state/**/events.jsonl` path from phase 1/provider-root support.
