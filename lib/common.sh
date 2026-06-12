#!/usr/bin/env bash
# lib/common.sh — paths, logging, sudo wrapper. Sourced by every other lib + bin.
#
# Conventions:
#   - All paths derived from XDG-ish defaults but honor VIGIL_* overrides for testing.
#   - log() writes structured lines to the daemon log AND stderr if interactive.
#   - sudo_n_pmset() is the ONLY way the rest of the codebase calls pmset with sudo.

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

# ---- sudo discipline --------------------------------------------------------

# Verify non-interactive sudo for the EXACT whitelisted pmset commands works.
# Returns 0 if usable, 1 otherwise. Never invokes plain `sudo` — that would
# prompt and (under launchd) hang.
#
# `sudo -n -l <cmd> [args...]` doesn't run the command — it tells us whether
# the command is allowed for this user. Output is the canonical command path
# on success, empty + non-zero on denial / no NOPASSWD. We test BOTH whitelist
# entries so a partial sudoers truncation doesn't silently look healthy.
sudo_n_pmset_check() {
    sudo -n -l /usr/bin/pmset -a disablesleep 0 >/dev/null 2>&1 || return 1
    sudo -n -l /usr/bin/pmset -a disablesleep 1 >/dev/null 2>&1 || return 1
    return 0
}

# Run `sudo -n /usr/bin/pmset -a disablesleep <0|1>`. Logs failure loudly.
# Returns whatever pmset returns; 1 if non-interactive sudo isn't available.
sudo_n_pmset_disablesleep() {
    local val="$1"
    case "$val" in 0|1) ;; *) log ERROR "invalid disablesleep value: $val"; return 2 ;; esac
    if ! sudo_n_pmset_check; then
        log ERROR "sudo -n /usr/bin/pmset failed — sudoers.d not configured. Run 'vigil setup' or 'vigil doctor'."
        return 1
    fi
    if ! sudo -n /usr/bin/pmset -a disablesleep "$val" 2>>"$VIGIL_LOG_FILE"; then
        log ERROR "sudo -n /usr/bin/pmset -a disablesleep $val failed"
        return 1
    fi
    log INFO "pmset -a disablesleep $val"
    return 0
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
