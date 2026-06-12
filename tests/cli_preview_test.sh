#!/usr/bin/env bash
# tests/cli_preview_test.sh — user-facing dry-run and JSON status surfaces.

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

_setup_cli_fake_env() {
    local root; root=$(mktemp -d -t vigil-cli-XXXXXX)
    export VIGIL_FAKE_ROOT="$root"
    export VIGIL_FAKE_SLEEP_FILE="$root/sleepdisabled"
    export VIGIL_STATE_DIR="$root/state"
    export VIGIL_LOG_DIR="$root/logs"
    export VIGIL_CONFIG_FILE="$root/no.conf"
    export HOME="$root/home"
    export VIGIL_CLAUDE_HOME="$HOME/provider/claude"
    export VIGIL_CODEX_HOME="$HOME/provider/codex"
    export VIGIL_COPILOT_HOME="$HOME/provider/copilot"
    mkdir -p "$root/bin" "$HOME" "$VIGIL_STATE_DIR/active" "$VIGIL_LOG_DIR"
    mkdir -p "$VIGIL_CODEX_HOME/sessions/2026/06/12"
    : > "$VIGIL_CODEX_HOME/sessions/2026/06/12/rollout-2026-06-12T00-00-00-test.jsonl"
    printf '1\n' > "$VIGIL_FAKE_SLEEP_FILE"

    cat > "$root/bin/pmset" <<'FAKE_PMSET'
#!/usr/bin/env bash
case "$1 ${2:-}" in
    "-g assertions")
        printf 'Assertion status system-wide:\nListed by owning process:\nNo assertions.\n'
        ;;
    "-g ps")
        printf "Now drawing from 'AC Power'\n -InternalBattery-0\t90%%; charged; 0:00 remaining present: true\n"
        ;;
    "-g therm")
        printf 'Note: No CPU power status has been recorded\n'
        ;;
    "-g ")
        printf ' SleepDisabled\t\t%s\n' "$(cat "$VIGIL_FAKE_SLEEP_FILE")"
        ;;
    *)
        exit 64
        ;;
esac
FAKE_PMSET

    cat > "$root/bin/launchctl" <<'FAKE_LAUNCHCTL'
#!/usr/bin/env bash
exit 1
FAKE_LAUNCHCTL

    cat > "$root/bin/visudo" <<'FAKE_VISUDO'
#!/usr/bin/env bash
exit 0
FAKE_VISUDO

    cat > "$root/bin/sudo" <<'FAKE_SUDO'
#!/usr/bin/env bash
printf 'sudo %s\n' "$*" >> "$VIGIL_FAKE_ROOT/sudo.log"
exit 97
FAKE_SUDO

    chmod +x "$root/bin/pmset" "$root/bin/launchctl" "$root/bin/visudo" "$root/bin/sudo"
    export PATH="$root/bin:$PATH"
}

test_status_json_reports_machine_readable_power_state() {
    _setup_cli_fake_env

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" status --json)
    assert_contains "$out" '"launchd_loaded": false' "launchd false in fake env"
    assert_contains "$out" '"pmset_disablesleep": 1' "SleepDisabled surfaced"
    assert_contains "$out" '"power_helper_ok": false' "helper check surfaced"
    assert_contains "$out" '"provider_roots": {' "provider roots surfaced"
    assert_contains "$out" "\"home\":\"$VIGIL_CODEX_HOME\"" "codex provider home surfaced"
    assert_contains "$out" '"exists":true' "existing provider session dir surfaced"
    assert_contains "$out" '"power_assertions_state": "none"' "assertion state surfaced"
    if command -v python3 >/dev/null 2>&1; then
        printf '%s\n' "$out" | python3 -c 'import json,sys; json.load(sys.stdin)' || {
            echo "    FAIL: status --json did not parse as JSON"
            rm -rf "$VIGIL_FAKE_ROOT"
            return 1
        }
        printf '%s\n' "$out" | VIGIL_EXPECT_CODEX_HOME="$VIGIL_CODEX_HOME" python3 -c '
import json, os, sys
data = json.load(sys.stdin)
codex = data["provider_roots"]["codex"]
assert codex["home"] == os.environ["VIGIL_EXPECT_CODEX_HOME"]
assert codex["exists"] is True
assert isinstance(codex["latest_activity_age_secs"], int)
' || {
            echo "    FAIL: status --json provider_roots fields were invalid"
            rm -rf "$VIGIL_FAKE_ROOT"
            return 1
        }
    fi

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_setup_non_dry_run_is_blocked_under_test_no_admin() {
    _setup_cli_fake_env

    local out status
    out=$(VIGIL_TEST_NO_ADMIN=1 "$VIGIL_REPO_ROOT/bin/vigil" setup 2>&1)
    status=$?
    assert_not_eq "$status" "0" "setup must fail when admin is blocked"
    assert_contains "$out" "admin operation blocked by VIGIL_TEST_NO_ADMIN" "blocked before admin path"
    assert_file_absent "$VIGIL_FAKE_ROOT/sudo.log" "blocked setup must not invoke sudo"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_setup_refuses_overridden_privileged_paths_before_sudo() {
    _setup_cli_fake_env

    local out status
    out=$(VIGIL_TEST_NO_ADMIN=0 VIGIL_ROOT_DIR="$VIGIL_FAKE_ROOT/root-override" "$VIGIL_REPO_ROOT/bin/vigil" setup 2>&1)
    status=$?
    assert_not_eq "$status" "0" "setup must fail for non-standard privileged root path"
    assert_contains "$out" "refusing non-standard VIGIL_ROOT_DIR" "privileged path guard"
    assert_file_absent "$VIGIL_FAKE_ROOT/sudo.log" "path guard must fire before sudo"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_setup_dry_run_previews_privileged_files_without_installing() {
    _setup_cli_fake_env

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" setup --dry-run)
    assert_contains "$out" "setup dry run" "dry-run header"
    assert_contains "$out" "LaunchDaemon helper plist (preview)" "helper plist preview"
    assert_not_contains "$out" "NOPASSWD:" "dry-run no longer previews sudoers"
    assert_contains "$out" "LaunchAgent plist (preview)" "plist preview"
    assert_contains "$out" "No files were installed" "no-change footer"
    assert_file_absent "$HOME/Library/LaunchAgents/com.thangaram.vigil.plist" "dry-run must not write plist"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_setup_dry_run_escapes_plist_values() {
    local root home out
    root=$(mktemp -d -t vigil-cli-xml-XXXXXX)
    home="$root/home & <vigil>"
    mkdir -p "$home"

    out=$(HOME="$home" VIGIL_CONFIG_FILE="$root/no.conf" "$VIGIL_REPO_ROOT/bin/vigil" setup --dry-run)
    assert_contains "$out" "home &amp; &lt;vigil&gt;" "plist XML escapes path values"
    assert_contains "$out" "LaunchDaemon helper plist (preview)" "dry-run still renders helper plist"

    rm -rf "$root"
}
