//! Cargo port of `tests/detect_test.sh` — consumes the SAME committed fixtures.
//!
//! Fixtures live at `<repo>/tests/fixtures/...`; the manifest dir is
//! `crates/vigil`, so the repo root is two levels up.

use vigil::procscan::detect::{AgentKind, agent_match_tsv, detect_all_text, detect_line};

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn read_fixture(name: &str) -> String {
    let p = repo_root().join("tests").join("fixtures").join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn detect_fixtures() -> Vec<vigil::procscan::detect::AgentMatch> {
    let comm = read_fixture("ps-axww-comm-snapshot.txt");
    let cmd = read_fixture("ps-axww-snapshot.txt");
    detect_all_text(&comm, &cmd)
}

fn tsv_blob() -> String {
    detect_fixtures()
        .iter()
        .map(agent_match_tsv)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn fixture_exists() {
    assert!(
        repo_root()
            .join("tests/fixtures/ps-axww-snapshot.txt")
            .exists()
    );
    assert!(
        repo_root()
            .join("tests/fixtures/ps-axww-comm-snapshot.txt")
            .exists()
    );
}

#[test]
fn picks_up_known_cli_processes() {
    let out = tsv_blob();
    assert!(
        out.contains("cli-claude"),
        "should detect at least one claude CLI"
    );
    assert!(out.contains("cli-copilot"), "should detect copilot CLI");
}

#[test]
fn excludes_claude_app_main() {
    let out = tsv_blob();
    assert!(
        !out.contains("/Applications/Claude.app/Contents/MacOS/Claude"),
        "should NOT detect Claude.app main"
    );
}

#[test]
fn detects_codex_app_as_app_codex() {
    let row = detect_fixtures()
        .into_iter()
        .find(|m| m.pid == 18355)
        .unwrap();
    assert_eq!(row.kind, AgentKind::AppCodex);
    assert_eq!(row.exe, "/Applications/Codex.app/Contents/MacOS/Codex");
}

#[test]
fn detects_vscode_main_as_copilot_chat_host() {
    let comm =
        "22222 /Applications/Visual Studio Code - Insiders.app/Contents/MacOS/Code - Insiders\n";
    let cmd = comm;
    let out = detect_all_text(comm, cmd);
    let row = out.into_iter().find(|m| m.pid == 22222).unwrap();
    assert_eq!(row.kind, AgentKind::AppVscodeCopilotChat);
}

#[test]
fn does_not_detect_vscode_helper_as_copilot_chat_host() {
    let comm = "22223 /Applications/Visual Studio Code - Insiders.app/Contents/Frameworks/Code - Insiders Helper.app/Contents/MacOS/Code - Insiders Helper\n";
    let cmd = "22223 /Applications/Visual Studio Code - Insiders.app/Contents/Frameworks/Code - Insiders Helper.app/Contents/MacOS/Code - Insiders Helper --type=utility\n";
    let out = detect_all_text(comm, cmd);
    assert!(
        !out.iter()
            .any(|m| m.kind == AgentKind::AppVscodeCopilotChat),
        "VS Code helpers must not be host anchors"
    );
}

#[test]
fn excludes_helpers_and_node_repl() {
    let out = tsv_blob();
    assert!(!out.contains("Helper"), "no Electron helpers");
    assert!(!out.contains("node_repl"), "no node_repl");
    assert!(!out.contains("crashpad"), "no crashpad");
    assert!(!out.contains("chrome-native-host"), "no chrome-native-host");
}

#[test]
fn excludes_codex_app_server() {
    let out = tsv_blob();
    assert!(
        !out.contains("/Applications/Codex.app/Contents/Resources/codex"),
        "should NOT detect Codex bundled app-server"
    );
}

#[test]
fn excludes_copilot_companion_node() {
    let out = tsv_blob();
    assert!(
        !out.contains("copilot-acp-daemon"),
        "should NOT detect copilot-companion node router daemon"
    );
}

#[test]
fn picks_up_copilot_companion_acp_worker() {
    let row = detect_fixtures()
        .into_iter()
        .find(|m| m.pid == 3670)
        .unwrap();
    assert_eq!(row.kind, AgentKind::CliCopilot);
    assert_eq!(row.exe, "/opt/homebrew/bin/copilot");
    assert!(
        row.args.contains("--acp"),
        "args must preserve --acp marker"
    );
}

#[test]
fn picks_up_claude_app_lam_bundled_cc() {
    let row = detect_fixtures()
        .into_iter()
        .find(|m| m.pid == 87838)
        .unwrap();
    assert_eq!(row.kind, AgentKind::CliClaude);
    assert_eq!(
        row.exe,
        "/Users/thanga-5521/Library/Application Support/Claude/claude-code/2.1.142/claude.app/Contents/MacOS/claude",
        "exe must preserve the spaced bundled-CC path"
    );
    assert!(
        row.args.contains("--output-format stream-json"),
        "args must preserve LAM-mode CC flags"
    );
}

#[test]
fn picks_up_vscode_chatgpt_extension_codex_worker() {
    let row = detect_fixtures()
        .into_iter()
        .find(|m| m.pid == 31322)
        .unwrap();
    assert_eq!(row.kind, AgentKind::CliCodex);
    assert_eq!(
        row.exe,
        "/Users/thanga-5521/.vscode-insiders/extensions/openai.chatgpt-26.5519.32039-darwin-arm64/bin/macos-aarch64/codex"
    );
    assert!(
        row.args.contains("app-server"),
        "args must preserve app-server subcommand"
    );
}

#[test]
fn output_format_is_tsv() {
    for m in detect_fixtures() {
        let row = agent_match_tsv(&m);
        let tabs = row.bytes().filter(|b| *b == b'\t').count();
        assert_eq!(tabs, 3, "TSV row should have exactly 3 tabs: {row}");
    }
}

#[test]
fn synthetic_codex_cli() {
    let m = detect_line(12345, "codex", "codex").unwrap();
    assert_eq!(agent_match_tsv(&m), "12345\tcli-codex\tcodex\t");
}
