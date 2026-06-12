#!/usr/bin/env bash
# tests/power_reconcile_test.sh — pmset/caffeinate transition tests with fake
# system binaries, so no real power settings are changed.

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

_setup_fake_power_env() {
    local root; root=$(mktemp -d -t vigil-power-XXXXXX)
    export VIGIL_FAKE_ROOT="$root"
    export VIGIL_FAKE_SLEEP_FILE="$root/sleepdisabled"
    export VIGIL_FAKE_EVENTS="$root/events.log"
    export VIGIL_STATE_DIR="$root/state"
    export VIGIL_LOG_DIR="$root/logs"
    export VIGIL_CONFIG_FILE="$root/no.conf"
    export VIGIL_LIB_DIR="$VIGIL_REPO_ROOT/lib"
    mkdir -p "$root/bin" "$VIGIL_STATE_DIR/active" "$VIGIL_LOG_DIR"
    printf '0\n' > "$VIGIL_FAKE_SLEEP_FILE"
    : > "$VIGIL_FAKE_EVENTS"

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

    cat > "$root/bin/caffeinate" <<'FAKE_CAFFEINATE'
#!/usr/bin/env bash
printf 'caffeinate %s\n' "$*" >> "$VIGIL_FAKE_EVENTS"
trap 'exit 0' TERM INT
while true; do sleep 60 & wait $!; done
FAKE_CAFFEINATE

    chmod +x "$root/bin/pmset" "$root/bin/caffeinate"
    export PATH="$root/bin:$PATH"

    # shellcheck source=../lib/common.sh
    source "$VIGIL_LIB_DIR/common.sh"
    # shellcheck source=../lib/pmset.sh
    source "$VIGIL_LIB_DIR/pmset.sh"
    vigil_power_helper_request() {
        local action="$1"
        printf 'helper %s\n' "$action" >> "$VIGIL_FAKE_EVENTS"
        case "$action" in
            engage)
                printf '1\n' > "$VIGIL_FAKE_SLEEP_FILE"
                printf 'status=ok\naction=engage\nbaseline=0\ncurrent=1\nmessage=ok\n'
                ;;
            release)
                printf '%s\n' "$(vigil_pmset_baseline_value)" > "$VIGIL_FAKE_SLEEP_FILE"
                printf 'status=ok\naction=release\nbaseline=none\ncurrent=%s\nmessage=ok\n' "$(cat "$VIGIL_FAKE_SLEEP_FILE")"
                ;;
            status)
                printf 'status=ok\naction=status\nbaseline=none\ncurrent=%s\nmessage=ok\n' "$(cat "$VIGIL_FAKE_SLEEP_FILE")"
                ;;
            *)
                printf 'status=error\naction=%s\nbaseline=none\ncurrent=%s\nmessage=bad_action\n' "$action" "$(cat "$VIGIL_FAKE_SLEEP_FILE")"
                return 1
                ;;
        esac
    }
    vigil_load_config
    vigil_ensure_dirs
}

_cleanup_fake_power_env() {
    if [[ -f "${VIGIL_CAFFEINATE_PIDFILE:-}" ]]; then
        local cpid; cpid=$(cat "$VIGIL_CAFFEINATE_PIDFILE" 2>/dev/null || true)
        [[ -n "$cpid" ]] && kill "$cpid" 2>/dev/null || true
        [[ -n "$cpid" ]] && wait "$cpid" 2>/dev/null || true
    fi
    rm -rf "${VIGIL_FAKE_ROOT:-}"
}

test_engage_and_release_restore_baseline() {
    _setup_fake_power_env

    vigil_pmset_engage
    assert_eq "$(cat "$VIGIL_FAKE_SLEEP_FILE")" "1" "engage sets SleepDisabled=1"
    assert_eq "$(vigil_pmset_baseline_value)" "0" "baseline captured before engage"
    assert_file_exists "$VIGIL_CAFFEINATE_PIDFILE" "engage writes caffeinate pidfile"
    vigil_pmset_caffeinate_alive

    vigil_pmset_release
    assert_eq "$(cat "$VIGIL_FAKE_SLEEP_FILE")" "0" "release restores baseline"
    assert_file_absent "$VIGIL_BASELINE_FILE" "release clears baseline"
    assert_file_absent "$VIGIL_CAFFEINATE_PIDFILE" "release clears caffeinate pidfile"

    _cleanup_fake_power_env
}

