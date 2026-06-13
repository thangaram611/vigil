# macOS lock modes and locked use

## Current `vigil lock`

`vigil lock` is a local freeze guard for the active logged-in GUI session. It
creates an active `kCGHIDEventTap` through CoreGraphics, registers itself
with Vigil's daemon refcount, and drops keyboard, mouse, drag, and scroll events
until the configured unlock chord is pressed or a watchdog timeout expires.

While the guard is armed, the daemon uses the same best-effort hold as agent
runs: `pmset disablesleep=1` plus `caffeinate -i`. This prevents system sleep
as strongly as macOS allows, including best-effort lid-close behavior, but it
does not hold a display assertion. The display may sleep and macOS may activate
the native Lock Screen according to the user's normal settings.

This is intentionally not the macOS Lock Screen. It cannot protect FileVault or
loginwindow, it cannot override the power button or lid close, and it cannot
combo-unlock the secure login UI.

While CoreGraphics reports `CGSSessionScreenIsLocked`, the helper passes events
through instead of dropping them. This keeps the macOS password/Touch ID flow
usable if the native Lock Screen appears before the Vigil combo is entered. Once
the user returns to the desktop session, the local freeze remains in force until
the configured combo is pressed.

## Run-loop requirement

The event tap's `CFMachPort` source may be added to `kCFRunLoopCommonModes`, but
the run loop itself must run in a concrete mode such as
`kCFRunLoopDefaultMode`. Passing `kCFRunLoopCommonModes` to
`CFRunLoopRunInMode`/`CFRunLoopRunSpecific` is invalid on macOS and produces:

```text
invalid mode 'kCFRunLoopCommonModes' provided to CFRunLoopRunSpecific
```

Vigil schedules the event tap source in common modes, then runs the helper loop
in default mode. This keeps the source broadly scheduled while avoiding the
CoreFoundation invalid-mode sentinel.

Relevant Apple docs:

- [Run Loops](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/RunLoopManagement/RunLoopManagement.html)
- [CFRunLoopRunInMode](https://developer.apple.com/documentation/corefoundation/cfrunloopruninmode%28_%3A_%3A_%3A%29)
- [Quartz Event Services](https://developer.apple.com/documentation/coregraphics/quartz-event-services)

## Event-tap boundary

Apple's SDK headers define three tap locations: HID, session, and annotated
session. Vigil uses a HID tap so the guard filters events at the earliest
CoreGraphics point available to a privacy-granted user helper:

```text
CGEventTapLocation::HID
CGEventTapPlacement::HeadInsertEventTap
CGEventTapOptions::Default
```

The helper still needs macOS privacy grants for the installed helper binary:

- Input Monitoring / listen event access
- Accessibility / assistive access

`vigil lock doctor` verifies those grants and also creates/enables a short-lived
production-shaped tap. Do not treat boolean privacy preflights alone as proof
that the active filter path is working.

For lock-screen pass-through, Vigil reads
[`CGSessionCopyCurrentDictionary()`](https://developer.apple.com/documentation/coregraphics/cgsessioncopycurrentdictionary%28%29)
and checks the `CGSSessionScreenIsLocked` boolean key when present. The public
SDK documents the session dictionary and the console/login keys; the locked
boolean is an observed CoreGraphics key, so the helper treats a missing key as
"not locked" rather than failing closed.

## What Codex-style locked use is doing

Codex's locked computer use is not just an event tap. The public Codex manual
describes it as a macOS-only feature that installs an Apple authorization
plug-in participating in the unlock flow. During an active, trusted computer-use
turn, Codex temporarily unlocks the Mac, blocks local use, covers every display,
and relocks or pauses automatic unlock when local keyboard or pointer input is
detected.

That model is deliberately narrow:

- It is not a general remote unlock path.
- It does not let arbitrary apps or local processes unlock the Mac.
- It is scoped to a short-lived trusted operation.
- It needs a privileged/security-reviewed component, not just a foreground CLI
  event tap.

Reference:

- [Codex Computer Use: Locked use](https://developers.openai.com/codex/app/computer-use#locked-use)
- [Apple Authorization Plug-ins](https://developer.apple.com/documentation/security/authorization-plug-ins)

## Possible future Vigil mode

A real `vigil locked-use` feature should be separate from `vigil lock` and
opt-in. A credible design would need:

- A signed installer path for an authorization plug-in or equivalent
  Apple-supported loginwindow integration.
- A small privileged component installed through launchd, with a narrow IPC
  protocol and root-owned files.
- A trusted-operation gate: never unlock merely because a local process asks.
- Full-screen local-use shielding while the desktop is temporarily unlocked.
- Immediate relock on untrusted local keyboard/pointer input.
- Clear admin uninstall and recovery paths.
- A security review before default release.

Until that exists, `vigil lock` should stay a local active-session freeze guard.
