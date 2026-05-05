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
