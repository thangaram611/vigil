#!/usr/bin/env bash
# tests/config_parity_test.sh — Parity oracle: asserts that `vigil config --kv`
# (Rust) matches the bash oracle (lib/common.sh + vigil_load_config) for each
# matrix case.
#
# CONF FORMAT NOTE: bash conf is shell syntax (`VIGIL_LOG_DIR="..."`); Rust TOML
# conf uses lowercase serde field names (`log_dir = "..."`). Parity is asserted
# on RESOLVED VALUES for semantically-equivalent inputs, not on conf file bytes.
#
# EXCLUDED from diff: VIGIL_REPO_ROOT ($(pwd)-dependent, non-deterministic across
# bash vs Rust invocation contexts).
#
# See spec §7 for the full matrix definition.
#
# Globbed by tests/run.sh via tests/*_test.sh — do NOT rename this file.

set -uo pipefail

VIGIL_REPO_ROOT="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
VIGIL_RUST_BIN="$VIGIL_REPO_ROOT/target/debug/vigil"
ORACLE="$VIGIL_REPO_ROOT/tests/fixtures/config/dump_bash_config.sh"

# ── helpers ──────────────────────────────────────────────────────────────────

_require_rust_bin() {
    if [[ ! -x "$VIGIL_RUST_BIN" ]]; then
        ( cargo build --quiet --manifest-path "$VIGIL_REPO_ROOT/crates/vigil/Cargo.toml" ) \
            || { printf '    FAIL: could not build vigil rust binary\n'; return 1; }
    fi
    [[ -x "$VIGIL_RUST_BIN" ]]
}

