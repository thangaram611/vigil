#!/usr/bin/env bash
# tests/detect_test.sh — verify vigil_detect_all against paired live-machine
# fixtures. The fixtures are kept as separate files (one for `ps -o command=`,
# one for `ps -o comm=`) to mirror what the daemon collects at runtime. The
# comm fixture is regenerated from the command fixture by
# tests/fixtures/gen_comm_from_command.py; see that script for the basename-
# derivation rules covering spaced exe paths.

VIGIL_LIB_DIR="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/lib"
# shellcheck source=../lib/detect.sh
source "$VIGIL_LIB_DIR/detect.sh"

FIXTURE="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/tests/fixtures/ps-axww-snapshot.txt"
FIXTURE_COMM="${VIGIL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/tests/fixtures/ps-axww-comm-snapshot.txt"

test_fixture_exists() {
    assert_file_exists "$FIXTURE" "ps -o command= fixture should be committed"
    assert_file_exists "$FIXTURE_COMM" "ps -o comm= fixture should be committed"
}

test_picks_up_known_cli_processes() {
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    assert_contains "$out" "cli-claude" "should detect at least one claude CLI"
    assert_contains "$out" "cli-copilot" "should detect copilot CLI"
}

test_excludes_claude_app_main() {
    # Claude.app's main Electron host (/Applications/Claude.app/Contents/MacOS/Claude)
    # has basename "Claude" (capital). It must NOT be detected as an agent
    # process — phase-3's audit (see future/phase-3-desktop-apps.md §5.2)
    # found that Claude.app's Local Agent Mode work happens in the bundled
    # CC subprocess (basename "claude", lowercase) under
    # ~/Library/Application Support/Claude/claude-code/<ver>/.../MacOS/claude,
    # which is covered separately.
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    assert_not_contains "$out" "/Applications/Claude.app/Contents/MacOS/Claude" "should NOT detect Claude.app main"
}

test_detects_codex_app_as_app_codex() {
    # Phase-3: the main Codex.app electron host
    # (/Applications/Codex.app/Contents/MacOS/Codex, basename "Codex") IS
    # detected as `app-codex`. It is then activity-gated on the existing
    # `codex_active` probe (~/.codex/sessions/) in refcount logic, so an
    # idle-but-open Codex.app does not hold sleep.
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    local row
    row=$(printf '%s\n' "$out" | grep -E $'^18355\t')
    assert_contains "$row" $'\tapp-codex\t' "fixture pid 18355 (Codex.app main) must map to app-codex"
    assert_contains "$row" "/Applications/Codex.app/Contents/MacOS/Codex" "exe column should be the Codex.app main path"
}

test_excludes_helpers_and_node_repl() {
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    assert_not_contains "$out" "Helper" "should NOT detect Electron helpers"
    assert_not_contains "$out" "node_repl" "should NOT detect Codex node_repl"
    assert_not_contains "$out" "crashpad" "should NOT detect crashpad workers"
    assert_not_contains "$out" "chrome-native-host" "should NOT detect Chrome MCP bridge"
}

test_excludes_codex_app_server() {
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    # Codex.app's bundled CLI sits at /Applications/Codex.app/Contents/Resources/codex
    # and runs as `codex app-server …`. The /Applications/* exclusion drops
    # it; phase 3 anchors detection on the Codex.app *main* process instead,
    # so the desktop app contributes exactly one app-codex PID file regardless
    # of how many app-server workers it spawns.
    assert_not_contains "$out" "/Applications/Codex.app/Contents/Resources/codex" "should NOT detect Codex bundled app-server"
}

test_excludes_copilot_companion_node() {
    # The copilot-companion node router daemon is the long-lived process that
    # spawns `copilot --acp` workers per Copilot session. Matching the router
    # itself would hold sleep 24/7; matching the worker (covered by
    # test_picks_up_copilot_companion_acp_worker) is the correct shape.
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    assert_not_contains "$out" "copilot-acp-daemon" "should NOT detect copilot-companion node router daemon"
}

