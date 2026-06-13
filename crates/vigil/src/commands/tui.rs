//! `src/commands/tui.rs` — a hand-rolled `@clack/prompts`-style terminal UI.
//!
//! This is the interactive presentation layer for the security-sensitive
//! lifecycle commands (`setup`, `uninstall`) and the `lock setup` capture flow.
//! It renders a left "rail" with connector glyphs — a sequential step log in the
//! clack idiom:
//!
//! ```text
//! ┌  vigil: setting up
//! │
//! ◇  preparing user directories
//! │  → state dir: …
//! ✓  user directories ready
//! │
//! └  setup complete
//! ```
//!
//! Built on mature crates only — `console` (styling + glyphs) and `dialoguer`
//! (confirm/input). Steps render STATICALLY — there is NO background animation
//! thread. The privileged lifecycle commands shell out to `sudo`, and a live
//! steady-tick spinner races sudo's password prompt and corrupts it (the prompt
//! gets overwritten by a tick and sudo never reads a password). So a step is a
//! plain `◇ start` / `✓ done` pair drawn on the rail, with the work — including
//! any `sudo` prompt or chord capture — running between them with the terminal
//! left entirely to that subprocess. No `indicatif`, no `cliclack`.
//!
//! ## The non-interactive contract (CRITICAL)
//!
//! Every method carries a SINGLE `interactive` bit (set once from the command's
//! `interactive(yes)` gate). When that bit is FALSE — `--yes`, piped, CI,
//! `NO_COLOR`, a non-tty on either end — EVERY method falls back to the EXACT
//! plain `anstream::println!` lines the commands printed before this module
//! existed. No rail glyph, no color, nothing that could block on a prompt. The
//! interactive bit is the only thing that changes the bytes; when it is false we
//! never touch `dialoguer` at all.
//!
//! So each method takes the **plain line** as the source of truth: in
//! non-interactive mode it is printed verbatim; in interactive mode it is
//! re-dressed with the rail. Methods that have a distinct interactive title
//! (e.g. a step whose `◇` header differs from the finished `✓` success line)
//! take both an interactive title and the plain fallback.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use console::style;

/// The rail connector glyphs, rendered in a muted cyan-blue (clack's rail).
const RAIL_TOP: &str = "\u{250c}"; // ┌
const RAIL_BAR: &str = "\u{2502}"; // │
const RAIL_BOTTOM: &str = "\u{2514}"; // └

/// Step status glyphs.
const G_SUCCESS: &str = "\u{2713}"; // ✓
const G_INFO: &str = "\u{25c7}"; // ◇
const G_WARN: &str = "\u{25b2}"; // ▲
// Part of the clack vocabulary (error step glyph). Exposed via `Tui::error` for
// callers beyond setup/uninstall; not yet wired into a step there.
#[allow(dead_code)]
const G_ERROR: &str = "\u{25a0}"; // ■
const G_ARROW: &str = "\u{2192}"; // →

/// Style the rail glyph the same muted blue everywhere (256-color 39 ≈ a soft
/// cyan-blue; falls back gracefully when color is stripped by anstream upstream
/// or by `console`'s own `NO_COLOR` handling).
fn rail(glyph: &str) -> console::StyledObject<&str> {
    style(glyph).color256(39)
}

/// A clack-style interactive UI bound to a single `interactive` decision.
///
/// Construct once per command from `Tui::new(interactive(yes))`; thread it into
/// every step. When `interactive` is false, every method degrades to the exact
/// pre-existing plain output (see the module docs).
#[derive(Clone, Copy)]
pub(crate) struct Tui {
    interactive: bool,
}

impl Tui {
    /// Bind the UI to a single interactive decision (the command's
    /// `interactive(yes)` gate). `false` selects the byte-frozen plain path.
    pub(crate) fn new(interactive: bool) -> Self {
        Self { interactive }
    }

    /// Whether this UI renders the rail (true) or the plain fallback (false).
    pub(crate) fn is_interactive(self) -> bool {
        self.interactive
    }

    /// Intro line: `┌  <bold title>` (interactive) or the verbatim plain line.
    pub(crate) fn intro(self, title: &str) {
        if self.interactive {
            anstream::println!("{}  {}", rail(RAIL_TOP), style(title).bold());
        } else {
            anstream::println!("{title}");
        }
    }

    /// Outro line: a blank rail bar, then `└  <green bold title>` (interactive),
    /// or the verbatim plain line.
    pub(crate) fn outro(self, title: &str) {
        if self.interactive {
            anstream::println!("{}", rail(RAIL_BAR));
            anstream::println!("{}  {}", rail(RAIL_BOTTOM), style(title).green().bold());
        } else {
            anstream::println!("{title}");
        }
    }