# Run the bash oracle in a subshell with a clean env built from the supplied
# associative-array entries (KEY VALUE KEY VALUE ...) and an optional TOML conf.
# Sets global _ORACLE_OUT.
_run_bash_oracle() {
    # $1 = bash conf content ('' for none)
    # $2... = KEY VALUE KEY VALUE... env vars
    local bash_conf="$1"; shift
    local -a env_pairs=("$@")

    local tmpdir; tmpdir=$(mktemp -d -t vigil-par-oracle-XXXXXX)
    local home_dir="$tmpdir/home"
    mkdir -p "$home_dir"

    local bash_conf_file="$tmpdir/vigil.sh.conf"
    if [[ -n "$bash_conf" ]]; then
        printf '%s\n' "$bash_conf" > "$bash_conf_file"
    else
        touch "$bash_conf_file"
    fi

    # Build the env for the subshell invocation.
    local env_cmd=()
    env_cmd+=( HOME="$home_dir" )
    env_cmd+=( VIGIL_CONFIG_FILE="$bash_conf_file" )
    env_cmd+=( VIGIL_REPO_ROOT="$VIGIL_REPO_ROOT" )
    # Pass caller-supplied env pairs.
    local i=0
    while (( i < ${#env_pairs[@]} )); do
        local k="${env_pairs[$i]}"
        local v="${env_pairs[$((i+1))]}"
        env_cmd+=( "$k=$v" )
        i=$(( i + 2 ))
    done

    # Unset VIGIL_LOG_FILE so the derivation is not masked.
    _ORACLE_OUT=$(
        env -i "${env_cmd[@]}" \
            VIGIL_LOG_FILE="" \
            bash -c "unset VIGIL_LOG_FILE; source '$ORACLE'" 2>&1
    )
    local rc=$?
    rm -rf "$tmpdir"
    # Strip the VIGIL_LOG_FILE="" no-op line that might appear.
    return $rc
}

# Run the Rust binary with the same env. Sets global _RUST_OUT.
_run_rust_config() {
    # $1 = TOML conf content ('' for none)
    # $2... = KEY VALUE KEY VALUE...
    local toml_conf="$1"; shift
    local -a env_pairs=("$@")

    local tmpdir; tmpdir=$(mktemp -d -t vigil-par-rust-XXXXXX)
    local home_dir="$tmpdir/home"
    mkdir -p "$home_dir"

    local toml_conf_file="$tmpdir/vigil.toml.conf"
    if [[ -n "$toml_conf" ]]; then
        printf '%s\n' "$toml_conf" > "$toml_conf_file"
    else
        touch "$toml_conf_file"
    fi

    local env_cmd=()
    env_cmd+=( HOME="$home_dir" )
    env_cmd+=( VIGIL_CONFIG_FILE="$toml_conf_file" )
    env_cmd+=( VIGIL_REPO_ROOT="$VIGIL_REPO_ROOT" )
    local i=0
    while (( i < ${#env_pairs[@]} )); do
        local k="${env_pairs[$i]}"
        local v="${env_pairs[$((i+1))]}"
        env_cmd+=( "$k=$v" )
        i=$(( i + 2 ))
    done

    _RUST_OUT=$(
        env -i "${env_cmd[@]}" \
            "$VIGIL_RUST_BIN" config --kv 2>&1
    )
    local rc=$?
    rm -rf "$tmpdir"
    return $rc
}

# Assert bash oracle == Rust output (excluding VIGIL_REPO_ROOT which is
# non-deterministic across bash-source vs Rust invocation contexts).
_assert_parity() {
    local label="$1"
    local bash_out="$1"; bash_out="$_ORACLE_OUT"
    local rust_out="$_RUST_OUT"

    # Filter out VIGIL_REPO_ROOT from both sides.
    bash_out=$(printf '%s\n' "$_ORACLE_OUT" | grep -v '^VIGIL_REPO_ROOT=')
    rust_out=$(printf '%s\n' "$_RUST_OUT"   | grep -v '^VIGIL_REPO_ROOT=')

    if [[ "$bash_out" == "$rust_out" ]]; then
        return 0
    else
        printf '    DIFF for %s:\n' "$label"
        diff <(printf '%s\n' "$bash_out") <(printf '%s\n' "$rust_out") | sed 's/^/      /'
        return 1
    fi
}

# ── Matrix tests ──────────────────────────────────────────────────────────────

_ORACLE_OUT=""
_RUST_OUT=""

# M1: LOG_DIR only in conf re-derives LOG_FILE (MUST-PASS)
test_m1_log_dir_in_conf_rederives_log_file() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m1-XXXXXX)
    local custom_log="$tmpdir/customlogs"
    mkdir -p "$custom_log"

    # Bash conf: shell syntax
    local bash_conf="VIGIL_LOG_DIR=\"$custom_log\""
    # TOML conf: serde field name (log_dir, not VIGIL_LOG_DIR)
    local toml_conf="log_dir = \"$custom_log\""

    _run_bash_oracle "$bash_conf"
    local oracle_log_file; oracle_log_file=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_LOG_FILE=' | cut -d= -f2-)
    _run_rust_config  "$toml_conf"
    local rust_log_file;   rust_log_file=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_LOG_FILE=' | cut -d= -f2-)

    rm -rf "$tmpdir"

    local expected="$custom_log/daemon.log"
    assert_eq "$oracle_log_file" "$expected" "M1 bash: VIGIL_LOG_FILE re-derived" || return 1
    assert_eq "$rust_log_file"   "$expected" "M1 rust: VIGIL_LOG_FILE re-derived" || return 1
    assert_eq "$oracle_log_file" "$rust_log_file" "M1: bash == rust" || return 1
}

# M2: explicit VIGIL_CLAUDE_HOME not clobbered by CLAUDE_CONFIG_DIR (MUST-PASS)
test_m2_explicit_vigil_claude_home_not_clobbered() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m2-XXXXXX)
    local explicit="$tmpdir/explicit"
    local provider="$tmpdir/provider"

    _run_bash_oracle "" \
        "VIGIL_CLAUDE_HOME" "$explicit" \
        "CLAUDE_CONFIG_DIR" "$provider"
    local oracle_ch; oracle_ch=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CLAUDE_HOME=' | cut -d= -f2-)
    local oracle_auto; oracle_auto=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CLAUDE_HOME_AUTO=' | cut -d= -f2-)

    _run_rust_config "" \
        "VIGIL_CLAUDE_HOME" "$explicit" \
        "CLAUDE_CONFIG_DIR" "$provider"
    local rust_ch; rust_ch=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CLAUDE_HOME=' | cut -d= -f2-)
    local rust_auto; rust_auto=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CLAUDE_HOME_AUTO=' | cut -d= -f2-)

    rm -rf "$tmpdir"

    assert_eq "$oracle_ch"   "$explicit" "M2 bash: VIGIL_CLAUDE_HOME == explicit" || return 1
    assert_eq "$oracle_auto" "0"         "M2 bash: AUTO == 0" || return 1
    assert_eq "$rust_ch"     "$explicit" "M2 rust: VIGIL_CLAUDE_HOME == explicit" || return 1
    assert_eq "$rust_auto"   "0"         "M2 rust: AUTO == 0" || return 1
}

# M3: pure defaults — full block must match using a SHARED home dir so paths agree.
test_m3_pure_defaults() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m3-XXXXXX)
    local home_dir="$tmpdir/home"
    mkdir -p "$home_dir"
    local bash_conf_file="$tmpdir/empty.sh.conf"
    local toml_conf_file="$tmpdir/empty.toml.conf"
    touch "$bash_conf_file" "$toml_conf_file"

    # Bash oracle: run with shared home and empty conf.
    _ORACLE_OUT=$(
        env -i \
            HOME="$home_dir" \
            VIGIL_CONFIG_FILE="$bash_conf_file" \
            VIGIL_REPO_ROOT="$VIGIL_REPO_ROOT" \
            VIGIL_LOG_FILE="" \
            bash -c "unset VIGIL_LOG_FILE; source '$ORACLE'" 2>&1
    )

    # Rust config: run with same shared home and empty toml conf.
    _RUST_OUT=$(
        env -i \
            HOME="$home_dir" \
            VIGIL_CONFIG_FILE="$toml_conf_file" \
            VIGIL_REPO_ROOT="$VIGIL_REPO_ROOT" \
            "$VIGIL_RUST_BIN" config --kv 2>&1
    )

    # The only expected difference is VIGIL_CONFIG_FILE (bash conf vs toml conf path).
    # Both are in the same tmpdir; the conf file names differ (.sh.conf vs .toml.conf).
    # Filter VIGIL_CONFIG_FILE from both sides for the full-block comparison.
    local bash_filtered; bash_filtered=$(printf '%s\n' "$_ORACLE_OUT" | grep -v '^VIGIL_CONFIG_FILE=')
    local rust_filtered; rust_filtered=$(printf '%s\n' "$_RUST_OUT"   | grep -v '^VIGIL_CONFIG_FILE=')

    rm -rf "$tmpdir"

    if [[ "$bash_filtered" != "$rust_filtered" ]]; then
        printf '    DIFF for M3 pure defaults:\n'
        diff <(printf '%s\n' "$bash_filtered") <(printf '%s\n' "$rust_filtered") | sed 's/^/      /'
        return 1
    fi
}

