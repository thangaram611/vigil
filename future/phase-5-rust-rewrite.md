# Phase 5 — Full Rust rewrite + UX overhaul + cross-OS

> **STATUS: PLANNING (umbrella plan).** This document supersedes the old sketch
> `future/phase-5-cross-os.md`. It is plan-altitude only: each `5.x` sub-phase
> gets its **own** detailed implementation doc when that sub-phase begins (repo
> policy — see the final section). Do not treat the code-shaped fragments here as
> implementation; they are intent markers a future engineer/agent can follow.

Vigil today is ~2090 lines of Bash (`bin/vigil` CLI 1509, `bin/vigil-daemon`
191, `bin/vigil-root-helper` 390) plus ~1360 lines of `lib/*.sh`, and one Rust
binary (`native/vigil-lock-helper`, ~929 lines). Phase 5 collapses the Bash into
a single Rust `vigil` binary plus two helper binaries (`vigil-lock-helper`,
already Rust, and a new `vigil-root-helper`), one subsystem at a time, deletes
the corresponding Bash once the Rust slice reaches parity, then adds Linux and
Windows behind already-stable platform seams. macOS reaches full parity and
ships first.

---

## 1. Decisions locked in this planning conversation

These were confirmed with the user this session. **Do not relitigate them.**

1. **Numbering = Phase 5.x** (5.1, 5.2, …). This supersedes the old
   `future/phase-5-cross-os.md` sketch.
2. **Strategy = incremental strangler.** Migrate ONE subsystem at a time to
   Rust, parity-test it, then delete the corresponding Bash. **No backward-compat,
   no dual-runtime at the subsystem level, no feature flags.** Single-user
   project: the old Bash for a slice is removed once the Rust slice reaches
   parity. (See §6 for the one honest caveat: a Rust-CLI / Bash-daemon
   *process-level* coexistence window is unavoidable and is managed via a frozen
   state-file ABI, not feature flags.)
3. **Each sub-phase is a VERTICAL SLICE** bundling, in the same pass: (a) the
   Rust port of a subsystem, (b) that subsystem's UX overhaul, (c) that
   subsystem's security hardening. UX and security ride *with* each slice; they
   are not separate trailing passes.
4. **Cross-OS = macOS parity FIRST.** Design portable seams (traits) now, but
   ship and stabilize macOS-on-Rust before any Linux code, then Windows, as
   later sub-phases. Only a Mac is available to test on.
5. **UI scope IN:** structured/colored CLI with progressive disclosure,
   interactive prompts + auto-fix in setup/doctor, and a native full-screen
   `lock` overlay window. **UI scope OUT (defer to Phase 6):** menu-bar/tray
   status item, full GUI app.
6. **doctor/status MERGE is deferred** (user explicitly deferred it). BUT the
   underlying check/diagnostic engine is **unified now** (one `CheckEngine`) so a
   future merge is trivial. Two commands stay for now.
7. **Use mature, industry-standard crates.** Do not reinvent per-OS
   abstractions.

### Two framing corrections that must not regress

- **Thermal is the agents' heat, not vigil's.** "Laptop gets hot when agents
  run" is the *agents' CPU load* while sleep is held open — not vigil's memory
  management. A Rust rewrite will **not** cool the machine. The thermal item is
  framed as a **smarter/configurable thermal-cutoff policy**, plus the small but
  real efficiency win of **one resident process** versus forking `ps`/`pmset`/
  `find` every 5-second tick. **Never claim Rust fixes thermals.**
- **Most UX pain needs no display/overlay.** The pain is plain terminal output
  (setup/doctor/status/start/stop/reload/uninstall). The overlay is **only** for
  `lock`.

---

## 2. What this is / is NOT

**IN scope for Phase 5:**

- Full port of the Bash CLI, daemon, and root helper to a single Rust `vigil`
  binary (+ `vigil-root-helper`, + the existing `vigil-lock-helper`).
- Structured, colored, progressively-disclosed CLI output; interactive
  prompts + auto-fix in setup/doctor.
- A native full-screen **lock overlay window** (the only display work in the
  entire rewrite).
- A smarter, configurable thermal-cutoff policy (numeric `CPU_Scheduler_Limit`
  parsing + threshold knob), defaulting to exact current behavior.
- Cross-OS seams now; Linux then Windows trait impls as trailing slices.
- A **unified `CheckEngine`** feeding both `doctor` and `status`.

**OUT of scope (Phase 6 or later):**

- Menu-bar / tray status item.
- A full GUI application.
- **Merging `doctor` and `status`** into one command (deferred — engine is
  unified now so the merge is later trivial).
- `cargo-dist` + Homebrew tap activation / first public release (gated by the
  ROADMAP "no release until all phases ship" rule).

---

## 3. Target architecture

### 3.1 Crate / binary layout

A thin `clap`-derive CLI dispatches to subsystem modules behind platform traits.

```
vigil (single binary)
  src/main.rs            clap Parser/Subcommand dispatch; exit-code discipline
  src/output/            anstream + owo-colors + comfy-table + serde_json render
  src/config/            figment layered config; provider-home cascade; path derivation
  src/log/               tracing + tracing-subscriber + tracing-appender (NonBlocking)
  src/procscan/          sysinfo process scan (fork-free)
  src/activity/          notify (FSEvents) watchers + vscode sha256 semantic gate
  src/refcount/          PID-file refcount + GC branches
  src/thermal/           PowerGuard impl (numeric CPU_Scheduler_Limit policy)
  src/battery/           PowerGuard impl (AC-aware floor check)
  src/power/             engage/release/reconcile/baseline state machine
  src/ipc/               request/response file client (user side)
  src/daemon/            single-instance guard + tick loop + atomic tick file
  src/service/           ServiceInstaller trait (launchd plists via `plist` crate)
  src/check/             ONE CheckEngine -> Vec<Check>, consumed by doctor AND status

vigil-root-helper (new binary)   resident root component; file-queue IPC; fixed pmset argv
vigil-lock-helper (existing)     HID CGEventTap freeze guard + (new) overlay window
```

Substrate slices (CLI/output → config/logging) come first because every later
slice prints, reads config, and logs through them. The privileged power path is
the highest-risk slice and is sequenced after the substrate and the unprivileged
detection core are stable, so the root-helper rewrite happens against a
known-good caller.

### 3.2 Platform-seam traits

Design the seams now; fill non-macOS impls in 5.8 (Linux) and 5.9 (Windows)
without refactoring the seams.

