#!/usr/bin/env bash
# tests/root_helper_test.sh — privileged helper validation without real pmset.

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

_root_helper_setup() {
    local root
    root=$(mktemp -d -t vigil-root-helper-XXXXXX)
    export ROOT_HELPER_TEST_DIR="$root"
    export HELPER_REQUEST_DIR="$root/requests"
    export HELPER_RESPONSE_DIR="$root/responses"
    export HELPER_STATE_DIR="$root/state"
    export HELPER_LOG_FILE="$root/logs/helper.log"
    export HELPER_ALLOWED_UID="$(id -u)"
    export HELPER_ALLOWED_USER="$(id -un)"
    export HELPER_POLL_SECS="1"
    export HELPER_ONCE=1
    export ROOT_HELPER_SLEEP_FILE="$root/sleepdisabled"
    export ROOT_HELPER_EVENTS="$root/events.log"
    export ROOT_HELPER_PMSET_FAIL=0
    mkdir -p "$HELPER_REQUEST_DIR" "$HELPER_RESPONSE_DIR" "$HELPER_STATE_DIR" "$(dirname "$HELPER_LOG_FILE")"
    printf '0\n' > "$ROOT_HELPER_SLEEP_FILE"
    : > "$ROOT_HELPER_EVENTS"

    VIGIL_ROOT_HELPER_LIB_ONLY=1 source "$VIGIL_REPO_ROOT/bin/vigil-root-helper"
    helper_pmset() {
        case "$*" in
            "-g")
                printf ' SleepDisabled\t\t%s\n' "$(cat "$ROOT_HELPER_SLEEP_FILE")"
                ;;
            "-a disablesleep 0")
                if [[ "${ROOT_HELPER_PMSET_FAIL:-0}" == "1" ]]; then
                    printf 'pmset fail -a disablesleep 0\n' >> "$ROOT_HELPER_EVENTS"
                    return 1
                fi
                printf 'pmset -a disablesleep 0\n' >> "$ROOT_HELPER_EVENTS"
                printf '0\n' > "$ROOT_HELPER_SLEEP_FILE"
                ;;
            "-a disablesleep 1")
                if [[ "${ROOT_HELPER_PMSET_FAIL:-0}" == "1" ]]; then
                    printf 'pmset fail -a disablesleep 1\n' >> "$ROOT_HELPER_EVENTS"
                    return 1
                fi
                printf 'pmset -a disablesleep 1\n' >> "$ROOT_HELPER_EVENTS"
                printf '1\n' > "$ROOT_HELPER_SLEEP_FILE"
                ;;
            *)
                printf 'unexpected pmset args: %s\n' "$*" >> "$ROOT_HELPER_EVENTS"
                return 64
                ;;
        esac
    }
}

_root_helper_teardown() {
    rm -rf "${ROOT_HELPER_TEST_DIR:-}"
}

_root_helper_request() {
    local id="$1" action="$2"
    local path="$HELPER_REQUEST_DIR/req.$id"
    printf '%s\n' "$action" > "$path"
    chmod 0600 "$path"
    printf '%s\n' "$path"
}

_root_helper_response() {
    local id="$1"
    cat "$HELPER_RESPONSE_DIR/resp.$id"
}

test_root_helper_accepts_only_known_actions() {
    _root_helper_setup

    _root_helper_request good status >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response good)" "status=ok" "status request accepted"

    _root_helper_request bad reboot >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response bad)" "status=error" "unknown command rejected"
    assert_contains "$(_root_helper_response bad)" "message=invalid_action" "reject reason recorded"

    _root_helper_teardown
}

test_root_helper_rejects_malformed_request_files() {
    _root_helper_setup

    local path="$HELPER_REQUEST_DIR/req.malformed"
    printf 'engage\nextra\n' > "$path"
    chmod 0600 "$path"
    helper_process_pending || true
    assert_contains "$(_root_helper_response malformed)" "status=error" "malformed request rejected"
    assert_contains "$(_root_helper_response malformed)" "message=extra_content" "extra content reason"

    _root_helper_teardown
}

test_root_helper_rejects_symlink_request_files() {
    _root_helper_setup

    local target="$ROOT_HELPER_TEST_DIR/target"
    printf 'engage\n' > "$target"
    ln -s "$target" "$HELPER_REQUEST_DIR/req.link"
    helper_process_pending || true
    assert_contains "$(_root_helper_response link)" "status=error" "symlink request rejected"
    assert_contains "$(_root_helper_response link)" "message=invalid_request_file" "symlink reason"
    assert_eq "$(cat "$ROOT_HELPER_SLEEP_FILE")" "0" "symlink did not change pmset"

    _root_helper_teardown
}

test_root_helper_rejects_hardlink_request_files() {
    _root_helper_setup

    local target="$ROOT_HELPER_TEST_DIR/hard-target"
    printf 'engage\n' > "$target"
    chmod 0600 "$target"
    ln "$target" "$HELPER_REQUEST_DIR/req.hard"
    helper_process_pending || true
    assert_contains "$(_root_helper_response hard)" "status=error" "hardlink request rejected"
    assert_contains "$(_root_helper_response hard)" "message=invalid_request_file" "hardlink reason"
    assert_eq "$(cat "$ROOT_HELPER_SLEEP_FILE")" "0" "hardlink did not change pmset"

    _root_helper_teardown
}