# M4: CLAUDE_CONFIG_DIR env, no VIGIL_CLAUDE_HOME (Case B)
test_m4_claude_config_dir_env_auto() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m4-XXXXXX)
    local custom="$tmpdir/c"

    _run_bash_oracle "" "CLAUDE_CONFIG_DIR" "$custom"
    local oracle_ch; oracle_ch=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CLAUDE_HOME=' | cut -d= -f2-)
    local oracle_auto; oracle_auto=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CLAUDE_HOME_AUTO=' | cut -d= -f2-)

    _run_rust_config "" "CLAUDE_CONFIG_DIR" "$custom"
    local rust_ch; rust_ch=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CLAUDE_HOME=' | cut -d= -f2-)
    local rust_auto; rust_auto=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CLAUDE_HOME_AUTO=' | cut -d= -f2-)

    rm -rf "$tmpdir"

    assert_eq "$oracle_ch"   "$custom" "M4 bash: VIGIL_CLAUDE_HOME == CLAUDE_CONFIG_DIR" || return 1
    assert_eq "$oracle_auto" "1"       "M4 bash: AUTO == 1" || return 1
    assert_eq "$rust_ch"     "$custom" "M4 rust: VIGIL_CLAUDE_HOME == CLAUDE_CONFIG_DIR" || return 1
    assert_eq "$rust_auto"   "1"       "M4 rust: AUTO == 1" || return 1
}

# M5-D: conf sets provider-env (Case D)
test_m5d_conf_provider_env_rederives_claude_home() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m5d-XXXXXX)
    local fromconf="$tmpdir/fromconf"

    # Bash conf: CLAUDE_CONFIG_DIR=... (shell var)
    local bash_conf="CLAUDE_CONFIG_DIR=\"$fromconf\""
    # TOML conf: claude_config_dir passthrough key
    local toml_conf="claude_config_dir = \"$fromconf\""

    _run_bash_oracle "$bash_conf"
    local oracle_ch; oracle_ch=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CLAUDE_HOME=' | cut -d= -f2-)
    local oracle_auto; oracle_auto=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CLAUDE_HOME_AUTO=' | cut -d= -f2-)

    _run_rust_config "$toml_conf"
    local rust_ch; rust_ch=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CLAUDE_HOME=' | cut -d= -f2-)
    local rust_auto; rust_auto=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CLAUDE_HOME_AUTO=' | cut -d= -f2-)

    rm -rf "$tmpdir"

    assert_eq "$oracle_ch"   "$fromconf" "M5-D bash: conf provider-env rederives" || return 1
    assert_eq "$oracle_auto" "1"         "M5-D bash: AUTO == 1" || return 1
    assert_eq "$rust_ch"     "$fromconf" "M5-D rust: conf provider-env rederives" || return 1
    assert_eq "$rust_auto"   "1"         "M5-D rust: AUTO == 1" || return 1
}

