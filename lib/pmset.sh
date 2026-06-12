#!/usr/bin/env bash
# lib/pmset.sh — sleep-prevention transitions.
#
# Vigil's macOS hold is intentionally unified:
#   - pmset disablesleep=1 for best-effort system/lid sleep prevention
#   - caffeinate -i for user-idle system sleep
#   - no display assertion, so native lock and display sleep remain undisturbed

# shellcheck source=common.sh
source "${VIGIL_LIB_DIR:-$(dirname "${BASH_SOURCE[0]}")}/common.sh"

# ---- baseline -----------------------------------------------------------------

# Capture current SleepDisabled and persist into baseline.json.
# Idempotent: if baseline.json already exists, leaves it alone.
vigil_pmset_capture_baseline() {
    if [[ -f "$VIGIL_BASELINE_FILE" ]]; then
        return 0
    fi
    local prior; prior=$(vigil_read_sleepdisabled)
    local ts; ts=$(vigil_now_unix)
    printf '{"SleepDisabled":%s,"captured_at":%s}\n' "$prior" "$ts" > "$VIGIL_BASELINE_FILE"
    log INFO "captured baseline SleepDisabled=$prior"
}

# Read baseline value. Returns "0" or "1". Defaults to "0" if missing.
vigil_pmset_baseline_value() {
    if [[ -f "$VIGIL_BASELINE_FILE" ]]; then
        # Use refcount.sh's parser if loaded; otherwise inline parameter-expansion.
        local v rest content
        if declare -F _vigil_pidfile_field >/dev/null 2>&1; then
            v=$(_vigil_pidfile_field "$VIGIL_BASELINE_FILE" "SleepDisabled")
        else
            content=$(<"$VIGIL_BASELINE_FILE")
            rest="${content#*\"SleepDisabled\":}"
            v="${rest%%,*}"; v="${v%%\}*}"
        fi
        case "$v" in 0|1) printf '%s\n' "$v"; return 0 ;; esac
    fi
    printf '0\n'
}

vigil_pmset_active_mode() {
    printf 'best-effort\n'
}

vigil_pmset_clear_baseline() {
    rm -f "$VIGIL_BASELINE_FILE"
}

# ---- transitions --------------------------------------------------------------

vigil_pmset_caffeinate_pid() {
    [[ -f "$VIGIL_CAFFEINATE_PIDFILE" ]] || return 1
    local cpid; cpid=$(cat "$VIGIL_CAFFEINATE_PIDFILE" 2>/dev/null)
    [[ "$cpid" =~ ^[0-9]+$ ]] || return 1
    printf '%s\n' "$cpid"
}

vigil_pmset_caffeinate_alive() {
    local cpid; cpid=$(vigil_pmset_caffeinate_pid) || return 1
    kill -0 "$cpid" 2>/dev/null || return 1
    local cmd exe base
    cmd=$(ps -p "$cpid" -o command= 2>/dev/null | sed 's/^ *//')
    [[ -n "$cmd" ]] || return 1
    exe="${cmd%% *}"
    base="${exe##*/}"
    [[ "$base" == "caffeinate" ]] || return 1
    # Older Vigil used `caffeinate -di`, which held a display assertion. Treat
    # any display-holding caffeinate as stale so the next reconcile replaces it.
    [[ ! "$cmd" =~ (^|[[:space:]])-[A-Za-z]*d[A-Za-z]*($|[[:space:]]) ]]
}

vigil_pmset_spawn_caffeinate() {
    if vigil_pmset_caffeinate_alive; then
        return 0
    fi
    local old_pid="" old_cmd="" old_exe="" old_base=""
    old_pid=$(vigil_pmset_caffeinate_pid 2>/dev/null || true)
    if [[ -n "$old_pid" ]] && kill -0 "$old_pid" 2>/dev/null; then
        old_cmd=$(ps -p "$old_pid" -o command= 2>/dev/null | sed 's/^ *//' || true)
        old_exe="${old_cmd%% *}"
        old_base="${old_exe##*/}"
        if [[ "$old_base" == "caffeinate" ]]; then
            kill "$old_pid" 2>/dev/null || true
            log INFO "replaced stale/display-holding caffeinate pid=$old_pid"
        fi
    fi
    rm -f "$VIGIL_CAFFEINATE_PIDFILE"
    caffeinate -i &
    echo $! > "$VIGIL_CAFFEINATE_PIDFILE"
    log INFO "spawned caffeinate -i pid=$(cat "$VIGIL_CAFFEINATE_PIDFILE")"
}

