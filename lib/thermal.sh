#!/usr/bin/env bash
# lib/thermal.sh — parse `pmset -g therm` for thermal cutoff signal.
#
# Returns 0 (true) when the system is reporting thermal pressure that should
# cause vigil to release sleep prevention; 1 otherwise.
#
# Logic borrowed from CharlonTank/agents-sleep-preventer src/main.rs:
#   - Look for "CPU_Scheduler_Limit" not preceded by "No CPU"
#   - OR "thermal warning level"
#
# Override entirely with VIGIL_FORCE=1.

# shellcheck source=common.sh
source "${VIGIL_LIB_DIR:-$(dirname "${BASH_SOURCE[0]}")}/common.sh"

# Allow tests to inject canned `pmset -g therm` output.
_vigil_pmset_therm() {
    if [[ -n "${VIGIL_THERMAL_FIXTURE:-}" ]]; then
        printf '%s\n' "$VIGIL_THERMAL_FIXTURE"
    else
        pmset -g therm 2>/dev/null
    fi
}

# Returns 0 if thermal cutoff should engage.
#
# `pmset -g therm` reports either "Note: No <field> has been recorded" lines (no
# pressure) or actual `<field> = <value>` lines (active pressure). We require an
# `=` separator so the "No ..." informational lines don't false-positive.
vigil_thermal_should_cut() {
    [[ "${VIGIL_FORCE:-0}" == "1" ]] && return 1
    local out; out=$(_vigil_pmset_therm)
    [[ -z "$out" ]] && return 1
    # Match a line that has either CPU_Scheduler_Limit or "thermal warning level"
    # followed (eventually) by `=`. Anchored with leading optional whitespace; the
    # "Note: No ..." lines never have `=` so they won't match.
    if printf '%s\n' "$out" | grep -qE '^[[:space:]]*(CPU_Scheduler_Limit|thermal warning level)[[:space:]]*='; then
        return 0
    fi
    return 1
}

# Pretty summary of current thermal state for `vigil status`.
vigil_thermal_summary() {
    local out; out=$(_vigil_pmset_therm)
    if [[ -z "$out" ]]; then
        printf 'unavailable\n'
    elif vigil_thermal_should_cut; then
        printf 'cutoff (%s)\n' "$(printf '%s' "$out" | tr '\n' ' ' | sed 's/  */ /g' | head -c 100)"
    else
        printf 'ok\n'
    fi
}
