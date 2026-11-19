#!/usr/bin/env bash
# End-to-end test of rooms with local daemons and a scripted stand-in for
# Claude (tests/fake-claude.sh), so it uses no tokens.
# Usage: tests/e2e-rooms.sh [path-to-agora-binary]
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${1:-$HERE/../target/debug/agora}"
T="$(mktemp -d)"
export AGORA_CLAUDE="$HERE/fake-claude.sh"
a() { AGORA_HOME="$T/a" "$BIN" "$@"; }
b() { AGORA_HOME="$T/b" "$BIN" "$@"; }
c() { AGORA_HOME="$T/c" "$BIN" "$@"; }
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
wait_for() { for _ in $(seq 1 "$1"); do eval "$2" && return 0; sleep 1; done; return 1; }
cleanup() { a stop >/dev/null 2>&1; b stop >/dev/null 2>&1; c stop >/dev/null 2>&1; rm -rf "$T"; }
trap cleanup EXIT
# Restarts the daemons so their sessions see a changed FAKE_* setting.
restart() {
  a stop >/dev/null; b stop >/dev/null; sleep 1
  a status >/dev/null; b status >/dev/null
  wait_for 40 'a status | grep -q "Heimserver *online"'
}

a setup --name Mac >/dev/null; b setup --name Heimserver >/dev/null; c setup --name Laptop >/dev/null
TICKET="$(a open | grep -o 'agora1[a-z0-9]*')"
b join "$TICKET" >/dev/null
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
check "tokens of all peers are added up (8 turns x 110)" 'grep -q "all peers: 880 tokens" "$T/out1"'
check "agora wait returns the result of a closed room" 'a wait "$ROOM" | grep -q "FINAL: the agreed plan."'

# 2. From level 2 on the invitation is accepted automatically; budgets end a stubborn discussion.
b trust Mac --level 2 >/dev/null
export FAKE_STUBBORN=1; restart
a discuss "Argue forever" --with Heimserver --turns 2 > "$T/out2" 2>&1
check "auto-accepted at level 2 and stopped by the turn budget" 'grep -q "closed: turn budget used up" "$T/out2"'
check "without consensus the human gets an overview" 'grep -q "OVERVIEW" "$T/out2"'
check "exactly 2 turns per peer after the briefing (7 messages)" 'grep -q "; 7 messages;" "$T/out2"'
a discuss "Argue expensively" --turns 20 --tokens 500 > "$T/out2b" 2>&1
check "the token budget stops the room" 'grep -q "closed: token budget used up" "$T/out2b"'
check "it stops at the first turn over the budget (5 messages)" 'grep -q "; 5 messages;" "$T/out2b"'

# 3. A human can speak in a running room, and `agora panic` stops it.
a discuss "Stop me" --turns 30 > "$T/out3" 2>&1 &
wait_for 10 'a rooms | grep -q "^room "'
ROOM3="$(a rooms | sed -n 's/^room \([a-z0-9]*\) .*/\1/p')"
b say "$ROOM3" "Please also think of backups" >/dev/null 2>&1
a panic >/dev/null
wait
check "panic stops the room" 'grep -q "stopped by" "$T/out3"'
check "no rooms left on either side" 'wait_for 10 "a rooms | grep -q \"no rooms\" && b rooms | grep -q \"no rooms\""'

# 4. NEED_HUMAN pauses the room; a human's answer from the other machine resumes it.
unset FAKE_STUBBORN; export FAKE_ASK=1; restart
a discuss "Pick a database" --turns 3 > "$T/out4" 2>&1
check "discuss returns with the question when a peer needs a human" 'grep -q "is paused" "$T/out4" && grep -q "Heimserver asks: Which database" "$T/out4"'
ROOM4="$(a rooms | sed -n 's/^room \([a-z0-9]*\) .*/\1/p')"
check "the room stays open while it waits" '[ -n "$ROOM4" ]'
b say "$ROOM4" "Use Postgres" >/dev/null
a wait "$ROOM4" > "$T/out4b" 2>&1
check "after the human answered the room reaches consensus" 'grep -q "closed: consensus" "$T/out4b"'
check "the human's answer is in the log as its own message" 'grep -q "Heimserver (human) · HUMAN" "$T/a/rooms/$ROOM4/log.md"'

# 5. Three peers: round-robin, joiners first.
unset FAKE_ASK; restart
c join "$TICKET" >/dev/null
wait_for 30 'a status | grep -q "join request"'; a approve Laptop >/dev/null
check "three peers form a full mesh" 'wait_for 30 "a status | grep -q \"Laptop *online\" && b status | grep -q \"Laptop *online\" && c status | grep -q \"Heimserver *online\""'
c trust Mac --level 2 >/dev/null
a discuss "Plan with three" --turns 3 > "$T/out5" 2>&1
check "three peers reach consensus in 10 messages" 'grep -q "closed: consensus; 10 messages" "$T/out5"'
ROOM5="$(ls -t "$T/c/rooms" | head -1)"
check "order is joiners, then opener" 'grep -q "#5 Heimserver · PROPOSAL" "$T/c/rooms/$ROOM5/log.md" && grep -q "#6 Laptop · PROPOSAL" "$T/c/rooms/$ROOM5/log.md" && grep -q "#7 Mac · PROPOSAL" "$T/c/rooms/$ROOM5/log.md"'

if [ "$fail" = 0 ]; then
  echo "ALL PASSED"
else
  for f in out1 out2 out2b out3 out4 out4b out5; do echo "--- $f"; cat "$T/$f" 2>/dev/null; done
  echo "--- daemon log a"; tail -15 "$T/a/daemon.log"
  echo "--- daemon log b"; tail -15 "$T/b/daemon.log"
fi
exit "$fail"
