#!/usr/bin/env bash
# tests/assertions_test.sh — verify vigil_assertions_summary against canned
# `pmset -g assertions` outputs.
#
# The fixtures here are the early-warning signal for Apple changing the
# "Listed by owning process:" block schema. When parsing breaks against a
# real macOS version, capture that version's `pmset -g assertions` output
# and add a fixture for it.

VIGIL_LIB_DIR="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/lib"
# shellcheck source=../lib/pmset.sh
source "$VIGIL_LIB_DIR/pmset.sh"

# All fixture-driven tests must isolate VIGIL_CAFFEINATE_PIDFILE so a stale
# real-system caffeinate.pid doesn't leak in and false-positive the vigil tag.
_vigil_assertions_test_setup() {
    VIGIL_STATE_DIR=$(mktemp -d -t vigil-assertions-test)
    VIGIL_CAFFEINATE_PIDFILE="$VIGIL_STATE_DIR/caffeinate.pid"
}
_vigil_assertions_test_teardown() {
    [[ -n "${VIGIL_STATE_DIR:-}" && -d "$VIGIL_STATE_DIR" ]] && rm -rf "$VIGIL_STATE_DIR"
}

test_empty_output_is_none() {
    _vigil_assertions_test_setup
    local out; out=$(VIGIL_ASSERTIONS_FIXTURE="" vigil_assertions_summary)
    _vigil_assertions_test_teardown
    assert_eq "$out" "(none)" "empty pmset output → (none)"
}

test_header_only_is_none() {
    # Only the system-wide assertion table; no "Listed by owning process:" block.
    _vigil_assertions_test_setup
    local fixture='Assertion status system-wide:
   PreventUserIdleSystemSleep    0
   UserIsActive                  0'
    local out; out=$(VIGIL_ASSERTIONS_FIXTURE="$fixture" vigil_assertions_summary)
    _vigil_assertions_test_teardown
    assert_eq "$out" "(none)" "header-only output → (none)"
}

test_block_present_but_empty_is_none() {
    # The block header is there but contains zero non-blank rows.
    _vigil_assertions_test_setup
    local fixture='Assertion status system-wide:
   PreventUserIdleSystemSleep    0
Listed by owning process:
No new entries'
    local out; out=$(VIGIL_ASSERTIONS_FIXTURE="$fixture" vigil_assertions_summary)
    _vigil_assertions_test_teardown
    assert_eq "$out" "(none)" "block present but empty → (none)"
}

test_single_assertion_holder_is_tsv() {
    _vigil_assertions_test_setup
    local fixture='Assertion status system-wide:
   PreventUserIdleSystemSleep    1
Listed by owning process:
  pid 200(loginwindow): [0x000049930006d2eb] 00:00:00 PreventUserIdleSystemSleep named: "com.apple.loginwindow.assertion"
No new entries'
    local out; out=$(VIGIL_ASSERTIONS_FIXTURE="$fixture" vigil_assertions_summary)
    _vigil_assertions_test_teardown
    assert_contains "$out" $'200\tloginwindow\tPreventUserIdleSystemSleep' "single-holder TSV row"
    assert_not_contains "$out" "← vigil" "non-vigil pid should not be tagged"
    assert_not_contains "$out" "(none)" "should not be (none)"
}