| Seam (trait)         | Responsibility                                  | macOS now                                              | Linux later (5.8)                          | Windows later (5.9)                                  |
|----------------------|-------------------------------------------------|--------------------------------------------------------|--------------------------------------------|------------------------------------------------------|
| `PowerController`    | Hold/release system sleep prevention            | `caffeinate -i` + root-helper `pmset -a disablesleep`  | `keepawake` → logind `Manager.Inhibit(idle,block)` via zbus | `keepawake` → `SetThreadExecutionState(ES_CONTINUOUS\|ES_SYSTEM_REQUIRED)` |
| `CaffeinateAssertion`| Idle-sleep-only assertion child (NOT display)   | `caffeinate -i` (identity-verified child)              | logind idle inhibitor FD (Drop releases)   | `SetThreadExecutionState` (no display flag)          |
| `ProcessScanner`     | Enumerate agent processes by name/exe           | `sysinfo` (name + exe path-prefix)                     | `sysinfo` (/proc) + optional `procfs`      | `sysinfo` (Windows backend)                          |
| `ActivityWatcher`    | Session-dir freshness + vscode semantic gate    | `notify` (FSEvents)                                    | `notify` (inotify)                          | `notify` (ReadDirectoryChangesW)                     |
| `PowerGuard`         | Thermal + battery cutoff predicates             | `pmset -g therm` / `pmset -g ps`                        | sysfs thermal / UPower                      | sysfs-equivalent / Win32 power APIs                  |
| `ServiceInstaller`   | Install/uninstall the resident service          | launchd LaunchAgent + LaunchDaemon (`plist` crate)     | systemd user unit (`service-manager` lifecycle) | Task Scheduler logon trigger **or** `windows-service` (decided on-device) |
| `LogRotation`        | Rotate the daemon log file                      | newsyslog drop-in (NEVER the appender's rotator)       | logrotate.d drop-in + `reload-log` subcmd  | `logroller` in-process (size 1MB, keep 5, gzip)      |
| `Locker`             | Native lock action                              | HID CGEventTap freeze (existing helper)                | X11/Wayland `_NET_WM_STATE_ABOVE` stub     | `LockWorkStation` (interactive desktop only)         |
| `LockOverlay`        | Full-screen armed-state overlay window          | `objc2-app-kit` NSWindow above dock/menu bar           | X11/Wayland topmost window                  | `SetWindowPos HWND_TOPMOST` + GDI                    |

Privileged file-queue IPC is **uniform across all OSes** (see §3.3). `getpeereid`
(macOS) and `SO_PEERCRED` (Linux) are intentionally **unused** — the
file-ownership model is the boundary on every platform.

### 3.3 Privilege boundary

**Decision (firm, non-relitigable): keep the file-based request/response queue.
Do NOT migrate to a unix socket.** On macOS `getpeereid` yields only
`(euid, egid)` with **no PID** — strictly weaker identity than the current
filesystem-ownership + per-uid-namespaced-dir model — while an accept loop adds
connection-lifecycle attack surface for zero gain. One resident root binary,
request files in, response files out.

**pmset lever:** keep shelling `/usr/bin/pmset` via `std::process::Command` with
a **fixed argv per action** (`-a disablesleep <0|1>`). Do **not** use the private
`IOPMSetSystemPowerSetting` SPI — its unstable schema is a poor fit for a root
binary. The `Command` is run with `env_clear()` + a minimal pinned
`PATH=/usr/bin:/bin:/usr/sbin:/sbin`, not an inherited launchd env.

**Hardening upgrades available only in Rust (these are net improvements over the
Bash, which is safe only by virtue of the move-into-root-0700-processing-dir):**

1. Open the request file with `nix::fcntl::open(O_NOFOLLOW | O_RDONLY)` so
   symlinks are rejected at the kernel `open(2)` level, then **`fstat` the open
   fd** (not the path) for `uid == ALLOWED_UID`, `S_ISREG`, `st_nlink == 1`, and
   not group/other-writable. This closes the residual TOCTOU.
2. **Apply the same `O_NOFOLLOW`-on-open + `fstat`-on-fd discipline to EVERY
   directory check on BOTH sides**, not just the request file (adversarial fix —
   see below): the per-poll request-DIR ownership re-check, the startup
   response/state/log root-dir checks, and the processing-dir check must use
   `open(O_NOFOLLOW | O_DIRECTORY)` + `fstat`, **never** `std::fs::metadata`
   (which follows symlinks and gives no fd guarantee). A path-based dir check is
   a silent symlink-redirect-of-root-writes regression.
3. Keep the **atomic move-into-root-owned processing dir BEFORE validation**
   (claim-then-validate, never validate-then-claim). The processing dir is a
   fixed subdir of the root-owned-validated `state_dir`, itself verified
   root-owned + 0700 + non-symlink via `open(O_NOFOLLOW | O_DIRECTORY)` + `fstat`
   before any `rename` into it. `O_NOFOLLOW` on the moved file is layered **on
   top of** this, not a replacement.
4. Response files written `O_WRONLY | O_CREAT | O_EXCL` to a temp, `fchmod 0644`,
   then `rename`. The **client** opens `resp.<id>` with `O_NOFOLLOW | O_RDONLY`
   **once**, `fstat`s that fd for `uid==0 / S_ISREG / nlink / not
   group-or-other-writable`, and reads the body from the **same fd** — never
   re-opening by path after the check (closes the symmetric client-side TOCTOU).
5. Construct the response path with `openat`/`renameat` relative to a **validated
   response-dir fd** so even a charset bug in the request id cannot escape the
   dir. The id charset `^[A-Za-z0-9_.-]+$` is the only traversal guard; also
   reject `.` and `..` explicitly.

**Matched-pair validation is preserved and defense-in-depth is NOT
"optimized away":** the helper validates requests (3 actions only —
engage/release/status; reject any content beyond line 1; per-tick request-DIR
ownership re-check; root-owned non-symlink response/state/log dirs) **and** the
client validates responses (root-owned, regular, non-symlink, not
group/other-writable) so a local process cannot forge success. Both checks remain
even though each side is also the other's trust anchor.

**Liveness preserved:** on ANY rejection (bad_filename, symlink, not_regular,
owner, hardlink, group/other-writable, invalid_action, extra_content) the moved
request file MUST be removed, and when the id is charset-valid an **error
response (`status=error`, `message=<reason>`) MUST be written atomically** — so a
rejection produces an error response, not a 10s client timeout, and the queue
never accumulates poison files. The doctor helper-reachability probe depends on
the rejected-vs-unreachable distinction.

**Fail-safe parsing (security-relevant):** all three baseline/`SleepDisabled`
parsers MUST fail **safe** — a missing, corrupt, or non-`0|1` value yields release
target `0` (sleep-enabled), never an error that aborts the release. A `serde`
field with no default, or a strict parse that errors on a corrupt baseline, would
either panic or leave `SleepDisabled=1` stuck (the exact stuck-state crash
recovery exists to prevent).

**Test seams become compile-time, not runtime:** `VIGIL_ROOT_HELPER_TESTING`,
`VIGIL_ROOT_HELPER_LIB_ONLY`, and `VIGIL_TEST_NO_ADMIN` become
`cfg(test)`/feature-gated constructs **compiled out of the shipped root binary** —
they cannot be flipped at runtime on the installed root binary. A red-team test
asserts a release-profile helper with `VIGIL_ROOT_HELPER_TESTING=1` in its env
STILL refuses to run as non-root and STILL uses the install-time-fixed
allowed-uid. `--allowed-uid` (numeric-validated `^[0-9]+$`) and `--allowed-user`
are baked into the plist at install time and are **never** derived from request
content.

**Crates:** `nix = { default-features = false, features = ["socket", "user",
"fs", "process"] }`, `libc` for `O_NOFOLLOW` / macOS constants.

---

## 4. The thermal & performance reality

**The correction, stated plainly so it never regresses:** the heat is the
*agents'* CPU load while sleep is held open. Vigil holding a sleep assertion does
not generate heat; rewriting vigil in Rust does **not** cool the machine. Any plan
text or log line that frames vigil as "protecting the machine" from heat it
caused is wrong.

**What Rust actually buys, honestly:**

1. **One resident process instead of ~12–20 forks per tick.** The Bash daemon
   forks `ps -axww` (twice), `find` (per agent), `awk`, `pmset`, `shasum`, and
   `date` (per log line) every 5-second tick. The Rust daemon holds one
   `sysinfo::System`, push-based FSEvents watchers, and a zero-alloc timestamp.
   This is a real efficiency and attack-surface win (it removes ~10 fork/exec
   sites carrying shell-injection-via-process-output risk), **not** a thermal fix.
2. **A smarter, configurable thermal-cutoff policy.** Today any presence of
   `CPU_Scheduler_Limit` or `thermal warning level` (lines with an `=` separator)
   triggers a cutoff. The Rust port **parses the numeric value** and gates on a
   configurable `VIGIL_THERMAL_CPU_LIMIT_FLOOR`. **Default = exact any-presence
   parity** (cutoff on any reported throttle) unless the user opts into the
   threshold — otherwise it is a silent policy change. The `=` anchor stays
   load-bearing (it excludes the `Note: No CPU_Scheduler_Limit has been recorded`
   informational lines).
3. **Clearer messaging.** Status/doctor say "paused: thermal pressure from running
   agents; will resume after cooldown (N s left)" with the parsed numeric throttle
   value, instead of the opaque truncated WARN blob. Raw thermal/battery readings
   land in the tick file so automation can read them.

---

## 5. Sub-phase sequence

Dependencies are derived from the **actual call graph**, not the conceptual
subsystem map. Critically (verified against `bin/vigil-daemon`, which `source`s
all seven libs, and `bin/vigil`, which has 19 `vigil_pmset_*`/assertions call
sites): **the monolithic Bash daemon and CLI source every lib and survive until
5.7.** There is no seam to `rm` a single lib along until its last Bash sourcer
dies. Therefore the per-slice "bashRetired" entries below are split into two
honest categories:

- **DEAD-FROM-RUST:** the lib stays physically on disk (the still-Bash daemon/CLI
  needs to source it) but becomes dead from all Rust callers. Physical deletion
  is deferred to 5.7 when the daemon and CLI die.
- **DELETED:** the file is physically removed in this slice — only possible for
  files with no surviving Bash sourcer (the root helper in 5.5; everything in
  5.7).

**Parity-oracle reality (verified):** of 13 `*_test.sh` files, only a handful
(`cli_preview_test`, `lock_test`, `wrapper_test`) drive the `vigil` binary as a
subprocess (a couple more, e.g. `newsyslog_test`, invoke it incidentally). The other ~10 **source `tests/lib.sh` and call lib functions
in-process** (`detect_test`→`vigil_detect_all`, `thermal_test`→
`vigil_thermal_should_cut`, `root_helper_test`→`VIGIL_ROOT_HELPER_LIB_ONLY=1
source bin/vigil-root-helper`, etc.). You **cannot** "re-point" a
`source thermal.sh; vigil_thermal_should_cut` test at a Rust binary with an env
var — once the lib is deleted the function does not exist. So:

- The 3 subprocess tests genuinely re-point at the Rust binary (gated via the
  5.1 shim, then directly).
- The ~10 function-level tests must be **ported to `cargo` tests** that consume
  the **same `tests/fixtures/` files as golden inputs** and reproduce the exact
  Bash assertions.
- **Gate-0 of every porting slice (5.2–5.5):** capture the current Bash output as
  golden fixtures BEFORE writing the Rust port. A slice may not retire its lib
  (dead-from-Rust or deleted) until BOTH its `cargo`-test rewrite passes AND the
  golden fixtures match.

**Per-slice rollback rule:** each slice's Bash retirement (dead-from-Rust cutover
or physical deletion) is a **separate commit** gated on the **full**
`tests/run.sh` staying green (not just the slice's own tests), so a regression
found after the fact can be reverted independently.

---

### Phase 5.1 — CLI skeleton + output/render substrate

**Goal.** Stand up the single `vigil` binary with `clap`-derive subcommand
dispatch and the `anstream`/`owo-colors`/`comfy-table` output layer every later
slice prints through. Establish the exit-code and help contracts. No subsystem
logic yet — unported commands delegate to the existing Bash via a thin shim, so
the binary is usable from day one.

**Rust deliverable.** `vigil` crate: `main.rs` (clap `Parser`/`Subcommand` for
setup/start/stop/status/doctor/lock/run/log/reload/uninstall/completions);
`src/output/` (`anstream::println/eprintln` at every print site; `owo-colors`
attributes; `colorchoice-clap` `--color=auto|always|never` flattened, with
`write_global()` called immediately after parse; `comfy-table` tables;
`serde_json` for `--json`). `vigil completions <shell>` via `clap_complete`.
`vigil --version`/`-V` prints `vigil <VERSION>`. Unknown subcommand → help to
stderr, exit 64. Shim layer `exec`s `bin/vigil` for not-yet-ported subcommands.

**UX deliverable.** Structured colored help (replaces the plain heredoc `cat`); a
pass/fail symbol + color vocabulary (check/cross/warn glyphs); the global
`--color` flag. `NO_COLOR`/`CLICOLOR`/non-tty stripping is automatic via
`anstream`. This sets the visual language all later slices reuse.

**Security deliverable.** Establish the test-mode guard pattern: a single
`cfg`/feature-gated `admin_allowed()` and a `VIGIL_TEST_NO_ADMIN` hard-abort
entrypoint that ALL future admin paths must call; wire the `EX_USAGE(64)` vs
`error(1)` exit-code discipline so later slices inherit it. No privileged code
yet — the guard skeleton lands now so no slice is tempted to add a privileged
path without it.

**Crates.** clap 4.6.1 (derive), clap_complete 4.6.5, anstream 1.0.0,
owo-colors 4.3.0, colorchoice-clap 1.0.8, comfy-table 7.2.2, serde 1.0.228,
serde_json 1.0.150.

**Parity tests / Bash retired.** New `tests/cli_dispatch_test.sh`: exit 64 on
unknown command; exact `--version` string; every subcommand reachable;
`--color=never` strips ANSI (pipe to file, assert no escape codes). Existing
`cli_preview_test.sh` re-pointed at the shim must still pass byte-identically.
`cargo test` for arg parsing + exit-code mapping. **Bash retired: NONE.** The shim
delegates; `cmd_help` and the top-level dispatch case are *superseded* for ported
commands but not deleted.

**Risks.** clap compile bloat — enable only derive+std features. The `anstream`
refactor must wrap EVERY print site or stripping is inconsistent. The shim could
mask behavioral drift — mitigated by keeping `cli_preview_test.sh` green against
the shim.

**Depends on.** (none).

**Recommended implementation model / agent strategy.** Sonnet, single pass.
Mechanical clap/anstream plumbing with a documented crate stack and a spelled-out
wiring order; low ambiguity, no security-critical or concurrency logic. A light
self-review for exit-code parity suffices.

---

### Phase 5.2 — Config + logging substrate (figment + tracing)

**Goal.** Port `VIGIL_*` config resolution and daemon/helper logging to Rust as
shared libraries every later slice consumes. Pure substrate: get the precedence
chain, provider-home cascade, derived paths, and file-as-source-of-truth log
format exactly right once.

**Rust deliverable.** `src/config/`: `VigilConfig` (serde `Deserialize`,
`#[serde(default)]` per field) loaded via figment layers `Serialized::defaults <
Toml::file(vigil.conf) < Env::prefixed("VIGIL_")` (**NO** split on `_`) `< CLI`.
Post-extraction passes: `derive_provider_homes()` replicating the explicit
`VIGIL_*_HOME` > provider-env (`CLAUDE_CONFIG_DIR`/`CODEX_HOME`/`COPILOT_HOME`) >
default cascade with the auto-flag semantics, run **after** conf load;
`derive_paths()` computing `VIGIL_LOG_FILE`, `VIGIL_ACTIVE_DIR`,
baseline/tick/pid/lock paths once (never re-read env at call sites). `src/log/`:
`tracing` + `tracing-subscriber` (fmt + env-filter) + `tracing-appender`
NonBlocking over an append-mode `File`, with a `LogRotation` seam (macOS =
newsyslog, NEVER the appender; `logroller` behind `cfg(windows)` later). Custom
`FormatEvent` reproducing `YYYY-MM-DDTHH:MM:SS%z LEVEL message`. `dirs 6.0.0` for
platform paths.

**UX deliverable.** A hidden/debug `vigil config --show` (or doctor hook) prints
resolved paths and provider homes so the sticky/override state is inspectable —
directly addresses the "no way to see resolved config" gap. vigil.conf format
(TOML) documented; users get a clear error if the old bash-sourced conf has shell
syntax.

**Security deliverable.** vigil.conf **stops being executable shell** — switch to
a strict TOML parser (closes arbitrary-code-execution-via-conf). Implement the
post-extraction validation of security-path vars (`VIGIL_ROOT_*`,
`VIGIL_POWER_*`, newsyslog, helper plist) against hardcoded canonical values with
**exact-equality** rejection (not prefix) — the allowlist later admin slices
depend on. State dir created chmod 0700.

**Crates.** figment 0.10.19 (toml feature), serde 1.0.228, serde_json 1.0.150,
toml 0.8 (via figment), tracing 0.1.44, tracing-subscriber 0.3.23
(fmt, env-filter), tracing-appender 0.2.5, dirs 6.0.0.

**Parity tests / Bash retired.** New `tests/config_parity_test.sh`: a matrix of
`VIGIL_*` + vigil.conf + provider-env combinations asserting the Rust `vigil
config --show` JSON matches what Bash `vigil_load_config` computes (capture Bash
output as **golden fixtures first**). Must test: `VIGIL_LOG_DIR` set ONLY in conf
re-derives `VIGIL_LOG_FILE`; explicit `VIGIL_CLAUDE_HOME` is NOT clobbered by
`CLAUDE_CONFIG_DIR` in conf. Log-format test greps a Rust-emitted line against the
operator regex. **Bash retired: DEAD-FROM-RUST only** — the config-loading half of
`lib/common.sh` (`vigil_load_config` + the `VIGIL_*` default block, ~lines 23–257)
and `log()` become dead from Rust callers; the file stays on disk until 5.7
(daemon/CLI still source it). Nothing is `rm`'d this slice.

**Risks.** figment `Env::split` footgun — must NOT split on `_` or
`VIGIL_IDLE_AFTER_SEC` breaks. Provider-home auto-flag re-derivation order is a
documented hard-won regression source — golden-fixture tests are mandatory. The
NonBlocking worker opens the file once — verify it re-opens after a newsyslog
rename, or add a reload subcommand (Linux later). `VIGIL_FORCE` / `VIGIL_LOCK_MAX_SECS`
typing: pick `u8` with explicit 0/1, document accepted values.

**Depends on.** 5.1.

**Recommended implementation model / agent strategy.** Sonnet for the
figment/tracing wiring; **Opus pass ONLY** for the provider-home cascade + path
derivation ordering and the security-path allowlist (documented silent-regression
hotspots). Sonnet implements, then a focused Opus diff-review against the golden
fixtures and must-preserve config invariants.

---

### Phase 5.3 — procscan + activity + refcount (unprivileged detection core)

**Goal.** Port the per-tick detection pipeline — process scan, activity probes,
PID-file refcount and GC — to Rust as a resident, fork-free core. This is the
efficiency win (replace ~12–20 forks/tick with one `sysinfo` snapshot +
push-based FS watchers) and the input to the power state machine. No privileged
calls; safe to land before the power slice.

**Rust deliverable.** `src/procscan/` over `sysinfo`: a single long-lived
`System`, `refresh_processes` scoped to cmd+exe only; agent detection via
`Process::name()` primary + `Process::exe()` path-prefix disambiguation;
hard-exclusion patterns (Electron Helpers, crashpad, `/Applications/*`) ported
verbatim using `exe()` `Path` matching (**NOT** `cmd()[0]` splitting — preserves
the spaced-path fix). `src/activity/` over `notify` (FSEvents): recursive
watchers on claude `projects/` + codex `sessions/` + copilot `session-state/`
updating per-agent `AtomicU64` last-activity; the vscode-copilot probe keeps the
sha256-on-change semantic gate (notify triggers, hash decides) with primed-first-
run suppression and the `active_until` cache; graceful handling of not-yet-
existent session dirs (watch parent, retry). Idle window: round-up minutes
formula preserved (`ceil(secs/60)`, min 1) to match BSD `find -mmin` granularity
rather than ns mtime. `src/refcount/`: PID-file count filtered by per-prefix
activity flags (`cli-*` / `app-*` / `wrapper`-always-counts), GC branches
(a) dead-PID, (b) PID-reuse via `start_ts` compare, (c) idle-CPU with the wrapper
carve-out from (c) **only**. Test seams `VIGIL_*_FIXTURE` / `VIGIL_VSCODE_PS_FIXTURE`
preserved as injection points.

**UX deliverable.** This slice delivers the data model + a debug dump so the
"providers section is useless for diagnosis" flaw is fixable in 5.7 (session-dir
path, exists, latest-activity-age). Per-agent active/idle/none state strings are
UNCHANGED values feeding the JSON schema.

**Security deliverable.** `sysinfo` `cmd()` same-user restriction documented as
parity (not a regression). State dir stays 0700 so refcount/PID files aren't
world-readable. No new privilege surface — and the slice removes ~10 fork/exec
sites (`ps`/`find`/`awk`), shrinking the shell-injection-via-process-output
surface the two-column `ps` join carried.

**Crates.** sysinfo 0.39.3, notify 8.2.0, notify-debouncer-mini 0.7.0 (vscode
probe only, optional), serde/serde_json (state files).

**Parity tests / Bash retired.** Port `detect_test.sh`, `activity_test.sh`,
`refcount_activity_test.sh`, **and `parser_test.sh`** (the `_vigil_pidfile_field`
PID-file JSON parser — guards the documented `awk -F'[:,}]'` start_ts bug; named
explicitly so it is not orphaned) to `cargo` tests consuming the same
`tests/fixtures/` files. Smoke test: `sysinfo` `Process::name()` returns bare
`claude` for a PATH-invoked process AND the full bundle path (with spaces) via
`exe()` for `Code.app` — the two open research questions. GC tests: dead PID
dropped; reused PID (start_ts differs) dropped; idle low-CPU dropped EXCEPT
wrapper. vscode hash test: mtime-only rewrite does NOT signal active; content
change does. **Bash retired: DEAD-FROM-RUST** — `lib/detect.sh` (194),
`lib/activity.sh` (281), `lib/refcount.sh` (202) become dead from Rust callers but
stay on disk until 5.7 (the Bash daemon still sources them). They are the safest
*meaningful* cutover; physical `rm` is in 5.7.

**Risks.** `sysinfo` first refresh allocates the full process table — keep ONE
`System` for the daemon lifetime, never per-tick. FSEvents ~1s batching is fine
for a 5-min window but verify the vscode hash-storm case (debouncer window TBD).
`notify` on a missing session dir must not error-and-die — watch the parent. The
idle-window granularity (find-floor vs precise mtime) could flip active/idle at
boundaries — preserve the round-up.

**Depends on.** 5.1, 5.2.

**Recommended implementation model / agent strategy.** Opus, single implementer +
one parity-review pass. Moderate difficulty: the GC branch logic and the vscode
semantic-hash gate are subtle and regression-prone, and the spaced-path
exe-matching is a hard-won fix. Not adversarial-security tier, but needs careful
behavioral fidelity — Opus over Sonnet.

---

### Phase 5.4 — thermal + battery guards (smarter, configurable policy)

**Goal.** Port the thermal and battery cutoff guards behind a `PowerGuard` trait
and ship the smarter/configurable thermal policy the user wants — parse the
**numeric** `CPU_Scheduler_Limit` and add a configurable threshold instead of
treating any field presence as a cutoff. Reframe the UX from "vigil protects the
machine" to "agents are the heat source; vigil backs off".

**Rust deliverable.** `src/thermal/` + `src/battery/` behind
`trait PowerGuard { fn thermal_cut() -> bool; fn battery_cut() -> bool }`. macOS
impl parses `pmset -g therm` / `pmset -g ps` (a **single** `pmset -g ps` read
collapsing the two Bash forks into one atomic snapshot — fixes the AC/battery
TOCTOU). `VIGIL_FORCE` checked **first** (before any subprocess). Thermal signal
preserved: line matching `^\s*(CPU_Scheduler_Limit|thermal warning level)\s*=`
(the `=` anchor is load-bearing). NEW: parse the numeric value and gate on a
configurable `VIGIL_THERMAL_CPU_LIMIT_FLOOR` — **default behavior = any-presence
cutoff for exact parity unless the new knob is set.** Battery: AC = no-cut,
unknown = no-cut, strict `pct < floor`, empty pct = no-cut. Cooldown
re-arm-every-pressure-tick sliding window preserved, `VIGIL_THERMAL_COOLDOWN_SECS`
configurable and independent of tick.

**UX deliverable.** Status/doctor messaging reframed: "paused: thermal pressure
from running agents; will resume after cooldown (N s left)" instead of the opaque
WARN. Expose the parsed numeric throttle value (not the `head -c 100` truncated
blob) and the new threshold knob. Battery shows labeled "on battery 18% (floor
20%)" vs the bare "AC ?%". Surface thermal/battery raw readings into the tick file
(addresses the "tick file only has binary cut flags" flaw).