test_release_uses_helper_release_when_baseline_is_one() {
    _setup_fake_power_env

    printf '1\n' > "$VIGIL_FAKE_SLEEP_FILE"
    vigil_pmset_engage
    assert_eq "$(vigil_pmset_baseline_value)" "1" "baseline captures pre-existing SleepDisabled=1"

    vigil_pmset_release
    assert_eq "$(cat "$VIGIL_FAKE_SLEEP_FILE")" "1" "release restores SleepDisabled=1 baseline"
    assert_contains "$(cat "$VIGIL_FAKE_EVENTS")" "helper release" "release requests helper release"
    assert_file_absent "$VIGIL_BASELINE_FILE" "release clears baseline when baseline is 1"

    _cleanup_fake_power_env
}

test_reconcile_reasserts_sleepdisabled_drift() {
    _setup_fake_power_env

    vigil_pmset_engage
    printf '0\n' > "$VIGIL_FAKE_SLEEP_FILE"
    vigil_pmset_reconcile_engaged
    assert_eq "$(cat "$VIGIL_FAKE_SLEEP_FILE")" "1" "reconcile restores SleepDisabled=1"
    assert_contains "$(cat "$VIGIL_FAKE_EVENTS")" "helper engage" "reconcile requested helper engage"

    vigil_pmset_release
    _cleanup_fake_power_env
}

test_reconcile_restarts_missing_caffeinate() {
    _setup_fake_power_env

    vigil_pmset_engage
    local old_pid; old_pid=$(cat "$VIGIL_CAFFEINATE_PIDFILE")
    kill "$old_pid" 2>/dev/null || true
    wait "$old_pid" 2>/dev/null || true
    vigil_pmset_reconcile_engaged
    local new_pid; new_pid=$(cat "$VIGIL_CAFFEINATE_PIDFILE")
    if [[ "$old_pid" == "$new_pid" ]]; then
        echo "    FAIL: reconcile reused missing caffeinate pid"
        _cleanup_fake_power_env
        return 1
    fi
    vigil_pmset_caffeinate_alive

    vigil_pmset_release
    _cleanup_fake_power_env
}

test_reconcile_rejects_reused_non_caffeinate_pid() {
    _setup_fake_power_env

    printf '%s\n' "$$" > "$VIGIL_CAFFEINATE_PIDFILE"
    if vigil_pmset_caffeinate_alive; then
        echo "    FAIL: current test shell must not count as vigil caffeinate"
        _cleanup_fake_power_env
        return 1
    fi
    printf '1\n' > "$VIGIL_FAKE_SLEEP_FILE"
    vigil_pmset_reconcile_engaged
    local new_pid; new_pid=$(cat "$VIGIL_CAFFEINATE_PIDFILE")
    if [[ "$new_pid" == "$$" ]]; then
        echo "    FAIL: reconcile did not replace reused non-caffeinate pid"
        _cleanup_fake_power_env
        return 1
    fi
    vigil_pmset_caffeinate_alive

    vigil_pmset_release
    _cleanup_fake_power_env
}

test_startup_recovery_keeps_hold_when_refs_remain() {
    _setup_fake_power_env

    printf '{"SleepDisabled":0,"captured_at":1700000000}\n' > "$VIGIL_BASELINE_FILE"
    if ! vigil_pmset_recover_startup 1 1; then
        echo "    FAIL: recovery should report engaged"
        _cleanup_fake_power_env
        return 1
    fi
    assert_eq "$(cat "$VIGIL_FAKE_SLEEP_FILE")" "1" "startup recovery reasserts SleepDisabled"
    assert_file_exists "$VIGIL_BASELINE_FILE" "startup recovery preserves baseline while active"
    vigil_pmset_caffeinate_alive

    vigil_pmset_release
    _cleanup_fake_power_env
}

test_startup_recovery_releases_when_no_refs() {
    _setup_fake_power_env

    printf '{"SleepDisabled":0,"captured_at":1700000000}\n' > "$VIGIL_BASELINE_FILE"
    printf '1\n' > "$VIGIL_FAKE_SLEEP_FILE"
    if vigil_pmset_recover_startup 0 1; then
        echo "    FAIL: recovery should not report engaged with no refs"
        _cleanup_fake_power_env
        return 1
    fi
    assert_eq "$(cat "$VIGIL_FAKE_SLEEP_FILE")" "0" "startup recovery restores baseline when idle"
    assert_file_absent "$VIGIL_BASELINE_FILE" "idle recovery clears baseline"

    _cleanup_fake_power_env
}
