use std::cell::{Cell, RefCell};
use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::combo::{self, ChordKey};
use crate::overlay::{LockOverlay, MacOverlay, OverlayState};
use crate::{EXIT_INVALID_ARGS, EXIT_OK, EXIT_TAP_FAIL, EXIT_WATCHDOG_FAIL};

use objc2::MainThreadMarker;
use objc2_core_foundation::{
    kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFBoolean, CFDictionary, CFMachPort, CFRetained,
    CFRunLoop, CFRunLoopMode, CFString, CFType,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType, CGPreflightListenEventAccess,
    CGPreflightPostEventAccess, CGRequestListenEventAccess, CGSessionCopyCurrentDictionary,
};

use libc::{c_int, c_void};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

const REENABLE_MAX_RETRIES: usize = 3;
const REENABLE_RETRY_SLEEP_MS: u64 = 20;
const LOOP_TICK_MS: u64 = 25;

#[derive(Debug, PartialEq, Eq)]
pub struct PermissionReport {
    pub listen_event_access: bool,
    pub accessibility_trusted: bool,
    pub post_event_access: bool,
    pub tap_create_active_hid_ok: bool,
}

impl PermissionReport {
    pub fn ready(&self) -> bool {
        self.listen_event_access && self.accessibility_trusted && self.tap_create_active_hid_ok
    }
}

impl fmt::Display for PermissionReport {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "{{\"platform\":\"macos\",\"listen_event_access\":{},\"accessibility_trusted\":{},\"post_event_access\":{},\"tap_create_active_hid_ok\":{}}}",
            self.listen_event_access, self.accessibility_trusted, self.post_event_access, self.tap_create_active_hid_ok
        )
    }
}

static UNLOCK_REQUESTED: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static REENABLE_REQUESTED: AtomicBool = AtomicBool::new(false);
static LAST_ERROR_CODE: AtomicI32 = AtomicI32::new(EXIT_WATCHDOG_FAIL);
static DEBUG_SLEEP_MS: AtomicUsize = AtomicUsize::new(0);

// ── freeze match state ────────────────────────────────────────────────────────
//
// The freeze callback runs as a bare C function pointer on the SAME thread as the
// freeze run loop, so the ordered-chord matcher and the previous modifier bitmask
// live in thread-locals (not atomics). `freeze()` installs the matcher before the
// loop; the callback feeds it translated events and sets UNLOCK_REQUESTED when the
// chord completes in order.
thread_local! {
    static FREEZE_MATCHER: RefCell<Option<combo::SequenceMatcher>> = const { RefCell::new(None) };
    static FREEZE_PREV_MODS: Cell<u8> = const { Cell::new(0) };
}

// ── capture-combo state (the `--capture-combo` mode) ──────────────────────────
//
// Same single-thread model: the ordered accumulator and the finalized sequence
// live in thread-locals; CAPTURE_DONE/CAPTURE_CANCELLED (atomics) signal the run
// loop. CAPTURE_DONE flips true once a result is recorded; CAPTURE_CANCELLED
// distinguishes an Esc-cancel from a real chord.
static CAPTURE_DONE: AtomicBool = AtomicBool::new(false);
static CAPTURE_CANCELLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static CAPTURE_ACC: RefCell<combo::CaptureAccumulator> =
        const { RefCell::new(combo::CaptureAccumulator::new()) };
    static CAPTURE_RESULT: RefCell<Option<Vec<ChordKey>>> = const { RefCell::new(None) };
}

/// The macOS virtual keycode for Esc (`kVK_Escape`). An Esc keydown cancels the
/// capture at any point.
const KEYCODE_ESCAPE: u16 = 53;

/// How long the capture mode waits for a chord before cancelling (seconds).
const CAPTURE_TIMEOUT_SECS: u64 = 30;

