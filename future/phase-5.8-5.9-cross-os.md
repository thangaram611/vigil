# Phase 5.8 / 5.9 — Cross-OS port (Linux + Windows)

> **Status: 5.8 IMPLEMENTATION STARTED (2026-06-19).**
> Sub-phases 5.1–5.7 of the Rust rewrite SHIPPED (macOS parity: CLI/output
> substrate, config/logging, detection core, thermal policy, the privileged
> power boundary, lock overlay, daemon + launchd service + unified CheckEngine).
> See git history + CHANGELOG + ROADMAP for that landed work. This file now
> carries ONLY the still-future cross-OS slices — **5.8 Linux** and **5.9
> Windows**. Gate 0/Gate 1/Gate 2 have landed the Linux compile baseline,
> platform power facade, and logind `idle:sleep` hold. Linux battery/thermal
> collectors plus focused power status/doctor text also landed; remaining Linux
> work is service install, log rotation, packaging docs, and on-device validation.
>
> Per repo policy (ROADMAP "re-plan every deferred phase before implementing"),
> **each of 5.8 and 5.9 gets its own detailed implementation doc**, in the
> voice/rigor of the shipped per-slice docs, before any code is written. The
> Linux doc is now [`phase-5.8-linux.md`](./phase-5.8-linux.md); this stub remains
> the cross-OS map and the Windows holding pen.

## 2026-06-19 research verdict

**Green signal:** pursue **5.8 Linux first**, then **5.9 Windows**. The platform
primitives are stable enough to plan against, and this work advances Vigil's
release gate more than another macOS-only agent detector.

Why this beats the Antigravity/Gemini detector candidate right now:

- Antigravity CLI is newly overhauled and cannot be responsibly supported from
  web docs alone; we need a fresh local install plus captured process/session
  artifacts before shipping a detector.
- Cross-OS is already the repo's release-blocking work, and the current Rust
  rewrite intentionally left platform seams for this exact slice.
- Linux/Windows sleep APIs are mature and testable through OS-specific CI plus
  small manual checks, while an emerging agent surface would be a moving target.

Primary sources checked:

- systemd inhibitor locks and logind D-Bus `Inhibit`: `idle` is the automatic
  idle logic; the D-Bus method returns an fd, and closing the fd releases the
  lock.
  https://systemd.io/INHIBITOR_LOCKS/
  https://manpages.ubuntu.com/manpages/noble/man5/org.freedesktop.login1.5.html
  https://man7.org/linux/man-pages/man1/systemd-inhibit.1.html
- Windows `SetThreadExecutionState`: `ES_CONTINUOUS` keeps the request active,
  `ES_SYSTEM_REQUIRED` keeps the system working, `ES_DISPLAY_REQUIRED` keeps the
  display on, and away mode is explicitly media-only. This matches Vigil's
  "system awake, display may sleep" invariant.
  https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate
  https://learn.microsoft.com/en-us/windows/win32/power/system-sleep-criteria
- Windows Task Scheduler supports `ONLOGON`, which is the preferred per-user
  resident-start shape unless on-device testing proves a service is required.
  https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/schtasks-create
- Linux battery/thermal probes have primary paths: UPower exposes `OnBattery`
  and display-device percentage over the system bus; Linux thermal sysfs exposes
  thermal zone `temp` and trip-point files.
  https://upower.freedesktop.org/docs/UPower.html
  https://docs.kernel.org/driver-api/thermal/sysfs-api.html

Current crate check from `cargo search` on 2026-06-19:

- `keepawake = 0.6.0`
- `zbus = 5.16.0`
- `logind-zbus = 5.3.2`
- `service-manager = 0.11.0`
- `logroller = 0.1.10`
- `windows-service = 0.8.1`

## Platform Work Map

The macOS slices froze several platform seams, but the live code still lacks the
`PowerController` facade named below. Phase 5.8 starts by adding that facade and
wrapping the current macOS `PowerMachine`; after that, Linux/Windows work should
stay additive behind the platform boundary.

