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
check "both logs hold the same 6 messages" '[ "$(grep -c "^\[#" "$LOG")" = 6 ] && [ "$(grep -c "^\[#" "$T/b/rooms/$ROOM/log.md")" = 6 ]'
check "the joiner has the result too" 'wait_for 5 "grep -q FINAL \"$T/b/rooms/$ROOM/result.md\""'
check "rooms are gone after closing" 'wait_for 5 "a rooms | grep -q \"no rooms\" && b rooms | grep -q \"no rooms\""'
check "tokens of all peers are added up (7 turns x 110)" 'grep -q "all peers: 770 tokens" "$T/out1"'
check "agora wait returns the result of a closed room" 'a wait "$ROOM" | grep -q "FINAL: the agreed plan."'

# 2. From level 2 on the invitation is accepted automatically; budgets end a stubborn discussion.
b trust Mac --level 2 >/dev/null
check "trust changes only what is given" 'b trust Mac --rights read | grep -q "level 2 and rights read" && b trust Mac --rights full | grep -q "level 2 and rights full"'
export FAKE_STUBBORN=1; restart
a discuss "Argue forever" --with Heimserver --turns 2 > "$T/out2" 2>&1
check "auto-accepted at level 2 and stopped by the turn budget" 'grep -q "closed: turn budget used up" "$T/out2"'
check "without consensus the human gets an overview" 'grep -q "OVERVIEW" "$T/out2"'
check "exactly 2 turns per peer after the briefing (7 messages)" 'grep -q "; 7 messages;" "$T/out2"'
a discuss "Argue expensively" --turns 20 --tokens 500 > "$T/out2b" 2>&1
check "the token budget stops the room" 'grep -q "closed: token budget used up" "$T/out2b"'
check "it stops at the first turn over the budget (5 messages)" 'grep -q "; 5 messages;" "$T/out2b"'

# 3. A human can speak in a running room, and `agora panic` stops it.
# Slow answers, so that the room is still running when it is stopped.
export FAKE_DELAY=0.3; restart
a discuss "Stop me" --turns 30 > "$T/out3" 2>&1 &
# Both sides have to be in the room: stopped earlier, it ends with another message.
wait_for 10 'a rooms | grep -q "^room " && b rooms | grep -q "^room "'
ROOM3="$(a rooms | sed -n 's/^room \([a-z0-9]*\) .*/\1/p')"
b say "$ROOM3" "Please also think of backups" >/dev/null 2>&1
a panic >/dev/null
wait
check "panic stops the room" 'grep -q "stopped by" "$T/out3"'
check "no rooms left on either side" 'wait_for 10 "a rooms | grep -q \"no rooms\" && b rooms | grep -q \"no rooms\""'

# 4. NEED_HUMAN pauses the room; a human's answer from the other machine resumes it.
unset FAKE_STUBBORN FAKE_DELAY; export FAKE_ASK=1; restart
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
check "three peers reach consensus in 9 messages" 'grep -q "closed: consensus; 9 messages" "$T/out5"'
ROOM5="$(ls -t "$T/c/rooms" | head -1)"
check "order is joiners, then opener" 'grep -q "#5 Heimserver · PROPOSAL" "$T/c/rooms/$ROOM5/log.md" && grep -q "#6 Laptop · PROPOSAL" "$T/c/rooms/$ROOM5/log.md" && grep -q "#7 Mac · PROPOSAL" "$T/c/rooms/$ROOM5/log.md"'

# 6. The session works in the local clone of the room's project.
mkdir -p "$T/repoA" "$T/repoB"
git -C "$T/repoA" init -q && git -C "$T/repoA" remote add origin git@github.com:Example/Demo.git
git -C "$T/repoB" init -q && git -C "$T/repoB" remote add origin https://github.com/example/demo
b project add "$T/repoB" >/dev/null
(cd "$T/repoA" && a discuss "Look at the code" --with Heimserver --turns 3 > "$T/out6" 2>&1)
ROOM6="$(ls -t "$T/a/rooms" | head -1)"
check "the joiner's session runs in its own clone of the project" 'grep -q "cwd=repoB" "$T/a/rooms/$ROOM6/log.md"'
check "the opener's clone is registered by itself" 'a project list | grep -q "repoA.*github.com/example/demo"'
(cd "$T" && a discuss "No repository here" --with Heimserver --turns 3 > "$T/out6b" 2>&1)
ROOM6B="$(ls -t "$T/a/rooms" | head -1)"
check "without a project the session runs in the room directory" 'grep -q "cwd=$ROOM6B" "$T/a/rooms/$ROOM6B/log.md"'
check "agora stats lists the rooms on both sides" 'a stats | grep -q "^$ROOM6 *opened" && b stats | grep -q "^$ROOM6 *joined"'