/// Collapse `CGEventFlags` to the compact `combo::MOD_*` bitset the state
/// machines speak (only the four chord modifiers; fn/caps-lock are ignored).
fn cg_flags_to_mod_bits(flags: CGEventFlags) -> u8 {
    let mut bits = 0;
    if flags.contains(CGEventFlags::MaskControl) {
        bits |= combo::MOD_CONTROL;
    }
    if flags.contains(CGEventFlags::MaskAlternate) {
        bits |= combo::MOD_OPTION;
    }
    if flags.contains(CGEventFlags::MaskShift) {
        bits |= combo::MOD_SHIFT;
    }
    if flags.contains(CGEventFlags::MaskCommand) {
        bits |= combo::MOD_COMMAND;
    }
    bits
}

#[cfg(target_os = "macos")]
extern "C" fn signal_handler(_sig: c_int) {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

fn is_supported_event_type(event_type: CGEventType) -> bool {
    matches!(
        event_type,
        CGEventType::KeyDown
            | CGEventType::KeyUp
            | CGEventType::FlagsChanged
            | CGEventType::LeftMouseDown
            | CGEventType::LeftMouseUp
            | CGEventType::RightMouseDown
            | CGEventType::RightMouseUp
            | CGEventType::MouseMoved
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::ScrollWheel
            | CGEventType::OtherMouseDown
            | CGEventType::OtherMouseUp
            | CGEventType::OtherMouseDragged
    )
}

fn production_events() -> Vec<CGEventType> {
    vec![
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::ScrollWheel,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::OtherMouseDragged,
    ]
}

/// `CGEventMaskBit(type)` == `1 << type`. Build the events-of-interest mask.
fn events_mask(events: &[CGEventType]) -> CGEventMask {
    events.iter().fold(0u64, |mask, ev| mask | (1u64 << ev.0))
}

fn install_signal_handlers() {
    unsafe {
        let handler = signal_handler as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

fn event_tap_source_mode() -> Option<&'static CFRunLoopMode> {
    unsafe { kCFRunLoopCommonModes }
}

fn event_tap_run_mode() -> Option<&'static CFRunLoopMode> {
    unsafe { kCFRunLoopDefaultMode }
}

/// Event-tap location. We deliberately use the **HID** tap, not the session
/// tap. The phase-4 plan originally specified `Session` on the theory that HID
/// is root-only, but the installed helper holds Input-Monitoring + Accessibility
/// TCC grants, under which a non-root user helper *can* create a HID tap. HID
/// filters at the earliest CoreGraphics insertion point, which is required to
/// swallow *all* hardware input during the freeze; a session tap can miss
/// lower-level synthetic/HID paths. The HID choice is the shipped, working
/// behavior (commit "Use HID event tap for vigil lock") and supersedes the
/// stale phase-4 spec text. See future/phase-5.6-lock-overlay-cf-migration.md.
fn lock_event_tap_location() -> CGEventTapLocation {
    CGEventTapLocation::HIDEventTap
}

fn session_screen_is_locked() -> bool {
    let Some(session) = CGSessionCopyCurrentDictionary() else {
        return false;
    };
    let key = CFString::from_static_str("CGSSessionScreenIsLocked");
    let key_ptr = (&*key as *const CFString).cast::<c_void>();
    // SAFETY: `key_ptr` is a valid CFString pointer for the duration of the
    // lookup; the returned pointer (if non-null) is a borrowed CFType owned by
    // the dictionary (get-rule), valid until `session` is dropped below.
    let value_ptr = unsafe { session.value(key_ptr) };
    if value_ptr.is_null() {
        return false;
    }
    let value: &CFType = unsafe { &*value_ptr.cast::<CFType>() };
    value
        .downcast_ref::<CFBoolean>()
        .map(CFBoolean::value)
        .unwrap_or(false)
}

struct TapResources {
    port: CFRetained<CFMachPort>,
    source: CFRetained<objc2_core_foundation::CFRunLoopSource>,
}

/// The event-tap callback ABI shared by the freeze and capture taps.
type TapCallback = unsafe extern "C-unwind" fn(
    CGEventTapProxy,
    CGEventType,
    NonNull<CGEvent>,
    *mut c_void,
) -> *mut CGEvent;

/// Create the event tap port + run-loop source on the current run loop, wired to
/// `callback`. Returns `None` if the tap could not be created (missing TCC grant)
/// or the source could not be made. Both the freeze and the capture mode build
/// the SAME HID tap (same location/placement/options/mask); only the callback
/// differs (freeze records the unlock match; capture records the first chord and
/// swallows it).
fn create_tap_on_current_runloop_with(callback: TapCallback) -> Option<TapResources> {
    // SAFETY: `callback` is a correctly-typed CGEventTapCallBack and takes no
    // user_info. tap_create returns a retained CFMachPort.
    let port = unsafe {
        CGEvent::tap_create(
            lock_event_tap_location(),
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            events_mask(&production_events()),
            Some(callback),
            std::ptr::null_mut(),
        )
    }?;

    let source = CFMachPort::new_run_loop_source(None, Some(&port), 0)?;
    let run_loop = CFRunLoop::current()?;
    run_loop.add_source(Some(&source), event_tap_source_mode());
    Some(TapResources { port, source })
}

/// Create the freeze event tap (the production unlock-watching tap).
fn create_tap_on_current_runloop() -> Option<TapResources> {
    create_tap_on_current_runloop_with(freeze_tap_callback)
}

fn create_doctor_tap() -> bool {
    let Some(res) = create_tap_on_current_runloop() else {
        return false;
    };
    let run_loop = match CFRunLoop::current() {
        Some(rl) => rl,
        None => return false,
    };
    CGEvent::tap_enable(&res.port, true);
    CFRunLoop::run_in_mode(event_tap_run_mode(), 0.040, false);
    let enabled = CGEvent::tap_is_enabled(&res.port);
    CGEvent::tap_enable(&res.port, false);
    run_loop.remove_source(Some(&res.source), event_tap_source_mode());
    enabled
}

pub fn check_permissions(prompt: bool) -> PermissionReport {
    if prompt {
        let _ = CGRequestListenEventAccess();
    }
    let listen_event_access = CGPreflightListenEventAccess();
    let post_event_access = CGPreflightPostEventAccess();
    let accessibility_trusted = if prompt {
        let prompt_options = prompt_dict_ptr();
        unsafe { AXIsProcessTrustedWithOptions(prompt_options) }
    } else {
        unsafe { AXIsProcessTrusted() }
    };
    let tap_create_active_hid_ok = create_doctor_tap();

    PermissionReport {
        listen_event_access,
        accessibility_trusted,
        post_event_access,
        tap_create_active_hid_ok,
    }
}

/// Build a `{ kAXTrustedCheckOptionPrompt: true }` dictionary and return its
/// pointer for `AXIsProcessTrustedWithOptions`. The dictionary is leaked for the
/// duration of the (synchronous) call by holding it in a `CFRetained` that is
/// dropped after the caller returns — so we keep it alive via a thread-local.
fn prompt_dict_ptr() -> *const c_void {
    use std::cell::RefCell;
    thread_local! {
        static PROMPT_DICT: RefCell<Option<CFRetained<CFDictionary>>> = const { RefCell::new(None) };
    }
    // kAXTrustedCheckOptionPrompt is a CFStringRef.
    let key: &CFString = unsafe { &*(kAXTrustedCheckOptionPrompt as *const CFString) };
    let value = CFBoolean::new(true);
    // `&value` deref-coerces `CFRetained<CFBoolean>` to the `&CFBoolean` the
    // slice element type requires; the borrow is load-bearing, not needless.
    #[allow(clippy::needless_borrow)]
    let dict: CFRetained<CFDictionary<CFString, CFBoolean>> =
        CFDictionary::from_slices(&[key], &[&value]);
    let dict: CFRetained<CFDictionary> = unsafe { CFRetained::cast_unchecked(dict) };
    let ptr = (&*dict as *const CFDictionary).cast::<c_void>();
    PROMPT_DICT.with(|slot| *slot.borrow_mut() = Some(dict));
    ptr
}

/// Feed a key-down into the freeze matcher; request unlock on completion.
fn feed_freeze_down(key: ChordKey) {
    FREEZE_MATCHER.with(|m| {
        if let Some(matcher) = m.borrow_mut().as_mut() {
            if matcher.on_down(key) {
                UNLOCK_REQUESTED.store(true, Ordering::SeqCst);
            }
        }
    });
}

/// Feed a key-up into the freeze matcher.
fn feed_freeze_up(key: ChordKey) {
    FREEZE_MATCHER.with(|m| {
        if let Some(matcher) = m.borrow_mut().as_mut() {
            matcher.on_up(key);
        }
    });
}

/// Translate a modifier-bitmask change into discrete modifier down/up events for
/// the matcher (FlagsChanged carries the whole set, so we diff against the prior).
fn feed_freeze_flags(now: u8) {
    let prev = FREEZE_PREV_MODS.get();
    FREEZE_PREV_MODS.set(now);
    let added = now & !prev;
    let removed = prev & !now;
    FREEZE_MATCHER.with(|m| {
        if let Some(matcher) = m.borrow_mut().as_mut() {
            for modk in combo::MODIFIER_TABLE {
                if added & modk.bit() != 0 && matcher.on_down(ChordKey::Mod(modk)) {
                    UNLOCK_REQUESTED.store(true, Ordering::SeqCst);
                }
                if removed & modk.bit() != 0 {
                    matcher.on_up(ChordKey::Mod(modk));
                }
            }
        }
    });
}

/// The bare C event-tap callback. Runs on the freeze CFRunLoop. Returns the
/// event pointer to keep it, or null to drop it. Every keyboard event is fed to
/// the ordered-chord matcher (and swallowed); the matcher requests unlock only
/// when the chord is pressed in its recorded order.
unsafe extern "C-unwind" fn freeze_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    _user_info: *mut c_void,
) -> *mut CGEvent {
    let keep = event.as_ptr();
    let drop_event = std::ptr::null_mut();

    let debug_sleep_ms = DEBUG_SLEEP_MS.load(Ordering::SeqCst);
    if debug_sleep_ms > 0 {
        std::thread::sleep(Duration::from_millis(debug_sleep_ms as u64));
    }

    if session_screen_is_locked() {
        return keep;
    }

    match event_type {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            REENABLE_REQUESTED.store(true, Ordering::SeqCst);
            keep
        }
        CGEventType::KeyDown => {
            let event_ref = unsafe { event.as_ref() };
            let keycode =
                CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode)
                    as u16;
            feed_freeze_down(ChordKey::Key(keycode));
            drop_event
        }
        CGEventType::KeyUp => {
            let event_ref = unsafe { event.as_ref() };
            let keycode =
                CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode)
                    as u16;
            feed_freeze_up(ChordKey::Key(keycode));
            drop_event
        }
        CGEventType::FlagsChanged => {
            let event_ref = unsafe { event.as_ref() };
            let now = cg_flags_to_mod_bits(CGEvent::flags(Some(event_ref)));
            feed_freeze_flags(now);
            drop_event
        }
        et if is_supported_event_type(et) => drop_event,
        _ => keep,
    }
}

