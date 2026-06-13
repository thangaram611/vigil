# Phase 5.7 — Daemon loop + service mgmt (launchd) + setup/uninstall/reload + unified CheckEngine; final Bash cutover

> **Status: DETAILED IMPLEMENTATION PLAN.** Ready for the implementation worker.
> Per repo policy (umbrella §10) this is the per-slice doc that must exist before
> any 5.7 code is written. The umbrella plan (`future/phase-5-rust-rewrite.md`
> §5.7, §7) is the obligation map; this document is the territory — exact
> struct/field layouts, the fixture→cargo-test mapping, the clap surface, the
> gate-0 golden-fixture capture, and the staged, per-commit, independently-
> revertable cutover ending in the irreversible Bash deletion.

This slice is written against six ground-truth contracts extracted from the live
bash (`bin/vigil`, `bin/vigil-daemon`, `lib/*.sh`) and the already-ported Rust
module APIs (`crates/vigil/src/*`). Every `file:line` citation below is verified
against the current tree.

---

## Decisions resolved (post-design review — locked by user, 2026-06-13)

These resolve the §9 open questions before implementation. Where a default is
correct without user input it is recorded here; only **Q1** required the user.

- **Q1 — Lock cutover → OPTION A (pull the lock CLI into 5.7).** `cmd_lock` +
  `cmd_lock_doctor` are ported to Rust in THIS slice (new **Commit 7**, before
  the deletion). Rationale: `bin/vigil` sources all 7 libs at startup and
  `cmd_lock` needs config/refcount, so leaving lock in bash forces keeping
  `bin/vigil` + libs alive — defeating "self-contained after 5.7." Net: **ALL
  bash (incl. `bin/vigil` + `shim.rs`) is deleted in 5.7.** 5.6 shrinks to the
  overlay window + `core-graphics`→`objc2-core-graphics` migration only
  (`vigil-lock-helper`-internal, no bash). The overlay stays in 5.6 because it is
  manual-verify-only and must not gate the irreversible deletion; the migration
  is gated on an unverified crate. The lock port covers: combo/max-secs parse,
  the `--max-secs 0` CLI-only guard, 3-2-1 countdown, the 64-vs-1 exit matrix,
  lock doctor readiness (computed alignment), and pre-arm-then-wait reading the
  new Rust tick file (§4.12 below).
- **Q2 — `version` field → `1`**, the FIRST `--json` key with a trailing comma;
  bump on any key add/remove/reorder. Sole allowed diff vs the bash golden.
- **Q3 — MSRV →** set the workspace `rust-version` to the higher of `sysinfo`'s
  and `plist`'s actual `rust-version` (verify with the freshly-added crate;
  floor ~1.95). Toolchain is 1.96.0. Add an explicit `rust-version`.
- **Q4 — `assert_vigil_tree_path` Documents-exclusion → ADD** the 6th rule, but
  the commit message flags it as a **hardening delta, not parity**. Verify the
  sandbox test fixtures do NOT live under `~/Documents`.
- **Q5 — privileged paths → extend to all 14** (bash-faithful), not 11.
- **Q6 — bootout/bootstrap 50×100ms poll → keep exactly;** the helper bootout has
  NO poll (asymmetry preserved). Do not "optimize" either.
- **Q7 — clap_complete → no auto-install** in setup; completions stay the
  explicit `vigil completions <shell>` the user pipes.
- **Q8 — plist optional keys → `skip_serializing_if` on every optional;** assert
  the rendered plist byte-matches the golden.
- **Q9 — one vs two `System`s → default two** on the daemon (simplest correct);
  optimize to one only if proven. Never blocks the slice.
- **Q10 — signal-safe cleanup → `AtomicBool` flag** checked at loop top +
  interruptible sleep; cleanup runs on the main thread; verify the `ExitTimeOut`
  window covers a slow release.

**Revised commit sequence:** the §6.3 sequence gains **Commit 7 — swap lock
native** (port `cmd_lock` + `cmd_lock_doctor`; overlay stays 5.6), and the
final-deletion **Commit 8** now also removes `bin/vigil` + `shim.rs` (full bash
deletion, binary self-contained).

---

## 0. Where 5.7 sits / current state

- The Rust `vigil` binary currently **SHIMS** ten subcommands to bash via
  `shim::exec_bash(...)` (`main.rs:257-266`): `setup, uninstall, start, stop,
  status, log, run, reload, lock, doctor`. Only `Completions`, `Config`, `Debug`
  are native (`main.rs:245-256`). There is **no `daemon` subcommand** in the
  `Command` enum yet — the daemon is still the bash `bin/vigil-daemon` script.
- The 5.2–5.5 subsystem cores are already ported and unit-tested: config,
  procscan/detect, activity (scan + vscode), refcount, thermal, battery,
  power_guard, the `PowerMachine`, `MacHelperClient`, the read-only `PowerView`
  and `debug::assemble`. 5.7 **WIRES** these — it does not re-derive them.
- `Lock` STAYS shimmed through 5.7 (per `MEMORY.md` rewrite cadence; the lock
  pre-arm path depends on the 5.7 tick-file fields, so 5.6 sequences AFTER 5.7).
  Everything else listed above goes native in this slice.
- After 5.7 the `vigil` binary is self-contained (plus the two helper binaries
  `vigil-lock-helper`, `vigil-root-helper`). `shim.rs` and ALL remaining bash are
  physically deleted as the final, separate, revertable commit.

**The thermal framing must not regress.** The "one resident System" requirement
is an **efficiency / attack-surface** win (one process vs. ~12–20 forks/tick),
NOT a thermal fix. Never claim Rust cools the machine (umbrella §4, lines 52–57).

---

## 1. Goal + scope recap

**Land four things, then delete bash:**

1. **`src/daemon/`** — the resident tick loop: single-instance `mkdir`-lock guard
   with PID-liveness stale recovery (NOT `flock`), startup crash recovery, the
   exact `desired=1` predicate, the engage/reconcile/soft-vs-full-release
   branches, the byte-stable tick-file writer, INT/TERM cleanup, and the
   **one long-lived `ProcScanner`/`System`** threaded through detect + vscode
   host-check + gc each tick.
2. **`src/service/`** — `trait ServiceInstaller` + `MacosLaunchdInstaller`:
   typed `plist`-crate structs for the user LaunchAgent + root LaunchDaemon, the
   bootout → poll-up-to-50×100ms → bootstrap dance (NOT `kickstart -k`), the TCC
   copy-out-of-Documents, the asymmetric IPC dir ownership matrix.
3. **`src/check/`** — ONE `CheckEngine` producing `Vec<Check>`, consumed by BOTH
   `vigil doctor` (three-state) and `vigil status` (always exit 0, `--json` flat
   schema with the new top-level `version` field). Includes the read-only
   `vigil_assertions_summary` tri-state parser ported into the status render path.
4. **The 7 command ports** — native `cmd_setup`, `cmd_uninstall`, `cmd_reload`,
   `cmd_start`, `cmd_stop`, `cmd_run` (NON-exec), `cmd_log` (paging/line-limit),
   plus `cmd_status`/`cmd_doctor` on the CheckEngine.

**Then the cutover:** golden-fixture the bash output, port every bash `*_test.sh`
to cargo (or prove an existing cargo test covers it), get the **full
`tests/run.sh` green against the Rust binary**, and only then `rm` `bin/vigil`,
`bin/vigil-daemon`, `lib/{common,pmset,detect,activity,refcount,thermal,
battery}.sh`, and `shim.rs`.

---

## 2. Module designs

### 2.1 `src/daemon/` — the resident tick loop

#### 2.1.1 Entry point: a hidden `daemon` subcommand

The LaunchAgent execs a single binary. To keep one shipped binary, add a HIDDEN
subcommand `daemon` to the `Command` enum (clap `#[command(hide = true)]`), not a
new `[[bin]]`. The plist's `ProgramArguments` becomes
`[ "<install_dir>/bin/vigil", "daemon" ]` (replacing the bash
`@VIGIL_DAEMON_PATH@` → `bin/vigil-daemon`). `main.rs:dispatch` routes
`Command::Daemon => daemon::run()` — and `daemon::run()` **never returns** except
via the signal-driven `exit(0)`/error `exit(1)` paths.

> Note: `@VIGIL_DAEMON_PATH@` in the bash plist resolved to
> `$VIGIL_INSTALL_DIR/bin/vigil-daemon` (Contract 2 §1a). In Rust it resolves to
> the **installed `vigil` binary** at `$VIGIL_INSTALL_DIR/bin/vigil` with the
> `daemon` argument. The install copy (NOT `~/Documents`) is mandatory for TCC.

#### 2.1.2 The daemon struct

```rust
// src/daemon/mod.rs
pub struct Daemon {
    cfg: VigilConfig,                         // resolved once, fields read directly
    scanner: ProcScanner,                     // THE one long-lived sysinfo::System
    sys_for_gc: sysinfo::System,              // see §3.1 (one System, two refresh scopes)
    machine: PowerMachine<'static-ish,        // PowerMachine over the Mac seams
        MacHelperClient, MacCaffeinate, MacSleepReader>,
    guard: EnvPowerGuard,                     // built from cfg; force()=VIGIL_FORCE
    engaged: bool,                            // DAEMON_ENGAGED (init false)
    cooldown_until: i64,                      // COOLDOWN_UNTIL epoch secs (init 0)
    lock: DaemonLock,                         // RAII lockdir guard (§2.1.5)
    vscode_state_file: PathBuf,               // cfg.vscode_copilot_state_file
}
```

