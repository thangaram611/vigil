#!/usr/bin/env bash
# tests/lock_test.sh — lock command and helper-plumbed permission-flow tests.

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

_setup_fake_base() {
    local root
    root=$(mktemp -d -t vigil-lock-XXXXXX)
    export VIGIL_FAKE_ROOT="$root"
    export VIGIL_STATE_DIR="$root/state"
    export VIGIL_LOG_DIR="$root/logs"
    export VIGIL_INSTALL_DIR="$root/install"
    export VIGIL_CONFIG_FILE="$root/no.conf"
    export VIGIL_LOCK_HELPER="$root/bin/vigil-lock-helper"
    export VIGIL_LOCK_COMBO="ctrl+alt+shift+cmd+l"
    export VIGIL_LOCK_MAX_SECS="28800"
    mkdir -p "$VIGIL_STATE_DIR" "$VIGIL_LOG_DIR" "$VIGIL_INSTALL_DIR/bin" "$root/bin"
    : > "$VIGIL_CONFIG_FILE"
    cat > "$root/bin/caffeinate" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" == "-i" ]]; then
    shift
    exec "$@"
fi
printf 'unexpected caffeinate args: %s\n' "$*" >&2
exit 64
EOF
    chmod +x "$root/bin/caffeinate"
    export PATH="$root/bin:$PATH"
}

_setup_fake_uname() {
    local kind="$1"
    cat > "$VIGIL_FAKE_ROOT/bin/uname" <<EOF
#!/usr/bin/env bash
printf '%s\n' "$kind"
EOF
    chmod +x "$VIGIL_FAKE_ROOT/bin/uname"
}

_setup_fast_sleep() {
    cat > "$VIGIL_FAKE_ROOT/bin/sleep" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$VIGIL_FAKE_ROOT/bin/sleep"
}

_cleanup_fake_lock_env() {
    rm -rf "${VIGIL_FAKE_ROOT:-}"
}

test_help_includes_lock_command() {
    local out
    out=$("$VIGIL_REPO_ROOT/bin/vigil" --help)
    assert_contains "$out" "vigil lock         Freeze input until configured combo, then continue" "top-level help lists lock"
    assert_contains "$out" "vigil lock --help  Show lock mode usage" "lock help line listed"
    assert_contains "$out" "vigil lock doctor  Run macOS permission smoke test for the lock helper" "lock doctor help line listed"
}

test_lock_lock_help() {
    local out
    out=$("$VIGIL_REPO_ROOT/bin/vigil" lock --help)
    assert_contains "$out" "Usage:" "lock help has usage header"
    assert_contains "$out" "vigil lock [--combo <combo>] [--max-secs <seconds>]" "lock usage printed"
    assert_contains "$out" "vigil lock doctor [--prompt]" "lock doctor usage printed"
}

test_lock_doctor_missing_helper() {
    _setup_fake_base
    _setup_fake_uname Darwin

    local out rc=0
    if out=$("$VIGIL_REPO_ROOT/bin/vigil" lock doctor 2>&1); then
        rc=0
    else
        rc=$?
    fi
    assert_not_eq "$rc" 0 "lock doctor missing helper should fail"
    assert_contains "$out" "vigil lock: missing helper" "missing helper is reported"
    assert_contains "$out" "vigil setup (or vigil reload)" "helpful guidance includes setup/reload"

    _cleanup_fake_lock_env
}

test_lock_doctor_reports_failed_fields() {
    _setup_fake_base
    _setup_fake_uname Darwin
    _setup_fast_sleep

    cat > "$VIGIL_LOCK_HELPER" <<'EOF'
#!/usr/bin/env bash
case "$1 $2" in
  "--check-permissions --json")
    printf '%s\n' '{"platform":"macos","listen_event_access":false,"accessibility_trusted":true,"post_event_access":true,"tap_create_active_session_ok":false}'
    exit 20
    ;;
  "--check-permissions --json --prompt")
    printf '%s\n' '{"platform":"macos","listen_event_access":false,"accessibility_trusted":true,"post_event_access":true,"tap_create_active_session_ok":false}'
    exit 20
    ;;
  "--freeze"*)
    echo "unexpected freeze args: $*"
    exit 1
    ;;
  *)
    echo "unexpected args: $*"
    exit 1
    ;;