fn ensure_tap_enabled(port: &CFMachPort, retries: usize) -> bool {
    for _ in 0..retries {
        CGEvent::tap_enable(port, true);
        if CGEvent::tap_is_enabled(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(REENABLE_RETRY_SLEEP_MS));
    }
    false
}

/// Build the per-tick overlay state from the remaining deadline.
fn overlay_state(deadline: Option<Instant>, now: Instant) -> OverlayState {
    let seconds_remaining =
        deadline.map(|end| end.saturating_duration_since(now).as_secs_f64().ceil() as u64);
    OverlayState {
        armed: true,
        seconds_remaining,
        status_line: "Vigil lock active".to_string(),
    }
}

pub fn freeze(combo: &str, max_secs: u64, debug_sleep_ms: Option<u64>) -> Result<(), i32> {
    UNLOCK_REQUESTED.store(false, Ordering::SeqCst);
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    REENABLE_REQUESTED.store(false, Ordering::SeqCst);
    LAST_ERROR_CODE.store(EXIT_WATCHDOG_FAIL, Ordering::SeqCst);
    DEBUG_SLEEP_MS.store(
        debug_sleep_ms.unwrap_or(0).try_into().unwrap_or(0),
        Ordering::SeqCst,
    );

    install_signal_handlers();

    let parsed = combo::parse_chord(combo).map_err(|_| EXIT_INVALID_ARGS)?;
    FREEZE_PREV_MODS.set(0);
    FREEZE_MATCHER.with(|m| *m.borrow_mut() = Some(combo::SequenceMatcher::new(parsed.keys)));

    let deadline = if max_secs == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(max_secs))
    };

    let res = create_tap_on_current_runloop().ok_or(EXIT_TAP_FAIL)?;
    let run_loop = CFRunLoop::current().ok_or(EXIT_TAP_FAIL)?;
    CGEvent::tap_enable(&res.port, true);

    // The overlay is created/updated/torn down on THIS run loop (the helper's
    // main thread) — no separate UI thread. It is only available when we are on
    // the main thread (which the freeze loop always is).
    let mut overlay = MainThreadMarker::new().map(MacOverlay::new);
    if let Some(overlay) = overlay.as_mut() {
        overlay.show(&overlay_state(deadline, Instant::now()));
    }

    let result = loop {
        let _ = CFRunLoop::run_in_mode(event_tap_run_mode(), LOOP_TICK_MS as f64 / 1000.0, false);

        // Refresh the overlay (drives the 1-Hz countdown text).
        if let Some(overlay) = overlay.as_mut() {
            overlay.show(&overlay_state(deadline, Instant::now()));
        }

        if UNLOCK_REQUESTED.swap(false, Ordering::SeqCst) {
            LAST_ERROR_CODE.store(EXIT_OK, Ordering::SeqCst);
            break Ok(());
        }
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            LAST_ERROR_CODE.store(EXIT_WATCHDOG_FAIL, Ordering::SeqCst);
            break Err(EXIT_WATCHDOG_FAIL);
        }
        if let Some(end_at) = deadline {
            if Instant::now() >= end_at {
                LAST_ERROR_CODE.store(EXIT_WATCHDOG_FAIL, Ordering::SeqCst);
                break Err(EXIT_WATCHDOG_FAIL);
            }
        }
        if REENABLE_REQUESTED.swap(false, Ordering::SeqCst)
            && !ensure_tap_enabled(&res.port, REENABLE_MAX_RETRIES)
        {
            LAST_ERROR_CODE.store(EXIT_WATCHDOG_FAIL, Ordering::SeqCst);
            break Err(EXIT_WATCHDOG_FAIL);
        }
        if !CGEvent::tap_is_enabled(&res.port)
            && !ensure_tap_enabled(&res.port, REENABLE_MAX_RETRIES)
        {
            LAST_ERROR_CODE.store(EXIT_WATCHDOG_FAIL, Ordering::SeqCst);
            break Err(EXIT_WATCHDOG_FAIL);
        }
    };

    if let Some(overlay) = overlay.as_mut() {
        overlay.hide();
    }
    CGEvent::tap_enable(&res.port, false);
    run_loop.remove_source(Some(&res.source), event_tap_source_mode());
    FREEZE_MATCHER.with(|m| *m.borrow_mut() = None);
    result
}