# 7. An agreed split starts the work phase: every peer works on its package.
export FAKE_SPLIT=1; restart
(cd "$T/repoA" && a discuss "Build it" --with Heimserver --turns 3 > "$T/out7" 2>&1)
ROOM7="$(ls -t "$T/a/rooms" | head -1)"
LOG7="$T/a/rooms/$ROOM7/log.md"
check "a consensus on SPLIT_WORK starts the work phase" 'grep -q "closed: work finished;" "$T/out7" && grep -q "WORKRESULT" "$T/out7"'
check "each peer reports its own package" 'grep -q "Heimserver · DONE" "$LOG7" && grep -q "Mac · DONE" "$LOG7"'
check "each package gets a branch per peer" 'grep -q "branch agora/$ROOM7/heimserver" "$LOG7" && grep -q "branch agora/$ROOM7/mac" "$LOG7"'
check "the joiner worked in its own clone" 'grep -A2 "Heimserver · DONE" "$LOG7" | grep -q "cwd=repoB"'
check "agora status shows each peer's capacity" 'a status | grep -q "Heimserver.*42 % of its limit used" && b status | grep -q "Mac.*42 % of its limit used"'

# 8. A peer that reaches its usage limit hands its package to a peer with capacity.
a stop >/dev/null; b stop >/dev/null; sleep 1
a status >/dev/null
export FAKE_LIMIT=1; b status >/dev/null; unset FAKE_LIMIT
wait_for 40 'a status | grep -q "Heimserver *online"'
(cd "$T/repoA" && a discuss "Build it again" --with Heimserver --turns 3 > "$T/out8" 2>&1)
ROOM8="$(ls -t "$T/a/rooms" | head -1)"
LOG8="$T/a/rooms/$ROOM8/log.md"
check "the peer at its limit reports it" 'grep -q "Heimserver · LIMIT" "$LOG8"'
check "its package is handed over, branch included" 'grep -A1 "Mac · HANDOVER" "$LOG8" | grep -q "frontend goes from Heimserver to Mac.*Branch: agora/$ROOM8/heimserver"'
check "the taker finishes both packages" '[ "$(grep -c "Mac · DONE" "$LOG8")" = 2 ] && grep -q "closed: work finished;" "$T/out8"'
check "the peer is shown at its limit afterwards" 'a status | grep -q "Heimserver.*at its usage limit"'

# 8b. A peer that had nothing to add before a proposal was made has not
# answered that proposal: the room goes on until it does.
a stop >/dev/null; b stop >/dev/null; sleep 1
a status >/dev/null
export FAKE_PASS=1; b status >/dev/null; unset FAKE_PASS
wait_for 40 'a status | grep -q "Heimserver *online" && b status | grep -q "Mac *online"'; sleep 2
(cd "$T" && a discuss "Pass first" --with Heimserver --turns 3 --project none > "$T/out8b" 2>&1)
LOG8B="$T/a/rooms/$(ls -t "$T/a/rooms" | head -1)/log.md"
check "a pass before a proposal is not an answer to it" 'grep -q "Heimserver · PASS" "$LOG8B" && grep -q "Heimserver · CONSENSUS" "$LOG8B" && grep -q "closed: work finished\|closed: consensus" "$T/out8b"'