**Security deliverable.** `VIGIL_THERMAL_FIXTURE`/`VIGIL_BATTERY_FIXTURE`
documented test-only; release build asserts/ignores them on the daemon launch
path. Raw pmset field values sanitized before going into structured logs/status
(log-injection hardening for the future TUI). No privilege change.

**Crates.** Reuses serde, tracing; pmset via `std::process::Command` — no new
crate.

**Parity tests / Bash retired.** Port `thermal_test.sh` + `battery_test.sh` to
`cargo` tests driven by `VIGIL_THERMAL_FIXTURE`/`VIGIL_BATTERY_FIXTURE` fixtures.
Assert the `=` anchor rejects "Note: No CPU_Scheduler_Limit has been recorded"
(no false cutoff). Assert `VIGIL_FORCE=1` short-circuits both before any pmset
call. Assert battery boundary: exactly 20% does NOT cut (strict `<`); empty pct =
no cut; AC = no cut; unknown = no cut. New tests for the numeric-threshold policy
with **default = exact-parity behavior**. **Bash retired: DEAD-FROM-RUST** —
`lib/thermal.sh` (53), `lib/battery.sh` (68) become dead from Rust callers but
stay on disk until 5.7 (Bash daemon still sources them at startup and per tick).

**Risks.** The numeric-threshold feature must **default to exact any-presence
parity** or it is a silent policy change. The `=` regex precision is subtle (a
substring match would false-positive). **Do not claim this cools the machine.**