esac
EOF
    chmod +x "$VIGIL_LOCK_HELPER"

    local out rc=0
    if out=$("$VIGIL_REPO_ROOT/bin/vigil" lock doctor 2>&1); then
        rc=0
    else
        rc=$?
    fi
    assert_not_eq "$rc" 0 "lock doctor should fail when fields are false"
    assert_contains "$out" "listen_event_access:       false" "listen field parsed"
    assert_contains "$out" "accessibility_trusted:     true" "accessibility field parsed"
    assert_contains "$out" "post_event_access:         true (informational)" "post-event field parsed"
    assert_contains "$out" "tap_create_active_session_ok: false" "tap field parsed"
    assert_contains "$out" "lock guard readiness: not ready" "doctor reports not ready"

    _cleanup_fake_lock_env
}

test_lock_requires_doorstep_doctor_before_arming() {
    _setup_fake_base
    _setup_fake_uname Darwin
    _setup_fast_sleep
    local events="$VIGIL_FAKE_ROOT/events.log"

    cat > "$VIGIL_LOCK_HELPER" <<EOF
#!/usr/bin/env bash
case "\$1 \$2" in
  "--check-permissions --json")
    printf '%s\n' '{"platform":"macos","listen_event_access":false,"accessibility_trusted":true,"post_event_access":true,"tap_create_active_session_ok":true}'
    exit 20
    ;;
  "--freeze"*)
    printf '%s\n' "\$*" >> "$events"
    exit 0
    ;;
  *)
    printf 'unexpected %s\n' "\$*" >> "$events"
    exit 1
    ;;
esac
EOF
    chmod +x "$VIGIL_LOCK_HELPER"

    local out rc=0
    if out=$("$VIGIL_REPO_ROOT/bin/vigil" lock --combo ctrl+alt+shift+cmd+l --max-secs 10 2>&1); then
        rc=0
    else
        rc=$?
    fi
    assert_not_eq "$rc" 0 "lock should refuse to arm when preflight fails"
    assert_contains "$out" "doctor preflight failed; run this first" "preflight refusal message"
    assert_file_absent "$events" "freeze path was not entered"

    _cleanup_fake_lock_env
}

test_lock_uses_config_and_launches_freeze_with_expected_args() {
    _setup_fake_base
    _setup_fake_uname Darwin
    _setup_fast_sleep
    export VIGIL_LOCK_COMBO="cmd+alt+shift+ctrl+x"
    export VIGIL_LOCK_MAX_SECS="42"
    local events="$VIGIL_FAKE_ROOT/events.log"

    cat > "$VIGIL_LOCK_HELPER" <<EOF
#!/usr/bin/env bash
case "\$1 \$2" in
  "--check-permissions --json")
    printf '%s\n' '{"platform":"macos","listen_event_access":true,"accessibility_trusted":true,"post_event_access":true,"tap_create_active_session_ok":true}'
    exit 0
    ;;
  "--freeze"*)
    printf '%s\n' "\$*" >> "$events"
    exit 0
    ;;
  *)
    printf 'unexpected %s\n' "\$*"
    exit 1
    ;;
esac
EOF
    chmod +x "$VIGIL_LOCK_HELPER"

    local out
    if ! out=$("$VIGIL_REPO_ROOT/bin/vigil" lock 2>&1); then
        echo "lock command failed unexpectedly"
        printf '%s\n' "$out"
        _cleanup_fake_lock_env
        return 1
    fi
    assert_file_exists "$events" "freeze was launched"
    local freeze_line
    freeze_line=$(cat "$events")
    assert_contains "$freeze_line" "--freeze" "freeze command executed"
    assert_contains "$freeze_line" "--combo cmd+alt+shift+ctrl+x" "configured combo passed to helper"
    assert_contains "$freeze_line" "--max-secs 42" "max secs passed from config"
    assert_contains "$out" "lock combo:  cmd+alt+shift+ctrl+x" "lock command displays config-combo"
    assert_contains "$out" "max seconds: 42" "lock command displays config max secs"
    assert_contains "$out" "sleep hold:  best effort" "lock command documents best-effort sleep hold"

    _cleanup_fake_lock_env
}

test_lock_rejects_non_macos_os_without_failing_mac_flow() {
    _setup_fake_base
    _setup_fake_uname Linux

    local out rc=0
    if out=$("$VIGIL_REPO_ROOT/bin/vigil" lock 2>&1); then
        rc=0
    else
        rc=$?
    fi
    assert_not_eq "$rc" 0 "lock should not run on non-macOS"
    assert_contains "$out" "phase-4 local lock guard is macOS-only" "non-macOS guard message"

    _cleanup_fake_lock_env
}
