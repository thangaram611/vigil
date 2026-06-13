# Gate-0 golden fixtures (Phase 5.7)

Frozen byte-for-byte captures of the **current bash `bin/vigil`** output, taken
in a fully sandboxed, deterministic environment. The later Rust render code
(`src/service/`, `src/check/`, `commands::{setup,status}`) must reproduce these
bytes exactly. See `future/phase-5.7-daemon-service-cutover.md` §7 and §5.

> These were produced by the **bash** binary (`bin/vigil`), NOT the Rust binary.
> They are the ABI oracle. Do not regenerate them against the Rust binary.

## The single allowed diff: `version`

The Rust `status --json` prepends one new FIRST key — `  "version": 1,` — right
after the opening `{`. That is the ONLY permitted byte difference vs. the bash
output (decision Q2). Two files encode this:

- `status_clean.json`        — raw **bash** output (no `version`).
- `status_clean.rust.json`   — the exact **Rust** output: identical to
  `status_clean.json` with `  "version": 1,\n` inserted as the first key.

Invariant (asserted in the cargo test): stripping the `  "version": 1,` line
from `status_clean.rust.json` yields `status_clean.json` byte-for-byte.

## Files

| file | command (bash `bin/vigil`) | notes |
|---|---|---|
| `setup_dryrun.txt`          | `vigil setup --dry-run`             | concise dry-run, previews hidden |
| `setup_dryrun_verbose.txt`  | `vigil setup --dry-run --verbose`   | includes the 3 previews, each indented 4 spaces |
| `user_agent.plist`          | `cmd_render_plist` (direct)         | un-indented LaunchAgent plist — what `src/service` must reproduce |
| `helper.plist`              | `cmd_render_helper_plist` (direct)  | un-indented root LaunchDaemon plist |
| `vigil.newsyslog`           | `cmd_render_newsyslog` (direct)     | un-indented newsyslog.d entry |
| `status_clean.json`         | `vigil status --json` (clean state) | no daemon/tick/baseline/caffeinate |
| `status_clean.rust.json`    | derived                             | clean + prepended `"version": 1,` |
| `status_engaged.json`       | `vigil status --json` (engaged hold)| loaded + fresh tick + baseline + live caffeinate |
| `status_pending.json`       | `vigil status --json` (pending scan)| loaded + daemon.pid, no tick → pending first-scan |
| `status_assertions.json`    | `vigil status --json` (clean + non-empty assertions) | exercises `power_assertions_state: ok` with 2 holders |
| `status_text.txt`           | `vigil status` (clean)              | plain text blocks |
| `status_verbose.txt`        | `vigil status --verbose` (clean)    | + provider roots + assertion rows |

The 3 rendered artifacts were captured by calling the bash render functions
**directly** (verbatim `cmd_render_plist` / `cmd_render_helper_plist` /
`cmd_render_newsyslog` extracted from `bin/vigil`), which produces the exact
bytes that the `setup --dry-run --verbose` previews show after un-indenting the
4-space `sed 's/^/    /'` indent. This was verified equal to the un-indented
previews in `setup_dryrun_verbose.txt`.

## The sandbox

A fixed, documented root is used so the rendered paths are byte-reproducible by
a Rust test that sets the same env. Created fresh (`rm -rf` then `mkdir`) and is
**not** under `~/Documents` (TCC-safe):

```
ROOT = /private/tmp/vigil-golden-sbx
  ROOT/bin/            # PATH-shadowing command stubs (date, launchctl, pmset, sudo, visudo, ps)
  ROOT/home/           # $HOME; provider/{claude,codex,copilot}
  ROOT/install/        # VIGIL_INSTALL_DIR
  ROOT/state/active/   # VIGIL_STATE_DIR
  ROOT/logs/           # VIGIL_LOG_DIR
```

A pinned codex session file exists so the `codex` provider reads `exists:true`:
```
ROOT/home/provider/codex/sessions/2026/06/12/rollout-2026-06-12T00-00-00-test.jsonl
```
Its mtime is pinned to FIXED_NOW (`touch -t 202311142213.20`, UTC) so
`codex.latest_activity_age_secs == 0` deterministically. With this 2023 mtime the
real-wall-clock `find -mmin` activity probe reads codex as **idle** (not active),
which is the state captured in `status_clean.json` / `status_pending.json` /
`status_verbose.txt`.

