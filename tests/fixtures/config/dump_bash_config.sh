#!/usr/bin/env bash
# Oracle: dump bash-resolved VIGIL_* + provider homes + auto flags, sorted KEY=VALUE.
#
# Usage: caller exports desired env (HOME, VIGIL_*, CLAUDE_CONFIG_DIR, etc.)
# and optionally VIGIL_CONFIG_FILE pointing at a shell-syntax conf before
# invoking. This script sources lib/common.sh, runs vigil_load_config, and
# prints the resolved VIGIL_* + provider-home + auto-flag values in a stable
# sorted KEY=VALUE form.
#
# VIGIL_REPO_ROOT may be set by the caller; otherwise resolved from this file's
# location.
set -euo pipefail

REPO="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)}"

# Caller exports the desired env before invoking. We deliberately do NOT unset
# VIGIL_* here — the caller controls the environment. The caller MUST unset
# VIGIL_LOG_FILE before calling to avoid masking the derivation under test.

# shellcheck source=../../../lib/common.sh
source "$REPO/lib/common.sh"
vigil_load_config

emit() { printf '%s=%s\n' "$1" "${!1}"; }

{
  for v in \
    VIGIL_INSTALL_DIR \
    VIGIL_STATE_DIR \
    VIGIL_LOG_DIR \
    VIGIL_LOG_FILE \
    VIGIL_CONFIG_FILE \
    VIGIL_ACTIVE_DIR \
    VIGIL_BASELINE_FILE \
    VIGIL_CAFFEINATE_PIDFILE \
    VIGIL_DAEMON_PIDFILE \
    VIGIL_DAEMON_TICK_FILE \
    VIGIL_LOCK_FILE \
    VIGIL_VSCODE_COPILOT_STATE_FILE \
    VIGIL_ROOT_DIR \
    VIGIL_ROOT_BIN_DIR \
    VIGIL_ROOT_HELPER \
    VIGIL_POWER_HELPER_DIR \
    VIGIL_POWER_REQUEST_BASE \
    VIGIL_POWER_RESPONSE_BASE \
    VIGIL_POWER_REQUEST_DIR \
    VIGIL_POWER_RESPONSE_DIR \
    VIGIL_POWER_STATE_DIR \
    VIGIL_POWER_LOG_DIR \
    VIGIL_POWER_LOG_FILE \
    VIGIL_POWER_HELPER_TIMEOUT_SECS \
    VIGIL_NEWSYSLOG_FILE \
    VIGIL_TICK_SECS \
    VIGIL_STALE_AGE_SECS \
    VIGIL_STALE_CPU_PCT \
    VIGIL_THERMAL_COOLDOWN_SECS \
    VIGIL_BATTERY_FLOOR_PCT \
    VIGIL_START_WAIT_SECS \
    VIGIL_LOCK_COMBO \
    VIGIL_LOCK_MAX_SECS \
    VIGIL_LOCK_HELPER \
    VIGIL_CLAUDE_HOME \
    VIGIL_CODEX_HOME \
    VIGIL_COPILOT_HOME \
    VIGIL_VSCODE_COPILOT_DISCOVER_SECS \
    VIGIL_VSCODE_COPILOT_RECENT_MINS \
    VIGIL_IDLE_AFTER_SEC \
    VIGIL_FORCE; do
    emit "$v"
  done
  printf 'VIGIL_CLAUDE_HOME_AUTO=%s\n'  "$_VIGIL_CLAUDE_HOME_AUTO"
  printf 'VIGIL_CODEX_HOME_AUTO=%s\n'   "$_VIGIL_CODEX_HOME_AUTO"
  printf 'VIGIL_COPILOT_HOME_AUTO=%s\n' "$_VIGIL_COPILOT_HOME_AUTO"
} | LC_ALL=C sort
