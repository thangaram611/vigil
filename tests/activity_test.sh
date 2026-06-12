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

test_session_dir_honors_vigil_provider_home_vars() {
    local root; root=$(_mk_root)
    VIGIL_CLAUDE_HOME="$root/claude custom"
    VIGIL_CODEX_HOME="$root/codex custom"
    VIGIL_COPILOT_HOME="$root/copilot custom"
    assert_eq "$(vigil_session_dir_for_agent claude)" "$root/claude custom/projects"
    assert_eq "$(vigil_session_dir_for_agent codex)" "$root/codex custom/sessions"
    assert_eq "$(vigil_session_dir_for_agent copilot)" "$root/copilot custom/session-state"
    rm -rf "$root"
}

test_provider_env_vars_are_used_when_sourced() {
    local root out expected
    root=$(_mk_root)
    out=$(
        CLAUDE_CONFIG_DIR="$root/claude-env" \
        CODEX_HOME="$root/codex-env" \
        COPILOT_HOME="$root/copilot-env" \
        VIGIL_REPO_ROOT="$VIGIL_REPO_ROOT" \
        bash -c '
            unset VIGIL_CLAUDE_HOME VIGIL_CODEX_HOME VIGIL_COPILOT_HOME
            VIGIL_LIB_DIR="$VIGIL_REPO_ROOT/lib"
            source "$VIGIL_LIB_DIR/activity.sh"
            vigil_session_dir_for_agent claude
            vigil_session_dir_for_agent codex
            vigil_session_dir_for_agent copilot
        '
    )
    expected=$(
        printf '%s\n' \
            "$root/claude-env/projects" \
            "$root/codex-env/sessions" \
            "$root/copilot-env/session-state"
    )
    assert_eq "$out" "$expected"
    rm -rf "$root"
}

test_provider_env_vars_from_vigil_config_are_used_after_load() {
    local root out
    root=$(_mk_root)
    printf 'CODEX_HOME=%q\n' "$root/codex-from-config" > "$root/vigil.conf"
    out=$(
        HOME="$root/home" \
        VIGIL_CONFIG_FILE="$root/vigil.conf" \
        VIGIL_REPO_ROOT="$VIGIL_REPO_ROOT" \
        bash -c '
            unset VIGIL_CODEX_HOME CODEX_HOME
            VIGIL_LIB_DIR="$VIGIL_REPO_ROOT/lib"
            source "$VIGIL_LIB_DIR/activity.sh"
            vigil_load_config
            vigil_session_dir_for_agent codex
        '
    )
    assert_eq "$out" "$root/codex-from-config/sessions"
    rm -rf "$root"
}

