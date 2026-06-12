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

# ---- VS Code + GitHub Copilot Chat ------------------------------------------
#
# Copilot Chat inside VS Code has no distinct per-chat worker process. The
# observed file signal is:
#   ~/Library/Application Support/Code{,- Insiders}/User/workspaceStorage/*/
#       chatEditingSessions/*/state.json
#
# Raw mtime is noisy: VS Code rewrites state.json while idle without changing
# its content. We therefore treat a semantic file-content hash change as the
# activity event and cache an active_until timestamp for VIGIL_IDLE_AFTER_SEC.

_vigil_vscode_ps() {
    if [[ "${VIGIL_VSCODE_PS_FIXTURE+set}" == "set" ]]; then
        printf '%s\n' "$VIGIL_VSCODE_PS_FIXTURE"
    else
        ps -axww -o command= 2>/dev/null
    fi
}

vigil_vscode_host_running() {
    local out; out=$(_vigil_vscode_ps)
    case "$out" in
        *"/Visual Studio Code.app/Contents/MacOS/"*|*"/Visual Studio Code - Insiders.app/Contents/MacOS/"*)
            return 0
            ;;
    esac
    return 1
}

vigil_vscode_workspace_roots() {
    local home="${1:-$HOME}"
    printf '%s\n' "$home/Library/Application Support/Code/User/workspaceStorage"
    printf '%s\n' "$home/Library/Application Support/Code - Insiders/User/workspaceStorage"
}

vigil_vscode_copilot_recent_state_files() {
    local home="${1:-$HOME}"
    local recent_mins="${VIGIL_VSCODE_COPILOT_RECENT_MINS:-10}"
    case "$recent_mins" in ''|*[!0-9]*) recent_mins=10 ;; esac
    (( recent_mins < 1 )) && recent_mins=1
    local root
    while IFS= read -r root; do
        [[ -d "$root" ]] || continue
        find "$root" -maxdepth 6 -type f \
            -path "$root/*/chatEditingSessions/*/state.json" \
            -mmin "-$recent_mins" -print 2>/dev/null
    done < <(vigil_vscode_workspace_roots "$home")
}

_vigil_file_sha256() {
    shasum -a 256 "$1" 2>/dev/null | awk '{print $1}'
}

_vigil_vscode_state_field() {
    local key="$1" file="${2:-$VIGIL_VSCODE_COPILOT_STATE_FILE}"
    [[ -f "$file" ]] || return 1
    awk -F '\t' -v k="$key" '$1 == k { print $2; exit }' "$file"
}

_vigil_vscode_state_hash_for_path() {
    local path="$1" file="${2:-$VIGIL_VSCODE_COPILOT_STATE_FILE}"
    [[ -f "$file" ]] || return 1
    awk -F '\t' -v p="$path" '$1 == "file" && $3 == p { print $2; exit }' "$file"
}

_vigil_vscode_write_state() {
    local state_file="$1" active_until="$2" last_scan="$3" primed="$4" file_lines="$5"
    mkdir -p "$(dirname "$state_file")"
    {
        printf 'active_until\t%s\n' "$active_until"
        printf 'last_scan\t%s\n' "$last_scan"
        printf 'primed\t%s\n' "$primed"
        printf '%s' "$file_lines"
    } > "$state_file"
}

vigil_vscode_copilot_chat_is_active() {
    local home="${1:-$HOME}"
    [[ "${VIGIL_VSCODE_COPILOT_ENABLED:-1}" == "0" ]] && return 1
    vigil_vscode_host_running || return 1

    local now; now=$(vigil_now_unix)
    local state_file="${VIGIL_VSCODE_COPILOT_STATE_FILE}"
    local active_until last_scan primed discover_secs
    active_until=$(_vigil_vscode_state_field active_until "$state_file" 2>/dev/null || echo 0)
    last_scan=$(_vigil_vscode_state_field last_scan "$state_file" 2>/dev/null || echo 0)
    primed=$(_vigil_vscode_state_field primed "$state_file" 2>/dev/null || echo 0)
    [[ "$active_until" =~ ^[0-9]+$ ]] || active_until=0
    [[ "$last_scan" =~ ^[0-9]+$ ]] || last_scan=0
    [[ "$primed" =~ ^[01]$ ]] || primed=0
    discover_secs="${VIGIL_VSCODE_COPILOT_DISCOVER_SECS:-30}"
    case "$discover_secs" in ''|*[!0-9]*) discover_secs=30 ;; esac
    (( discover_secs < 5 )) && discover_secs=5

    if (( now - last_scan < discover_secs )); then
        (( active_until > now ))
        return
    fi

    local changed=0 file sha old file_lines=""
    local -a current_paths=()
    while IFS= read -r file; do
        [[ -f "$file" ]] || continue
        sha=$(_vigil_file_sha256 "$file")
        [[ -n "$sha" ]] || continue
        old=$(_vigil_vscode_state_hash_for_path "$file" "$state_file" 2>/dev/null || true)
        if (( primed == 1 )) && [[ -n "$old" && "$old" != "$sha" ]]; then
            changed=1
        fi
        current_paths+=("$file")
        file_lines+="file	${sha}	${file}"$'\n'
    done < <(vigil_vscode_copilot_recent_state_files "$home")

    # Preserve hashes for files that were seen before but are not recent in
    # this scan. This avoids false activity when VS Code later performs an
    # mtime-only rewrite of an old state file with unchanged content.
    if [[ -f "$state_file" ]]; then
        local tag old_sha old_path keep current_path
        while IFS=$'\t' read -r tag old_sha old_path; do
            [[ "$tag" == "file" && -n "$old_sha" && -n "$old_path" ]] || continue
            keep=1
            if (( ${#current_paths[@]} > 0 )); then
                for current_path in "${current_paths[@]}"; do
                    if [[ "$current_path" == "$old_path" ]]; then
                        keep=0
                        break
                    fi
                done
            fi
            (( keep == 1 )) && file_lines+="file	${old_sha}	${old_path}"$'\n'
        done < "$state_file"
    fi

    if (( changed == 1 )); then
        active_until=$(( now + VIGIL_IDLE_AFTER_SEC ))
        log INFO "vscode-copilot-chat activity — semantic state changed"
    fi
    _vigil_vscode_write_state "$state_file" "$active_until" "$now" 1 "$file_lines"

    (( active_until > now ))
}

vigil_vscode_copilot_chat_state() {
    if ! vigil_vscode_host_running; then
        printf 'none\n'
        return 0
    fi
    if vigil_vscode_copilot_chat_is_active; then
        printf 'active\n'
    else
        printf 'idle\n'
    fi
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