| Seam (trait)         | Responsibility                              | Linux (5.8)                                                  | Windows (5.9)                                              |
|----------------------|---------------------------------------------|-------------------------------------------------------------|-----------------------------------------------------------|
| `PowerController`    | Hold/release system sleep prevention        | logind `Inhibit` via `zbus`/`logind-zbus`; test `idle`, `sleep`, and `idle:sleep` before choosing default | `SetThreadExecutionState(ES_CONTINUOUS\|ES_SYSTEM_REQUIRED)` |
| `CaffeinateAssertion`| Idle-sleep-only assertion (NOT display)     | logind inhibitor FD (Drop/close releases)                   | `SetThreadExecutionState` (no display flag)               |
| `ProcessScanner`     | Enumerate agent processes by name/exe       | `sysinfo` (/proc) + optional `procfs`                       | `sysinfo` (Windows backend)                               |
| `ActivityWatcher`    | Session-dir freshness + vscode semantic gate| `notify` (inotify)                                          | `notify` (ReadDirectoryChangesW)                          |
| `PowerGuard`         | Thermal + battery cutoff predicates         | sysfs thermal / UPower                                      | sysfs-equivalent / Win32 power APIs                       |
| `ServiceInstaller`   | Install/uninstall the resident service      | systemd user unit (`service-manager` lifecycle)             | Task Scheduler logon trigger **or** `windows-service` (decided on-device) |
| `LogRotation`        | Rotate the daemon log file                  | logrotate.d drop-in + `vigil reload-log` postrotate subcmd  | `logroller` in-process (1MB, keep 5, gzip), `cfg(windows)`|
| `Locker`             | Native lock action                          | deferred in 5.8; command returns explicit unsupported error | `LockWorkStation` (interactive desktop only)              |
| `LockOverlay`        | Full-screen armed-state overlay window      | deferred in 5.8                                             | `SetWindowPos HWND_TOPMOST` + GDI                         |

**Uniform invariants (already true on macOS, must hold on every OS):**
- Default holds prevent **system/idle** sleep only; never hold a display-awake
  assertion and never suppress the native OS lock screen.
- Use the narrowest native authority boundary. macOS keeps the root helper
  because `pmset disablesleep` is privileged. Linux logind inhibitors and
  Windows execution-state calls are user-scoped, so do not introduce a
  root/admin helper unless a detailed plan proves one is necessary.
  File/owner/ACL hardening remains mandatory for state, IPC, and service
  artifacts.
- All baseline/sleep-disabled parsers fail **safe** (corrupt/missing → release
  target = sleep-enabled), never abort the release.
- Test seams (`VIGIL_ROOT_HELPER_TESTING`, etc.) are compiled OUT of release.

---

## Phase 5.8 — Linux port

Detailed implementation plan: [`phase-5.8-linux.md`](./phase-5.8-linux.md).

**Goal.** Implement the Linux side of every platform seam as the next detailed
plan. Additive `#[cfg(target_os = "linux")]` impls; no unrelated macOS refactor.

**Deliverables.**
- `PowerController`/`CaffeinateAssertion`: logind
  `Manager.Inhibit(..., "vigil", "...", "block")` via `zbus` 5.x or the typed
  `logind-zbus` wrapper. Test `idle`, `sleep`, and `idle:sleep` on-device before
  choosing the default. Keep returned fds alive for the whole hold. No shutdown,
  key, display, or screen-locker inhibition by default.
- `ProcessScanner`: `sysinfo` (/proc), `procfs` only if extra /proc data needed.
- `Locker`/`LockOverlay`: explicitly unsupported in 5.8 unless a later Linux
  GUI/input plan proves an implementation across Wayland and X11.
- `ServiceInstaller`: `SystemdUserInstaller` (`service-manager` lifecycle;
  generate the `.service` unit content directly).
- `LogRotation`: logrotate.d drop-in + a `vigil reload-log` postrotate subcommand
  to reinit the NonBlocking writer.
