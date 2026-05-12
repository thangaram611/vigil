#!/usr/bin/env bash
# tests/newsyslog_test.sh — regression-guard for two things:
#
#   1. The init-order fix in lib/common.sh — a vigil.conf that sets only
#      VIGIL_LOG_DIR (NOT VIGIL_LOG_FILE directly) must correctly re-derive
#      VIGIL_LOG_FILE after vigil_load_config runs. Before the fix, the
#      top-level `VIGIL_LOG_FILE="${VIGIL_LOG_FILE:-...}"` resolved at
#      source-time against the default VIGIL_LOG_DIR and stayed pinned.
#
#   2. The etc/vigil.newsyslog.in template renders the expected fields
#      (owner, mode, count, size, GZ flag) — drift here means we silently
#      shipped a broken rotation config.
#
# This test MUST drive the VIGIL_LOG_DIR → vigil_load_config → VIGIL_LOG_FILE
# derivation path. Setting VIGIL_LOG_FILE directly in the test would bypass
# the bug.

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

# Build a clean environment: temp config dir, temp log dir, fresh sourcing.
# We isolate VIGIL_CONFIG_FILE and unset any pre-existing VIGIL_LOG_FILE so
# the regression-guard is meaningful.
_newsyslog_setup() {
    NEWSYSLOG_TEST_DIR=$(mktemp -d -t vigil-newsyslog-test)
    NEWSYSLOG_TEST_LOG_DIR="$NEWSYSLOG_TEST_DIR/custom-log-dir"
    mkdir -p "$NEWSYSLOG_TEST_LOG_DIR"
    NEWSYSLOG_TEST_CONF="$NEWSYSLOG_TEST_DIR/vigil.conf"
    cat > "$NEWSYSLOG_TEST_CONF" <<EOF
# Test config — overrides VIGIL_LOG_DIR only. If the init-order bug regresses,
# VIGIL_LOG_FILE will NOT be re-derived and the rendered newsyslog config will
# point at \$HOME/Library/Logs/vigil/daemon.log instead of the path below.
VIGIL_LOG_DIR="$NEWSYSLOG_TEST_LOG_DIR"
EOF

    # Critical: blow away VIGIL_LOG_FILE from the environment before sourcing.
    # If a parent shell exported it, the bug could be masked.
    unset VIGIL_LOG_FILE

    # Source common.sh fresh and load the test config.
    export VIGIL_CONFIG_FILE="$NEWSYSLOG_TEST_CONF"
    # shellcheck source=../lib/common.sh
    source "$VIGIL_REPO_ROOT/lib/common.sh"
    vigil_load_config
}

_newsyslog_teardown() {
    [[ -n "${NEWSYSLOG_TEST_DIR:-}" && -d "$NEWSYSLOG_TEST_DIR" ]] && rm -rf "$NEWSYSLOG_TEST_DIR"
}

# Inline the same sed substitution that cmd_render_newsyslog runs. We can't
# source bin/vigil directly because it dispatches via `main "$@"` at the
# bottom; replicating the one-liner here keeps the test focused on the bug.
_newsyslog_render() {
    local user; user=$(id -un)
    sed \
        -e "s|@VIGIL_LOG_FILE@|$VIGIL_LOG_FILE|g" \
        -e "s|@VIGIL_USER@|$user|g" \
        "$VIGIL_REPO_ROOT/etc/vigil.newsyslog.in"
}

test_log_file_is_rederived_from_config_log_dir() {
    _newsyslog_setup
    # The primary assertion: VIGIL_LOG_FILE must reflect the config-overridden
    # VIGIL_LOG_DIR, NOT the default ~/Library/Logs/vigil/daemon.log.
    local expected="$NEWSYSLOG_TEST_LOG_DIR/daemon.log"
    assert_eq "$VIGIL_LOG_FILE" "$expected" \
        "VIGIL_LOG_FILE must be derived from config-overridden VIGIL_LOG_DIR"
    # And it must NOT have stayed pinned to the default location.
    assert_not_contains "$VIGIL_LOG_FILE" "Library/Logs/vigil/daemon.log" \
        "VIGIL_LOG_FILE must not point at the default path when config overrides VIGIL_LOG_DIR"
    _newsyslog_teardown
}

test_rendered_newsyslog_uses_overridden_log_path() {
    _newsyslog_setup
    local rendered; rendered=$(_newsyslog_render)
    # The first non-comment, non-blank line must start with the derived log
    # path. grep -v skips comment + blank; head -n1 grabs the data row.
    local data_line; data_line=$(printf '%s\n' "$rendered" | grep -v '^[[:space:]]*#' | grep -v '^[[:space:]]*$' | head -n 1)
    local first_col;  first_col=$(printf '%s' "$data_line" | awk '{print $1}')
    assert_eq "$first_col" "$NEWSYSLOG_TEST_LOG_DIR/daemon.log" \
        "rendered first column must be the derived log path"
    _newsyslog_teardown
}

test_rendered_newsyslog_has_expected_fields() {
    _newsyslog_setup
    local rendered; rendered=$(_newsyslog_render)
    local user; user=$(id -un)
    local data_line; data_line=$(printf '%s\n' "$rendered" | grep -v '^[[:space:]]*#' | grep -v '^[[:space:]]*$' | head -n 1)
    # Columns (newsyslog.conf(5) — order matters):
    #   1: logfilename   2: owner:group   3: mode   4: count   5: size   6: when   7: flags
    local owner mode count size when flags
    owner=$(printf '%s' "$data_line" | awk '{print $2}')
    mode=$( printf '%s' "$data_line" | awk '{print $3}')
    count=$(printf '%s' "$data_line" | awk '{print $4}')
    size=$( printf '%s' "$data_line" | awk '{print $5}')
    when=$( printf '%s' "$data_line" | awk '{print $6}')
    flags=$(printf '%s' "$data_line" | awk '{print $7}')
    assert_eq "$owner" "${user}:staff" "owner field must be \${user}:staff"
    assert_eq "$mode"  "644"           "mode field must be 644"
    assert_eq "$count" "5"             "count field must be 5 (5 generations kept)"
    assert_eq "$size"  "1024"          "size field must be 1024 KiB (1 MiB rotation threshold)"
    assert_eq "$when"  "*"             "when field must be '*' (size-only)"
    assert_eq "$flags" "GZ"            "flags field must be GZ (gzip rotated files)"
    _newsyslog_teardown
}
