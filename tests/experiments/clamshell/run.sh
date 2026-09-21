#!/usr/bin/env bash
# tests/experiments/clamshell/run.sh — does `pmset disablesleep` beat the lid
# sensor on this Mac? Drives the real product path: `vigil mode off` is the
# control, `vigil mode on` is the treatment.
#
#   bash tests/experiments/clamshell/run.sh start control     # then close the lid ~5 min
#   bash tests/experiments/clamshell/run.sh finish
#   bash tests/experiments/clamshell/run.sh start treatment   # then close the lid ~20 min
#   bash tests/experiments/clamshell/run.sh finish
#
# Preconditions, enforced by `start`: on battery, no external display. With a
# display attached the Mac is in clamshell mode and any result is a false pass.
#
# Method: a heartbeat writes the epoch every 5s. A sleeping Mac writes nothing,
# so the largest gap between beats is the time spent asleep. The control must
# show a gap (the lid really does sleep this Mac); the treatment must not.
#
# `finish` always restores `vigil mode auto`. Runs land in runs/ (gitignored).

set -uo pipefail

self_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
runs="$self_dir/runs"
beat_secs=5
asleep_gap_secs=60

die() { echo "clamshell: $*" >&2; exit 1; }
sleep_disabled() { ioreg -rd1 -c IOPMrootDomain | awk -F'= ' '/"SleepDisabled"/ {print $2}'; }

cmd_start() {
    local arm="${1:-}" want mode
    case "$arm" in
        control)   mode=off; want=No ;;
        treatment) mode=on;  want=Yes ;;
        *) die "usage: run.sh start <control|treatment>" ;;
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
        got="$(sleep_disabled)"
        [ "$got" = "$want" ] && break
        sleep 1
    done
    [ "$got" = "$want" ] || { vigil mode auto >/dev/null; die "SleepDisabled is '$got', wanted '$want' — see 'vigil status'"; }

    mkdir -p "$runs"
    local log="$runs/$arm-$(date +%Y%m%dT%H%M%S).log"
    nohup bash -c "while :; do date +%s; sleep $beat_secs; done" >>"$log" 2>/dev/null &
    printf '%s\n%s\n%s\n' "$arm" "$log" "$!" >"$runs/current"

    echo "clamshell: $arm armed (vigil mode $mode, SleepDisabled=$got, battery, no external display)."
    if [ "$arm" = control ]; then
        echo "Close the lid for 5 minutes, reopen, then: run.sh finish"
    else
        echo "Close the lid for 20 minutes, reopen, then: run.sh finish"
    fi
}

cmd_finish() {
    [ -e "$runs/current" ] || die "no run in progress"
    local arm log pid
    { read -r arm; read -r log; read -r pid; } <"$runs/current"
    kill "$pid" 2>/dev/null
    rm -f "$runs/current"
    vigil mode auto >/dev/null

    local first last
    first="$(head -1 "$log")"; last="$(tail -1 "$log")"
    local gap
    gap="$(awk 'NR>1 { d=$1-p; if (d>m) m=d } { p=$1 } END { print m+0 }' "$log")"
    echo "clamshell: $arm ran $(( (last - first) / 60 )) min, max heartbeat gap ${gap}s (asleep means > ${asleep_gap_secs}s)"

    echo "--- pmset log, sleep/wake events in the window ---"
    pmset -g log | awk -v from="$(date -r "$first" '+%Y-%m-%d %H:%M:%S')" \
        '$1" "$2 >= from && $4 ~ /^(Sleep|Wake|DarkWake)$/' | cut -c1-160 | tail -20
    echo "--- vigil daemon releases in the window ---"
    vigil log 2>/dev/null | awk -v from="$(date -r "$first" '+%Y-%m-%dT%H:%M:%S')" '$1 >= from && /release/' | tail -10

    if [ "$arm" = control ]; then
        if [ "$gap" -gt "$asleep_gap_secs" ]; then
            echo "CONTROL VALID: with SleepDisabled off, the closed lid slept this Mac."
        else
            echo "CONTROL INVALID: the Mac never slept. Something else held it awake; the treatment would prove nothing."
        fi
    else
        if [ "$gap" -gt "$asleep_gap_secs" ]; then
            echo "TREATMENT FAIL: the Mac slept with SleepDisabled on. disablesleep does not beat the lid sensor here."
        else
            echo "TREATMENT PASS: the Mac stayed awake with the lid closed, on battery."
        fi
    fi
}

case "${1:-}" in
    start)  cmd_start "${2:-}" ;;
    finish) cmd_finish ;;
    *) die "usage: run.sh start <control|treatment> | finish" ;;
esac
