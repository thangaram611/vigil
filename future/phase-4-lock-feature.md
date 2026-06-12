# Phase 4 — Lock feature

> **Status: RESEARCH-CORRECTED SKETCH.** Replace with a detailed plan before
> implementation. The prior `NSWorkspace.lockScreen()` assumption is not a
> confirmed public binding/API surface in the current Rust/AppKit stack.

## What

`vigil lock` enters "frozen laptop" mode: the laptop is unusable until a configured key combination is pressed. Useful when you walk away from a long agent run and don't want anyone else touching the machine.

## Direction

Native helper required — Bash can't grab keyboard input system-wide. **Rust** (not Swift) per the cross-OS preference.

- Helper: `bin/vigil-lock-helper` (Rust binary, macOS-only in this phase).
- macOS input impl: `CGEventTap` (via `core-graphics`) for system-wide keyboard capture; consume all keyboard events except the configured unlock combo. Use a normal, non-listen-only tap so dropped events are actually suppressed.
- macOS lock impl: **must be re-selected during detailed planning.** Current candidates:
  - invoke an Apple-supported lock mechanism via shell/system command if one is stable enough;
  - synthesize the system lock shortcut after the unlock combo, if event-tap ownership and TCC make that reliable;
  - use private API only if this remains local-only and the risk is accepted explicitly.
- Trigger: `vigil lock` (CLI subcommand) launches the helper; helper exits on unlock.
- Configurable combo via `~/.config/vigil/vigil.conf` — default `Ctrl+Shift+Alt+Cmd+L` or similar deliberately-awkward chord.

## Open questions

- Permissions: event taps can require Input Monitoring and/or Accessibility consent. Add `vigil lock doctor` before the helper can freeze input. If permissions are missing, print exact System Settings guidance and exit without installing the event tap.
- TCC identity: prompts are tied to the executable identity. A stable signed/bundled helper is preferable to repeatedly changing dev binaries.
- Should lock also dim/blank the display, or rely solely on `lockScreen()`? Probably the latter — that's what macOS already does well.
- What about Touch ID / biometric unlock? Should the helper allow biometric unlock as a backup, or strictly require the key combo? User decision.
- What's the failure mode if the helper crashes? Don't want a permanently locked laptop. Watchdog thread that auto-exits after N minutes if no key activity?
- Event-tap lifecycle: callbacks must stay fast and must handle disabled taps by re-enabling or failing open.

## Out of scope here (saved for phase 5)

- Linux / Windows lock implementations.

## When this phase begins

Replace this file with: exact crates, exact key-combo capture/filtering logic,
the selected public/private lock mechanism and its proof, exact failure-mode
handling (watchdog, biometric fallback decision), TCC first-run UX, and a test
plan for permission denial, helper crash, and event-tap disable/re-enable.