**Depends on.** 5.1, 5.2.

**Recommended implementation model / agent strategy.** Sonnet for the port
(small, well-specified, fixture-driven), with a short Opus design note for the
numeric-threshold policy default to ensure exact-parity-by-default. Low
difficulty, high specification.

---

### Phase 5.5 — Privileged power path: vigil-root-helper (Rust) + IPC client + power state machine

**Goal.** The highest-risk slice. Port the root helper, the file-based IPC, and
the engage/release/reconcile/baseline/caffeinate state machine to Rust —
preserving the privilege boundary verbatim while UPGRADING file validation to
`O_NOFOLLOW` + `fstat`-on-fd (§3.3). Sequenced after the unprivileged core is
stable so the new helper is exercised by a known-good caller.

**Rust deliverable.** New `vigil-root-helper` binary: `--serve`/`--once`,
`--allowed-uid`/`--allowed-user` (install-time-fixed), `--poll-secs` (default 1).
Request validation per §3.3 (O_NOFOLLOW open + fstat-on-fd for
uid/S_ISREG/nlink/mode; id charset `^[A-Za-z0-9_.-]+$` plus reject `.`/`..`;
single-line/extra-content reject — read the WHOLE validated file and reject any
content after the first newline-terminated action line, including trailing
content without a newline; **per-poll** request-DIR ownership re-check via
O_NOFOLLOW|O_DIRECTORY+fstat; root-owned non-symlink response/state/log dirs via
the same; atomic move-into-validated-root-0700-processing-dir BEFORE validate;
fixed argv `/usr/bin/pmset -a disablesleep <0|1>` with release target from the
root-owned baseline only; pinned PATH + `env_clear()`; engaged-marker vs baseline
as TWO separate 0600 files; idle-release no-op; atomic `O_EXCL` response write +
rename; **error response on every rejection + always-consume the request**). The
helper's own `helper.log` must re-guard the log dir as non-symlink before each
append (open once with `O_NOFOLLOW` at startup and refuse a symlinked dir).
Orphaned `processing/` files from a crashed prior instance (KeepAlive restart)
are validated root-owned before any cleanup. `src/power/` (state machine in
`vigil`): engage (capture-baseline-idempotent → helper engage → only-then spawn
`caffeinate -i`); full release (helper release THEN **always** kill caffeinate
even on failure THEN clear baseline); soft_release (thermal: drop hold, kill
caffeinate, KEEP baseline); per-tick `reconcile_engaged` (re-read SleepDisabled,
reassert on drift, verify caffeinate by IDENTITY not bare `kill -0`, reject any
display-flag caffeinate as stale — match any argv token of the form
`-<letters-including-d>` i.e. the regex `(^|space)-[A-Za-z]*d[A-Za-z]*($|space)`
so `-di` and `-dimsu` are both replaced, AND verify basename == `caffeinate`);
crash-recovery decision tree (active + can_hold ⇒ reassert; else restore + clear;
recapture baseline if caffeinate pidfile exists without baseline; **can_hold
evaluates thermal + battery at startup**). `src/ipc/` client: atomic
`.req`→`req` under umask 077/0600, high-entropy id, response-file root-owned
fd-based validation (§3.3), timeout cleanup. caffeinate behind a
`CaffeinateAssertion` seam (macOS = `caffeinate -i`).

**UX deliverable.** doctor/status get a first-class helper round-trip probe
("root helper: reachable" vs "unreachable / timing out") so silent helper
failures stop being invisible. Surface the sticky-baseline state: "baseline=1
(another tool may hold SleepDisabled; vigil will not re-enable sleep)" with a
hint. Distinguish "idle, not engaged" from "engaged" so the release-no-op is
explicable.

**Security deliverable.** The entire boundary, hardened per §3.3:
O_NOFOLLOW+fstat-on-fd on BOTH sides and on every dir check; all request checks +
dir checks + id charset + extra-content + fixed argv + pinned/cleared env
preserved as a matched pair with user-side response validation; error-response-
on-rejection liveness; fail-safe baseline parsing (target 0 on
corrupt/missing). `VIGIL_ROOT_HELPER_TESTING` / `VIGIL_ROOT_HELPER_LIB_ONLY`
become `cfg(test)`/feature-gated, COMPILED OUT of the shipped root binary.
`VIGIL_TEST_NO_ADMIN` + exact-equality privileged-path allowlist enforced before
any sudo install/rm of the helper.

**Crates.** nix 0.31.3 (socket, user, fs, process), libc 0.2.186 (O_NOFOLLOW,
macOS constants), serde/serde_json (baseline, response fields), tracing
(helper.log via its own subscriber).

**Parity tests / Bash retired.** Two test layers, kept distinct: (1)
`cargo` tests for the validation/state-machine logic (run as non-root via the
`cfg(test)` seam) using `tests/fixtures/`; (2) a **subprocess-level adversarial
test** that spawns the real Rust helper binary. Adversarial cases proven BEFORE
deletion: symlinked request rejected (O_NOFOLLOW errors at open); hardlink
(nlink≠1) rejected; group/other-writable rejected; wrong-owner rejected;
multi-line rejected; bad-charset id rejected; **every rejection yields a
`status=error` response, not a client timeout, and leaves no file in request or
processing dir**; symlinked state/response/log/processing dir rejected at open;
request-dir re-owned mid-run skips that poll's batch; forged root-owned response
accepted only when truly root-owned; release-while-idle is a no-op (no pmset
call); soft_release keeps baseline / full release clears it; reconcile reasserts
on externally-flipped SleepDisabled; PID-reused caffeinate treated as dead; `-di`
AND `-dimsu` caffeinate replaced with `-i`; **corrupt baseline ⇒ release runs
`pmset -a disablesleep 0` and clears engaged**; red-team: release-profile helper
with `VIGIL_ROOT_HELPER_TESTING=1` STILL refuses non-root and uses the fixed
allowed-uid. **Bash retired: DELETED** — `bin/vigil-root-helper` (390) is
physically removed once the full adversarial + reconcile + crash-recovery matrix
is green (it has no surviving Bash sourcer except `root_helper_test.sh`, which is
replaced by the cargo + subprocess tests). `lib/pmset.sh` is **NOT** fully
deleted here (see below) — its **privileged** engage/release/reconcile/baseline
state machine becomes DEAD-FROM-RUST, but the **read-only status helpers**
(`vigil_assertions_summary`, `vigil_pmset_caffeinate_alive`,
`vigil_read_sleepdisabled`) are still called by the Bash status/doctor path
(`cmd_status`/`cmd_doctor`/`cmd_doctor_power`) until 5.7. Physical `rm` of
`lib/pmset.sh` is deferred to 5.7. (The assertions-summary tri-state parser and
`assertions_test.sh` are owned by the 5.7 status path, see 5.7.)

