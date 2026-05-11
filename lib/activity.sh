#!/usr/bin/env bash
# lib/activity.sh — per-agent session-mtime activity probe.
#
# Each supported CLI agent writes a per-turn JSONL file under a known
# directory. We treat an agent as "active" iff at least one of its session
# files was modified within VIGIL_IDLE_AFTER_SEC. Probe is a single
# `find -mmin -<N> -print -quit` — exits on first match.
#
# Confirmed signals (all on macOS):
#   claude  — ~/.claude/projects/<encoded-cwd>/<uuid>.jsonl
#   codex   — ~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl
#   copilot — ~/.copilot/session-state/<uuid>/events.jsonl
# None of the three writes a heartbeat while idle.

# shellcheck source=common.sh
source "${VIGIL_LIB_DIR:-$(dirname "${BASH_SOURCE[0]}")}/common.sh"

# Map an agent token to its session directory. Echoes path on success.
# Optional 2nd arg overrides $HOME (used by tests). Returns 1 on unknown.
vigil_session_dir_for_agent() {
    local agent="$1" home="${2:-$HOME}"
    case "$agent" in
        claude)  printf '%s\n' "$home/.claude/projects" ;;
        codex)   printf '%s\n' "$home/.codex/sessions" ;;
        copilot) printf '%s\n' "$home/.copilot/session-state" ;;
        *) return 1 ;;
    esac
}

# Map an agent token to the find -name glob for its per-turn file.
vigil_agent_pattern_for() {
    case "$1" in
        claude)  printf '*.jsonl\n' ;;
        codex)   printf 'rollout-*.jsonl\n' ;;
        copilot) printf 'events.jsonl\n' ;;
        *) return 1 ;;
    esac
}

# 0 (active) iff any matching file under the agent's session dir was modified
# within VIGIL_IDLE_AFTER_SEC. Missing or empty dir → 1 (idle).
# Optional 2nd arg overrides $HOME (tests).
vigil_agent_is_active() {
    local agent="$1" home="${2:-$HOME}"
    local dir pat
    dir=$(vigil_session_dir_for_agent "$agent" "$home") || return 1
    pat=$(vigil_agent_pattern_for "$agent") || return 1
    [[ -d "$dir" ]] || return 1
    # BSD `find -mmin` is whole-minute granularity — round up.
    local secs="${VIGIL_IDLE_AFTER_SEC:-300}"
    local mins=$(( (secs + 59) / 60 ))
    (( mins < 1 )) && mins=1
    local hit
    hit=$(find "$dir" -type f -name "$pat" -mmin "-$mins" -print -quit 2>/dev/null)
    [[ -n "$hit" ]]
}

# Tri-state for status display: "active" | "idle" | "none".
# "none" means the session dir doesn't exist on disk at all.
vigil_agent_state() {
    local agent="$1" home="${2:-$HOME}"
    local dir
    dir=$(vigil_session_dir_for_agent "$agent" "$home") || { printf 'none\n'; return 0; }
    [[ -d "$dir" ]] || { printf 'none\n'; return 0; }
    if vigil_agent_is_active "$agent" "$home"; then
        printf 'active\n'
    else
        printf 'idle\n'
    fi
}
