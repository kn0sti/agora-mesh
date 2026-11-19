#!/usr/bin/env bash
# End-to-end test of the everyday commands and of rooms that lose a peer, with
# local daemons and the scripted stand-in for Claude, so it uses no tokens.
# Usage: tests/e2e-everyday.sh [path-to-agora-binary]
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
restart() {
  a stop >/dev/null; b stop >/dev/null; c stop >/dev/null 2>&1; sleep 1
  a status >/dev/null; b status >/dev/null; c status >/dev/null
  wait_for 40 'a status | grep -q "Heimserver *online" && a status | grep -q "Laptop *online"'
}

a setup --name Mac >/dev/null; b setup --name Heimserver >/dev/null; c setup --name Laptop >/dev/null
TICKET="$(a open | grep -o 'agora1[a-z0-9]*')"
b join "$TICKET" >/dev/null
wait_for 30 'a status | grep -q "join request"'; a approve Heimserver >/dev/null
c join "$TICKET" >/dev/null
wait_for 30 'a status | grep -q "join request"'; a approve Laptop >/dev/null
wait_for 30 'b status | grep -q "Mac *online" && c status | grep -q "Mac *online"'
b trust Mac --level 2 >/dev/null; c trust Mac --level 2 >/dev/null

# Status
check "status shows how a peer is reached" 'a status | grep -Eq "Heimserver +online \((direct|relay)"'

# agora ask
a ask "What is the deadline?" --with Heimserver > "$T/ask" 2>&1
check "ask returns the peer's answer" 'grep -q "The deadline is Friday" "$T/ask"'
check "ask starts no session on the asking machine" 'grep -q "none were used on this machine" "$T/ask" && [ ! -f "$T/a/fake-args.log" ]'
check "ask without a name fails when several peers are online" '! a ask "Who?" >/dev/null 2>&1'
check "the asked machine keeps the question in its statistics" 'b stats | grep -q "answered"'

# agora delegate and agora peers
(cd "$T" && a delegate "Tidy the notes" --with Heimserver --project none > "$T/delegate" 2>&1)
check "delegate lets the peer do the task and returns its report" 'grep -q "^DONE" "$T/delegate" && grep -q "Did the task" "$T/delegate" && grep -q "reported back after" "$T/delegate"'
check "peers lists everyone with reachability and usage limit" 'a peers | grep -Eq "^Heimserver +online \((direct|relay).* idle +42 % of its limit used" && a peers | grep -q "^Mac .*this machine"'
check "a peer without a session yet shows an unknown limit" 'a peers | grep -q "^Laptop .*limit unknown"'

# agora review
mkdir -p "$T/repoA" "$T/repoB"
G() { git -C "$1" -c user.name=t -c user.email=t@example.com "${@:2}"; }
G "$T/repoA" init -q; G "$T/repoA" remote add origin git@github.com:Example/Demo.git
echo "first line" > "$T/repoA/notes.txt"; G "$T/repoA" add notes.txt; G "$T/repoA" commit -q -m init
G "$T/repoB" init -q; G "$T/repoB" remote add origin https://github.com/example/demo
b project add "$T/repoB" >/dev/null
echo "changed line" > "$T/repoA/notes.txt"
(cd "$T/repoA" && a review --with Heimserver --base HEAD > "$T/review" 2>&1)
check "review sends the diff and returns the findings" 'grep -q "saw the diff" "$T/review"'
G "$T/repoA" checkout -q notes.txt
check "review says so when there is nothing to review" '(cd "$T/repoA" && a review --with Heimserver --base HEAD 2>&1) | grep -q "nothing to review"'

# agora notify
a notify "echo \"\$AGORA_EVENT_KIND: \$AGORA_EVENT_TEXT\" >> '$T/notify.log'" >/dev/null
a remember "The deadline moved." --title "Deadline" --importance high >/dev/null
check "every event runs the notify command" 'wait_for 5 "grep -q \"^memory: Deadline\" \"$T/notify.log\""'
a notify off >/dev/null

