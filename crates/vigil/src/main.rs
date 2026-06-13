//! vigil — single-binary CLI skeleton (Phase 5.1) + config/logging substrate (Phase 5.2).
//!
//! clap-derive dispatch with exit-code discipline. `--version`, help, `completions`,
//! and `config` are handled natively; every real command delegates to the existing
//! bash `bin/vigil` via `shim::exec_bash` (execv) so exit codes and signals propagate
//! verbatim.

// config, log, output, and the detection modules are declared in lib.rs;
// reference them via the crate root.
use vigil::{config, debug, output};

mod exit;
mod shim;

use std::ffi::OsString;

use clap::error::ErrorKind;
use clap::{ColorChoice, CommandFactory, FromArgMatches, Parser, Subcommand};

/// Top-level CLI. `--color` is a global flag via colorchoice-clap.
#[derive(Parser, Debug)]
#[command(
    name = "vigil",
    version,
    about = "vigil — keep Mac awake while AI agents are working",
    styles = output::clap_styles(),
)]
pub(crate) struct Cli {
    /// Global color choice (auto|always|never). Flattened from colorchoice-clap.
    #[command(flatten)]
    color: colorchoice_clap::Color,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Install root helper + newsyslog entry, create dirs, load LaunchAgent
    Setup {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Remove helper + plist, restore baseline, wipe state
    Uninstall {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Bootstrap the LaunchAgent
    Start {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Boot out the LaunchAgent
    Stop {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Show service, activity, and power state
    Status {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// cat / tail -f the daemon log
    Log {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Wrapper: hold sleep prevention while <cmd> runs
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Re-sync daemon + libs into install dir, restart launchd
    Reload {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Freeze input until configured combo (and `lock doctor`)
    Lock {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Diagnose installation
    Doctor {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Generate shell completion script to stdout (native)
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
    /// Show the fully-resolved configuration (native; not delegated to bash).
    ///
    /// Reads vigil.conf as strict TOML, merges env overrides, and prints all
    /// VIGIL_* resolved values. No side effects — does not create directories.
    Config {
        /// Print machine-readable JSON object (stable sorted keys).
        #[arg(long, conflicts_with = "show", conflicts_with = "kv")]
        json: bool,
        /// Print human-readable table (default if neither flag given).
        #[arg(long)]
        show: bool,
        /// Print sorted KEY=VALUE lines (used by the parity oracle test).
        #[arg(long, hide = true)]
        kv: bool,
    },
    /// Read-only diagnostic dump of the detection data model (native; never
    /// mutates state). Hidden from the public surface (parity with the ten bash
    /// subcommands + completions/config).
    #[command(hide = true)]
    Debug {
        #[command(subcommand)]
        sub: Option<DebugSub>,
        /// Emit machine-readable JSON instead of the table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum DebugSub {
    /// Hidden fixture-mode detect oracle (pure; reads two ps text files).
    #[command(hide = true)]
    Detect {
        #[arg(long)]
        ps_comm: std::path::PathBuf,
        #[arg(long)]
        ps_cmd: std::path::PathBuf,
    },
}

fn main() {
    let cli = parse_or_exit();

    // --color was already applied to the process-global ColorChoice inside
    // `parse_or_exit` (so help/version/error rendering during parsing is also
    // governed). Re-applying here is a harmless no-op kept for clarity/safety.
    cli.color.write_global();

    dispatch(cli.command);
}

/// Resolve the global `--color` value from argv with a tiny hand-rolled scan.
///
/// clap emits help/usage/version/errors DURING parsing — before the typed
/// `Cli.color` field exists — so we must learn the color choice up front. A
/// clap `ignore_errors(true)` pre-pass is NOT reliable here: when `--help` or
/// `--version` appears, clap short-circuits and the global `--color` value is
/// absent from the partial matches (it reports the default `Auto`), which is
/// exactly the case the substrate must color correctly. So we scan argv
/// directly for `--color=<v>` and `--color <v>`. Unknown/garbage values fall
/// back to `Auto` (clap will reject them with a usage error during real parse).
/// Last occurrence wins, matching clap's override semantics.
fn resolve_color_choice() -> ColorChoice {
    let mut choice = ColorChoice::Auto;
    let mut args = std::env::args_os().skip(1).peekable();
    while let Some(arg) = args.next() {
        let s = arg.to_string_lossy();
        let value = if let Some(v) = s.strip_prefix("--color=") {
            Some(v.to_string())
        } else if s == "--color" {
            args.peek().map(|n| n.to_string_lossy().into_owned())
        } else {
            None
        };
        if let Some(v) = value {
            choice = match v.as_str() {
                "always" => ColorChoice::Always,
                "never" => ColorChoice::Never,
                _ => ColorChoice::Auto,
            };
        }
    }
    choice
}

/// Parse argv. On a clap error, map DisplayHelp/DisplayVersion to a clean exit 0
/// (printed to stdout); everything else (unknown subcommand/arg, missing value)
/// to stderr + exit 64.
fn parse_or_exit() -> Cli {
    // Resolve and apply `--color` BEFORE clap renders any help/version/error
    // text, because that output is produced during parsing. Two channels need
    // it: (1) the process-global ColorChoice (anstream/owo-colors prints, and
    // the streams clap writes through); (2) clap's OWN help/version/error
    // colorization, which is governed by the Command's `.color()` setting and
    // NOT by the colorchoice global — so we must set both or `--color` would be
    // a no-op for exactly the styled output this substrate governs.
    let choice = resolve_color_choice();
    colorchoice_clap::Color { color: choice }.write_global();

    // Build the command with the resolved color so help/version/error rendering
    // (DisplayHelp/DisplayVersion as well as usage errors from try_get_matches)
    // honors `--color=always|never` rather than clap's default Auto.
    let matches = match Cli::command()
        .color(choice)
        .try_get_matches_from(std::env::args_os())
    {
        Ok(m) => m,
        Err(e) => exit_on_clap_error(e),
    };
    match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        // from_arg_matches only fails on an internal mismatch; surface it as a
        // usage error through the same colored, exit-coded path.
        Err(e) => exit_on_clap_error(e.with_cmd(&Cli::command().color(choice))),
    }
}

/// Print a clap error with its configured color and exit: help/version → 0
/// (stdout); every other kind (unknown subcommand/arg, missing value) → 64
/// (stderr).
fn exit_on_clap_error(e: clap::Error) -> ! {
    match e.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            // clap formats help/version to stdout for these kinds; exit 0.
            let _ = e.print();
            std::process::exit(0);
        }
        _ => {
            // Unknown command/subcommand, bad/missing arg, etc.
            let _ = e.print(); // goes to stderr for error kinds
            std::process::exit(exit::EX_USAGE); // 64
        }
    }
}

/// Dispatch a parsed command. Returns `!` — every arm either execs (never
/// returns) or exits.
fn dispatch(command: Command) -> ! {
    match command {
        Command::Completions { shell } => {
            generate_completions(shell);
            std::process::exit(0);
        }
        Command::Config { json, show: _, kv } => {
            cmd_config(json, kv);
            std::process::exit(0);
        }
        Command::Debug { sub, json } => {
            cmd_debug(sub, json);
            std::process::exit(0);
        }
        Command::Setup { args } => shim::exec_bash("setup", &args),
        Command::Uninstall { args } => shim::exec_bash("uninstall", &args),
        Command::Start { args } => shim::exec_bash("start", &args),
        Command::Stop { args } => shim::exec_bash("stop", &args),
        Command::Status { args } => shim::exec_bash("status", &args),
        Command::Log { args } => shim::exec_bash("log", &args),
        Command::Run { args } => shim::exec_bash("run", &args),
        Command::Reload { args } => shim::exec_bash("reload", &args),
        Command::Lock { args } => shim::exec_bash("lock", &args),
        Command::Doctor { args } => shim::exec_bash("doctor", &args),
    }
}

/// Handle `vigil config [--show|--json|--kv]`.
///
/// Loads the fully-resolved config (no side effects) and prints it.
/// On a malformed conf, emits a clear error to stderr and exits EX_USAGE (64).
fn cmd_config(json: bool, kv: bool) {
    let conf_path = std::env::var("VIGIL_CONFIG_FILE")
        .unwrap_or_else(|_| format!("{}/.config/vigil/vigil.conf", home_dir()));

    let cfg = match config::load(&conf_path, None) {
        Ok(c) => c,
        Err(e) => {
            anstream::eprintln!("{e}");
            std::process::exit(exit::EX_USAGE);
        }
    };

    let map = cfg.to_kv_map();

    if json {
        // Machine-readable: pretty JSON object (stable sorted keys via BTreeMap).
        match output::print_json(&map) {
            Ok(()) => {}
            Err(e) => {
                anstream::eprintln!("vigil: config --json: {e}");
                std::process::exit(exit::EX_ERROR);
            }
        }
    } else if kv {
        // Hidden --kv mode: sorted KEY=VALUE lines (used by the parity oracle test).
        for (k, v) in &map {
            anstream::println!("{k}={v}");
        }
    } else {
        // Default: human-readable table (--show or no flag).
        let mut t = output::table(&["KEY", "VALUE"]);
        for (k, v) in &map {
            t.add_row([k.as_str(), v.as_str()]);
        }
        anstream::println!("{t}");
    }
}

/// Generate a completion script for `shell` to stdout. (Lives here because it
/// needs the binary-only `Cli` command factory.)
fn generate_completions(shell: clap_complete::Shell) {
    use std::io::Write;
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    // clap_complete writes raw bytes; stdout is fine (scripts have no ANSI).
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    let _ = std::io::stdout().flush();
}

/// Handle `vigil debug [detect ...] [--json]`.
///
/// - `debug detect --ps-comm <f> --ps-cmd <f>`: the hidden fixture-mode parity
///   oracle. Pure: reads two ps text files, prints byte-exact TSV rows via plain
///   `println!` (NOT anstream-styled — machine output that must match bash).
/// - `debug [--json]`: the READ-ONLY data-model dump (never mutates state).
fn cmd_debug(sub: Option<DebugSub>, json: bool) {
    match sub {
        Some(DebugSub::Detect { ps_comm, ps_cmd }) => {
            let comm_text = std::fs::read_to_string(&ps_comm).unwrap_or_else(|e| {
                anstream::eprintln!("vigil: debug detect: {}: {e}", ps_comm.display());
                std::process::exit(exit::EX_ERROR);
            });
            let cmd_text = std::fs::read_to_string(&ps_cmd).unwrap_or_else(|e| {
                anstream::eprintln!("vigil: debug detect: {}: {e}", ps_cmd.display());
                std::process::exit(exit::EX_ERROR);
            });
            for row in debug::detect_oracle_rows(&comm_text, &cmd_text) {
                // Plain println: byte-exact machine output, no ANSI.
                println!("{row}");
            }
        }
        None => {
            // READ-ONLY dump. Load config with NO side effects (no ensure_state_dir).
            let conf_path = std::env::var("VIGIL_CONFIG_FILE")
                .unwrap_or_else(|_| format!("{}/.config/vigil/vigil.conf", home_dir()));
            let cfg = match config::load(&conf_path, None) {
                Ok(c) => c,
                Err(e) => {
                    anstream::eprintln!("{e}");
                    std::process::exit(exit::EX_USAGE);
                }
            };
            let now = chrono::Local::now().timestamp();
            let dump = debug::assemble(&cfg, now);
            debug::render(&dump, json);
        }
    }
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}