### Deterministic clock

`vigil_now_unix()` is `date +%s`. The `ROOT/bin/date` stub returns a fixed epoch
for `+%s` and passes every other format through to `/bin/date`:

```
FIXED_NOW = 1700000000   # 2023-11-14T22:13:20Z
```

This pins `daemon_scan_age_secs` and `*.latest_activity_age_secs`. (The
real-wall-clock `find -mmin` activity probe is NOT pinned by this — see the codex
note above; that is why all reproducible captures keep agents at idle/none.)

## Environment per capture

All captures export the **common env** below. Per-capture deltas are listed
after it. `id -u` / `id -un` / `id -gn` are read by the bash render path directly
(NOT overridable) — recorded here so a Rust test can substitute equivalents.

### Host identity baked into the goldens (this capture machine)

| field | value | appears in |
|---|---|---|
| `id -u`  (uid)    | `1993776753` | `helper.plist` `--allowed-uid` |
| `id -un` (user)   | `thanga-5521` | `helper.plist` `--allowed-user`; `vigil.newsyslog` owner |
| `id -gn` (group)  | `staff` | (not in these goldens; helper-plist owner is hardcoded `staff` via newsyslog template) |
| `uname -s`        | `Darwin` | n/a (gates lock-helper build, not rendered) |
| `uname -m`        | `arm64`  | n/a (doctor --power only; not captured here) |

> A Rust render test must substitute the *test host's* uid/user, OR pin them.
> The recommended Rust approach: assert the plist render against these goldens
> with uid/user templated from `id`, OR (cleaner) capture goldens with a fixed
> uid/user the Rust test also forces. The render fns themselves use `id -u` /
> `id -un`; the Rust port should read the same.

### Common env (all captures)

```sh
PATH="$ROOT/bin:$PATH"                 # shadow date/launchctl/pmset/sudo/visudo/ps
HOME="$ROOT/home"
VIGIL_REPO_ROOT="<repo>"               # /Users/.../personal/vigil
VIGIL_INSTALL_DIR="$ROOT/install"
VIGIL_STATE_DIR="$ROOT/state"
VIGIL_LOG_DIR="$ROOT/logs"
VIGIL_CONFIG_FILE="$ROOT/no.conf"      # non-existent → no vigil.conf sourced
VIGIL_CLAUDE_HOME="$HOME/provider/claude"
VIGIL_CODEX_HOME="$HOME/provider/codex"
VIGIL_COPILOT_HOME="$HOME/provider/copilot"
VIGIL_POWER_REQUEST_DIR="$ROOT/install/helper/requests/UID"   # pins helper.plist render
VIGIL_POWER_RESPONSE_DIR="$ROOT/install/helper/responses/UID"
VIGIL_THERMAL_FIXTURE='Note: No CPU power status has been recorded'   # thermal "ok"
VIGIL_BATTERY_FIXTURE=$'Now drawing from \'AC Power\'\n -InternalBattery-0\t90%; charged; 0:00 remaining present: true'   # battery "AC 90%"
VIGIL_ASSERTIONS_FIXTURE=''            # present-but-empty → assertions "(none)"
```

> `VIGIL_POWER_REQUEST_DIR` / `VIGIL_POWER_RESPONSE_DIR` end in the literal
> string `UID` (not the numeric uid) ON PURPOSE, so the helper-plist
> `--request-dir` / `--response-dir` args render to a stable, machine-independent
> path. The bash render substitutes these vars verbatim. (Live setup would use
> `$root/helper/requests/$(id -u)`; the goldens pin them for reproducibility.)

#### Command stubs (in `$ROOT/bin`, on PATH)

| stub | behavior |
|---|---|
| `date`      | `+%s` → `1700000000`; everything else → real `/bin/date` |
| `launchctl` | clean/pending vary (see deltas); default `exit 1` (= not loaded) |
| `pmset`     | `-g` → ` SleepDisabled\t\t<N>` where N is read from `$ROOT/sleepdisabled` (default 0). `-g therm/ps/assertions` are served from the `VIGIL_*_FIXTURE` seams and never reach the stub (it `exit 64`s if they do). |
| `sudo`      | logs to `$ROOT/sudo.log`, `exit 97` (non-mutating captures never invoke it) |
| `visudo`    | `exit 0` |
| `ps`        | pending capture: returns nothing for the `-axww` detect queries (deterministic empty detect); else `exec /bin/ps` |

