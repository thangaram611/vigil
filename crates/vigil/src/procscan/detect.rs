//! PURE detection core — the byte-exact Rust port of `lib/detect.sh`.
//!
//! No sysinfo, no env, no IO. This module is fed text/records and produces
//! [`AgentMatch`] rows that are byte-identical to bash `vigil_detect_line`. The
//! live sysinfo collector lives in the parent module (`procscan::mod`).
//!
//! Classification order is LOAD-BEARING (mirrors `vigil_detect_line`):
//!   1. `comm` ends with `/Codex.app/Contents/MacOS/Codex`  -> `app-codex`
//!      (carved out BEFORE the `/Applications/*` exclusion).
//!   2. `comm` contains `/Visual Studio Code.app/Contents/MacOS/` or
//!      `/Visual Studio Code - Insiders.app/Contents/MacOS/` -> vscode host.
//!   3. exclusion on the FULL `command_line` (substring).
//!   4. `basename(comm)` in {claude,codex,copilot} -> `cli-<basename>`.
//!   5. else no row.

use std::collections::HashMap;

/// The kinds of agent rows vigil emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentKind {
    CliClaude,
    CliCodex,
    CliCopilot,
    AppCodex,
    AppVscodeCopilotChat,
}

impl AgentKind {
    /// Stable name string, byte-identical to the bash `<name>` column.
    pub fn name(&self) -> &'static str {
        match self {
            AgentKind::CliClaude => "cli-claude",
            AgentKind::CliCodex => "cli-codex",
            AgentKind::CliCopilot => "cli-copilot",
            AgentKind::AppCodex => "app-codex",
            AgentKind::AppVscodeCopilotChat => "app-vscode-copilot-chat",
        }
    }
}

/// One matched agent process row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMatch {
    pub pid: u32,
    pub kind: AgentKind,
    /// The exe path (= bash `comm`); space-safe (full path or bare basename).
    pub exe: String,
    /// The recovered argv tail (possibly empty).
    pub args: String,
}

/// Substring exclusions on the FULL command line (mirrors `_vigil_is_excluded_cmd`).
///
/// bash `case`:
///   /Applications/*        -> prefix match
///   */Helper*              -> `/Helper` substring (NOTE the required leading `/`)
///   *crashpad*             -> bare substring
///   *chrome-native-host*   -> bare substring
///   *node_repl*            -> bare substring
fn is_excluded_cmd(command_line: &str) -> bool {
    command_line.starts_with("/Applications/")
        || command_line.contains("/Helper")
        || command_line.contains("crashpad")
        || command_line.contains("chrome-native-host")
        || command_line.contains("node_repl")
}

/// Recover argv from `(comm, command_line)` — exact `_vigil_args_from_command`.
///
/// `command_line == comm`           -> empty args.
/// `command_line == "comm "<rest>`  -> `<rest>` (strip exactly one space).
/// otherwise                        -> empty args (exe stays comm).
fn args_from_command(comm: &str, command_line: &str) -> String {
    if command_line == comm {
        return String::new();
    }
    if let Some(rest) = command_line.strip_prefix(comm)
        && let Some(rest) = rest.strip_prefix(' ')
    {
        return rest.to_string();
    }
    String::new()
}

/// basename = everything after the last `/`, or the whole string if no `/`.
fn basename(comm: &str) -> &str {
    match comm.rfind('/') {
        Some(i) => &comm[i + 1..],
        None => comm,
    }
}

