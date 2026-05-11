#!/usr/bin/env bash
# tests/wrapper_test.sh — verify `vigil run` writes a PID file, holds it during
# the child's lifetime, and removes it via the EXIT trap (NOT exec).

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

test_wrapper_creates_and_cleans_pidfile() {
    local tmpstate; tmpstate=$(mktemp -d)
    export VIGIL_STATE_DIR="$tmpstate"
    export VIGIL_LOG_DIR="$tmpstate/logs"
    mkdir -p "$VIGIL_STATE_DIR/active" "$VIGIL_LOG_DIR"

    # Run a wrapper around a short-lived `sleep`. Snapshot active/ in the
    # middle of its lifetime, then again after it exits.
    "$VIGIL_REPO_ROOT/bin/vigil" run sleep 0.6 &
    local wrapper_pid=$!
    sleep 0.2
    local during; during=$(ls "$tmpstate/active/" 2>/dev/null || true)
    wait "$wrapper_pid" 2>/dev/null || true
    local after;  after=$(ls "$tmpstate/active/" 2>/dev/null || true)

    if [[ -z "$during" ]]; then
        echo "    FAIL: no wrapper PID file existed during the child's lifetime"
        rm -rf "$tmpstate"
        return 1
    fi
    assert_contains "$during" "wrapper-" "during-snapshot should show wrapper-*.pid"
    assert_eq "${after:-}" "" "after the child exits, active/ should be empty"

    rm -rf "$tmpstate"
}

test_wrapper_pidfile_survives_stale_gc() {
    # A wrapper around a low-CPU command (`sleep 60`) used to be GC'd at 30s
    # because branch (c) of vigil_refcount_gc deletes any pidfile with
    # age > VIGIL_STALE_AGE_SECS AND cpu < VIGIL_STALE_CPU_PCT. Wrappers are
    # written once (no per-tick mtime refresh), so they aged out. The fix:
    # GC skips wrapper records in branch (c). We exercise that here.
    local tmpstate; tmpstate=$(mktemp -d)
    export VIGIL_STATE_DIR="$tmpstate"
    export VIGIL_LOG_DIR="$tmpstate/logs"
    export VIGIL_ACTIVE_DIR="$tmpstate/active"
    mkdir -p "$VIGIL_ACTIVE_DIR" "$VIGIL_LOG_DIR"

    # Source under our overrides so VIGIL_ACTIVE_DIR resolves correctly.
    # shellcheck source=../lib/common.sh
    source "$VIGIL_REPO_ROOT/lib/common.sh"
    # shellcheck source=../lib/refcount.sh
    source "$VIGIL_REPO_ROOT/lib/refcount.sh"

    # Use $$ (this test's bash) — guaranteed alive and ~0% CPU, exactly the
    # condition that used to trigger the bug.
    local pidfile="$VIGIL_ACTIVE_DIR/wrapper-$$.pid"
    local start_ts; start_ts=$(vigil_pid_start_ts "$$")
    printf '{"pid":%s,"comm":"wrapper","start_ts":%s,"cmd":"sleep 99"}\n' "$$" "$start_ts" > "$pidfile"
    # Backdate the file beyond the stale threshold (default 30s).
    touch -t 200001010000 "$pidfile"

    VIGIL_STALE_AGE_SECS=30 VIGIL_STALE_CPU_PCT=0.5 vigil_refcount_gc

    assert_file_exists "$pidfile" "wrapper pidfile must survive idle-CPU GC"

    rm -rf "$tmpstate"
}
