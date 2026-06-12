# Phase 4 - Lock feature

> **Status: DETAILED PLAN.** Ready for implementation after review.

## Goal

Add `vigil lock`, a local "freeze guard" for the active macOS GUI session. When
armed, Vigil consumes user input until the configured unlock chord is pressed.
This protects a long-running agent session from accidental or casual local input
while the machine stays awake.

This is not a replacement for macOS authentication. If the user wants the real
macOS Lock Screen, they should use the OS lock flow and unlock with password,
Touch ID, or Watch unlock. A user-space event tap should not be treated as
capable of unlocking the secure login UI.

## Confirmed platform facts

Validated on 2026-06-12 against the local Command Line Tools SDK and current
Rust crate metadata:

- `AppKit.framework/Headers/NSWorkspace.h` contains no public
  `lockScreen` symbol. Do not implement the old `NSWorkspace.lockScreen`
  sketch.
- `CoreGraphics.framework/Headers/CGSession.h` exposes
  `CGSessionCopyCurrentDictionary` and session-notification constants only; it
  does not expose a public lock call.
- The legacy executable
  `/System/Library/CoreServices/Menu Extras/User.menu/Contents/Resources/CGSession`
  is not present on this machine. Do not hard-code it as the lock path.
- `CoreGraphics.framework/Headers/CGEvent.h` exposes `CGEventTapCreate`,
  `CGEventTapEnable`, `CGEventTapIsEnabled`,
  `CGPreflightListenEventAccess`, `CGRequestListenEventAccess`,
  `CGPreflightPostEventAccess`, and `CGRequestPostEventAccess`.
- `ApplicationServices.framework/.../AXUIElement.h` exposes
  `AXIsProcessTrustedWithOptions` and `kAXTrustedCheckOptionPrompt`.
- Latest crate choices checked by `cargo search`:
  `core-graphics = "0.25.0"` and `core-foundation = "0.10.1"`.

## Product shape

Ship two user-facing commands:

```text
vigil lock
vigil lock doctor
```

`vigil lock`:

- Requires macOS for this phase. On other OSes, exit with a clear phase-5
  message.
- Refuses to arm if `vigil lock doctor` would fail. Never install a best-effort
  event tap when required permissions are missing.
- Prints the selected unlock chord, a short arming countdown, and recovery
  instructions before input is consumed.
- Starts the Rust helper in the foreground and returns when the helper exits.

`vigil lock doctor`:

- Checks helper existence and executable bit at
  `$VIGIL_INSTALL_DIR/bin/vigil-lock-helper`.
- Runs the helper's permission probe and prints:
  - Input Monitoring/listen-event access status.
  - Accessibility trust status.
  - Post-event access status if optional system-lock support is enabled later.
- Prints exact System Settings guidance:
  `System Settings > Privacy & Security > Input Monitoring` and
  `System Settings > Privacy & Security > Accessibility`.

## Scope decisions

- Implement the custom freeze guard first. Do not trigger the macOS Lock Screen
  in the default phase-4 path.
- Treat real OS locking as a future optional mode, not as phase-4 baseline.
  Synthetic `Control+Command+Q` needs event posting permission and hands control
  to the secure login UI, which conflicts with the combo-unlock requirement.
- Private Login.framework APIs are rejected for phase 4. Revisit only if the
  user explicitly accepts private-API risk for a local-only feature.
- Freeze keyboard, mouse buttons, mouse motion, and scroll input. Keyboard-only
  capture is not enough for "laptop unusable."

## Implementation layout

Add a small Rust helper and keep the Bash CLI as the dispatcher:

```text
Cargo.toml
native/vigil-lock-helper/Cargo.toml
native/vigil-lock-helper/src/main.rs
native/vigil-lock-helper/src/macos.rs
```

Build/install flow:

- `vigil setup` and `vigil reload` build the helper on macOS with:
  `cargo build --release --manifest-path "$VIGIL_REPO_ROOT/native/vigil-lock-helper/Cargo.toml"`.
- Install the release binary to:
  `$VIGIL_INSTALL_DIR/bin/vigil-lock-helper`.
- Keep the installed path stable because TCC permission is tied to executable
  identity. Document that rebuilding a local unsigned helper can require
  re-granting permissions.

Rust dependencies:

```toml
core-graphics = "0.25.0"
core-foundation = "0.10.1"
libc = "0.2"
```

Use `core-graphics` for `CGEventTap`, `CallbackResult::Drop`, event fields,
and key-code constants. Add direct `extern "C"` declarations for
`CGPreflightListenEventAccess`, `CGRequestListenEventAccess`, and
`AXIsProcessTrustedWithOptions` if the selected crate surface does not expose
them directly.

## Helper interface

The helper should be non-interactive except for the captured input stream:

```text
vigil-lock-helper --check-permissions --json
vigil-lock-helper --freeze --combo ctrl+alt+shift+cmd+l --max-secs 28800
```

Exit codes:

- `0`: unlocked by chord or permission probe passed.
- `10`: unsupported platform.
- `20`: missing required permission.
- `30`: invalid combo or arguments.
- `40`: failed to create or enable event tap.
- `50`: watchdog timeout or tap could not be re-enabled.

