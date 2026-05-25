#!/usr/bin/env bash
# tests/refcount_activity_test.sh — vigil_refcount_count / list with activity flags.

VIGIL_LIB_DIR="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/lib"
# shellcheck source=../lib/common.sh
source "$VIGIL_LIB_DIR/common.sh"
# shellcheck source=../lib/refcount.sh
source "$VIGIL_LIB_DIR/refcount.sh"

# Set up a temp state dir before each test. The pidfile contents don't matter
# for these tests — count/list parse only the filename.
_setup_active_dir() {
    local d; d=$(mktemp -d -t vigil-rc-XXXXXX)
    export VIGIL_STATE_DIR="$d"
    export VIGIL_ACTIVE_DIR="$d/active"
    mkdir -p "$VIGIL_ACTIVE_DIR"
    printf '%s' "$VIGIL_ACTIVE_DIR"
}

_make_pidfile() {
    local dir="$1" name="$2"
    : > "$dir/${name}.pid"
}

test_count_with_claude_active_only() {
    local active; active=$(_setup_active_dir)
    _make_pidfile "$active" "cli-claude-1001"
    _make_pidfile "$active" "cli-claude-1002"
    _make_pidfile "$active" "cli-codex-1003"
    _make_pidfile "$active" "wrapper-1004"
    assert_eq "$(vigil_refcount_count 1 0 0)" "3" "2 claude + wrapper"
    assert_eq "$(vigil_refcount_count_total)" "4"
    rm -rf "$VIGIL_STATE_DIR"
}

test_count_with_app_codex_gated_on_codex_flag() {
    # Phase-3: app-codex is the Codex.app host process. It must contribute
    # to the refcount iff `codex_active=1`, mirroring cli-codex. An idle
    # Codex.app open in the background must not hold sleep.
    local active; active=$(_setup_active_dir)
    _make_pidfile "$active" "app-codex-2700"
    _make_pidfile "$active" "cli-claude-1001"
    assert_eq "$(vigil_refcount_count 1 0 0)" "1" "codex idle: only cli-claude counts (app-codex gated out)"
    assert_eq "$(vigil_refcount_count 1 1 0)" "2" "codex active: app-codex joins the refcount"
    assert_eq "$(vigil_refcount_count 0 1 0)" "1" "claude idle, codex active: only app-codex"
    rm -rf "$VIGIL_STATE_DIR"
}

test_count_when_all_idle() {
    local active; active=$(_setup_active_dir)
    _make_pidfile "$active" "cli-claude-1001"
    _make_pidfile "$active" "cli-claude-1002"
    _make_pidfile "$active" "cli-codex-1003"
    _make_pidfile "$active" "wrapper-1004"
    assert_eq "$(vigil_refcount_count 0 0 0)" "1" "wrapper only"
    rm -rf "$VIGIL_STATE_DIR"
}

test_list_state_column_matches_flags() {
    local active; active=$(_setup_active_dir)
    _make_pidfile "$active" "cli-claude-1001"
    _make_pidfile "$active" "cli-codex-1002"
    _make_pidfile "$active" "app-codex-2700"
    _make_pidfile "$active" "wrapper-1003"
    local out; out=$(vigil_refcount_list 1 0 0)
    assert_contains "$out" "1001	cli-claude" "claude row present"
    assert_contains "$out" "1002	cli-codex"  "codex row present"
    assert_contains "$out" "2700	app-codex"  "app-codex row present"
    assert_contains "$out" "1003	wrapper"    "wrapper row present"
    # Each row's last column should match its expected state.
    local claude_row;    claude_row=$(echo    "$out" | awk -F'\t' '$2=="cli-claude"')
    local codex_row;     codex_row=$(echo     "$out" | awk -F'\t' '$2=="cli-codex"')
    local app_codex_row; app_codex_row=$(echo "$out" | awk -F'\t' '$2=="app-codex"')
    local wrapper_row;   wrapper_row=$(echo   "$out" | awk -F'\t' '$2=="wrapper"')
    assert_eq "$(echo "$claude_row"    | awk -F'\t' '{print $NF}')" "active" "claude row state"
    assert_eq "$(echo "$codex_row"     | awk -F'\t' '{print $NF}')" "idle"   "codex row state"
    assert_eq "$(echo "$app_codex_row" | awk -F'\t' '{print $NF}')" "idle"   "app-codex row state mirrors codex flag"
    assert_eq "$(echo "$wrapper_row"   | awk -F'\t' '{print $NF}')" "active" "wrapper row state"
    rm -rf "$VIGIL_STATE_DIR"
}

test_filename_parser_handles_all_prefixes() {
    local active; active=$(_setup_active_dir)
    _make_pidfile "$active" "cli-claude-1"
    _make_pidfile "$active" "cli-codex-2"
    _make_pidfile "$active" "cli-copilot-3"
    _make_pidfile "$active" "app-codex-4"
    _make_pidfile "$active" "wrapper-5"
    # All five counted when every flag is on.
    assert_eq "$(vigil_refcount_count 1 1 1)" "5" "all five prefixes count when active"
    rm -rf "$VIGIL_STATE_DIR"
}

test_wrappers_count_regardless_of_agent_flags() {
    local active; active=$(_setup_active_dir)
    _make_pidfile "$active" "wrapper-1234"
    assert_eq "$(vigil_refcount_count 0 0 0)" "1" "wrapper counts even when all agents idle"
    rm -rf "$VIGIL_STATE_DIR"
}
