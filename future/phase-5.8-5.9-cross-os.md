# Phase 5.8 / 5.9 — Cross-OS port (Linux + Windows)

> **Status: PLANNING (forward-looking stub).** Sub-phases 5.1–5.7 of the Rust
> rewrite SHIPPED (macOS parity: CLI/output substrate, config/logging, detection
> core, thermal policy, the privileged power boundary, lock overlay, daemon +
> launchd service + unified CheckEngine). See git history + CHANGELOG + ROADMAP
> for that landed work. This file now carries ONLY the still-future cross-OS
> slices — **5.8 Linux** and **5.9 Windows** — which fill platform-trait impls
> behind the already-stable macOS seams.
>
> Per repo policy (ROADMAP "re-plan every deferred phase before implementing"),
> **each of 5.8 and 5.9 gets its own detailed implementation doc**, in the
> voice/rigor of the shipped per-slice docs, before any code is written. This
> stub is the map; that doc is the territory (exact struct/field layouts, the
> fixture→cargo-test mapping, the clap surface, the gate-0 golden-fixture step).

## The stable seams (do NOT refactor — fill non-macOS impls only)

The macOS slices froze these platform traits. Linux (5.8) and Windows (5.9) are
purely additive `#[cfg(target_os = ...)]` impls behind them — no seam changes.

| Seam (trait)         | Responsibility                              | Linux (5.8)                                                  | Windows (5.9)                                              |
|----------------------|---------------------------------------------|-------------------------------------------------------------|-----------------------------------------------------------|
| `PowerController`    | Hold/release system sleep prevention        | `keepawake` → logind `Manager.Inhibit('idle','block')` via `zbus` | `keepawake` → `SetThreadExecutionState(ES_CONTINUOUS\|ES_SYSTEM_REQUIRED)` |
| `CaffeinateAssertion`| Idle-sleep-only assertion (NOT display)     | logind idle inhibitor FD (Drop releases)                    | `SetThreadExecutionState` (no display flag)               |
| `ProcessScanner`     | Enumerate agent processes by name/exe       | `sysinfo` (/proc) + optional `procfs`                       | `sysinfo` (Windows backend)                               |
| `ActivityWatcher`    | Session-dir freshness + vscode semantic gate| `notify` (inotify)                                          | `notify` (ReadDirectoryChangesW)                          |
| `PowerGuard`         | Thermal + battery cutoff predicates         | sysfs thermal / UPower                                      | sysfs-equivalent / Win32 power APIs                       |
| `ServiceInstaller`   | Install/uninstall the resident service      | systemd user unit (`service-manager` lifecycle)             | Task Scheduler logon trigger **or** `windows-service` (decided on-device) |
| `LogRotation`        | Rotate the daemon log file                  | logrotate.d drop-in + `vigil reload-log` postrotate subcmd  | `logroller` in-process (1MB, keep 5, gzip), `cfg(windows)`|
| `Locker`             | Native lock action                          | X11/Wayland `_NET_WM_STATE_ABOVE` stub                      | `LockWorkStation` (interactive desktop only)              |
| `LockOverlay`        | Full-screen armed-state overlay window      | X11/Wayland topmost window                                  | `SetWindowPos HWND_TOPMOST` + GDI                         |

**Uniform invariants (already true on macOS, must hold on every OS):**
- Default holds prevent **system/idle** sleep only; never hold a display-awake
  assertion and never suppress the native OS lock screen.
- Privileged IPC stays the **file-based request/response queue**; file-ownership
  is the boundary on every platform. `getpeereid` (macOS) and `SO_PEERCRED`
  (Linux) are intentionally **unused** — do not migrate to a unix socket.
- All baseline/sleep-disabled parsers fail **safe** (corrupt/missing → release
  target = sleep-enabled), never abort the release.
- Test seams (`VIGIL_ROOT_HELPER_TESTING`, etc.) are compiled OUT of release.

---

## Phase 5.8 — Linux port

**Goal.** After macOS is shipped and stable, implement the Linux side of every
platform trait. Additive `#[cfg(target_os = "linux")]` impls; no seam refactor.

**Deliverables.**
- `PowerController`/`CaffeinateAssertion`: `keepawake` → logind
  `Manager.Inhibit('idle','block')` via `zbus` 5.x — **idle-only** for
  `caffeinate -i` parity: no sleep-inhibit, no screensaver/screen-locker
  inhibit, no `shutdown` inhibit. Keep the returned FD alive for the whole hold
  (Drop releases).
- `ProcessScanner`: `sysinfo` (/proc), `procfs` only if extra /proc data needed.
- `Locker`/`LockOverlay`: X11/Wayland topmost stub (`_NET_WM_STATE_ABOVE`).
- `ServiceInstaller`: `SystemdUserInstaller` (`service-manager` lifecycle;
  generate the `.service` unit content directly).
- `LogRotation`: logrotate.d drop-in + a `vigil reload-log` postrotate subcommand
  to reinit the NonBlocking writer.
- `PowerGuard`: Linux sysfs/UPower equivalents behind the same trait.
- Privilege boundary: file-based queue ported as-is, Linux canonical paths.

**UX.** setup installs the systemd user unit + logrotate drop-in with the same
colored checklist; doctor reports Linux readiness (logind reachable, systemd
user manager present). Same CLI/output substrate, Linux-accurate messages.

**Crates.** keepawake 0.6.0, zbus 5.x, sysinfo 0.39.x, procfs 0.18 (optional),
service-manager 0.11.x, notify (inotify).

**Tests.** Cargo integration tests on Linux CI covering detection, activity,
refcount, thermal/battery via fixtures, IPC validation, plus trait-contract
tests shared with macOS. Cannot verify on the available Mac.

**Risks.** No Linux hardware locally (relies on CI). Non-systemd Linux
(OpenRC/runit) needs an explicit "unsupported" error path. logind
block-inhibitor semantics changed in systemd 257. Watch the `zbus`/`keepawake`
version diamond (`cargo tree -d`). The one subtle correctness point is the
logind inhibitor lifecycle (FD-held-for-duration, Drop-releases).

---

## Phase 5.9 — Windows port

**Goal.** Final trailing slice: Windows impls of every platform trait, additive
`#[cfg(target_os = "windows")]` behind the stable seams.

**Deliverables.**
- `PowerController`/`CaffeinateAssertion`: `keepawake` →
  `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` ONLY (no
  `ES_DISPLAY_REQUIRED`, no `ES_AWAYMODE_REQUIRED` by default).
- `Locker`: `LockWorkStation` via `windows` crate (interactive desktop only).
- `ProcessScanner`: `sysinfo` (Windows backend).
- `LogRotation`: `logroller` as a `MakeWriter` (1MB, keep 5, gzip), `cfg(windows)`
  — Windows has no newsyslog/logrotate. The `MakeWriter` seam keeps the
  single-maintainer crate swappable; pin in `Cargo.lock`.
- `ServiceInstaller`: Task Scheduler logon trigger (per-user, avoids UAC —
  recommended) **or** `windows-service` real service. **OPEN DECISION, settled
  on-device** when a Windows test machine exists; the seam accommodates both.
- `LockOverlay`: `SetWindowPos HWND_TOPMOST` + GDI.
- Privilege boundary: file-based queue retained; document the Windows ACL
  equivalents of the request/response dir ownership matrix (the macOS uid checks
  map to SID/owner checks). **This POSIX-uid→SID/ACL translation is the one
  genuinely non-mechanical part — resource it like a mini privilege slice.**

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
