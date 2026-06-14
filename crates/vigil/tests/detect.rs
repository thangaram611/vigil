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
fn tsv_blob_excludes_non_agents() {
    // Substrings that must NOT appear in the detected TSV: the bundled app
    // mains/servers and the Electron-helper / node-REPL / crashpad noise. Each
    // former negative test is one needle row. (The AppVscodeCopilotChat helper
    // negative is an AgentKind check, not a substring, and stays separate.)
    let out = tsv_blob();
    let needles: &[&str] = &[
        "/Applications/Claude.app/Contents/MacOS/Claude", // Claude.app main
        "/Applications/Codex.app/Contents/Resources/codex", // Codex bundled app-server
        "copilot-acp-daemon",                             // copilot-companion node router
        "Helper",                                         // Electron helpers
        "node_repl",
        "crashpad",
        "chrome-native-host",
    ];
    for needle in needles {
        assert!(!out.contains(needle), "TSV must NOT contain {needle:?}");
    }
}

#[test]
fn detects_known_pids_with_kind_exe_and_args() {
    // Each fixture pid resolves to an exact kind + exe (the spaced bundled paths
    // are load-bearing, so exe is assert_eq! not contains); when an arg substring
    // is given it must survive in args, and the AppCodex row (None) skips the arg
    // check. The synthetic chat-host / vscode-helper / detect_line tests assert
    // different shapes and stay separate.
    let fixtures = detect_fixtures();
    let cases: &[(u32, AgentKind, &str, Option<&str>)] = &[
        (
            18355,
            AgentKind::AppCodex,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            None,
        ),
        (
            3670,
            AgentKind::CliCopilot,
            "/opt/homebrew/bin/copilot",
            Some("--acp"),
        ),
        (
            87838,
            AgentKind::CliClaude,
            "/Users/thanga-5521/Library/Application Support/Claude/claude-code/2.1.142/claude.app/Contents/MacOS/claude",
            Some("--output-format stream-json"),
        ),
        (
            31322,
            AgentKind::CliCodex,
            "/Users/thanga-5521/.vscode-insiders/extensions/openai.chatgpt-26.5519.32039-darwin-arm64/bin/macos-aarch64/codex",
            Some("app-server"),
        ),
    ];
    for (pid, kind, exe, arg_substr) in cases {
        let row = fixtures
            .iter()
            .find(|m| m.pid == *pid)
            .unwrap_or_else(|| panic!("pid {pid} not detected"));
        assert_eq!(&row.kind, kind, "pid {pid} kind");
        assert_eq!(row.exe, *exe, "pid {pid} exe");
        if let Some(sub) = arg_substr {
            assert!(
                row.args.contains(*sub),
                "pid {pid} args must contain {sub:?}: {}",
                row.args
            );
        }
    }
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
