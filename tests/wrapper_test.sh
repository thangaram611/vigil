#!/usr/bin/env bash
# tests/wrapper_test.sh — verify `vigil run` writes a PID file, holds it during
# the child's lifetime, and removes it via the EXIT trap (NOT exec).

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

test_wrapper_creates_and_cleans_pidfile() {
    local tmpstate; tmpstate=$(mktemp -d)
    export VIGIL_STATE_DIR="$tmpstate"
    export VIGIL_LOG_DIR="$tmpstate/logs"
    mkdir -p "$VIGIL_STATE_DIR/active" "$VIGIL_LOG_DIR"

    # Run a wrapper around a short-lived `sleep 0.5`. While it's running we
    # snapshot whether a wrapper-*.pid file exists.
    local snapshot_file="$tmpstate/snapshot"
    (
        "$VIGIL_REPO_ROOT/bin/vigil" run sleep 0.5 &
        local wrapper_pid=$!
        sleep 0.2
        ls "$tmpstate/active/" > "$snapshot_file" 2>/dev/null || true
        wait "$wrapper_pid"
    )

    local during; during=$(cat "$snapshot_file" 2>/dev/null)
    local after; after=$(ls "$tmpstate/active/" 2>/dev/null)

    if [[ -z "$during" ]]; then
        echo "    FAIL: no wrapper PID file existed during the child's lifetime"
        rm -rf "$tmpstate"
        return 1
    fi
    assert_contains "$during" "wrapper-" "during-snapshot should show wrapper-*.pid"
    assert_eq "${after:-}" "" "after the child exits, active/ should be empty"

    rm -rf "$tmpstate"
}
