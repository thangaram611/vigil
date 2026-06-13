#!/usr/bin/env bash
# tests/detect_parity_test.sh — Golden detect oracle. Asserts that the Rust
# `vigil debug detect --ps-comm <f> --ps-cmd <f>` produces byte-identical TSV
# rows (after sorting) to the bash `vigil_detect_all <comm> <cmd>` two-file mode,
# over the SAME committed fixtures and over a synthetic VS Code Insiders
# main-vs-helper pair.
#
# Mirrors tests/config_parity_test.sh's oracle shape: build the Rust bin on
# demand, run both engines, sort both sides, diff. An empty diff is the parity
# proof. Globbed by tests/run.sh via tests/*_test.sh — do NOT rename this file.

set -uo pipefail

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
VIGIL_RUST_BIN="$VIGIL_REPO_ROOT/target/debug/vigil"

_require_rust_bin() {
    if [[ ! -x "$VIGIL_RUST_BIN" ]]; then
        ( cargo build --quiet --manifest-path "$VIGIL_REPO_ROOT/crates/vigil/Cargo.toml" ) \
            || { printf '    FAIL: could not build vigil rust binary\n'; return 1; }
    fi
    [[ -x "$VIGIL_RUST_BIN" ]]
}

# Run the bash oracle: source detect.sh, call vigil_detect_all <comm> <cmd>.
_bash_detect() {
    local comm="$1" cmd="$2"
    VIGIL_LIB_DIR="$VIGIL_REPO_ROOT/lib" bash -c \
        'source "$VIGIL_LIB_DIR/detect.sh"; vigil_detect_all "$1" "$2"' _ \
        "$comm" "$cmd"
}

# Run the Rust oracle: vigil debug detect --ps-comm <comm> --ps-cmd <cmd>.
_rust_detect() {
    local comm="$1" cmd="$2"
    "$VIGIL_RUST_BIN" debug detect --ps-comm "$comm" --ps-cmd "$cmd"
}

# Assert sorted bash == sorted Rust for a given (comm, cmd) pair.
_assert_detect_parity() {
    local label="$1" comm="$2" cmd="$3"
    local bash_out rust_out
    bash_out=$(_bash_detect "$comm" "$cmd" | LC_ALL=C sort)
    rust_out=$(_rust_detect "$comm" "$cmd" | LC_ALL=C sort)
    if [[ "$bash_out" == "$rust_out" ]]; then
        return 0
    fi
    printf '    DIFF for %s:\n' "$label"
    diff <(printf '%s\n' "$bash_out") <(printf '%s\n' "$rust_out") | sed 's/^/      /'
    return 1
}

# Canonical committed fixtures.
test_detect_parity_on_committed_fixtures() {
    _require_rust_bin || return 1
    _assert_detect_parity "committed fixtures" \
        "$VIGIL_REPO_ROOT/tests/fixtures/ps-axww-comm-snapshot.txt" \
        "$VIGIL_REPO_ROOT/tests/fixtures/ps-axww-snapshot.txt" || return 1
}

# Synthetic VS Code Insiders main vs Helper — pins the host/helper carve-out
# cross-engine (mirrors the bash detect_test synthetic cases).
test_detect_parity_vscode_host_vs_helper() {
    _require_rust_bin || return 1
    local tmp_comm tmp_cmd
    tmp_comm=$(mktemp); tmp_cmd=$(mktemp)
    {
        printf '22222 /Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Code - Insiders\n'
        printf '22223 /Applications/Visual Studio Code - Insiders.app/Contents/Frameworks/Code - Insiders Helper.app/Contents/MacOS/Code - Insiders Helper\n'
    } > "$tmp_comm"
    {
        printf '22222 /Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Code - Insiders\n'
        printf '22223 /Applications/Visual Studio Code - Insiders.app/Contents/Frameworks/Code - Insiders Helper.app/Contents/MacOS/Code - Insiders Helper --type=utility\n'
    } > "$tmp_cmd"
    _assert_detect_parity "vscode host vs helper" "$tmp_comm" "$tmp_cmd"
    local rc=$?
    rm -f "$tmp_comm" "$tmp_cmd"
    return $rc
}