    /// Cancel/abort outro: `└  <red title>` (interactive) or the verbatim plain
    /// line. Used for the "declined the confirm" no-op exit.
    pub(crate) fn outro_cancel(self, msg: &str) {
        if self.interactive {
            anstream::println!("{}", rail(RAIL_BAR));
            anstream::println!("{}  {}", rail(RAIL_BOTTOM), style(msg).red());
        } else {
            anstream::println!("{msg}");
        }
    }

    /// A lone rail spacer (`│`) between steps (interactive), or the blank line
    /// the plain path printed between steps.
    pub(crate) fn rail_space(self) {
        if self.interactive {
            anstream::println!("{}", rail(RAIL_BAR));
        } else {
            anstream::println!();
        }
    }

    /// Success step: `✓  <bold title>` with a GREEN check (interactive), or the
    /// verbatim plain line. (API surface; the static step's [`Step::done`] is
    /// what setup/uninstall use to land a step on a green check.)
    #[allow(dead_code)]
    pub(crate) fn step_success(self, title: &str, plain: &str) {
        if self.interactive {
            anstream::println!("{}  {}", style(G_SUCCESS).green(), style(title).bold());
        } else {
            anstream::println!("{plain}");
        }
    }

    /// Info step: `◇  <title>` (interactive) or the verbatim plain line. (API
    /// surface; setup/uninstall open each numbered step via [`Tui::step`], which
    /// carries the same `◇` rail prefix.)
    #[allow(dead_code)]
    pub(crate) fn step_info(self, title: &str, plain: &str) {
        if self.interactive {
            anstream::println!("{}  {}", rail(G_INFO), title);
        } else {
            anstream::println!("{plain}");
        }
    }

    /// Warning: `▲  <yellow title>` (interactive) or the verbatim plain line.
    /// (API surface for callers beyond setup/uninstall.)
    #[allow(dead_code)]
    pub(crate) fn warn(self, title: &str, plain: &str) {
        if self.interactive {
            anstream::println!("{}  {}", style(G_WARN).yellow(), style(title).yellow());
        } else {
            anstream::println!("{plain}");
        }
    }

    /// Error: `■  <red title>` (interactive) or the verbatim plain line. (API
    /// surface for callers beyond setup/uninstall.)
    #[allow(dead_code)]
    pub(crate) fn error(self, title: &str, plain: &str) {
        if self.interactive {
            anstream::println!("{}  {}", style(G_ERROR).red(), style(title).red());
        } else {
            anstream::println!("{plain}");
        }
    }

    /// Sub-detail under a step: `│  → <dim text>` (interactive) or the verbatim
    /// plain line. The `→` and text are dim/grey. Used both by [`Step::detail`]
    /// (details printed by the command) and by the shared install helpers
    /// (`cmd_sync_install`, `cmd_install_root_helper`, …) so their progress lines
    /// sit on the rail in setup/uninstall while staying byte-identical in the
    /// plain (reload/CI) path.
    pub(crate) fn detail(self, text: &str, plain: &str) {
        if self.interactive {
            anstream::println!(
                "{}  {} {}",
                rail(RAIL_BAR),
                style(G_ARROW).dim(),
                style(text).dim()
            );
        } else {
            anstream::println!("{plain}");
        }
    }

    /// Open a step.
    ///
    /// Interactive: prints `◇  <title>` immediately (the in-progress marker) and
    /// returns a [`Step`] you land on a green `✓` via [`Step::done`]. The header
    /// gives immediate feedback for slow steps (e.g. a `cargo build`).
    ///
    /// Non-interactive: prints the plain header line immediately (matching the
    /// pre-existing `step_line` behavior) and returns an inert [`Step`] whose
    /// methods all take the plain path.
    ///
    /// Rendered STATICALLY — no background thread. See the module docs for why a
    /// live spinner is unusable next to `sudo`.
    pub(crate) fn step(self, title: &str, plain_header: &str) -> Step {
        if self.interactive {
            anstream::println!("{}  {}", rail(G_INFO), title);
        } else {
            anstream::println!("{plain_header}");
        }
        Step {
            interactive: self.interactive,
        }
    }

    /// Confirm prompt. Interactive: a rail-prefixed `dialoguer::Confirm`.
    /// Non-interactive: returns `default` WITHOUT prompting (so CI/piped runs
    /// never block) — exactly the behavior the `interactive(yes)` gate gave the
    /// callers, which skipped the confirm entirely.
    pub(crate) fn confirm(self, prompt: &str, default: bool) -> bool {
        if !self.interactive {
            return default;
        }
        // Rail-prefix the prompt text so it sits on the rail like a step.
        let railed = format!("{}  {}", rail(G_INFO), prompt);
        dialoguer::Confirm::new()
            .with_prompt(railed)
            .default(default)
            .interact()
            .unwrap_or(default)
    }

    /// Free-text input. Interactive: a rail-prefixed `dialoguer::Input`.
    /// Non-interactive: returns the empty string without prompting.
    pub(crate) fn input(self, prompt: &str) -> String {
        if !self.interactive {
            return String::new();
        }
        let railed = format!("{}  {}", rail(G_INFO), prompt);
        dialoguer::Input::<String>::new()
            .with_prompt(railed)
            .allow_empty(true)
            .interact_text()
            .unwrap_or_default()
    }
}