test_multi_holder_with_continuation_lines() {
    _vigil_assertions_test_setup
    local fixture='Assertion status system-wide:
   PreventUserIdleSystemSleep    1
   PreventUserIdleDisplaySleep   1
Listed by owning process:
  pid 200(loginwindow): [0x000049930006d2eb] 00:00:00 PreventUserIdleSystemSleep named: "com.apple.loginwindow.assertion"
    Details: blah blah continuation
    Timeout will fire in 60 seconds Action=TimeoutActionRelease
  pid 41(coreaudiod): [0x000049930006d2ec] 00:12:34 PreventUserIdleSystemSleep named: "com.apple.audio.AudioServiceForApp"
  pid 9999(caffeinate): [0x000049930006d2ed] 00:00:01 PreventUserIdleDisplaySleep named: "caffeinate command-line tool"
No new entries'
    local out; out=$(VIGIL_ASSERTIONS_FIXTURE="$fixture" vigil_assertions_summary)
    _vigil_assertions_test_teardown
    # 3 holders → 3 TSV rows.
    local rows; rows=$(printf '%s\n' "$out" | grep -c $'\t' || true)
    assert_eq "$rows" "3" "three holders → three TSV rows"
    assert_contains "$out" $'200\tloginwindow\tPreventUserIdleSystemSleep' "loginwindow row"
    assert_contains "$out" $'41\tcoreaudiod\tPreventUserIdleSystemSleep'    "coreaudiod row"
    assert_contains "$out" $'9999\tcaffeinate\tPreventUserIdleDisplaySleep' "caffeinate row"
    # Continuation lines (Details:, Timeout will fire) must NOT bleed into output.
    assert_not_contains "$out" "Details:" "Details lines must not appear in output"
    assert_not_contains "$out" "Timeout will fire" "Timeout lines must not appear in output"
}

test_our_caffeinate_pid_is_tagged() {
    _vigil_assertions_test_setup
    # Record our caffeinate PID in the per-test state.
    echo "9999" > "$VIGIL_CAFFEINATE_PIDFILE"
    local fixture='Assertion status system-wide:
   PreventUserIdleDisplaySleep   1
Listed by owning process:
  pid 9999(caffeinate): [0x000049930006d2ed] 00:00:01 PreventUserIdleDisplaySleep named: "caffeinate command-line tool"
  pid 200(loginwindow): [0x000049930006d2eb] 00:00:00 PreventUserIdleSystemSleep named: "com.apple.loginwindow.assertion"
No new entries'
    local out; out=$(VIGIL_ASSERTIONS_FIXTURE="$fixture" vigil_assertions_summary)
    _vigil_assertions_test_teardown
    # Our PID 9999 should be tagged; pid 200 should not.
    local vigil_row;     vigil_row=$(printf '%s\n' "$out" | grep '^9999\b')
    local non_vigil_row; non_vigil_row=$(printf '%s\n' "$out" | grep '^200\b')
    assert_contains "$vigil_row"     "← vigil" "our caffeinate PID should be tagged"
    assert_not_contains "$non_vigil_row" "← vigil" "non-vigil PID should NOT be tagged"
}

test_malformed_block_is_parse_failed() {
    # Block present, contains pid-looking rows that DON'T match the expected
    # shape (no bracketed id, no timestamp). Must trigger the parse-failed branch.
    _vigil_assertions_test_setup
    local fixture='Assertion status system-wide:
   PreventUserIdleSystemSleep    1
Listed by owning process:
  pid_owner=200 name=loginwindow type=PreventUserIdleSystemSleep
  pid_owner=41  name=coreaudiod  type=PreventUserIdleSystemSleep'
    local out; out=$(VIGIL_ASSERTIONS_FIXTURE="$fixture" vigil_assertions_summary)
    _vigil_assertions_test_teardown
    assert_contains "$out" "(parse-failed; raw output:)" "malformed block → parse-failed"
    # Raw output should be included (first ~10 lines).
    assert_contains "$out" "pid_owner=200" "raw output should be appended"
}

test_no_assertions_literal_is_none() {
    # Block present, contains the "No assertions." informational message instead
    # of rows. Must collapse to (none), not parse-failed.
    _vigil_assertions_test_setup
    local fixture='Assertion status system-wide:
Listed by owning process:
   No assertions.
No new entries'
    local out; out=$(VIGIL_ASSERTIONS_FIXTURE="$fixture" vigil_assertions_summary)
    _vigil_assertions_test_teardown
    assert_eq "$out" "(none)" "explicit \"No assertions\" → (none), not parse-failed"
}