The JSON permission probe should be small and stable:

```json
{
  "platform": "macos",
  "listen_event_access": true,
  "accessibility_trusted": true,
  "post_event_access": false
}
```

## Event-tap behavior

- Create an active HID event tap:
  - Location: `CGEventTapLocation::HID`.
  - Placement: `HeadInsertEventTap`.
  - Options: `CGEventTapOptions::Default`, not `ListenOnly`.
- Run the tap on the current thread's CFRunLoop.
- Consume all configured input events with `CallbackResult::Drop`.
- Keep the callback fast: update atomic state only; no logging, allocation, disk
  I/O, process spawning, or blocking calls inside the callback.
- Track modifier state from `FlagsChanged`, `KeyDown`, and `KeyUp` events.
- Detect the unlock chord on `KeyDown` for the configured final key while all
  required modifiers are active.
- On unlock, atomically mark the run loop for shutdown, drop the unlock key
  event, remove the run-loop source, invalidate the tap, and exit `0`.
- Handle `kCGEventTapDisabledByTimeout` and `kCGEventTapDisabledByUserInput` by
  scheduling one re-enable attempt outside the callback. If re-enable fails,
  fail open by exiting non-zero.

Default combo:

```text
ctrl+alt+shift+cmd+l
```

Parsing rules:

- Case-insensitive tokens separated by `+`.
- Accept aliases: `control/ctrl`, `option/opt/alt`, `command/cmd/super`,
  `shift`.
- Require at least three modifiers plus one non-modifier key.
- Reject combos using only modifier keys, `escape`, or single-letter shortcuts
  without enough modifiers.
- Use physical key codes from `core_graphics::event::KeyCode`; document that
  letter keys are physical ANSI positions, not layout-translated characters.

## Config

Add these config variables to `lib/common.sh` after phase-4 implementation:

```bash
VIGIL_LOCK_COMBO="${VIGIL_LOCK_COMBO:-ctrl+alt+shift+cmd+l}"
VIGIL_LOCK_MAX_SECS="${VIGIL_LOCK_MAX_SECS:-28800}"
VIGIL_LOCK_HELPER="${VIGIL_LOCK_HELPER:-$VIGIL_INSTALL_DIR/bin/vigil-lock-helper}"
```

`VIGIL_LOCK_MAX_SECS=0` means no watchdog timeout and should require an explicit
warning from `vigil lock`.

## Safety and recovery

- Default max runtime: 8 hours. This prevents a forgotten combo from becoming a
  permanent local input block.
- Before arming, print:
  - selected combo;
  - max runtime;
  - installed helper path;
  - recovery command from another terminal/SSH session:
    `pkill -TERM vigil-lock-helper`.
- Handle SIGINT/SIGTERM by invalidating the tap and exiting.
- If the helper crashes, macOS drops the event tap and input fails open.
- If the event tap is disabled and cannot be re-enabled, exit fail-open and tell
  the CLI to print a diagnostic.
- Do not run the helper under launchd in phase 4. Foreground ownership is easier
  to reason about and avoids a hidden process consuming input.

## CLI integration plan

In `bin/vigil`:

- Update the subcommand list and help text.
- Add `cmd_lock` with nested dispatch:
  - `vigil lock`
  - `vigil lock doctor`
  - `vigil lock --combo <combo>`
  - `vigil lock --max-secs <seconds>`
- Add `cmd_lock_doctor` that calls the helper probe and formats human-readable
  output.
- Keep `vigil doctor` unchanged except for one summary line that points to
  `vigil lock doctor` when the helper is installed.
- Update `cmd_sync_install` to build/install the helper on macOS. Non-macOS
  setup should skip helper build until phase 5.

## Tests

Shell tests:

- `vigil lock --help` and top-level help include the new command.
- `vigil lock doctor` handles missing helper, bad JSON, and failed permission
  probe.
- Config variables are sourced and passed to the helper.
- Unsupported OS path exits cleanly under a mocked `uname`.

Rust tests:

- Combo parser accepts aliases and canonicalizes tokens.
- Parser rejects weak or ambiguous combos.
- Event-type classification marks keyboard, mouse, and scroll events as
  droppable.
- Permission JSON serialization is stable.

Manual macOS verification:

- Fresh install with no TCC grants: `vigil lock doctor` fails and names the
  exact permissions without arming.
- Grant Input Monitoring/Accessibility to the installed helper path; doctor
  passes.
- `vigil lock` blocks keyboard, trackpad click/move, mouse, and scroll.
- Unlock combo exits and restores input immediately.
- SIGTERM from another terminal restores input.
- Force an event-tap timeout/disable path if practical; helper either
  re-enables or exits fail-open.
- Rebuild helper and confirm whether TCC permission must be re-granted; document
  observed behavior in README if it does.

## Documentation updates during implementation

- README: add `vigil lock`, `vigil lock doctor`, TCC setup, recovery command,
  and clear wording that this is a freeze guard rather than macOS password
  authentication.
- ROADMAP: mark phase 4 shipped after implementation and remove the old
  `NSWorkspace.lockScreen` wording.
- CHANGELOG: record the helper, permissions, config keys, and macOS-only scope.
