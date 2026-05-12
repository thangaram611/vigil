#!/usr/bin/env bash
# lib/pmset.sh — capture/restore baseline SleepDisabled, plus enable/disable transitions.
#
# Why baseline state matters: if Amphetamine or anything else already had
# disablesleep=1 when vigil engages, we must not clobber it back to 0 on release.
# We snapshot the prior value at first acquire and restore exactly that on last release.

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

vigil_pmset_clear_baseline() {
    rm -f "$VIGIL_BASELINE_FILE"
}

# ---- transitions --------------------------------------------------------------

# 0 → >0 transition. Captures baseline, sets disablesleep=1, spawns caffeinate -di.
vigil_pmset_engage() {
    vigil_pmset_capture_baseline
    if ! sudo_n_pmset_disablesleep 1; then
        log ERROR "engage failed — pmset rejected disablesleep=1"
        return 1
    fi
    # Spawn caffeinate -di if not already running.
    if [[ -f "$VIGIL_CAFFEINATE_PIDFILE" ]] && kill -0 "$(cat "$VIGIL_CAFFEINATE_PIDFILE" 2>/dev/null)" 2>/dev/null; then
        return 0
    fi
    caffeinate -di &
    echo $! > "$VIGIL_CAFFEINATE_PIDFILE"
    log INFO "spawned caffeinate -di pid=$(cat "$VIGIL_CAFFEINATE_PIDFILE")"
}

# >0 → 0 transition. Restores baseline value, kills caffeinate child.
vigil_pmset_release() {
    local target; target=$(vigil_pmset_baseline_value)
    if ! sudo_n_pmset_disablesleep "$target"; then
        log ERROR "release failed — pmset rejected disablesleep=$target"
        # Don't return early — still try to clean up caffeinate child.
    fi
    if [[ -f "$VIGIL_CAFFEINATE_PIDFILE" ]]; then
        local cpid; cpid=$(cat "$VIGIL_CAFFEINATE_PIDFILE" 2>/dev/null)
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
    sudo_n_pmset_disablesleep "$target" || true
    if [[ -f "$VIGIL_CAFFEINATE_PIDFILE" ]]; then
        local cpid; cpid=$(cat "$VIGIL_CAFFEINATE_PIDFILE" 2>/dev/null)
        [[ -n "$cpid" ]] && kill "$cpid" 2>/dev/null || true
        rm -f "$VIGIL_CAFFEINATE_PIDFILE"
    fi
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
    [[ -f "$VIGIL_CAFFEINATE_PIDFILE" ]] && our_pid=$(cat "$VIGIL_CAFFEINATE_PIDFILE" 2>/dev/null)

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
