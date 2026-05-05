#!/usr/bin/env bash
# lib/battery.sh — battery guard. Don't disable sleep on a near-dead laptop on battery.
#
# Logic:
#   - On AC power: never cut off based on battery.
#   - On battery: cut off if level < VIGIL_BATTERY_FLOOR_PCT (default 20).
#   - VIGIL_FORCE=1 overrides everything.

# shellcheck source=common.sh
source "${VIGIL_LIB_DIR:-$(dirname "${BASH_SOURCE[0]}")}/common.sh"

_vigil_pmset_ps() {
    if [[ -n "${VIGIL_BATTERY_FIXTURE:-}" ]]; then
        printf '%s\n' "$VIGIL_BATTERY_FIXTURE"
    else
        pmset -g ps 2>/dev/null
    fi
}

# Returns 0 (true) iff drawing from battery, 1 if AC, 2 if unknown.
vigil_battery_on_battery() {
    local out; out=$(_vigil_pmset_ps)
    case "$out" in
        *"AC Power"*)      return 1 ;;
        *"Battery Power"*) return 0 ;;
        *)                 return 2 ;;
    esac
}

# Battery percentage (integer). Empty if unparseable.
vigil_battery_pct() {
    local out; out=$(_vigil_pmset_ps)
    # Sample line: " -InternalBattery-0 (id=...)\t96%; discharging; ... present: true"
    printf '%s\n' "$out" | awk '
        match($0, /[0-9]+%/) {
            pct = substr($0, RSTART, RLENGTH-1)
            print pct
            exit
        }
    '
}

# Returns 0 if the battery guard should cut off sleep prevention now.
vigil_battery_should_cut() {
    [[ "${VIGIL_FORCE:-0}" == "1" ]] && return 1
    if ! vigil_battery_on_battery; then
        return 1  # AC or unknown -> don't cut for battery reasons
    fi
    local pct; pct=$(vigil_battery_pct)
    [[ -z "$pct" ]] && return 1
    if (( pct < VIGIL_BATTERY_FLOOR_PCT )); then
        return 0
    fi
    return 1
}

# Pretty summary for `vigil status`.
vigil_battery_summary() {
    local pct rc=0
    pct=$(vigil_battery_pct)
    # `|| rc=$?` keeps set -e from killing us when the function returns non-zero.
    vigil_battery_on_battery || rc=$?
    case "$rc" in
        0) printf 'battery %s%% (cutoff floor %s%%)\n' "${pct:-?}" "$VIGIL_BATTERY_FLOOR_PCT" ;;
        1) printf 'AC %s%%\n' "${pct:-?}" ;;
        *) printf 'unknown\n' ;;
    esac
}
