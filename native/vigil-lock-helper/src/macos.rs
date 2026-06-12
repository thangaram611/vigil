use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::combo::{self, RequiredFlags};
use crate::{EXIT_INVALID_ARGS, EXIT_OK, EXIT_TAP_FAIL, EXIT_WATCHDOG_FAIL};
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CallbackResult, EventField,
};
use libc::{c_int, c_void};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    #[allow(dead_code)]
    fn CGRequestPostEventAccess() -> bool;
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

const REENABLE_MAX_RETRIES: usize = 3;
const REENABLE_RETRY_SLEEP_MS: u64 = 20;
const LOOP_TICK_MS: u64 = 25;

#[derive(Debug, PartialEq, Eq)]
pub struct PermissionReport {
    pub listen_event_access: bool,
    pub accessibility_trusted: bool,
    pub post_event_access: bool,
    pub tap_create_active_session_ok: bool,
}

impl PermissionReport {
    pub fn ready(&self) -> bool {
        self.listen_event_access && self.accessibility_trusted && self.tap_create_active_session_ok
    }
}

impl fmt::Display for PermissionReport {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "{{\"platform\":\"macos\",\"listen_event_access\":{},\"accessibility_trusted\":{},\"post_event_access\":{},\"tap_create_active_session_ok\":{}}}",
            self.listen_event_access, self.accessibility_trusted, self.post_event_access, self.tap_create_active_session_ok
        )
    }
}

static UNLOCK_REQUESTED: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static REENABLE_REQUESTED: AtomicBool = AtomicBool::new(false);
static LAST_ERROR_CODE: AtomicI32 = AtomicI32::new(EXIT_WATCHDOG_FAIL);
static DEBUG_SLEEP_MS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct ParsedCombo {
    required_flags: RequiredFlags,
    final_keycode: u16,
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

fn prompt_dict() -> CFDictionary<CFString, CFBoolean> {
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())])
}

fn install_signal_handlers() {
    unsafe {
        let handler = signal_handler as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

fn create_doctor_tap() -> bool {
    let callback =
        |_proxy: CGEventTapProxy, event_type: CGEventType, _event: &CGEvent| match event_type {
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                REENABLE_REQUESTED.store(true, Ordering::SeqCst);
                CallbackResult::Keep
            }
            _ => CallbackResult::Keep,
        };

    let tap = match CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        production_events(),
        callback,
    ) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let loop_source = match tap.mach_port().create_runloop_source(0) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let run_loop = CFRunLoop::get_current();
    run_loop.add_source(&loop_source, unsafe { kCFRunLoopCommonModes });
    unsafe {
        CGEventTapEnable(tap.mach_port().as_concrete_TypeRef(), true);
    }
    CFRunLoop::run_in_mode(
        unsafe { kCFRunLoopCommonModes },
        Duration::from_millis(40),
        false,
    );
    let enabled = unsafe { CGEventTapIsEnabled(tap.mach_port().as_concrete_TypeRef()) };
    unsafe {
        CGEventTapEnable(tap.mach_port().as_concrete_TypeRef(), false);
    }
    run_loop.remove_source(&loop_source, unsafe { kCFRunLoopCommonModes });
    enabled
}

pub fn check_permissions(prompt: bool) -> PermissionReport {
    if prompt {
        let _ = unsafe { CGRequestListenEventAccess() };
    }
    let listen_event_access = unsafe { CGPreflightListenEventAccess() };
    let post_event_access = unsafe { CGPreflightPostEventAccess() };
    let accessibility_trusted = if prompt {
        let dict = prompt_dict();
        unsafe { AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef() as *const c_void) }
    } else {
        unsafe { AXIsProcessTrusted() }
    };
    let tap_create_active_session_ok = create_doctor_tap();

    PermissionReport {
        listen_event_access,
        accessibility_trusted,
        post_event_access,
        tap_create_active_session_ok,
    }
}