# 8c. A human's go before the work. With --approve the room waits once a split
# is agreed: go, say what to change, or cancel.
both_online() { wait_for 40 'a status | grep -q "Heimserver *online" && b status | grep -q "Mac *online"'; sleep 2; }
room_of() { sed -n 's/^\[room \([a-z0-9]*\) waits for your go.*/\1/p' "$1"; }
a stop >/dev/null; b stop >/dev/null; sleep 1
a status >/dev/null; b status >/dev/null; both_online
(cd "$T/repoA" && a discuss "Build with a go" --with Heimserver --turns 3 --approve > "$T/out8c" 2>&1)
ROOM8C="$(room_of "$T/out8c")"
LOG8C="$T/a/rooms/$ROOM8C/log.md"
check "with --approve an agreed split waits for a go and shows who does what" '[ -n "$ROOM8C" ] && grep -q "#1 Mac: backend (branch agora/$ROOM8C/mac)" "$T/out8c" && grep -q "#2 Heimserver: frontend (branch agora/$ROOM8C/heimserver)" "$T/out8c"'
check "nobody works before the go" '! grep -q "· WORK\]" "$LOG8C" && a rooms | grep -q "the agreed split waits for your go"'
a say "$ROOM8C" "Please keep the names short" --turns 1 >/dev/null
a wait "$ROOM8C" > "$T/out8c2" 2>&1
check "what a human says instead goes back to the peers, and the room waits again" 'grep -q "waits for your go" "$T/out8c2" && grep -q "· HUMAN\]" "$LOG8C" && [ "$(grep -c "· HOLD\]" "$LOG8C")" = 2 ] && ! grep -q "· WORK\]" "$LOG8C"'
a go "$ROOM8C" > "$T/out8c3" 2>&1
a wait "$ROOM8C" > "$T/out8c4" 2>&1
check "the go starts the work" 'grep -q "The work starts" "$T/out8c3" && grep -q "closed: work finished;" "$T/out8c4" && grep -q "Heimserver · DONE" "$LOG8C"'
check "a room that is not waiting takes no go" '! a go "$ROOM8C" >/dev/null 2>&1'

(cd "$T/repoA" && a discuss "Build, or not" --with Heimserver --turns 3 --approve > "$T/out8d" 2>&1)
ROOM8D="$(room_of "$T/out8d")"
a cancel "$ROOM8D" >/dev/null 2>&1
a wait "$ROOM8D" > "$T/out8d2" 2>&1
check "cancel closes the room without the work and keeps what was agreed" 'grep -q "closed: a human stopped the room before the work" "$T/out8d2" && grep -q "PLAN: the agreed split" "$T/out8d2" && ! grep -q "· WORK\]" "$T/a/rooms/$ROOM8D/log.md"'

# The same on the receiving side: a package that lands here waits for this machine's human.
b trust Mac --work ask > "$T/out8e0" 2>&1
check "a peer can be set so that its packages wait here" 'grep -q "waits for .agora go. here" "$T/out8e0"'
(cd "$T/repoA" && a discuss "Build, if they let us" --with Heimserver --turns 3 > "$T/out8e" 2>&1) &
wait_for 20 'b rooms | grep -q "waits for your go"'
ROOM8E="$(b rooms | sed -n 's/^room \([a-z0-9]*\) .*/\1/p' | head -1)"
check "a package from that peer waits on the receiving machine" 'b rooms | grep -q "package #2 \"frontend\" on branch agora/$ROOM8E/heimserver waits for your go"'
check "the other peer does its package meanwhile" 'wait_for 10 "grep -q \"Mac · DONE\" \"$T/a/rooms/$ROOM8E/log.md\""'
b go "$ROOM8E" > "$T/out8e2" 2>&1
wait
check "the go on the receiving machine lets it build" 'grep -q "being built on this machine" "$T/out8e2" && grep -q "closed: work finished;" "$T/out8e" && grep -q "Heimserver · DONE" "$T/a/rooms/$ROOM8E/log.md"'
(cd "$T/repoA" && a discuss "Build, and they say no" --with Heimserver --turns 3 > "$T/out8f" 2>&1) &
wait_for 20 'b rooms | grep -q "waits for your go"'
ROOM8F="$(b rooms | sed -n 's/^room \([a-z0-9]*\) .*/\1/p' | head -1)"
b cancel "$ROOM8F" >/dev/null 2>&1
wait
check "a package the receiving human refuses is reported as blocked" 'grep -A2 "Heimserver · BLOCKED" "$T/a/rooms/$ROOM8F/log.md" | grep -q "a human on Heimserver refused the package" && grep -q "closed: work finished with open packages" "$T/out8f"'
b trust Mac --work auto >/dev/null

