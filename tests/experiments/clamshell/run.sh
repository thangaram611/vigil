#!/usr/bin/env bash
# tests/experiments/clamshell/run.sh — what does this Mac really do when the lid
# closes under Vigil? Drives the real product path through `vigil mode`.
#
#   bash tests/experiments/clamshell/run.sh start <arm>    # then close the lid
#   bash tests/experiments/clamshell/run.sh finish         # after reopening it
#
# Arms, in the order to run them:
#
#   control    mode off. Lid closed 5 min. The Mac MUST sleep — otherwise
#              something other than Vigil holds it awake and no later arm means
#              anything.
#   treatment  mode on. Lid closed 20 min. Does `pmset disablesleep` beat the lid
#              sensor? Closed for only 2 min, it is also the no-fix half of the
#              lock comparison: note whether reopening shows the lock screen.
#   release    mode on. 2 min after the lid closes the script sets mode off; if
#              the Mac is still awake 3 min later it runs `pmset sleepnow`. Lid
#              closed 8 min. Does a release put a closed Mac to sleep, and does
#              `sleepnow` when it does not?
#   lock       mode on. 20 s after the lid closes the script runs
#              `pmset displaysleepnow`. Lid closed 2 min. Does that lock a held,
#              closed Mac without sleeping it? Note whether reopening shows the
#              lock screen.
#
# Preconditions, enforced by `start`: on battery, no external display. With a
# display attached the Mac is in clamshell mode and any result is a false pass.
#
# Method: a heartbeat writes `epoch SleepDisabled lidClosed locked` every 5s. A
# sleeping Mac writes nothing, so the largest gap between beats is the time
# spent asleep, and the other columns are observed rather than inferred.
#
# The `locked` column is only meaningful with the lid OPEN. With the lid closed
# macOS raises the lock when the display wakes, so the flag stays `No` even when
# reopening will show the lock screen. For a closed lid, what you see on
# reopening is the measurement.
#
# Do not touch the Vigil menu bar item during a run: switching the mode changes
# the hold under the test, and `finish` reports the arm as contaminated.
#
# `finish` always restores `vigil mode auto`. Runs land in runs/ (gitignored).

set -uo pipefail

self="${BASH_SOURCE[0]}"
self_dir="$(cd "$(dirname "$self")" && pwd)"
runs="$self_dir/runs"
beat_secs=5
asleep_gap_secs=60

die() { echo "clamshell: $*" >&2; exit 1; }

# `SleepDisabled lidClosed` from one ioreg read, e.g. `Yes No`.
power_state() {
    ioreg -rd1 -c IOPMrootDomain | awk -F'= ' \
        '/"SleepDisabled"/ { sd=$2 } /"AppleClamshellState"/ { lid=$2 } END { print sd, lid }'
}
locked() {
    if ioreg -n Root -d1 | grep -q 'CGSSessionScreenIsLocked"=Yes'; then echo Yes; else echo No; fi
}

# Hidden: the heartbeat loop.
cmd_beat() {
    while :; do
        echo "$(date +%s) $(power_state) $(locked)"
        sleep "$beat_secs"
    done
}

# Hidden: wait for the lid to close, then run the arm's timed actions, recording
# each as `epoch what` in the events file.
cmd_trigger() {
    local arm="$1" events="$2"
    until [ "$(power_state | cut -d' ' -f2)" = Yes ]; do sleep 1; done
    echo "$(date +%s) lid-closed" >>"$events"
    case "$arm" in
        release)
            sleep 120
            vigil mode off >/dev/null
            echo "$(date +%s) mode-off" >>"$events"
            sleep 180
            # Only ever sleep a closed Mac: if it slept on its own, this timer was
            # paused with it and resumes after the lid is already open again.
            if [ "$(power_state | cut -d' ' -f2)" = Yes ]; then
                echo "$(date +%s) sleepnow" >>"$events"
                pmset sleepnow >/dev/null
            fi
            ;;
        lock)
            sleep 20
            echo "$(date +%s) displaysleepnow" >>"$events"
            pmset displaysleepnow >/dev/null
            ;;
    esac
}

