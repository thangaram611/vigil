# Phase N — Conflict-detection-driven release

> **Status: SKETCH ONLY.** Replace with a detailed plan before implementation.
> Phase number deliberately TBD — this can land in any phase after the
> `vigil_assertions_summary` building block in phase 1, but is gated on real-
> world need, not on a calendar slot.

## Why

Today Vigil's hold engages unconditionally whenever its refcount transitions
from 0 to ≥1: `pmset -a disablesleep 1` plus `caffeinate -i`. The
baseline-stickiness design (see `docs/architecture.md` → "Why baseline
restoration matters") correctly handles the `disablesleep` *release* side when
another tool was already holding `SleepDisabled=1` — we restore to the captured
value and don't clobber it.

But the *engage* side has no conflict awareness:

- If Amphetamine is already preventing system idle sleep, Vigil still spawns a
  second `caffeinate -i` child for the same effect. Redundant, but harmless.
- If another `caffeinate -i` is already in flight (e.g. from a user's shell
  alias), same redundancy.
- If a system process — `powerd`, `bluetoothd`, `sharingd`, `loginwindow` — is
  holding the equivalent system-idle IOKit assertion, Vigil's caffeinate adds
  nothing.

Phase 1 added `vigil_assertions_summary` (parses `pmset -g assertions`,
tags vigil's own caffeinate with `← vigil`). That parser is the building
block for skipping the redundant caffeinate spawn when an external holder
already covers the relevant assertion type — and for delaying release while
an external holder remains.

## Direction

Two complementary behaviors:

1. **Engage-time skip.** On the 0 → ≥1 refcount transition:
   - Run `vigil_assertions_summary`.
   - If any non-Vigil holder asserts `PreventUserIdleSystemSleep` (i.e.
     equivalent to default `caffeinate -i`), record the holders in
     `state/conflict.json` and skip the caffeinate spawn for this engage cycle.
   - Still flip `pmset -a disablesleep 1` ourselves — that's a separate lever
     from IOKit caffeinate assertions and not all tools touch both.
2. **Release-time deferral.** On the ≥1 → 0 refcount transition:
   - If `state/conflict.json` exists, re-check `vigil_assertions_summary`
     before releasing.
   - If the original external holders (or any equivalent-shape external
     holder) are still present, don't restore baseline — defer until they
     drop too. Re-check each tick.
   - This prevents "vigil released, Amphetamine still wants sleep prevention,
     but the user's screen dimmed anyway because vigil flipped SleepDisabled
     back to 0" — except we already don't flip it back if baseline was 1, so
     this scenario only matters when baseline was 0 and an external tool
     started holding sleep AFTER vigil engaged.

## Open questions

- **Equivalence rule.** Which assertion combinations count as "covers what our
  caffeinate would have provided"? `PreventUserIdleSystemSleep` is the default
  match. Should we also consider `PreventSystemSleep` (which only works on AC)
  as an equivalent? Answer probably depends on whether the refcounted agent is
  doing CPU work or background/network work.
- **Detection cost.** `pmset -g assertions` spawns an external binary and
  parses unstable output. Running it every tick (5s) adds 12 spawns/min just
  for conflict awareness. Acceptable, but worth measuring.
- **Race window.** Between engage-time check and the caffeinate spawn, the
  external holder could release. Our refcount would then be ≥1 with NO
  caffeinate. Mitigation: detect missing-but-expected caffeinate at tick
  boundaries and spawn one then. Or accept the race as benign (next tick
  catches it; max one tick of missing prevention).
- **External tool semantics.** Amphetamine and `caffeinate -i` users likely
  don't expect another tool to piggyback. If vigil skips its caffeinate, the
  user closes Amphetamine, vigil now has refcount ≥1 but no caffeinate —
  detection must catch this. (See "Race window" above.)
- **State file lifecycle.** `state/conflict.json` lives only between engage
  and release. Stale files (daemon crash mid-engage) are already handled by
  `vigil uninstall` and the `vigil_pmset_clear_baseline` path; conflict.json
  follows the same lifecycle.
- **Cross-OS portability.** This feature is built on `pmset -g assertions`,
  which has no Linux/Windows equivalent. Phase 5 (Rust cross-OS) would need
  per-OS implementations: D-Bus `Inhibit` introspection on Linux,
  `PowerGetEffectiveOverlayScheme` / event-tracing on Windows. Whether the
  feature is worth the per-OS effort is part of the phase-5 scope decision.

## What this is NOT

- **Not a replacement for baseline restoration.** Baseline handles the case
  where SleepDisabled was already 1 at first engage. Conflict detection
  handles the additional case where someone STARTS holding sleep mid-engage,
  or where we'd be redundant if we engaged at all.
- **Not a way to share refcount with other tools.** Other tools' refcounts
  are opaque to vigil. We only observe their assertions.
- **Not a hard dependency for phase 1 correctness.** Phase 1 already releases
  cleanly when baseline=1 was captured; this feature is an optimization +
  edge-case hardening, not a correctness fix.

## When this phase begins

Replace this file with: the equivalence rule (assertion combinations), the
state-file schema, the engage- and release-tick algorithms in pseudocode,
test fixtures (multi-holder, holder-disappears, holder-appears-mid-engage),
and the trade-off decision on detection cost vs. tick frequency.
