# Changelog

> Empty until phase 1 ships. The first entry will be v0.1.0-pre or similar — likely no public release until phase 5.

## [Unreleased]

### Phase 1 (in progress)

- Initial scaffold + roadmap.
- Phase 1 hardening (non-roadmap): newsyslog.d log rotation, `power assertions:` block in `vigil status`, plist `ExitTimeOut`/`ThrottleInterval`, baseline-stickiness docs, fixed `VIGIL_LOG_FILE` init-order so `vigil.conf` overrides of `VIGIL_LOG_DIR` are honored.

### Phase 2 (audited — no code change needed)

- Audited copilot-companion's runtime architecture. The companion's `copilot-acp-daemon.mjs` is a long-lived router that spawns a `copilot --acp` worker per session; that worker is the real `copilot` CLI binary and writes session events to `~/.copilot/session-state/<uuid>/events.jsonl`. Phase 1's existing process match (`detect.sh`) + activity probe (`activity.sh`) already cover it correctly. Verified live: companion worker spawns → vigil's refcount tracks it → `copilot=active` while events.jsonl is written → release after 5 min of no writes.
- Added `tests/detect_test.sh::test_picks_up_copilot_companion_acp_worker` to pin the contract that the fixture's `--acp` worker line maps to `cli-copilot` with the resolved binary path and `--acp` marker preserved in the TSV row.
- Replaced `future/phase-2-copilot-companion.md` sketch with an audit closeout document.
