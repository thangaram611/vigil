#!/usr/bin/env bash
# tests/detect_test.sh — verify vigil_detect_all against the live-machine fixture.

VIGIL_LIB_DIR="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/lib"
# shellcheck source=../lib/detect.sh
source "$VIGIL_LIB_DIR/detect.sh"

FIXTURE="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/tests/fixtures/ps-axww-snapshot.txt"

test_fixture_exists() {
    assert_file_exists "$FIXTURE" "ps fixture should be committed"
}

test_picks_up_known_cli_processes() {
    local out; out=$(vigil_detect_all "$FIXTURE")
    assert_contains "$out" "cli-claude" "should detect at least one claude CLI"
    assert_contains "$out" "cli-copilot" "should detect copilot CLI"
}

test_excludes_desktop_apps() {
    local out; out=$(vigil_detect_all "$FIXTURE")
    assert_not_contains "$out" "/Applications/Claude.app/Contents/MacOS/Claude" "should NOT detect Claude.app main"
    assert_not_contains "$out" "/Applications/Codex.app/Contents/MacOS/Codex" "should NOT detect Codex.app main"
}

test_excludes_helpers_and_node_repl() {
    local out; out=$(vigil_detect_all "$FIXTURE")
    assert_not_contains "$out" "Helper" "should NOT detect Electron helpers"
    assert_not_contains "$out" "node_repl" "should NOT detect Codex node_repl"
    assert_not_contains "$out" "crashpad" "should NOT detect crashpad workers"
    assert_not_contains "$out" "chrome-native-host" "should NOT detect Chrome MCP bridge"
}

test_excludes_codex_app_server() {
    local out; out=$(vigil_detect_all "$FIXTURE")
    # Codex.app's bundled CLI sits at /Applications/Codex.app/Contents/Resources/codex
    # and runs as `codex app-server …`. The /Applications/* exclusion should drop it.
    assert_not_contains "$out" "/Applications/Codex.app/Contents/Resources/codex" "should NOT detect Codex bundled app-server"
}

test_excludes_copilot_companion_node() {
    # The copilot-companion node router daemon is the long-lived process that
    # spawns `copilot --acp` workers per Copilot session. Matching the router
    # itself would hold sleep 24/7; matching the worker (covered by
    # test_picks_up_copilot_companion_acp_worker) is the correct shape.
    local out; out=$(vigil_detect_all "$FIXTURE")
    assert_not_contains "$out" "copilot-acp-daemon" "should NOT detect copilot-companion node router daemon"
}

test_picks_up_copilot_companion_acp_worker() {
    # Phase 2 audit: the worker the copilot-companion daemon spawns is a real
    # `copilot` CLI process invoked with `--acp` and ACP-mode flags. It must
    # be matched as cli-copilot and the --acp marker must survive in the args
    # column, since downstream callers (debug logs, future filters) may key
    # on it. The fixture line is captured live and pins this contract.
    local out; out=$(vigil_detect_all "$FIXTURE")
    local row
    row=$(printf '%s\n' "$out" | grep -E $'^3670\t')
    assert_contains "$row" $'\tcli-copilot\t' "fixture pid 3670 (companion-spawned worker) must map to cli-copilot"
    assert_contains "$row" "/opt/homebrew/bin/copilot" "exe column must preserve the resolved copilot binary path"
    assert_contains "$row" "--acp" "args column must preserve the --acp marker that distinguishes companion workers"
}

test_output_format_is_tsv() {
    local out; out=$(vigil_detect_all "$FIXTURE")
    # Every output line should have exactly 3 tab characters: pid<TAB>name<TAB>exe<TAB>args
    while IFS= read -r line; do
        [[ -z "$line" ]] && continue
        local tabs="${line//[^	]/}"
        assert_eq "${#tabs}" "3" "TSV row should have exactly 3 tabs: $line"
    done <<< "$out"
}

test_synthetic_codex_cli() {
    # Inject a fake codex CLI line to be sure detection picks it up too. The fixture
    # I captured happened not to have the standalone codex CLI at the moment.
    local synthetic="12345 codex"
    local tmp; tmp=$(mktemp)
    printf '%s\n' "$synthetic" > "$tmp"
    local out; out=$(vigil_detect_all "$tmp")
    rm -f "$tmp"
    assert_contains "$out" "12345	cli-codex	codex" "synthetic codex CLI should match"
}
