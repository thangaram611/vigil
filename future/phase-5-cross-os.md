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

## Logging & rotation

Phase 1 established the logging strategy that must carry forward unchanged into the Rust rewrite:

- **File = source of truth on all three OSes.** A flat append-only log file (one event per line) is the only abstraction macOS, Linux, and Windows share. The `vigil log [-f]` UX stays a single implementation across OSes. Per-OS structured sinks (`os_log` subsystem on macOS, `sd_journal` on Linux, ETW on Windows) are *additive* mirrors, never a replacement for the file. See the rationale captured in the phase 1 hardening plan (file logs survive longer than unified-log auto-pruning; `logger(1)` on macOS lacks `--subsystem`; native helpers for logging alone don't justify themselves; peer shell-daemon tooling like `hiddenest/awake` all uses files).
- **Per-OS rotation, native mechanisms.** Phase 1 ships `newsyslog.d` on macOS (the canonical Apple-side rotation drop-in, evaluated by `com.apple.newsyslog` ~hourly). The Rust port keeps this shape per-OS:
  - **macOS**: stay on `newsyslog.d` (install via `vigil setup`), or migrate to in-process `tracing-appender` if we want uniformity across OSes. Either keeps the file as source of truth.
  - **Linux**: drop-in `/etc/logrotate.d/vigil` (same shape as newsyslog.d: external rotator, log file held open by daemon but reopened by `log()` on each write — or use `tracing-appender`'s rolling-file appender to avoid the drop-in entirely).
  - **Windows**: in-process rotation via `tracing-appender` (no system-wide log rotator equivalent to logrotate). Optionally mirror to Event Log for IT-visible events while keeping a smaller file.
- **Rejected: single-sink unified logging.** Migrating phase 1 to macOS `os_log` as the sole sink would lock the design to one OS and force re-engineering the `vigil log` UX three times in phase 5. The decision to keep file-based was made on cross-OS correctness grounds, not on phase-1 effort grounds.

## When this phase begins

Replace this file with: exact crate dependencies, exact per-OS launcher mechanism, exact distribution targets, exact test matrix (CI on three runners).
