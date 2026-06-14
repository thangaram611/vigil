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
