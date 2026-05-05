#!/usr/bin/env bash
# lib/detect.sh — match running AI agent processes from `ps -axww -o pid= -o command=`.
#
# Phase 1 scope: CLI processes claude, codex, copilot.
# Hard-excluded: /Applications/* (desktop apps + their bundled servers + node_repl), Helpers,
# crashpad workers, chrome-native-host.
#
# Output format (tab-separated, one match per line, on stdout):
#   <pid>\t<name>\t<exe>\t<args>
# where <name> is one of: cli-claude, cli-codex, cli-copilot.

# shellcheck source=common.sh
source "${VIGIL_LIB_DIR:-$(dirname "${BASH_SOURCE[0]}")}/common.sh"

# Phase-1 CLI agents we recognise. Adding a new one is a one-line case below.
VIGIL_CLI_AGENTS=("claude" "codex" "copilot")

# Hard-exclude patterns. These match against the full executable path (the
# first whitespace-separated token of the `command=` field).
_vigil_is_excluded_exe() {
    local exe="$1"
    case "$exe" in
        /Applications/*)         return 0 ;;  # any desktop app or its bundled tools
        */Helper*)               return 0 ;;  # Electron helpers (Renderer, GPU, etc.)
        *crashpad*)              return 0 ;;  # crash reporters
        *chrome-native-host*)    return 0 ;;  # Chrome MCP bridge
        *node_repl*)             return 0 ;;  # Codex.app's bundled Node REPL
    esac
    return 1
}

# Match a single (pid, command_line) pair. Echoes a TSV row on match, nothing otherwise.
vigil_detect_line() {
    local pid="$1" command_line="$2"
    local exe args base
    # Split command_line into exe + args. exe is the first whitespace-separated token.
    exe="${command_line%% *}"
    args="${command_line#"$exe"}"; args="${args# }"
    base="${exe##*/}"

    if _vigil_is_excluded_exe "$exe"; then
        return 0
    fi

    case "$base" in
        claude|codex|copilot)
            printf '%s\t%s\t%s\t%s\n' "$pid" "cli-$base" "$exe" "$args"
            ;;
    esac
}

# Iterate over `ps -axww -o pid= -o command=` (or a fixture file via $1).
# Emits TSV rows for matches.
vigil_detect_all() {
    local source="${1:--}"  # default: pipe stdin
    local input
    if [[ "$source" == "-" ]]; then
        input=$(ps -axww -o pid= -o command= 2>/dev/null)
    else
        input=$(<"$source")
    fi

    # Each ps line: leading spaces, <pid>, single space, <command_line>.
    # We strip leading spaces, split on first space.
    while IFS= read -r line; do
        line="${line#"${line%%[![:space:]]*}"}"  # ltrim
        [[ -z "$line" ]] && continue
        local pid="${line%% *}"
        local rest="${line#"$pid"}"; rest="${rest# }"
        # Skip non-numeric pid (shouldn't happen with `ps -o pid=`, but defensive).
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        vigil_detect_line "$pid" "$rest"
    done <<< "$input"
}
