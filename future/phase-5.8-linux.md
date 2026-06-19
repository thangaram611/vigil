# Phase 5.8 -- Linux Port Detailed Implementation Plan

> Status: IMPLEMENTATION STARTED (2026-06-19). Gate 0/Gate 1/Gate 2 are complete
> in the working tree: Linux now builds, the daemon has a platform power facade,
> and Linux uses a logind `idle:sleep` inhibitor controller. Linux battery,
> thermal, and focused power status/doctor surfaces have also landed. Remaining
> Linux gates are service install, log rotation, packaging docs, and deeper
> on-device logind validation. This plan intentionally stops at Linux; Windows
> remains Phase 5.9.

## Goal

Ship Linux support behind Vigil's existing Rust seams without weakening the
macOS behavior that shipped in phases 5.1-5.7.

Linux must preserve Vigil's user-facing invariant:

- Prevent idle/system sleep while agents are actively working.
- Do not keep the display awake.
- Do not suppress the native lock screen.
- Do not add a root/admin helper unless the implementation proves one is
  necessary. The expected Linux path is user-scoped logind inhibition.

Linux is not byte-for-byte identical to macOS power management. Phase 5.8 must
choose the Linux hold mode from evidence rather than assuming `idle` is enough:
the candidates are `idle`, `sleep`, and `idle:sleep` through logind. The target
is the same core product behavior - active agents do not get interrupted by
system sleep - while still avoiding display-awake, lock-screen, shutdown, and
hardware-key inhibition by default.

## Source Anchors

Primary API facts verified for this plan:

- systemd logind `Inhibit` accepts `what`, `who`, `why`, and `mode`, returns an
  fd, and releases the inhibitor when the fd is closed.
  https://systemd.io/INHIBITOR_LOCKS/
  https://manpages.ubuntu.com/manpages/noble/man5/org.freedesktop.login1.5.html
- systemd separates `idle` from `sleep`: `idle` covers automatic idle handling,
  while `sleep` covers suspend/hibernate requests. `systemd-inhibit` defaults to
  `idle:sleep:shutdown`; Vigil must test `idle`, `sleep`, and `idle:sleep`, then
  choose the narrowest mode that satisfies the product invariant. Do not use
  `shutdown`, `handle-lid-switch`, or display/screen-locker inhibition by
  default.
  https://systemd.io/INHIBITOR_LOCKS/
- UPower exposes `OnBattery` on `/org/freedesktop/UPower`; its display device
  exposes `Percentage`.
  https://upower.freedesktop.org/docs/UPower.html
- Linux thermal zones expose readable sysfs state under thermal-zone files.
  https://docs.kernel.org/driver-api/thermal/sysfs-api.html

Crate checks on 2026-06-19:

- `zbus = 5.16.0`; default features include `blocking-api`.
- `logind-zbus = 5.3.2`; exposes `ManagerProxyBlocking::inhibit`.
- `service-manager = 0.11.0`.
- `sysinfo = 0.39.3`; already used by the repo and current on crates.io.
- `notify = 8.2.0`; already used by the repo and current stable. Crates.io also
  lists `9.0.0-rc.4`, but do not move Vigil to a release candidate unless a
  Linux-specific bug forces it.
- `procfs = 0.18.0` if `sysinfo` needs Linux-only process details.

Dependency policy:

- Use latest stable crate releases, not prereleases, unless a prerelease fixes a
  concrete blocker.
- Prefer existing dependencies (`sysinfo`, `notify`) when they already satisfy
  the Linux contract.
- Treat `keepawake` as a reference implementation, not a planned dependency,
  unless the direct logind path proves worse in code or maintenance cost.

## Non-Goals

- No Windows code in Phase 5.8.
- No Antigravity/Gemini provider work.
- No Homebrew/release work.
- No redesign of provider detection, refcount semantics, idle windows, or the
  VS Code semantic-hash gate.
- No Linux root helper by default. File, owner, and mode hardening still apply
  to user-owned state and service artifacts.

## Current Repo Reality

