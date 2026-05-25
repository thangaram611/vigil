#!/usr/bin/env bash
# tests/experiments/phase-3/observe.sh — single-shot snapshot for phase-3
# desktop-app detection experiments. Designed to be invoked in a loop:
#
#   while sleep 5; do bash tests/experiments/phase-3/observe.sh codex-E1; done
#
# Each invocation appends ONE NDJSON line to:
#   tests/experiments/phase-3/runs/<label>.ndjson
#
# Captures:
#   ts           unix seconds
#   procs        ps rows matching any candidate substring (claude/codex/vscode)
#   mtimes       newest file mtime under each candidate session-dir root (or null)
#   sd_disable   pmset SleepDisabled at the time of the snapshot
#
# The output is intentionally not jq-shaped — every line is independent and
# can be filtered with `grep`/`awk` or read directly. Keeping the JSON keys
# short to make a long-running observation file scrollable in a terminal.

set -uo pipefail

label="${1:?usage: observe.sh <label>}"
self_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out_dir="$self_dir/runs"
mkdir -p "$out_dir"
out_file="$out_dir/${label}.ndjson"

ts=$(date +%s)
home="${HOME%/}"

# ---------------------------------------------------------------------------
# Candidate substrings. The list is intentionally wide — we want to OVER-
# capture during experiments and narrow in synthesis. Each substring is a
# literal grep -F pattern; no regex semantics.
# ---------------------------------------------------------------------------
read -r -d '' candidate_subs <<'EOF' || true
/Applications/Claude.app/
Library/Application Support/Claude/claude-code/
/.local/bin/claude
/.local/share/claude/
/Applications/Codex.app/
codex app-server
codex_chronicle
/Applications/Visual Studio Code
/Applications/Visual Studio Code - Insiders
Code Helper
copilot-language-server
copilot-chat
copilot-acp
.vscode-insiders/extensions/github.copilot
.vscode/extensions/github.copilot
EOF

# Build a single anchored grep pattern from the substrings, escaping for `grep -F`.
patterns_file=$(mktemp -t vigil-obs-patterns)
trap 'rm -f "$patterns_file"' EXIT
printf '%s\n' "$candidate_subs" > "$patterns_file"

# ---------------------------------------------------------------------------
# Process snapshot.
# ---------------------------------------------------------------------------
procs_json=""
sep=""
while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    # Trim leading whitespace and split pid + rest.
    line="${line#"${line%%[![:space:]]*}"}"
    pid="${line%% *}"
    cmd="${line#"$pid"}"; cmd="${cmd# }"
    [[ "$pid" =~ ^[0-9]+$ ]] || continue
    # JSON-escape the command string. Only need to handle backslash + double-quote;
    # control chars are extremely rare in argv on macOS and we accept any noise.
    esc="${cmd//\\/\\\\}"
    esc="${esc//\"/\\\"}"
    procs_json+="${sep}{\"pid\":${pid},\"cmd\":\"${esc}\"}"
    sep=","
done < <(ps -axww -o pid= -o command= 2>/dev/null | grep -F -f "$patterns_file" || true)

# ---------------------------------------------------------------------------
# Activity-file roots. For each, find the single newest file's mtime (or null
# if the directory is missing/empty). `find -mtime` would be cheaper but we
# want the raw timestamp for post-hoc analysis, so we stat the newest file.
# ---------------------------------------------------------------------------
roots=(
    "claude_projects:$home/.claude/projects"
    "claude_app_cc:$home/Library/Application Support/Claude/claude-code"
    "claude_app_sessions:$home/Library/Application Support/Claude/claude-code-sessions"
    "claude_app_lam:$home/Library/Application Support/Claude/local-agent-mode-sessions"
    "codex_sessions:$home/.codex/sessions"
    "codex_state:$home/.codex"
    "codex_app_support:$home/Library/Application Support/Codex"
    "copilot_session_state:$home/.copilot/session-state"
    "vscode_workspace_storage:$home/Library/Application Support/Code/User/workspaceStorage"
    "vscode_global_storage:$home/Library/Application Support/Code/User/globalStorage"
    "vscode_logs:$home/Library/Application Support/Code/logs"
    "vscode_ins_workspace_storage:$home/Library/Application Support/Code - Insiders/User/workspaceStorage"
    "vscode_ins_global_storage:$home/Library/Application Support/Code - Insiders/User/globalStorage"
    "vscode_ins_logs:$home/Library/Application Support/Code - Insiders/logs"
)

mtimes_json=""
sep=""
for entry in "${roots[@]}"; do
    name="${entry%%:*}"
    path="${entry#*:}"
    mt="null"
    if [[ -e "$path" ]]; then
        if [[ -d "$path" ]]; then
            # Newest file mtime in the tree (excluding directories). Bounded
            # depth + path excludes so a single tick stays under ~1s even on
            # deep Code/Claude support trees. Cache_Data, leveldb, GPUCache,
            # Crashpad are pure noise (Electron's internal storage that
            # advances every few seconds while the app is open) so we filter
            # them at the find layer rather than the analysis layer.
            newest=$(find "$path" -maxdepth 6 -type f \
                        -not -path '*/.git/*' \
                        -not -path '*/Cache_Data/*' \
                        -not -path '*/Code Cache/*' \
                        -not -path '*/GPUCache/*' \
                        -not -path '*/DawnGraphiteCache/*' \
                        -not -path '*/DawnWebGPUCache/*' \
                        -not -path '*/Crashpad/*' \
                        -not -path '*/blob_storage/*' \
                        -not -path '*/leveldb/*' \
                        -not -path '*/Shared Dictionary/*' \
                        -print0 2>/dev/null \
                     | xargs -0 stat -f '%m' 2>/dev/null | sort -nr | head -1)
        else
            newest=$(stat -f '%m' "$path" 2>/dev/null)
        fi
        [[ -n "$newest" ]] && mt="$newest"
    fi
    mtimes_json+="${sep}\"${name}\":${mt}"
    sep=","
done

# ---------------------------------------------------------------------------
# SleepDisabled (so we can verify vigil's behavior alongside the observation).
# ---------------------------------------------------------------------------
sd=$(pmset -g 2>/dev/null | awk '/SleepDisabled/ {print $NF}')
case "$sd" in 0|1) ;; *) sd="null" ;; esac

# ---------------------------------------------------------------------------
# Emit one line.
# ---------------------------------------------------------------------------
printf '{"ts":%s,"label":"%s","sd":%s,"procs":[%s],"mtimes":{%s}}\n' \
    "$ts" "$label" "$sd" "$procs_json" "$mtimes_json" >> "$out_file"

# Also echo a human-readable summary to stderr so an interactive caller sees
# what was captured this tick without tailing the file.
n_procs=$(printf '%s' "$procs_json" | grep -o '"pid":' | wc -l | tr -d ' ')
printf 'observe[%s] ts=%s procs=%s sd=%s -> %s\n' \
    "$label" "$ts" "$n_procs" "$sd" "$out_file" >&2