test_home_override_bypasses_live_provider_home_vars() {
    local root home
    root=$(_mk_root)
    home="$root/home"
    VIGIL_CODEX_HOME="$root/codex-live"
    assert_eq "$(vigil_session_dir_for_agent codex "$home")" "$home/.codex/sessions"
    rm -rf "$root"
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

test_latest_activity_age_uses_newest_matching_file() {
    local home; home=$(_mk_root)
    local dir="$home/.codex/sessions/2026/06/12"
    mkdir -p "$dir"
    : > "$dir/rollout-old.jsonl"
    touch -t 200001010000 "$dir/rollout-old.jsonl"
    : > "$dir/rollout-new.jsonl"
    local age; age=$(vigil_agent_latest_activity_age_secs codex "$home")
    [[ "$age" =~ ^[0-9]+$ ]] || {
        printf '    FAIL: expected numeric activity age, got %q\n' "$age"
        rm -rf "$home"; return 1
    }
    if (( age > 60 )); then
        printf '    FAIL: expected newest activity age <= 60s, got %ss\n' "$age"
        rm -rf "$home"; return 1
    fi
    rm -rf "$home"
}

_mk_vscode_state_file() {
    local home="$1" epoch="$2"
    local dir="$home/Library/Application Support/Code - Insiders/User/workspaceStorage/hash/chatEditingSessions/session"
    mkdir -p "$dir"
    printf '{"version":1,"timeline":{"checkpoints":[{"checkpointId":"c%s","epoch":%s,"label":"l","description":"d"}],"currentEpoch":%s,"fileBaselines":[],"operations":[],"epochCounter":%s},"recentSnapshot":{"entries":[]},"initialFileContents":[]}\n' \
        "$epoch" "$epoch" "$epoch" "$epoch" > "$dir/state.json"
    printf '%s\n' "$dir/state.json"
}

_reset_vscode_scan_timer() {
    local state_file="$1"
    [[ -f "$state_file" ]] || return 0
    awk -F'\t' 'BEGIN{OFS="\t"} $1=="last_scan" {$2=0} {print}' "$state_file" > "$state_file.tmp"
    mv "$state_file.tmp" "$state_file"
}

test_vscode_copilot_chat_hash_change_drives_activity() {
    local root home state_file chat_state
    root=$(_mk_root)
    home="$root/home"
    state_file="$root/vscode-copilot.state"
    chat_state=$(_mk_vscode_state_file "$home" 1)
    export VIGIL_VSCODE_PS_FIXTURE="/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Code - Insiders"
    export VIGIL_VSCODE_COPILOT_STATE_FILE="$state_file"
    export VIGIL_VSCODE_COPILOT_DISCOVER_SECS=5
    export VIGIL_VSCODE_COPILOT_RECENT_MINS=10

    if vigil_vscode_copilot_chat_is_active "$home"; then
        printf '    FAIL: first scan should prime existing VS Code chat state without counting active\n'
        rm -rf "$root"; return 1
    fi

    # Mtime-only rewrites are the noisy behavior observed live; they must not
    # mark activity if the semantic file hash did not change.
    touch "$chat_state"
    _reset_vscode_scan_timer "$state_file"
    if vigil_vscode_copilot_chat_is_active "$home"; then
        printf '    FAIL: mtime-only VS Code chat state rewrite should stay idle\n'
        rm -rf "$root"; return 1
    fi

    chat_state=$(_mk_vscode_state_file "$home" 2)
    _reset_vscode_scan_timer "$state_file"
    VIGIL_IDLE_AFTER_SEC=300 vigil_vscode_copilot_chat_is_active "$home" || {
        printf '    FAIL: semantic VS Code chat state change should count active\n'
        rm -rf "$root"; return 1
    }

    rm -rf "$root"
    unset VIGIL_VSCODE_PS_FIXTURE VIGIL_VSCODE_COPILOT_STATE_FILE VIGIL_VSCODE_COPILOT_DISCOVER_SECS VIGIL_VSCODE_COPILOT_RECENT_MINS
}

test_vscode_copilot_chat_retains_hashes_after_file_ages_out() {
    local root home state_file chat_state
    root=$(_mk_root)
    home="$root/home"
    state_file="$root/vscode-copilot.state"
    chat_state=$(_mk_vscode_state_file "$home" 1)
    export VIGIL_VSCODE_PS_FIXTURE="/Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Code - Insiders"
    export VIGIL_VSCODE_COPILOT_STATE_FILE="$state_file"
    export VIGIL_VSCODE_COPILOT_DISCOVER_SECS=5
    export VIGIL_VSCODE_COPILOT_RECENT_MINS=1

    vigil_vscode_copilot_chat_is_active "$home" && {
        printf '    FAIL: first scan should only prime\n'
        rm -rf "$root"; return 1
    }
    touch -t 200001010000 "$chat_state"
    _reset_vscode_scan_timer "$state_file"
    vigil_vscode_copilot_chat_is_active "$home" && {
        printf '    FAIL: aged-out known file should not count active\n'
        rm -rf "$root"; return 1
    }
    touch "$chat_state"
    _reset_vscode_scan_timer "$state_file"
    vigil_vscode_copilot_chat_is_active "$home" && {
        printf '    FAIL: unchanged known file reappearing as recent should not count active\n'
        rm -rf "$root"; return 1
    }

    rm -rf "$root"
    unset VIGIL_VSCODE_PS_FIXTURE VIGIL_VSCODE_COPILOT_STATE_FILE VIGIL_VSCODE_COPILOT_DISCOVER_SECS VIGIL_VSCODE_COPILOT_RECENT_MINS
}

test_vscode_copilot_chat_state_is_none_without_host() {
    local old_fixture="${VIGIL_VSCODE_PS_FIXTURE-__unset__}"
    export VIGIL_VSCODE_PS_FIXTURE=""
    assert_eq "$(vigil_vscode_copilot_chat_state)" "none"
    if [[ "$old_fixture" == "__unset__" ]]; then
        unset VIGIL_VSCODE_PS_FIXTURE
    else
        VIGIL_VSCODE_PS_FIXTURE="$old_fixture"
    fi
}