The live code now exposes `crates/vigil/src/power/platform.rs` with a
`PowerController` facade and `PowerSummary`. The daemon owns a boxed platform
controller; on macOS, `MacPowerController` wraps the existing macOS-shaped
`PowerMachine<MacHelperClient, MacCaffeinate, MacSleepReader>`:

- `MacHelperClient` talks to the privileged root helper.
- `MacCaffeinate` spawns `/usr/bin/caffeinate -i -t 1800`.
- `MacSleepReader` reads `pmset`.
- The tick loop and startup recovery now go through the facade.
- Focused power status/doctor output is now platform-aware: macOS still reports
  `pmset`/`caffeinate`/root-helper state, while Linux reports
  `logind-idle:sleep`, logind reachability, and Vigil-owned logind hold state.
- Setup, start, stop, reload, uninstall, service install, and log rotation still
  need Linux-specific implementations.

Linux now builds to a real `LinuxLogindPower` controller. It holds one logind fd
for `idle` and one for `sleep`, releases by dropping the fds, and reports
`logind-idle:sleep` through the status snapshot. The implementation deliberately
does not inhibit shutdown, display, lock, lid-switch, or hardware-key handling.

## Implementation Order

### Gate 0 -- Linux Compile Baseline

Make the workspace compile on Linux before adding real power behavior.

Tasks:

- Add target-specific dependency sections in `crates/vigil/Cargo.toml`:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
logind-zbus = "5.3.2"
zbus = "5.16.0"
service-manager = "0.11.0"
```

- Keep `procfs = "0.18.0"` out until `sysinfo` proves insufficient.
- Guard macOS-only root-helper code with `#[cfg(target_os = "macos")]`.
- Keep the `vigil-root-helper` binary macOS-only in behavior. It may remain a
  target in Cargo if cheap, but on Linux it must not be installed or required.
- Keep `native/vigil-lock-helper` buildable on non-macOS through its existing
  stub path, but do not install it as a Linux lock implementation in this phase.
- Add Linux-friendly platform labels before output changes:
  `PlatformKind::{Macos, Linux, Windows}` or equivalent.

Acceptance:

- `cargo check --workspace` passes on macOS.
- Linux CI target can run `cargo check --workspace` after the cfg gates land.
- Existing macOS goldens are unchanged.

Implementation note (2026-06-19):

- `cargo check --workspace` passes on macOS.
- `cargo check --workspace --target x86_64-unknown-linux-gnu` passes without
  dead-code warnings from `native/vigil-lock-helper`; macOS-only helper modules
  are cfg-gated out of non-macOS builds.
- Existing macOS goldens remain unchanged.

### Gate 1 -- Platform Power Facade

Introduce the minimum facade needed by the daemon and status engine.

Add a new module, one of:

- `crates/vigil/src/power/platform.rs`, or
- `crates/vigil/src/platform/power.rs`.

Preferred shape:

```rust
pub trait PowerController {
    fn recover_startup(&mut self, startup_count: u32, can_hold: bool, now: i64) -> bool;
    fn engage(&mut self, now: i64) -> Result<(), String>;
    fn reconcile_engaged(&mut self) -> Result<(), String>;
    fn full_release(&mut self);
    fn soft_release(&mut self);
    fn observable_engaged(&self) -> bool;
    fn summary(&self) -> PowerSummary;
}

pub struct PowerSummary {
    pub mode: String,
    pub platform_hold: bool,
    pub baseline: Option<u8>,
    pub helper_ok: Option<bool>,
    pub assertion_pid: Option<u32>,
    pub assertion_alive: bool,
}
```

Mac adapter:

- Wrap the current `PowerMachine` without changing its pure decision functions.
- Preserve partial-engage adoption, fail-safe baseline parsing, soft release for
  thermal cuts, and full release for battery/no-work cuts.
- Preserve `pmset_disablesleep`, `baseline`, `caffeinate_pid`, and
  `caffeinate_alive` in status JSON on macOS.

Linux adapter:

- Own the logind inhibitor fd directly.
- No baseline file for a privileged system toggle; `baseline` becomes `null` in
  Linux JSON.
