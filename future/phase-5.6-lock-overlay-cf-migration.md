# Phase 5.6 — Native lock overlay window + CoreGraphics/CoreFoundation → objc2 migration

> **Status: IMPLEMENTED.** This is the per-slice doc for 5.6. Per the rewrite
> cadence, 5.6 ran AFTER 5.7 (which already ported `cmd_lock`/`cmd_lock_doctor`
> into the `vigil` CLI crate and deleted all bash). 5.6 is therefore confined to
> the standalone helper binary `native/vigil-lock-helper/`. It touches zero bash
> (there is none left), and does NOT modify the `vigil` crate, the daemon, the
> service layer, or the check code.

This slice landed three things inside `vigil-lock-helper`:

1. a native borderless overlay window raised above the dock + menu bar, behind a
   portable `LockOverlay` trait with a macOS impl and a headless test stub;
2. the full CoreGraphics + CoreFoundation surface migrated off the old
   `core-graphics 0.25` / `core-foundation 0.10` crates onto the `objc2-*`
   family (`objc2-core-graphics`, `objc2-core-foundation`, plus `objc2-app-kit` /
   `objc2-foundation` for the overlay) — achieving the "one CF family" goal;
3. the four deferred clippy lints cleared so the helper is clean WORKSPACE-WIDE
   under `cargo clippy --all-targets -- -D warnings`.

The umbrella plan `future/phase-5-rust-rewrite.md` §5.6 (lines 728–851) was the
original obligation map, but it was written before 5.7. Every part of §5.6 that
talks about the CLI (pre-arm, tick-file waiting, exit-code matrix, indicatif
countdown, lock-doctor alignment, `--max-secs 0` guard) is DONE in 5.7 and was
intentionally ignored here.

---

## Decisions resolved

### D1 — Migrate vs descope "one CF family" → **OPTION (a): MIGRATE.**

`objc2-core-graphics` is published at **0.3.2** and, verified against its source
(`~/.cargo/registry/.../objc2-core-graphics-0.3.2/src/generated/`), exposes the
**entire** CGEventTap surface the helper needs as first-class, non-deprecated
API:

- `CGEvent::tap_create` / `tap_enable` / `tap_is_enabled` (the deprecated free
  `CGEventTapCreate`/`CGEventTapEnable`/`CGEventTapIsEnabled` re-exports exist too
  but we use the non-deprecated associated fns so `-D warnings` stays clean);
- the typed callback `CGEventTapCallBack = Option<unsafe extern "C-unwind"
  fn(CGEventTapProxy, CGEventType, NonNull<CGEvent>, *mut c_void) -> *mut CGEvent>`;
- `CGEventTapLocation` (`HIDEventTap`/`SessionEventTap`/`AnnotatedSessionEventTap`),
  `CGEventTapPlacement::HeadInsertEventTap`, `CGEventTapOptions::Default`;
- `CGEventType`, `CGEventField::KeyboardEventKeycode`, `CGEventFlags`
  (`MaskControl/MaskShift/MaskAlternate/MaskCommand`), `CGEventMask`;
- `CGEvent::flags` / `CGEvent::integer_value_field` accessors;
- `CGPreflightListenEventAccess` / `CGRequestListenEventAccess` /
  `CGPreflightPostEventAccess` (safe wrappers);
- `CGSessionCopyCurrentDictionary` (→ `Option<CFRetained<CFDictionary>>`);
- `kCGPopUpMenuWindowLevel` (= 101) in `CGWindowLevel`.

`objc2-core-foundation 0.3.2` provides the rest (`CFRunLoop`, `CFRunLoopMode`,
`kCFRunLoopCommonModes`/`kCFRunLoopDefaultMode`, `CFMachPort::new_run_loop_source`,
`CFDictionary`, `CFString`, `CFBoolean`, `CFType::downcast_ref`) in its default
feature set. Because the crate IS usable, option (b) (keep `core-graphics` for the
tap, add only `objc2-app-kit`, accept a two-CF-family binary) was rejected.

**Result:** `core-graphics` and `core-foundation` are entirely removed from the
binary — confirmed by their absence from `Cargo.lock`. The only framework symbol
NOT available through objc2 is `AXIsProcessTrusted` /
`AXIsProcessTrustedWithOptions` / `kAXTrustedCheckOptionPrompt` (these live in the
ApplicationServices/AXUIElement header, not CoreGraphics), so a small
`#[link(name = "ApplicationServices")] extern "C"` block survives for those three
— same as before. This is not a CF-family regression; it is a separate framework
that neither the old nor the new CF crates ever covered.

### D2 — `CGEventTapLocation::HID` vs `Session` → **KEEP HID (code was already correct; the phase-4 spec text is stale).**

`future/phase-4-lock-feature.md` lines 31–35 claimed `kCGHIDEventTap` is
"root-only" and that `CGEventTapCreate` returns NULL for a non-root HID tap, and
therefore prescribed `CGEventTapLocation::Session`. That claim is **wrong for the
shipped configuration**: the installed helper holds **Input-Monitoring (listen
event access) + Accessibility** TCC grants, and under those grants a normal
foreground user helper *can* create a HID tap. The reality is recorded by:

