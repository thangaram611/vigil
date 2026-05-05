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
        # Don't depend on jq — keep deps minimal. Greppable JSON shape.
        local v
        v=$(awk -F'[:,}]' '/SleepDisabled/ {gsub(/[" ]/,"",$2); print $2; exit}' "$VIGIL_BASELINE_FILE" 2>/dev/null)
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
