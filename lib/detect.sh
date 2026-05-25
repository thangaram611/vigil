#!/usr/bin/env bash
# lib/detect.sh — match running AI agent processes from `ps -axww`.
#
# Phase-1 scope: CLI processes `claude`, `codex`, `copilot`.
# Phase-3 addition: the Codex.app main process
#   `/Applications/Codex.app/Contents/MacOS/Codex` is detected as
#   `app-codex`; refcount counts it iff `codex_active=1`. This complements
#   phase 1's `cli-codex` for CLI users / the OpenAI ChatGPT VS Code
#   extension (whose bundled `codex app-server` lives outside
#   `/Applications/` and is already matched as `cli-codex`).
#
# Hard exclusions still apply, matched as substrings on the full command:
#   /Applications/*          (excluded — Codex.app main is carved out first)
#   */Helper*                (Electron helpers)
#   *crashpad*               (crash reporters)
#   *chrome-native-host*     (Chrome MCP bridge)
#   *node_repl*              (Codex.app's bundled Node REPL)
#
# Why two `ps` columns. BSD ps prints `command=` without quoting paths
# that contain literal spaces, and the first whitespace-separated token
# is therefore not the executable when the exe path has a space —
# notably Claude.app's bundled CC at
# `~/Library/Application Support/Claude/claude-code/<ver>/claude.app/Contents/MacOS/claude`
# (phase-3 experiment 2026-05-25). `ps -o comm=` on macOS prints the
# full executable path (or the bare basename for PATH-style invocations
# like a shell-launched `claude`) WITHOUT arguments — exactly the
# space-safe "exe" we need. The two columns are joined by pid; vigil
# uses comm for the exe path and command for path-based exclusions and
# arg recovery.
#
# Output format (tab-separated, one match per line, on stdout):
#   <pid>\t<name>\t<exe>\t<args>
# where <name> is one of: cli-claude, cli-codex, cli-copilot, app-codex.

# shellcheck source=common.sh
source "${VIGIL_LIB_DIR:-$(dirname "${BASH_SOURCE[0]}")}/common.sh"

# Phase-1 CLI agents we recognise. Adding a new one is a one-line case below.
VIGIL_CLI_AGENTS=("claude" "codex" "copilot")

# Exclusions on the FULL command line (substring match). Substring
# matching here is intentional — applying the same patterns to just the
# exe would re-introduce the spaced-path parsing bug fixed in phase 3.
_vigil_is_excluded_cmd() {
    local cmd="$1"
    case "$cmd" in
        /Applications/*)         return 0 ;;
        */Helper*)               return 0 ;;
        *crashpad*)              return 0 ;;
        *chrome-native-host*)    return 0 ;;
        *node_repl*)             return 0 ;;
    esac
    return 1
}

# Recover argv from (comm, command_line). comm is the exe path (or bare
# basename for PATH invocations); strip it (plus a trailing space) from
# the front of command_line to get args. Handles the no-args case where
# command_line == comm.
_vigil_args_from_command() {
    local comm="$1" command_line="$2"
    if [[ "$command_line" == "$comm" ]]; then
        printf '\n'
        return 0
    fi
    if [[ "$command_line" == "$comm "* ]]; then
        printf '%s\n' "${command_line#"$comm "}"
        return 0
    fi
    # Mismatch — ps may have shown a slightly different argv head than
    # comm (rare; can happen if the process re-exec'd between the two ps
    # samples). Fall back to empty args; exe remains comm.
    printf '\n'
}

