//! Output substrate every later slice prints through:
//! - anstream print macros (auto NO_COLOR/CLICOLOR/non-tty + --color stripping)
//! - owo-colors styled pass/fail symbol + color vocabulary
//! - comfy-table table helper
//! - serde_json --json helper
//! - colored clap help styles

use std::io::Write;

use owo_colors::OwoColorize;

// ---- glyphs (the visual language later slices reuse) ------------------------
pub const CHECK: &str = "\u{2713}"; // ✓
pub const CROSS: &str = "\u{2717}"; // ✗
pub const WARN: &str = "\u{26a0}"; // ⚠
pub const ARROW: &str = "\u{2192}"; // →

// ---- styled symbol helpers (owo-colors; stripped automatically) -------------
// Return owo-colors styled strings; anstream's stdout/stderr strips ANSI when
// color is off. Use these at every pass/fail print site.
#[allow(dead_code)]
pub fn pass() -> impl std::fmt::Display {
    CHECK.green().to_string()
}
#[allow(dead_code)]
pub fn fail() -> impl std::fmt::Display {
    CROSS.red().to_string()
}
#[allow(dead_code)]
pub fn warn() -> impl std::fmt::Display {
    WARN.yellow().to_string()
}
#[allow(dead_code)]
pub fn arrow() -> impl std::fmt::Display {
    ARROW.cyan().to_string()
}

/// Print a `<symbol> <label>` status line through anstream (auto-stripped).
#[allow(dead_code)]
pub fn status_line(symbol: &str, label: &str) {
    anstream::println!("{symbol} {label}");
}

// ---- clap colored help ------------------------------------------------------
/// anstyle-based styles for colored clap help/usage/error output.
pub fn clap_styles() -> clap::builder::Styles {
    use anstyle::{AnsiColor, Style};
    clap::builder::Styles::styled()
        .header(Style::new().bold().fg_color(Some(AnsiColor::Green.into())))
        .usage(Style::new().bold().fg_color(Some(AnsiColor::Green.into())))
        .literal(Style::new().fg_color(Some(AnsiColor::Cyan.into())))
        .placeholder(Style::new().fg_color(Some(AnsiColor::Cyan.into())))
}

// ---- json helper ------------------------------------------------------------
/// Print a serde value as pretty JSON to stdout (for future `--json`).
#[allow(dead_code)]
pub fn print_json<T: serde::Serialize>(value: &T) -> serde_json::Result<()> {
    let s = serde_json::to_string_pretty(value)?;
    anstream::println!("{s}");
    Ok(())
}

// ---- comfy-table helper -----------------------------------------------------
/// Build a comfy-table with the project's default preset and header row.
#[allow(dead_code)]
pub fn table(headers: &[&str]) -> comfy_table::Table {
    use comfy_table::{Table, presets::UTF8_FULL};
    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(headers.iter().copied());
    t
}

// ---- completions ------------------------------------------------------------
/// Generate a completion script for `shell` to stdout.
pub fn generate_completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    let mut cmd = crate::Cli::command();
    let name = cmd.get_name().to_string();
    // clap_complete writes raw bytes; stdout is fine (scripts have no ANSI).
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_builds() {
        let t = super::table(&["a", "b"]);
        assert!(t.header().is_some());
    }

    #[test]
    fn json_serializes() {
        #[derive(serde::Serialize)]
        struct X {
            v: u8,
        }
        // round-trip via serde_json directly (print_json writes to stdout)
        assert_eq!(serde_json::to_string(&X { v: 1 }).unwrap(), "{\"v\":1}");
    }

    #[test]
    fn glyphs_are_defined() {
        assert_eq!(super::CHECK, "\u{2713}");
        assert_eq!(super::CROSS, "\u{2717}");
        assert_eq!(super::WARN, "\u{26a0}");
        assert_eq!(super::ARROW, "\u{2192}");
    }
}