Lifetime/borrow note: `PowerMachine<'a, …>` borrows its three seam impls
(`power/mod.rs:101`). Construct the seams as owned fields on a `DaemonSeams`
struct held alongside (or `Box`/`Arc`) so the `PowerMachine` borrow is valid for
the daemon's whole life. The implementer chooses the exact ownership shape; the
constraint is that `engage/full_release/soft_release/reconcile_engaged/
recover_startup` are callable every tick without reconstructing the machine.

#### 2.1.3 The per-tick order (EXACT — Contract 1 §2, Contract 6 §9)

Each tick, in this precise order:

1. **Detect + touch.** `let matches = self.scanner.detect();` (`procscan/mod.rs:108`
   — internally `collect()` then `detect_line` per record). For each `AgentMatch`
   with a non-empty pid: write `refcount::pidfile_body(name, pid, exe, start_ts)`
   (`refcount/mod.rs:74`) to `{active_dir}/{name}-{pid}.pid`. There is **no
   `touch()` helper** — the daemon writes the file itself (Contract 6 §4). `name`
   is `AgentMatch.kind.name()` → `cli-claude`/`cli-codex`/`cli-copilot`/
   `app-codex`/`app-vscode-copilot-chat` (`detect.rs:30-39`). `start_ts` per the
   refcount module's pid-start convention.
2. **GC.** `refcount::gc(active_dir, &mut self.sys_for_gc, stale_age_secs,
   stale_cpu_pct, now)` (`refcount/mod.rs:288`). See §3.1 for the
   `MINIMUM_CPU_UPDATE_INTERVAL` spacing — driven from loop cadence, not an
   in-gc sleep, on the shared System.
3. **Per-agent activity (computed once/tick).**
   - `claude_active = scan::is_active(session_dir(claude), pattern(Claude),
     idle_after_sec, now)` (`activity/scan.rs:123`); same for codex, copilot.
   - `vscode_active = vscode::chat_is_active(copilot_home? /* see note */,
     vscode_state_file, now, idle_after_sec, discover_secs, recent_mins,
     ps_override=None)` (`vscode.rs:319`). `ps_override=None` keeps the
     `VIGIL_VSCODE_PS_FIXTURE` seam reachable in `host_running`'s live branch
     (`vscode.rs:211-221`) — see §3.1.
4. **Activity-filtered count.** `let count = refcount::count(active_dir,
   claude_active, codex_active, copilot_active, vscode_active)`
   (`refcount/mod.rs:179`). Wrappers always count (unconditional +1, Contract 3).
5. **Cutoff checks.** `let now = vigil_now_unix();` (epoch secs).
   - `cut_thermal = thermal::live_should_cut(force, &thermal::read_therm_raw(),
     cfg.thermal_cpu_limit_floor)` (`thermal/mod.rs:158,176`).
   - `cut_battery = battery::live_should_cut(force, &battery::read_ps_raw(),
     cfg.battery_floor_pct)` (`battery/mod.rs:117,133`).
   - **Cooldown re-arm + cooling**, in one pure call (replaces the two bash
     statements): `let (cooldown_until, cooling) =
     power_guard::cooldown_state(now, cut_thermal, self.cooldown_until,
     cfg.thermal_cooldown_secs)` (`power_guard/mod.rs:152`). Store
     `self.cooldown_until = cooldown_until`. `cooling` is the bool. This is the
     sliding window: each thermal-pressure tick re-arms `now + cooldown_secs`
     (Contract 1 §4).
6. **Decide** (§2.1.4).
7. **Act** (§2.1.4 branch table).
8. **Write tick file** (§2.1.6) with the **post-action** `engaged`.
9. **Sleep** `tick_secs`.

#### 2.1.4 The desired-hold predicate + act branches

**Predicate — VERBATIM from the bash contract (Contract 1 §2, `vigil-daemon:156-159`):**

```text
desired=0
if (( count > 0 && cut_thermal == 0 && cut_battery == 0 && cooling == 0 )); then
    desired=1
fi
```

Rust:

```rust
let desired = count > 0 && !cut_thermal && !cut_battery && !cooling;
```

**Act branches — four mutually-exclusive arms on `(desired, engaged)`** (Contract
1 §3, `vigil-daemon:161-186`). Release-reason priority is load-bearing:
**thermal → SOFT (keep baseline) > battery → FULL > count==0 → FULL.**

| `(desired, engaged)` | Action | `engaged` after |
|---|---|---|
| `(true, false)` | log `engage — count=… thermal=ok battery=ok claude=… …`; `machine.engage(now)` (`power/mod.rs:187`) | `true` **iff** `engage` returns `Ok`; else stays `false` |
| `(true, true)` | `machine.reconcile_engaged()` (`power/mod.rs:219`) | stays `true`; set `false` **only if** it returns `Err` |
| `(false, true)` | priority sub-branch ↓ | `false` (always) |
| `(false, false)` | no-op | unchanged |

Sub-branch when `(false, true)`:
```text
if cut_thermal { log WARN "release — thermal cutoff (cooldown {N}s)"; machine.soft_release(); }
else if cut_battery { log WARN "release — battery floor ({battery_summary})"; machine.full_release(); }
else if count == 0 { log INFO "release — no active agents"; machine.full_release(); }
engaged = false;  // always, even if a release no-ops
```
`soft_release` keeps `baseline.json` (`power/mod.rs:210`); `full_release` clears
it (`power/mod.rs:201`). `battery_summary` from `battery::battery_summary`
(`battery/mod.rs:103`).

#### 2.1.5 Single-instance guard — atomic `mkdir` lock dir (NOT flock)

macOS has no `flock(1)`. The guard is an atomic-`mkdir` directory lock (Contract
1 §1, `vigil-daemon:36-54`; umbrella §7 line 113).

```rust
// src/daemon/lock.rs
pub struct DaemonLock { dir: PathBuf }   // {state_dir}/state.lock.d

impl DaemonLock {
    /// Returns Acquired(self) | LiveContention | TookOver(self) | Failed.
    pub fn acquire(lock_file: &Path, my_pid: u32) -> LockOutcome { … }
}
```

Algorithm (byte-faithful):
- `dir = lock_file + ".d"` → `{state_dir}/state.lock.d`.
- `std::fs::create_dir(&dir)` succeeds atomically iff absent → **acquired**.
- On `AlreadyExists`: read `{dir}/pid`. If non-empty AND `kill(other, 0)` (signal
  0 liveness via `nix::sys::signal::kill`) succeeds → **live contention**: log
  `WARN "another vigil-daemon (pid={other}) holds {dir} — exiting"` and
  `exit(0)` (clean). Else **stale**: log `WARN "stale lock at {dir}
  (pid={other?} not running) — taking over"`, `remove_dir_all(&dir)`, retry
  `create_dir`. If retry fails → `ERROR "could not acquire lock"; exit(1)`.
- After acquiring: write `{dir}/pid` = `my_pid`; write `{daemon_pidfile}` =
  `my_pid`; **`remove_file(daemon_tick_file)`** (drop a previous run's stale
  snapshot so consumers never read it).
- `Drop`/cleanup removes the lock dir (see §2.1.7).

Respawn-safe: launchd `KeepAlive` respawns a clean `exit(0)`; the
`ThrottleInterval=10` plus the live-contention `exit(0)` tolerate the overlap.

#### 2.1.6 Tick-file write — FROZEN ABI (Contract 1 §5, Contract 6 §9)

Atomic tmp+rename: write `{tick_file}.{pid}`, then `rename` over `daemon.tick`.
**Exactly nine `key=value\n` lines, in this order, no JSON, no quoting, no extra
fields, no trailing blank line:**

```text
pid=<daemon pid>
updated_at=<now epoch secs>
tick_secs=<cfg.tick_secs>
refcount_active=<count>
desired_hold=<0|1>
engaged=<0|1, POST-action>
thermal_cut=<0|1>
battery_cut=<0|1>
cooling=<0|1>
```

`engaged` is the value AFTER the act-branch mutates it. The reader
(`cmd_daemon_tick_field`, `vigil:395-399`) does `awk -F=` first-match, so `=`
must be the first separator and one field per line. `pid` and `updated_at` MUST
be byte-faithful: a wrong `pid` permanently classifies the scan as
`pending`/`missing`; a non-numeric `updated_at` does the same (Contract 1 §5,
Contract 4 §1a). The scan-state classifier reads only `pid`, `updated_at`,
`tick_secs`; the lock pre-arm (5.6) reads `refcount_active`, `engaged`,
`thermal_cut`, `battery_cut`, `cooling`. Freeze all nine.

```rust
// src/daemon/tick.rs
pub fn write_tick(tick_file: &Path, t: &TickSnapshot) -> io::Result<()> {
    let tmp = tick_file.with_extension(format!("tick.{}", t.pid));
    let body = format!(
        "pid={}\nupdated_at={}\ntick_secs={}\nrefcount_active={}\n\
         desired_hold={}\nengaged={}\nthermal_cut={}\nbattery_cut={}\ncooling={}\n",
        t.pid, t.updated_at, t.tick_secs, t.refcount_active,
        b(t.desired_hold), b(t.engaged), b(t.thermal_cut),
        b(t.battery_cut), b(t.cooling));  // b(): bool -> 0|1
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, tick_file)
}
```

#### 2.1.7 Startup recovery, signals, shutdown

**Pre-flight (after lock, before loop):** call `cfg.ensure_state_dir()`
(`config/mod.rs:763` — creates state/active/log, chmod 0700 state). Then a
helper round-trip: `machine`'s `MacHelperClient::status()`; if it returns
`IpcError::DirsMissing` (`ipc/mod.rs:227-229`) → `ERROR "root helper is not
available — run 'vigil setup' or 'vigil doctor'"; exit(1)` (Contract 1 §1,
Contract 6 §7).