- `PowerGuard`: Linux UPower battery plus sysfs thermal collectors behind the
  same trait.
- Privilege boundary: no root helper by default. The daemon owns the logind fd
  in the user session; state paths still use strict ownership/mode validation.

**UX.** setup installs the systemd user unit + logrotate drop-in with the same
colored checklist; doctor reports Linux readiness (logind reachable, systemd
user manager present). Same CLI/output substrate, Linux-accurate messages.

**Crates.** keepawake 0.6.0 (optional reference), zbus 5.16.x or
logind-zbus 5.3.x, sysinfo 0.39.x, procfs 0.18 (optional),
service-manager 0.11.x, notify 8.2.x stable (do not chase 9.x release
candidates unless required).

**Tests.** Cargo integration tests on Linux CI covering detection, activity,
refcount, thermal/battery via fixtures, IPC validation, plus trait-contract
tests shared with macOS. Cannot verify on the available Mac.

**Risks.** No Linux hardware locally (relies on CI unless a Linux VM is added).
Non-systemd Linux (OpenRC/runit) needs an explicit "unsupported" error path.
The one subtle correctness point is the logind inhibitor lifecycle
(FD-held-for-duration, Drop/close releases). Watch the `zbus`/`logind-zbus`
version surface (`cargo tree -d`).

---

## Phase 5.9 — Windows port

**Goal.** Final trailing slice: Windows impls of every platform trait, additive
`#[cfg(target_os = "windows")]` behind the platform boundary after the 5.8 power
facade lands.

**Deliverables.**
- `PowerController`/`CaffeinateAssertion`:
  `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` ONLY (no
  `ES_DISPLAY_REQUIRED`, no `ES_AWAYMODE_REQUIRED` by default). Clear with
  `SetThreadExecutionState(ES_CONTINUOUS)`.
- `Locker`: `LockWorkStation` via `windows` crate (interactive desktop only).
- `ProcessScanner`: `sysinfo` (Windows backend).
- `LogRotation`: `logroller` as a `MakeWriter` (1MB, keep 5, gzip), `cfg(windows)`
  — Windows has no newsyslog/logrotate. The `MakeWriter` seam keeps the
  single-maintainer crate swappable; pin in `Cargo.lock`.
- `ServiceInstaller`: Task Scheduler logon trigger (per-user, avoids UAC —
  recommended) **or** `windows-service` real service. **OPEN DECISION, settled
  on-device** when a Windows test machine exists; Task Scheduler is the starting
  assumption because the native power call is user-scoped and the lock action is
  interactive-desktop-bound.
- `LockOverlay`: `SetWindowPos HWND_TOPMOST` + GDI.
- Privilege boundary: no admin service by default. Document Windows SID/owner/ACL
  equivalents for Vigil state, logs, and task artifacts. Add an elevated service
  only if the detailed plan proves Task Scheduler cannot satisfy startup,
  shutdown, and interactive-lock constraints.

**UX.** setup registers the Task Scheduler logon task (or service) with the same
colored checklist; doctor reports Windows readiness; overlay renders topmost.

**Crates.** keepawake 0.6.0, windows (Win32_System_Power,
Win32_UI_WindowsAndMessaging), sysinfo 0.39.x, logroller (cfg windows),
windows-service (only if the real-service path is chosen).

**Tests.** Cargo integration tests on Windows CI covering detection, refcount,
IPC validation, and log rotation. Overlay + `LockWorkStation` verified manually
on-device. Cannot verify on the available Mac.

**Risks.** No Windows hardware (CI + future on-device). `SetThreadExecutionState`
cannot block user-initiated sleep/lid-close — document the weaker guarantee.
`LockWorkStation` fails from a service context (argues for Task Scheduler).

---

## Release gate (unchanged)

Per ROADMAP: **no version tag / GitHub release / Homebrew tap until every phase
ships and stabilizes**, then v1.0.0. cargo-dist + Homebrew tap may be configured
in `Cargo.toml` metadata but stay inactive until this gate lifts.
