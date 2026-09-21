# Vigil — viability review, 2026-08-04

> **Status: uncommitted working note.** Nothing in the repo was changed as a
> result of this review. No code was deleted, no docs edited, no tags cut.
> This file is the whole output. Delete it or act on it later.

---

## TL;DR

**No unique selling proposition survives scrutiny. Recommend archiving Vigil as a
product; keep it as a personal macOS tool if you actually use it.**

Three things happened between the project's start and today:

1. **Anthropic and OpenAI shipped Vigil's core into their own agents** — verified
   in the shipping binaries on this machine, not inferred from docs.
2. **A competitor took the macOS niche** — `adrafinil`, 428 stars, ships the
   `pmset disablesleep` clamshell hold + thermal cutout + hookless agent
   detection.
3. **Someone already built the exact cross-platform product this review was
   asked to evaluate** — `airtaxi/LidGuard`, all three OSes, full three-layer
   design. It has 10 stars.

The engineering is good and several design calls were independently correct. The
market absorbed the problem anyway.

---

## 1. Primary evidence (verified on this machine, macOS 26.5.2 / M5 Pro)

This is the strongest evidence in the review because it came from binaries, not
from search results.

| Tool | Self-inhibits sleep? | Mechanism | Platforms |
|---|---|---|---|
| Claude Code 2.1.220 | **Yes** | `caffeinate -i -t 300`; refcount engage on 0→1; respawn every 240s; SIGKILL 30s after last release; feature flag hardcoded enabled | **macOS only** — `if (platform === "macos") return [...]; return null;` |
| Codex CLI 0.146.0 | **Yes** | `IOPMAssertionCreateWithName(PreventUserIdleSystemSleep, "Codex is running an active turn")` | **macOS only** — crate `codex_utils_sleep_inhibitor` ships `macos.rs`; no `linux.rs`/`windows.rs` |
| GitHub Copilot CLI 1.0.71 | **No** | none — IOKit not linked; only slash command registered is `/wrapper`; every `keep-alive` string is an HTTP header | — |

Reproduce:

```bash
pmset -g assertions | grep -A3 caffeinate      # shows caffeinate parented to `claude`
ps -o pid,ppid,command -p <caffeinate_pid>
strings -a "$(which codex)" | grep -o "utils/sleep-inhibitor/src/[a-z_]*\.rs" | sort -u
nm -u "$(which codex)" | grep IOPM
```

**Implication:** Claude Code independently converged on Vigil's exact state
machine — idle-only assertion, refcounted engage/release, self-expiring so a
crash cannot strand the hold. Its 300s window equals Vigil's
`VIGIL_IDLE_AFTER_SEC` default. Its activity gate is *better* than Vigil's,
because it knows it is mid-turn rather than inferring it from session-file mtimes.

