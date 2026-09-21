# The Apple Silicon lid-closed reality

This is the technical caveat behind Vigil's best-effort closed-lid mode.
Default Vigil operation uses `pmset disablesleep` plus `caffeinate -i`: it
prevents system sleep as strongly as macOS allows while still allowing display
sleep and the native macOS Lock Screen.

## What `pmset disablesleep` actually does

`sudo pmset -a disablesleep 1` writes the boolean `kIOPMSleepDisabledKey` ("SleepDisabled") to system power settings via the private SPI `IOPMSetSystemPowerSetting(CFStringRef, CFTypeRef)`. This is the strongest software lever for sleep prevention that macOS exposes.

Verified by reading [`iccir/Fermata/Source/AppleSPI.h`](https://github.com/iccir/Fermata/blob/main/Source/AppleSPI.h) and [`Source/RestlessEngine.m`](https://github.com/iccir/Fermata/blob/main/Source/RestlessEngine.m) — Fermata's own private-SPI implementation:

```c
// From AppleSPI.h
extern IOReturn IOPMSetSystemPowerSetting(CFStringRef key, CFTypeRef value);
extern const CFStringRef kIOPMSleepDisabledKey;  // == CFSTR("SleepDisabled")
```

`pmset disablesleep` and Fermata's "disable lid-close sleep" feature **end up at the same kernel call with the same key**. There is no hidden private API that does more. The reason Fermata uses an SMJobBless privileged helper and Vigil uses a LaunchDaemon root helper is privilege-boundary UX — not capability.

## Why "lid closed" is fundamentally limited on Apple Silicon

From macOS Ventura onward, on Apple Silicon Macs, Apple introduced a **hardware-level magnet sensor** that triggers sleep when the lid closes — below the OS layer that `pmset disablesleep` reaches. The Apple-supported workflow for keeping a Mac awake with the lid closed is **clamshell mode**: external display + power adapter + external keyboard or mouse.

Sources:

- Apple Support, [Use a Mac with the lid closed (clamshell mode)](https://support.apple.com/en-us/102282) — Apple's official clamshell-mode guide; the only documented closed-lid workflow.
- Apple Support, [Keep your Mac laptop within acceptable operating temperatures](https://support.apple.com/en-in/102336) — thermal guidance for closed-lid use.
- Macworld, [How to use MacBook with lid closed and stop closed Mac sleeping](https://www.macworld.com/article/673295/how-to-use-macbook-with-lid-closed-stop-closed-mac-sleeping.html) — community-confirmed: software-only lid-closed prevention is unreliable on M-series.
- Pasquale Pillitteri, [Disable laptop sleep when lid closed for AI agents](https://pasqualepillitteri.it/en/news/779/disable-laptop-sleep-lid-close-ai-agents) — same conclusion in the AI-agent context.

Empirically, `pmset disablesleep 1` works **most of the time** on M-series, but the behavior is not Apple-supported and may regress with macOS updates. If you need reliable closed-lid operation for overnight agent runs, plug in an external display.

## Measuring it on this Mac

"Works most of the time" is not a measurement. [`tests/experiments/clamshell/run.sh`](../tests/experiments/clamshell/run.sh) is one: a heartbeat writes the time every 5 seconds, a sleeping Mac writes nothing, so the longest gap between beats is the time spent asleep.

It must run **on battery with no external display** — with a display attached the Mac is in clamshell mode and any result is a false pass. The script refuses to start otherwise.

```bash
bash tests/experiments/clamshell/run.sh start <control|treatment|release|lock>   # then close the lid
bash tests/experiments/clamshell/run.sh finish                                   # after reopening it
```

The control is mandatory. Agents hold their own `caffeinate -i` while they run, so without a run proving the closed lid *does* sleep this Mac, a passing treatment would not show that `disablesleep` did anything.

### Results — 2026-09-21, MacBook Pro M5 Pro, macOS 26.7, on battery, no external display

| Arm | Lid closed | What happened | Verdict |
| --- | --- | --- | --- |
| control (mode off) | 5 min | "Clamshell Sleep" 3 s after closing; asleep 321 s; lock screen on reopen | the lid sleeps this Mac — baseline valid |
| treatment (mode on) | 20 min | 240 beats, `SleepDisabled = Yes` on all, longest gap 6 s, no sleep events | **`disablesleep` beats the lid sensor** |
| release (mode on → off after 2 min) | 8 min | awake for the full 3 min after the release with `SleepDisabled = No`; asleep 5 s after `pmset sleepnow` | **a release does not sleep a closed Mac; `sleepnow` does** |
| treatment, short | 2 min | awake; reopened straight to the desktop | **a held, closed Mac is not locked** |
| lock (`displaysleepnow` 20 s after closing) | 2 min | awake, hold intact; reopened to the lock screen | **`displaysleepnow` locks it without breaking the hold** |

Two consequences, both now handled by the daemon:

- **Clamshell sleep is edge-triggered.** It fires when the lid closes. If sleep was disabled at that moment, re-enabling it later does nothing until the idle timer expires — and an agent's own `caffeinate -i` stops that timer too. So when Vigil releases with the lid closed, it runs `pmset sleepnow`. Without this the thermal and battery guards would release the hold and leave the Mac running in the bag.
- **A held Mac does not lock when the lid closes.** macOS locks on sleep or display sleep, and a held, closed Mac does neither until the display-sleep timer (15 min here) expires. So when the lid closes under a hold, Vigil runs `pmset displaysleepnow`.

Both apply only when `AppleClamshellCausesSleep` is true — with an external display attached, a closed lid is normal desk use and is left alone. The session-locked flag cannot verify the second fix: with the lid closed macOS raises the lock only when the display wakes, so the evidence is the paired observation above.

## Thermal safety

`man caffeinate` and Apple's thermal guidance both call out that running a laptop with the lid closed and no external cooling can lead to thermal throttling. Vigil's daemon parses `pmset -g therm` every tick and releases sleep prevention for at least 60 seconds when the kernel reports `CPU_Scheduler_Limit` or `thermal warning level`. This logic is borrowed from [`CharlonTank/agents-sleep-preventer`](https://github.com/CharlonTank/agents-sleep-preventer/blob/main/src/main.rs).

The thermal cutoff is fail-closed and always enforced: if `pmset -g therm` cannot be read, Vigil cuts the hold rather than risk holding the machine awake while blind to heat. There is no override.

## Practical recommendation

For overnight closed-lid agent runs:

1. Plug in an external display (any HDMI-equipped monitor works).
2. Plug in to AC.
3. Set the laptop on a hard, ventilated surface — not a bed, not a backpack, not under papers.
4. Leave a USB keyboard or wired mouse connected — Apple's clamshell-mode requirement.

Vigil does what software can do. The hardware caveats are not vigil bugs.
