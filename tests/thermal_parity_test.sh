#!/usr/bin/env bash
# tests/thermal_parity_test.sh — Cross-engine thermal cutoff oracle.
#
# Asserts that the bash `vigil_thermal_should_cut` EXIT CODE agrees with the
# Rust `vigil debug thermal` output (cut/nocut) over a fixture table covering
# every branch, with VIGIL_THERMAL_CPU_LIMIT_FLOOR left UNSET (the parity
# contract: a default-config Rust run and bash agree on every fixture).
#
# The SET-floor smarter policy (CPU_Scheduler_Limit numeric < F) has NO bash
# counterpart and is proven ONLY by cargo unit tests in src/thermal — it is
# deliberately NOT exercised here, because asserting a smarter-than-bash decision
# against bash would be a false parity failure.
#
# Mirrors tests/detect_parity_test.sh: build the Rust bin on demand, run both
# engines under the SAME env, compare. Globbed by tests/run.sh via
# tests/*_test.sh — do NOT rename this file.

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

# Run the bash oracle: source thermal.sh, call vigil_thermal_should_cut under the
# given fixture/force env. Returns the exit code (0 = cut, 1 = nocut).
_bash_thermal_cut() {
    local fixture="$1" force="$2"
    VIGIL_LIB_DIR="$VIGIL_REPO_ROOT/lib" \
    VIGIL_THERMAL_FIXTURE="$fixture" \
    VIGIL_FORCE="$force" \
        bash -c 'source "$VIGIL_LIB_DIR/thermal.sh"; vigil_thermal_should_cut'
}

# Run the Rust oracle: vigil debug thermal under the same env. Prints cut|nocut.
# The floor knob is intentionally left UNSET (parity contract).
_rust_thermal_cut() {
    local fixture="$1" force="$2"
    VIGIL_THERMAL_FIXTURE="$fixture" \
    VIGIL_FORCE="$force" \
    VIGIL_CONFIG_FILE="$VIGIL_REPO_ROOT/tests/fixtures/does-not-exist.conf" \
        "$VIGIL_RUST_BIN" debug thermal
}

# Assert bash exit-code-to-cut/nocut AGREES with rust stdout for one fixture.
_assert_thermal_parity() {
    local label="$1" fixture="$2" force="${3:-0}"
    local bash_rc bash_decision rust_decision
    _bash_thermal_cut "$fixture" "$force"; bash_rc=$?
    if [[ "$bash_rc" -eq 0 ]]; then bash_decision="cut"; else bash_decision="nocut"; fi
    rust_decision=$(_rust_thermal_cut "$fixture" "$force")
    if [[ "$bash_decision" == "$rust_decision" ]]; then
        return 0
    fi
    printf '    DIFF for %s: bash=%s rust=%s (fixture=%q force=%q)\n' \
        "$label" "$bash_decision" "$rust_decision" "$fixture" "$force"
    return 1
}

test_thermal_parity_no_warning() {
    _require_rust_bin || return 1
    _assert_thermal_parity "no-warning informational" \
        "Note: No thermal warning level has been recorded
Note: No performance warning level has been recorded
Note: No CPU power status has been recorded"
}

test_thermal_parity_warning_cuts() {
    _require_rust_bin || return 1
    _assert_thermal_parity "thermal warning" "thermal warning level = warning"
}

test_thermal_parity_scheduler_limit_cuts() {
    _require_rust_bin || return 1
    _assert_thermal_parity "scheduler limit multiline" \
        "CPU_Scheduler_Limit = 50
CPU_Available_CPUs = 4"
}

test_thermal_parity_empty_no_cut() {
    _require_rust_bin || return 1
    _assert_thermal_parity "empty fixture" ""
}

test_thermal_parity_force_overrides() {
    _require_rust_bin || return 1
    _assert_thermal_parity "force override" "thermal warning level = critical" "1"
}

# Whitespace-padded keyword line — bash's [[:space:]]* allowances must agree
# with the hand-rolled Rust anchor.
test_thermal_parity_whitespace_padding() {
    _require_rust_bin || return 1
    _assert_thermal_parity "whitespace padding" "  CPU_Scheduler_Limit  = 50"
}

# Keyword mentioned WITHOUT an '=' — the false-positive guard. Must be nocut on
# both engines.
test_thermal_parity_keyword_without_equals() {
    _require_rust_bin || return 1
    _assert_thermal_parity "keyword no equals" \
        "Note: No CPU_Scheduler_Limit has been recorded"
}
