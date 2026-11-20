#!/usr/bin/env bash
# End-to-end test of the shared memory with three local daemons and the
# scripted stand-in for Claude, so it uses no tokens.
# Usage: tests/e2e-memory.sh [path-to-agora-binary]
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

a setup --name Mac >/dev/null; b setup --name Heimserver >/dev/null; c setup --name Laptop >/dev/null
TICKET="$(a open | grep -o 'agora1[a-z0-9]*')"
b join "$TICKET" >/dev/null
wait_for 30 'a status | grep -q "join request"'; a approve Heimserver --level 2 >/dev/null
wait_for 15 'b status | grep -q "Mac *online"'; b trust Mac --level 2 >/dev/null
NET="$(ls "$T/a/shared" 2>/dev/null | head -1)"

# Writing and replication
a remember "Deploys go through the run script." --title "Deploy" --importance high >/dev/null
NET="$(ls "$T/a/shared" | head -1)"
check "an entry reaches the other peer" 'wait_for 10 "b memory | grep -q \"^deploy .*high .*Mac\""'
check "it is a readable file there, listed in the index" 'grep -q "run script" "$T/b/shared/$NET/deploy.md" && grep -q "\[Deploy\](deploy.md)" "$T/b/shared/$NET/INDEX.md"'
check "high importance is announced on the other machine" 'b events | grep -q "memory: Deploy (by Mac"'
check "the writing machine is told too, for its other sessions" 'a events | grep -q "memory: Deploy"'

# Conflict: the later change wins, the older version is kept
b remember "Deploys go through CI now." --title "Deploy" >/dev/null
check "a later change replaces the entry on both sides" 'wait_for 10 "a memory show deploy | grep -q \"through CI\"" && b memory show deploy | grep -q "through CI"'
check "the older version is kept in history" '[ -f "$T/a/shared/$NET/history/deploy.1.md" ] && grep -q "run script" "$T/a/shared/$NET/history/deploy.1.md"'
check "normal importance is not announced" '[ "$(a events | grep -c "memory:")" = 1 ]'

# Importance low: kept, but not in the index
a remember "Scratch thought." --title "Scratch note" --importance low >/dev/null
check "low importance goes to the archive, not the index" 'wait_for 10 "[ -f \"$T/b/shared/$NET/archive/scratch-note.md\" ]" && ! grep -q "Scratch" "$T/b/shared/$NET/INDEX.md"'

# Block list
a block add "Weber" >/dev/null
check "text with a blocked term is refused" '! a remember "Offer for weber GmbH" --title "Offer" >/dev/null 2>&1 && ! a memory | grep -q "^offer "'
check "the refusal names the term" 'a remember "Offer for weber GmbH" --title "Offer" 2>&1 | grep -q "block list"'

# Obsidian mirror
mkdir -p "$T/vault"
a obsidian "$T/vault" >/dev/null
a remember "Mirror me." --title "Mirror" >/dev/null
b remember "From the other side." --title "Remote note" >/dev/null
check "new entries are mirrored into the vault folder" '[ -f "$T/vault/agora/$NET/mirror.md" ] && wait_for 10 "[ -f \"$T/vault/agora/$NET/remote-note.md\" ]"'

# Obsidian in the other direction: a note written in the vault becomes an entry everywhere
printf 'Backups run every night at two.\n' > "$T/vault/agora/$NET/Backup plan.md"
check "a note written in the vault reaches the other peer" 'wait_for 20 "b memory show backup-plan 2>/dev/null | grep -q \"every night at two\""'
check "it is filed under its id, with the file name as title" 'wait_for 5 "[ -f \"$T/vault/agora/$NET/backup-plan.md\" ] && [ ! -f \"$T/vault/agora/$NET/Backup plan.md\" ]" && grep -q "^title: Backup plan" "$T/vault/agora/$NET/backup-plan.md"'
sleep 3
printf '\nRestore tests are on Fridays.\n' >> "$T/vault/agora/$NET/backup-plan.md"
check "a change made in the vault replaces the entry on all peers" 'wait_for 20 "b memory show backup-plan | grep -q \"Restore tests\""'
check "an entry that came from a peer is not imported back as a change" '[ "$(ls "$T/a/shared/$NET/history" | grep -c "^remote-note")" = 0 ]'
printf 'Offer for Weber, 4200 EUR.\n' > "$T/vault/agora/$NET/Offer.md"
sleep 7
check "a note with a blocked term stays out of the shared memory" '! a memory | grep -q "^offer " && ! b memory | grep -q "^offer "'

