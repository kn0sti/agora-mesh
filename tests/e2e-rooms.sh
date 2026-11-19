#!/usr/bin/env bash
# End-to-end test of rooms with two daemons on one machine and a scripted
# stand-in for Claude (tests/fake-claude.sh), so it uses no tokens.
# Usage: tests/e2e-rooms.sh [path-to-agora-binary]
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${1:-$HERE/../target/debug/agora}"
T="$(mktemp -d)"
export AGORA_CLAUDE="$HERE/fake-claude.sh"
a() { AGORA_HOME="$T/a" "$BIN" "$@"; }
b() { AGORA_HOME="$T/b" "$BIN" "$@"; }
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
wait_for() { for _ in $(seq 1 "$1"); do eval "$2" && return 0; sleep 1; done; return 1; }
cleanup() { a stop >/dev/null 2>&1; b stop >/dev/null 2>&1; rm -rf "$T"; }
trap cleanup EXIT

a setup --name Mac >/dev/null; b setup --name Heimserver >/dev/null
b join "$(a open | grep -o 'agora1[a-z0-9]*')" >/dev/null
wait_for 30 'a status | grep -q "join request"'; a approve Heimserver >/dev/null
check "peers are connected" 'wait_for 15 "a status | grep -q \"Heimserver *online\" && b status | grep -q \"Mac *online\""'

# 1. Invitation needs `agora accept` while the inviter has level 1.
a discuss "Plan the migration" --turns 3 > "$T/out1" 2>&1 &
check "invitation waits for a human at level 1" 'wait_for 15 "b rooms | grep -q \"invitation .* from \\\"Mac\\\"\""'
ROOM="$(b rooms | sed -n 's/^invitation \([a-z0-9]*\) .*/\1/p')"
b accept "$ROOM" >/dev/null
wait
check "discussion ends in consensus" 'grep -q "closed: consensus" "$T/out1"'
check "the asking side gets the result" 'grep -q "FINAL: the agreed plan." "$T/out1"'
LOG="$T/a/rooms/$ROOM/log.md"
check "briefing comes first: BRIEF, KNOWN_MISSING, ANSWER" 'grep -q "#1 Mac · BRIEF" "$LOG" && grep -q "#2 Heimserver · KNOWN_MISSING" "$LOG" && grep -q "#3 Mac · ANSWER" "$LOG"'
check "then equal turns, joiner first" 'grep -q "#4 Heimserver · PROPOSAL · 2 left" "$LOG" && grep -q "#5 Mac · PROPOSAL · 2 left" "$LOG"'
check "both logs hold the same 7 messages" '[ "$(grep -c "^\[#" "$LOG")" = 7 ] && [ "$(grep -c "^\[#" "$T/b/rooms/$ROOM/log.md")" = 7 ]'
check "the joiner has the result too" 'wait_for 5 "grep -q FINAL \"$T/b/rooms/$ROOM/result.md\""'
check "rooms are gone after closing" 'wait_for 5 "a rooms | grep -q \"no rooms\" && b rooms | grep -q \"no rooms\""'

# 2. From level 2 on the invitation is accepted automatically; the budget ends a stubborn discussion.
b trust Mac --level 2 >/dev/null
a stop >/dev/null; b stop >/dev/null; sleep 1
export FAKE_STUBBORN=1
a status >/dev/null; b status >/dev/null
wait_for 40 'a status | grep -q "Heimserver *online"'
a discuss "Argue forever" --with Heimserver --turns 2 > "$T/out2" 2>&1
check "auto-accepted at level 2 and stopped by the turn budget" 'grep -q "closed: turn budget used up" "$T/out2"'
check "without consensus the human gets an overview" 'grep -q "OVERVIEW" "$T/out2"'
check "exactly 2 turns per peer after the briefing (7 messages)" 'grep -q "; 7 messages;" "$T/out2"'

# 3. A human can speak in a running room, and `agora panic` stops it.
a discuss "Stop me" --turns 30 > "$T/out3" 2>&1 &
wait_for 10 'a rooms | grep -q "^room "'
ROOM3="$(a rooms | sed -n 's/^room \([a-z0-9]*\) .*/\1/p')"
b say "$ROOM3" "Please also think of backups" >/dev/null 2>&1
a panic >/dev/null
wait
check "panic stops the room" 'grep -q "stopped by" "$T/out3"'
check "the human's message reached the other side's log" 'grep -q "Heimserver (human) · HUMAN" "$T/a/rooms/$ROOM3/log.md" || grep -q "closed" "$T/out3"'
check "no rooms left on either side" 'wait_for 10 "a rooms | grep -q \"no rooms\" && b rooms | grep -q \"no rooms\""'

if [ "$fail" = 0 ]; then
  echo "ALL PASSED"
else
  for f in out1 out2 out3; do echo "--- $f"; cat "$T/$f"; done
  echo "--- daemon log a"; tail -15 "$T/a/daemon.log"
  echo "--- daemon log b"; tail -15 "$T/b/daemon.log"
  [ -n "${ROOM:-}" ] && { echo "--- room log a"; cat "$T/a/rooms/$ROOM/log.md"; }
fi
exit "$fail"