- `soft_release` and `full_release` both close/drop the inhibitor fd. The
  distinction is retained at the call site for cross-platform logging and future
  parity, but Linux has no `SleepDisabled` baseline to preserve.

Acceptance:

- `daemon::act` is generic over the new facade or calls a dynamic boxed facade.
- Existing `PowerMachine` unit tests remain meaningful.
- Add trait-contract unit tests with a fake controller for engage, reconcile,
  soft release, full release, and failed engage.

Implementation note (2026-06-19):

- `daemon::act` now takes `&mut dyn PowerController`.
- The resident daemon stores `Box<dyn PowerController>`.
- `MacPowerController` delegates to the existing `PowerMachine`, preserving the
  existing pure and side-effect tests.
- `UnsupportedPowerController` remains only for non-macOS/non-Linux targets.

### Gate 2 -- Linux Logind Hold

Implement `LinuxLogindPower` with a synchronous logind client.

Dependency choice:

- Start with `logind-zbus = "5.3.2"` because it exposes typed enums and
  `ManagerProxyBlocking`.
- Use raw `zbus::blocking::Proxy` only if the typed crate creates version or
  feature friction.

Candidate inhibitors and selection:

- Test `idle`, `sleep`, and `idle:sleep` before setting the default. This is a
  release gate, not a nice-to-have.
- The expected final default is either `idle` (if automatic-idle suspend is the
  only supported no-admin behavior) or `idle:sleep` (if `sleep` works
  unprivileged and best matches the macOS product guarantee without excessive
  user-intent blocking).
- Keep the internal representation capable of holding multiple logind inhibitor
  fds from the first patch, so selecting `idle:sleep` does not reshape the
  daemon.
- Do not default to `shutdown`, `handle-lid-switch`, power-key, suspend-key, or
  hibernate-key inhibitors.

Concrete hold path:

```rust
use logind_zbus::manager::{InhibitType, ManagerProxyBlocking, Mode};

let conn = zbus::blocking::Connection::system()?;
let manager = ManagerProxyBlocking::new(&conn)?;
let mut inhibitor_fds = Vec::new();
for what in selected_inhibitors {
    let fd = manager.inhibit(
        what,
        "vigil",
        "AI agents are actively working",
        <&str>::from(Mode::Block),
    )?;
    inhibitor_fds.push(fd);
}
```

With `logind-zbus`, each `InhibitType` call returns one `OwnedFd`; multiple
selected inhibitors mean multiple fds. If raw `zbus` is later chosen, a single
colon-separated `what` string is also acceptable, but the daemon-owned state
should still model "one or more fds held".

State shape:

```rust
pub struct LinuxLogindPower {
    conn: Option<zbus::blocking::Connection>,
    inhibitor_fds: Vec<zbus::zvariant::OwnedFd>,
}
```

Rules:

- Keep the `Connection` alive for at least as long as the fds.
- Holding means `!inhibitor_fds.is_empty()`.
- Release means drain the fd vector and let each fd drop.
- Reconcile means if desired and no fd, acquire a new fd; if fd exists, do
  nothing.
- If logind denies the inhibitor or the system bus is unavailable, log the
  failure, leave `engaged=false`, and surface the failure in doctor/status.

Doctor/status probe:

- `logind reachable`: can connect to system bus and create `ManagerProxyBlocking`.
- `idle inhibit`: optional active check through `ListInhibitors`, filtered by
  `who == "vigil"` and current pid, or a local `!inhibitor_fds.is_empty()` check
  in the daemon tick state.
- `power_hold_mode`: `"logind-idle"`, `"logind-sleep"`, or
  `"logind-idle:sleep"` depending on the selected default.
- `pmset_disablesleep`: keep the existing JSON key for schema v1, but emit `0`
  on Linux and document it as macOS-only textually.
- `power_helper_ok`: keep the existing JSON key for schema v1, but emit `true`
  when logind is reachable and `false` when not.

Acceptance:

- A unit test with a fake `LogindClient` proves fd-held and fd-dropped
  lifetimes.