# Forgetting
a forget mirror >/dev/null
check "forgetting removes the entry on all peers" 'wait_for 10 "! b memory | grep -q \"^mirror \"" && [ ! -f "$T/b/shared/$NET/mirror.md" ] && [ ! -f "$T/vault/agora/$NET/mirror.md" ]'

# Catching up after being away
b stop >/dev/null; sleep 1
a remember "Written while the other peer was off." --title "Away" >/dev/null
b status >/dev/null
check "a peer that was offline catches up when it reconnects" 'wait_for 40 "b memory | grep -q \"^away \""'

# A third peer gets everything when it joins, and entries pass through the mesh
c join "$TICKET" >/dev/null
wait_for 30 'a status | grep -q "join request"'; a approve Laptop >/dev/null
check "a new peer receives the existing entries" 'wait_for 30 "c memory | grep -q \"^deploy \" && c memory | grep -q \"^away \""'
c remember "From the third peer." --title "Third" >/dev/null
check "its entries reach both others" 'wait_for 15 "a memory | grep -q \"^third .*Laptop\" && b memory | grep -q \"^third .*Laptop\""'

# Room results and running sessions
a discuss "Plan the migration" --with Heimserver --turns 3 > "$T/out1" 2>&1
ROOM="$(ls -t "$T/a/rooms" | head -1)"
check "a finished room leaves its result in the shared memory of all peers" 'wait_for 10 "b memory show room-$ROOM-plan-the-migration | grep -q \"FINAL: the agreed plan\""'
a discuss "Do not keep this" --with Heimserver --turns 3 --remember none > "$T/out2" 2>&1
check "--remember none keeps a result out of the memory" '! a memory | grep -q "do-not-keep-this"'

a stop >/dev/null; b stop >/dev/null; sleep 1
export FAKE_STUBBORN=1 FAKE_DELAY=0.3
a status >/dev/null; b status >/dev/null
wait_for 40 'a status | grep -q "Heimserver *online" && b status | grep -q "Mac *online"'; sleep 2
a discuss "A long argument" --with Heimserver --turns 30 --remember none > "$T/out3" 2>&1 &
wait_for 10 'a rooms | grep -q "^room "'
ROOM3="$(a rooms | sed -n 's/^room \([a-z0-9]*\) .*/\1/p')"
sleep 2
b remember "The deadline moved to Monday." --title "Deadline" --importance high >/dev/null
check "running sessions on both machines are told about an important entry" 'wait_for 15 "grep -q \"Heimserver · CRITIQUE.*\" \"$T/a/rooms/$ROOM3/log.md\" && [ \"\$(grep -c saw-memory-notice \"$T/a/rooms/$ROOM3/log.md\")\" -ge 2 ] && grep -B1 saw-memory-notice \"$T/a/rooms/$ROOM3/log.md\" | grep -q \"Mac · CRITIQUE\" && grep -B1 saw-memory-notice \"$T/a/rooms/$ROOM3/log.md\" | grep -q \"Heimserver · CRITIQUE\""'
a panic >/dev/null
wait

if [ "$fail" = 0 ]; then
  echo "ALL PASSED"
else
  echo "--- memory a"; a memory
  echo "--- memory b"; b memory
  echo "--- events a"; a events
  echo "--- daemon log a"; tail -15 "$T/a/daemon.log"
  echo "--- daemon log b"; tail -15 "$T/b/daemon.log"
  [ -n "${ROOM3:-}" ] && { echo "--- room 3"; tail -20 "$T/a/rooms/$ROOM3/log.md"; }
fi
exit "$fail"