# Match a single (pid, comm, command_line) triple. Echoes a TSV row on
# match, nothing otherwise.
#   pid          — process id (string)
#   comm         — exe path from `ps -o comm=`. Full path (e.g.
#                  /Applications/Codex.app/Contents/MacOS/Codex) when the
#                  process was launched with an explicit path, or just
#                  the basename (e.g. "claude") for PATH-style invocation.
#                  Space-safe in both cases.
#   command_line — full argv from `ps -o command=`. May contain literal
#                  spaces in the exe portion; never trust the first
#                  whitespace split.
vigil_detect_line() {
    local pid="$1" comm="$2" command_line="$3"
    local basename="${comm##*/}"

    # Phase-3 special case: Codex.app main process. Matched on any path
    # ending in `/Codex.app/Contents/MacOS/Codex`, so a per-user install
    # under `~/Applications/...` or `/Volumes/<external>/Codex.app/...`
    # is detected the same as a system install at `/Applications/...`.
    # `Codex Helper` (and `Codex Helper (Renderer)` etc.) have basename
    # "Codex Helper" — won't match this suffix glob. Carved out BEFORE
    # the /Applications/* exclusion so the desktop-app host anchor is
    # preserved for the canonical /Applications/Codex.app/... case.
    # Assumes Codex.app ships a single MacOS binary at this exact
    # bundle-relative path; if upstream restructures (e.g. an
    # arch-specific thin wrapper), this match needs to be updated.
    if [[ "$comm" == */Codex.app/Contents/MacOS/Codex ]]; then
        local args; args=$(_vigil_args_from_command "$comm" "$command_line")
        printf '%s\t%s\t%s\t%s\n' "$pid" "app-codex" "$comm" "$args"
        return 0
    fi

    if _vigil_is_excluded_cmd "$command_line"; then
        return 0
    fi

    case "$basename" in
        claude|codex|copilot)
            local args; args=$(_vigil_args_from_command "$comm" "$command_line")
            printf '%s\t%s\t%s\t%s\n' "$pid" "cli-$basename" "$comm" "$args"
            ;;
    esac
}

# Join two ps streams (pid+comm and pid+command) into joined TSV rows
# `<pid>\t<comm>\t<command_line>`. Both arguments are the full ps text
# (multi-line strings). Awk handles the pid-keyed inner join; comm
# returned by macOS ps may contain spaces (e.g. "Codex Helper" or
# "/Users/.../Application Support/.../claude"), so we use a strict
# pid-prefix parse rather than splitting on whitespace.
_vigil_ps_join() {
    local comm_text="$1" cmd_text="$2"
    awk '
        function trim(s)   { sub(/^[ \t]+/, "", s); sub(/[ \t]+$/, "", s); return s }
        function head(s)   { match(s, /^[0-9]+/); return substr(s, RSTART, RLENGTH) }
        function tail(s,p) { return substr(s, length(p)+1) }
        FNR == NR {
            line = trim($0)
            pid  = head(line)
            if (pid == "") next
            rest = tail(line, pid)
            sub(/^[ \t]+/, "", rest)
            comm[pid] = rest
            next
        }
        {
            line = trim($0)
            pid  = head(line)
            if (pid == "") next
            rest = tail(line, pid)
            sub(/^[ \t]+/, "", rest)
            if (pid in comm) {
                printf "%s\t%s\t%s\n", pid, comm[pid], rest
            }
        }
    ' <(printf '%s\n' "$comm_text") <(printf '%s\n' "$cmd_text")
}

# Iterate matches.
#   no args / "-": samples the live system (two ps invocations, joined).
#   two files (comm, command): reads each, joins, then matches.
# A single-arg "pre-joined fixture" mode was considered and dropped — no
# caller exists, and silently accepting a phase-1-shaped single fixture
# would produce zero output (the row parser expects three tab fields).
vigil_detect_all() {
    local joined
    if [[ $# -eq 0 || "$1" == "-" ]]; then
        local comm_text cmd_text
        comm_text=$(ps -axww -o pid= -o comm= 2>/dev/null)
        cmd_text=$(ps -axww -o pid= -o command= 2>/dev/null)
        joined=$(_vigil_ps_join "$comm_text" "$cmd_text")
    elif [[ $# -eq 2 ]]; then
        local comm_text cmd_text
        comm_text=$(<"$1")
        cmd_text=$(<"$2")
        joined=$(_vigil_ps_join "$comm_text" "$cmd_text")
    else
        return 2
    fi

    while IFS=$'\t' read -r pid comm command_line; do
        [[ -z "$pid" ]] && continue
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        vigil_detect_line "$pid" "$comm" "$command_line"
    done <<< "$joined"
}
