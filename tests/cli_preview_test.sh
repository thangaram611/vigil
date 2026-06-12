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

    cat > "$root/bin/sudo" <<'FAKE_SUDO'
#!/usr/bin/env bash
if [[ "$1" == "-n" && "$2" == "-l" ]]; then
    exit 0
fi
exit 1
FAKE_SUDO

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

    chmod +x "$root/bin/sudo" "$root/bin/pmset" "$root/bin/launchctl" "$root/bin/visudo"
    export PATH="$root/bin:$PATH"
}

test_status_json_reports_machine_readable_power_state() {
    _setup_cli_fake_env

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" status --json)
    assert_contains "$out" '"launchd_loaded": false' "launchd false in fake env"
    assert_contains "$out" '"pmset_disablesleep": 1' "SleepDisabled surfaced"
    assert_contains "$out" '"sudoers_ok": true' "sudoers check surfaced"
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

test_setup_dry_run_previews_privileged_files_without_installing() {
    _setup_cli_fake_env

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" setup --dry-run)
    assert_contains "$out" "setup dry run" "dry-run header"
    assert_contains "$out" "NOPASSWD: /usr/bin/pmset -a disablesleep 0" "sudoers preview"
    assert_contains "$out" "LaunchAgent plist (preview)" "plist preview"
    assert_contains "$out" "No files were installed" "no-change footer"
    assert_file_absent "$HOME/Library/LaunchAgents/com.thangaram.vigil.plist" "dry-run must not write plist"

    rm -rf "$VIGIL_FAKE_ROOT"
}
