#!/usr/bin/env bash
# tests/thermal_test.sh — verify thermal cutoff parser against synthetic outputs.

VIGIL_LIB_DIR="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/lib"
# shellcheck source=../lib/thermal.sh
source "$VIGIL_LIB_DIR/thermal.sh"

test_no_warning_does_not_cut() {
    local fixture="Note: No thermal warning level has been recorded
Note: No performance warning level has been recorded
Note: No CPU power status has been recorded"
    if VIGIL_THERMAL_FIXTURE="$fixture" vigil_thermal_should_cut; then
        echo "    FAIL: 'No thermal warning' false-positively triggered cutoff"
        return 1
    fi
}

test_thermal_warning_cuts() {
    local fixture="thermal warning level = warning"
    if ! VIGIL_THERMAL_FIXTURE="$fixture" vigil_thermal_should_cut; then
        echo "    FAIL: explicit thermal warning did NOT trigger cutoff"
        return 1
    fi
}

test_cpu_scheduler_limit_cuts() {
    local fixture="CPU_Scheduler_Limit = 50
CPU_Available_CPUs = 4"
    if ! VIGIL_THERMAL_FIXTURE="$fixture" vigil_thermal_should_cut; then
        echo "    FAIL: CPU_Scheduler_Limit did NOT trigger cutoff"
        return 1
    fi
}

test_force_overrides() {
    local fixture="thermal warning level = critical"
    if VIGIL_FORCE=1 VIGIL_THERMAL_FIXTURE="$fixture" vigil_thermal_should_cut; then
        echo "    FAIL: VIGIL_FORCE=1 should have bypassed the thermal cutoff"
        return 1
    fi
}

test_empty_output_does_not_cut() {
    if VIGIL_THERMAL_FIXTURE="" vigil_thermal_should_cut; then
        echo "    FAIL: empty fixture should not be a cutoff"
        return 1
    fi
}