test_root_helper_rejects_request_files_not_owned_by_expected_user() {
    _root_helper_setup

    HELPER_ALLOWED_UID="999999"
    _root_helper_request wrong_owner engage >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response wrong_owner)" "status=error" "wrong owner rejected"
    assert_contains "$(_root_helper_response wrong_owner)" "message=invalid_request_file" "owner reason"
    assert_eq "$(cat "$ROOT_HELPER_SLEEP_FILE")" "0" "wrong owner did not change pmset"

    _root_helper_teardown
}

test_root_helper_reports_engage_pmset_failure() {
    _root_helper_setup

    ROOT_HELPER_PMSET_FAIL=1
    _root_helper_request fail_engage engage >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response fail_engage)" "status=error" "engage pmset failure reported"
    assert_contains "$(_root_helper_response fail_engage)" "message=pmset_engage_failed" "engage failure reason"
    assert_file_absent "$HELPER_STATE_DIR/engaged" "failed engage does not mark helper engaged"

    _root_helper_teardown
}

test_root_helper_reports_release_pmset_failure_and_keeps_engaged() {
    _root_helper_setup

    _root_helper_request engage engage >/dev/null
    helper_process_pending || true
    ROOT_HELPER_PMSET_FAIL=1
    _root_helper_request fail_release release >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response fail_release)" "status=error" "release pmset failure reported"
    assert_contains "$(_root_helper_response fail_release)" "message=pmset_release_failed" "release failure reason"
    assert_file_exists "$HELPER_STATE_DIR/engaged" "failed release leaves helper engaged for retry"
    assert_eq "$(cat "$ROOT_HELPER_SLEEP_FILE")" "1" "failed release did not change SleepDisabled"

    _root_helper_teardown
}

test_root_helper_engage_and_release_restore_baseline() {
    _root_helper_setup

    _root_helper_request engage engage >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response engage)" "status=ok" "engage accepted"
    assert_eq "$(cat "$ROOT_HELPER_SLEEP_FILE")" "1" "engage set SleepDisabled"
    assert_eq "$(cat "$HELPER_STATE_DIR/baseline")" "0" "baseline captured"

    _root_helper_request release release >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response release)" "status=ok" "release accepted"
    assert_eq "$(cat "$ROOT_HELPER_SLEEP_FILE")" "0" "release restored baseline"
    assert_eq "$(cat "$HELPER_STATE_DIR/baseline")" "0" "release keeps root baseline for idempotent restore"
    assert_file_absent "$HELPER_STATE_DIR/engaged" "release marks helper idle"
    assert_contains "$(cat "$ROOT_HELPER_EVENTS")" "pmset -a disablesleep 1" "engage used fixed pmset argv"
    assert_contains "$(cat "$ROOT_HELPER_EVENTS")" "pmset -a disablesleep 0" "release used fixed pmset argv"

    _root_helper_teardown
}

test_root_helper_release_restores_baseline_one() {
    _root_helper_setup

    printf '1\n' > "$ROOT_HELPER_SLEEP_FILE"
    _root_helper_request engage engage >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response engage)" "status=ok" "engage accepted with baseline 1"
    assert_eq "$(cat "$HELPER_STATE_DIR/baseline")" "1" "baseline 1 captured"

    _root_helper_request release release >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response release)" "status=ok" "release accepted with baseline 1"
    assert_eq "$(cat "$ROOT_HELPER_SLEEP_FILE")" "1" "release restored baseline 1"
    assert_eq "$(cat "$HELPER_STATE_DIR/baseline")" "1" "release keeps baseline 1 for idempotent restore"
    assert_file_absent "$HELPER_STATE_DIR/engaged" "release marks helper idle after baseline 1"
    assert_eq "$(grep -c 'pmset -a disablesleep 1' "$ROOT_HELPER_EVENTS")" "2" "release used fixed pmset argv for baseline 1"

    _root_helper_teardown
}

test_root_helper_engage_recaptures_after_release() {
    _root_helper_setup

    _root_helper_request first_engage engage >/dev/null
    helper_process_pending || true
    assert_eq "$(cat "$HELPER_STATE_DIR/baseline")" "0" "first engage captures baseline 0"

    _root_helper_request first_release release >/dev/null
    helper_process_pending || true
    assert_file_absent "$HELPER_STATE_DIR/engaged" "release marks helper idle before recapture"

    printf '1\n' > "$ROOT_HELPER_SLEEP_FILE"
    _root_helper_request second_engage engage >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response second_engage)" "status=ok" "second engage accepted"
    assert_eq "$(cat "$HELPER_STATE_DIR/baseline")" "1" "fresh engage recaptures current baseline after release"
    assert_file_exists "$HELPER_STATE_DIR/engaged" "second engage marks helper engaged"

    _root_helper_teardown
}

test_root_helper_idle_release_is_noop() {
    _root_helper_setup

    _root_helper_request engage engage >/dev/null
    helper_process_pending || true
    _root_helper_request release release >/dev/null
    helper_process_pending || true

    printf '1\n' > "$ROOT_HELPER_SLEEP_FILE"
    _root_helper_request idle_release release >/dev/null
    helper_process_pending || true
    assert_contains "$(_root_helper_response idle_release)" "status=ok" "idle release accepted"
    assert_eq "$(cat "$ROOT_HELPER_SLEEP_FILE")" "1" "idle release does not clobber external SleepDisabled"

    _root_helper_teardown
}