Corroborating: [anthropics/claude-code#21432](https://github.com/anthropics/claude-code/issues/21432)
is users asking Anthropic to turn the behaviour **off**. The loudest demand
signal in the ecosystem points opposite to Vigil's pitch.

### The one thing the vendors do not cover

Verified from `man caffeinate` on macOS 26:

- `-i` — "prevent the system from idle sleeping." Idle only.
- `-s` — "**valid only when system is running on AC power**."

So no assertion-based approach — vendor, hook, or wrapper — covers **unplugged +
lid closed**. Only a policy change (`pmset disablesleep`) can. `disablesleep` is
absent from `man pmset` entirely (0 hits); the `SleepDisabled` property still
exists in `IOPMrootDomain` on macOS 26.

Vigil's hardest architectural call — policy change over assertion, behind a root
helper — is **correct**. `adrafinil`'s README reaches the same conclusion
independently: *"public IOPM assertions don't beat clamshell sleep."*

---

## 2. Competitive landscape

All verified 2026-08-03/04.

| Tool | Platforms | Lid-close? | Notes | Stars |
|---|---|---|---|---|
| [kageroumado/adrafinil](https://github.com/kageroumado/adrafinil) | macOS | **Yes** (`pmset disablesleep`) | IOPMAssertion + clamshell, **thermal cutout**, **hookless process sniffing**, menu bar | **428** |
| [samber/cc-caffeine](https://github.com/samber/cc-caffeine) | mac/Linux/Win | No (Electron `powerSaveBlocker` = assertion) | Claude Code hooks only | 65 |
| [jmslau/claude-caffeine](https://github.com/jmslau/claude-caffeine) | macOS | **Yes** (`pmset` via sudoers helper) | IOKit clamshell sensor, **low-battery guard**, idle release | 9 |
| `airtaxi/LidGuard` | mac/Linux/Win | **Yes** | **Vigil's full design**: Win lid override + Linux `handle-lid-switch` + `pmset disablesleep` + baseline restore + thermal + agent hooks | **10** |
| `Bug-Finderr/wake-rs` | mac/Linux/Win | **Yes** (`--even-lid`) | Rust, same architecture | **1** |
| [getwarden.org](https://www.getwarden.org/) (Warden) | macOS | — | Input freeze + **Touch ID unlock**, marketed at *"Claude Code or Cursor running a long refactor"* | $3.99 app |
| [LidRun](https://lidrun.com/) | macOS | Yes | thermal in free tier | — |
| CharlonTank/agents-sleep-preventer, ramb5144/claude-stay-awake | macOS | varies | — | — |

**Market size reality check:** adrafinil converted 428 stars into **69 lifetime
Homebrew installs**. The whole category is plausibly a few hundred real users,
concentrated on macOS.

**Cross-platform demand:** zero port requests across ~170 issues on six competing
tools. One competitor's README explicitly solicits Linux requests and received
none. `claude-code#71875` (best-argued Windows request) sits at 0 reactions,
0 comments after five weeks.

---

## 3. Platform feasibility — corrected

### Linux — I was wrong; it is feasible and the problem is real

An earlier pass of this review claimed "Linux TAM ≈ 0 because Ubuntu defaults to
never-suspend on AC." **Refuted:**

- Ubuntu overrides **only** `sleep-inactive-ac-timeout=0`. Battery stays at
  upstream **900s → suspend**. (Verified by extracting 24.04.6 / 25.10.6 /
  26.04.6 `.deb`s.)
- **Fedora Workstation and Arch ship 900s on *both* rails.** (Verified by
  scanning Fedora 44 filelists metadata — the only package overriding the power
  schema is `fedora-release-identity-server`.)
- `HandleLidSwitch=suspend` is the upstream default and **no major distro
  overrides it** (Ubuntu 26.04 ships `logind.conf` with every key commented out).

Mechanism is **easier than macOS**: a `block`-mode `handle-lid-switch` inhibitor.
polkit default `org.freedesktop.login1.inhibit-handle-lid-switch` is
`allow_active=yes` → **no root, no helper, no sudoers**. The lock is an fd the
kernel releases on crash, so none of the baseline-capture / restore /
partial-engage-recovery machinery macOS needed exists here.

**Three traps that would ship broken code if Linux is ever resumed:**

1. **The current implementation does not do this.** `crates/vigil/src/power/linux.rs:49-51`
   holds `vec![Idle, Sleep]` only — no `handle-lid-switch`. As built, Vigil-Linux
   is idle-sleep-only, i.e. the same class as `caffeinate -i`, i.e. exactly the
   layer that just became redundant on macOS. The spec is also wrong:
   `future/phase-5.8-linux.md:43` and `:230` say *"do not default to
   `handle-lid-switch`."*
2. **Inactive sessions silently no-op.** `manager_handle_action()` checks the lid
   inhibitor with `MANAGER_IS_INHIBITED_IGNORE_INACTIVE`. A lock held from SSH or
   a background VT is skipped *for lid events* while still appearing in
   `loginctl list-inhibitors`. Vigil would report "engaged" and the laptop would
   sleep anyway. Needs a session-active probe with loud refusal.
3. **The `sleep` block lock also blocks the user's own manual suspend** — that is
   the bug class that gets a tool uninstalled. Correct design: GNOME session
   inhibitor for idle, logind lid lock for the lid, `sleep` block only while a
   turn is genuinely active.

**KDE note:** `org.freedesktop.PowerManagement.Inhibit` does **not** stop
lid-close suspend on Plasma (`HandleButtonEvents::triggerAction()` passes
`{"Explicit", true}`, and `Action::trigger()` bypasses `requirePolicyCheck()` on
that branch). The working layer on KDE is the logind `sleep` block lock.

**WSL2 is a detection case, not a platform.** logind runs there,
`systemd-inhibit` succeeds, `loginctl` shows the lock — and nothing reaches
Windows. A Linux binary without a WSL check installs cleanly, prints "engaged,"
and lies. Ship a hard refusal or don't ship Linux.

### Windows — my "structurally impossible" was a category error, but it is still unproven

I originally judged Windows on `SetThreadExecutionState` — an *assertion* API —
and never looked at the policy layer. That is the same error as judging macOS on
`caffeinate -i`. A real, maintained policy layer exists:

- `SelectLidCloseActionOnBattery` (GUID `5CA83367-6E45-459F-A27B-476B1D01C936`,
  value 0 = take no action), `StandbyTimeoutOnBattery`,
  `UnattendedSleepTimeoutOnBattery`, `HibernateTimeoutOnBattery`.
- Writable via `PowerWriteDCValueIndex` + `PowerSetActiveScheme`; GUIDs already
  in the official `windows` Rust crate. Admin once at install, not UAC per run.

**But end-to-end is unproven and the docs lean negative on Modern Standby:**

- `PowerSetRequest` Remarks: power requests "are terminated upon user-initiated
  system sleep entry (power button, **lid close** …)" and on Modern Standby
  **on DC power** are "terminated **5 minutes** after the system sleep timeout has
  expired." The DC 5-minute No-CS cap is stated twice.
- "The display will not turn on in clamshell mode if the lid is closed, as
  **input suppression is activated when on DC power**…" combined with "A system
  enters modern standby **when the display turns off**" reads as: `LIDACTION=0`
  suppresses the *action* but not standby *entry*.
- DAM (which suspends desktop apps) documents **no exemption** — only opt-in
  notification that you are about to be frozen. `PowerRequestExecutionRequired`
  exempts from PLM, which applies to Store apps.
- The single positive empirical data point (MS Q&A 1664813) is Win10 Pro,
  idle-induced not lid-induced, the reporter said the resolution was "not fully
  understood," and there is a 2026-04-28 follow-up reporting it **regressed**.

**Honest status: UNPROVEN, ~60/40 against on Modern Standby.** Likely fine on
legacy S3, a shrinking minority. Also unknown: Windows 11 **Home** honouring the
policy store; ARM/Surface behaviour; OEM/EC-level lid override. Only a hardware
test resolves this.

**Failure mode if wrong is silent total freeze**, not degradation. Thermal is a
liability shape, not a nicety — PowerToys #34479 is an open burn-hazard report
with the repro "close lid, put laptop in a bag."

---

## 4. The decision framework

A USP must clear **four** bars, not one:

1. **Real** — a capability competitors and vendors genuinely lack, verified in
   source or docs.
2. **Wanted** — someone other than you has voiced the pain.
3. **Defensible** — not erasable by a 3-line upstream PR or an OS settings toggle.
4. **Deliverable** — buildable *and testable* by a solo maintainer with a
   MacBook and no Windows hardware.

| Candidate | Real | Wanted | Defensible | Deliverable | Verdict |
|---|---|---|---|---|---|
| Cross-platform | ✅ | ❌ | n/a | ⚠️ | **Reach, not USP** |
| Lid-close survival on battery | ✅ | ⚠️ | ⚠️ | ✅ (macOS) | Shipped by adrafinil + claude-caffeine |
| "Tells you the truth / refuses loudly" | ✅ | ❌ | ✅ | ✅ | Elegant, market-untested |
| Baseline restore | ✅ | ❌ | ✅ | ✅ | Invisible when it works |
| GUI/IDE surfaces hooks can't see | ⚠️ unverified | ❓ | ⚠️ | ✅ | <1h to check; can't carry a product |
| Thermal + battery guards | ❌ | — | — | — | **Already commoditized** |

**Every candidate fails bar 2.** That is the answer.

**On cross-platform specifically:** an individual user runs one OS. A Windows
developer's alternative set is {PowerToys Awake, Insomnia, LidGuard}; Vigil also
running on macOS changes nothing in that comparison. Cross-platform is a project
asset (reach, contributors, credibility) and a feature-table row. It is not a
purchase reason.

**The decisive datum — closest thing to a controlled experiment available:**
`LidGuard` shipped Vigil's exact complete design across all three OSes and got
**10 stars**. `wake-rs` shipped cross-platform `--even-lid` and got **1**.
Meanwhile macOS-only `adrafinil` got 428 (→ 69 installs). Someone already ran
this strategy. The market returned nothing.

---

## 5. Recommendation

**Archive as a product.** Specifically:

1. **Do not build Windows.** Do not buy hardware. If ever revisited, the gate is
   a hardware test (below), not more reading.
2. **Do not finish Linux.** Not because it is infeasible — it is the cheapest
   platform Vigil would ever ship, ~30–50h, and the problem is genuinely real by
   default on every laptop. But "real problem, cheap fix, nobody cares" is
   exactly what LidGuard already demonstrated.
3. **Keep it as a personal macOS tool** if you use it. That needs no USP and is
   entirely legitimate.
4. **Optionally tag `v0.1.0` macOS-only and publish.** The code exists and works;
   this costs about a day.
5. **The highest-value artifact is the research, not the tool.** Nobody has
   written up "Claude Code and Codex self-inhibit sleep, macOS-only, here's the
   binary evidence, and here's why Windows probably can't do it." That post is
   worth more than the utility.

### What to cut if the project is ever resumed

- `/etc/systemd/logind.conf` root path and any Linux root helper — worse on every
  axis and inert on KDE.
- WSL2 as a supported target; replace with a hard refusal.
- Server / CI / container positioning.
- "Cross-platform" as tagline; also "nobody does cross-platform" and "unclaimed
  whitespace" — both refuted, both die to one search.
- `vigil lock` (~3,491 LOC incl. `combo.rs` 860 + `commands/lock.rs` 1,112):
  zero evidenced demand in any primary source, carries an Accessibility grant and
  a known `validate_combo`/`parse_chord` divergence hazard, and Warden ships a
  better version with Touch ID for $3.99.
- The all-phases release gate (`ROADMAP.md:3`, `:25`, `:29`;
  `future/phase-5.8-5.9-cross-os.md:192`). This was the single most
  value-destructive decision — it held a working macOS tool unreleased through
  the exact window in which adrafinil took the category.

---

## 6. Open experiments (none run yet)

### A. Clamshell test — the only thing that validates Vigil's core mechanism

Never run. Determines whether `pmset disablesleep=1` actually beats the Apple
Silicon magnet sensor on **M5 Pro**. Requires: DELL P2422H **disconnected** (an
external display means clamshell mode and a false pass), on battery, lid
physically shut.

A **control run is mandatory** because Claude Code holds `caffeinate -i` whenever
it is running — without the control, a "pass" would not prove `pmset` did
anything.

```
CONTROL   : disablesleep OFF, lid closed 5 min   -> machine MUST sleep
TREATMENT : disablesleep ON,  lid closed 20 min  -> machine must NOT sleep
```

Protocol:

```bash
# baseline
ioreg -rd1 -c IOPMrootDomain | grep SleepDisabled   # record it

# CONTROL
sudo pmset -a disablesleep 0
( while :; do date +%s; sleep 15; done >> /tmp/lid.log ) &
# close lid 5 min, reopen
kill %1; awk 'NR>1{d=$1-p; if(d>m) m=d} {p=$1} END{print "max gap:", m"s"}' /tmp/lid.log
pmset -g log | grep -E "\b(Sleep|Wake from|DarkWake)\b" | tail -12
# expect a gap > 60s  -> control valid

# TREATMENT
sudo pmset -a disablesleep 1
ioreg -rd1 -c IOPMrootDomain | grep SleepDisabled   # must read "Yes"
rm -f /tmp/lid.log
( while :; do date +%s; sleep 15; done >> /tmp/lid.log ) &
# close lid 20 min, reopen
kill %1; awk 'NR>1{d=$1-p; if(d>m) m=d} {p=$1} END{print "max gap:", m"s"}' /tmp/lid.log

# RESTORE — always
sudo pmset -a disablesleep 0     # or 1 if that was the recorded baseline
```

Interpretation: control gap >60s and treatment gap ≈15s → **pass**. Treatment
gap >60s → `pmset disablesleep` does not beat the magnet sensor on this hardware,
the root helper buys nothing over `caffeinate -i`, and macOS redundancy is total.

### B. adrafinil / keepresso source read (~1h)

Do they detect in-IDE GUI agents (VS Code + Copilot Chat, Claude.app LAM,
Codex.app), or only CLI processes? This is the last unverified differentiator.
Currently INFERRED from READMEs, not source.

### C. Windows hardware test (only if Windows is ever reconsidered)

One used Modern Standby Win11 24H2/25H2 laptop (~$300–600). Vary settings **one
at a time** or the result says nothing:

```
baseline -> LIDACTION(DC)=0 -> +STANDBYIDLE=0 -> +UNATTENDSLEEP=0 -> +HIBERNATEIDLE=0
```

Lid physically shut, on battery, busy console process. Read `powercfg /sleepstudy`
for whether the session stays in Screen Off or descends past DAM. Repeat
non-elevated vs elevated, scheme-write vs Policies-key, and on Home if possible.

---

## 7. What would change the verdict

- **A cross-platform lid-capable tool crosses ~200 stars** → cross-platform reach
  starts to look causal.
- **Windows hardware test comes back green on Modern Standby DC** → Windows moves
  from "cut" to "fund," though Linux-first ordering would still hold.
- **Competitor source shows adrafinil/keepresso already detect in-IDE GUI agents**
  → the last differentiator collapses (this makes the verdict *stronger*).
- **Anthropic ships an opt-out for its caffeinate** → the reliability argument
  weakens further.
- **You personally want to keep using and maintaining it** → entirely legitimate,
  changes the conclusion but not the scope: a one-person tool still should not
  carry 3,491 lines of event-tap code, a Windows phase, or a release gate.

**Unknowns not papered over:** Windows Home policy-store behaviour; ARM/Surface
lid behaviour; OEM/EC lid overrides; elogind (Devuan/Artix/Gentoo); COSMIC's
power stack; and real-world Reddit/forum demand volume on Windows and Linux,
which is genuinely *unmeasured* rather than measured-negative.

**Claim rejected during review:** research asserted GitHub Copilot CLI shipped
cross-platform `/keep-alive` since v1.0.36. Checked binary 1.0.71 — every
`keep-alive` string is an HTTP header, the only registered slash command is
`/wrapper`, and release notes 1.0.76–1.0.78 mention nothing. **Copilot CLI does
no sleep prevention.**

---

## 8. What was actually good here

Worth recording, because none of the above is a criticism of the engineering:

- The **policy-over-assertion call** (`pmset disablesleep` behind a root helper,
  because `caffeinate -s` dies on battery) is correct, and the 428-star category
  leader reached it independently.
- **Release correctness and baseline restore** are unmatched in the category.
  Every first-party implementation has an open bug from getting release wrong —
  including a leaked-caffeinate report at 4,567 processes and 47GB RAM.
- **System-only hold, never a display assertion** is the right design argument,
  and someone filed it as a bug against Anthropic (Claude Desktop uses
  `caffeinate -d`, pinning the screen on) which was closed *not planned*.
- The narrow fixed-argv IPC boundary, `O_NOFOLLOW` + fd-based validation, and
  772 lines of helper tests are careful work.

Roughly 4,600 lines carry all the surviving value. The rest is engineering that
was satisfying to write and that the market did not need.

---

## Addendum — 2026-09-20: native UI question + market refresh

**Question asked:** add native menu-bar/tray UI on mac/Linux/Windows (Phase 6), or
stay on the roadmap? **Answer: neither. Verdict of §5 stands and is stronger.**

### Correction to this review

The "Claim rejected" block above is **wrong**. Copilot CLI ships `/keep-alive`
(`on` / `off` / `busy` / duration) — documented in
[GitHub Docs](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/steer-remotely)
and the [2026-04-13 changelog](https://github.blog/changelog/2026-04-13-remote-control-cli-sessions-on-web-and-mobile-in-public-preview/).
The binary is a 150 MB Mach-O with a compressed JS payload; `strings`/`grep`
returning nothing (re-checked on 1.0.86) is a false negative, not evidence of
absence. It is opt-in per session
([copilot-cli#2880](https://github.com/github/copilot-cli/issues/2880) asks for a
persistent default). All three vendors now cover idle sleep. Codex has also
landed Linux + Windows inhibitors (`features.prevent_idle_sleep`, PR #11766,
experimental/off by default).

### Experiment B — closed

Read adrafinil source (`AdrafinilShared/.../Models/AgentKind.swift`). The sniffer
matches CLI basenames only: `claude`, `codex` (+ Homebrew triple-suffixed),
`cursor`, `gemini`, `aider`, `hermes`, `opencode`, `cline`, `pi`. No Copilot CLI,
no VS Code Copilot Chat. Issue #27 (IDE harness request) was closed with "paste
hooks manually." adrafinil also has `LowBatteryCutoutEvaluator` — battery guard
is commoditized too. It resets `disablesleep 0` on helper respawn rather than
restoring the prior value.

**Vigil-only residue:** hookless Copilot CLI + VS Code Copilot Chat detection,
prior-`SleepDisabled` baseline restore, integrated lock guard. Real, personally
useful, not a product.

### Does native UI move adoption? Evidence says no.

| Tool | UI | Platforms | Stars |
|---|---|---|---|
| adrafinil | menu bar + CLI, signed DMG, brew cask | macOS 26+ | 478 (was 428; v1.8 on 2026-09-19) |
| CharlonTank/agents-sleep-preventer | **Rust, menu bar + Windows tray** | mac/Win | 12 |
| samber/cc-caffeine | tray (Electron) | mac/Linux/Win | 71 — **deprecated**, author points to Amphetamine |
| stanley-projects/Insomnia | tray (Electron), winget/scoop | Windows | 3 |
| airtaxi/LidGuard | CLI + TUI dashboard | mac/Linux/Win | 12 |

- `agents-sleep-preventer` is the exact proposal (Rust + native UI + second OS). 12 stars.
- Menu bar is table stakes *for a distributed macOS app*; adrafinil pairs it with
  notarization + cask. Vigil has no release, so UI has nothing to attach to.
- Linux: GNOME ships no tray; SNI needs a user-installed extension. "Native UI on
  Linux" is ≥2 implementations for a platform with zero measured demand.
- HN thread (124 pts, 78 comments): no Linux/Windows asks; top pushback was
  "Amphetamine exists" and "Codex/Claude already do this."

### What to do

1. Drop Phase 6, 5.8, 5.9 and the all-phases release gate. Vigil = personal macOS tool.
2. Run **Experiment A** (clamshell control/treatment). Still never run. It is the
   only thing that tells you whether Vigil does anything Claude Code's own
   `caffeinate` doesn't on this machine.
3. If a glanceable indicator is wanted personally: SwiftBar/xbar plugin over
   `vigil status --json` (~20 lines of shell). No native UI code.