test_picks_up_copilot_companion_acp_worker() {
    # Phase 2 audit: the worker the copilot-companion daemon spawns is a real
    # `copilot` CLI process invoked with `--acp` and ACP-mode flags. It must
    # be matched as cli-copilot and the --acp marker must survive in the args
    # column, since downstream callers (debug logs, future filters) may key
    # on it. The fixture line is captured live and pins this contract.
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    local row
    row=$(printf '%s\n' "$out" | grep -E $'^3670\t')
    assert_contains "$row" $'\tcli-copilot\t' "fixture pid 3670 (companion-spawned worker) must map to cli-copilot"
    assert_contains "$row" "/opt/homebrew/bin/copilot" "exe column must preserve the resolved copilot binary path"
    assert_contains "$row" "--acp" "args column must preserve the --acp marker that distinguishes companion workers"
}

test_picks_up_claude_app_lam_bundled_cc() {
    # Phase 3 — Claude.app's Local Agent Mode spawns the bundled Claude Code
    # binary at ~/Library/Application Support/Claude/claude-code/<ver>/claude.app/Contents/MacOS/claude.
    # basename is "claude", the path is NOT under /Applications/, AND the path
    # contains a literal space ("Application Support"). The pre-phase-3
    # detect.sh misparsed this — splitting on the first whitespace yielded
    # exe=/Users/.../Library/Application, basename="Application", no match.
    # This test pins the contract that the parsing fix correctly identifies
    # the bundled CC as cli-claude.
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    local row
    row=$(printf '%s\n' "$out" | grep -E $'^87838\t')
    assert_contains "$row" $'\tcli-claude\t' "fixture pid 87838 (Claude.app LAM bundled CC) must map to cli-claude"
    assert_contains "$row" "/Users/thanga-5521/Library/Application Support/Claude/claude-code/2.1.142/claude.app/Contents/MacOS/claude" \
        "exe column must preserve the full bundled-CC path including the space"
    assert_contains "$row" "--output-format stream-json" \
        "args column must preserve the LAM-mode CC flags"
}

test_picks_up_vscode_chatgpt_extension_codex_worker() {
    # Phase 3 — VS Code's OpenAI ChatGPT extension spawns its own bundled codex
    # binary at ~/.vscode-insiders/extensions/openai.chatgpt-<ver>/bin/<arch>/codex.
    # basename "codex", path not under /Applications/, no spaces — phase 1's
    # detect logic already matched it correctly. This test pins the contract
    # so a future refactor doesn't silently regress.
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
    local row
    row=$(printf '%s\n' "$out" | grep -E $'^31322\t')
    assert_contains "$row" $'\tcli-codex\t' "fixture pid 31322 (ChatGPT extension's codex) must map to cli-codex"
    assert_contains "$row" "/Users/thanga-5521/.vscode-insiders/extensions/openai.chatgpt-26.5519.32039-darwin-arm64/bin/macos-aarch64/codex" \
        "exe column must preserve the extension-bundled codex path"
    assert_contains "$row" "app-server" "args column must preserve the app-server subcommand"
}

test_output_format_is_tsv() {
    local out; out=$(vigil_detect_all "$FIXTURE_COMM" "$FIXTURE")
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
    # Synthesis: comm fixture line `12345 codex` + command fixture line `12345 codex`.
    local tmp_cmd tmp_comm
    tmp_cmd=$(mktemp); tmp_comm=$(mktemp)
    printf '12345 codex\n' > "$tmp_cmd"
    printf '12345 codex\n' > "$tmp_comm"
    local out; out=$(vigil_detect_all "$tmp_comm" "$tmp_cmd")
    rm -f "$tmp_cmd" "$tmp_comm"
    assert_contains "$out" "12345	cli-codex	codex" "synthetic codex CLI should match"
}
