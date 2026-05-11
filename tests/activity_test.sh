#!/usr/bin/env bash
# tests/activity_test.sh — vigil_session_dir_for_agent / vigil_agent_is_active

VIGIL_LIB_DIR="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/lib"
# shellcheck source=../lib/activity.sh
source "$VIGIL_LIB_DIR/activity.sh"

# All tests pass an explicit home_override so they don't depend on real $HOME.
_mk_root() { mktemp -d -t vigil-activity-XXXXXX; }

test_session_dir_for_known_agents() {
    local home; home=$(_mk_root)
    assert_eq "$(vigil_session_dir_for_agent claude  "$home")" "$home/.claude/projects"
    assert_eq "$(vigil_session_dir_for_agent codex   "$home")" "$home/.codex/sessions"
    assert_eq "$(vigil_session_dir_for_agent copilot "$home")" "$home/.copilot/session-state"
    rm -rf "$home"
}

test_session_dir_for_unknown_agent() {
    local out; out=$(vigil_session_dir_for_agent "zzz" "/tmp" 2>/dev/null) || true
    assert_eq "$out" "" "unknown agent should produce no output"
    if vigil_session_dir_for_agent "zzz" "/tmp" >/dev/null 2>&1; then
        printf '    FAIL: expected nonzero exit on unknown agent\n'
        return 1
    fi
}

test_pattern_for_each_agent() {
    assert_eq "$(vigil_agent_pattern_for claude)"  "*.jsonl"
    assert_eq "$(vigil_agent_pattern_for codex)"   "rollout-*.jsonl"
    assert_eq "$(vigil_agent_pattern_for copilot)" "events.jsonl"
}

test_agent_is_active_when_recent_jsonl() {
    local home; home=$(_mk_root)
    local dir="$home/.claude/projects/some-cwd"
    mkdir -p "$dir"
    : > "$dir/abc.jsonl"   # mtime = now
    VIGIL_IDLE_AFTER_SEC=300 vigil_agent_is_active claude "$home" \
        || { printf '    FAIL: expected claude=active\n'; rm -rf "$home"; return 1; }
    rm -rf "$home"
}

test_agent_is_inactive_when_old_file() {
    local home; home=$(_mk_root)
    local dir="$home/.claude/projects/some-cwd"
    mkdir -p "$dir"
    : > "$dir/abc.jsonl"
    touch -t 200001010000 "$dir/abc.jsonl"
    if VIGIL_IDLE_AFTER_SEC=300 vigil_agent_is_active claude "$home"; then
        printf '    FAIL: expected claude=idle for stale file\n'
        rm -rf "$home"; return 1
    fi
    rm -rf "$home"
}

test_agent_is_inactive_when_dir_missing() {
    local home; home=$(_mk_root)
    if VIGIL_IDLE_AFTER_SEC=300 vigil_agent_is_active claude "$home"; then
        printf '    FAIL: expected idle when session dir missing\n'
        rm -rf "$home"; return 1
    fi
    rm -rf "$home"
}

test_agent_is_active_for_codex_subdir_layout() {
    local home; home=$(_mk_root)
    local dir="$home/.codex/sessions/2026/05/06"
    mkdir -p "$dir"
    : > "$dir/rollout-2026-05-06T10-10-10-uuid.jsonl"
    VIGIL_IDLE_AFTER_SEC=300 vigil_agent_is_active codex "$home" \
        || { printf '    FAIL: codex deep subdir not detected\n'; rm -rf "$home"; return 1; }
    rm -rf "$home"
}

test_pattern_filter_rejects_wrong_extension() {
    local home; home=$(_mk_root)
    local dir="$home/.copilot/session-state/abc-uuid"
    mkdir -p "$dir"
    : > "$dir/notes.txt"
    if VIGIL_IDLE_AFTER_SEC=300 vigil_agent_is_active copilot "$home"; then
        printf '    FAIL: copilot should be idle when only notes.txt present\n'
        rm -rf "$home"; return 1
    fi
    rm -rf "$home"
}

test_agent_state_returns_none_when_dir_missing() {
    local home; home=$(_mk_root)
    assert_eq "$(VIGIL_IDLE_AFTER_SEC=300 vigil_agent_state copilot "$home")" "none"
    rm -rf "$home"
}