// ── capture-combo: register an unlock chord by pressing it ────────────────────

/// Apply the accumulator's verdict to the shared signal flags.
fn apply_capture_step(step: combo::CaptureStep) {
    match step {
        combo::CaptureStep::Continue => {}
        combo::CaptureStep::Cancel => {
            CAPTURE_CANCELLED.store(true, Ordering::SeqCst);
            CAPTURE_DONE.store(true, Ordering::SeqCst);
        }
        combo::CaptureStep::Done(seq) => {
            CAPTURE_RESULT.with(|r| *r.borrow_mut() = Some(seq));
            CAPTURE_DONE.store(true, Ordering::SeqCst);
        }
    }
}

/// The capture event-tap callback. Runs on the capture CFRunLoop. SWALLOWS all
/// supported input (returns null to drop) so the pressed chord never reaches app
/// shortcuts, and feeds every keyboard event to the ordered [`combo::
/// CaptureAccumulator`], which records the chord in press order and finalizes when
/// the user releases the anchor (the first key pressed).
///
/// - Each regular `KeyDown`/`KeyUp` and modifier `FlagsChanged` is fed in order.
/// - An `Esc` keydown (keycode 53) cancels at any point.
/// - Tap-disable notifications re-enable via the run loop (same as freeze).
unsafe extern "C-unwind" fn capture_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    _user_info: *mut c_void,
) -> *mut CGEvent {
    let keep = event.as_ptr();
    let drop_event = std::ptr::null_mut();

    match event_type {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            REENABLE_REQUESTED.store(true, Ordering::SeqCst);
            keep
        }
        CGEventType::KeyDown => {
            let event_ref = unsafe { event.as_ref() };
            let keycode =
                CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode)
                    as u16;
            let step = CAPTURE_ACC.with(|a| {
                a.borrow_mut()
                    .on_key_down(keycode, keycode == KEYCODE_ESCAPE)
            });
            apply_capture_step(step);
            drop_event
        }
        CGEventType::KeyUp => {
            let event_ref = unsafe { event.as_ref() };
            let keycode =
                CGEvent::integer_value_field(Some(event_ref), CGEventField::KeyboardEventKeycode)
                    as u16;
            let step = CAPTURE_ACC.with(|a| a.borrow_mut().on_key_up(keycode));
            apply_capture_step(step);
            drop_event
        }
        CGEventType::FlagsChanged => {
            let event_ref = unsafe { event.as_ref() };
            let now = cg_flags_to_mod_bits(CGEvent::flags(Some(event_ref)));
            let step = CAPTURE_ACC.with(|a| a.borrow_mut().on_flags(now));
            apply_capture_step(step);
            drop_event
        }
        // Swallow every other supported event so nothing leaks while the user
        // presses the chord; pass through anything we don't filter.
        et if is_supported_event_type(et) => drop_event,
        _ => keep,
    }
}

