//! Native lock overlay window.
//!
//! The overlay is a borderless full-screen window raised above the dock and
//! menu bar (`kCGPopUpMenuWindowLevel` = 101). `winit`/`eframe` cannot reach
//! that window level, which is why this is a hand-rolled `NSWindow` over
//! `objc2-app-kit`. It is created, updated, and torn down on the helper's
//! existing main-thread CFRunLoop freeze loop (`macos.rs`) — there is NO
//! separate UI thread.
//!
//! Everything is expressed behind the [`LockOverlay`] trait so the non-macOS
//! build (and unit tests) compile against the headless [`StubOverlay`], which
//! merely records the last [`OverlayState`] it was shown.

/// The data the overlay renders. Cheap to clone; the freeze loop builds a fresh
/// one each tick and hands it to [`LockOverlay::show`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OverlayState {
    /// True while the freeze is armed (the tap is swallowing input).
    pub armed: bool,
    /// Whole seconds remaining before the watchdog times out. `None` when the
    /// freeze was armed with `--max-secs 0` (no deadline).
    pub seconds_remaining: Option<u64>,
    /// One-line status string shown prominently (e.g. `"Vigil lock active"`).
    pub status_line: String,
}

impl OverlayState {
    /// The unlock-chord hint line as rendered to the user. Deliberately generic:
    /// the overlay never displays the literal chord, so an onlooker cannot read
    /// the unlock combination off the locked screen.
    pub fn chord_hint(&self) -> String {
        "Press your unlock chord to continue".to_string()
    }

    /// The status line including the countdown suffix when a deadline exists.
    pub fn status_text(&self) -> String {
        match self.seconds_remaining {
            Some(secs) => format!("{} — {}s remaining", self.status_line, secs),
            None => self.status_line.clone(),
        }
    }
}

/// Show / update / hide the lock overlay. Implementations own their native
/// resources and tear them down on [`hide`](LockOverlay::hide) or `Drop`.
pub trait LockOverlay {
    /// Create (first call) or update (subsequent calls) the overlay to reflect
    /// `state`. Idempotent per-state: calling with an unchanged state is cheap.
    fn show(&mut self, state: &OverlayState);
    /// Tear the overlay down. Safe to call when nothing is shown.
    fn hide(&mut self);
}

/// Headless overlay used on non-macOS targets and in unit tests. Records the
/// transition history so the freeze-loop wiring can be tested without a window
/// server. On macOS the real [`MacOverlay`] is used at runtime, so the stub is
/// only exercised by tests there.
#[cfg_attr(all(target_os = "macos", not(test)), allow(dead_code))]
#[derive(Debug, Default)]
pub struct StubOverlay {
    /// Every state passed to `show`, in order.
    pub shown: Vec<OverlayState>,
    /// The most recent state passed to `show`, or `None` after `hide`.
    pub last: Option<OverlayState>,
    /// Number of times `hide` was called.
    pub hide_count: usize,
    /// True while a state is currently shown (between `show` and `hide`).
    pub visible: bool,
}

impl StubOverlay {
    #[cfg_attr(all(target_os = "macos", not(test)), allow(dead_code))]
    pub fn new() -> Self {
        Self::default()
    }
}

impl LockOverlay for StubOverlay {
    fn show(&mut self, state: &OverlayState) {
        self.shown.push(state.clone());
        self.last = Some(state.clone());
        self.visible = true;
    }

    fn hide(&mut self) {
        self.hide_count += 1;
        self.last = None;
        self.visible = false;
    }
}

#[cfg(target_os = "macos")]
pub use macos::MacOverlay;

#[cfg(target_os = "macos")]
mod macos {
    use super::{LockOverlay, OverlayState};
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont,
        NSScreen, NSTextAlignment, NSTextField, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    /// `kCGPopUpMenuWindowLevel`. Raised above the dock (20) and menu bar (24)
    /// so the overlay is not occluded. winit/eframe top out at the normal
    /// window level, which is why this is native.
    const POPUP_MENU_WINDOW_LEVEL: isize = 101;