- A Linux integration test behind `#[cfg(target_os = "linux")]` can run against
  live logind when available and is skipped with a clear message when unavailable.
- Manual testing records whether `idle`, `sleep`, and `idle:sleep` prevent the
  target distro's automatic idle suspend, manual suspend, and lid-close suspend.
  The final default must be documented with the evidence and its weaker/stronger
  guarantee.
- Manual Linux check:

```bash
vigil status --json
vigil run sleep 30 &
systemd-inhibit --list | grep vigil
```

Implementation note (2026-06-19):

- `crates/vigil/src/power/linux.rs` implements `LinuxLogindPower` with a fakeable
  `LogindClient` seam and production `SystemLogindClient`.
- The selected default is `idle:sleep`, surfaced as `logind-idle:sleep`. This is
  intentionally narrower than `systemd-inhibit`'s default because Vigil does not
  block shutdown, display, lock, lid-switch, or hardware-key handling.
- Partial acquire failure drops already-acquired fds before returning an error.
- Startup recovery reacquires when refs remain and safety gates allow a hold;
  otherwise it releases.
- `cargo test -p vigil` covers the fake logind contract tests on macOS and in a
  Linux Podman container.

### Gate 3 -- Linux Thermal And Battery

Keep the existing `PowerGuard` trait, but split collectors by platform.

Battery:

- Add `battery::read_platform_raw()` or `battery::read_linux()`.
- On Linux, use UPower as the single battery authority:
  - Read `org.freedesktop.UPower.OnBattery`.
  - Read `/org/freedesktop/UPower/devices/DisplayDevice` `Percentage`.
- If UPower is absent or unreadable, battery state is `Unknown`; do not add a
  `/sys/class/power_supply` fallback or backward shim.
- Preserve fixture seams:
  - `VIGIL_BATTERY_FIXTURE` remains accepted.
  - Add Linux fixture text/JSON only if needed; do not break existing macOS
    fixture parser tests.

Linux parsed model:

```rust
pub enum LinuxAcState {
    Ac,
    Battery,
    Unknown,
}

pub struct LinuxBatteryReading {
    pub ac: LinuxAcState,
    pub pct: Option<u32>,
}
```

Thermal:

- Add `thermal::read_platform()` or `thermal::read_linux()`.
- Read `/sys/class/thermal/thermal_zone*/temp` and zone metadata.
- Treat a readable high temperature as a cut when it crosses a conservative
  threshold. If a trip-point threshold is discoverable, use it; otherwise use
  the documented 85C policy threshold.
- Keep fail-closed behavior for unreadable live thermal state during a hold.
- Keep `VIGIL_THERMAL_FIXTURE` for deterministic tests.

Acceptance:

- Unit tests for UPower parsing:
  - AC 100 -> no cut.
  - battery 5 with floor 20 -> cut.
  - battery 20 with floor 20 -> no cut.
  - decimal percentages floor conservatively before comparing with the floor.
  - missing or unreadable UPower -> unknown/no cut, doctor/status report unknown.
- Unit tests for sysfs thermal fixtures:
  - no zones/readable cool zone -> no cut.
  - hot zone above threshold -> cut.
  - unreadable live thermal during daemon hold -> cut.

Implementation note (2026-06-19):

- Linux battery collection now reads UPower over D-Bus and emits an internal
  `VIGIL_LINUX_UPOWER=1` parseable snapshot.
- No `/sys/class/power_supply` fallback is shipped. If UPower is unavailable,
  battery state is unknown and Vigil does not infer AC/battery state from a
  second source.
- Linux thermal collection reads `/sys/class/thermal/thermal_zone*/temp`, uses a
  reasonable trip-point threshold when available, and otherwise falls back to
  85C. Unreadable live thermal state remains fail-closed.
- Existing macOS fixture parser behavior is preserved, and Linux-specific UPower
  and thermal parser tests now run on macOS and Linux.

### Gate 4 -- Linux Service Install

Implement systemd user service installation behind `ServiceInstaller`.

Service unit:

```ini
[Unit]
Description=Vigil agent activity sleep guard

[Service]
Type=simple
ExecStart=<resolved_install_dir>/bin/vigil daemon
Restart=always
RestartSec=10
Environment=VIGIL_STATE_DIR=<resolved_state_dir>
Environment=VIGIL_LOG_DIR=<resolved_log_dir>

[Install]
WantedBy=default.target
```

Path decision:

- Prefer XDG paths on Linux:
  - install dir: `${XDG_DATA_HOME:-~/.local/share}/vigil`
  - state dir: `${XDG_STATE_HOME:-~/.local/state}/vigil/state`
  - log dir: `${XDG_STATE_HOME:-~/.local/state}/vigil/logs`
  - config file remains `${XDG_CONFIG_HOME:-~/.config}/vigil/vigil.conf`
- Keep macOS defaults unchanged.
- Ensure all generated paths are printed by `vigil setup --dry-run`.

Installer behavior:

- No `sudo`.
- Create user-owned dirs with mode `0700` for state and `0755`/`0700` as
  appropriate for install/log dirs.
- Install user unit under
  `${XDG_CONFIG_HOME:-~/.config}/systemd/user/com.thangaram.vigil.service`.
- Render the unit from Vigil-owned code and golden-test it. Use
  `service-manager` for systemd user lifecycle calls if its behavior stays
  compatible, but do not let a third-party renderer control the byte-stable
  dry-run preview.
- Run:

```bash
systemctl --user daemon-reload
systemctl --user enable --now com.thangaram.vigil.service
```

- Non-systemd Linux exits with an explicit unsupported message:
  `vigil setup: systemd --user is required on Linux in Phase 5.8`.
- Do not enable linger by default. `loginctl enable-linger` changes session
  semantics and may need policy/admin approval; doctor may mention it only as an
  optional note if the user wants the daemon to survive logout.

Acceptance:

- Golden render for Linux service unit.
- `setup --dry-run --verbose` prints Linux paths and unit body without touching
  system state.
- `start`, `stop`, `reload`, and `uninstall` use the Linux installer when built
  on Linux.

### Gate 5 -- Linux Log Rotation

Implement external logrotate support.

Plan:

- Add `vigil reload-log` as a hidden subcommand that reopens the daemon log
  writer.
- Install a user-readable logrotate snippet where the distro supports it.
- If a user-level logrotate include path cannot be found, doctor warns and the
  daemon still runs. Do not require root solely for log rotation.

Minimum viable Phase 5.8 behavior:

- The log file remains append-only and bounded only by user-configured external
  rotation when logrotate exists.
- Doctor reports `log rotation: warning (logrotate not configured)` rather than
  failing setup.

Acceptance:

- Hidden CLI parse test for `reload-log`.
- Unit test that `reload-log` swaps/reopens the appender seam without changing
  the log-line format.

### Gate 6 -- Linux Status And Doctor Text

Keep `status --json` schema version at `1` unless keys are added or removed.

Linux text changes:

- Replace `launchd` labels with `systemd user`.
- Replace `root helper` with `logind`.
- Replace `pmset/caffeinate` dependencies with `systemctl`, `dbus/logind`, and
  optional `upower`.
- Keep agent activity text unchanged.
- Keep power assertion summary macOS-only; on Linux use an inhibitor summary:
  `inhibitors: 1 active (vigil)` when inspectable.

Doctor groups:

- platform
- dependencies
- logind
- user service
- directories
- providers
- power guards
- lock helper (warning: Linux lock overlay is not shipped in 5.8)

Acceptance:

- Linux golden text fixtures for:
  - clean installed/running.
  - not installed.
  - logind unavailable.
  - non-systemd unsupported.
- Existing macOS status/doctor goldens remain unchanged.

Implementation note (2026-06-19):

- The focused power status/doctor path is platform-aware for the landed Linux
  power gate. Linux reports `logind-idle:sleep`, logind reachability, and whether
  Vigil-owned `idle` and `sleep` block inhibitors are visible.
