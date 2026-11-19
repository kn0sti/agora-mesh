#!/usr/bin/env bash
# End-to-end test of stage 1 with three daemons on one machine.
# Usage: tests/e2e-local.sh [path-to-agora-binary]
set -u
BIN="${1:-$(cd "$(dirname "$0")/.." && pwd)/target/debug/agora}"
T="$(mktemp -d)"
a() { AGORA_HOME="$T/a" "$BIN" "$@"; }
b() { AGORA_HOME="$T/b" "$BIN" "$@"; }
c() { AGORA_HOME="$T/c" "$BIN" "$@"; }
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
wait_for() { for _ in $(seq 1 "$1"); do eval "$2" && return 0; sleep 1; done; return 1; }
cleanup() { a stop >/dev/null 2>&1; b stop >/dev/null 2>&1; c stop >/dev/null 2>&1; rm -rf "$T"; }
trap cleanup EXIT

a setup --name Mac >/dev/null; b setup --name Heimserver >/dev/null; c setup --name Fremd >/dev/null
TICKET="$(a open | grep -o 'agora1[a-z0-9]*')"
check "open prints a network ID" '[ -n "$TICKET" ]'

b join "$TICKET" >/dev/null
check "join request reaches the opener" 'wait_for 30 "a status | grep -q \"join request: \\\"Heimserver\\\"\""'
WA="$(a status | sed -n 's/.*join request.*words: //p')"
WB="$(b status | sed -n 's/.*words: //p')"
check "both sides show the same four words ($WA)" '[ -n "$WA" ] && [ "$WA" = "$WB" ]'
check "not a member before approval" '! a status | grep -q "Heimserver .*online"'

a approve Heimserver >/dev/null
check "online on both sides after approval" 'wait_for 10 "a status | grep -q \"Heimserver *online\" && b status | grep -q \"Mac *online\""'

b stop >/dev/null
check "peer shown offline after it stops" 'wait_for 60 "a status | grep -q \"Heimserver *offline\""'
b status >/dev/null
check "reconnects on its own after a restart" 'wait_for 40 "a status | grep -q \"Heimserver *online\" && b status | grep -q \"Mac *online\""'

c join "$TICKET" >/dev/null
check "third peer appears as a join request, not as a member" 'wait_for 30 "a status | grep -q \"join request: \\\"Fremd\\\"\""'
a reject Fremd >/dev/null
check "rejected peer drops the network" 'wait_for 15 "! c status | grep -q \"waiting for approval\""'

# A member list that is out of date does not undo a revocation.
c join "$TICKET" >/dev/null
wait_for 30 'a status | grep -q "join request: \"Fremd\""'; a approve Fremd >/dev/null
check "three peers know each other" 'wait_for 30 "b status | grep -q \"Fremd *online\" && c status | grep -q \"Heimserver *online\""'
b stop >/dev/null
a revoke Fremd >/dev/null
check "the revoking peer no longer lists the revoked one" '! a status | grep -q Fremd'
b status >/dev/null
check "a peer that was away does not bring the revoked one back" 'wait_for 40 "a status | grep -q \"Heimserver *online\"" && sleep 3 && ! a status | grep -q Fremd'
check "and takes over the newer list itself" 'wait_for 20 "! b status | grep -q Fremd"'
check "the revoked peer is out of the network" 'wait_for 30 "! c status | grep -q \"network .* peer\""'

a revoke Heimserver >/dev/null
check "revoked peer is gone on the revoking side" '! a status | grep -q Heimserver'
check "revoked peer drops the network itself" 'wait_for 15 "! b status | grep -q \"Mac\""'

if [ "$fail" = 0 ]; then
  echo "ALL PASSED"
else
  echo "--- daemon log a"; tail -15 "$T/a/daemon.log"
  echo "--- daemon log b"; tail -15 "$T/b/daemon.log"
fi
exit "$fail"