    /// macOS overlay. All fields are main-thread-only AppKit objects; the whole
    /// type is therefore `!Send`/`!Sync`, which matches its single-threaded use
    /// on the freeze CFRunLoop.
    pub struct MacOverlay {
        mtm: MainThreadMarker,
        window: Option<Retained<NSWindow>>,
        status_field: Option<Retained<NSTextField>>,
        chord_field: Option<Retained<NSTextField>>,
        last_state: Option<OverlayState>,
    }

    impl MacOverlay {
        /// Construct an overlay bound to the main thread. Must be called from
        /// the thread that owns the CFRunLoop the freeze loop runs on.
        pub fn new(mtm: MainThreadMarker) -> Self {
            Self {
                mtm,
                window: None,
                status_field: None,
                chord_field: None,
                last_state: None,
            }
        }

        fn ensure_window(&mut self) {
            if self.window.is_some() {
                return;
            }
            let mtm = self.mtm;

            // Accessory activation policy: a UI-bearing process with no Dock
            // icon and no menu bar of its own.
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

            let screen_frame: NSRect = match NSScreen::mainScreen(mtm) {
                Some(screen) => screen.frame(),
                None => NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0)),
            };

            // Borderless window covering the whole main screen.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    screen_frame,
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };

            window.setLevel(POPUP_MENU_WINDOW_LEVEL);
            // Fully opaque: nothing behind the overlay (e.g. a terminal still
            // showing the unlock combo you typed) must be visible through it.
            window.setOpaque(true);
            let bg = NSColor::colorWithSRGBRed_green_blue_alpha(0.05, 0.05, 0.07, 1.0);
            window.setBackgroundColor(Some(&bg));
            // Belt-and-suspenders: the event tap already swallows input, but in
            // case the tap is briefly disabled we never want clicks to reach the
            // window either.
            window.setIgnoresMouseEvents(true);
            // We retain the window ourselves; do not let -close free it.
            unsafe { window.setReleasedWhenClosed(false) };

            let content = window
                .contentView()
                .unwrap_or_else(|| panic!("borderless window must have a content view"));

            // Both labels are created with a throwaway zero frame; their real
            // size and position are computed in `apply_text` once the strings
            // (and thus their fitted widths) are known. See `apply_text` for the
            // centering math, which runs on every text change.
            let status_field = Self::make_label(mtm, 34.0);
            content.addSubview(&status_field);

            let chord_field = Self::make_label(mtm, 20.0);
            content.addSubview(&chord_field);

            window.orderFrontRegardless();

            self.window = Some(window);
            self.status_field = Some(status_field);
            self.chord_field = Some(chord_field);
        }

        fn make_label(mtm: MainThreadMarker, font_size: f64) -> Retained<NSTextField> {
            let empty = NSString::from_str("");
            let field = NSTextField::labelWithString(&empty, mtm);
            field.setBezeled(false);
            field.setDrawsBackground(false);
            field.setEditable(false);
            field.setBordered(false);
            // NSTextAlignmentCenter == 2; objc2-app-kit only names Left/Right
            // /Justified/Natural, so build the center variant directly.
            field.setAlignment(NSTextAlignment(2));
            let white = NSColor::whiteColor();
            field.setTextColor(Some(&white));
            let font = NSFont::systemFontOfSize(font_size);
            field.setFont(Some(&font));
            field
        }

        fn apply_text(&self, state: &OverlayState) {
            // Set the strings first, then size each label to its text. We center
            // by computing frame origins from the fitted sizes rather than by
            // relying on cell alignment inside an over-wide frame (which can let
            // the glyph run drift to an edge once the string is assigned).
            if let Some(field) = &self.status_field {
                let s = NSString::from_str(&state.status_text());
                field.setStringValue(&s);
                field.sizeToFit();
            }
            if let Some(field) = &self.chord_field {
                let s = NSString::from_str(&state.chord_hint());
                field.setStringValue(&s);
                field.sizeToFit();
            }

            if let (Some(status), Some(chord), Some(window)) =
                (&self.status_field, &self.chord_field, &self.window)
            {
                if let Some(content) = window.contentView() {
                    // The stock NSWindow content view is NOT flipped: its origin
                    // is bottom-left and y grows UPWARD. The math below centers
                    // the two-line block around the content's vertical midpoint
                    // on that basis. If the content view is ever replaced with a
                    // flipped one, the y terms invert.
                    let content_size = content.frame().size;
                    let cw = content_size.width;
                    let ch = content_size.height;

                    let status_size = status.frame().size;
                    let chord_size = chord.frame().size;

                    let gap = 16.0;
                    let total = status_size.height + gap + chord_size.height;
                    let block_bottom = (ch - total) / 2.0;

                    // Hint sits below the status line; status sits above it.
                    chord.setFrameOrigin(NSPoint::new((cw - chord_size.width) / 2.0, block_bottom));
                    status.setFrameOrigin(NSPoint::new(
                        (cw - status_size.width) / 2.0,
                        block_bottom + chord_size.height + gap,
                    ));

                    // Force a redraw so the 1-Hz countdown renders promptly.
                    let bounds: NSRect = content.frame();
                    content.setNeedsDisplayInRect(bounds);
                }
            }
        }
    }

    impl LockOverlay for MacOverlay {
        fn show(&mut self, state: &OverlayState) {
            self.ensure_window();
            // Skip the AppKit round-trip when nothing changed (the loop ticks
            // far faster than once per second).
            if self.last_state.as_ref() == Some(state) {
                return;
            }
            self.apply_text(state);
            self.last_state = Some(state.clone());
        }

        fn hide(&mut self) {
            if let Some(window) = self.window.take() {
                window.orderOut(None);
                window.close();
            }
            self.status_field = None;
            self.chord_field = None;
            self.last_state = None;
        }
    }

    impl Drop for MacOverlay {
        fn drop(&mut self) {
            self.hide();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_records_arm_then_countdown_then_hide() {
        let mut overlay = StubOverlay::new();

        let armed = OverlayState {
            armed: true,
            seconds_remaining: Some(3),
            status_line: "Vigil lock active".to_string(),
        };
        overlay.show(&armed);
        assert!(overlay.visible);
        assert_eq!(overlay.last.as_ref(), Some(&armed));

        // Countdown ticks 3 -> 2 -> 1.
        for secs in [2u64, 1] {
            let tick = OverlayState {
                seconds_remaining: Some(secs),
                ..armed.clone()
            };
            overlay.show(&tick);
            assert_eq!(overlay.last.as_ref().unwrap().seconds_remaining, Some(secs));
        }

        overlay.hide();
        assert!(!overlay.visible);
        assert_eq!(overlay.hide_count, 1);
        assert!(overlay.last.is_none());

        // Full transition history: armed + two countdown ticks.
        assert_eq!(overlay.shown.len(), 3);
        assert_eq!(overlay.shown[0].seconds_remaining, Some(3));
        assert_eq!(overlay.shown[1].seconds_remaining, Some(2));
        assert_eq!(overlay.shown[2].seconds_remaining, Some(1));
    }

    #[test]
    fn status_and_chord_text_formats() {
        let with_deadline = OverlayState {
            armed: true,
            seconds_remaining: Some(42),
            status_line: "Vigil lock active".to_string(),
        };
        assert_eq!(
            with_deadline.status_text(),
            "Vigil lock active — 42s remaining"
        );
        assert_eq!(
            with_deadline.chord_hint(),
            "Press your unlock chord to continue"
        );

        let no_deadline = OverlayState {
            seconds_remaining: None,
            status_line: "Vigil lock active".to_string(),
            ..Default::default()
        };
        assert_eq!(no_deadline.status_text(), "Vigil lock active");
        assert_eq!(
            no_deadline.chord_hint(),
            "Press your unlock chord to continue"
        );
    }
}