- the most recent commit, `5b21934 "Use HID event tap for vigil lock"`;
- the live code (`lock_event_tap_location()` returns `HIDEventTap`) and its unit
  test `lock_tap_uses_hid_location`;
- `docs/macos-lock-and-locked-use.md` §"Event-tap boundary" (lines 5, 49–57).

HID is the **correct** location for this feature because it filters at the
earliest CoreGraphics insertion point, which is what is required to swallow *all*
hardware input during the freeze; a Session-level tap sits later in the pipeline
and can miss lower-level synthetic/HID-injected paths. The freeze's whole purpose
is to drop every key/mouse/scroll event until the unlock chord, so the
earliest-point tap is the right boundary.

**Resolution:** keep `HIDEventTap`. The discrepancy is documented here and in a
code comment on `lock_event_tap_location()` in `src/macos.rs`. The phase-4 spec
text is superseded and should not be treated as authoritative on this point.

### D3 — Edition / crate versions.

- Edition stays **2021**. No objc2 dependency forced an edition bump.
- All crates pulled at **latest** via `cargo add`, no hand-pinned versions:
  `objc2 0.6.4`, `objc2-app-kit 0.3.2`, `objc2-foundation 0.3.2`,
  `objc2-core-foundation 0.3.2`, `objc2-core-graphics 0.3.2` (the plan's stale
  "0.3.2"-era numbers happened to match, but they were resolved by cargo, not
  copied from the plan). Default feature sets of all five cover every symbol used
  (AppKit defaults include `NSWindow`/`NSScreen`/`NSTextField`/`NSColor`/
  `NSApplication`/`NSFont`; CG defaults include `CGEvent`/`CGEventTypes`/
  `CGSession`/`CGWindowLevel`; CF defaults include `CFRunLoop`/`CFMachPort`/
  `CFDictionary`/`CFString`/`CFNumber`), so no explicit feature lists were needed.
- The Carbon `kVK_*` virtual key codes (`ANSI_A`..`Z`, digits, `F1`..`F12`,
  `space`/`tab`/`return`) are NOT exposed as named constants by
  `objc2-core-graphics` the way the old `core-graphics` crate's `KeyCode::ANSI_*`
  were. They are carried as a small frozen `keycode` module in `src/combo.rs`
  (values verified against `HIToolbox/Events.h`). These are a stable macOS ABI.

---

## What was built

### Overlay (`src/overlay.rs`, new)

- `trait LockOverlay { fn show(&mut self, &OverlayState); fn hide(&mut self); }`
  and `struct OverlayState { armed, seconds_remaining: Option<u64>, unlock_chord,
  status_line }` with `status_text()` (appends `— {n}s remaining` when a deadline
  exists) and `chord_hint()` rendering helpers.
- **macOS impl `MacOverlay`** over `objc2-app-kit`:
  - a borderless `NSWindow` (`NSWindowStyleMask::Borderless`,
    `NSBackingStoreType::Buffered`) covering `NSScreen::mainScreen().frame()`;
  - raised via `setLevel(101)` (`kCGPopUpMenuWindowLevel`) — above dock (20) and
    menu bar (24), a level winit/eframe cannot reach, which is the reason this is
    native;
  - `NSApplicationActivationPolicy::Accessory` so no Dock icon appears;
  - two `NSTextField` label subviews (status line + unlock-chord hint), NOT a
    custom `drawRect`/Core-Text path;
  - `setIgnoresMouseEvents(true)` as belt-and-suspenders (the event tap already
    swallows input);
  - created on the **main thread** via `MainThreadMarker`; `show()` updates the
    two text fields and calls `setNeedsDisplayInRect` so the 1-Hz countdown text
    re-renders; `hide()`/`Drop` order the window out and close it.
- **Headless `StubOverlay`** (used by unit tests and the non-macOS build) records
  `shown` history, `last`, `hide_count`, and `visible` so arm→countdown→hide
  transitions are unit-testable without a window server.

### Freeze-loop wiring (`src/macos.rs`)

