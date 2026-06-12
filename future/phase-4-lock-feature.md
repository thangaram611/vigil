# Phase 4 - Lock feature

> **Status: SECOND-OPINION-INCORPORATED IMPLEMENTATION PLAN.** Ready for the
> implementation worker.

## Goal

Add `vigil lock`, a local "freeze guard" for the active macOS GUI session. When
armed, Vigil consumes user input until the configured unlock chord is pressed.
This protects a long-running agent session from accidental or casual local input
while the machine stays awake.

This is not a replacement for macOS authentication. If the user wants the real
macOS Lock Screen, they should use the OS lock flow and unlock with password,
Touch ID, or Watch unlock. A user-space event tap cannot combo-unlock the secure
login UI and should not pretend it can.

## Reviewed platform facts

Validated on 2026-06-12 against the local Command Line Tools SDK, current Apple
docs, current Rust crate metadata, and a second-opinion agent review:

- `AppKit.framework/Headers/NSWorkspace.h` contains no public `lockScreen`
  symbol. Do not implement the old `NSWorkspace.lockScreen` sketch.
- `CoreGraphics.framework/Headers/CGSession.h` exposes
  `CGSessionCopyCurrentDictionary` and session-notification constants only; it
  does not expose a public lock/unlock call.
- The legacy executable
  `/System/Library/CoreServices/Menu Extras/User.menu/Contents/Resources/CGSession`
  is not present on this machine. Do not hard-code it as the lock path.
- `kCGHIDEventTap` / `CGEventTapLocation::HID` is root-only. A normal foreground
  user helper must not use it; `CGEventTapCreate` returns NULL for non-root HID
  taps.
- Phase 4 must use an active session tap:
  `CGEventTapLocation::Session`, `CGEventTapPlacement::HeadInsertEventTap`,
  `CGEventTapOptions::Default`.
- `CoreGraphics.framework/Headers/CGEvent.h` exposes `CGEventTapCreate`,
  `CGEventTapEnable`, `CGEventTapIsEnabled`,
  `CGPreflightListenEventAccess`, `CGRequestListenEventAccess`,
  `CGPreflightPostEventAccess`, and `CGRequestPostEventAccess`.
- `ApplicationServices.framework/.../AXUIElement.h` exposes
  `AXIsProcessTrusted`, `AXIsProcessTrustedWithOptions`, and
  `kAXTrustedCheckOptionPrompt`. Accessibility prompting is asynchronous and
  does not make the current return value true.
- Latest small-model-friendly crate choices checked by `cargo search`:
  `core-graphics = "0.25.0"` and `core-foundation = "0.10.1"`.
- Keep `objc2-core-graphics` out of phase 4. It is current, but generated,
  feature-flag-heavy, and lower-level than needed for this helper.

## Product shape

Ship these user-facing commands:

```text
vigil lock
vigil lock --combo <combo>
vigil lock --max-secs <seconds>
vigil lock doctor
vigil lock doctor --prompt
```

`vigil lock`:

- Requires macOS for this phase. On other OSes, exit with a clear phase-5
  message.
- Runs the helper's no-prompt doctor probe before arming.
- Refuses to arm unless the helper reports that the production session tap can
  be created and enabled. Do not rely only on preflight booleans.
- Prints the selected unlock chord, max runtime, helper path, and recovery
  command before input is consumed.
- Starts the Rust helper in the foreground and returns when the helper exits.

`vigil lock doctor`:

- Checks helper existence and executable bit at `$VIGIL_LOCK_HELPER`.
- Runs the helper's no-prompt permission/tap probe.
- Reports these fields separately:
  - `listen_event_access`
  - `accessibility_trusted`
  - `post_event_access`
  - `tap_create_active_session_ok`
- Prints exact System Settings guidance:
  - `System Settings > Privacy & Security > Input Monitoring`
  - `System Settings > Privacy & Security > Accessibility`
- Does not call prompt APIs by default.