**Risks.** Single highest-risk surface (runs as root). Ergonomic `std::fs` would
silently drop symlink/hardlink/follow defenses (`metadata()` follows symlinks,
`is_file()` follows, no nlink) — MUST use raw `open(O_NOFOLLOW)` + fstat-on-fd
everywhere. The two-state engaged-vs-baseline + idle-release-no-op + soft-vs-full
release interactions regress to a single `held` bool if modeled naively (baseline
stickiness loss + third-party-setting clobber). Crash recovery must evaluate
thermal+battery at startup for `can_hold`. caffeinate liveness-by-identity vs bare
`kill -0`. pmset SPI deliberately NOT used (Command keeps stability).

**Depends on.** 5.2, 5.3, 5.4.

**Recommended implementation model / agent strategy.** Opus, **adversarial
security-review panel**. Highest difficulty + highest blast radius. Opus
implements behind the trait; then a dedicated adversarial review pass (a separate
Opus reviewer instructed to break each request/response validation and each
state-machine asymmetry) gates the deletion. **Do not let a single happy-path-
green run authorize removing `bin/vigil-root-helper`.**

---

### Phase 5.6 — Lock subsystem: UX overhaul + native overlay window + CF-family migration

**Goal.** Upgrade the already-Rust lock subsystem: add the full-screen overlay
window (the ONLY display work in the whole rewrite), overhaul the lock
CLI/countdown UX, and migrate the lock helper's CoreFoundation/CoreGraphics
bindings to the `objc2` family. Port `vigil lock` / `vigil lock doctor`
orchestration from Bash into the Rust CLI.

**Rust deliverable.** In `vigil-lock-helper`: add `src/overlay.rs` behind
`trait LockOverlay { show(&OverlayState); hide() }` with a macOS impl over
`objc2-app-kit` (borderless NSWindow at `kCGPopUpMenuWindowLevel`=101 via
`setLevel` — winit/eframe CANNOT reach this; `NSApplicationActivationPolicy::Accessory`
to suppress the dock icon; two `NSTextField` subviews for status + unlock-chord
hint to avoid core-text/drawRect ceremony; created/updated on the existing
CFRunLoop tap loop, no separate thread). **CF-family migration scope (corrected —
this is bigger than core-foundation alone):** the lock helper depends on BOTH
`core-foundation = "0.10.1"` **and `core-graphics = "0.25.0"`**, and `src/macos.rs`
imports the entire event-tap surface (`CGEvent`, `CGEventFlags`, `CGEventTap`,
`CGEventTapLocation`, `CGEventTapOptions`, `CGEventTapPlacement`,
`CGEventTapProxy`, `CGEventType`, `EventField`) from `core-graphics`, plus
`CGSessionCopyCurrentDictionary` / `CGPreflight*EventAccess` / `CGEventTapEnable`
via raw `extern "C"`. Migrating only core-foundation→`objc2-core-foundation`
would leave `core-graphics` (which itself pulls core-foundation 0.10.x) → the
"one CF family" goal is **unachievable** that way. **Pick one and state it in the
5.6 impl doc:** (a) ALSO migrate `core-graphics 0.25.0` → `objc2-core-graphics`
0.3.x and re-bind the full CGEventTap surface onto the objc2 family (accounting
for the substantial unsafe-FFI re-port of the tap callback signature); **or**
(b) explicitly DESCOPE the "one CF family" goal, keep core-graphics +
core-foundation 0.10.x for the tap, and add only the `objc2-app-kit` overlay
(accepting a two-CF-family binary). The current middle-ground neither compiles to
the stated goal nor is honest about the trade — choose at implementation time
(recommended: (a) if `objc2-core-graphics` is published at a usable version, else
(b)). In `vigil` CLI: port `cmd_lock` + `cmd_lock_doctor` — pre-arm sleep hold via
the refcount wrapper BEFORE launching the helper and wait up to
`VIGIL_START_WAIT_SECS` for the daemon to reflect the hold; 3-2-1 countdown;
`--combo`/`--max-secs` with the `--max-secs 0` guard (0 accepted ONLY via explicit
CLI, never from `VIGIL_LOCK_MAX_SECS=0` config — note the helper itself is
permissive, so the CLI is the **sole** gate and the guard must not be dropped in
the port); exit 64 for unknown subcommand/non-macOS/unknown arg, 1 for
permission/helper-missing; lock doctor exit 0 iff
`listen_event_access + accessibility_trusted + tap_create_active_hid_ok`
(`post_event_access` informational); `--prompt` gates the permission dialog.

**UX deliverable.** The marquee item: a full-screen overlay showing armed state,
countdown, and the unlock-chord hint, above dock + menu bar. CLI countdown becomes
a clean indicatif-style render; the critical "recover" instruction is emphasized
(color/bold); lock doctor output uses **computed** alignment (fixes the by-eye
padding) with the exact field labels + "lock guard readiness: ready/not ready"
summary preserved.

**Security deliverable.** `--max-secs 0` footgun guard preserved (no accidental
permanent freeze from config drift). Resolve and document the flagged
`CGEventTapLocation::HID`-vs-`Session` discrepancy (research found HID where the
phase-4 spec said Session). `setIgnoresMouseEvents` belt-and-suspenders since the
tap already drops input. Keep `vigil-lock-helper` a SEPARATE binary so its
Input-Monitoring/Accessibility TCC grant stays pinned to a stable path (merging
into `vigil` would force the main binary to require those grants — see Open
Decisions).

**Crates.** objc2 0.6.4, objc2-foundation 0.3.2, objc2-app-kit 0.3.2
(NSWindow, NSApplication, NSScreen, NSColor, NSView), objc2-core-foundation 0.3.2,
**objc2-core-graphics 0.3.x (linchpin — confirm it exists/version at impl time;
required by migration option (a))**, objc2-io-kit 0.3.2 (if migrating existing IO
usage). Removes core-foundation 0.10.1 (and, under option (a), core-graphics
0.25.0) from this binary.

**Parity tests / Bash retired.** `lock_test.sh` re-pointed at the Rust CLI:
exit-code matrix (64/1/0) for unknown subcommand, non-macOS, permission-denied,
helper-missing; lock doctor field labels + readiness summary; `--max-secs 0`
rejected from env but accepted from CLI. **Newly written** (current `lock_test.sh`
only asserts a help string): a pre-arm-then-wait ordering test (hold reflected in
the tick file BEFORE countdown/exec) — note this depends on the working
power+tick path, hence the added dependency below. The overlay is **manually
verified on the Mac** (window level above dock/menu bar, countdown updates,
teardown on unlock/timeout) since it is a display artifact; `cargo` tests cover
combo parsing (existing) and overlay state transitions behind a headless stub.
**Bash retired: DELETED** — `cmd_lock` and `cmd_lock_doctor` orchestration in the
CLI (the lock helper itself was already Rust; this retires its Bash front-end).
Because these live in the still-Bash `bin/vigil`, "deleted" here means the Rust
CLI owns them and the shim no longer delegates lock/lock-doctor; the surrounding
`bin/vigil` file is physically removed in 5.7.

