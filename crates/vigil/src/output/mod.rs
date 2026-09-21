//! Shared output substrate:
//! - comfy-table table helper
//! - serde_json --json helper
//! - colored clap help styles

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
/// Print a serde value as pretty JSON to stdout.
pub fn print_json<T: serde::Serialize>(value: &T) -> serde_json::Result<()> {
    let s = serde_json::to_string_pretty(value)?;
    anstream::println!("{s}");
    Ok(())
}

// ---- comfy-table helper -----------------------------------------------------
/// Build a comfy-table with the project's default preset and header row.
pub fn table(headers: &[&str]) -> comfy_table::Table {
    use comfy_table::{Table, presets::UTF8_FULL};
    let mut t = Table::new();
    t.load_preset(UTF8_FULL);
    t.set_header(headers.iter().copied());
    t
}

#[cfg(test)]
mod tests {
    #[test]
    fn table_builds() {
        let t = super::table(&["a", "b"]);
        assert!(t.header().is_some());
    }
}