/// A statically-rendered step (no animation thread).
///
/// In interactive mode it printed a `◇  <title>` marker at construction and you
/// land it on `✓  <success>` via [`Step::done`]; the work runs between the two
/// lines. In non-interactive mode the plain header was already printed and every
/// method takes the plain path.
///
/// Static rendering is deliberate: the privileged steps shell out to `sudo`,
/// whose password prompt must own the terminal. With no background thread there
/// is nothing to race the prompt — the (former) live spinner did, which is the
/// regression this replaced.
#[derive(Clone, Copy)]
pub(crate) struct Step {
    interactive: bool,
}

impl Step {
    /// Print a sub-detail line under the step. Interactive: `│  → <dim text>`.
    /// Non-interactive: the verbatim plain line.
    pub(crate) fn detail(self, text: &str, plain: &str) {
        if self.interactive {
            anstream::println!(
                "{}  {} {}",
                rail(RAIL_BAR),
                style(G_ARROW).dim(),
                style(text).dim()
            );
        } else {
            anstream::println!("{plain}");
        }
    }

    /// Run a block that owns the terminal (a `sudo` password prompt, a chord
    /// capture). With static rendering there is nothing to suspend — this just
    /// runs the closure. It is kept so call sites document "this region owns the
    /// terminal" and so the shape survived the live-spinner → static-step change.
    pub(crate) fn suspend<T, F: FnOnce() -> T>(self, f: F) -> T {
        f()
    }

    /// Land the step on its success line: `✓  <green bold success>`
    /// (interactive) or nothing (the plain header already covered this step,
    /// matching the pre-existing `finish_step` no-op).
    pub(crate) fn done(self, success_title: &str) {
        if self.interactive {
            anstream::println!(
                "{}  {}",
                style(G_SUCCESS).green(),
                style(success_title).bold()
            );
        }
    }
}

/// Run `work` while animating a spinner on STDERR labelled `label`.
///
/// INTERACTIVE ONLY: when `interactive` is false (piped, CI, non-tty) this just
/// runs `work` with NO output, so the caller's stdout stays byte-identical (the
/// golden status/doctor tests depend on that). The spinner draws to STDERR and
/// clears its line before returning, so it never touches the command's stdout
/// report. Read-only commands (`status`/`doctor`) use this so their first paint
/// isn't a silent wait on the privileged-helper liveness probe.
pub(crate) fn with_activity<T>(interactive: bool, label: &str, work: impl FnOnce() -> T) -> T {
    if !interactive {
        return work();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let stop = Arc::clone(&stop);
        let label = label.to_string();
        std::thread::spawn(move || {
            const FRAMES: [&str; 10] = [
                "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
                "\u{2827}", "\u{2807}", "\u{280f}",
            ];
            let mut i = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let mut err = std::io::stderr();
                let _ = write!(
                    err,
                    "\r{}  {} ",
                    rail(FRAMES[i % FRAMES.len()]),
                    style(&label).dim()
                );
                let _ = err.flush();
                i = i.wrapping_add(1);
                std::thread::sleep(Duration::from_millis(90));
            }
            // Clear the spinner line so the stdout report paints cleanly.
            let mut err = std::io::stderr();
            let _ = write!(err, "\r\u{1b}[2K");
            let _ = err.flush();
        })
    };
    let result = work();
    stop.store(true, Ordering::Relaxed);
    let _ = ticker.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The non-interactive UI never animates and never blocks — a step on the
    /// plain path is inert: it prints the plain header at construction and its
    /// methods take the plain branch without panicking. This locks the "CI/piped
    /// runs stay byte-identical to the old plain path" contract.
    #[test]
    fn non_interactive_step_is_plain() {
        let ui = Tui::new(false);
        assert!(!ui.is_interactive());
        let st = ui.step("running", "  1. running");
        assert!(
            !st.interactive,
            "non-interactive step must take the plain path"
        );
        // Methods must not panic on the plain path.
        st.detail("x", "  x");
        let ran = st.suspend(|| 7);
        assert_eq!(ran, 7, "suspend must run the closure and return its value");
        st.done("done");
    }

    /// Non-interactive `confirm` returns the default WITHOUT prompting, so a
    /// piped/CI run can never block and proceeds exactly as the old skip-the-
    /// confirm path did.
    #[test]
    fn non_interactive_confirm_returns_default() {
        let ui = Tui::new(false);
        assert!(ui.confirm("proceed?", true));
        assert!(!ui.confirm("proceed?", false));
    }

    /// Non-interactive `input` returns the empty string without prompting.
    #[test]
    fn non_interactive_input_is_empty() {
        let ui = Tui::new(false);
        assert_eq!(ui.input("name?"), "");
    }
}