fn callback_for_freeze(
    parsed: ParsedCombo,
) -> impl Fn(CGEventTapProxy, CGEventType, &CGEvent) -> CallbackResult {
    move |_, event_type, event| {
        let debug_sleep_ms = DEBUG_SLEEP_MS.load(Ordering::SeqCst);
        if debug_sleep_ms > 0 {
            std::thread::sleep(Duration::from_millis(debug_sleep_ms as u64));
        }

        match event_type {
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                REENABLE_REQUESTED.store(true, Ordering::SeqCst);
                CallbackResult::Keep
            }
            event_type
                if is_supported_event_type(event_type)
                    && matches!(
                        event_type,
                        CGEventType::KeyDown | CGEventType::KeyUp | CGEventType::FlagsChanged
                    ) =>
            {
                let keycode =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                let modifiers = event.get_flags();
                if matches!(event_type, CGEventType::KeyDown)
                    && has_required_flags(&modifiers, parsed.required_flags)
                    && keycode == parsed.final_keycode
                {
                    UNLOCK_REQUESTED.store(true, Ordering::SeqCst);
                }
                CallbackResult::Drop
            }
            _ if is_supported_event_type(event_type) => CallbackResult::Drop,
            _ => CallbackResult::Keep,
        }
    }
}

fn has_required_flags(modifiers: &CGEventFlags, req: RequiredFlags) -> bool {
    (!req.control || modifiers.contains(CGEventFlags::CGEventFlagControl))
        && (!req.option || modifiers.contains(CGEventFlags::CGEventFlagAlternate))
        && (!req.shift || modifiers.contains(CGEventFlags::CGEventFlagShift))
        && (!req.command || modifiers.contains(CGEventFlags::CGEventFlagCommand))
}

fn ensure_tap_enabled(tap: &CGEventTap<'_>, retries: usize) -> bool {
    for _ in 0..retries {
        unsafe {
            CGEventTapEnable(tap.mach_port().as_concrete_TypeRef(), true);
        }
        if unsafe { CGEventTapIsEnabled(tap.mach_port().as_concrete_TypeRef()) } {
            // SAFETY: see above in this block — we only call CGEventTapIsEnabled against
            // an owned tap's current port handle.
            return true;
        }
        std::thread::sleep(Duration::from_millis(REENABLE_RETRY_SLEEP_MS));
    }
    false
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

    let parsed = combo::parse_combo(combo).map_err(|_| EXIT_INVALID_ARGS)?;
    let deadline = if max_secs == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(max_secs))
    };

    let callback = callback_for_freeze(ParsedCombo {
        required_flags: parsed.required_flags,
        final_keycode: parsed.final_keycode,
    });
    let tap = CGEventTap::new(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        production_events(),
        callback,
    )
    .map_err(|_| EXIT_TAP_FAIL)?;

    let loop_source = tap
        .mach_port()
        .create_runloop_source(0)
        .map_err(|_| EXIT_TAP_FAIL)?;

    let run_loop = CFRunLoop::get_current();
    run_loop.add_source(&loop_source, unsafe { kCFRunLoopCommonModes });
    unsafe {
        CGEventTapEnable(tap.mach_port().as_concrete_TypeRef(), true);
    }

    let result = loop {
        let _ = CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopCommonModes },
            Duration::from_millis(LOOP_TICK_MS),
            false,
        );

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
        if REENABLE_REQUESTED.swap(false, Ordering::SeqCst) {
            if !ensure_tap_enabled(&tap, REENABLE_MAX_RETRIES) {
                LAST_ERROR_CODE.store(EXIT_WATCHDOG_FAIL, Ordering::SeqCst);
                break Err(EXIT_WATCHDOG_FAIL);
            }
        }
        if !unsafe { CGEventTapIsEnabled(tap.mach_port().as_concrete_TypeRef()) } {
            if !ensure_tap_enabled(&tap, REENABLE_MAX_RETRIES) {
                LAST_ERROR_CODE.store(EXIT_WATCHDOG_FAIL, Ordering::SeqCst);
                break Err(EXIT_WATCHDOG_FAIL);
            }
        }
    };

    unsafe {
        CGEventTapEnable(tap.mach_port().as_concrete_TypeRef(), false);
    }
    run_loop.remove_source(&loop_source, unsafe { kCFRunLoopCommonModes });
    result
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
            tap_create_active_session_ok: true,
        };
        assert_eq!(
            report.to_string(),
            r#"{"platform":"macos","listen_event_access":true,"accessibility_trusted":false,"post_event_access":false,"tap_create_active_session_ok":true}"#
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
}