# M5-E: env explicit VIGIL_CLAUDE_HOME + conf sets CLAUDE_CONFIG_DIR (Case E)
test_m5e_env_explicit_survives_conf_provider_env() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m5e-XXXXXX)
    local explicit="$tmpdir/exp"
    local fromconf="$tmpdir/fromconf"

    local bash_conf="CLAUDE_CONFIG_DIR=\"$fromconf\""
    local toml_conf="claude_config_dir = \"$fromconf\""

    _run_bash_oracle "$bash_conf" "VIGIL_CLAUDE_HOME" "$explicit"
    local oracle_ch; oracle_ch=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CLAUDE_HOME=' | cut -d= -f2-)
    local oracle_auto; oracle_auto=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CLAUDE_HOME_AUTO=' | cut -d= -f2-)

    _run_rust_config "$toml_conf" "VIGIL_CLAUDE_HOME" "$explicit"
    local rust_ch; rust_ch=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CLAUDE_HOME=' | cut -d= -f2-)
    local rust_auto; rust_auto=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CLAUDE_HOME_AUTO=' | cut -d= -f2-)

    rm -rf "$tmpdir"

    assert_eq "$oracle_ch"   "$explicit" "M5-E bash: env-explicit survives conf" || return 1
    assert_eq "$oracle_auto" "0"         "M5-E bash: AUTO == 0" || return 1
    assert_eq "$rust_ch"     "$explicit" "M5-E rust: env-explicit survives conf" || return 1
    assert_eq "$rust_auto"   "0"         "M5-E rust: AUTO == 0" || return 1
}

# M6: conf sets VIGIL_CODEX_HOME directly (Case F)
test_m6_conf_codex_home_direct() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m6-XXXXXX)
    local cx="$tmpdir/cx"

    # Bash conf: sets VIGIL_CODEX_HOME
    local bash_conf="VIGIL_CODEX_HOME=\"$cx\""
    # TOML conf: codex_home field
    local toml_conf="codex_home = \"$cx\""

    _run_bash_oracle "$bash_conf"
    local oracle_codex; oracle_codex=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CODEX_HOME=' | cut -d= -f2-)
    local oracle_auto;  oracle_auto=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_CODEX_HOME_AUTO=' | cut -d= -f2-)

    _run_rust_config "$toml_conf"
    local rust_codex; rust_codex=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CODEX_HOME=' | cut -d= -f2-)
    local rust_auto;  rust_auto=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_CODEX_HOME_AUTO=' | cut -d= -f2-)

    rm -rf "$tmpdir"

    assert_eq "$oracle_codex" "$cx" "M6 bash: VIGIL_CODEX_HOME from conf" || return 1
    assert_eq "$oracle_auto"  "1"   "M6 bash: AUTO == 1 for conf-set" || return 1
    assert_eq "$rust_codex"   "$cx" "M6 rust: VIGIL_CODEX_HOME from conf" || return 1
    assert_eq "$rust_auto"    "1"   "M6 rust: AUTO == 1 for conf-set" || return 1
}

# M7: numeric env overrides (split footgun guard)
test_m7_numeric_env_overrides() {
    _require_rust_bin || return 1

    _run_bash_oracle "" \
        "VIGIL_IDLE_AFTER_SEC"           "999" \
        "VIGIL_POWER_HELPER_TIMEOUT_SECS" "42" \
        "VIGIL_TICK_SECS"                "3"
    local oracle_idle; oracle_idle=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_IDLE_AFTER_SEC=' | cut -d= -f2-)
    local oracle_pht;  oracle_pht=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_POWER_HELPER_TIMEOUT_SECS=' | cut -d= -f2-)
    local oracle_tick; oracle_tick=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_TICK_SECS=' | cut -d= -f2-)

    _run_rust_config "" \
        "VIGIL_IDLE_AFTER_SEC"           "999" \
        "VIGIL_POWER_HELPER_TIMEOUT_SECS" "42" \
        "VIGIL_TICK_SECS"                "3"
    local rust_idle; rust_idle=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_IDLE_AFTER_SEC=' | cut -d= -f2-)
    local rust_pht;  rust_pht=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_POWER_HELPER_TIMEOUT_SECS=' | cut -d= -f2-)
    local rust_tick; rust_tick=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_TICK_SECS=' | cut -d= -f2-)

    assert_eq "$oracle_idle" "999" "M7 bash: VIGIL_IDLE_AFTER_SEC=999" || return 1
    assert_eq "$oracle_pht"  "42"  "M7 bash: VIGIL_POWER_HELPER_TIMEOUT_SECS=42" || return 1
    assert_eq "$oracle_tick" "3"   "M7 bash: VIGIL_TICK_SECS=3" || return 1
    assert_eq "$rust_idle"   "999" "M7 rust: VIGIL_IDLE_AFTER_SEC=999 (env split)" || return 1
    assert_eq "$rust_pht"    "42"  "M7 rust: VIGIL_POWER_HELPER_TIMEOUT_SECS=42" || return 1
    assert_eq "$rust_tick"   "3"   "M7 rust: VIGIL_TICK_SECS=3" || return 1
}