# Limits of the work phase.
check "a package has to be allowed at least a minute" '! a discuss "X" --with Heimserver --project none --work-minutes 0 >/dev/null 2>&1'
a stop >/dev/null; b stop >/dev/null; sleep 1
export FAKE_SPLIT=2; a status >/dev/null; b status >/dev/null; export FAKE_SPLIT=1; both_online
(cd "$T/repoA" && a discuss "Build under a cap" --with Heimserver --turns 3 --work-tokens 1 > "$T/out8g" 2>&1)
LOG8G="$T/a/rooms/$(ls -t "$T/a/rooms" | head -1)/log.md"
check "with a token cap, a package that would start after it is used up is not started" 'grep -q "Not started: the work phase has used more than the 1 tokens" "$LOG8G" && [ "$(grep -c "Mac · DONE" "$LOG8G")" = 1 ] && grep -q "closed: work finished with open packages" "$T/out8g"'

# 9. A lean room discusses without tools and starts a second session for the
# work. If that one cannot start, the package is blocked; the session without
# tools does not do the work instead, and the room goes on for the others.
once() {
  # A stand-in that can be started once: the second start finds no program.
  printf '#!/usr/bin/env bash\nmv "$0" "$0.used"\nexec "%s/fake-claude.sh" "$@"\n' "$HERE" > "$T/once.sh"
  chmod +x "$T/once.sh"
}
once
a stop >/dev/null; b stop >/dev/null; sleep 1
a status >/dev/null
export AGORA_CLAUDE="$T/once.sh"; b status >/dev/null; export AGORA_CLAUDE="$HERE/fake-claude.sh"
# Both sides have to see the link: right after a restart one of two links is closed again.
wait_for 40 'a status | grep -q "Heimserver *online" && b status | grep -q "Mac *online"'; sleep 2
(cd "$T/repoA" && a discuss "Build it lean" --with Heimserver --turns 3 --lean > "$T/out9" 2>&1)
LOG9="$T/a/rooms/$(ls -t "$T/a/rooms" | head -1)/log.md"
check "a peer whose working session cannot start reports its package as blocked" 'grep -A2 "Heimserver · BLOCKED" "$LOG9" | grep -q "could not start a session with tools"'
check "the room goes on and closes with the other package done" 'grep -q "Mac · DONE" "$LOG9" && grep -q "closed: work finished with open packages" "$T/out9"'

once
a stop >/dev/null; b stop >/dev/null; sleep 1
export AGORA_CLAUDE="$T/once.sh"; a status >/dev/null; export AGORA_CLAUDE="$HERE/fake-claude.sh"
b status >/dev/null
wait_for 40 'a status | grep -q "Heimserver *online" && b status | grep -q "Mac *online"'; sleep 2
(cd "$T/repoA" && a discuss "Build it lean again" --with Heimserver --turns 3 --lean > "$T/out10" 2>&1)
LOG10="$T/a/rooms/$(ls -t "$T/a/rooms" | head -1)/log.md"
check "an opener whose working session cannot start blocks only its own package" 'grep -A2 "Mac · BLOCKED" "$LOG10" | grep -q "could not start a session with tools" && grep -q "Heimserver · DONE" "$LOG10"'
check "its room still closes with a result" 'grep -q "closed: work finished with open packages" "$T/out10" && grep -q "WORKRESULT" "$T/out10"'

if [ "$fail" = 0 ]; then
  echo "ALL PASSED"
else
  for f in out1 out2 out2b out3 out4 out4b out5 out6 out6b out7 out8 out8b out8c out8c2 out8c4 out8d2 out8e out8f out8g out9 out10; do echo "--- $f"; cat "$T/$f" 2>/dev/null; done
  echo "--- daemon log a"; tail -15 "$T/a/daemon.log"
  echo "--- daemon log b"; tail -15 "$T/b/daemon.log"
  [ -n "${LOG8:-}" ] && { echo "--- room log 8"; cat "$LOG8"; }
fi
exit "$fail"
