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
    assert_contains "$out" '"daemon_scan_state": "unloaded"' "daemon scan state surfaced"
    assert_contains "$out" '"pending_active_matches": 0' "pending matches surfaced"
    assert_contains "$out" '"power_hold_mode": "best-effort"' "power hold mode surfaced"
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

test_uninstall_non_dry_run_is_blocked_under_test_no_admin() {
    _setup_cli_fake_env

    local out status
    out=$(VIGIL_TEST_NO_ADMIN=1 "$VIGIL_REPO_ROOT/bin/vigil" uninstall 2>&1)
    status=$?
    assert_not_eq "$status" "0" "uninstall must fail when admin is blocked"
    assert_contains "$out" "admin operation blocked by VIGIL_TEST_NO_ADMIN" "blocked before admin path"
    assert_file_absent "$VIGIL_FAKE_ROOT/sudo.log" "blocked uninstall must not invoke sudo"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_uninstall_refuses_overridden_privileged_paths_before_sudo() {
    _setup_cli_fake_env

    local out status
    out=$(VIGIL_TEST_NO_ADMIN=0 VIGIL_ROOT_DIR="$VIGIL_FAKE_ROOT/root-override" "$VIGIL_REPO_ROOT/bin/vigil" uninstall 2>&1)
    status=$?
    assert_not_eq "$status" "0" "uninstall must fail for non-standard privileged root path"
    assert_contains "$out" "refusing non-standard VIGIL_ROOT_DIR" "privileged path guard"
    assert_file_absent "$VIGIL_FAKE_ROOT/sudo.log" "path guard must fire before sudo"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_uninstall_rejects_args_before_admin_path() {
    _setup_cli_fake_env

    local out status
    out=$(VIGIL_TEST_NO_ADMIN=1 "$VIGIL_REPO_ROOT/bin/vigil" uninstall --dry-run 2>&1)
    status=$?
    assert_not_eq "$status" "0" "uninstall should reject unknown args"
    assert_contains "$out" "usage: vigil uninstall" "usage surfaced"
    assert_not_contains "$out" "admin operation blocked" "argument check happens before admin guard"
    assert_file_absent "$VIGIL_FAKE_ROOT/sudo.log" "argument error must not invoke sudo"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_setup_dry_run_previews_privileged_files_without_installing() {
    _setup_cli_fake_env

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" setup --dry-run)
    assert_contains "$out" "setup dry run" "dry-run header"
    assert_contains "$out" "LaunchDaemon:" "helper plist target surfaced"
    assert_contains "$out" "generated file previews: hidden" "default dry-run stays concise"
    assert_not_contains "$out" "LaunchDaemon helper plist (preview)" "default dry-run hides raw helper plist"
    assert_not_contains "$out" "NOPASSWD:" "dry-run no longer previews sudoers"
    assert_contains "$out" "No files were installed" "no-change footer"
    assert_file_absent "$HOME/Library/LaunchAgents/com.thangaram.vigil.plist" "dry-run must not write plist"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_setup_dry_run_verbose_previews_generated_files() {
    _setup_cli_fake_env

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" setup --dry-run --verbose)
    assert_contains "$out" "setup dry run" "dry-run header"
    assert_contains "$out" "LaunchDaemon helper plist (preview)" "verbose helper plist preview"
    assert_contains "$out" "LaunchAgent plist (preview)" "verbose launch agent preview"
    assert_contains "$out" "newsyslog (preview)" "verbose newsyslog preview"
    assert_not_contains "$out" "NOPASSWD:" "dry-run no longer previews sudoers"
    assert_file_absent "$HOME/Library/LaunchAgents/com.thangaram.vigil.plist" "dry-run must not write plist"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_setup_dry_run_escapes_plist_values() {
    local root home out
    root=$(mktemp -d -t vigil-cli-xml-XXXXXX)
    home="$root/home & <vigil>"
    mkdir -p "$home"

    out=$(HOME="$home" VIGIL_CONFIG_FILE="$root/no.conf" "$VIGIL_REPO_ROOT/bin/vigil" setup --dry-run --verbose)
    assert_contains "$out" "home &amp; &lt;vigil&gt;" "plist XML escapes path values"
    assert_contains "$out" "LaunchDaemon helper plist (preview)" "dry-run still renders helper plist"

    rm -rf "$root"
}

test_status_default_is_concise_but_actionable() {
    _setup_cli_fake_env

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" status)
    assert_contains "$out" "vigil status" "status header"
    assert_contains "$out" "service" "service section"
    assert_contains "$out" "activity" "activity section"
    assert_contains "$out" "power" "power section"
    assert_contains "$out" "scan:" "daemon scan surfaced"
    assert_contains "$out" "root helper:" "helper status surfaced"
    assert_contains "$out" "assertions:" "assertion summary surfaced"
    assert_contains "$out" "use 'vigil status --verbose'" "verbose hint"
    assert_not_contains "$out" "provider roots:" "default status hides provider roots"
    assert_not_contains "$out" "power assertions:" "default status hides raw assertion rows"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_status_explains_pending_first_scan() {
    _setup_cli_fake_env
    printf '0\n' > "$VIGIL_FAKE_SLEEP_FILE"
    printf '4242\n' > "$VIGIL_STATE_DIR/daemon.pid"

    cat > "$VIGIL_FAKE_ROOT/bin/launchctl" <<'FAKE_LAUNCHCTL'
#!/usr/bin/env bash
case "$1 ${2:-}" in
    "print gui/"*|"print system/"*) exit 0 ;;
    *) exit 1 ;;
esac
FAKE_LAUNCHCTL
    cat > "$VIGIL_FAKE_ROOT/bin/ps" <<'FAKE_PS'
#!/usr/bin/env bash
case "$*" in
    "-axww -o pid= -o comm=")
        printf '5151 /usr/local/bin/codex\n'
        ;;
    "-axww -o pid= -o command=")
        printf '5151 /usr/local/bin/codex\n'
        ;;
    "-axww -o command=")
        ;;
    *)
        ;;
esac
FAKE_PS
    chmod +x "$VIGIL_FAKE_ROOT/bin/launchctl" "$VIGIL_FAKE_ROOT/bin/ps"

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" status)
    assert_contains "$out" "scan:          pending first scan" "first scan state is explicit"
    assert_contains "$out" "pending scan:  1 live match(es) not counted yet" "live-but-uncounted match surfaced"
    assert_contains "$out" "expected hold: pending (daemon first scan has not completed)" "power hold pending explanation"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_status_json_reports_pending_first_scan() {
    _setup_cli_fake_env
    printf '0\n' > "$VIGIL_FAKE_SLEEP_FILE"
    printf '4242\n' > "$VIGIL_STATE_DIR/daemon.pid"

    cat > "$VIGIL_FAKE_ROOT/bin/launchctl" <<'FAKE_LAUNCHCTL'
#!/usr/bin/env bash
case "$1 ${2:-}" in
    "print gui/"*|"print system/"*) exit 0 ;;
    *) exit 1 ;;