**Risks.** NSWindow must be created on the main thread (`MainThreadMarker`) — the
CFRunLoop tap loop IS the main thread, so it fits, but interspersing
NSApplication event pumping in the tap loop needs verification. **Run-loop mode (corrected against the
actual source — `native/vigil-lock-helper/src/macos.rs:135-141`).** The freeze
loop pumps the **standard `kCFRunLoopDefaultMode`**: `event_tap_run_mode()`
returns `kCFRunLoopDefaultMode`, while the tap *source* is added to
`kCFRunLoopCommonModes` via `event_tap_source_mode()`. The existing
`run_loop_uses_specific_mode_not_common_modes_sentinel` test only asserts the run
mode is a concrete mode distinct from the `CommonModes` *set* — NOT that it is a
private/custom mode. This is **favorable** for the overlay: an NSWindow registers
its drawing/timer sources on `kCFRunLoopDefaultMode`/`commonModes` by default —
exactly the mode the existing loop already pumps — so the overlay's redraw and
countdown are serviced by the loop that already runs, with no separate thread and
no custom mode. The genuine remaining risks are the ordinary AppKit ones:
(1) `NSApplication` must be initialized and finished-launching so a borderless
`Accessory`-policy window actually composites; (2) the loop runs in short
(tens-of-ms) `run_in_mode(DefaultMode, …, false)` slices inside a tight `while`,
so confirm the cadence redraws a 1-Hz countdown smoothly (fallback: drive
`setNeedsDisplay` per tick); (3) NSApplication event pumping must not starve the
tap loop. Keep this a stated acceptance check ("countdown visibly updates, window
stays above dock/menu bar") — it now verifies AppKit init/cadence, not a
run-loop-mode mismatch. The
core-foundation→objc2 migration must not regress the existing tap/permission code.
The overlay is the only thing that cannot be unit-parity-tested — manual
verification on the single available Mac.

**Depends on.** 5.1, 5.2, 5.3, **5.5, 5.7** (corrected): the overlay/CLI/objc2
work truly needs only 5.1/5.2/5.3, but `cmd_lock`'s pre-arm path calls
`vigil_refcount_touch_wrapper` then waits on the daemon **tick-file fields**
(`refcount_active`/`engaged`/`thermal_cut`/`battery_cut`/`cooling`) — fields
produced by the 5.7 daemon — and its "hold engaged before freeze" guarantee
depends on the 5.5 power state machine. Therefore: the overlay + lock-doctor + CLI
exit-code port may land after 5.3, but the **`cmd_lock` pre-arm + power-hold-wait
port is gated on 5.5 + 5.7** (or the tick-file-schema + refcount-touch +
power-hold-wait pieces thereof). Practically, sequence 5.6 after 5.7, or split it
so the pre-arm lands last.

**Recommended implementation model / agent strategy.** Opus for the objc2 overlay
+ CFRunLoop-mode integration (unsafe FFI, main-thread rules, window-level + run-
loop-mode subtleties) and the core-graphics migration; one Opus implementer since
the overlay and migration are intertwined. One manual on-device verification pass.

---

### Phase 5.7 — Daemon loop + service mgmt (launchd) + setup/uninstall/reload + unified CheckEngine; final Bash cutover

**Goal.** Land the resident daemon main loop and the install lifecycle, unify
doctor + status onto one `CheckEngine`, and complete the strangler: after this
slice the `vigil` binary is self-contained and ALL remaining Bash is physically
deleted. This is the integration slice that ties the prior subsystems into the
tick loop and the launchd plumbing — and the slice where every "dead-from-Rust"
lib from 5.2–5.5 is finally `rm`'d.

**Rust deliverable.** `src/daemon/`: single-instance guard via atomic `mkdir`
lock dir (not `flock` — macOS has no `flock(1)`) with stale-lock PID-liveness
recovery; tick loop wiring procscan + activity + refcount + thermal + battery +
power into the exact `desired=1` predicate
(`count>0 && !thermal && !battery && !cooling`) and the engage/reconcile/soft-vs-
full-release branches; atomic tick-file write (tmp + rename) with the EXACT field
set (`pid, updated_at, tick_secs, refcount_active, desired_hold, engaged,
thermal_cut, battery_cut, cooling`) so the existing scan-state enum still
classifies. `src/service/` behind `trait ServiceInstaller`:
`MacosLaunchdInstaller` generates the user LaunchAgent + root LaunchDaemon plists
via the `plist` crate (typed structs, XML-escaping automatic, **not** heredocs)
and runs bootout → poll-up-to-50×100ms → bootstrap (**not** `kickstart -k`); TCC
copy-out-of-Documents preserved (binary copied to
`~/Library/Application Support/vigil/bin` before the plist points at it).
`cmd_setup` (dry-run + `--verbose` plist preview; silent `cmd_stop` before sync),
`cmd_uninstall` (5-step ordered teardown, zero-flag strict, sudo'd root removal
behind the path allowlist, logs preserved, legacy sudoers cleanup), `cmd_reload`
(full bootout/bootstrap, re-render plist), `cmd_start` (bounded wait for first
scan, "pending" not an error), `cmd_stop` (the 50×100ms bootout poll), `cmd_run`
(**NON-exec** wrapper PID-file with trap-equivalent cleanup on
EXIT/INT/TERM/HUP, propagate child exit code), `cmd_log` (-f follow / full cat
with paging/line-limit, soft message if missing). `src/check/`: ONE `CheckEngine`
producing `Vec<Check>` consumed by BOTH `vigil doctor` (three-state
not-installed/needs-repair/ready[+warnings], lock-helper-absent = warning not
error, `--power` subset) and `vigil status` (always exit 0, `--json` flat schema
with a versioned top-level `version` field, `daemon_scan_state` enum, agents
sub-object, expected-hold messaging, `--verbose` progressive disclosure). doctor
stays a separate command but shares the engine — future merge is then trivial.
**The read-only pmset status helpers** (`vigil_assertions_summary` tri-state
parser + sentinels, `vigil_pmset_caffeinate_alive` display path,
`vigil_read_sleepdisabled`) are ported here as part of the status/doctor render
path (they are NOT privilege-boundary code) and feed `power_assertions_state`.

**UX deliverable.** setup/uninstall become a colored numbered checklist with
pass/fail symbols and a single source of step numbering (fixes
manual/inconsistent numbering). status gets computed column alignment + a labeled
power line (fixes the 5-fields-in-one-string + uneven-width flaws). The "expected
hold: pending" catch-all gets actionable sub-states. The always-shown `--verbose`
hint is suppressed when not applicable. status/doctor share output shape (same
data, one format) without merging the commands. `vigil log` without `-f` gets
paging/line-limit (no megabyte dump). reload prints a "what changed" summary.
start gets its own header line (fixes the start/stop output asymmetry). doctor's
providers section shows session-dir diagnostics by default.

**Security deliverable.** `cmd_assert_standard_privileged_paths` (exact-equality,
11 paths) + `cmd_assert_vigil_tree_path` (absolute, no newline/CR, not `/` or
`$HOME`, ends in `/vigil`, not under `~/Documents`/TCC paths) +
`VIGIL_TEST_NO_ADMIN` abort enforced before EVERY sudo in setup/uninstall/reload.
Plist values XML-escaped via the `plist` crate (no manual escaping bugs). The
two-plist asymmetric-ownership IPC dir matrix (request dir user-0700,
response/state/log root) created with the exact owners/modes the helper validators
require. launchd labels hardcoded (not overridable).

**Crates.** plist 1.9.0, service-manager 0.11.0 (DEFERRED — Linux only, not used
on macOS), sysinfo (single-instance PID liveness), tracing/tracing-appender
(daemon log + newsyslog NEVER the appender), clap/anstream/comfy-table
(status/doctor render).

**Parity tests / Bash retired.** Re-point `newsyslog_test.sh`, `wrapper_test.sh`,
`cli_preview_test.sh`, and the status/doctor portions of the suite at the Rust
binary. **Golden-fixture the `--json` status schema** (every key +
`daemon_scan_state` enum + agents state strings + the new top-level `version`)
against current Bash output; assert byte-stable. Port `assertions_test.sh` (the
early-warning oracle for Apple changing the pmset assertions block schema) here,
in the status path that owns the tri-state sentinel contract. Assert: stop's
50×100ms bootout poll present (mock launchctl); reload uses bootout/bootstrap not
kickstart; setup `--dry-run` touches nothing; uninstall ordering +
strict-zero-flag; `vigil run` does NOT exec (PID file cleaned on trap, child exit
code propagated); doctor exit 0 with lock-helper-absent warning, exit 1 on
needs-repair. **The full `tests/run.sh` must be green against the Rust binary
before any Bash file is removed.** **Bash retired: DELETED (the final deletion)** —
`bin/vigil` (1509), `bin/vigil-daemon` (191), and ALL remaining libs:
`lib/common.sh` (288), `lib/pmset.sh` (306), `lib/detect.sh`, `lib/activity.sh`,
`lib/refcount.sh`, `lib/thermal.sh`, `lib/battery.sh` (the dead-from-Rust files
from 5.2–5.5). The 5.1 shim is removed. The `vigil` binary is now self-contained
(plus the two helper binaries).

**Risks.** Largest integration surface — a regression in the desired-hold
predicate or the tick-file field names silently misclassifies scan state. The
bootout/bootstrap race poll MUST stay (fast machines fail setup without it).
`cmd_run` MUST NOT exec (trap/cleanup leak). TCC copy-out-of-Documents is a
silent-launch-failure if dropped. The `plist` crate needs `skip_serializing_if`
on optional keys (launchd uses absence). Set MSRV from `sysinfo` (1.95) and
`plist` (1.88) — take the higher. **5.3 wiring carry-overs (the efficiency goal
is only realized HERE):** the 5.3 cores ship resident-ready but are still called
ad-hoc — `procscan::host_running(None)`, `refcount::gc`, and the `vigil debug`
dump each construct their OWN `ProcScanner`/`System` per call (fine one-shot,
a per-tick full-table alloc in a loop). The daemon must own ONE long-lived
`ProcScanner`/`System` for its lifetime and thread it (or a single pre-collected
snapshot) through detect + vscode host-check + gc each tick — never per-subsystem
construction. `refcount::gc` self-spaces its two `with_cpu()` refreshes with a
`MINIMUM_CPU_UPDATE_INTERVAL` sleep; once gc runs on the shared tick `System`,
drive that spacing from the loop cadence instead of sleeping inside gc. The vscode
`VIGIL_VSCODE_PS_FIXTURE` seam is honored in `host_running(None)`'s live branch
(parity with bash `_vigil_vscode_ps`); keep that path reachable when the daemon
supplies a live snapshot so the ported integration tests can still inject it.

**Depends on.** 5.3, 5.4, 5.5.

**Recommended implementation model / agent strategy.** Opus, multi-pass
(implement → parity-review → full-suite gate). High difficulty due to
integration breadth and the launchd-race / TCC / exec footguns. One Opus
implementer with an explicit checklist of the must-preserve daemon + service
invariants, then a parity-review pass that runs the **entire** `tests/run.sh`
against the Rust binary and blocks the final Bash deletion until green.

---

### Phase 5.8 — Linux port: fill platform-trait impls behind stable macOS seams

**Goal.** After macOS is fully shipped and stable on Rust, implement the Linux
side of every platform trait. Purely additive `#[cfg(target_os = "linux")]`
impls — no refactoring of the now-stable seams.

**Rust deliverable.** `PowerController`/`CaffeinateAssertion`: `keepawake` →
logind `Manager.Inhibit('idle','block')` via `zbus` 5.x (idle-only for
`caffeinate -i` parity; no sleep-inhibit, no screensaver-inhibit).
`ProcessScanner`: `sysinfo` (/proc backend) + `procfs` 0.18 only if extra /proc
data is needed. `Locker`/`LockOverlay`: X11/Wayland stub → `_NET_WM_STATE_ABOVE`.
`ServiceInstaller`: `SystemdUserInstaller` (`service-manager` 0.11.0 for
enable/disable/start/stop lifecycle; generate the `.service` unit content
directly). `LogRotation`: logrotate.d drop-in + a `vigil reload-log` postrotate
subcommand to reinit the NonBlocking writer. Privilege boundary: file-based queue
ported as-is; `getpeereid`/`SO_PEERCRED` unused (file model retained uniformly).
`PowerGuard`: Linux sysfs/UPower equivalents behind the same trait.

**UX deliverable.** setup installs the systemd user unit + logrotate drop-in with
the same colored checklist; doctor reports Linux-specific readiness (logind
reachable, systemd user manager present). Same CLI/output substrate — only
Linux-accurate check messages.

**Security deliverable.** Same file-based boundary and exact-path allowlist with
Linux canonical paths. Document that `getpeereid` is intentionally NOT used; the
file-ownership model is the uniform boundary. systemd unit installed with
restrictive permissions.

**Crates.** keepawake 0.6.0, zbus 5.x, sysinfo 0.39.3, procfs 0.18.0
(linux-only, optional), service-manager 0.11.0 (systemd lifecycle), notify
(inotify backend).

**Parity tests / Bash retired.** Run the behavior-level portions of `tests/run.sh`
on Linux CI (process detection, activity, refcount, thermal/battery via fixtures,
IPC validation). Cannot be verified on the available Mac — gated on Linux CI.
Trait-contract tests shared with macOS ensure the Linux impls satisfy the same
invariants. **Bash retired: NONE** (all deleted by 5.7).

**Risks.** Cannot test on the available hardware — relies on CI. Non-systemd Linux
(OpenRC/runit) needs an explicit "unsupported" error path. logind block-inhibitor
semantics changed in systemd 257. `zbus` version diamond with `keepawake` (both
5.x) — run `cargo tree -d`.

**Depends on.** 5.7.

**Recommended implementation model / agent strategy.** Sonnet for the mechanical
trait-impl fill (seams and contracts fixed by the macOS slices), with an Opus
spot-check on the logind inhibitor lifecycle (FD-held-for-duration, Drop-releases)
since that is the one subtle correctness point. Lower difficulty than the macOS
slices — no design is open.

---

### Phase 5.9 — Windows port: fill platform-trait impls + in-process log rotation

**Goal.** Final trailing slice: implement the Windows side of every platform
trait. Additive `#[cfg(target_os = "windows")]` impls behind the stable seams.
Resolve the service-vs-Task-Scheduler decision when a Windows test machine is
available.

**Rust deliverable.** `PowerController`/`CaffeinateAssertion`: `keepawake` →
`SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` ONLY (no
`ES_AWAYMODE_REQUIRED`). `Locker`: `LockWorkStation` via `windows` 0.62.2
(interactive desktop only). `ProcessScanner`: `sysinfo` (Windows backend).
`LogRotation`: `logroller` 0.1.10 as `MakeWriter` (size 1MB, keep 5, gzip) gated
`cfg(windows)` since Windows has no newsyslog/logrotate. `ServiceInstaller`:
`WindowsTaskSchedulerInstaller` (logon trigger, per-user, avoids UAC —
recommended) OR `windows-service` 0.8.1 real service — decided on-device.
`LockOverlay`: `SetWindowPos HWND_TOPMOST` + GDI. Privilege boundary: file-based
queue retained; macOS `getpeereid` / Linux `SO_PEERCRED` both unused, file model
uniform.

**UX deliverable.** setup registers the Task Scheduler logon task (or service)
with the same colored checklist; doctor reports Windows-specific readiness.
Overlay renders the lock window topmost. Same CLI/output substrate.

**Security deliverable.** File-based boundary with Windows canonical paths;
document the Windows ACL equivalents for the request/response dir ownership matrix
(the macOS uid checks map to SID/owner checks). logroller-rotated logs in a
user-private dir. Test seams compiled out of release.

**Crates.** keepawake 0.6.0, windows 0.62.2 (Win32_System_Power,
Win32_UI_WindowsAndMessaging), sysinfo 0.39.3, logroller 0.1.10 (cfg windows),
windows-service 0.8.1 (if the real-service path is chosen).

**Parity tests / Bash retired.** Behavior-level `tests/run.sh` subset on Windows
CI (detection, refcount, IPC validation, log rotation). Overlay + LockWorkStation
manually verified on a Windows machine. Cannot be verified on the available Mac.
**Bash retired: NONE** (all deleted by 5.7).

**Risks.** No Windows hardware available; entirely CI + future on-device.
`SetThreadExecutionState` cannot block user-initiated sleep/lid-close — document
the weaker guarantee. `LockWorkStation` fails from a service context (argues for
Task Scheduler). `logroller` is single-maintainer — the `MakeWriter` seam keeps
it swappable; pin in `Cargo.lock`. The Windows file-ACL ownership model differs
from POSIX uid — the IPC validation needs a Windows-specific implementation, the
one genuinely non-mechanical part.

**Depends on.** 5.7.

**Recommended implementation model / agent strategy.** Sonnet for the mechanical
trait fills, but **Opus for the Windows IPC-dir ownership/ACL validation** (the
POSIX-uid-to-SID mapping is a real security-correctness translation, not a port)
and the service-vs-Task-Scheduler decision. Resource the ACL piece like a mini
privilege slice.

---

## 6. The unavoidable process-level coexistence window (honest caveat)

The strangler honors "no dual-runtime" at the **subsystem-source** level (one
impl per subsystem). It does NOT, and cannot, avoid a **process-level**
coexistence window: from 5.1 through 5.7 the Rust `vigil` binary (CLI) and the
Bash `vigil-daemon` (running under launchd) are both live and **share state
files** — `daemon.tick`, `baseline.json`, the active/ PID files, the lock state.

This must be managed deliberately, not pretended away:

- **The state-file ABI is frozen from 5.1.** The field names/format of
  `daemon.tick` (and the request/response/baseline files) are an implicit ABI
  between the two runtimes during the window. The Rust CLI reading a Bash-written
  tick file (and the 5.6 lock pre-arm reading it) must classify the **exact**
  current field set. 5.7 owns the tick-file schema, but **the freeze of that
  schema is a 5.1-era commitment** — no Rust slice may change a shared state-file
  field name until 5.7 owns both producer and consumer.
- This is the concrete mechanism by which the system bridges the two runtimes;
  the binary is **not** self-contained until 5.7.

---

## 7. Must-preserve inventory (consolidated checklist)

The rewrite cannot drop any of these. (Source refs live in the original audit;
this is the gate checklist.)

**Privilege boundary (root helper + IPC):**
- [ ] File-based request/response queue (NOT a unix socket); one resident root
      binary; request files in, response files out.
- [ ] Three actions only — engage/release/status — validated on BOTH sides
      (defense-in-depth, not optimized away).
- [ ] Fixed argv `/usr/bin/pmset -a disablesleep <0|1>`, absolute path, pinned +
      cleared env; release target from the root-owned baseline only.
- [ ] Request-file checks as a strict AND: not-symlink (O_NOFOLLOW), regular file,
      owner == ALLOWED_UID, nlink == 1, not group/other-writable, valid mode.
- [ ] Filename charset `^[A-Za-z0-9_.-]+$` (+ reject `.`/`..`) — the only response-
      path traversal guard; construct response paths via `openat` on a validated
      response-dir fd.
- [ ] Extra-content rejection: exactly one line (the action); any trailing
      content rejected.
- [ ] Atomic move-into-validated-root-0700-processing-dir BEFORE validation
      (claim-then-validate).
- [ ] Per-poll request-DIR ownership re-check; startup root-owned non-symlink
      response/state/log/processing dir checks — ALL via O_NOFOLLOW|O_DIRECTORY +
      fstat-on-fd (NOT `std::fs::metadata`).
- [ ] User-side response validation: root-owned, regular, non-symlink, not
      group/other-writable — via single O_NOFOLLOW open + fstat-on-fd, read body
      from the same fd.
- [ ] Every rejection consumes the request AND writes a `status=error` response
      (rejected ≠ timeout); queue never accumulates poison.
- [ ] Helper refuses non-root unless test-gated; test seams compiled OUT of the
      release root binary; `--allowed-uid`/`--allowed-user` install-time-fixed,
      numeric-validated, never from request content.
- [ ] helper.log path re-guarded as non-symlink before each append.
- [ ] Two separate 0600 files: `engaged` marker vs `baseline` value.
- [ ] All three baseline/SleepDisabled parsers fail SAFE → target 0 on
      corrupt/missing.

**Power state machine:**
- [ ] Idle-release no-op (release while not engaged does NOT run pmset — prevents
      clobbering a third-party setting).
- [ ] baseline.json stickiness; soft_release (thermal) KEEPS baseline; full
      release (battery / count==0) CLEARS it.
- [ ] Per-tick reconcile while engaged: reassert on externally-flipped
      SleepDisabled; respawn caffeinate if dead.
- [ ] caffeinate is `caffeinate -i` (NOT `-d`); any display-flag caffeinate
      (`-di`/`-dimsu`/clustered-d) is stale → replaced.
- [ ] caffeinate liveness by IDENTITY (basename == caffeinate, not group/other
      flag, start_ts) — not bare `kill -0`.
- [ ] Crash recovery: active + can_hold (thermal+battery evaluated at startup) ⇒
      reassert; else restore + clear.
- [ ] Engage order: capture baseline → helper engage → only-then spawn caffeinate;
      no caffeinate on engage failure.

**Detection / refcount:**
- [ ] Two-column ps equivalent preserved via `exe()` path matching (spaced-path
      fix); hard-exclusion patterns verbatim.
- [ ] Activity idle window: `ceil(secs/60)`, min 1 (BSD find-mmin granularity).
- [ ] vscode Copilot Chat: sha256 content-change gate (NOT mtime), primed-first-
      run suppression, `active_until` cache, `VIGIL_VSCODE_COPILOT_DISCOVER_SECS`
      throttle, both stable + Insiders roots.
- [ ] Refcount per-prefix activity gating; wrapper-ALWAYS-counts; GC branches
      (a) dead-PID, (b) PID-reuse (start_ts), (c) idle-CPU with wrapper carve-out
      from (c) ONLY.
- [ ] All `VIGIL_*_FIXTURE` / `VIGIL_VSCODE_PS_FIXTURE` test seams.

**Thermal / battery:**
- [ ] `VIGIL_FORCE` checked first (before any subprocess), overrides BOTH cutoffs.
- [ ] Thermal `=`-anchored signal; numeric-threshold knob DEFAULTS to exact
      any-presence parity.
- [ ] Sliding thermal cooldown re-arm on every pressure tick; cooldown
      independent of tick interval.
- [ ] Battery: AC = no-cut, unknown = no-cut, empty pct = no-cut, strict
      `pct < floor` (exactly 20% does NOT cut).

**Config / env / paths:**
- [ ] ALL `VIGIL_*` overrides + provider-home cascade (`CLAUDE_CONFIG_DIR`/
      `CODEX_HOME`/`COPILOT_HOME` → `VIGIL_*_HOME`), resolved once, re-derived
      after conf load; explicit `VIGIL_*_HOME` never clobbered.
- [ ] `VIGIL_LOG_FILE` re-derived after conf; install dir under
      `~/Library/Application Support` (NOT `~/Documents` — TCC).
- [ ] Exact-equality privileged-path allowlist; `cmd_assert_vigil_tree_path`
      guards.
- [ ] `VIGIL_INSTALL_DIR` must end in `/vigil`, absolute, not `/` or `$HOME`, no
      newline/CR.
- [ ] All default values (tick 5, stale-age 30, stale-cpu 0.5, thermal-cooldown
      60, battery-floor 20, start-wait 6, idle-after 300, lock-combo, lock-max
      28800).

**CLI / UX contracts:**
- [ ] Exit-code discipline: unknown command/subcommand/non-macOS = 64; status
      always 0; doctor 0 (incl. ready-with-warnings) / 1 (not installed / needs
      repair); lock-helper-absent = warning not error; doctor `--power` 0/1; lock
      64 vs 1 split; lock doctor 0/1 (post_event_access informational).
- [ ] `--json` flat schema (every documented key + `daemon_scan_state` enum +
      agents sub-object) + NEW top-level `version` field; byte-stable vs Bash
      golden.
- [ ] `--max-secs 0` accepted ONLY via explicit CLI, rejected from
      `VIGIL_LOCK_MAX_SECS=0` config; CLI is the sole gate (helper is permissive).
- [ ] lock pre-arms sleep hold + waits for the tick to reflect it BEFORE
      countdown/exec; 3-2-1 countdown.
- [ ] `vigil run`: NON-exec wrapper PID file, trap cleanup on EXIT/INT/TERM/HUP,
      child exit-code propagation.
- [ ] setup `--dry-run`/`--verbose`; setup silent `cmd_stop` first; uninstall
      5-step order + zero-flag strict + logs preserved; reload = full
      bootout/bootstrap (NOT kickstart); stop's 50×100ms bootout poll; start's
      bounded "pending"-not-error wait.
- [ ] `--version` exact `vigil <VERSION>`; legacy sudoers cleanup on
      setup/uninstall.

**Infra:**
- [ ] launchd labels hardcoded; LaunchAgent + root LaunchDaemon plists generated
      typed (XML-escaped); TCC copy-out-of-Documents.
- [ ] Single-instance via atomic mkdir lock dir + PID-liveness stale recovery (NOT
      flock).
- [ ] Atomic tick-file write (tmp + rename); file-as-source-of-truth logging;
      per-OS native rotation (newsyslog macOS).
- [ ] State files chmod 0700; asymmetric IPC dir ownership matrix.

---

## 8. Crate appendix

Confidence reflects the research cheat-sheet. **Versions and any low-confidence /
"linchpin" crates must be confirmed at implementation time** (a one-time
`cargo add` / registry check before 5.1, and again for `objc2-core-graphics`
before 5.6).

| Crate | Version | Confidence | Role |
|-------|---------|-----------|------|
| clap | 4.6.1 | high | CLI parse + subcommand dispatch |
| clap_complete | 4.6.5 | high | shell completions (`vigil completions <shell>`) |
| anstream | 1.0.0 | high | ANSI stream wrapper; NO_COLOR/CLICOLOR/non-tty stripping |
| owo-colors | 4.3.0 | high | color/style attributes at call sites |
| colorchoice-clap | 1.0.8 | high | `--color=auto\|always\|never` mixin |
| indicatif | 0.18.4 | high | spinners (setup/doctor); lock countdown render |
| dialoguer | 0.12.0 | high | interactive prompts in setup/doctor auto-fix |
| comfy-table | 7.2.2 | high | tabular status/doctor output |
| serde | 1.0.228 | high | (de)serialize all round-trip types |
| serde_json | 1.0.150 | high | `--json` + internal state files |
| figment | 0.10.19 | high | layered config (defaults < toml < env < CLI) |
| toml | 0.8.x (via figment) | high | parse vigil.conf as TOML |
| dirs | 6.0.0 | high | platform state/log/config dirs |
| tracing | 0.1.44 | high | instrumentation facade |
| tracing-subscriber | 0.3.23 | high | fmt + env-filter layers |
| tracing-appender | 0.2.5 | high | NonBlocking writer (NOT its rotator) |
| logroller | 0.1.10 | medium | in-process rotation, Windows only (single-maintainer; keep swappable) |
| keepawake | 0.6.0 | high | cross-OS sleep-prevention facade (Linux/Windows) |
| sysinfo | 0.39.3 | high | fork-free process scan (sets MSRV ~1.95) |
| notify | 8.2.0 | high | FS event watcher (FSEvents/inotify/RDCW) |
| notify-debouncer-mini | 0.7.0 | medium | optional debounce for the vscode probe |
| procfs | 0.18.0 | low | Linux-only /proc extras (5.8, optional) |
| nix | 0.31.3 | high | O_NOFOLLOW open, fstat, getpeereid (unused), syscalls |
| libc | 0.2.186 | high | O_NOFOLLOW / macOS constants |
| plist | 1.9.0 | high | typed LaunchAgent/LaunchDaemon plist gen (sets MSRV ~1.88) |
| objc2 | 0.6.4 | high | ObjC runtime bridge, MainThreadMarker, Retained |
| objc2-foundation | 0.3.2 | high | NSString/NSRect/NSColor geometry+string types |
| objc2-app-kit | 0.3.2 | high | NSWindow/NSApplication overlay |
| objc2-core-foundation | 0.3.2 | high | CFString/CFDictionary/CFTypeRef |
| objc2-core-graphics | 0.3.x | **unverified** | **CGEventTap family for the 5.6 migration option (a) — confirm exists/version before 5.6; linchpin of the "one CF family" goal** |
| objc2-io-kit | 0.3.2 | high | IOKit (if migrating existing IO usage) |
| core-text | 22.0.0 | medium | overlay label text (only if NSTextField is insufficient) |
| zbus | 5.16.0 | high | Linux logind Inhibit (5.8) |
| windows | 0.62.2 | high | Win32 SetThreadExecutionState / LockWorkStation (5.9) |
| service-manager | 0.11.0 | low | DEFERRED — Linux systemd lifecycle only; NOT macOS |
| windows-service | 0.8.1 | medium | DEFERRED — Windows real-service path (5.9, if chosen) |
| cargo-dist | 0.32.0 | high | DEFERRED — packaging once the release gate lifts |

---

## 9. Open decisions / Phase 6 deferrals

**Open decisions to settle in the named slice:**
- **5.6 — CF-family migration depth:** migrate `core-graphics 0.25.0` →
  `objc2-core-graphics` (achieves "one CF family", larger unsafe re-port) **or**
  descope the goal and keep two CF families. Confirm `objc2-core-graphics`
  exists at a usable version first.
- **5.6 — overlay run-loop cadence:** the existing tap loop already pumps the
  standard `kCFRunLoopDefaultMode` (tap source on `CommonModes`), which services a
  default NSWindow — so the open item is AppKit init/cadence, NOT a mode mismatch:
  confirm `NSApplication` is launched and the short `run_in_mode` slices redraw the
  countdown smoothly (else drive redraw per tick). Hard acceptance check:
  "countdown visibly updates, window stays above dock/menu bar".
- **5.6 — `CGEventTapLocation::HID` vs `Session`:** resolve and document the
  research-flagged discrepancy.
- **5.6 — lock-helper binary boundary:** keep `vigil-lock-helper` separate
  (recommended, pins TCC grant) vs merge into `vigil` (simpler packaging, forces
  main-binary grants). Final call in 5.6.
- **5.1/5.7 — `clap_complete` install policy on macOS w/o Homebrew:** brew prefix
  if detected, else `~/.zsh/completions` + fpath hint, else print-only.
- **5.7 — `--json` schema versioning:** top-level `version` field (always 1 for
  now).
- **5.2/5.6 — `VIGIL_FORCE` / `VIGIL_LOCK_MAX_SECS` typing:** settle the serde
  representation (`u8` 0/1) and document accepted values in doctor.
- **5.9 — Windows service vs Task Scheduler logon-trigger:** decided on-device;
  `ServiceInstaller` accommodates both.
- **vigil.conf TOML cutover timing:** ensure no surviving Bash subsystem sources
  vigil.conf after 5.2 ships the TOML parser. (Resolved by the dead-from-Rust
  model: the Bash that reads it stays sourcing the Bash config path until 5.7;
  the Rust path uses TOML from 5.2. No Bash re-reads the TOML.)
- **cargo-dist + Homebrew tap:** configured in `Cargo.toml` metadata but NOT
  activated until the ROADMAP "no release until all phases ship" gate lifts.

**Phase 6 deferrals (explicitly OUT of this rewrite — do NOT create 5.x
sub-phases for them):**
- Menu-bar / tray status item.
- Full GUI app.
- **Merge `doctor` and `status`** into one command. 5.7 unifies the underlying
  `CheckEngine` so the merge is then trivial; the two commands stay separate for
  now.

---

## 10. When each sub-phase begins

Per repo policy, **each `5.x` sub-phase gets its own detailed implementation doc
before any code is written for it** (in the voice/rigor of
`future/phase-4-lock-feature.md`). This umbrella plan is the map; the per-slice
doc is the territory. That detailed doc is where line-level decisions live:
exact struct/field layouts, the full fixture-to-cargo-test mapping, the precise
clap arg surface, and the slice's gate-0 golden-fixture capture step. No slice
deletes (or marks dead-from-Rust) its Bash until: (1) its `cargo`-test rewrite
passes, (2) its golden fixtures match the captured Bash output, and (3) the FULL
`tests/run.sh` is green — and the retirement lands as a separate, independently-
revertable commit.