- The machine JSON schema keeps the historical key names for compatibility, but
  values are platform-aware. Golden tests normalize only the platform mode and
  remain byte-exact otherwise.
- Broader service lifecycle commands are still macOS-shaped and remain in later
  Linux gates.

### Gate 7 -- Linux Lock Command Boundary

Do not promise a real Linux lock overlay in Phase 5.8 unless it can be tested on
both Wayland and X11.

Behavior:

- `vigil lock` on Linux exits with:
  `vigil lock: Linux lock overlay is not shipped yet`
- `vigil lock doctor` reports the same as a warning, not a setup failure.
- Keep `native/vigil-lock-helper` macOS behavior unchanged.

Reason:

- A trustworthy Linux input-freeze guard is materially different across Wayland,
  X11, and desktop portals. Shipping the power/sleep Linux port must not be
  blocked by an unverified lock overlay.

Acceptance:

- CLI tests for Linux lock unsupported text.
- README says Linux Phase 5.8 covers sleep prevention first; lock overlay is
  deferred unless implemented and manually verified.

### Gate 8 -- CI And Verification

Add Linux CI only after Gate 0 compiles locally or in a Linux runner.

Required checks:

```bash
cargo fmt --check
cargo build
cargo clippy -p vigil --all-targets -- -D warnings
cargo test
cargo test --features helper-test-seam
```

Linux runner checks:

```bash
cargo check --workspace
cargo test -p vigil
```

Manual Linux smoke:

```bash
vigil setup --dry-run --verbose
vigil doctor
vigil status --json
vigil run sleep 20
systemd-inhibit --list | grep vigil
vigil uninstall --yes
```

## File-Level Change Map

Expected files:

- `crates/vigil/Cargo.toml`
  - Add Linux target dependencies.
- `crates/vigil/src/power/platform.rs`
  - New `PowerController` facade and `PowerSummary`.
- `crates/vigil/src/power/linux.rs`
  - `LinuxLogindPower` plus fakeable logind client seam.
- `crates/vigil/src/power/mod.rs`
  - Re-export platform facade; keep existing macOS pure functions.
- `crates/vigil/src/daemon/mod.rs`
  - Own a platform power controller instead of concrete macOS helper/caffeinate
    fields.
- `crates/vigil/src/config/mod.rs`
  - Add OS-specific path defaults without changing macOS defaults.
- `crates/vigil/src/service/mod.rs`
  - Add `SystemdUserInstaller`; preserve `MacosLaunchdInstaller`.
- `crates/vigil/src/commands/{setup,start,stop,reload,uninstall}.rs`
  - Select installer by target OS.
- `crates/vigil/src/commands/{status,doctor,lock}.rs`
  - Platform wording and Linux unsupported lock behavior.
- `crates/vigil/src/battery/mod.rs`
  - Platform collectors while retaining existing pure macOS parser tests.
- `crates/vigil/src/thermal/mod.rs`
  - Platform collectors while retaining existing pure macOS parser tests.
- `crates/vigil/tests/golden/`
  - Add Linux service/status/doctor goldens.
- `.github/workflows/*` or equivalent CI file if the repo adopts CI in this
  slice.

## Acceptance Definition

Phase 5.8 is done only when:

- macOS tests still pass with unchanged shipped behavior.
- Linux build and unit tests pass on a Linux runner.
- `vigil setup --dry-run --verbose` renders Linux unit/log/config paths without
  privilege.
- A real Linux session can acquire and release the selected logind `idle:sleep`
  inhibitors.
- `vigil status` and `vigil doctor` explain Linux state without mentioning
  launchd, pmset, caffeinate, or a root helper as required dependencies.
- `vigil lock` has either a tested Linux implementation or an explicit
  unsupported error that is documented and tested.

## Next Coding Item

Proceed to the Linux service/install gate: add a systemd user service installer
behind `ServiceInstaller`, keep macOS launchd behavior unchanged, and verify in a
Linux container or VM. Follow with log rotation and packaging docs. A real Linux
laptop pass should additionally verify logind inhibition against suspend behavior
 and confirm UPower battery plus sysfs thermal readings on hardware.
