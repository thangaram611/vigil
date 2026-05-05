#!/usr/bin/env bash
# lib/refcount.sh — PID-file-based refcount with stale-GC.
#
# Each detected agent (and each `vigil run` wrapper) gets a file under
#   $VIGIL_ACTIVE_DIR/<name>-<pid>.pid
# containing a single JSON-ish line:
#   {"pid":<n>,"comm":"<exe>","start_ts":<unix>,"name":"<match-name>"}
#
# Daemon counts files; transitions trigger pmset enable/disable.
# Stale GC drops files where:
#   (a) the PID is dead, OR
#   (b) on-disk start_ts doesn't match the live PID's start time (PID reuse), OR
#   (c) age > VIGIL_STALE_AGE_SECS AND CPU% < VIGIL_STALE_CPU_PCT.

# shellcheck source=common.sh
source "${VIGIL_LIB_DIR:-$(dirname "${BASH_SOURCE[0]}")}/common.sh"

# ---- file ops -----------------------------------------------------------------

# Write/refresh a PID file. Args: <name> <pid> <exe> [start_ts]
vigil_refcount_touch() {
    local name="$1" pid="$2" exe="$3" start_ts="${4:-}"
    [[ -z "$start_ts" ]] && start_ts=$(vigil_pid_start_ts "$pid")
    local pidfile="$VIGIL_ACTIVE_DIR/${name}-${pid}.pid"
    # Escape exe for JSON-ish line. We're not robust to embedded "; daemon callers control input.
    local safe_exe="${exe//\"/}"
    printf '{"pid":%s,"comm":"%s","start_ts":%s,"name":"%s"}\n' \
        "$pid" "$safe_exe" "$start_ts" "$name" > "$pidfile"
}

# Wrapper invocation. Different prefix so the daemon's stale-GC treats it gently.
vigil_refcount_touch_wrapper() {
    local pid="$1" cmd="$2"
    local start_ts; start_ts=$(vigil_pid_start_ts "$pid")
    [[ -z "$start_ts" ]] && start_ts=$(vigil_now_unix)
    local pidfile="$VIGIL_ACTIVE_DIR/wrapper-${pid}.pid"
    local safe_cmd="${cmd//\"/}"
    printf '{"pid":%s,"comm":"wrapper","start_ts":%s,"cmd":"%s"}\n' \
        "$pid" "$start_ts" "$safe_cmd" > "$pidfile"
}

# Count of currently-alive PID files.
vigil_refcount_count() {
    local n=0
    if [[ -d "$VIGIL_ACTIVE_DIR" ]]; then
        # `find` to avoid glob errors when empty.
        n=$(find "$VIGIL_ACTIVE_DIR" -maxdepth 1 -type f -name '*.pid' 2>/dev/null | wc -l | tr -d ' ')
    fi
    printf '%s\n' "$n"
}

# List active matches as TSV: <pid>\t<name>\t<age_secs>
vigil_refcount_list() {
    local now; now=$(vigil_now_unix)
    if [[ -d "$VIGIL_ACTIVE_DIR" ]]; then
        find "$VIGIL_ACTIVE_DIR" -maxdepth 1 -type f -name '*.pid' 2>/dev/null | while read -r f; do
            local base; base=$(basename "$f" .pid)
            local pid="${base##*-}"
            local name="${base%-*}"
            local mtime; mtime=$(stat -f %m "$f" 2>/dev/null || echo 0)
            printf '%s\t%s\t%s\n' "$pid" "$name" "$((now - mtime))"
        done
    fi
}

# ---- stale GC ----------------------------------------------------------------

# Get the start time of a PID in unix seconds. Empty string if no such PID.
# Uses `ps -o lstart=` and parses via `date -j`.
vigil_pid_start_ts() {
    local pid="$1"
    local lstart
    lstart=$(ps -o lstart= -p "$pid" 2>/dev/null | sed 's/^ *//')
    [[ -z "$lstart" ]] && return 0
    # macOS `date -j -f` parser. Format from `ps -o lstart=`:
    #   "Mon May  5 15:14:23 2026"
    date -j -f "%a %b %e %T %Y" "$lstart" "+%s" 2>/dev/null
}

# CPU percent of a PID. Empty/0 if no such PID.
vigil_pid_cpu_pct() {
    local pid="$1"
    ps -o %cpu= -p "$pid" 2>/dev/null | awk '{$1=$1; print}'
}

# Compare two floats: returns 0 (true) if $1 < $2, else 1.
_vigil_lt() { awk -v a="$1" -v b="$2" 'BEGIN{exit !(a+0 < b+0)}'; }

# Drop stale PID files. Pass-through inputs come from environment tunables.
vigil_refcount_gc() {
    local now; now=$(vigil_now_unix)
    [[ -d "$VIGIL_ACTIVE_DIR" ]] || return 0
    find "$VIGIL_ACTIVE_DIR" -maxdepth 1 -type f -name '*.pid' 2>/dev/null | while read -r f; do
        local base; base=$(basename "$f" .pid)
        local pid="${base##*-}"
        local name="${base%-*}"
        local mtime; mtime=$(stat -f %m "$f" 2>/dev/null || echo 0)
        local age=$((now - mtime))
        # (a) dead PID — drop unconditionally
        if ! kill -0 "$pid" 2>/dev/null; then
            rm -f "$f"
            log DEBUG "gc dead pid=$pid name=$name"
            continue
        fi
        # (b) PID reuse — start_ts on disk doesn't match the live process's start time
        local on_disk_start
        on_disk_start=$(awk -F'[:,}]' '/start_ts/ {gsub(/[" ]/,"",$2); print $2; exit}' "$f" 2>/dev/null)
        local live_start; live_start=$(vigil_pid_start_ts "$pid")
        if [[ -n "$on_disk_start" && -n "$live_start" && "$on_disk_start" != "$live_start" ]]; then
            rm -f "$f"
            log DEBUG "gc pid-reuse pid=$pid name=$name on_disk=$on_disk_start live=$live_start"
            continue
        fi
        # (c) idle — old file + low CPU
        if (( age > VIGIL_STALE_AGE_SECS )); then
            local cpu; cpu=$(vigil_pid_cpu_pct "$pid")
            if [[ -n "$cpu" ]] && _vigil_lt "$cpu" "$VIGIL_STALE_CPU_PCT"; then
                rm -f "$f"
                log DEBUG "gc idle pid=$pid name=$name age=${age}s cpu=${cpu}%"
                continue
            fi
        fi
    done
}

# Wipe everything. Used by daemon shutdown / vigil uninstall.
vigil_refcount_clear() {
    [[ -d "$VIGIL_ACTIVE_DIR" ]] && find "$VIGIL_ACTIVE_DIR" -maxdepth 1 -type f -name '*.pid' -delete 2>/dev/null
}