# A third peer joins a running room
export FAKE_STUBBORN=1 FAKE_DELAY=0.4; restart
a discuss "A long argument" --with Heimserver --turns 6 --remember none > "$T/late" 2>&1 &
wait_for 10 'a rooms | grep -q "^room "'
ROOM="$(a rooms | sed -n 's/^room \([a-z0-9]*\) .*/\1/p')"
sleep 3
a peers > "$T/peers-during" 2>&1
a invite "$ROOM" Laptop >/dev/null
wait
LOG="$T/a/rooms/$ROOM/log.md"
check "peers showed who was working while the room ran" 'grep -q "working on 1 room" "$T/peers-during"'
check "the late peer is announced" 'grep -q "Laptop · JOIN" "$LOG"'
check "it got what was said before instead of nothing" 'grep -A2 "Laptop · CRITIQUE" "$LOG" | grep -q "saw-catch-up"'
check "it takes its turns between the others" '[ "$(grep -c "Laptop · CRITIQUE" "$LOG")" -ge 2 ]'
check "its log starts where it joined, not at message 1" '! grep -q "^\[#1 " "$T/c/rooms/$ROOM/log.md" && grep -q "Laptop · JOIN" "$T/c/rooms/$ROOM/log.md"'
check "the peers that were there first still use all their turns" 'grep -q "Heimserver · CRITIQUE · 0 left" "$LOG" && grep -q "Mac · CRITIQUE · 0 left" "$LOG"'
check "after joining, the order is joiner, late peer, opener" 'grep "· CRITIQUE" "$LOG" | sed -n "/Laptop/,\$p" | sed -n "1,4p" | tr "\n" " " | grep -q "Laptop.*Mac.*Heimserver.*Laptop"'
check "the room still ends on its budget" 'grep -q "closed: turn budget used up" "$T/late"'

# A lean room discusses without tools and works with them
unset FAKE_STUBBORN FAKE_DELAY; export FAKE_SPLIT=1; restart
rm -f "$T/a/fake-args.log" "$T/b/fake-args.log"
(cd "$T/repoA" && a discuss "Build it lean" --with Heimserver --turns 3 --lean > "$T/lean" 2>&1)
check "a lean room finishes its work" 'grep -q "closed: work finished;" "$T/lean"'
check "its discussion sessions start without tools" 'head -1 "$T/a/fake-args.log" | grep -q -- "--tools *$" && head -1 "$T/b/fake-args.log" | grep -q -- "--tools *$"'
check "a second session with tools does the work on each side" '[ "$(grep -c . "$T/a/fake-args.log")" = 2 ] && [ "$(grep -c . "$T/b/fake-args.log")" = 2 ] && tail -1 "$T/b/fake-args.log" | grep -q -- "--permission-mode"'
ROOML="$(ls -t "$T/a/rooms" | head -1)"
check "the working sessions are told the agreed plan" '[ "$(grep -c "got-the-plan" "$T/a/rooms/$ROOML/log.md")" = 2 ]'

# A joiner's daemon restarts in the middle of a room
unset FAKE_SPLIT; export FAKE_STUBBORN=1 FAKE_DELAY=0.4; restart
a discuss "Lose the joiner" --with Heimserver --turns 20 --remember none > "$T/lost" 2>&1 &
wait_for 10 'a rooms | grep -q "^room "'
sleep 2
b stop >/dev/null; sleep 1; b status >/dev/null
wait
check "the room ends with an overview when a peer's daemon restarted" 'grep -q "closed: a peer left the room" "$T/lost" && grep -q "OVERVIEW" "$T/lost"'

# The opener's daemon restarts in the middle of a room
wait_for 40 'a status | grep -q "Heimserver *online"'
a discuss "Lose the opener" --with Heimserver --turns 20 --remember none > "$T/lost2" 2>&1 &
wait_for 10 'b rooms | grep -q "^room "'
sleep 2
a stop >/dev/null; sleep 1; a status >/dev/null
wait
check "the joiner lets go of the room once the opener is back without it" 'wait_for 40 "b rooms | grep -q \"no rooms\""'

if [ "$fail" = 0 ]; then
  echo "ALL PASSED"
else
  for f in ask delegate review peers-during late lean lost lost2; do echo "--- $f"; cat "$T/$f" 2>/dev/null; done
  echo "--- args a"; cat "$T/a/fake-args.log" 2>/dev/null
  echo "--- daemon log a"; tail -15 "$T/a/daemon.log"
  echo "--- daemon log b"; tail -15 "$T/b/daemon.log"
  [ -n "${LOG:-}" ] && { echo "--- late room"; cat "$LOG"; }
fi
exit "$fail"