- The overlay is constructed only when `MainThreadMarker::new()` succeeds (the
  freeze loop always runs on the helper's main thread), shown when the freeze
  arms, refreshed every loop tick (driving the per-second countdown), and
  hidden on unlock / timeout / stop — all on the **existing CFRunLoop tap loop,
  no separate thread**, exactly as required.
- `overlay_state()` computes `seconds_remaining` by `ceil`-ing the time left to
  the deadline; `--max-secs 0` (no deadline) yields `None` (overlay shows the
  status line with no countdown).

### CF/CG migration (`src/macos.rs`, `src/combo.rs`, `Cargo.toml`)

- The event tap is now created with `CGEvent::tap_create` taking the bare
  `unsafe extern "C-unwind"` `freeze_tap_callback`. Because the callback is a raw
  C function pointer (no captured environment), the combo it must match is
  published into process atomics (`FREEZE_FINAL_KEYCODE: AtomicU16`,
  `FREEZE_REQUIRED_FLAGS: AtomicU32` with bit flags) before the loop arms, and
  read back inside the callback. All the other coordination flags
  (`UNLOCK_REQUESTED`, `STOP_REQUESTED`, `REENABLE_REQUESTED`, `DEBUG_SLEEP_MS`)
  keep their previous static-atomic shape.
- The events-of-interest mask is built as `Σ 1<<event_type.0` (`CGEventMaskBit`).
- `session_screen_is_locked()` reads `CGSessionCopyCurrentDictionary()`, looks up
  `CGSSessionScreenIsLocked` via `CFDictionary::value` and `CFType::downcast_ref::
  <CFBoolean>()`, treating a missing key as "not locked" (unchanged semantics).
- The run-loop discipline is preserved: the tap source is scheduled in
  `kCFRunLoopCommonModes`, the loop runs in `kCFRunLoopDefaultMode` (passing the
  common-modes sentinel to `CFRunLoopRunInMode` is invalid; see
  `docs/macos-lock-and-locked-use.md`). With objc2 these modes are
  `Option<&'static CFRunLoopMode>` statics.
- `src/combo.rs` migrated from `core_graphics::event::{CGEventFlags, KeyCode}` to
  `objc2_core_graphics::CGEventFlags` + the local `keycode` module; the non-macOS
  stub `CGEventFlags` was updated to the `Mask*` constant names. The
  `event_matches_combo` helper + its `#[allow(dead_code)]` were moved BEFORE the
  test module (fixing the `items_after_test_module` lint).

### Clippy lints cleared (the four deferred for 5.6)

- `function_casts_as_integer` — `signal_handler as libc::sighandler_t` is now
  `signal_handler as *const () as libc::sighandler_t`.
- two `collapsible_if` sites in `freeze()` — the `REENABLE_REQUESTED` re-enable
  and the `!tap_is_enabled` re-enable each collapsed to a single `&&` guard.
- `items_after_test_module` in `combo.rs` — `event_matches_combo` moved up.
- `map_identity` in `main.rs` — `.map_err(|e| e)?` → `?`.

Verified: `cargo clippy --all-targets -- -D warnings` is clean WORKSPACE-WIDE
(previously only `-p vigil` was clean).

### Tests added

`overlay::tests` (run via the stub, no window server needed):
- `stub_records_arm_then_countdown_then_hide` — arm (3s) → tick 2 → tick 1 →
  hide; asserts visibility, the full 3-entry transition history, and the
  countdown values.
- `status_and_chord_text_formats` — `status_text()`/`chord_hint()` with and
  without a deadline.

`macos::tests` gained `events_mask_sets_one_bit_per_event` and
`required_flags_round_trip_through_bits`; the existing
`run_loop_uses_specific_mode_not_common_modes_sentinel` and
`lock_tap_uses_hid_location` tests were adapted to the objc2 types. The combo
tests are unchanged in behavior.

---

## What is manual-verify-only (cannot be checked on the build host)

The macOS overlay **compiles** on this Mac (it is NOT hidden behind a cfg that
skips compilation), and the state-transition logic is unit-tested via the stub.
But the following are **on-device visual checks** a human must run, because the
build host cannot observe the window server, TCC prompts, or real hardware input:

1. The overlay window actually renders **above the dock and the menu bar** when
   the freeze arms (the `setLevel(101)` behavior).
2. No Dock icon appears (the `Accessory` activation policy).
3. The **countdown animates at ~1 Hz** and reads down to the deadline; with
   `--max-secs 0` the overlay shows the status line and no countdown.
4. The overlay **tears down** on unlock-chord, on timeout, and on SIGINT/SIGTERM.
5. With Input-Monitoring + Accessibility granted, the HID tap is actually
   **created** (`vigil lock doctor` → `tap_create_active_hid_ok: true`) and
   swallows all keyboard/mouse/scroll input during the freeze, releasing only on
   the configured chord.
6. Lock-screen pass-through still works (events pass through while
   `CGSSessionScreenIsLocked` is true).

---

## Acceptance checks (run from the repo root)

| Gate | Command | Expected |
|---|---|---|
| fmt | `cargo fmt --check` | clean (no diff) |
| build | `cargo build` | whole workspace compiles, incl. the macOS overlay path |
| clippy | `cargo clippy --all-targets -- -D warnings` | clean WORKSPACE-WIDE incl. `vigil-lock-helper` |
| test | `cargo test` | all suites green |
| release | `cargo build -p vigil-lock-helper --release` | helper builds optimized |

Plus the manual on-device checks (1–6) above.

All five automated gates passed at implementation time. `core-graphics` and
`core-foundation` no longer appear in `Cargo.lock`.

---

## Out of scope / descoped

- Nothing was descoped from the 5.6 deliverables — option (a) (the harder
  migrate path) was taken in full, so the "one CF family" goal is **met**, not
  descoped.
- All CLI-side §5.6 obligations in the umbrella plan (pre-arm/tick-file wait,
  exit-code matrix, indicatif countdown, lock-doctor alignment, `--max-secs 0`
  guard) were already delivered in 5.7 and are not part of this slice.
- The helper remains a **separate binary** (its Input-Monitoring/Accessibility
  TCC grant is pinned to a stable install path); it was NOT merged into `vigil`.