# Reassert the live engaged state after drift. This is used while already
# engaged and during crash recovery, so it intentionally does not capture or
# clear baseline state.
vigil_pmset_reconcile_engaged() {
    local sd; sd=$(vigil_read_sleepdisabled)
    if [[ "$sd" != "1" ]]; then
        log WARN "SleepDisabled drifted to $sd while engaged — reasserting"
        vigil_power_engage || return 1
    fi
    if ! vigil_pmset_caffeinate_alive; then
        log WARN "caffeinate assertion missing while engaged — restarting"
        vigil_pmset_spawn_caffeinate || return 1
    fi
}

# Startup recovery for a daemon restart after an unclean exit. If active work
# still exists, keep the original baseline and reassert. Otherwise restore.
# Returns 0 when the caller should treat the daemon as engaged, 1 otherwise.
vigil_pmset_recover_startup() {
    local active_count="$1" can_hold="${2:-1}"
    [[ -f "$VIGIL_BASELINE_FILE" || -f "$VIGIL_CAFFEINATE_PIDFILE" ]] || return 1
    if [[ ! -f "$VIGIL_BASELINE_FILE" && -f "$VIGIL_CAFFEINATE_PIDFILE" ]]; then
        log WARN "caffeinate pidfile present without baseline — recapturing baseline"
        vigil_pmset_capture_baseline
    fi
    if (( active_count > 0 && can_hold == 1 )); then
        log WARN "baseline.json present at startup and active refs remain — recovering engaged state"
        vigil_pmset_reconcile_engaged || return 1
        return 0
    fi
    log WARN "baseline.json present at startup with no active refs — restoring prior state"
    vigil_pmset_release || true
    return 1
}

# 0 → >0 transition. Captures baseline, sets SleepDisabled=1, and spawns
# caffeinate -i without holding the display awake.
vigil_pmset_engage() {
    vigil_pmset_capture_baseline
    if ! vigil_power_engage; then
        log ERROR "engage failed — pmset rejected disablesleep=1"
        return 1
    fi
    vigil_pmset_spawn_caffeinate
}

# >0 → 0 transition. Restores baseline SleepDisabled, then kills the
# caffeinate child.
vigil_pmset_release() {
    local target; target=$(vigil_pmset_baseline_value)
    if ! vigil_power_release; then
        log ERROR "release failed — pmset rejected disablesleep=$target"
        # Don't return early — still try to clean up caffeinate child.
    fi
    if [[ -f "$VIGIL_CAFFEINATE_PIDFILE" ]]; then
        local cpid; cpid=$(vigil_pmset_caffeinate_pid 2>/dev/null || true)
        if [[ -n "$cpid" ]] && kill -0 "$cpid" 2>/dev/null; then
            kill "$cpid" 2>/dev/null || true
            log INFO "killed caffeinate pid=$cpid"
        fi
        rm -f "$VIGIL_CAFFEINATE_PIDFILE"
    fi
    vigil_pmset_clear_baseline
}

# Soft release used by the thermal cutoff path: drops sleep prevention WITHOUT
# clearing baseline, so when thermal recovers we can re-engage and still know
# the original state.
vigil_pmset_soft_release() {
    local target; target=$(vigil_pmset_baseline_value)
    vigil_power_release || log WARN "soft release failed — pmset rejected disablesleep=$target"
    if [[ -f "$VIGIL_CAFFEINATE_PIDFILE" ]]; then
        local cpid; cpid=$(vigil_pmset_caffeinate_pid 2>/dev/null || true)
        [[ -n "$cpid" ]] && kill -0 "$cpid" 2>/dev/null && kill "$cpid" 2>/dev/null || true
        rm -f "$VIGIL_CAFFEINATE_PIDFILE"
    fi
}

vigil_pmset_hold_engaged() {
    vigil_pmset_caffeinate_alive && return 0
    if [[ -f "$VIGIL_BASELINE_FILE" ]]; then
        [[ "$(vigil_read_sleepdisabled)" == "1" ]] && return 0
    fi
    return 1
}

# ---- power assertions summary -------------------------------------------------
#
# Parses `pmset -g assertions` and emits one of three things, in priority order:
#
#   1. One TSV row per matched assertion-holder:
#          <pid>\t<process>\t<assertion-type>[\t← vigil]
#      The "← vigil" suffix marks our own caffeinate child (PID matches
#      $VIGIL_CAFFEINATE_PIDFILE). cmd_status renders these for the user.
#
#   2. A literal `(none)` line when there are no holders:
#        - pmset output is empty / unavailable, OR
#        - the "Listed by owning process:" block is absent, OR
#        - the block is present but contains only header/"No assertions"/blank
#          rows (zero non-blank, non-informational rows under it).
#
#   3. A literal `(parse-failed; raw output:)` line followed by the first ~10
#      lines of `pmset -g assertions` output, when the block IS present and
#      contains ≥1 non-blank, non-informational row but NONE of those rows
#      match the expected `pid <num>(<name>):` pattern. This is the early-
#      warning signal for Apple changing the output schema. When you see this:
#      add a new fixture to tests/assertions_test.sh and adjust the parser.
#
# Brittleness caveat: `man pmset` does NOT define a machine-stable schema for
# the "Listed by owning process:" block. The shape parsed here is the format
# observed across macOS 13–15:
#     pid <num>(<process-name>): [<hex-id>] <HH:MM:SS> <AssertionType> named: "..."
# with `Details:` / `Timeout will fire in...` continuation lines indented
# further. Any line whose first non-whitespace token is not literally "pid"
# is treated as a continuation and silently skipped (no parse-failure bump).
#
# This parser is BEST-EFFORT. The tri-state contract is the user-facing API,
# not the regex.