`vigil lock doctor --prompt`:

- May call `CGRequestListenEventAccess`.
- May call `AXIsProcessTrustedWithOptions` with
  `kAXTrustedCheckOptionPrompt=true`.
- Must explain that prompts/settings changes are asynchronous and that the user
  should rerun `vigil lock doctor` after granting access.
- Must not call `CGRequestPostEventAccess` in phase 4. Posting synthetic events
  is only for a future optional OS-lock mode.

## Scope decisions

- Implement the custom freeze guard first. Do not trigger the macOS Lock Screen
  in the default phase-4 path.
- Treat real OS locking as a future optional mode. Synthetic
  `Control+Command+Q` needs post-event permission and hands control to the
  secure login UI, which conflicts with the combo-unlock requirement.
- Private Login.framework APIs are rejected for phase 4. Revisit only if the
  user explicitly accepts private-API risk for a local-only feature.
- Freeze the logged-in GUI session only. It will not block the power button, lid
  close, Touch ID, secure login UI, FileVault/loginwindow, SSH, or remote admin.
- Freeze keyboard, mouse buttons, mouse motion, drag events, and scroll input.
  Keyboard-only capture is not enough for "laptop unusable."

## Implementation layout

Add a small Rust helper and keep the Bash CLI as the dispatcher:

```text
Cargo.toml
native/vigil-lock-helper/Cargo.toml
native/vigil-lock-helper/src/main.rs
native/vigil-lock-helper/src/macos.rs
native/vigil-lock-helper/src/combo.rs
```

Build/install flow:

- `vigil setup` and `vigil reload` build the helper on macOS with:
  `cargo build --release --manifest-path "$VIGIL_REPO_ROOT/native/vigil-lock-helper/Cargo.toml"`.
- Install the release binary to:
  `$VIGIL_INSTALL_DIR/bin/vigil-lock-helper`.
- Keep the installed path stable because TCC permission is tied to executable
  identity. Document that rebuilding a local unsigned helper can require
  re-granting permissions.
- Non-macOS setup should skip the helper build until phase 5.

Rust dependencies:

```toml
core-graphics = "0.25.0"
core-foundation = "0.10.1"
libc = "0.2"
```

Avoid extra CLI/JSON dependencies in phase 4. Use a tiny manual argument parser
and manual JSON formatting so the helper stays easy to audit.

## Helper CLI

Implement exactly:

```text
vigil-lock-helper --check-permissions --json
vigil-lock-helper --check-permissions --json --prompt
vigil-lock-helper --freeze --combo ctrl+alt+shift+cmd+l --max-secs 28800
vigil-lock-helper --debug-sleep-in-callback-ms 1000 --freeze --combo ... --max-secs ...
```

The debug flag is hidden from `vigil lock` and exists only to force an
event-tap timeout during manual testing.

Exit codes:

- `0`: unlocked by chord, or permission probe passed.
- `10`: unsupported platform.
- `20`: missing required permission or production tap smoke test failed.
- `30`: invalid combo or arguments.
- `40`: failed to create or enable event tap during freeze.
- `50`: watchdog timeout or tap could not be re-enabled.

Permission JSON must be stable and single-line:

```json
{"platform":"macos","listen_event_access":true,"accessibility_trusted":true,"post_event_access":false,"tap_create_active_session_ok":true}
```

For phase 4, `post_event_access` is informational only and must not block
`vigil lock`.

## macOS FFI

Use `core-graphics` for event taps, event types, event fields, flags, and key
codes. Add direct FFI for APIs that `core-graphics` does not expose as safe
wrappers.

Expected FFI declarations in `macos.rs`:

```rust
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
    fn CGEventTapIsEnabled(tap: core_foundation::mach_port::CFMachPortRef) -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(
        options: core_foundation::dictionary::CFDictionaryRef,
    ) -> bool;
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
}
```

If exact `core-foundation` type names differ during compile, use the matching
crate aliases, but keep the same C APIs.