**Crash recovery before the loop** (Contract 1 §6, `vigil-daemon:95-115`):
1. Refresh evidence FIRST — run the same detect→write-pidfile→`gc` pass so a
   leftover baseline is judged against CURRENT work.
2. Compute `startup_count` via the same activity-filtered `refcount::count`.
3. `startup_can_hold = !thermal_should && !battery_should` — uses
   `EnvPowerGuard` (force respected) or the live `should_cut` calls; this
   evaluates thermal AND battery at startup.
4. If `baseline_file` exists: `engaged =
   machine.recover_startup(startup_count, &guard, now)` (`power/mod.rs:244`,
   returns `true` iff engaged).

**Signals.** Install INT/TERM handlers (not HUP). On INT/TERM:
`cleanup_and_exit`:
1. log `INFO "shutting down — releasing sleep prevention and cleaning up"`.
2. if `engaged` → `machine.full_release()` (restore baseline + kill caffeinate +
   clear baseline).
3. `remove_file(daemon_pidfile)`, `remove_file(daemon_tick_file)`.
4. `remove_dir_all(lock_dir)`.
5. `exit(0)`.

Implementation note: a Rust signal handler cannot safely call most of this
directly. Use a self-pipe / `signal_hook`-style flag (or set an
`AtomicBool` and check it at the top of each loop iteration and during the
sleep) so the cleanup runs on the main thread. The launchd `ExitTimeOut=60`
gives this path time before SIGKILL. Do NOT trap HUP.

---

### 2.2 `src/service/` — `ServiceInstaller` + `MacosLaunchdInstaller`

#### 2.2.1 The trait (portable seam — Linux fills it in 5.8)

```rust
// src/service/mod.rs
pub trait ServiceInstaller {
    /// Render + write the user agent plist to its canonical path.
    fn install_user_agent(&self, paths: &VigilConfig) -> Result<(), ServiceError>;
    /// Render the user agent plist to a String (for setup --verbose / --dry-run).
    fn render_user_agent(&self, paths: &VigilConfig) -> Result<String, ServiceError>;
    /// Render + write the root LaunchDaemon plist (sudo-installed by the caller).
    fn render_helper_daemon(&self, paths: &VigilConfig) -> Result<String, ServiceError>;
    /// Bootstrap (load) the user agent: bootstrap + enable (best-effort).
    fn start_user_agent(&self, label: &str, plist: &Path) -> Result<StartState, ServiceError>;
    /// Bootout the user agent with the 50×100ms poll. Idempotent.
    fn stop_user_agent(&self, label: &str) -> Result<StopState, ServiceError>;
    /// True iff the agent is currently loaded (launchctl print succeeds).
    fn is_loaded(&self, label: &str) -> bool;
}

pub struct MacosLaunchdInstaller;   // #[cfg(target_os = "macos")]
```

`StartState { AlreadyLoaded, Bootstrapped }`, `StopState { BootedOut,
NotLoaded }` so the command layer can print the exact bash strings.

#### 2.2.2 Typed plists via the `plist` crate (NOT heredocs)

Add `plist = "1.9.0"` (umbrella §5.7 line 97; crate appendix). Model both plists
as `#[derive(Serialize)]` structs; the `plist` crate XML-escapes automatically —
**no manual `cmd_plist_escape`** (which was XML-escape THEN sed-escape,
`vigil:54-74`). Use `#[serde(skip_serializing_if = "Option::is_none")]` on every
optional key — **launchd uses key ABSENCE** (umbrella §5.7 line 125 risk).

**User LaunchAgent** (Contract 2 §1a, template `etc/com.thangaram.vigil.plist.in`):

```rust
#[derive(Serialize)]
struct UserAgentPlist {
    #[serde(rename = "Label")]            label: String,              // "com.thangaram.vigil" (hardcoded)
    #[serde(rename = "ProgramArguments")] program_arguments: Vec<String>, // [ "<install>/bin/vigil", "daemon" ]
    #[serde(rename = "RunAtLoad")]        run_at_load: bool,          // true
    #[serde(rename = "KeepAlive")]        keep_alive: bool,           // true
    #[serde(rename = "ProcessType")]      process_type: String,       // "Background"
    #[serde(rename = "ExitTimeOut")]      exit_timeout: i64,          // 60
    #[serde(rename = "ThrottleInterval")] throttle_interval: i64,     // 10
    #[serde(rename = "StandardOutPath")]  stdout_path: String,        // "{log_dir}/daemon.out.log"
    #[serde(rename = "StandardErrorPath")] stderr_path: String,       // "{log_dir}/daemon.err.log"
    #[serde(rename = "EnvironmentVariables")] env: BTreeMap<String,String>,
}
```

`EnvironmentVariables` (order preserved by emitting an explicit ordered map):
`PATH = /opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`,
`VIGIL_STATE_DIR = {state_dir}`, `VIGIL_LOG_DIR = {log_dir}`.

**Root LaunchDaemon (helper)** (Contract 2 §1b, template
`etc/com.thangaram.vigil.helper.plist.in`): Label
`com.thangaram.vigil.helper`; `ExitTimeOut = 10` (NOT 60); `ThrottleInterval =
10`; stdout/stderr `{power_log_dir}/helper.{out,err}.log`; **NO
EnvironmentVariables dict**. `ProgramArguments` is the FROZEN 14-element argv the
helper validates (order exact):

```text
{root_helper}  --serve
  --request-dir   {power_request_dir}
  --response-dir  {power_response_dir}
  --state-dir     {power_state_dir}
  --log-file      {power_log_file}
  --allowed-uid   {uid}            (raw integer)
  --allowed-user  {username}
```

Labels are HARDCODED in both the `Label` field and (because the templates put
the literal in the `<string>`) must be emitted verbatim — the Rust structs hold
the constant strings, never an overridable var (Contract 2 §0).

#### 2.2.3 bootout → poll-50×100ms → bootstrap (NOT kickstart)

`stop_user_agent` (Contract 2 §2, `vigil:848-871`):
```text
domain = "gui/{uid}"
if launchctl print "{domain}/{label}" succeeds:
    launchctl bootout "{domain}/{label}"        (ignore error)
    for _ in 0..50:                             # EXACTLY 50
        if launchctl print "{domain}/{label}" fails { break }
        sleep 100ms                             # EXACTLY 100ms
    remove daemon.tick
    -> BootedOut
else:
    remove daemon.tick (best-effort)
    -> NotLoaded
```
The bound is 5s. **This poll MUST stay** — fast machines fail setup/reload
without it (umbrella §5.7 risk). `sleep 100ms` here is genuine wall-clock waiting
in a command path (not the daemon loop), so a real `std::thread::sleep` is
correct.

`start_user_agent` (Contract 2 §2, `vigil:833-846`): die if plist absent; if
already loaded → `AlreadyLoaded` (idempotent); else `launchctl bootstrap
{domain} {plist}`, then `launchctl enable {domain}/{label}` (best-effort, ignore
failure), then the bounded first-scan wait (§2.3.5). NO `kickstart`.

**Root helper** bootout/bootstrap (Contract 2 §2, `vigil:218-220`): `sudo
launchctl bootout system/{helper_label}` (ignore) → `sudo launchctl bootstrap
system {helper_plist}` → `sudo launchctl enable system/{helper_label}`
(best-effort). NO 50×100ms poll on the helper (asymmetry preserved).

#### 2.2.4 Why bootout/bootstrap, never `kickstart -k`

`kickstart` restarts the process but launchd keeps the CACHED plist, so plist
changes never take effect; reload MUST re-read the plist via bootout/bootstrap
(Contract 2 §2, comment `vigil:1262-1266`). The only `kickstart` mention in the
entire codebase is that negative-rationale comment. The Rust port keeps it that
way.

---

### 2.3 `src/check/` — the unified CheckEngine

#### 2.3.1 The `Check` shape

```rust
// src/check/mod.rs
pub enum Severity { Info, Ok, Warn, Error }

pub struct Check {
    pub group: &'static str,   // "dependencies","privileged helper","user agent",…
    pub label: String,         // "caffeinate", "LaunchAgent", "lock helper", …
    pub severity: Severity,
    pub detail: String,        // "ok", "missing (run vigil setup)", "ok (mode 700)"…
    pub install_marker: bool,  // contributes to install_markers (LaunchAgent/daemon/state dir)
}

pub struct CheckReport {
    pub checks: Vec<Check>,
    pub errs: u32,             // count(Severity::Error)
    pub warns: u32,            // count(Severity::Warn)
    pub install_markers: u32,  // count(install_marker && severity != Error)
    pub snapshot: StatusSnapshot,   // the data model below (for --json + status text)
}
```

`CheckEngine::run(cfg, mode)` returns a `CheckReport`. `mode` selects which
groups to populate (full doctor vs `--power` subset vs status). Both `doctor` and
`status` consume the SAME engine — doctor renders the grouped checklist + the
three-state resolution; status renders the operational blocks / `--json` from
`snapshot`. The commands stay separate (umbrella decision §1.6) but share the
engine so a future merge is trivial.

#### 2.3.2 The `StatusSnapshot` data model

Build on the existing read-only model. `debug::assemble(cfg, now)`
(`debug/mod.rs:85`) already gathers per-agent session state, detected processes,
refcount, vscode-active (read-only). Status additionally needs the daemon
round-trip + tick-file read that `assemble` does NOT do. `StatusSnapshot` carries
every `--json` value (§5) — `launchd_loaded`, `daemon_pid`, `daemon_scan_state`
(+age), refcount counts, agents tri-state, provider roots, the pmset fields, the
thermal/battery summaries, `power_helper_ok`, and the
`power_assertions_state`/array.