# Allow tests to inject canned `pmset -g assertions` output. We check for the
# variable being *set* (not just non-empty) so that an intentional empty
# fixture — used to exercise the "pmset returned nothing" branch — doesn't
# fall through and leak real-system assertions into the test.
_vigil_pmset_assertions() {
    if [[ "${VIGIL_ASSERTIONS_FIXTURE+set}" == "set" ]]; then
        printf '%s\n' "$VIGIL_ASSERTIONS_FIXTURE"
    else
        pmset -g assertions 2>/dev/null
    fi
}

vigil_assertions_summary() {
    local raw; raw=$(_vigil_pmset_assertions)
    if [[ -z "$raw" ]]; then
        printf '(none)\n'
        return 0
    fi

    # Header absent → no holders to enumerate.
    if ! printf '%s\n' "$raw" | grep -q '^Listed by owning process:'; then
        printf '(none)\n'
        return 0
    fi

    # Slice out the block. Ends at "No new entries", a fresh "Assertion status"
    # section, or EOF. Tolerates intra-block blank lines.
    #
    # LC_ALL=C: pmset assertion names sometimes contain non-ASCII (e.g. a Unicode
    # apostrophe in "John's Magic Mouse"). BSD awk under a UTF-8 locale errors
    # out on invalid byte sequences with "multibyte conversion failure". Forcing
    # the C locale makes awk treat the input as bytes — we don't do any
    # character-class operations, only literal-string anchors.
    local block
    block=$(printf '%s\n' "$raw" | LC_ALL=C awk '
        /^Listed by owning process:/ { flag=1; next }
        /^No new entries/            { flag=0 }
        /^Assertion status/          { flag=0 }
        flag                          { print }
    ')

    local our_pid=""
    our_pid=$(vigil_pmset_caffeinate_pid 2>/dev/null || true)

    local matched=0 non_matching=0
    local pid_re='^[[:space:]]*pid[[:space:]]+([0-9]+)\(([^)]+)\):.*\][[:space:]]+[0-9:]+[[:space:]]+([A-Za-z]+)'
    # Continuation-line gate: under real pmset output, holder rows are indented
    # 2 spaces ("  pid X(name): ...") and continuation rows (`Details:`,
    # `Timeout will fire ...`) are indented further (typically 4+ spaces). We
    # use 4+ leading whitespace chars as the cutoff. Lines with less leading
    # whitespace are "candidate" rows that must either match the regex
    # (matched) or be flagged as schema drift (non_matching).
    local cont_re='^[[:space:]]{4,}'
    local out=""
    local line trimmed pid proc atype suffix

    while IFS= read -r line; do
        [[ -z "${line// /}" ]] && continue
        trimmed="${line#"${line%%[![:space:]]*}"}"
        # Informational "No new entries" / "No assertions" — skip silently.
        case "$trimmed" in
            'No '*) continue ;;
        esac
        # Deeply-indented = continuation of the previous holder row.
        if [[ "$line" =~ $cont_re ]]; then
            continue
        fi

        if [[ "$line" =~ $pid_re ]]; then
            pid="${BASH_REMATCH[1]}"
            proc="${BASH_REMATCH[2]}"
            atype="${BASH_REMATCH[3]}"
            suffix=""
            [[ -n "$our_pid" && "$pid" == "$our_pid" ]] && suffix=$'\t← vigil'
            out+="${pid}	${proc}	${atype}${suffix}"$'\n'
            matched=$((matched+1))
        else
            # Block-level row that doesn't match expected shape — schema drift.
            non_matching=$((non_matching+1))
        fi
    done <<< "$block"

    if (( matched > 0 )); then
        printf '%s' "$out"
        return 0
    fi
    if (( non_matching > 0 )); then
        printf '(parse-failed; raw output:)\n'
        printf '%s\n' "$raw" | head -n 10 | sed 's/^/  /'
        return 0
    fi
    printf '(none)\n'
}
