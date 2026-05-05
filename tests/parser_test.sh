#!/usr/bin/env bash
# tests/parser_test.sh — verify _vigil_pidfile_field parses our PID-file JSON
# correctly. Caught a real bug where a naive awk -F'[:,}]' picked the wrong
# field for `start_ts` (returned the leading "pid" value instead).

VIGIL_LIB_DIR="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/lib"
# shellcheck source=../lib/refcount.sh
source "$VIGIL_LIB_DIR/refcount.sh"

_make_fixture() {
    local content="$1" tmp
    tmp=$(mktemp)
    printf '%s\n' "$content" > "$tmp"
    printf '%s' "$tmp"
}

test_extracts_pid() {
    local f; f=$(_make_fixture '{"pid":1234,"comm":"claude","start_ts":1700000000,"name":"cli-claude"}')
    local v; v=$(_vigil_pidfile_field "$f" "pid")
    assert_eq "$v" "1234" "extract pid"
    rm -f "$f"
}

test_extracts_start_ts_not_pid() {
    # The original bug: awk -F'[:,}]' returned "1234" (the pid) when asked for start_ts.
    local f; f=$(_make_fixture '{"pid":1234,"comm":"claude","start_ts":1700000000,"name":"cli-claude"}')
    local v; v=$(_vigil_pidfile_field "$f" "start_ts")
    assert_eq "$v" "1700000000" "extract start_ts must NOT return the pid"
    rm -f "$f"
}

test_extracts_string_field() {
    local f; f=$(_make_fixture '{"pid":1234,"comm":"claude","start_ts":1700000000,"name":"cli-claude"}')
    local v; v=$(_vigil_pidfile_field "$f" "name")
    assert_eq "$v" "cli-claude" "extract string field"
    rm -f "$f"
}

test_returns_nothing_when_key_missing() {
    local f; f=$(_make_fixture '{"pid":1234}')
    local v; v=$(_vigil_pidfile_field "$f" "nope" 2>/dev/null || true)
    assert_eq "${v:-}" "" "missing key should yield empty"
    rm -f "$f"
}

test_baseline_sleepdisabled() {
    # SleepDisabled is the FIRST field in baseline.json — verify the parser still picks it.
    local f; f=$(_make_fixture '{"SleepDisabled":1,"captured_at":1700000000}')
    local v; v=$(_vigil_pidfile_field "$f" "SleepDisabled")
    assert_eq "$v" "1" "extract SleepDisabled when first field"
    rm -f "$f"
}