esac
FAKE_LAUNCHCTL
    cat > "$VIGIL_FAKE_ROOT/bin/ps" <<'FAKE_PS'
#!/usr/bin/env bash
case "$*" in
    "-axww -o pid= -o comm=")
        printf '5151 /usr/local/bin/codex\n'
        ;;
    "-axww -o pid= -o command=")
        printf '5151 /usr/local/bin/codex\n'
        ;;
    "-axww -o command=")
        ;;
    *)
        ;;
esac
FAKE_PS
    chmod +x "$VIGIL_FAKE_ROOT/bin/launchctl" "$VIGIL_FAKE_ROOT/bin/ps"

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" status --json)
    assert_contains "$out" '"daemon_scan_state": "pending"' "JSON scan state"
    assert_contains "$out" '"daemon_scan_age_secs": null' "JSON pending scan has no age"
    assert_contains "$out" '"pending_active_matches": 1' "JSON pending active matches"
    if command -v python3 >/dev/null 2>&1; then
        printf '%s\n' "$out" | python3 -c 'import json,sys; json.load(sys.stdin)' || {
            echo "    FAIL: pending status JSON did not parse as JSON"
            rm -rf "$VIGIL_FAKE_ROOT"
            return 1
        }
    fi

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_status_reports_missing_scan_snapshot_for_old_daemon() {
    _setup_cli_fake_env
    printf '4242\n' > "$VIGIL_STATE_DIR/daemon.pid"
    touch -t 202001010000 "$VIGIL_STATE_DIR/daemon.pid"

    cat > "$VIGIL_FAKE_ROOT/bin/launchctl" <<'FAKE_LAUNCHCTL'
#!/usr/bin/env bash
case "$1 ${2:-}" in
    "print gui/"*|"print system/"*) exit 0 ;;
    *) exit 1 ;;
