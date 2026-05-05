# Phase 4 — Lock feature

> **Status: SKETCH ONLY.** Replace with a detailed plan before implementation.

## What

`vigil lock` enters "frozen laptop" mode: the laptop is unusable until a configured key combination is pressed. Useful when you walk away from a long agent run and don't want anyone else touching the machine.

## Direction

Native helper required — Bash can't grab keyboard input system-wide. **Rust** (not Swift) per the cross-OS preference.

- Helper: `bin/vigil-lock-helper` (Rust binary, macOS-only in this phase).
- macOS impl: `CGEventTap` (via `core-graphics` crate) for system-wide keyboard capture; consume all events except the configured unlock combo. On unlock combo: `NSWorkspace.shared.lockScreen()` (via `objc2`), then exit.
- Trigger: `vigil lock` (CLI subcommand) launches the helper; helper exits on unlock.
- Configurable combo via `~/.config/vigil/vigil.conf` — default `Ctrl+Shift+Alt+Cmd+L` or similar deliberately-awkward chord.

## Open questions

- Accessibility permission: CGEventTap requires the calling process to be granted Accessibility access in System Settings. Need to handle the first-run permission prompt gracefully.
- Should lock also dim/blank the display, or rely solely on `lockScreen()`? Probably the latter — that's what macOS already does well.
- What about Touch ID / biometric unlock? Should the helper allow biometric unlock as a backup, or strictly require the key combo? User decision.
- What's the failure mode if the helper crashes? Don't want a permanently locked laptop. Watchdog thread that auto-exits after N minutes if no key activity?

## Out of scope here (saved for phase 5)

- Linux / Windows lock implementations.

## When this phase begins

Replace this file with: exact crates, exact key-combo capture/filtering logic, exact failure-mode handling (watchdog, biometric fallback decision), test plan for accessibility-permission first-run.
