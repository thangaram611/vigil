#!/usr/bin/env bash
# tests/battery_test.sh — verify battery guard against synthetic outputs.

VIGIL_LIB_DIR="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/lib"
# shellcheck source=../lib/battery.sh
source "$VIGIL_LIB_DIR/battery.sh"

_ac_100=$'Now drawing from \'AC Power\'\n -InternalBattery-0 (id=1)\t100%; charged; 0:00 remaining present: true'
_battery_5=$'Now drawing from \'Battery Power\'\n -InternalBattery-0 (id=1)\t5%; discharging; 0:30 remaining present: true'
_battery_50=$'Now drawing from \'Battery Power\'\n -InternalBattery-0 (id=1)\t50%; discharging; 5:00 remaining present: true'

test_ac_does_not_cut() {
    if VIGIL_BATTERY_FIXTURE="$_ac_100" vigil_battery_should_cut; then
        echo "    FAIL: AC power 100% should not trigger cutoff"
        return 1
    fi
}

test_battery_low_cuts() {
    if ! VIGIL_BATTERY_FIXTURE="$_battery_5" vigil_battery_should_cut; then
        echo "    FAIL: battery 5% should trigger cutoff"
        return 1
    fi
}

test_battery_50_does_not_cut() {
    if VIGIL_BATTERY_FIXTURE="$_battery_50" vigil_battery_should_cut; then
        echo "    FAIL: battery 50% should not trigger cutoff (default floor 20%)"
        return 1
    fi
}

test_force_overrides_battery_low() {
    if VIGIL_FORCE=1 VIGIL_BATTERY_FIXTURE="$_battery_5" vigil_battery_should_cut; then
        echo "    FAIL: VIGIL_FORCE=1 should bypass battery cutoff"
        return 1
    fi
}

test_pct_parser() {
    local pct
    pct=$(VIGIL_BATTERY_FIXTURE="$_ac_100" vigil_battery_pct)
    assert_eq "$pct" "100" "AC fixture pct"
    pct=$(VIGIL_BATTERY_FIXTURE="$_battery_5" vigil_battery_pct)
    assert_eq "$pct" "5" "5% fixture pct"
}

test_ac_summary_says_ac() {
    local out; out=$(VIGIL_BATTERY_FIXTURE="$_ac_100" vigil_battery_summary)
    assert_contains "$out" "AC" "summary on AC should say AC"
}

test_battery_summary_says_battery() {
    local out; out=$(VIGIL_BATTERY_FIXTURE="$_battery_50" vigil_battery_summary)
    assert_contains "$out" "battery" "summary on battery should say battery"
}