### Per-capture deltas

**`setup_dryrun.txt`, `setup_dryrun_verbose.txt`, `user_agent.plist`,
`helper.plist`, `vigil.newsyslog`** — common env only. (Dry-run/render are
non-mutating and read no daemon/tick state.)

**`status_clean.json`, `status_clean.rust.json`, `status_text.txt`,
`status_verbose.txt`** — common env; `launchctl → exit 1` (not loaded);
`$ROOT/sleepdisabled = 0`; no `daemon.pid` / `daemon.tick` / `baseline.json` /
`caffeinate.pid`; `state/active/` empty.

**`status_engaged.json`** — common env, PLUS:
- `launchctl print gui/…` / `print system/…` → `exit 0` (loaded).
- `$ROOT/sleepdisabled = 1` → `pmset_disablesleep: 1`.
- `state/daemon.pid` = `4242`.
- `state/daemon.tick` (9 frozen fields):
  ```
  pid=4242
  updated_at=1699999998      # FIXED_NOW - 2 → age 2 < stale_after(15) → "fresh"
  tick_secs=5
  refcount_active=1
  desired_hold=1
  engaged=1
  thermal_cut=0
  battery_cut=0
  cooling=0
  ```
- `state/baseline.json` = `{"SleepDisabled":0,"captured_at":1699999900}` → `baseline: 0`.
- `state/active/wrapper-4243.pid` = a wrapper pidfile (wrappers ALWAYS count →
  `refcount_active: 1`, `refcount_total: 1`, independent of agent activity).
- A real `/usr/bin/caffeinate -i -t 120` was spawned; its pid written to
  `state/caffeinate.pid` → `caffeinate_alive: true`.

  ⚠️ **`caffeinate_pid` in `status_engaged.json` is NON-reproducible** — it is the
  live caffeinate process's real OS pid at capture time (`95947` in this capture).
  A Rust test reproducing the engaged state will get a different pid. The test
  MUST treat `caffeinate_pid` as a numeric wildcard (assert "is a number", not
  the literal `95947`) or regenerate the engaged golden with its own spawned pid.
  Every OTHER field in `status_engaged.json` IS reproducible.

**`status_pending.json`** — common env, PLUS:
- `launchctl print …` → `exit 0` (loaded).
- `state/daemon.pid` = `4242`, mtime pinned to FIXED_NOW (`touch -t 202311142213.20`)
  so it is NOT classified `missing` (age 0 ≤ `missing_after`).
- NO `daemon.tick` → tick `pid` ≠ daemon pid → scan state `pending`,
  `daemon_scan_age_secs: null`.
- `ps` stub returns nothing for the detect queries → `pending_active_matches: 0`
  (deterministic). The live-match sub-case (`pending_active_matches: 1`) requires
  a real-wall-clock `find -mmin` hit and is intentionally NOT captured here — it
  is not byte-reproducible (the agent-active age field would be a wall-clock
  delta). Skipped per §7 "skip if too fiddly."

**`status_assertions.json`** — common env, PLUS clean state and a representative
non-empty `VIGIL_ASSERTIONS_FIXTURE` (two holders, neither is our caffeinate):
```
Assertion status system-wide:
   PreventUserIdleSystemSleep            1
   PreventUserIdleDisplaySleep           0
Listed by owning process:
  pid 312(powerd): [0x0000000a00000457] 00:10:23 PreventUserIdleSystemSleep named: "com.apple.powermanagement.ttydisksleep"
  pid 988(Music): [0x0000000b000004a1] 01:02:03 PreventUserIdleDisplaySleep named: "com.apple.Music.playback"
No new entries.
```
→ `power_assertions_state: "ok"`, `power_assertions` = the 2 parsed holders
(`vigil:false` since no caffeinate pidfile is present). This exercises the
TSV→`ok` branch of the §2.3.3 tri-state parser. The empty-fixture `(none)` branch
is covered by all the other status captures.

## Reproducing

Re-run the capture by rebuilding `$ROOT` exactly as above, exporting the common
env + per-capture deltas, and running the bash `bin/vigil`. Determinism holds for
every field EXCEPT `caffeinate_pid` in `status_engaged.json` (documented above).
