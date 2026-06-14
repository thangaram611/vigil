//! End-to-end CLI tests driving the built `vigil` binary via CARGO_BIN_EXE_vigil.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vigil"))
}

#[test]
fn version_is_exact() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "vigil 0.1.0-dev\n");
}

#[test]
fn help_exits_zero() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn unknown_command_exits_64() {
    let out = bin().arg("definitely-not-a-command").output().unwrap();
    assert_eq!(out.status.code(), Some(64));
    // usage/error goes to stderr
    assert!(!out.stderr.is_empty());
}

#[test]
fn color_never_help_has_no_ansi() {
    let out = bin().args(["--color", "never", "--help"]).output().unwrap();
    assert!(out.status.success());
    assert!(
        !out.stdout.contains(&0x1b),
        "help with --color=never must contain zero ESC (0x1b) bytes"
    );
}

/// Run the built binary under a pseudo-tty via `script -q /dev/null <bin> ...`
/// and return the captured (combined) bytes. A real tty is required to exercise
/// the `--color` contract: piped output is stripped by clap's default Auto
/// detection regardless of the flag, which would mask whether the flag works.
#[cfg(unix)]
fn run_under_tty(args: &[&str]) -> Vec<u8> {
    let exe = env!("CARGO_BIN_EXE_vigil");
    // macOS/BSD `script`: `script -q <file> command [args...]`.
    let out = Command::new("script")
        .arg("-q")
        .arg("/dev/null")
        .arg(exe)
        .args(args)
        .output()
        .expect("spawn `script` (PTY harness)");
    out.stdout
}

fn esc_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == 0x1b).count()
}

/// Under a real tty, `--color` MUST govern clap's styled help rendering (which
/// clap produces during parsing) — not a no-op masked by the non-tty Auto
/// default. `never` strips to zero ESC bytes; `always` emits ANSI; subcommand
/// help is governed too. Each row carries `want_ansi` so the assertion DIRECTION
/// (esc==0 vs esc>0) is preserved per case.
#[cfg(unix)]
#[test]
fn color_flag_governs_styled_output_under_tty() {
    let cases: &[(&[&str], bool, &str)] = &[
        (&["--color=never", "--help"], false, "never strips help"),
        (
            &["--color=always", "--help"],
            true,
            "always emits help ANSI",
        ),
        (
            &["--color=never", "status", "--help"],
            false,
            "never strips subcommand help",
        ),
    ];
    for &(args, want_ansi, label) in cases {
        let esc = esc_count(&run_under_tty(args));
        if want_ansi {
            assert!(esc > 0, "{label}: expected ANSI under a tty");
        } else {
            assert_eq!(esc, 0, "{label}: expected zero ESC bytes under a tty");
        }
    }
}

#[test]
fn every_subcommand_listed_in_help() {
    let out = bin().arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    for sub in [
        "setup",
        "uninstall",
        "start",
        "stop",
        "status",
        "log",
        "run",
        "reload",
        "lock",
        "doctor",
        "completions",
    ] {
        assert!(help.contains(sub), "help missing subcommand: {sub}");
    }
}

#[test]
fn completions_bash_generates() {
    let out = bin().args(["completions", "bash"]).output().unwrap();
    assert!(out.status.success());
    assert!(!out.stdout.is_empty());
}

#[test]
fn bad_completions_shell_exits_64() {
    let out = bin().args(["completions", "notashell"]).output().unwrap();
    assert_eq!(out.status.code(), Some(64));
    assert!(!out.stderr.is_empty());
}
