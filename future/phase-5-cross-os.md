# Phase 5 — Cross-OS port

> **Status: SKETCH ONLY.** Replace with a detailed plan before implementation.

## What

Full **Rust** rewrite of the daemon and helpers, supporting Linux and Windows alongside macOS. Bash phase 1 stays the macOS reference implementation.

## Direction

Per-platform sleep prevention (mirrors `keepawake-rs` architecture):

- **macOS**: IOKit `IOPMAssertionCreateWithName` for idle/display assertions. `IOPMSetSystemPowerSetting(kIOPMSleepDisabledKey, true)` for closed-lid (same lever as phase 1's `pmset disablesleep`).
- **Linux**: D-Bus to `org.freedesktop.login1.Manager.Inhibit` (`sleep:idle:shutdown` + `block` mode) for sleep prevention. `org.freedesktop.ScreenSaver.Lock` for the lock feature.
- **Windows**: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)` for sleep prevention. `LockWorkStation()` for lock. **Note: `ES_AWAYMODE_REQUIRED` doesn't work on Windows with modern standby; document this caveat.**

Process detection per-platform:

- **macOS**: keep `ps` for now, or migrate to `sysinfo` crate for portability.
- **Linux**: `/proc` directly, or `procfs` crate.
- **Windows**: `EnumProcesses` + `GetModuleFileNameEx` via the `windows` crate.

Likely depend on or fork [`segevfiner/keepawake-rs`](https://github.com/segevfiner/keepawake-rs) rather than re-implementing the per-OS sleep abstractions from scratch.

## Open questions

- Stay as a daemon, or refactor into a single long-running CLI process? Daemon is more familiar; single-process is simpler on Windows where launchd-equivalent is `Task Scheduler` and is awkward.
- Cross-platform launch infrastructure: launchd (macOS), systemd-user-units (Linux), Task Scheduler (Windows). Three install paths. Worth the complexity?
- Distribution: `cargo install vigil` for everyone? Plus Homebrew (macOS), `apt`/`dnf` (Linux — probably skip and tell users `cargo install`), Scoop/Winget (Windows)?
- Single binary or multi-binary? Probably one `vigil` binary with subcommands (`vigil daemon`, `vigil run`, `vigil lock`, etc.).

## When this phase begins

Replace this file with: exact crate dependencies, exact per-OS launcher mechanism, exact distribution targets, exact test matrix (CI on three runners).
