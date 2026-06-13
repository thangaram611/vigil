//! `src/commands/conf_writer.rs` — the format-preserving vigil.conf writer.
//!
//! `vigil lock setup` persists the registered unlock chord (`lock_combo`) and the
//! preferred timeout (`lock_max_secs`) back into the user's `vigil.conf`. The conf
//! is strict TOML (see `config/mod.rs`), so we edit it with `toml_edit` —
//! preserving ALL other keys, comments, and formatting rather than rewriting the
//! file from a serialized struct (which would drop comments and reorder keys).
//!
//! The TOML keys are the serde field names on `RawConfig` (`config/mod.rs`):
//! `lock_combo` and `lock_max_secs` (lowercase snake_case, NOT the `VIGIL_*`
//! env-var spellings). After writing, `vigil lock` (no args) reads
//! `cfg.lock_combo` / `cfg.lock_max_secs` as its defaults, so the chord "just
//! runs".

use std::path::Path;

use toml_edit::{DocumentMut, value};

/// Resolve the conf file path the same way `cmd_config` / `config_file_path` do:
/// `$VIGIL_CONFIG_FILE`, else `$HOME/.config/vigil/vigil.conf`.
pub(crate) fn conf_path() -> String {
    super::config_file_path()
}

/// Persist `lock_combo` and `lock_max_secs` into the TOML conf at `path`,
/// preserving every other key, comment, and the existing formatting.
///
/// - The parent directory and the file are created if absent.
/// - An absent/empty file starts from an empty `DocumentMut`.
/// - Returns `Err(message)` on any read/parse/write failure so the command can
///   surface a clear error + `EX_ERROR`.
pub(crate) fn write_lock_settings(path: &str, combo: &str, max_secs: u32) -> Result<(), String> {
    let p = Path::new(path);
    if let Some(parent) = p.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create config dir {}: {e}", parent.display()))?;
    }

    // Read the existing file (empty doc if absent). A present-but-unreadable file
    // is a hard error — we must not clobber a conf we could not read.
    let existing = match std::fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("could not read {path}: {e}")),
    };

    let mut doc: DocumentMut = existing
        .parse()
        .map_err(|e| format!("could not parse {path} as TOML: {e}"))?;

    // Set both keys at the document root (the serde field names). Setting an
    // existing key edits in place (preserving its surrounding trivia); a new key
    // is appended. `max_secs` fits an i64 for any u32.
    doc["lock_combo"] = value(combo);
    doc["lock_max_secs"] = value(max_secs as i64);

    std::fs::write(p, doc.to_string()).map_err(|e| format!("could not write {path}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The writer sets BOTH keys and preserves an unrelated pre-existing key and a
    /// comment + the original formatting around them.
    #[test]
    fn writes_both_keys_and_preserves_unrelated_key_and_comment() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("vigil.conf");
        std::fs::write(
            &conf,
            "# my hand-written vigil config\nidle_after_sec = 999  # keep me\nlock_combo = \"ctrl+alt+shift+cmd+z\"\n",
        )
        .unwrap();

        write_lock_settings(conf.to_str().unwrap(), "ctrl+alt+shift+cmd+l", 600).unwrap();

        let out = std::fs::read_to_string(&conf).unwrap();
        // Both target keys are set to the new values.
        assert!(
            out.contains("lock_combo = \"ctrl+alt+shift+cmd+l\""),
            "lock_combo must be updated: {out}"
        );
        assert!(
            out.contains("lock_max_secs = 600"),
            "lock_max_secs must be set: {out}"
        );
        // The unrelated key + its inline comment + the leading comment survive.
        assert!(
            out.contains("# my hand-written vigil config"),
            "leading comment must be preserved: {out}"
        );
        assert!(
            out.contains("idle_after_sec = 999"),
            "unrelated key must be preserved: {out}"
        );
        assert!(
            out.contains("# keep me"),
            "inline comment must be preserved: {out}"
        );
        // The old lock_combo value is gone (edited in place, not duplicated).
        assert!(
            !out.contains("ctrl+alt+shift+cmd+z"),
            "old combo must be replaced, not left behind: {out}"
        );

        // And the result re-parses as valid TOML with exactly the new values.
        // (We re-parse the file directly rather than via config::load, which
        // would read process-wide VIGIL_* env that other tests mutate.)
        let doc: DocumentMut = out.parse().unwrap();
        assert_eq!(doc["lock_combo"].as_str(), Some("ctrl+alt+shift+cmd+l"));
        assert_eq!(doc["lock_max_secs"].as_integer(), Some(600));
        assert_eq!(doc["idle_after_sec"].as_integer(), Some(999));
    }

    /// An absent file is created (with its parent dir) and both keys are written.
    #[test]
    fn creates_file_and_parent_dir_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("nested/sub/vigil.conf");
        assert!(!conf.exists());

        write_lock_settings(conf.to_str().unwrap(), "ctrl+shift+cmd+5", 0).unwrap();

        assert!(conf.exists(), "conf file must be created");
        let out = std::fs::read_to_string(&conf).unwrap();
        let doc: DocumentMut = out.parse().unwrap();
        assert_eq!(doc["lock_combo"].as_str(), Some("ctrl+shift+cmd+5"));
        assert_eq!(doc["lock_max_secs"].as_integer(), Some(0));
    }
}