/// Classify one `(pid, comm, command_line)` triple. Returns `None` when bash
/// emits no row.
pub fn detect_line(pid: u32, comm: &str, command_line: &str) -> Option<AgentMatch> {
    // 1. Codex.app main process — carved out BEFORE /Applications/ exclusion.
    if comm.ends_with("/Codex.app/Contents/MacOS/Codex") {
        return Some(AgentMatch {
            pid,
            kind: AgentKind::AppCodex,
            exe: comm.to_string(),
            args: args_from_command(comm, command_line),
        });
    }

    // 2. VS Code / VS Code Insiders main process (host anchor). Helpers do NOT
    //    match because their `.app` directory is the Helper bundle, not the
    //    top-level "Visual Studio Code{ - Insiders}.app".
    if comm.contains("/Visual Studio Code.app/Contents/MacOS/")
        || comm.contains("/Visual Studio Code - Insiders.app/Contents/MacOS/")
    {
        return Some(AgentMatch {
            pid,
            kind: AgentKind::AppVscodeCopilotChat,
            exe: comm.to_string(),
            args: args_from_command(comm, command_line),
        });
    }

    // 3. Hard exclusions on the FULL command line.
    if is_excluded_cmd(command_line) {
        return None;
    }

    // 4. CLI agents by basename.
    let kind = match basename(comm) {
        "claude" => AgentKind::CliClaude,
        "codex" => AgentKind::CliCodex,
        "copilot" => AgentKind::CliCopilot,
        _ => return None,
    };
    Some(AgentMatch {
        pid,
        kind,
        exe: comm.to_string(),
        args: args_from_command(comm, command_line),
    })
}

/// Parse `ps -o pid= -o <comm|command>=` text into a pid-keyed map.
///
/// Per line: trim leading+trailing ASCII whitespace, take the LEADING run of
/// ASCII digits as the pid, strip that pid and any following whitespace; the
/// remainder is the comm/cmd (may contain spaces — NEVER split beyond the pid
/// head). Lines with no leading-digit pid are skipped. Later duplicate pids
/// overwrite earlier (awk array-assignment semantics).
pub fn parse_ps_pid_keyed(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim_matches([' ', '\t']);
        // Leading run of ASCII digits.
        let digit_end = line
            .char_indices()
            .find(|(_, c)| !c.is_ascii_digit())
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        if digit_end == 0 {
            continue; // no leading-digit pid
        }
        let Ok(pid) = line[..digit_end].parse::<u32>() else {
            continue;
        };
        // Strip the pid head and any following whitespace.
        let rest = line[digit_end..].trim_start_matches([' ', '\t']);
        map.insert(pid, rest.to_string());
    }
    map
}

/// Join comm-text and cmd-text by pid; only pids present in BOTH are emitted.
///
/// Returns `Vec<(pid, comm, command_line)>`. Iterates the cmd-text in file order
/// and looks up comm (mirrors awk's second-pass emit order). Ordering is not
/// load-bearing because the parity oracle sorts both sides.
pub fn ps_join(comm_text: &str, cmd_text: &str) -> Vec<(u32, String, String)> {
    let comm = parse_ps_pid_keyed(comm_text);
    let cmd = parse_ps_pid_keyed(cmd_text);
    let mut out = Vec::new();
    // Re-walk cmd_text to preserve its file order (the HashMap loses it).
    let mut seen = std::collections::HashSet::new();
    for raw in cmd_text.lines() {
        let line = raw.trim_matches([' ', '\t']);
        let digit_end = line
            .char_indices()
            .find(|(_, c)| !c.is_ascii_digit())
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        if digit_end == 0 {
            continue;
        }
        let Ok(pid) = line[..digit_end].parse::<u32>() else {
            continue;
        };
        // awk overwrites; only emit each pid once, using the cmd map's final value.
        if !seen.insert(pid) {
            continue;
        }
        if let (Some(c), Some(cl)) = (comm.get(&pid), cmd.get(&pid)) {
            out.push((pid, c.clone(), cl.clone()));
        }
    }
    out
}

/// Pure detect over two ps text blobs. Mirrors `vigil_detect_all`'s two-file
/// mode. Returns matched rows in join order.
pub fn detect_all_text(comm_text: &str, cmd_text: &str) -> Vec<AgentMatch> {
    ps_join(comm_text, cmd_text)
        .into_iter()
        .filter_map(|(pid, comm, cmd)| detect_line(pid, &comm, &cmd))
        .collect()
}