# M8: VIGIL_INSTALL_DIR env override cascades to state subpaths
test_m8_install_dir_env_cascades() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m8-XXXXXX)
    local inst="$tmpdir/inst"

    _run_bash_oracle "" "VIGIL_INSTALL_DIR" "$inst"
    local oracle_state;  oracle_state=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_STATE_DIR=' | cut -d= -f2-)
    local oracle_active; oracle_active=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_ACTIVE_DIR=' | cut -d= -f2-)
    local oracle_lhelper; oracle_lhelper=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_LOCK_HELPER=' | cut -d= -f2-)

    _run_rust_config "" "VIGIL_INSTALL_DIR" "$inst"
    local rust_state;  rust_state=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_STATE_DIR=' | cut -d= -f2-)
    local rust_active; rust_active=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_ACTIVE_DIR=' | cut -d= -f2-)
    local rust_lhelper; rust_lhelper=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_LOCK_HELPER=' | cut -d= -f2-)

    rm -rf "$tmpdir"

    assert_eq "$oracle_state"   "$inst/state"         "M8 bash: state_dir follows install_dir" || return 1
    assert_eq "$oracle_active"  "$inst/state/active"  "M8 bash: active_dir follows state_dir"  || return 1
    assert_eq "$oracle_lhelper" "$inst/bin/vigil-lock-helper" "M8 bash: lock_helper follows install_dir" || return 1
    assert_eq "$rust_state"     "$inst/state"         "M8 rust: state_dir follows install_dir" || return 1
    assert_eq "$rust_active"    "$inst/state/active"  "M8 rust: active_dir follows state_dir"  || return 1
    assert_eq "$rust_lhelper"   "$inst/bin/vigil-lock-helper" "M8 rust: lock_helper follows install_dir" || return 1
}

# M9: log_dir + idle via env together
test_m9_log_dir_and_idle_env() {
    _require_rust_bin || return 1

    local tmpdir; tmpdir=$(mktemp -d -t vigil-m9-XXXXXX)
    local custom_log="$tmpdir/l"

    _run_bash_oracle "" \
        "VIGIL_LOG_DIR"       "$custom_log" \
        "VIGIL_IDLE_AFTER_SEC" "60"
    local oracle_ldir; oracle_ldir=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_LOG_DIR=' | cut -d= -f2-)
    local oracle_lfile; oracle_lfile=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_LOG_FILE=' | cut -d= -f2-)
    local oracle_idle; oracle_idle=$(printf '%s\n' "$_ORACLE_OUT" | grep '^VIGIL_IDLE_AFTER_SEC=' | cut -d= -f2-)

    _run_rust_config "" \
        "VIGIL_LOG_DIR"       "$custom_log" \
        "VIGIL_IDLE_AFTER_SEC" "60"
    local rust_ldir; rust_ldir=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_LOG_DIR=' | cut -d= -f2-)
    local rust_lfile; rust_lfile=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_LOG_FILE=' | cut -d= -f2-)
    local rust_idle; rust_idle=$(printf '%s\n' "$_RUST_OUT" | grep '^VIGIL_IDLE_AFTER_SEC=' | cut -d= -f2-)

    rm -rf "$tmpdir"

    assert_eq "$oracle_ldir"  "$custom_log"              "M9 bash: LOG_DIR reflected" || return 1
    assert_eq "$oracle_lfile" "$custom_log/daemon.log"   "M9 bash: LOG_FILE re-derived" || return 1
    assert_eq "$oracle_idle"  "60"                       "M9 bash: IDLE_AFTER_SEC=60" || return 1
    assert_eq "$rust_ldir"    "$custom_log"              "M9 rust: LOG_DIR reflected" || return 1
    assert_eq "$rust_lfile"   "$custom_log/daemon.log"   "M9 rust: LOG_FILE re-derived" || return 1
    assert_eq "$rust_idle"    "60"                       "M9 rust: IDLE_AFTER_SEC=60" || return 1
}
