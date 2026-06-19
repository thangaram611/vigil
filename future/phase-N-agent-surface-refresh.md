# Phase N — Emerging agent surface refresh

> **Status: DEFERRED (2026-06-19 research note).** This is not the next
> implementation track. Revisit after the Linux/Windows slices, or earlier only
> if the user has a fresh local install of the target agent and can provide live
> process/session artifacts.

## Why this is deferred

Vigil's agent detector should track tools that are both important to real
overnight workflows and locally testable. Antigravity CLI is directionally
interesting, but it is newly overhauled and low-confidence as an immediate
implementation target:

- Google announced the Gemini CLI to Antigravity CLI transition on 2026-05-19,
  with consumer/free Gemini CLI access stopping on 2026-06-18. That makes the
  old `gemini` surface a migration target, not a strong new default.
- Antigravity CLI is positioned around background/multi-agent work, which does
  fit Vigil's purpose, but the tool is too new to treat its process names,
  history paths, and idle/activity markers as stable without a fresh local
  install.
- We cannot responsibly ship a detector based only on web docs. Existing Vigil
  detectors were validated against real process/session artifacts and have
  focused tests; this surface needs the same bar.

Primary research links:

- Google Developers Blog, "Transitioning Gemini CLI to Antigravity CLI":
  https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/
- Antigravity CLI repository/readme:
  https://github.com/google-antigravity/antigravity-cli
- Gemini CLI session-management docs:
  https://geminicli.com/docs/cli/session-management/

## Activation criteria

Before implementation, replace this note with a real plan that includes:

- a fresh install of the target CLI on this machine;
- captured `ps` rows for idle, active, background, and completed sessions;
- captured session/history file paths and update behavior;
- an activity gate that distinguishes idle-open from actively working;
- status/doctor JSON/text schema changes, if adding a new named agent;
- fixture-backed tests for process detection, activity gating, refcounting, and
  stale-session behavior.

## Explicit non-goal for the next phase

Do not add Antigravity/Gemini support in Phase 5.8 or 5.9 unless it is needed to
prove the cross-OS abstractions. The next product work is the multi-OS port:
Linux first, then Windows.