AX prompt implementation:

- No-prompt doctor uses `AXIsProcessTrusted()`.
- Prompt doctor builds a CFDictionary containing
  `kAXTrustedCheckOptionPrompt: kCFBooleanTrue`, calls
  `AXIsProcessTrustedWithOptions`, and returns the immediate result.
- The CLI must still tell the user to rerun doctor after granting access.

## Production event mask

Use the same mask for doctor smoke test and freeze mode:

```text
KeyDown
KeyUp
FlagsChanged
LeftMouseDown
LeftMouseUp
RightMouseDown
RightMouseUp
MouseMoved
LeftMouseDragged
RightMouseDragged
ScrollWheel
OtherMouseDown
OtherMouseUp
OtherMouseDragged
```

Do not use `CGEventMaskForAllEvents` for the first implementation. Keep the
surface explicit so tests can reason about what Vigil drops.

## Doctor smoke test

`--check-permissions --json` must do more than read preflight flags:

1. Read `CGPreflightListenEventAccess`.
2. Read `AXIsProcessTrusted`.
3. Read `CGPreflightPostEventAccess` for informational reporting only.
4. Create an active session tap with the production event mask:
   `CGEventTapLocation::Session`, `HeadInsertEventTap`,
   `CGEventTapOptions::Default`.
5. Use a no-op callback that returns `CallbackResult::Keep`.
6. Add the tap to the current run loop long enough to call `enable()`.
7. Confirm `CGEventTapIsEnabled(tap.mach_port())` through the direct FFI.
8. Invalidate/drop the tap immediately.
9. Set `tap_create_active_session_ok=true` only if creation and enable both
   worked.

If the tap cannot be created or enabled, `vigil lock doctor` must fail even if a
preflight boolean looks acceptable.

## Freeze state machine

Create an active session event tap:

- Location: `CGEventTapLocation::Session`.
- Placement: `CGEventTapPlacement::HeadInsertEventTap`.
- Options: `CGEventTapOptions::Default`, not `ListenOnly`.
- Events: production event mask above.
- Run loop: current thread's `CFRunLoop`.

Shared state should be atomics plus small fixed-size data:

```text
UNLOCK_REQUESTED: AtomicBool
STOP_REQUESTED: AtomicBool
REENABLE_REQUESTED: AtomicBool
LAST_ERROR_CODE: AtomicI32
```

Callback behavior:

- For `TapDisabledByTimeout` or `TapDisabledByUserInput`:
  - set `REENABLE_REQUESTED=true`;
  - return `CallbackResult::Keep`;
  - do not call CoreFoundation/CoreGraphics from inside the callback.
- For keyboard events:
  - update modifier state from event flags and/or modifier keycodes;
  - on `KeyDown`, compare final keycode plus required modifiers;
  - if combo matches, set `UNLOCK_REQUESTED=true` and return
    `CallbackResult::Drop`.
- For all other production events while armed, return `CallbackResult::Drop`.
- The callback must not allocate, log, write files, spawn processes, sleep, or
  block. The hidden debug sleep flag is the only exception and must be guarded
  by an explicit runtime flag.

Main loop behavior:

- Add the tap source to `kCFRunLoopCommonModes`.
- Use a short CFRunLoop timeout/tick, or a CFRunLoop timer, so the main thread
  can inspect atomics.
- If `UNLOCK_REQUESTED`, remove source/invalidate/drop tap and exit `0`.
- If `STOP_REQUESTED`, remove source/invalidate/drop tap and exit `50`.
- If `max_secs` expires, set `STOP_REQUESTED`, cleanup, and exit `50`.
- If `REENABLE_REQUESTED`, clear it and call the helper's `event_tap.enable()`
  outside the callback. Then call `CGEventTapIsEnabled`.
- Retry re-enable at most three times with short spacing. If still disabled,
  cleanup and exit `50` fail-open.

Signal handling:

- Do not call CoreFoundation/CoreGraphics from signal handlers.
- Either use a minimal signal crate or simple libc handlers that only set an
  atomic flag.
- SIGINT/SIGTERM should set `STOP_REQUESTED=true`; the main loop does cleanup.
- Process crash/death is already fail-open because macOS drops the event tap.

## Combo parser

Default combo:

```text
ctrl+alt+shift+cmd+l
```

Parsing rules:

- Case-insensitive tokens separated by `+`.
- Trim whitespace around tokens.
- Accept aliases:
  - `control`, `ctrl`
  - `option`, `opt`, `alt`
  - `command`, `cmd`, `super`
  - `shift`
- Require at least three modifiers plus one non-modifier key.
- Reject combos using only modifier keys.
- Reject `escape` as the final key.
- Reject duplicate tokens.
- Reject unknown final keys with a clear error.
- Use physical key codes from `core_graphics::event::KeyCode`; document that
  letter keys are physical ANSI positions, not layout-translated characters.
- Minimum final-key support for phase 4:
  - `a` through `z`
  - `0` through `9`
  - `f1` through `f12`
  - `space`, `tab`, `return`

Expose parser functions for Rust unit tests:

```rust
pub struct Combo {
    pub required_flags: RequiredFlags,
    pub final_keycode: u16,
    pub canonical: String,
}

pub fn parse_combo(input: &str) -> Result<Combo, String>;
pub fn event_matches_combo(combo: &Combo, keycode: u16, flags: CGEventFlags) -> bool;
```

## Config

Add these config variables to `lib/common.sh`:

```bash
VIGIL_LOCK_COMBO="${VIGIL_LOCK_COMBO:-ctrl+alt+shift+cmd+l}"
VIGIL_LOCK_MAX_SECS="${VIGIL_LOCK_MAX_SECS:-28800}"
VIGIL_LOCK_HELPER="${VIGIL_LOCK_HELPER:-$VIGIL_INSTALL_DIR/bin/vigil-lock-helper}"
```

`VIGIL_LOCK_MAX_SECS=0` means no watchdog timeout. `vigil lock` may allow it
only when the user passes `--max-secs 0` explicitly; config alone should print a
warning and require an explicit CLI override.

## Bash CLI integration

In `bin/vigil`:

- Update header comment subcommand list.
- Add config variables by sourcing `lib/common.sh` as usual.
- Add helper path validation:
  - missing helper -> tell user to run `vigil setup` or `vigil reload`;
  - non-executable helper -> fail.
- Add `cmd_lock` dispatcher:
  - `vigil lock`
  - `vigil lock --combo <combo>`
  - `vigil lock --max-secs <seconds>`
  - `vigil lock --combo <combo> --max-secs <seconds>`
  - `vigil lock doctor`
  - `vigil lock doctor --prompt`
  - `vigil lock --help`
- Add `cmd_lock_doctor`:
  - invokes `"$VIGIL_LOCK_HELPER" --check-permissions --json`;
  - invokes `--prompt` only when requested;
  - parses the small JSON with shell pattern checks, not jq;
  - reports each field and exits non-zero if listen/accessibility/tap smoke
    test fail.
- `vigil lock` must call no-prompt doctor first and refuse to arm if it fails.
- Print before arming:
  - combo;
  - max seconds;
  - helper path;
  - `pkill -TERM vigil-lock-helper`;
  - "This is a local freeze guard, not the macOS Lock Screen."
- Add a three-second countdown before launching freeze mode.

In `cmd_sync_install`:

- On macOS (`uname -s` is `Darwin`), build and install the helper.
- Keep existing daemon/lib install behavior unchanged.
- Build failure should fail setup/reload with a clear error.

In `cmd_doctor`:

- Keep existing power/install doctor behavior.
- Add one non-fatal line:
  - installed lock helper path when present; or
  - "lock helper: missing (run vigil setup/reload if using phase 4)".