/// Interactively capture a single unlock chord by pressing it.
///
/// Builds the SAME HID event tap the freeze uses, but with a callback that
/// swallows input and records the chord in press order, finalizing when the anchor
/// (first key pressed) is released. Returns the canonical (order-preserving) combo
/// string on success; on cancel (Esc), 30s timeout, an unmapped key, or
/// tap-creation failure returns the appropriate non-zero exit code (the caller
/// prints no combo in that case).
pub fn capture_combo() -> Result<String, i32> {
    CAPTURE_DONE.store(false, Ordering::SeqCst);
    CAPTURE_CANCELLED.store(false, Ordering::SeqCst);
    CAPTURE_ACC.with(|a| *a.borrow_mut() = combo::CaptureAccumulator::new());
    CAPTURE_RESULT.with(|r| *r.borrow_mut() = None);
    REENABLE_REQUESTED.store(false, Ordering::SeqCst);
    STOP_REQUESTED.store(false, Ordering::SeqCst);

    install_signal_handlers();

    let res = create_tap_on_current_runloop_with(capture_tap_callback).ok_or(EXIT_TAP_FAIL)?;
    let run_loop = CFRunLoop::current().ok_or(EXIT_TAP_FAIL)?;
    CGEvent::tap_enable(&res.port, true);

    let deadline = Instant::now() + Duration::from_secs(CAPTURE_TIMEOUT_SECS);

    let outcome = loop {
        let _ = CFRunLoop::run_in_mode(event_tap_run_mode(), LOOP_TICK_MS as f64 / 1000.0, false);

        if CAPTURE_DONE.load(Ordering::SeqCst) {
            if CAPTURE_CANCELLED.load(Ordering::SeqCst) {
                break Err(EXIT_WATCHDOG_FAIL); // Esc cancel
            }
            let seq = CAPTURE_RESULT.with(|r| r.borrow_mut().take());
            match seq.as_deref().and_then(combo::canonical_from_sequence) {
                Some(canonical) if !canonical.is_empty() => break Ok(canonical),
                _ => break Err(EXIT_INVALID_ARGS), // unmapped / empty
            }
        }
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            break Err(EXIT_WATCHDOG_FAIL); // SIGINT/SIGTERM
        }
        if Instant::now() >= deadline {
            break Err(EXIT_WATCHDOG_FAIL); // 30s timeout cancel
        }
        if REENABLE_REQUESTED.swap(false, Ordering::SeqCst)
            && !ensure_tap_enabled(&res.port, REENABLE_MAX_RETRIES)
        {
            break Err(EXIT_TAP_FAIL);
        }
        if !CGEvent::tap_is_enabled(&res.port)
            && !ensure_tap_enabled(&res.port, REENABLE_MAX_RETRIES)
        {
            break Err(EXIT_TAP_FAIL);
        }
    };

    CGEvent::tap_enable(&res.port, false);
    run_loop.remove_source(Some(&res.source), event_tap_source_mode());
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_json_uses_required_fields_in_stable_order() {
        let report = PermissionReport {
            listen_event_access: true,
            accessibility_trusted: false,
            post_event_access: false,
            tap_create_active_hid_ok: true,
        };
        assert_eq!(
            report.to_string(),
            r#"{"platform":"macos","listen_event_access":true,"accessibility_trusted":false,"post_event_access":false,"tap_create_active_hid_ok":true}"#
        );
    }

    #[test]
    fn event_types_are_droppable_set() {
        assert!(is_supported_event_type(CGEventType::KeyDown));
        assert!(is_supported_event_type(CGEventType::MouseMoved));
        assert!(is_supported_event_type(CGEventType::LeftMouseDragged));
        assert!(is_supported_event_type(CGEventType::ScrollWheel));
        assert!(!is_supported_event_type(CGEventType::Null));
    }

    #[test]
    fn run_loop_uses_specific_mode_not_common_modes_sentinel() {
        // Default mode and common modes are distinct CF run-loop modes.
        assert!(event_tap_run_mode().is_some());
        assert!(event_tap_source_mode().is_some());
        assert!(!std::ptr::eq(
            event_tap_run_mode().unwrap(),
            event_tap_source_mode().unwrap()
        ));
    }

    #[test]
    fn lock_tap_uses_hid_location() {
        assert!(matches!(
            lock_event_tap_location(),
            CGEventTapLocation::HIDEventTap
        ));
    }

    #[test]
    fn events_mask_sets_one_bit_per_event() {
        let mask = events_mask(&[CGEventType::KeyDown, CGEventType::KeyUp]);
        assert_eq!(mask, (1u64 << 10) | (1u64 << 11));
    }

    #[test]
    fn cg_flags_to_mod_bits_maps_the_four_chord_modifiers() {
        let all = CGEventFlags::MaskControl
            | CGEventFlags::MaskAlternate
            | CGEventFlags::MaskShift
            | CGEventFlags::MaskCommand;
        assert_eq!(
            cg_flags_to_mod_bits(all),
            combo::MOD_CONTROL | combo::MOD_OPTION | combo::MOD_SHIFT | combo::MOD_COMMAND
        );
        assert_eq!(
            cg_flags_to_mod_bits(CGEventFlags::MaskControl | CGEventFlags::MaskCommand),
            combo::MOD_CONTROL | combo::MOD_COMMAND
        );
    }
}