#### 2.3.3 The pmset assertions tri-state parser (ported HERE)

Port `vigil_assertions_summary` (`lib/pmset.sh:226-306`) into the status render
path (NOT privilege-boundary code; umbrella §5.7 lines 72–75). Place it in
`src/power/pmset.rs` alongside the read-only `parse_sleepdisabled` (already
there). Three output states (Contract 4 §4a):
1. **TSV rows** (≥1 holder) — `<pid>\t<process>\t<atype>[\t← vigil]`, the
   `← vigil` suffix iff `pid == caffeinate_pid`.
2. **`(none)`** — empty output / no `Listed by owning process:` header / block
   with zero holder rows and zero non-matching rows.
3. **`(parse-failed; raw output:)`** + first 10 raw lines indented 2 — ≥1
   non-blank non-informational row but NONE match the holder shape (the
   Apple-changed-the-schema sentinel).

Preserve: `LC_ALL=C` equivalent (Rust strings are byte-safe, but the block
extraction must tolerate non-ASCII assertion names — operate on bytes/`str`
without locale-dependent parsing); the holder regex capturing pid/process/type;
the ≥4-space continuation gate (skip silently); `No `-prefix informational skip;
the `matched>0 → rows; elif non_matching>0 → parse-failed; else → (none)`
decision. Honor the `VIGIL_ASSERTIONS_FIXTURE` seam checked with **presence**
(even empty ""), so an empty fixture exercises the "pmset returned nothing"
branch without leaking real assertions.

Map to `power_assertions_state` (Contract 4 §1d, §4a): `(none)` → `"none"`;
`(parse-failed;`* → `"parse_failed"`; else → `"ok"` (and build the
`power_assertions` array).

#### 2.3.4 `daemon_scan_state` enum (Contract 1 §5, Contract 4 §1a)

Six values: **`unloaded, starting, pending, missing, stale, fresh`**. Decision
(thresholds exact):
- `unloaded` — agent not loaded.
- `starting` — loaded but `daemon_pid` not numeric (pidfile not yet written).
- `pending` / `missing` — tick `pid` ≠ `daemon_pid` OR `updated_at` non-numeric;
  `missing` iff pidfile age > `missing_after = max(10, wait_secs + tick_secs +
  3)` (`wait_secs = VIGIL_START_WAIT_SECS`, default 6), else `pending`.
- `stale` / `fresh` — tick `pid == daemon_pid` and `updated_at` numeric;
  `stale` iff `age = now - updated_at > stale_after = max(15, tick_secs*2 + 5)`,
  else `fresh`. `tick_secs` taken from the tick file if numeric, else cfg.

This is why the tick-file `pid`/`updated_at` must be byte-faithful (§2.1.6).

#### 2.3.5 The bounded first-scan wait (`cmd_start` helper)

`wait_for_daemon_scan` (Contract 2 §2, `vigil:873-898`): `wait_secs =
VIGIL_START_WAIT_SECS` (non-numeric → 6; `<1` → return immediately). Loop
`wait_secs*10` ticks, `sleep 100ms`. Each tick read the pidfile, classify via
the scan-state logic; `fresh` → print `  daemon scan: ready (…)` return 0;
`launchctl print` fails mid-wait → `  daemon scan: service not running` return 1;
timeout → `  daemon scan: pending (run 'vigil status' for details)` return 0
(**pending is NOT an error**).

---

## 3. The wiring plan

### 3.1 ONE long-lived `ProcScanner`/`System` threaded each tick

Today THREE paths each build their OWN `System` per call (umbrella §5.7 lines
127–139; Contract 6 §2):
1. `debug::assemble` → `ProcScanner::new().detect()` (`debug/mod.rs:117`).
2. `vscode::host_running(None)` live branch → `ProcScanner::new()`
   (`vscode.rs:221`).
3. `refcount::gc` takes a `&mut sysinfo::System` and self-spaces two
   `with_cpu()` refreshes with `MINIMUM_CPU_UPDATE_INTERVAL` (`refcount/mod.rs:
   280-301`).

The daemon owns ONE `ProcScanner` (`procscan/mod.rs:52`, holds the long-lived
`System`) for its whole lifetime:
- **Detect**: `self.scanner.detect()` (`procscan/mod.rs:108`) — scoped refresh
  `nothing().with_cmd(Always).with_exe(Always)`, no cpu/mem.
- **vscode host-check**: `chat_is_active(..., ps_override=None)` reaches
  `host_running(None)` whose live branch builds a `ProcScanner`. 5.7 feeds the
  shared snapshot here while **keeping the `VIGIL_VSCODE_PS_FIXTURE` seam
  reachable** (umbrella §5.7 lines 136–139). Two acceptable shapes: (a) overload
  the host-check to accept the already-collected `Vec<ProcRecord>` text via the
  existing `ps_text_override` param (pass a ps-formatted projection of the
  shared snapshot, with `None` still honoring the env fixture); or (b) refactor
  `host_running` to take `Option<&ProcScanner>`. Prefer (a) — it changes no
  signature and the fixture branch is untouched. **Constraint: the env-fixture
  branch must remain the first thing checked when the override is `None`-ish**,
  so the ported integration tests can still inject `VIGIL_VSCODE_PS_FIXTURE`.
- **GC**: `gc` needs `with_cpu()` for the idle-CPU branch, which `detect`'s scope
  excludes. Keep a SECOND `System` (`sys_for_gc`) on the daemon for the cpu
  refresh, refreshed once per tick — OR, preferred, drive the two-refresh
  spacing from the LOOP cadence: since `tick_secs` (default 5s) ≥
  `MINIMUM_CPU_UPDATE_INTERVAL`, refresh cpu ONCE per tick on the shared gc
  System and read `cpu_usage()` without the in-gc sleep. The plan explicitly
  calls for this (umbrella §5.7 lines 134–136). The implementer may keep
  `refcount::gc`'s current internal spacing for v1 and tighten to loop-cadence
  spacing as a follow-up within the slice; correctness first (the in-gc sleep
  yields correct results, just at a small per-tick cost).

> Decision to record: whether `detect` and `gc` can share ONE `System` with two
> refresh scopes, or need two `System`s, depends on sysinfo 0.39's per-`System`
> refresh-scope statefulness. Default to **two `System`s on the daemon** (one for
> detect via `ProcScanner`, one bare for gc cpu) — simplest correct shape — and
> optimize to one only if proven safe. This is an efficiency detail, not a
> correctness one; never let it block the slice.

### 3.2 `main.rs` dispatch swap

`main.rs:dispatch` (`main.rs:243-268`) currently shims ten commands. 5.7 swaps:

| Arm (current) | becomes |
|---|---|
| `Setup { args } => exec_bash("setup", …)` | `=> commands::setup::run(args)` |
| `Uninstall { args } => exec_bash("uninstall", …)` | `=> commands::uninstall::run(args)` |
| `Start { args } => …` | `=> commands::start::run(args)` |
| `Stop { args } => …` | `=> commands::stop::run(args)` |
| `Status { args } => …` | `=> commands::status::run(args)` |
| `Log { args } => …` | `=> commands::log::run(args)` |
| `Run { args } => …` | `=> commands::run::run(args)` |
| `Reload { args } => …` | `=> commands::reload::run(args)` |
| `Doctor { args } => …` | `=> commands::doctor::run(args)` |
| `Lock { args } => exec_bash("lock", …)` | **UNCHANGED** (stays shimmed → 5.6) |
| (new) `Daemon` | `=> daemon::run()` (hidden subcommand) |

Each `commands::*::run` returns an exit code (or `!`/`exit`) honoring the
existing `exit.rs` discipline: `EX_USAGE = 64` for clap/usage, `EX_ERROR = 1`
for operational failure (bash `die`). `admin_allowed()` (`exit.rs:23-28`,
honors `VIGIL_TEST_NO_ADMIN=1`) is the single choke point every privileged path
routes through; `require_admin_allowed()` (`exit.rs:34-39`, currently
`#[allow(dead_code)]`) is activated here.

**Shim removal**: `Lock` is the LAST `exec_bash` caller after this slice's swaps.
Because `Lock` stays shimmed until 5.6, `shim.rs` cannot be deleted in 5.7
*unless* `Lock` is also cut over. Per the cadence, `Lock` is NOT in 5.7's scope.
**Resolution (open question Q1):** keep `shim.rs` + the bash `bin/vigil` lock
path alive ONLY for `lock` until 5.6, OR pull the lock cutover forward into 5.7.
The umbrella's "ALL remaining Bash is physically deleted" in 5.7 conflicts with
"Lock stays shimmed until 5.6". See §9 Q1 — this MUST be resolved before the
final-deletion commit. The conservative reading (and what this doc assumes): the
final bash deletion in 5.7 removes the daemon + lib bash; `bin/vigil`'s
lock-only path + `shim.rs` survive iff lock is still shimmed. The cleanest
outcome is to **sequence the lock cutover into this slice or immediately before
the deletion commit** so `shim.rs` and `bin/vigil` die together.

### 3.3 Logging — `tracing` + `tracing-appender`, newsyslog NEVER the appender

The daemon and command layer log via `tracing`/`tracing-appender` (already
deps). The appender writes `{log_dir}/daemon.log` (`VIGIL_LOG_FILE`, re-derived
post-config as `{log_dir}/daemon.log`, `config/mod.rs:509`). Rotation is owned by
**newsyslog** (macOS native), NOT by the appender — the appender must NOT own
rotation (umbrella §7). The newsyslog config line (Contract 2 §1c) is rendered
by the service layer and installed to `/etc/newsyslog.d/vigil.conf`.

---

## 4. The 7 command ports

