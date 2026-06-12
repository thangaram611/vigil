#!/usr/bin/env bash
# tests/lib.sh — assertion helpers shared by both the runner and individual
# test files (when run standalone). No side-effects; safe to source repeatedly.

assert_eq() {
    if [[ "$1" != "$2" ]]; then
        printf '    FAIL: expected %q, got %q  (%s)\n' "$2" "$1" "${3:-no msg}"
        return 1
    fi
}

assert_not_eq() {
    if [[ "$1" == "$2" ]]; then
        printf '    FAIL: expected not %q, got %q  (%s)\n' "$1" "$2" "${3:-no msg}"
        return 1
    fi
}

assert_contains() {
    if [[ "$1" != *"$2"* ]]; then
        printf '    FAIL: expected to contain %q, got %q  (%s)\n' "$2" "$1" "${3:-no msg}"
        return 1
    fi
}

assert_not_contains() {
    if [[ "$1" == *"$2"* ]]; then
        printf '    FAIL: expected NOT to contain %q, got %q  (%s)\n' "$2" "$1" "${3:-no msg}"
        return 1
    fi
}

assert_file_exists() {
    if [[ ! -e "$1" ]]; then
        printf '    FAIL: expected file %q to exist  (%s)\n' "$1" "${2:-no msg}"
        return 1
    fi
}

assert_file_absent() {
    if [[ -e "$1" ]]; then
        printf '    FAIL: expected file %q to NOT exist  (%s)\n' "$1" "${2:-no msg}"
        return 1
    fi
}