esac
FAKE_LAUNCHCTL
    cat > "$VIGIL_FAKE_ROOT/bin/ps" <<'FAKE_PS'
#!/usr/bin/env bash
exit 0
FAKE_PS
    chmod +x "$VIGIL_FAKE_ROOT/bin/launchctl" "$VIGIL_FAKE_ROOT/bin/ps"

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" status)
    assert_contains "$out" "scan:          scan snapshot missing (run 'vigil reload')" "old daemon snapshot guidance"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_status_verbose_includes_diagnostic_details() {
    _setup_cli_fake_env

    local out; out=$("$VIGIL_REPO_ROOT/bin/vigil" status --verbose)
    assert_contains "$out" "provider roots:" "verbose provider roots"
    assert_contains "$out" "state=active" "provider state surfaced"
    assert_contains "$out" "power assertions:" "verbose assertion rows"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_doctor_default_is_grouped_and_concise() {
    _setup_cli_fake_env
    rm -rf "$VIGIL_STATE_DIR"

    local out status
    out=$("$VIGIL_REPO_ROOT/bin/vigil" doctor 2>&1)
    status=$?
    assert_eq "$status" "1" "doctor should return 1 when Vigil is not installed for this user"
    assert_contains "$out" "vigil doctor" "doctor header"
    assert_contains "$out" "platform" "platform section"
    assert_contains "$out" "dependencies" "dependencies section"
    assert_contains "$out" "privileged helper" "helper section"
    assert_contains "$out" "user agent" "user agent section"
    assert_contains "$out" "providers" "providers section"
    assert_contains "$out" "result:" "result line"
    assert_contains "$out" "next:" "next action line"
    assert_contains "$out" "state:  not installed" "uninstalled state is explicit"
    assert_contains "$out" "result: setup required" "uninstalled result is setup-oriented"
    assert_contains "$out" "use 'vigil doctor --verbose'" "verbose hint"
    assert_not_contains "$out" "provider roots:" "default doctor hides provider roots"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_doctor_marks_partial_user_install_as_needs_repair() {
    _setup_cli_fake_env

    local out status
    out=$("$VIGIL_REPO_ROOT/bin/vigil" doctor 2>&1)
    status=$?
    assert_eq "$status" "1" "doctor should return 1 when current user install is incomplete"
    assert_contains "$out" "state:  needs repair" "partial install state is explicit"
    assert_contains "$out" "result:" "result surfaced"
    assert_contains "$out" "next:   vigil setup" "repair next step"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_doctor_power_returns_nonzero_when_power_path_unavailable() {
    _setup_cli_fake_env

    local out status
    out=$("$VIGIL_REPO_ROOT/bin/vigil" doctor --power 2>&1)
    status=$?
    assert_eq "$status" "1" "power doctor should fail when helper IPC is unavailable"
    assert_contains "$out" "vigil power doctor" "power doctor header"
    assert_contains "$out" "power hold mode:    best-effort" "hold mode surfaced"
    assert_contains "$out" "display sleep:      allowed" "display sleep policy surfaced"
    assert_contains "$out" "root helper:        FAIL" "helper failure surfaced"
    assert_contains "$out" "result: 1 power path check(s) failed" "power result line"
    assert_contains "$out" "next:   vigil setup" "power next step"

    rm -rf "$VIGIL_FAKE_ROOT"
}

test_doctor_verbose_includes_paths_and_provider_roots() {
    _setup_cli_fake_env
    rm -rf "$VIGIL_STATE_DIR"

    local out status
    out=$("$VIGIL_REPO_ROOT/bin/vigil" doctor --verbose 2>&1)
    status=$?
    assert_eq "$status" "1" "doctor should return 1 in fake uninstalled user env"
    assert_contains "$out" "paths" "verbose path section"
    assert_contains "$out" "provider roots:" "verbose provider roots"
    assert_contains "$out" "LaunchAgent:" "launch agent path surfaced"
    assert_contains "$out" "state=active" "provider state surfaced"

    rm -rf "$VIGIL_FAKE_ROOT"
}
