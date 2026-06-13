#!/usr/bin/env bash
# tests/battery_parity_test.sh — Cross-engine battery cutoff oracle.
#
# Asserts that the bash `vigil_battery_should_cut` EXIT CODE agrees with the
# Rust `vigil debug battery` output (cut/nocut) over a fixture table covering
# every branch (AC/battery/unknown, boundary 20%, sub-floor, empty pct, force),
# under a matched VIGIL_BATTERY_FIXTURE/VIGIL_FORCE/VIGIL_BATTERY_FLOOR_PCT env.
#
# The floor knob (VIGIL_BATTERY_FLOOR_PCT) is a SHARED bash+Rust knob (both read
# it), so the table may set it; the new Rust-only VIGIL_THERMAL_CPU_LIMIT_FLOOR
# is a thermal concern and irrelevant here. The TOCTOU fix (one pmset read) does
# not change the cut decision, so the oracle still agrees.
#
# Mirrors tests/detect_parity_test.sh: build the Rust bin on demand, run both
# engines under the SAME env, compare. Globbed by tests/run.sh.

set -uo pipefail

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
VIGIL_RUST_BIN="$VIGIL_REPO_ROOT/target/debug/vigil"

_require_rust_bin() {
    if [[ ! -x "$VIGIL_RUST_BIN" ]]; then
        ( cargo build --quiet --manifest-path "$VIGIL_REPO_ROOT/crates/vigil/Cargo.toml" ) \
            || { printf '    FAIL: could not build vigil rust binary\n'; return 1; }
    fi
    [[ -x "$VIGIL_RUST_BIN" ]]
}

# Run the bash oracle: source battery.sh + common.sh (for the default floor),
# call vigil_battery_should_cut. Returns exit code (0 = cut, 1 = nocut).
_bash_battery_cut() {
    local fixture="$1" force="$2" floor="$3"
    VIGIL_LIB_DIR="$VIGIL_REPO_ROOT/lib" \
    VIGIL_BATTERY_FIXTURE="$fixture" \
    VIGIL_FORCE="$force" \
    VIGIL_BATTERY_FLOOR_PCT="$floor" \
        bash -c 'source "$VIGIL_LIB_DIR/battery.sh"; vigil_battery_should_cut'
}

# Run the Rust oracle: vigil debug battery. Prints cut|nocut.
_rust_battery_cut() {
    local fixture="$1" force="$2" floor="$3"
    VIGIL_BATTERY_FIXTURE="$fixture" \
    VIGIL_FORCE="$force" \
    VIGIL_BATTERY_FLOOR_PCT="$floor" \
    VIGIL_CONFIG_FILE="$VIGIL_REPO_ROOT/tests/fixtures/does-not-exist.conf" \
        "$VIGIL_RUST_BIN" debug battery
}

# Assert bash exit-code-to-cut/nocut AGREES with rust stdout for one fixture.
_assert_battery_parity() {
    local label="$1" fixture="$2" force="${3:-0}" floor="${4:-20}"
    local bash_rc bash_decision rust_decision
    _bash_battery_cut "$fixture" "$force" "$floor"; bash_rc=$?
    if [[ "$bash_rc" -eq 0 ]]; then bash_decision="cut"; else bash_decision="nocut"; fi
    rust_decision=$(_rust_battery_cut "$fixture" "$force" "$floor")
    if [[ "$bash_decision" == "$rust_decision" ]]; then
        return 0
    fi
    printf '    DIFF for %s: bash=%s rust=%s (fixture=%q force=%q floor=%q)\n' \
        "$label" "$bash_decision" "$rust_decision" "$fixture" "$force" "$floor"
    return 1
}

# Fixtures byte-identical in shape to tests/battery_test.sh.
_ac_100=$'Now drawing from \'AC Power\'\n -InternalBattery-0 (id=1)\t100%; charged; 0:00 remaining present: true'
_battery_5=$'Now drawing from \'Battery Power\'\n -InternalBattery-0 (id=1)\t5%; discharging; 0:30 remaining present: true'
_battery_50=$'Now drawing from \'Battery Power\'\n -InternalBattery-0 (id=1)\t50%; discharging; 5:00 remaining present: true'
_battery_20=$'Now drawing from \'Battery Power\'\n -InternalBattery-0 (id=1)\t20%; discharging; 2:00 remaining present: true'
_battery_no_pct=$'Now drawing from \'Battery Power\'\n -InternalBattery-0 (id=1) present: true'

test_battery_parity_ac_no_cut() {
    _require_rust_bin || return 1
    _assert_battery_parity "AC 100%" "$_ac_100"
}

test_battery_parity_low_cuts() {
    _require_rust_bin || return 1
    _assert_battery_parity "battery 5%" "$_battery_5"
}

test_battery_parity_50_no_cut() {
    _require_rust_bin || return 1
    _assert_battery_parity "battery 50%" "$_battery_50"
}

# Boundary: exactly 20% with default floor 20 => NO cut (STRICT '<') on BOTH.
test_battery_parity_boundary_20() {
    _require_rust_bin || return 1
    _assert_battery_parity "battery 20% boundary" "$_battery_20"
}

test_battery_parity_force_overrides() {
    _require_rust_bin || return 1
    _assert_battery_parity "force override" "$_battery_5" "1"
}

# Unknown power source (neither AC nor Battery marker) => NO cut on both.
test_battery_parity_unknown_source() {
    _require_rust_bin || return 1
    _assert_battery_parity "unknown source" "some unrelated text 9%"
}

# On battery but no parseable percentage => NO cut (fail-safe) on both.
test_battery_parity_empty_pct() {
    _require_rust_bin || return 1
    _assert_battery_parity "empty pct" "$_battery_no_pct"
}