All under `src/commands/`. Each preserves the exact ordering/strings the bash
prints (UX overhaul refines symbols/colors but the step numbering and the
machine-relevant strings stay).

### 4.1 `cmd_setup` (Contract 2 §4, `vigil:684-766`)

Flags: `--dry-run`, `--verbose`; any other → `die "usage: vigil setup
[--dry-run] [--verbose]"`.

**Dry-run** touches NOTHING: print user/root path summary; `--verbose` adds
newsyslog + LaunchAgent + LaunchDaemon plist previews (each indented 4); final
`vigil: dry run complete. No files were installed and launchd was not changed.`

**Real setup**, guards FIRST (in this order): `require_admin_allowed()`
(VIGIL_TEST_NO_ADMIN), `cfg.validate_security_paths()` (the path allowlist,
§4.8), `assert_vigil_tree_path("install dir", install_dir)` (§4.9). Then the
printed numbered steps (single source of numbering 1–5):
1. `1. preparing user directories` → `cfg.ensure_state_dir()`.
2. **Silent `cmd_stop`** (`stop_user_agent` with output suppressed) — boots out
   any prior daemon before replacing its binary.
3. `2. installing user daemon` → `cmd_sync_install` (§4.7, the TCC copy-out).
4. `3. installing privileged power helper` → optionally preview LaunchDaemon
   plist (verbose), then `cmd_install_root_helper` (§4.8, §4.10), then
   legacy-sudoers cleanup (`sudo rm -f /etc/sudoers.d/vigil` if present).
5. `4. installing log rotation` → render newsyslog to a temp, `chmod 0644`,
   `sudo install -m 0644 -o root -g wheel` to `/etc/newsyslog.d/vigil.conf`.
6. `5. loading user LaunchAgent` → write the rendered plist to
   `~/Library/LaunchAgents/com.thangaram.vigil.plist`.
7. `cmd_start`.
8. `vigil: setup complete` + `  next: vigil status`.

### 4.2 `cmd_uninstall` (Contract 2 §5, `vigil:768-831`)

**Strict zero-flag**: any argument → `die "usage: vigil uninstall"`. Same three
guards as setup. The 5 ordered steps:
1. `1. stopping user LaunchAgent` → `cmd_stop || true`.
2. `2. releasing power hold` → `machine.full_release()` (best-effort); echo
   "power hold: released if active".
3. `3. removing user LaunchAgent` → rm the plist if present.
4. `4. removing privileged helper and log rotation` — sudo'd root removal gated
   behind a combined existence check (newsyslog / legacy-sudoers / helper-plist
   / power-helper-dir / root-helper); rm newsyslog, rm legacy sudoers,
   `cmd_remove_root_helper`.
5. `5. clearing local state` → rm `baseline.json` if present; then `rm -rf`
   `install_dir`.
Final: `vigil: uninstall complete` + `  logs preserved: {log_dir}`. **LOGS ARE
NEVER REMOVED** (`{log_dir}` = `~/Library/Logs/vigil`).

`cmd_remove_root_helper`: guards, `sudo launchctl bootout
system/{helper_label}` (|| true), conditional `sudo rm -f {helper_plist}`,
`sudo rm -rf {power_helper_dir}`, `sudo rm -f {root_helper}`, `sudo rmdir`
{root_bin_dir} and {root_dir} (best-effort; non-empty → kept).

### 4.3 `cmd_reload` (Contract 2 §2, `vigil:1258-1275`)

`cmd_sync_install` → re-render the plist if present → `cmd_stop` (bootout +
50×100ms poll) → `cmd_start` (bootstrap + enable + wait) → `vigil: reload
complete.` NO kickstart. UX: print a "what changed" summary (umbrella §5.7 UX).

### 4.4 `cmd_start` / 4.5 `cmd_stop`

`cmd_start` (§2.2.3): die if plist absent; already-loaded → idempotent;
bootstrap + enable + bounded first-scan wait (pending NOT an error). UX: its own
header line (fixes start/stop asymmetry). `cmd_stop` (§2.2.3): the 50×100ms
bootout poll; removes `daemon.tick`; idempotent "not loaded" path.

### 4.6 `cmd_run` — NON-exec wrapper (Contract 3 §1)

**MUST NOT exec.** Sequence:
1. Zero args → `die "usage: vigil run <cmd> [args...]"` → exit 1 (via the `log
   ERROR` path, not a bare echo).
2. `ensure_state_dir()`.
3. `cmd_str = args joined by single space`.
4. Write the wrapper pidfile:
   `refcount::wrapper_pidfile_body(pid, cmd_str, now)` (`refcount/mod.rs:84`) →
   `{"pid":N,"comm":"wrapper","start_ts":T,"cmd":"<cmd>"}\n` (`"` stripped from
   cmd; `start_ts` = wall-clock now, NOT proc start) to
   `{active_dir}/wrapper-{pid}.pid`.
5. Install an RAII guard / signal handler that deletes the pidfile on EXIT and on
   INT/TERM/HUP (idempotent `remove_file`). The path is captured BY VALUE so
   cleanup survives scope exit.
6. **Spawn the child NON-exec** (`std::process::Command::spawn` + `wait`), NOT
   `execv`. Exec would replace the process and the cleanup would never run,
   leaking the pidfile.
7. Propagate the child's exit status: normal exit → that code; signal-terminated
   → `128 + signal` (shell convention) for byte-faithful status. Cleanup runs,
   then the `vigil` process exits with the propagated status.

Wrapper-always-counts: the daemon's `refcount::count` treats any `wrapper-*.pid`
as an unconditional +1 (`refcount/mod.rs` prefix branch); `cmd_run` just produces
the file.

### 4.7 `cmd_sync_install` — TCC copy-out-of-Documents (Contract 2 §3)

Binary copied to the install dir BEFORE the plist points at it. Order:
`mkdir -p {install}/bin {install}/lib` → install the Rust `vigil` binary (and the
`vigil-lock-helper`, cargo-built `--release`, Darwin-only) to `{install}/bin`.
Source is the repo / cargo target (lives under `~/Documents`); dest is
`~/Library/Application Support/vigil` — OUTSIDE Documents, so launchd execs it
without TCC consent. The plist's program path resolves to the COPIED binary,
never the repo source. **Sync-then-point is mandatory**: `cmd_sync_install` runs
BEFORE the plist is written in both setup and reload.

> Bash copied `bin/vigil-daemon` + `lib/*.sh`. The Rust port copies the single
> `vigil` binary (which IS the daemon via the hidden subcommand) + the
> `vigil-lock-helper`. No `lib/*.sh` to copy.

### 4.8 The privileged-path allowlist + vigil-tree guard (Contract 2 §6)

