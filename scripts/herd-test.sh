#!/bin/sh
# herd-test.sh — run herdr-herd in a dedicated herdr session, isolated from the
# session you actually work in.
#
#   sh scripts/herd-test.sh
#
# Run it from a plain terminal tab: herdr refuses to nest by default, so this
# cannot start from inside an existing herdr session.
#
# What it does, in order:
#   1. builds the binary with the `dev-marker` feature, so every strip shows the
#      version, commit and build time it is running;
#   2. backgrounds a waiter that discovers the test session's socket and starts
#      the controller against it;
#   3. attaches to the test session in the foreground.
#
# The controller runs as an outside socket client. It needs only
# HERDR_SOCKET_PATH: it enumerates that session's tabs and injects the strips
# itself, so panes are never placed by hand. Strip panes it spawns inherit the
# right socket from that session's own server.
#
# Dry-run: `herd-test.sh --print-plan` prints the resolved plan and exits
# without building, spawning, or contacting herdr. tests/herd_test_script.rs
# uses it. HERDR_HERD_TEST_SESSION overrides the session name.
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SESSION=${HERDR_HERD_TEST_SESSION:-herd-test}
BIN="$REPO/target/release/herdr-herd"
# Own config dir, so a setting tried out here never leaks into the installed
# plugin's global config that the real session reads.
CONFIG_DIR=${HERDR_HERD_CONFIG_DIR:-$REPO/.herd-test/config}
LOG="$REPO/target/herd-test-controller.log"

if [ "${1:-}" = "--print-plan" ]; then
  echo "session: $SESSION"
  echo "build: cargo build --release --features dev-marker"
  echo "socket-lookup: herdr session list --json"
  echo "config-dir: $CONFIG_DIR"
  echo "controller: $BIN control"
  echo "attach: herdr --session $SESSION"
  exit 0
fi

command -v jq >/dev/null 2>&1 || {
  echo "herd-test: jq is required to read 'herdr session list --json'" >&2
  exit 1
}

cargo build --release --features dev-marker --manifest-path "$REPO/Cargo.toml"
mkdir -p "$CONFIG_DIR" "$REPO/target"

# The socket does not exist until the session does, and attaching blocks — so
# the controller waits in the background while the foreground attaches. A
# second controller for the same session exits on its own (the lock is keyed by
# socket path), so re-running this script is safe.
(
  tries=0
  while [ "$tries" -lt 300 ]; do
    sock=$(herdr session list --json 2>/dev/null |
      jq -r --arg s "$SESSION" \
        '.sessions[]? | select(.name == $s and .running) | .socket_path' |
      head -n1)
    if [ -n "${sock:-}" ] && [ -S "$sock" ]; then
      echo "herd-test: controller attaching to $sock"
      HERDR_ENV=1 \
        HERDR_SOCKET_PATH="$sock" \
        HERDR_HERD_CONFIG_DIR="$CONFIG_DIR" \
        exec "$BIN" control
    fi
    tries=$((tries + 1))
    sleep 0.2
  done
  echo "herd-test: session '$SESSION' never came up; controller not started" >&2
) >>"$LOG" 2>&1 &

echo "herd-test: controller log -> $LOG"
exec herdr --session "$SESSION"
