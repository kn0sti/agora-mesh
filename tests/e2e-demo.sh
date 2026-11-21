#!/usr/bin/env bash
# `agora demo`: runs without an account, uses no real agent, and leaves nothing behind.
# Usage: tests/e2e-demo.sh [path-to-agora-binary]
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${1:-$HERE/../target/debug/agora}"
# The real path: that is how the daemons appear in the process list.
BIN="$(cd "$(dirname "$BIN")" && pwd -P)/$(basename "$BIN")"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
wait_for() { for _ in $(seq 1 "$1"); do eval "$2" && return 0; sleep 1; done; return 1; }
# The demo's daemons run as `<binary> daemon`.
daemons() { pgrep -f "^$BIN daemon\$" | wc -l | tr -d ' '; }

# A home with a setup that must stay as it is, a `claude` that must not be
# started, and git settings that would break a commit if the demo used them.
mkdir -p "$T/home" "$T/bin" "$T/tmp" "$T/outer"
printf '#!/bin/sh\ntouch "%s/claude-was-started"\nexit 1\n' "$T" > "$T/bin/claude"; chmod +x "$T/bin/claude"
printf '[commit]\n\tgpgsign = true\n[gpg]\n\tprogram = false\n[user]\n\tname = someone\n' > "$T/home/.gitconfig"
git init -q "$T/outer"
run() { (cd "$T/outer" && env -u AGORA_HOME -u AGORA_CLAUDE HOME="$T/home" USERPROFILE="$T/home" TMPDIR="$T/tmp" PATH="$T/bin:$PATH" GIT_DIR="$T/outer/.git" "$BIN" "$@"); }

run demo --fast > "$T/out" 2>&1; code=$?
check "the demo finishes" '[ "$code" = 0 ]'
check "the agents argue before they agree" 'grep -q "CRITIQUE" "$T/out" && grep -q "CONSENSUS" "$T/out"'
check "each pushes its own branch" '[ "$(grep -c "Checked on .* and pushed" "$T/out")" = 2 ]'
check "the two branches merge" 'grep -q "Both branches merge without a conflict" "$T/out"'
check "it says that the agents are scripted" 'grep -q "The agents are scripted" "$T/out"'
check "no real agent was started" '[ ! -e "$T/claude-was-started" ]'
check "it creates no setup of its own in the home" '[ ! -e "$T/home/.agora" ]'
check "the surrounding repository is untouched" '[ -z "$(git -C "$T/outer" for-each-ref)" ]'
check "the temporary folder is gone" '[ -z "$(ls -A "$T/tmp")" ]'
check "no daemon is left running" '[ "$(daemons)" = 0 ]'

# Ctrl-C in the middle of a room.
run demo > "$T/out2" 2>&1 & pid=$!
wait_for 60 'grep -q "KNOWN_MISSING" "$T/out2"'
home="$(ls -d "$T"/tmp/agora-demo-* 2>/dev/null | head -1)"
check "a second demo is running, with two daemons" '[ -n "$home" ] && [ "$(daemons)" = 2 ]'
pkill -INT -P "$pid" 2>/dev/null; kill -INT "$pid" 2>/dev/null
wait_for 20 '! kill -0 "$pid" 2>/dev/null'
check "Ctrl-C ends the demo" '! kill -0 "$pid" 2>/dev/null'
check "and removes its folder" 'wait_for 10 "[ -z \"\$(ls -A \"$T/tmp\")\" ]"'
check "and leaves no daemon running" 'wait_for 10 "[ \"\$(daemons)\" = 0 ]"'
kill -9 "$pid" 2>/dev/null

if [ "$fail" != 0 ]; then echo "--- out"; tail -30 "$T/out"; echo "--- out2"; tail -15 "$T/out2"; fi
exit $fail