cmd_start() {
    local arm="${1:-}" mode want minutes
    case "$arm" in
        control)   mode=off; want=No;  minutes=5 ;;
        treatment) mode=on;  want=Yes; minutes=20 ;;
        release)   mode=on;  want=Yes; minutes=8 ;;
        lock)      mode=on;  want=Yes; minutes=2 ;;
        *) die "usage: run.sh start <control|treatment|release|lock>" ;;
    esac
    command -v vigil >/dev/null || die "vigil is not on PATH — run 'vigil setup' first"
    [ -e "$runs/current" ] && die "a run is in progress — 'run.sh finish' first"

    pmset -g batt | grep -q "'Battery Power'" || die "unplug the charger: the test must run on battery"
    local displays
    displays="$(system_profiler SPDisplaysDataType | grep -c 'Resolution:')"
    [ "$displays" -eq 1 ] || die "disconnect external displays ($displays found): clamshell mode is a false pass"

    vigil mode "$mode" >/dev/null || die "vigil mode $mode failed"
    local i got=""
    for i in $(seq 1 15); do
        got="$(power_state | cut -d' ' -f1)"
        [ "$got" = "$want" ] && break
        sleep 1
    done
    [ "$got" = "$want" ] || { vigil mode auto >/dev/null; die "SleepDisabled is '$got', wanted '$want' — see 'vigil status'"; }

    mkdir -p "$runs"
    local log="$runs/$arm-$(date +%Y%m%dT%H%M%S).log"
    nohup bash "$self" _beat >>"$log" 2>/dev/null &
    local beat_pid=$!
    nohup bash "$self" _trigger "$arm" "$log.events" >/dev/null 2>&1 &
    printf '%s\n%s\n%s\n%s\n' "$arm" "$log" "$beat_pid" "$!" >"$runs/current"

    echo "clamshell: $arm armed (vigil mode $mode, SleepDisabled=$got, battery, no external display)."
    echo "Close the lid for $minutes minutes, reopen, then: run.sh finish"
}

# Seconds asleep in [from, to): the sum of heartbeat gaps over the threshold.
asleep_between() {
    awk -v from="$2" -v to="$3" -v min="$asleep_gap_secs" \
        'NR>1 && p>=from && p<to && $1-p>min { s+=$1-p } { p=$1 } END { print s+0 }' "$1"
}

