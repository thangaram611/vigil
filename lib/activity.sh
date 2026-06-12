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
# Provider home overrides are honored:
#   CLAUDE_CONFIG_DIR / VIGIL_CLAUDE_HOME
#   CODEX_HOME        / VIGIL_CODEX_HOME
#   COPILOT_HOME      / VIGIL_COPILOT_HOME
# None of the three writes a heartbeat while idle.

# shellcheck source=common.sh
source "${VIGIL_LIB_DIR:-$(dirname "${BASH_SOURCE[0]}")}/common.sh"

# Map an agent token to its provider home. Echoes path on success.
# Optional 2nd arg overrides $HOME (used by tests) and intentionally bypasses
# live provider env vars so fixtures remain deterministic.
vigil_agent_home_for() {
    local agent="$1" home_override="${2:-}"
    if [[ -n "$home_override" ]]; then
        case "$agent" in
            claude)  printf '%s\n' "$home_override/.claude" ;;
            codex)   printf '%s\n' "$home_override/.codex" ;;
            copilot) printf '%s\n' "$home_override/.copilot" ;;
            *) return 1 ;;
        esac
        return 0
    fi
    case "$agent" in
        claude)  printf '%s\n' "$VIGIL_CLAUDE_HOME" ;;
        codex)   printf '%s\n' "$VIGIL_CODEX_HOME" ;;
        copilot) printf '%s\n' "$VIGIL_COPILOT_HOME" ;;
        *) return 1 ;;
    esac
}

# Map an agent token to its session directory. Echoes path on success.
# Optional 2nd arg overrides $HOME (used by tests). Returns 1 on unknown.
vigil_session_dir_for_agent() {
    local agent="$1" home_override="${2:-}"
    local root
    root=$(vigil_agent_home_for "$agent" "$home_override") || return 1
    case "$agent" in
        claude)  printf '%s\n' "$root/projects" ;;
        codex)   printf '%s\n' "$root/sessions" ;;
        copilot) printf '%s\n' "$root/session-state" ;;
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
    local agent="$1" home="${2:-}"
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

# Newest matching activity-file mtime for diagnostics. Prints unix seconds, or
# nothing with nonzero status when the session dir is missing or has no matches.
# Optional 2nd arg overrides $HOME (tests).
vigil_agent_latest_activity_mtime() {
    local agent="$1" home="${2:-}"
    local dir pat latest=0 f m
    dir=$(vigil_session_dir_for_agent "$agent" "$home") || return 1
    pat=$(vigil_agent_pattern_for "$agent") || return 1
    [[ -d "$dir" ]] || return 1
    while IFS= read -r f; do
        [[ -z "$f" ]] && continue
        m=$(stat -f %m "$f" 2>/dev/null || echo 0)
        [[ "$m" =~ ^[0-9]+$ ]] || continue
        (( m > latest )) && latest="$m"
    done < <(find "$dir" -type f -name "$pat" 2>/dev/null)
    (( latest > 0 )) || return 1
    printf '%s\n' "$latest"
}

# Newest matching activity-file age in seconds for diagnostics. Prints age, or
# nothing with nonzero status when no matching activity file exists.
vigil_agent_latest_activity_age_secs() {
    local agent="$1" home="${2:-}"
    local mtime now
    mtime=$(vigil_agent_latest_activity_mtime "$agent" "$home") || return 1
    now=$(vigil_now_unix)
    printf '%s\n' "$(( now - mtime ))"
}

# Tri-state for status display: "active" | "idle" | "none".
# "none" means the session dir doesn't exist on disk at all.
vigil_agent_state() {
    local agent="$1" home="${2:-}"
    local dir
    dir=$(vigil_session_dir_for_agent "$agent" "$home") || { printf 'none\n'; return 0; }
    [[ -d "$dir" ]] || { printf 'none\n'; return 0; }
    if vigil_agent_is_active "$agent" "$home"; then
        printf 'active\n'
    else
        printf 'idle\n'
    fi
}
