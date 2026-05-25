#!/usr/bin/env python3
"""
Derive a `ps -o pid= -o comm=` fixture from a `ps -o pid= -o command=`
snapshot. Used to refresh tests/fixtures/ps-axww-comm-snapshot.txt
whenever ps-axww-snapshot.txt is regenerated.

macOS `ps -o comm=` prints the EXECUTABLE PATH only (no arguments).
That is:
- If the process was launched with an absolute path, comm is that full
  path — e.g. "/Applications/Codex.app/Contents/MacOS/Codex" or
  "/Users/.../Library/Application Support/Claude/.../MacOS/claude".
  These paths may contain literal spaces.
- If the process was launched via PATH lookup from a shell, comm is the
  bare basename — e.g. "claude" or "codex" (no leading slash).

We can't simply split on whitespace because the path may contain spaces.
We exploit two macOS bundle conventions plus an argument-boundary fallback:

1. `<prefix><bundle>.app/Contents/MacOS/<bundle>` — the MacOS subdir's
   executable always has the same name as the `.app` bundle. So the
   basename equals the segment immediately preceding `.app/`, even if
   that segment contains spaces or parens (`Codex Helper (Renderer)`).
   Captured via a named backreference.

2. `<prefix>.app/Contents/(Helpers|Resources)/<single-token>` and
   `<prefix>.framework/Versions/<ver>/Helpers/<single-token>` — the
   basename is a single token without spaces (e.g. `chrome-native-host`,
   `codex`, `chrome_crashpad_handler`). Matched with `[^ /]+`.

3. For everything else (no .app/.framework landmark), take the first
   whitespace-separated token as the exe — correct for absolute paths
   without spaces, relative paths like "./SomeBin", and bare basenames.
   Non-.app paths with embedded spaces (e.g. /Library/Application
   Support/...) are a known limitation: those rows in the fixture won't
   match live `ps -o comm=`, but they're all non-agent processes that
   detect.sh excludes via `command_line` substring match regardless.

Usage:
  python3 tests/fixtures/gen_comm_from_command.py \
      < tests/fixtures/ps-axww-snapshot.txt \
      > tests/fixtures/ps-axww-comm-snapshot.txt
"""

import re
import sys

# Case 1: MacOS subdir — basename equals .app bundle name (may contain spaces / parens).
APP_MACOS = re.compile(
    r"^(.+?(?P<bundle>[^/]+?)\.app/Contents/MacOS/(?P=bundle))(?:\s|$)"
)

# Case 2a: Helpers / Resources under .app — basename is a single space-free token.
APP_AUX = re.compile(
    r"^(.+?\.app/Contents/(?:Helpers|Resources)/[^ /]+)(?:\s|$)"
)

# Case 2b: Helpers under .framework — basename is a single space-free token.
FRAMEWORK_HELPER = re.compile(
    r"^(.+?\.framework/Versions/[^/]+/Helpers/[^ /]+)(?:\s|$)"
)


def exe_path(cmd: str) -> str:
    for rx in (APP_MACOS, APP_AUX, FRAMEWORK_HELPER):
        m = rx.match(cmd)
        if m:
            return m.group(1)
    return cmd.split(" ", 1)[0]


def main() -> None:
    src = sys.argv[1] if len(sys.argv) > 1 else None
    fh = open(src) if src else sys.stdin
    try:
        for line in fh:
            stripped = line.rstrip("\n").lstrip()
            if not stripped:
                continue
            m = re.match(r"^(\d+)\s+(.*)$", stripped)
            if not m:
                continue
            pid, cmd = m.group(1), m.group(2)
            print(f"{pid} {exe_path(cmd)}")
    finally:
        if src:
            fh.close()


if __name__ == "__main__":
    main()
