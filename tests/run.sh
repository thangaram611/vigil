#!/usr/bin/env bash
# tests/run.sh — minimal test runner. Tiny "expect" framework, no bats dependency.
#
#   ./tests/run.sh                  # run all tests in tests/*_test.sh
#   ./tests/run.sh detect           # run only tests matching "detect"
#
# Each test file is sourced and every function whose name starts with "test_"
# runs in a subshell. A failing assertion exits the subshell with non-zero,
# which is caught by the runner.

set -uo pipefail

PASS=0
FAIL=0
FILTER="${1:-}"
TEST_TMP_ROOT=$(mktemp -d -t vigil-tests-XXXXXX)
TEST_GUARD_BIN="$TEST_TMP_ROOT/bin"
TEST_NO_SUDO_LOG="$TEST_TMP_ROOT/sudo-attempts.log"
mkdir -p "$TEST_GUARD_BIN"
cat > "$TEST_GUARD_BIN/sudo" <<'EOF'
#!/usr/bin/env bash
printf 'ERROR: tests must not invoke sudo: sudo %s\n' "$*" >&2
if [[ -n "${VIGIL_TEST_NO_SUDO_LOG:-}" ]]; then
    printf 'sudo %s\n' "$*" >> "$VIGIL_TEST_NO_SUDO_LOG"
fi
exit 97
EOF
chmod +x "$TEST_GUARD_BIN/sudo"
export VIGIL_TEST_NO_ADMIN=1
export VIGIL_TEST_NO_SUDO_LOG="$TEST_NO_SUDO_LOG"
export PATH="$TEST_GUARD_BIN:$PATH"
trap 'rm -rf "$TEST_TMP_ROOT"' EXIT

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

run_file() {
    local file="$1"
    local short; short=$(basename "$file" .sh)
    [[ -n "$FILTER" && "$short" != *"$FILTER"* ]] && return 0

    printf '%s\n' "$short"
    # Capture function names that start with "test_" by sourcing in a subshell.
    local fns
    fns=$(bash -c "set +u; source '$(dirname "$file")/lib.sh'; source '$file'; declare -F | awk '\$3 ~ /^test_/ {print \$3}'")
    while read -r fn; do
        [[ -z "$fn" ]] && continue
        if (
            set -uo pipefail
            # shellcheck source=/dev/null
            source "$(dirname "$file")/lib.sh"
            # shellcheck source=/dev/null
            source "$file"
            "$fn"
        ) >/tmp/vigil_test_out 2>&1; then
            printf '  ok %s\n' "$fn"
            PASS=$((PASS + 1))
        else
            printf '  FAIL %s\n' "$fn"
            sed 's/^/    /' /tmp/vigil_test_out
            FAIL=$((FAIL + 1))
        fi
    done <<< "$fns"
}

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export VIGIL_REPO_ROOT="$repo_root"

for f in "$repo_root"/tests/*_test.sh; do
    [[ -e "$f" ]] || continue
    run_file "$f"
done

echo
if [[ -s "$TEST_NO_SUDO_LOG" ]]; then
    echo "sudo attempts were blocked during tests:"
    sed 's/^/  /' "$TEST_NO_SUDO_LOG"
    FAIL=$((FAIL + 1))
fi
printf 'pass=%s fail=%s\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