`cfg.validate_security_paths()` (`config/mod.rs:681`) — exact-equality on the
privileged paths. Contract 2 flags a discrepancy: the bash
`cmd_assert_standard_privileged_paths` asserts **14** exact paths (the 11
`$root`-derived `VIGIL_POWER_*`/`VIGIL_ROOT_*` PLUS helper-plist `/Library/
LaunchDaemons/com.thangaram.vigil.helper.plist`, newsyslog
`/etc/newsyslog.d/vigil.conf`, legacy-sudoers `/etc/sudoers.d/vigil`). The
existing `validate_security_paths` is documented as 11 — **extend it to assert
all 14** to be bash-faithful (the umbrella's "11" is the `$root` subset). See §9
Q5.

`assert_vigil_tree_path(label, path)` (Contract 2 §6c, `vigil:82-90`) — 5 rules:
absolute; no newline/CR; not `/`; not `$HOME`; basename == `vigil`. **The
umbrella's "not under `~/Documents`/TCC" 6th rule is NOT in the current bash** —
adding it is NEW hardening, not a faithful port. **Decision (Q4):** add it (it is
correct and the umbrella asks for it), but flag it in the commit message as a
hardening delta, not parity. The "not under Documents" property is otherwise
enforced structurally by the copy-out in §4.7.

`VIGIL_TEST_NO_ADMIN` abort (`exit.rs` `admin_allowed`) MUST fire before EVERY
sudo/launchctl/root-file touch in setup/uninstall — and, per umbrella §5.7 line
91, reload too (reload reaches sudo only through `cmd_sync_install`, which is
non-privileged in bash; if the Rust reload ever touches root, gate it).

### 4.9 (folded into §4.8)

### 4.10 Root-helper install-path switch to the Rust binary (Contract 2 §7)

The 5.5 deferral resolves here. The ONLY changing line is the install SOURCE:
bash `vigil:214` installed `$VIGIL_REPO_ROOT/bin/vigil-root-helper` (the bash
script). 5.7 installs the **cargo-built Rust `vigil-root-helper`** release binary
(`src/bin/vigil-root-helper.rs`) instead. Dest
`/Library/Application Support/vigil/bin/vigil-root-helper`, mode `0755`, owner
`root:wheel`, and the plist argv[0] ALL stay byte-identical so the helper's own
argv/path validators keep passing.

Per `MEMORY.md`: 5.5 kept the bash `bin/vigil-root-helper` because the bash setup
still installed it; its last sourcer is `root_helper_test.sh`. Once that test is
deleted (§6) and setup installs the Rust binary, the bash helper file is deleted
in the final cutover.

**The asymmetric IPC dir ownership matrix** (Contract 2 §8) — created via `sudo
install -d` in this exact order/owner/mode:

| # | Path | Mode | Owner:Group |
|---|---|---|---|
| 1 | `…/vigil` | 0755 | root:wheel |
| 2 | `…/vigil/bin` | 0755 | root:wheel |
| 3 | `…/vigil/helper` | 0755 | root:wheel |
| 4 | `…/helper/requests` | 0755 | root:wheel |
| 5 | `…/helper/responses` | 0755 | root:wheel |
| 6 | `…/helper/state` | **0700** | root:wheel |
| 7 | `…/helper/logs` | 0755 | root:wheel |
| 8 | `…/helper/requests/{uid}` | **0700** | **{user}:{group}** |
| 9 | `…/helper/responses/{uid}` | 0755 | root:wheel |

Asymmetry (the privilege boundary): request dir (#8) user-owned 0700 (user writes
requests); response dir (#9) root-owned 0755 (root writes, user reads); state dir
(#6) root-private 0700. Then root-helper binary, helper plist (0644 root:wheel),
then bootout/bootstrap/enable.

### 4.11 `cmd_log` (Contract 3 §2)

Flag: `$1` exactly `-f` or `--follow` → follow; anything else ignored (no error).
Missing log → print `no log yet at {log_file}` to STDOUT, return 0 (NOT an
error). Follow → `tail -f` semantics (unbounded stream). **No-follow → paging /
line-limit** (the ONE intentional deviation: bash `cat`s the whole file; Rust
MUST cap / page — no megabyte dump; umbrella §5.7 lines 83, 902–903). Does NOT
call `ensure_state_dir` (read-only).

---

## 5. status / doctor on the unified CheckEngine

### 5.1 `vigil status --json` — the FROZEN flat schema (Contract 4 §1)

Exit **always 0** on the happy path; only a usage violation → `die`/exit 1
(`[[ $# -eq 0 ]] || die "usage: vigil status [--json]"`). Object opens `{\n`,
closes `}\n`, two-space indent on every key, sequential `printf`.

**Insert `"version": 1` as the NEW FIRST key** (confirmed absent in bash — Q2).
The remaining 21 keys keep this EXACT order and types (byte-stable vs golden):

| # | key | type | source |
|---|---|---|---|
| (new) | `version` | number | literal `1` |
| 1 | `launchd_loaded` | bool | `launchctl print gui/{uid}/{label}` |
| 2 | `daemon_pid` | number\|null | pidfile if `^[0-9]+$` |
| 3 | `daemon_scan_state` | string enum | §2.3.4 (escaped) |
| 4 | `daemon_scan_age_secs` | number\|null | scan age if numeric |
| 5 | `refcount_active` | number | `refcount::count` |
| 6 | `refcount_total` | number | `refcount::count_total` |
| 7 | `pending_active_matches` | number | live-match count or 0 |
| 8 | `idle_window_minutes` | number | `(idle_after_sec + 59) / 60` |
| 9 | `agents` | sub-object | §5.2 |
| 10 | `provider_roots` | sub-object | claude/codex/copilot, each {home,session_dir,exists,latest_activity_age_secs} |
| 11 | `power_hold_mode` | string | literal `best-effort` |
| 12 | `pmset_disablesleep` | number 0\|1 | `vigil_read_sleepdisabled` — UNQUOTED |
| 13 | `baseline` | number\|null | baseline value if file exists |
| 14 | `caffeinate_pid` | number\|null | pid if pidfile + numeric |
| 15 | `caffeinate_alive` | bool | `caffeinate_alive` (identity check) |
| 16 | `thermal` | string | `thermal_summary` (escaped) |
| 17 | `battery` | string | `battery_summary` (escaped) |
| 18 | `power_helper_ok` | bool | helper status round-trip |
| 19 | `power_assertions_state` | string enum | `ok`\|`none`\|`parse_failed` |
| 20 | `power_assertions` | array | §2.3.3; `[]` whenever state ≠ `ok` |

Trailing-comma rule: every key except the last (`power_assertions`) ends `,\n`;
`power_assertions` has none. `"version": 1,\n` when prepended. Use the existing
`vigil_json_escape` semantics (escapes `\ " \t \r \n`); `serde_json`'s escaper is
a superset and matches byte-for-byte on these short ASCII operational strings —
but for `--json` byte-stability prefer hand-emitting the object in the frozen key
order (do NOT rely on a derived `Serialize` whose field order or escaping could
drift). `power_hold_mode` is the literal `best-effort`.

### 5.2 `agents` sub-object (Contract 4 §1b)

Fixed shape, 4 keys in order:
`{"claude":"%s","codex":"%s","copilot":"%s","vscode_copilot_chat":"%s"}`. Per-agent
tri-state enum `active | idle | none` (`activity/mod.rs` via `scan::agent_state`
→ `AgentState`): `none` if the session dir doesn't exist; else `active` if
`is_active`, else `idle`. vscode variant: `none` if `!host_running`, else
active/idle. Values json-quoted, closed enum (no escape needed).

### 5.3 Plain `status` + `expected_hold` (Contract 4 §2)

Exit ALWAYS 0 (non-usage). Flags: `--json` (delegates), `--verbose`, default;
else `die "usage: vigil status [--json|--verbose]"`. Progressive disclosure:
non-verbose prints service/activity/power blocks + the `--verbose` hint;
`--verbose` adds provider roots + the assertion rows. **UX:** computed column
alignment (comfy-table) + a labeled power line (fixes the 5-fields-in-one-string
flaw); the always-shown hint is suppressed when not applicable.

`expected_hold` sub-states (Contract 4 §2a) — computed only when work exists but
no hold (`(active>0 || pending>0) && !hold_engaged`), priority-ordered:
1. `blocked by thermal cutoff` (cut_thermal)
2. `blocked by battery floor` (cut_battery)
3. `pending (LaunchAgent is not loaded)`
4. `pending (daemon first scan has not completed)` (scan `starting`|`pending`)
5. `pending (daemon scan is unavailable; try 'vigil reload')` (scan `stale`|`missing`)
6. `pending (live matches are waiting for the next daemon scan)` (pending>0)
7. `pending (daemon/helper transition in progress)` (else)

`cut_thermal`/`cut_battery` here come from live `should_cut`, NOT the tick file.

### 5.4 `doctor` three-state + exit-code matrix (Contract 4 §3)

Counters: `errs`, `warns`, `install_markers` (LaunchAgent plist, daemon binary,
state dir). State resolution:
- `errs==0 && warns>0` → `ready with warnings` → **exit 0**
- `errs==0 && warns==0` → `ready` → **exit 0**
- `errs>0 && install_markers>0` → `needs repair` → **exit 1**
- `errs>0 && install_markers==0` → `not installed` → **exit 1**

**`lock helper` missing is the ONLY `warns++` site** — it produces the
`ready with warnings` third state (`[[ -x {install}/bin/vigil-lock-helper ]]` →
ok else `missing (run vigil setup/reload before using vigil lock)` WARN). Every
other failed check is `errs++`. `launchd: loaded/not loaded` is informational (no
counter). `cmd_doctor --power` runs its own `errs` counter → exit 0/1 on its own
checks. UX: doctor's providers section shows session-dir diagnostics by default.

**Exit-code summary** (umbrella §7 CLI contracts): status always 0 (usage → 1);
doctor 0 (incl. ready-with-warnings) / 1 (not-installed / needs-repair); doctor
`--power` 0/1; unknown command/subcommand → 64 (top-level dispatch, `exit.rs`
`EX_USAGE`). None of status/doctor use 64.

### 5.5 The assertions tri-state tests ported here (Contract 4 §5)

Port the 8 `assertions_test.sh` cases (Contract 5 #2) to cargo, driving
`VIGIL_ASSERTIONS_FIXTURE`: empty/no-header/empty-block/explicit-"No" → `(none)`;
1+/multi/continuation-filtered/vigil-tag → TSV; non-matching-rows →
`parse_failed`. This is a HARD blocker on deletion (§6) — zero cargo coverage
exists today.

---

## 6. THE STAGED CUTOVER PLAN (the most important section)

**Rule (umbrella §10, §5 strangler rule):** no Bash is deleted until (1) its
cargo-test rewrite passes, (2) golden fixtures match captured Bash output, (3)
the FULL `tests/run.sh` is green against the Rust binary. The retirement lands as
a SEPARATE, independently-revertable commit. Every commit below must keep the
full suite green and be revertable on its own.

### 6.1 Per-file test fate table (from Contract 5)

| tests/ file | Cat | Fate | Cargo replacement / GAP |
|---|---|---|---|
| `activity_test.sh` | A | DELETE | `tests/activity.rs` (15) + `activity/*` unit tests — exists |
| `assertions_test.sh` | A | **DELETE after GAP filled** | **GAP** — no cargo port; §5.5 ports it into the status path (HARD blocker) |
| `battery_parity_test.sh` | C | KEEP-CONVERT→rust-only / DELETE | `battery/mod.rs` (12) covers Rust side; bash oracle dies |
| `battery_test.sh` | A | DELETE | `battery/mod.rs` (12) — exists |
| `cli_dispatch_test.sh` | B | REPOINT (already Rust) — prune 1 | `tests/cli.rs`; DELETE `test_rust_delegates_to_bash_via_exec` at cutover |
| `cli_preview_test.sh` | B | REPOINT bash→Rust | **GAP** — status/doctor/setup/uninstall still shimmed; 5.7-owned; needs `src/check/`+`src/service/`+golden `--json` |
| `config_parity_test.sh` | C | KEEP-CONVERT→rust-only / DELETE | `config/mod.rs` (13); bash oracle `dump_bash_config.sh` dies with `lib/common.sh` |
| `detect_parity_test.sh` | C | KEEP-CONVERT→rust-only / DELETE | `tests/detect.rs` (19) + `procscan/detect.rs` (5) + `tests/debug.rs:96` oracle |
| `detect_test.sh` | A | DELETE | `tests/detect.rs` — 1:1 port, same fixtures |
| `lock_test.sh` | B | REPOINT → Rust bin (5.6) | **GAP** — no cargo lock coverage; `src/lock/` is 5.6. See Q1 |
| `newsyslog_test.sh` | A/MIXED | split: DELETE lib half; REPOINT/REWRITE template half | (a) re-derive covered by `config/mod.rs:1101`; (b) **GAP** — template-render → new 5.7 native test |
| `parser_test.sh` | A | DELETE | `tests/refcount.rs` field() cases |
| `power_reconcile_test.sh` | A | DELETE | `power/mod.rs` (14) + `power/caffeinate.rs` (3) + `power/pmset.rs` (1) |
| `refcount_activity_test.sh` | A | DELETE | `tests/refcount.rs` count/list |
| `root_helper_test.sh` | A | DELETE (last bash-helper sourcer) | `tests/helper_adversarial.rs` (17) + `tests/root_helper_redteam.rs` (2) + `helper/*` |
| `thermal_parity_test.sh` | C | KEEP-CONVERT→rust-only / DELETE | `thermal/mod.rs` (14) incl. `unset_floor_matches_bash_grep_semantics` |
| `thermal_test.sh` | A | DELETE | `thermal/mod.rs` (14) |
| `wrapper_test.sh` | B+A | REPOINT test1; DELETE test2 | **GAP** — test1 (non-exec run pidfile lifecycle) → 5.7 `cmd_run` cargo test; test2 gc covered by `tests/refcount.rs:217` |

Special: `tests/fixtures/config/dump_bash_config.sh` DELETE (with
`config_parity`); `tests/fixtures/ps-axww*-snapshot.txt` **KEEP** (Rust uses
them); `etc/vigil.newsyslog.in` **KEEP** (rendered template survives, render test
moves to Rust). `tests/lib.sh` + `tests/run.sh`: DELETE once no bash `*_test.sh`
remains (cargo test becomes the sole suite).

### 6.2 The COVERAGE GAPS that MUST be closed before the irreversible `rm`

These are bash tests with NO current cargo equivalent. Each is a hard blocker:

1. **`assertions_test.sh`** — pmset assertions tri-state parser + sentinels. Zero
   cargo coverage. Port into `src/power/pmset.rs` + a cargo test driving
   `VIGIL_ASSERTIONS_FIXTURE` (§5.5). **HARD blocker.**
2. **`cli_preview_test.sh`** — status `--json` machine schema, pending/missing
   scan, `--verbose` diagnostics; setup/uninstall blocked under
   `VIGIL_TEST_NO_ADMIN`, privileged-path refusal before sudo, `--dry-run`
   touches nothing, dry-run plist XML-escape; doctor grouped/concise,
   partial-install→needs-repair, `--power` nonzero. Needs `src/check/` +
   `src/service/` natives + the golden `--json` fixture. **HARD blocker.**
3. **`lock_test.sh`** — lock command + lock doctor + freeze-launch args +
   non-macOS reject. Needs `src/lock/` (5.6). **Gates the FULL-suite-green
   precondition** — see Q1: if lock stays bash-shimmed, `lock_test.sh` must be
   repointed at the Rust `vigil` (which still execs bash lock) and remain green;
   the bash lock path then survives the 5.7 deletion.
4. **`newsyslog_test.sh` template-render half** — `etc/vigil.newsyslog.in`
   rendering. New 5.7 native service-layer render test.
5. **`wrapper_test.sh` non-exec half** — `vigil run` pidfile lifecycle / trap
   cleanup / non-exec. New 5.7 `cmd_run` cargo test (drive `vigil run sleep N`,
   assert `wrapper-*.pid` created during child life + removed after; assert the
   binary did NOT exec by checking cleanup ran).

### 6.3 The per-commit sequence (each gated on full `tests/run.sh` green)

> Throughout, `tests/run.sh` runs the SURVIVING bash tests; new cargo tests run
> via `cargo test`. A commit is mergeable only if BOTH are green. The
> `VIGIL_RUST_BIN` env points the B/C tests at the Rust binary as each command
> goes native.

- **Commit 0 — Gate-0 golden capture (touches NO Rust logic).** Run the current
  bash and capture golden fixtures: `vigil status --json` (clean + engaged +
  pending states), `vigil setup --dry-run` / `--dry-run --verbose` output,
  `vigil uninstall` output (in a sandboxed `VIGIL_INSTALL_DIR`), `vigil doctor`
  (+ `--power`, + partial-install) output, the rendered plists. Commit them under
  `crates/vigil/tests/golden/`. See §7. Revertable: pure additive fixtures.

- **Commit 1 — `src/service/` (no dispatch change).** `ServiceInstaller` +
  `MacosLaunchdInstaller` + typed plists via `plist` crate + unit tests asserting
  the rendered plists match the Commit-0 goldens (byte-stable, XML-escape). Add
  `plist = "1.9.0"`. No command dispatch swap yet. Revertable: additive module.

- **Commit 2 — `src/check/` + the assertions parser (no dispatch change).**
  `CheckEngine` + `StatusSnapshot` + `daemon_scan_state` + the ported
  `assertions_summary` tri-state parser in `src/power/pmset.rs`. Port the 8
  `assertions_test.sh` cases to cargo (closes GAP #1). Unit-test the `--json`
  emitter against the Commit-0 golden. Dispatch still shims status/doctor.
  Revertable: additive.

- **Commit 3 — `src/daemon/` (no dispatch change; not yet launched by anything).**
  The tick loop + mkdir-lock + tick-file writer + crash recovery + signals + the
  one-System wiring. Unit-test the predicate, the tick-file bytes, the lock
  stale-recovery, the release-priority branches with fake seams. Add the hidden
  `Daemon` subcommand to the enum but DON'T point the plist at it yet (plist
  still points at bash `vigil-daemon`). Revertable: additive subcommand.

- **Commit 4 — swap status + doctor native.** `main.rs` dispatch:
  `Status`/`Doctor` → `commands::{status,doctor}::run`. Repoint `cli_preview_test`
  status/doctor assertions at the Rust bin; assert against Commit-0 goldens
  (closes the status/doctor half of GAP #2). Bash `cmd_status`/`cmd_doctor`
  remain in `bin/vigil` (still reachable via the daemon-bash for its own use, but
  the user-facing command is now Rust). Full suite green.

- **Commit 5 — swap run + log native.** `Run`/`Log` → native. Add the `cmd_run`
  non-exec cargo test (closes GAP #5) and the `cmd_log` paging test. Repoint
  `wrapper_test.sh` test1. Full suite green.

- **Commit 6 — swap setup/uninstall/reload/start/stop native + root-helper
  Rust-binary install.** These go native together (reload calls stop+start;
  setup calls stop+sync+start). Switch the install SOURCE to the Rust
  `vigil-root-helper` (§4.10). Now the installed plist points at
  `{install}/bin/vigil daemon` (the Rust daemon) — the daemon goes live. Repoint
  `cli_preview_test` setup/uninstall + `newsyslog_test` template half (closes GAP
  #2 remainder + #4). Assert the 50×100ms poll (mock launchctl), reload uses
  bootout/bootstrap, dry-run touches nothing, uninstall strict-zero-flag + order,
  `VIGIL_TEST_NO_ADMIN` blocks before sudo. Full suite green — INCLUDING a live
  setup→start→status→stop→uninstall round-trip on the Rust daemon.

- **Commit 7 — resolve lock (Q1).** Either (a) repoint `lock_test.sh` at the Rust
  `vigil` (which still execs bash `lock` via `shim.rs`) and keep it green —
  deferring `shim.rs` + the bash lock path to 5.6; OR (b) pull the lock cutover
  forward. Decide BEFORE Commit 8. If (a): `shim.rs` and `bin/vigil`'s lock path
  survive the next commit; document that the "ALL bash deleted" umbrella line is
  satisfied except the lock shim, which 5.6 removes.

- **Commit 8 — THE IRREVERSIBLE DELETION (separate, revertable commit).** Only
  when the FULL `tests/run.sh` (now entirely cargo, or cargo + the surviving lock
  bash) is green against the Rust binary and all goldens match: `rm`
  `bin/vigil-daemon`, `lib/{common,pmset,detect,activity,refcount,thermal,
  battery}.sh`, the bash `bin/vigil-root-helper` (its last sourcer
  `root_helper_test.sh` is deleted), the deleted bash `*_test.sh` files,
  `tests/fixtures/config/dump_bash_config.sh`, `tests/lib.sh`, `tests/run.sh`. If
  Q1=(a), KEEP `bin/vigil` (lock-only) + `shim.rs` until 5.6; else `rm` them too.
  This commit is revertable as a unit (a single `git revert` restores the bash).

### 6.4 The full-suite-green precondition (non-negotiable)

Per umbrella §5.7 line 113 and §10: **the full `tests/run.sh` must be green
against the Rust binary before any Bash file is removed.** `tests/run.sh`
installs a sudo-blocking PATH guard and exports `VIGIL_TEST_NO_ADMIN=1` /
`VIGIL_REPO_ROOT`; as the suite migrates to cargo, that sudo-guard + NO_ADMIN env
MUST be replicated in the Rust tests that exercise setup/uninstall (the
`helper-test-seam` pattern and `VIGIL_TEST_NO_ADMIN` are already wired in
`exit.rs`).

---

## 7. Golden-fixture capture step (Gate-0)

BEFORE writing any 5.7 Rust render code, capture the current bash output as
golden fixtures so byte-stability is provable (umbrella §10 gate-0 requirement;
§5.7 line 104 "golden-fixture the `--json` status schema … assert byte-stable"):

Capture (in a sandboxed `VIGIL_INSTALL_DIR`/`VIGIL_STATE_DIR`/`VIGIL_LOG_DIR`,
with the `VIGIL_*_FIXTURE` seams set so output is deterministic):
- `vigil status --json` in several states: clean/no-daemon, engaged, pending
  first-scan, missing-scan, with/without power assertions (drive
  `VIGIL_ASSERTIONS_FIXTURE`, `VIGIL_THERMAL_FIXTURE`, `VIGIL_BATTERY_FIXTURE`).
- `vigil status` and `vigil status --verbose` (text blocks).
- `vigil setup --dry-run` and `--dry-run --verbose` (incl. the plist previews —
  proves XML-escape parity).
- `vigil uninstall` output (sandboxed).
- `vigil doctor`, `vigil doctor --verbose`, `vigil doctor --power`,
  partial-install doctor (→ needs-repair).
- The rendered user-agent plist + helper plist + newsyslog (via the bash render
  fns, sandboxed).

Store under `crates/vigil/tests/golden/`. The cargo `--json` test asserts the
Rust output == golden byte-for-byte, with the SOLE allowed diff being the new
`"version": 1,` first line (assert the rest is byte-identical by stripping that
line, or capture a NEW golden that includes it and assert the bash output matches
golden-minus-version). Keep both: a `status.bash.json` (no version) and a
`status.rust.json` (with version) where `rust == version-line + bash`.

---

## 8. Must-preserve checklist for 5.7 (gate)

Pulled from umbrella §7, scoped to this slice. Every box must be checked before
Commit 8.

**Infra:**
- [ ] launchd labels HARDCODED (`com.thangaram.vigil`, `com.thangaram.vigil.helper`)
      in both the `Label` field and the literal string; not overridable.
- [ ] LaunchAgent + root LaunchDaemon plists generated typed (XML-escaped via the
      `plist` crate, `skip_serializing_if` on optionals); TCC copy-out-of-Documents.
- [ ] Single-instance via atomic `mkdir` lock dir + PID-liveness stale recovery
      (NOT flock); live-contention → exit(0), stale → take over, re-mkdir-fail →
      exit(1).
- [ ] Atomic tick-file write (tmp + rename); 9 fields exact order; `engaged` =
      post-action; `pid`/`updated_at` byte-faithful.
- [ ] File-as-source-of-truth logging (`tracing-appender`); newsyslog owns
      rotation, NEVER the appender.
- [ ] State dir chmod 0700; the asymmetric IPC dir ownership matrix (#8 user-0700,
      #6 root-0700, #9 root-0755).

**Daemon predicate / power:**
- [ ] `desired = count>0 && !thermal && !battery && !cooling` (byte-exact).
- [ ] Release priority thermal(soft, KEEP baseline) > battery(full, CLEAR) >
      count==0(full, CLEAR); `engaged` flipped false on any release; engage sets
      true only on Ok; reconcile sets false only on Err.
- [ ] Sliding thermal cooldown re-arm every pressure tick via `cooldown_state`;
      independent of tick interval.
- [ ] Crash recovery: refresh evidence FIRST; `can_hold = !thermal && !battery`
      (both evaluated at startup); `recover_startup` → engaged iff true.
- [ ] INT/TERM → full_release (if engaged) → rm pidfile+tickfile → rm lockdir →
      exit(0). HUP NOT trapped. Respawn-safe under KeepAlive.
- [ ] `VIGIL_FORCE` checked FIRST (inside the guards, before any subprocess).
- [ ] ONE resident `System`/`ProcScanner` threaded through detect + vscode +
      gc; `VIGIL_VSCODE_PS_FIXTURE` seam reachable in `host_running(None)`.

**CLI / UX / security:**
- [ ] Exit codes: status always 0 (usage → 1); doctor 0 (incl. ready-with-warnings)
      / 1 (not-installed / needs-repair); doctor `--power` 0/1; unknown → 64.
- [ ] `--json` flat schema, every key, `daemon_scan_state` enum, agents
      sub-object, NEW top-level `version`; byte-stable vs golden.
- [ ] lock-helper-absent = WARNING (the only warns++ site).
- [ ] `vigil run` NON-exec; trap cleanup on EXIT/INT/TERM/HUP; child exit-code
      propagation (128+signal).
- [ ] setup `--dry-run`/`--verbose`; setup silent `cmd_stop` first; uninstall
      5-step order + strict-zero-flag + logs preserved; reload full
      bootout/bootstrap (NOT kickstart); stop's 50×100ms poll; start's bounded
      "pending"-not-error wait.
- [ ] `--version` exact `vigil <VERSION>`; legacy sudoers cleanup on setup/uninstall.
- [ ] Exact-equality privileged-path allowlist (all 14, §4.8);
      `assert_vigil_tree_path` 5 rules (+ optional Documents-exclusion hardening
      flagged as a delta); `VIGIL_TEST_NO_ADMIN` abort before EVERY sudo in
      setup/uninstall/reload.
- [ ] `vigil log` no-`-f` gets paging/line-limit; soft "no log yet" message
      returns 0.

---

## 9. Open questions / risks the implementer must resolve

**Q1 — Lock vs the "ALL bash deleted" line (the central sequencing conflict).**
The umbrella says 5.7 physically deletes ALL remaining bash, but `MEMORY.md` +
the umbrella's own 5.6 dependency note keep `lock` shimmed until 5.6. These
conflict on `shim.rs` + `bin/vigil`'s lock path. RESOLVE before Commit 8:
either (a) keep `bin/vigil` (lock-only) + `shim.rs` alive until 5.6 and repoint
`lock_test.sh` at the Rust `vigil` (which execs bash lock), OR (b) pull the lock
cutover forward into 5.7. Recommended: (b) if the 5.6 objc2 overlay work is
ready, so `shim.rs` and `bin/vigil` die together and the umbrella's deletion line
is literally satisfied; else (a) with an explicit documented carve-out.

**Q2 — `version` field value + policy.** Confirmed NEW (no bash counterpart).
This doc uses literal `1`. Confirm the value and that it is the FIRST key with a
trailing comma. Decide the bump policy (any key add/remove/reorder → bump). This
is the only allowed diff vs the bash golden.

**Q3 — MSRV.** `sysinfo` (0.39.x — umbrella cites 1.95) and `plist` (umbrella
cites 1.88) set the floor; take the HIGHER (1.95). `plist 1.9.0` and `sysinfo
0.39.3` are already pinned/used — verify their actual `rust-version` at
implementation time and set the workspace MSRV accordingly. The crate currently
declares `edition = "2024"` with no explicit `rust-version` — add one.

**Q4 — `assert_vigil_tree_path` Documents-exclusion rule.** The umbrella's "not
under `~/Documents`/TCC" rule is NOT in the current bash (5 rules only). Adding
it is NEW hardening. Decision: add it (correct + umbrella-requested) but flag it
in the commit as a hardening delta, not parity. Confirm it doesn't break the
sandboxed test fixtures (which may live under a temp dir, not Documents).

**Q5 — 14 vs 11 privileged paths.** Extend `validate_security_paths` from the
documented 11 to all 14 (adds helper-plist / newsyslog / legacy-sudoers exact
checks) to be bash-faithful. Confirm the existing `config/mod.rs:681`
implementation and widen it.

**Q6 — bootout/bootstrap race poll.** The 50×100ms bootout poll MUST stay (fast
machines fail setup/reload without it). The helper bootout deliberately has NO
poll (asymmetry). Do not "optimize" either. Mock launchctl in the cargo test to
assert the poll loops until `print` fails.

**Q7 — clap_complete install policy.** `clap_complete` is already a dep and
`Completions` is native. 5.7 does not change completion generation, but confirm
setup does NOT auto-install completions to a system dir (no bash precedent);
keep completion generation an explicit `vigil completions <shell>` the user pipes
themselves. Flag if setup should optionally install them (defer to Phase 6 if
out of scope).

**Q8 — `plist` crate optional-key absence.** launchd treats key ABSENCE as
meaningful. Every optional plist key needs `skip_serializing_if`. A bug here
silently changes daemon behavior (e.g. emitting `KeepAlive=false` instead of
omitting). Assert the rendered plist byte-matches the golden.

**Q9 — one-System sharing vs two Systems.** Whether `detect` (no-cpu scope) and
`gc` (cpu scope) can share ONE `sysinfo::System` depends on 0.39's refresh-scope
statefulness. Default to two Systems on the daemon (simplest correct); optimize
to one only if proven. Never let this block the slice — it is efficiency, not
correctness, and the thermal framing forbids overselling it.

**Q10 — signal-safe cleanup on the daemon main thread.** A Rust signal handler
cannot safely run `full_release` (subprocess spawn, file IO). Use a flag/self-pipe
checked on the main thread (top of loop + interruptible sleep) so cleanup runs
outside the handler. Verify the `ExitTimeOut=60` window covers a slow release.

---

## 10. Crate appendix (5.7 additions)

- `plist = "1.9.0"` — NEW. Typed LaunchAgent/LaunchDaemon serialization, auto
  XML-escape. `skip_serializing_if` on optionals.
- `sysinfo = "0.39.3"` — already present; the daemon's single resident `System`.
- `tracing` / `tracing-appender` — already present; daemon log (newsyslog rotates).
- `comfy-table` / `anstream` / `owo-colors` / `clap` / `clap_complete` — already
  present; status/doctor render + completions.
- `nix` (process/signal) — already present; `kill(pid, 0)` liveness, signal traps.
- `service-manager 0.11.0` — DEFERRED (Linux-only, 5.8); NOT used on macOS.

(Everything else the daemon wires — config, procscan, activity, refcount,
thermal, battery, power_guard, power, ipc, debug — is already a dependency-free
in-crate module.)