cmd_finish() {
    [ -e "$runs/current" ] || die "no run in progress"
    local arm log beat_pid trigger_pid
    { read -r arm; read -r log; read -r beat_pid; read -r trigger_pid; } <"$runs/current"
    kill "$beat_pid" ${trigger_pid:+"$trigger_pid"} 2>/dev/null
    rm -f "$runs/current"
    vigil mode auto >/dev/null

    local first last gap
    first="$(head -1 "$log" | cut -d' ' -f1)"; last="$(tail -1 "$log" | cut -d' ' -f1)"
    gap="$(awk 'NR>1 { d=$1-p; if (d>m) m=d } { p=$1 } END { print m+0 }' "$log")"
    echo "clamshell: $arm ran $(( (last - first) / 60 )) min, max heartbeat gap ${gap}s (asleep means > ${asleep_gap_secs}s)"

    echo "--- beats with the lid closed: SleepDisabled / locked ---"
    awk '$3=="Yes" { n++; sd[$2]++; lk[$4]++ }
         END { if (!n) { print "  none recorded"; exit }
               printf "  %d beats; SleepDisabled Yes=%d No=%d; locked Yes=%d No=%d\n", n, sd["Yes"], sd["No"], lk["Yes"], lk["No"] }' "$log"
    if [ -s "$log.events" ]; then
        echo "--- timed actions ---"
        while read -r at what; do echo "  $(date -r "$at" '+%H:%M:%S') $what"; done <"$log.events"
    fi
    echo "--- pmset log, sleep/wake events in the window ---"
    pmset -g log | awk -v from="$(date -r "$first" '+%Y-%m-%d %H:%M:%S')" \
        '$1" "$2 >= from && $4 ~ /^(Sleep|Wake|DarkWake)$/ && $5 != "Requests"' | cut -c1-160 | tail -20
    local from_ts
    from_ts="$(date -r "$first" '+%Y-%m-%dT%H:%M:%S')"
    echo "--- vigil daemon mode/hold changes in the window ---"
    vigil log 2>/dev/null | awk -v from="$from_ts" '$1 >= from && / (mode|engage|release) /' | tail -10

    # An arm only means something if the hold never changed under it, except
    # where the arm itself changes it: a control must never engage, and a
    # treatment or lock run must never release.
    local breach=""
    case "$arm" in
        control) breach=' engage ' ;;
        treatment | lock) breach=' release ' ;;
    esac
    if [ -n "$breach" ] && vigil log 2>/dev/null |
        awk -v from="$from_ts" -v b="$breach" '$1 >= from && index($0, b) { f=1 } END { exit !f }'; then
        echo "$(echo "$arm" | tr a-z A-Z) CONTAMINATED: the hold state changed during the run (see above). Re-run it, and leave the menu bar alone."
        return
    fi

    local at_off at_now
    case "$arm" in
        control)
            if [ "$gap" -gt "$asleep_gap_secs" ]; then
                echo "CONTROL VALID: with SleepDisabled off, the closed lid slept this Mac."
            else
                echo "CONTROL INVALID: the Mac never slept. Something else held it awake; no other arm would prove anything."
            fi
            ;;
        treatment)
            if [ "$gap" -gt "$asleep_gap_secs" ]; then
                echo "TREATMENT FAIL: the Mac slept with SleepDisabled on. disablesleep does not beat the lid sensor here."
            else
                echo "TREATMENT PASS: the Mac stayed awake with the lid closed, on battery."
            fi
            ;;
        release)
            at_off="$(awk '$2=="mode-off" { print $1 }' "$log.events" 2>/dev/null)"
            at_now="$(awk '$2=="sleepnow" { print $1 }' "$log.events" 2>/dev/null)"
            if [ -z "$at_off" ]; then
                echo "RELEASE INCOMPLETE: the lid was reopened before the release fired."
            elif [ "$(asleep_between "$log" "$at_off" "${at_now:-$last}")" -gt 0 ]; then
                echo "RELEASE SLEEPS: the closed Mac slept on its own after the release."
            elif [ -z "$at_now" ]; then
                echo "RELEASE INCOMPLETE: awake after the release, but the lid was reopened before sleepnow fired."
            elif [ "$(asleep_between "$log" "$at_now" "$last")" -gt 0 ]; then
                echo "RELEASE DOES NOT SLEEP, SLEEPNOW DOES: the closed Mac stayed awake 3 min after the release and slept on 'pmset sleepnow'."
            else
                echo "RELEASE DOES NOT SLEEP, NOR DOES SLEEPNOW: the closed Mac stayed awake through both."
            fi
            ;;
        lock)
            at_now="$(awk '$2=="displaysleepnow" { print $1 }' "$log.events" 2>/dev/null)"
            if [ -z "$at_now" ]; then
                echo "LOCK INCOMPLETE: the lid was reopened before displaysleepnow fired."
            else
                if [ "$gap" -gt "$asleep_gap_secs" ]; then
                    echo "LOCK BREAKS THE HOLD: the Mac slept after displaysleepnow."
                else
                    echo "LOCK KEEPS THE HOLD: awake throughout. Did reopening show the lock screen? That is the result."
                fi
            fi
            ;;
    esac
}

case "${1:-}" in
    start)    cmd_start "${2:-}" ;;
    finish)   cmd_finish ;;
    _beat)    cmd_beat ;;
    _trigger) cmd_trigger "$2" "$3" ;;
    *) die "usage: run.sh start <control|treatment|release|lock> | finish" ;;
esac
