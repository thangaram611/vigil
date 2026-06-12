#!/usr/bin/env bash
# lib/common.sh — paths, logging, root-helper IPC. Sourced by every other lib + bin.
#
# Conventions:
#   - All paths derived from XDG-ish defaults but honor VIGIL_* overrides for testing.
#   - log() writes structured lines to the daemon log AND stderr if interactive.
#   - vigil_power_helper_request() is the ONLY runtime path for privileged pmset changes.

set -euo pipefail

vigil_repo_root() {
    # Resolve the repo root from the location of THIS file. Works regardless
    # of where the caller cd'd to or how the script was invoked.
    local src="${BASH_SOURCE[0]}"
    while [[ -L "$src" ]]; do
        local dir; dir=$(cd -P "$(dirname "$src")" && pwd)
        src=$(readlink "$src")
        [[ "$src" != /* ]] && src="$dir/$src"
    done
    cd -P "$(dirname "$src")/.." && pwd
}

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(vigil_repo_root)}"
# Install location for the daemon + libs. Must NOT be ~/Documents/* — macOS TCC
# blocks user-domain launchd from executing files there. ~/Library/Application
# Support is unprotected for the user's own LaunchAgents.
VIGIL_INSTALL_DIR="${VIGIL_INSTALL_DIR:-$HOME/Library/Application Support/vigil}"
VIGIL_STATE_DIR="${VIGIL_STATE_DIR:-$VIGIL_INSTALL_DIR/state}"
VIGIL_LOG_DIR="${VIGIL_LOG_DIR:-$HOME/Library/Logs/vigil}"
VIGIL_CONFIG_FILE="${VIGIL_CONFIG_FILE:-$HOME/.config/vigil/vigil.conf}"
VIGIL_ACTIVE_DIR="$VIGIL_STATE_DIR/active"
VIGIL_BASELINE_FILE="$VIGIL_STATE_DIR/baseline.json"
VIGIL_CAFFEINATE_PIDFILE="$VIGIL_STATE_DIR/caffeinate.pid"
VIGIL_DAEMON_PIDFILE="$VIGIL_STATE_DIR/daemon.pid"
VIGIL_LOCK_FILE="$VIGIL_STATE_DIR/state.lock"
VIGIL_VSCODE_COPILOT_STATE_FILE="$VIGIL_STATE_DIR/vscode-copilot-chat.state"
VIGIL_ROOT_DIR="${VIGIL_ROOT_DIR:-/Library/Application Support/vigil}"
VIGIL_ROOT_BIN_DIR="${VIGIL_ROOT_BIN_DIR:-$VIGIL_ROOT_DIR/bin}"
VIGIL_ROOT_HELPER="${VIGIL_ROOT_HELPER:-$VIGIL_ROOT_BIN_DIR/vigil-root-helper}"
VIGIL_POWER_HELPER_DIR="${VIGIL_POWER_HELPER_DIR:-$VIGIL_ROOT_DIR/helper}"
VIGIL_POWER_REQUEST_BASE="${VIGIL_POWER_REQUEST_BASE:-$VIGIL_POWER_HELPER_DIR/requests}"
VIGIL_POWER_RESPONSE_BASE="${VIGIL_POWER_RESPONSE_BASE:-$VIGIL_POWER_HELPER_DIR/responses}"
VIGIL_POWER_REQUEST_DIR="${VIGIL_POWER_REQUEST_DIR:-$VIGIL_POWER_REQUEST_BASE/$(id -u)}"
VIGIL_POWER_RESPONSE_DIR="${VIGIL_POWER_RESPONSE_DIR:-$VIGIL_POWER_RESPONSE_BASE/$(id -u)}"
VIGIL_POWER_STATE_DIR="${VIGIL_POWER_STATE_DIR:-$VIGIL_POWER_HELPER_DIR/state}"
VIGIL_POWER_LOG_DIR="${VIGIL_POWER_LOG_DIR:-$VIGIL_POWER_HELPER_DIR/logs}"
VIGIL_POWER_LOG_FILE="${VIGIL_POWER_LOG_FILE:-$VIGIL_POWER_LOG_DIR/helper.log}"
VIGIL_POWER_HELPER_TIMEOUT_SECS="${VIGIL_POWER_HELPER_TIMEOUT_SECS:-10}"
# System-managed log-rotation drop-in. Owned by root, installed by `vigil setup`,
# removed by `vigil uninstall`. NOT user-overridable — newsyslog only reads
# /etc/newsyslog.d/.
VIGIL_NEWSYSLOG_FILE="/etc/newsyslog.d/vigil.conf"
# VIGIL_LOG_FILE is intentionally NOT set here. It's derived inside
# vigil_load_config so that a vigil.conf overriding VIGIL_LOG_DIR re-derives
# the log path. See the init-order note in vigil_load_config below.

# Tunables (overridable in vigil.conf — sourced by daemon if present)
VIGIL_TICK_SECS="${VIGIL_TICK_SECS:-5}"
VIGIL_STALE_AGE_SECS="${VIGIL_STALE_AGE_SECS:-30}"
VIGIL_STALE_CPU_PCT="${VIGIL_STALE_CPU_PCT:-0.5}"
VIGIL_THERMAL_COOLDOWN_SECS="${VIGIL_THERMAL_COOLDOWN_SECS:-60}"
VIGIL_BATTERY_FLOOR_PCT="${VIGIL_BATTERY_FLOOR_PCT:-20}"
VIGIL_LOCK_COMBO="${VIGIL_LOCK_COMBO:-ctrl+alt+shift+cmd+l}"
VIGIL_LOCK_MAX_SECS="${VIGIL_LOCK_MAX_SECS:-28800}"
VIGIL_LOCK_HELPER="${VIGIL_LOCK_HELPER:-$VIGIL_INSTALL_DIR/bin/vigil-lock-helper}"
# Agent state roots. These mirror each provider's documented home override:
#   Claude Code: CLAUDE_CONFIG_DIR replaces ~/.claude
#   Codex:       CODEX_HOME replaces ~/.codex
#   Copilot CLI: COPILOT_HOME replaces ~/.copilot
_VIGIL_CLAUDE_HOME_AUTO=0
_VIGIL_CODEX_HOME_AUTO=0
_VIGIL_COPILOT_HOME_AUTO=0
if [[ -z "${VIGIL_CLAUDE_HOME+x}" ]]; then
    _VIGIL_CLAUDE_HOME_AUTO=1
    VIGIL_CLAUDE_HOME="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
fi
if [[ -z "${VIGIL_CODEX_HOME+x}" ]]; then
    _VIGIL_CODEX_HOME_AUTO=1
    VIGIL_CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
fi
if [[ -z "${VIGIL_COPILOT_HOME+x}" ]]; then
    _VIGIL_COPILOT_HOME_AUTO=1
    VIGIL_COPILOT_HOME="${COPILOT_HOME:-$HOME/.copilot}"
fi
# VS Code Copilot Chat detection is content-change based. Discovery is cached
# because workspaceStorage can contain many historical chatEditingSessions files.
VIGIL_VSCODE_COPILOT_DISCOVER_SECS="${VIGIL_VSCODE_COPILOT_DISCOVER_SECS:-30}"
VIGIL_VSCODE_COPILOT_RECENT_MINS="${VIGIL_VSCODE_COPILOT_RECENT_MINS:-10}"
# Idle window: a CLI agent only counts toward the refcount if its session
# storage was modified within this many seconds. BSD `find -mmin` rounds up
# to whole minutes, so values < 60s silently floor to 60s.
VIGIL_IDLE_AFTER_SEC="${VIGIL_IDLE_AFTER_SEC:-300}"
VIGIL_FORCE="${VIGIL_FORCE:-0}"

# ---- logging ----------------------------------------------------------------

log() {
    # Usage: log LEVEL message...
    # Levels: INFO WARN ERROR DEBUG
    local level="$1"; shift
    local ts; ts=$(date '+%Y-%m-%dT%H:%M:%S%z')
    local line="$ts $level $*"
    # daemon log (best-effort; never fail the caller because we couldn't log)
    if [[ -n "${VIGIL_LOG_FILE:-}" ]]; then
        printf '%s\n' "$line" >> "$VIGIL_LOG_FILE" 2>/dev/null || true
    fi
    # stderr if attached to a tty (interactive vigil CLI)
    if [[ -t 2 ]]; then
        printf '%s\n' "$line" >&2
    fi
}

die() {
    log ERROR "$*"
    exit 1
}

# ---- privileged power helper ------------------------------------------------

vigil_power_response_field() {
    local field="$1" response="$2"
    printf '%s\n' "$response" | awk -F= -v k="$field" '$1 == k { sub(/^[^=]*=/, ""); print; exit }'
}

vigil_power_stat_uid() {
    stat -f '%u' "$1" 2>/dev/null || stat -c '%u' "$1"
}

vigil_power_stat_mode() {
    stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1"
}

vigil_power_group_or_other_writable() {
    local mode="$1" group_digit other_digit
    group_digit="${mode: -2:1}"
    other_digit="${mode: -1:1}"
    [[ "$group_digit" =~ ^[0-7]$ && "$other_digit" =~ ^[0-7]$ ]] || return 0
    (( (10#$group_digit & 2) != 0 || (10#$other_digit & 2) != 0 ))
}

vigil_power_response_file_ok() {
    local path="$1" uid mode
    if [[ -L "$path" || ! -f "$path" ]]; then
        log ERROR "root helper response is not a regular file: $path"
        return 1
    fi
    uid=$(vigil_power_stat_uid "$path") || return 1
    if [[ "$uid" != "0" ]]; then
        log ERROR "root helper response has unexpected owner uid=$uid path=$path"
        return 1
    fi
    mode=$(vigil_power_stat_mode "$path") || return 1
    if vigil_power_group_or_other_writable "$mode"; then
        log ERROR "root helper response is group/other writable mode=$mode path=$path"
        return 1
    fi
    return 0
}

vigil_power_helper_request() {
    local action="$1"
    case "$action" in engage|release|status) ;; *) log ERROR "invalid power helper action: $action"; return 2 ;; esac
    if [[ ! -d "$VIGIL_POWER_REQUEST_DIR" || ! -d "$VIGIL_POWER_RESPONSE_DIR" ]]; then
        log ERROR "root helper IPC dirs missing — run 'vigil setup' or 'vigil doctor'"
        return 1
    fi

    local id req_tmp req_file resp_file response status waited max_ticks
    id="$(id -u).$$.$(date +%s).$RANDOM$RANDOM"
    req_tmp="$VIGIL_POWER_REQUEST_DIR/.req.$id"
    req_file="$VIGIL_POWER_REQUEST_DIR/req.$id"
    resp_file="$VIGIL_POWER_RESPONSE_DIR/resp.$id"

    (
        umask 077
        printf '%s\n' "$action" > "$req_tmp"
    ) || return 1
    chmod 0600 "$req_tmp" 2>/dev/null || true
    mv "$req_tmp" "$req_file"

    waited=0
    max_ticks=$(( VIGIL_POWER_HELPER_TIMEOUT_SECS * 10 ))
    while (( waited < max_ticks )); do
        if [[ -f "$resp_file" ]]; then
            vigil_power_response_file_ok "$resp_file" || return 1
            response=$(cat "$resp_file" 2>/dev/null || true)
            status=$(vigil_power_response_field status "$response")
            if [[ "$status" == "ok" ]]; then
                printf '%s\n' "$response"
                return 0
            fi
            log ERROR "root helper action=$action failed: $(vigil_power_response_field message "$response")"
            printf '%s\n' "$response"
            return 1
        fi
        sleep 0.1
        waited=$(( waited + 1 ))
    done

    rm -f "$req_file" "$req_tmp" 2>/dev/null || true
    log ERROR "root helper action=$action timed out after ${VIGIL_POWER_HELPER_TIMEOUT_SECS}s"
    return 1
}

vigil_power_helper_check() {
    vigil_power_helper_request status >/dev/null 2>&1
}

vigil_power_engage() {
    vigil_power_helper_request engage >/dev/null
}

vigil_power_release() {
    vigil_power_helper_request release >/dev/null
}

vigil_power_set_disablesleep() {
    local val="$1"
    case "$val" in
        1) vigil_power_engage ;;
        0) vigil_power_release ;;
        *) log ERROR "invalid disablesleep value: $val"; return 2 ;;
    esac
}

# ---- config file ------------------------------------------------------------

vigil_load_config() {
    local auto_claude_home="$VIGIL_CLAUDE_HOME"
    local auto_codex_home="$VIGIL_CODEX_HOME"
    local auto_copilot_home="$VIGIL_COPILOT_HOME"
    if [[ -f "$VIGIL_CONFIG_FILE" ]]; then
        # shellcheck source=/dev/null
        source "$VIGIL_CONFIG_FILE"
    fi
    # Re-derive auto provider homes after config sourcing. This lets a
    # vigil.conf set CODEX_HOME / CLAUDE_CONFIG_DIR / COPILOT_HOME directly,
    # while still preserving explicit VIGIL_*_HOME overrides.
    if (( _VIGIL_CLAUDE_HOME_AUTO == 1 )) && [[ "$VIGIL_CLAUDE_HOME" == "$auto_claude_home" ]]; then
        VIGIL_CLAUDE_HOME="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
    fi
    if (( _VIGIL_CODEX_HOME_AUTO == 1 )) && [[ "$VIGIL_CODEX_HOME" == "$auto_codex_home" ]]; then
        VIGIL_CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
    fi
    if (( _VIGIL_COPILOT_HOME_AUTO == 1 )) && [[ "$VIGIL_COPILOT_HOME" == "$auto_copilot_home" ]]; then
        VIGIL_COPILOT_HOME="${COPILOT_HOME:-$HOME/.copilot}"
    fi
    # Derive VIGIL_LOG_FILE AFTER config sourcing so a vigil.conf that sets
    # only VIGIL_LOG_DIR correctly re-derives the log file path. Unconditional
    # assignment — no `:-` fallback — because the `:-` form is precisely what
    # silently kept the stale top-level value before this fix. There is no
    # legitimate use case for overriding the basename ("daemon.log") without
    # also changing the directory; users override VIGIL_LOG_DIR only.
    VIGIL_LOG_FILE="$VIGIL_LOG_DIR/daemon.log"
}

# ---- misc helpers -----------------------------------------------------------

vigil_ensure_dirs() {
    mkdir -p "$VIGIL_STATE_DIR" "$VIGIL_ACTIVE_DIR" "$VIGIL_LOG_DIR"
    chmod 0700 "$VIGIL_STATE_DIR" 2>/dev/null || true
}

vigil_now_unix() { date +%s; }

# basename of a path, no trailing slash assumed
vigil_basename() { printf '%s\n' "${1##*/}"; }

# Minimal JSON string escaping for status output. This intentionally stays small:
# status fields are short operational strings, not arbitrary documents.
vigil_json_escape() {
    local s="${1:-}"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\t'/\\t}"
    s="${s//$'\r'/\\r}"
    s="${s//$'\n'/\\n}"
    printf '%s' "$s"
}

# Read SleepDisabled from `pmset -g`. Returns "0" or "1" on stdout (default 0).
vigil_read_sleepdisabled() {
    # Output line shape: "  SleepDisabled        0" (varies by macOS version)
    local out; out=$(pmset -g 2>/dev/null | awk '/SleepDisabled/ {print $NF}')
    case "$out" in 0|1) printf '%s\n' "$out" ;; *) printf '0\n' ;; esac
}