- Do not run the lock doctor from generic doctor; it may involve TCC prompts or
  privacy state the user did not ask to inspect.

## Implementation checklist for Codex Spark

Follow this order:

1. Add the Rust workspace files:
   - root `Cargo.toml` with `[workspace] members = ["native/vigil-lock-helper"]`;
   - helper `Cargo.toml` with the dependencies listed above;
   - `main.rs`, `combo.rs`, `macos.rs`.
2. Implement `combo.rs` first and add unit tests.
3. Implement helper argument parsing in `main.rs`.
4. Implement non-macOS stubs returning exit code `10`.
5. Implement macOS permission JSON and active session tap smoke test.
6. Implement freeze mode with active session tap, atomics, watchdog, signal
   handling, and re-enable retry state machine.
7. Run `cargo test --manifest-path native/vigil-lock-helper/Cargo.toml`.
8. Add Bash config defaults in `lib/common.sh`.
9. Add helper build/install to `cmd_sync_install`.
10. Add `vigil lock` and `vigil lock doctor` command handling.
11. Add focused shell tests for help/config/doctor failure paths. Do not require
    real TCC permission grants in automated tests.
12. Update README, ROADMAP, CHANGELOG.
13. Run:
    - `cargo test --manifest-path native/vigil-lock-helper/Cargo.toml`
    - `bash -n bin/vigil bin/vigil-daemon lib/*.sh`
    - `./tests/run.sh`
    - `git diff --check`

Do not run a real `vigil lock` freeze test automatically. Manual testing must
be explicit because it consumes user input.

## Automated tests

Shell tests:

- `vigil lock --help` and top-level help include the new command.
- `vigil lock doctor` handles missing helper.
- `vigil lock doctor` handles helper JSON with failed fields.
- `vigil lock` refuses to arm when doctor fails.
- `VIGIL_LOCK_COMBO`, `VIGIL_LOCK_MAX_SECS`, and `VIGIL_LOCK_HELPER` are
  sourced and passed to the helper.
- Unsupported OS path exits cleanly under a mocked `uname` where practical.

Rust tests:

- Combo parser accepts aliases and canonicalizes tokens.
- Parser rejects weak, duplicate, unknown, and ambiguous combos.
- `event_matches_combo` accepts required modifiers and final key.
- Event-type classification marks keyboard, mouse, drag, and scroll events as
  droppable.
- Permission JSON serialization is stable.

## Manual macOS verification

- Fresh install with no TCC grants: `vigil lock doctor` fails and names the
  exact permissions without arming.
- Optional reset on a test account only:
  `tccutil reset ListenEvent` and `tccutil reset Accessibility`.
- `vigil lock doctor --prompt` opens/requests the relevant permissions and
  tells the user to rerun doctor.
- Grant Input Monitoring/Accessibility to the installed helper path; restart
  the helper/terminal if macOS requires it; doctor passes.
- `vigil lock --max-secs 5` blocks input and then exits fail-open after five
  seconds.
- `vigil lock` blocks keyboard, trackpad click/move, mouse, drag, and scroll.
- Unlock combo exits and restores input immediately.
- `pkill -TERM vigil-lock-helper` from SSH or a second terminal restores input.
- Hidden debug timeout path:
  `vigil-lock-helper --debug-sleep-in-callback-ms 1000 --freeze ...` causes
  tap timeout; helper either re-enables or exits fail-open.
- Confirm the real macOS Lock Screen remains out of scope: after the OS lock
  screen is active, Vigil cannot combo-unlock it.
- Rebuild helper and confirm whether TCC permission must be re-granted; document
  observed behavior in README if it does.

## Documentation updates during implementation

- README: add `vigil lock`, `vigil lock doctor`, TCC setup, recovery command,
  and clear wording that this is a freeze guard rather than macOS password
  authentication.
- ROADMAP: mark phase 4 shipped after implementation and keep the active
  session-tap wording.
- CHANGELOG: record the helper, permissions, config keys, and macOS-only scope.