/// Render an [`AgentMatch`] as the bash TSV row: `<pid>\t<name>\t<exe>\t<args>`.
/// Four fields, three tabs; args may be empty. No trailing newline.
pub fn agent_match_tsv(m: &AgentMatch) -> String {
    format!("{}\t{}\t{}\t{}", m.pid, m.kind.name(), m.exe, m.args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_handles_path_and_bare() {
        assert_eq!(basename("/opt/homebrew/bin/copilot"), "copilot");
        assert_eq!(basename("claude"), "claude");
        assert_eq!(basename(""), "");
    }

    #[test]
    fn codex_app_matched_before_applications_exclusion() {
        let m = detect_line(
            18355,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            "/Applications/Codex.app/Contents/MacOS/Codex",
        )
        .unwrap();
        assert_eq!(m.kind, AgentKind::AppCodex);
        assert_eq!(m.args, "");
    }

    #[test]
    fn vscode_helper_not_matched() {
        let comm = "/Applications/Visual Studio Code - Insiders.app/Contents/Frameworks/Code - Insiders Helper.app/Contents/MacOS/Code - Insiders Helper";
        let cmd = format!("{comm} --type=utility");
        assert!(detect_line(22223, comm, &cmd).is_none());
    }

    #[test]
    fn args_recovery_strips_one_space() {
        assert_eq!(args_from_command("codex", "codex"), "");
        assert_eq!(args_from_command("codex", "codex app-server"), "app-server");
        assert_eq!(args_from_command("codex", "codexx"), "");
    }

    #[test]
    fn parse_ps_skips_non_numeric_and_trims() {
        let m = parse_ps_pid_keyed(
            "    1 /sbin/launchd\nfoo bar\n 3670 /opt/homebrew/bin/copilot --acp\n",
        );
        assert_eq!(m.get(&1).unwrap(), "/sbin/launchd");
        assert_eq!(m.get(&3670).unwrap(), "/opt/homebrew/bin/copilot --acp");
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn cli_basename_maps_each_kind_with_path_prefix() {
        // Step 4: basename(comm) in {claude,codex,copilot} -> cli-<basename>.
        // A full path prefix is stripped to the basename; exe stays the full
        // comm and args are recovered (one-space strip).
        let cases: &[(&str, &str, AgentKind, &str)] = &[
            (
                "/opt/homebrew/bin/claude",
                "/opt/homebrew/bin/claude --resume",
                AgentKind::CliClaude,
                "--resume",
            ),
            (
                "/usr/local/bin/codex",
                "/usr/local/bin/codex app-server",
                AgentKind::CliCodex,
                "app-server",
            ),
            (
                "/opt/homebrew/bin/copilot",
                "/opt/homebrew/bin/copilot",
                AgentKind::CliCopilot,
                "",
            ),
            // bare basename (no '/') also matches.
            ("claude", "claude", AgentKind::CliClaude, ""),
        ];
        for (comm, cmd, kind, args) in cases {
            let m = detect_line(4242, comm, cmd)
                .unwrap_or_else(|| panic!("basename {comm:?} must match"));
            assert_eq!(m.kind, *kind, "kind for {comm:?}");
            assert_eq!(m.exe, *comm, "exe stays the full comm for {comm:?}");
            assert_eq!(m.args, *args, "args for {comm:?}");
            assert_eq!(m.pid, 4242);
        }
        // a basename that is none of the three -> no row.
        assert!(detect_line(7, "/bin/bash", "/bin/bash -lc x").is_none());
    }

    #[test]
    fn agent_match_tsv_is_exact_three_tab_format() {
        // <pid>\t<name>\t<exe>\t<args> — four fields, three tabs, no trailing
        // newline. Byte-exact rows for a path-prefixed match (with args) and a
        // bare match (empty args -> trailing tab).
        let with_args = detect_line(
            18355,
            "/opt/homebrew/bin/copilot",
            "/opt/homebrew/bin/copilot --acp",
        )
        .unwrap();
        assert_eq!(
            agent_match_tsv(&with_args),
            "18355\tcli-copilot\t/opt/homebrew/bin/copilot\t--acp"
        );
        // empty args -> the args field is empty (a bare trailing tab, no newline).
        let no_args = detect_line(3670, "claude", "claude").unwrap();
        assert_eq!(agent_match_tsv(&no_args), "3670\tcli-claude\tclaude\t");
        // app-codex name column.
        let app = detect_line(
            22,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            "/Applications/Codex.app/Contents/MacOS/Codex",
        )
        .unwrap();
        assert_eq!(
            agent_match_tsv(&app),
            "22\tapp-codex\t/Applications/Codex.app/Contents/MacOS/Codex\t"
        );
    }

    #[test]
    fn is_excluded_cmd_matches_new_bare_substrings() {
        // crashpad / chrome-native-host / node_repl are bare substrings anywhere
        // in the FULL command line.
        assert!(is_excluded_cmd(
            "/Users/x/Library/.../chrome_crashpad_handler"
        ));
        assert!(is_excluded_cmd("node /path/to/chrome-native-host.js"));
        assert!(is_excluded_cmd("node --experimental node_repl helper"));
        // none present -> not excluded.
        assert!(!is_excluded_cmd("/opt/homebrew/bin/copilot --acp"));
    }

    #[test]
    fn excluded_substring_suppresses_a_basename_match() {
        // The exclusion (step 3) runs BEFORE the basename match (step 4), so a
        // comm whose basename IS one of the three is still dropped when the FULL
        // command line carries an excluded substring. One row per substring.
        let cases: &[(&str, &str)] = &[
            ("/x/claude", "/x/claude crashpad-handler"),
            ("/x/codex", "/x/codex --chrome-native-host"),
            ("/x/copilot", "/x/copilot node_repl"),
        ];
        for (comm, cmd) in cases {
            assert!(
                detect_line(9, comm, cmd).is_none(),
                "excluded substring in {cmd:?} must suppress the basename match"
            );
        }
    }

    #[test]
    fn ps_join_emits_pids_in_both_maps_file_order_dup_overwrite() {
        // comm map: {100,200}; cmd map walked in FILE order. pid 100 appears
        // twice in cmd_text (awk-overwrite: the LAST value wins, but it is
        // emitted at its FIRST file position); pid 300 is only in cmd_text
        // (not in comm) so it is NOT emitted; pid 200 precedes pid 100 because
        // that is the cmd_text order.
        let comm_text = "100 /bin/claude\n200 /bin/codex\n";
        let cmd_text = "200 codex serve\n100 claude --first\n100 claude --last\n300 ghost\n";
        let joined = ps_join(comm_text, cmd_text);
        assert_eq!(
            joined,
            vec![
                (200u32, "/bin/codex".to_string(), "codex serve".to_string()),
                // file-order: 100 emitted once, at its first position, with the
                // OVERWRITTEN (last) cmd value.
                (
                    100u32,
                    "/bin/claude".to_string(),
                    "claude --last".to_string()
                ),
            ],
            "pid-in-both-maps, file-order, duplicate-pid overwrite"
        );
    }

    #[test]
    fn detect_all_text_joins_then_classifies() {
        // End-to-end over two ps blobs: only pids in both maps reach detect_line,
        // in join (cmd-file) order; pid 300 (cmd-only) is dropped.
        let comm_text = "100 /opt/homebrew/bin/copilot\n200 /bin/bash\n";
        let cmd_text = "200 /bin/bash -lc x\n100 /opt/homebrew/bin/copilot --acp\n300 ghost\n";
        let rows = detect_all_text(comm_text, cmd_text);
        // pid 200 -> /bin/bash basename not in the set -> no row; pid 100 matches.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 100);
        assert_eq!(rows[0].kind, AgentKind::CliCopilot);
        assert_eq!(rows[0].args, "--acp");
    }
}
