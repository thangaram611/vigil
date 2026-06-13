#!/usr/bin/env bash
# tests/cli_dispatch_test.sh — drives the BUILT Rust vigil binary
# (target/debug/vigil). The companion cli_preview_test.sh drives the bash
# bin/vigil directly and stays untouched.

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
VIGIL_RUST_BIN="$VIGIL_REPO_ROOT/target/debug/vigil"

_require_rust_bin() {
    if [[ ! -x "$VIGIL_RUST_BIN" ]]; then
        # Build on demand so the suite is self-contained.
        ( cargo build --quiet --manifest-path "$VIGIL_REPO_ROOT/crates/vigil/Cargo.toml" ) \
            || { echo "    FAIL: could not build vigil rust binary"; return 1; }
    fi
    [[ -x "$VIGIL_RUST_BIN" ]]
}

test_rust_version_string_is_exact() {
    _require_rust_bin || return 1
    local out; out=$("$VIGIL_RUST_BIN" --version)
    assert_eq "$out" "vigil 0.1.0-dev" "rust --version matches bash byte-for-byte"
}

test_rust_unknown_command_exits_64() {
    _require_rust_bin || return 1
    local rc
    "$VIGIL_RUST_BIN" boguscmd >/dev/null 2>&1
    rc=$?
    assert_eq "$rc" "64" "unknown command exits EX_USAGE"
}

test_rust_help_exits_zero() {
    _require_rust_bin || return 1
    local rc
    "$VIGIL_RUST_BIN" --help >/dev/null 2>&1
    rc=$?
    assert_eq "$rc" "0" "--help exits 0"
}

test_rust_help_lists_every_subcommand() {
    _require_rust_bin || return 1
    local out; out=$("$VIGIL_RUST_BIN" --help 2>&1)
    local sub
    for sub in setup uninstall start stop status log run reload lock doctor completions; do
        assert_contains "$out" "$sub" "help lists subcommand: $sub" || return 1
    done
}

test_rust_color_never_emits_no_ansi() {
    _require_rust_bin || return 1
    local tmp; tmp=$(mktemp -t vigil-ansi-XXXXXX)
    "$VIGIL_RUST_BIN" --color=never --help >"$tmp" 2>&1
    # Assert no ESC (0x1b) byte present.
    if LC_ALL=C grep -q $'\x1b' "$tmp"; then
        echo "    FAIL: --color=never help contained ANSI escape bytes"
        rm -f "$tmp"
        return 1
    fi
    rm -f "$tmp"
}

# Run the binary under a pseudo-tty so anstream/clap see a terminal. Without this
# the non-tty default (Auto) strips ANSI regardless of --color, which would MASK
# whether the flag is actually honored. macOS `script`: `script -q <file> cmd...`.
_count_esc_under_tty() {
    local tmp; tmp=$(mktemp -t vigil-tty-XXXXXX)
    # Redirect stdin from /dev/null: `script` reads stdin, and without this it
    # would consume the test runner's `while read` loop input (silently dropping
    # subsequent tests in run.sh).
    script -q /dev/null "$VIGIL_RUST_BIN" "$@" >"$tmp" 2>&1 </dev/null
    local n; n=$(LC_ALL=C grep -c $'\x1b' "$tmp")
    rm -f "$tmp"
    printf '%s' "${n//[^0-9]/}"
}

# Contract under a REAL tty: --color=never must STRIP clap's styled help (0 ESC),
# proving the flag governs help/version/error rendering and isn't a no-op masked
# by the non-tty default.
test_rust_color_never_strips_ansi_under_tty() {
    _require_rust_bin || return 1
    local n; n=$(_count_esc_under_tty --color=never --help)
    assert_eq "$n" "0" "--color=never strips ANSI from help under a tty"
}

# Companion: --color=always under a tty must EMIT ANSI for styled help, so the
# forcing side of the contract is exercised too (not just the stripping side).
test_rust_color_always_emits_ansi_under_tty() {
    _require_rust_bin || return 1
    local n; n=$(_count_esc_under_tty --color=always --help)
    if [[ "$n" -le 0 ]]; then
        echo "    FAIL: --color=always emitted no ANSI under a tty (got $n)"
        return 1
    fi
}

# A subcommand's --help is also styled output the flag must govern.
test_rust_color_never_strips_subcommand_help_under_tty() {
    _require_rust_bin || return 1
    local n; n=$(_count_esc_under_tty --color=never status --help)
    assert_eq "$n" "0" "--color=never strips ANSI from subcommand help under a tty"
}

test_rust_delegates_to_bash_via_exec() {
    _require_rust_bin || return 1
    # VIGIL_BASH_BIN points the shim at the real bash vigil; `status --json`
    # in a faked env must produce bash's output (exec succeeded, output flows).
    local root; root=$(mktemp -d -t vigil-shim-XXXXXX)
    export VIGIL_FAKE_ROOT="$root"
    export VIGIL_STATE_DIR="$root/state"
    export VIGIL_LOG_DIR="$root/logs"
    export VIGIL_CONFIG_FILE="$root/no.conf"
    export HOME="$root/home"
    mkdir -p "$root/bin" "$HOME" "$VIGIL_STATE_DIR/active" "$VIGIL_LOG_DIR"
    # Minimal fake launchctl so bash status doesn't blow up.
    cat > "$root/bin/launchctl" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
    chmod +x "$root/bin/launchctl"
    export PATH="$root/bin:$PATH"

    local out rc
    out=$(VIGIL_BASH_BIN="$VIGIL_REPO_ROOT/bin/vigil" "$VIGIL_RUST_BIN" status --json 2>&1)
    rc=$?
    assert_eq "$rc" "0" "delegated status --json exits 0 via exec" || { rm -rf "$root"; return 1; }
    assert_contains "$out" '"daemon_scan_state"' "bash status output flowed through the exec shim" \
        || { rm -rf "$root"; return 1; }

    rm -rf "$root"
}
