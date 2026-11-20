#!/usr/bin/env bash
# End-to-end test of harness profiles: Claude Code pointed at another model
# service, and an agent that speaks the Agent Client Protocol. Uses local
# daemons and scripted stand-ins, so it uses no tokens. Needs python3.
# Usage: tests/e2e-harness.sh [path-to-agora-binary]
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${1:-$HERE/../target/debug/agora}"
T="$(mktemp -d)"
export AGORA_CLAUDE="$HERE/fake-claude.sh"
# The key of the pretended model service, and a key for Anthropic that must
# never reach that service.
export FAKE_SERVICE_KEY="service-key-5417"
export FAKE_EXPECT_TOKEN="$FAKE_SERVICE_KEY"
export ANTHROPIC_API_KEY="anthropic-key-9020"
export FAKE_ACP_TOOL=1
a() { AGORA_HOME="$T/a" "$BIN" "$@"; }
b() { AGORA_HOME="$T/b" "$BIN" "$@"; }
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
wait_for() { for _ in $(seq 1 "$1"); do eval "$2" && return 0; sleep 1; done; return 1; }
cleanup() { a stop >/dev/null 2>&1; b stop >/dev/null 2>&1; rm -rf "$T"; }
trap cleanup EXIT
AGENT="python3 $HERE/fake-acp.py"

a setup --name Mac >/dev/null; b setup --name Heimserver >/dev/null

# Profiles
check "without a file, Claude Code is the profile" 'a harness list | grep -q "^\* claude"'
check "a key is not accepted in place of its source" '! a harness add deepseek --key sk-plain-key >/dev/null 2>&1 && [ ! -f "$T/a/harness.toml" ]'
a harness add deepseek --key env:FAKE_SERVICE_KEY > "$T/add" 2>&1
check "a preset becomes a profile" 'a harness list | grep -q "deepseek .*api.deepseek.com/anthropic.*deepseek-flash.*external"'
check "adding says that the service is external" 'grep -q "Marked external" "$T/add"'
check "adding says what auto mode cannot do on another service" 'grep -q "cannot run its safety check" "$T/add"'
check "the profile names the key's source, not the key" 'grep -q "env:FAKE_SERVICE_KEY" "$T/a/harness.toml" && ! grep -q "$FAKE_SERVICE_KEY" "$T/a/harness.toml"'
check "a preset without a model asks for one" '! a harness add openrouter --key env:FAKE_SERVICE_KEY >/dev/null 2>&1'
a harness add ollama --model qwen3 >/dev/null
check "a local service is not external" 'a harness list | grep "ollama " | grep -vq external'
check "an unknown profile cannot become the default" '! a harness default nothing >/dev/null 2>&1'
a harness add acp --name agent --command "$AGENT" --local --model large >/dev/null
check "an agent becomes a profile" 'a harness list | grep -q "agent .*fake-acp.py"'
a harness remove ollama >/dev/null
check "a profile can be removed" '! a harness list | grep -q "ollama "'

# One session of each kind, without a network
a harness test deepseek > "$T/test-claude" 2>&1
check "Claude Code on another service answers and keeps the conversation" 'grep -q "answers: *yes" "$T/test-claude" && grep -q "remembers the last turn: yes" "$T/test-claude"'
check "its session is pointed at the service with the profile's key" 'grep -q "base=https://api.deepseek.com/anthropic token_ok=yes" "$T/a/fake-env.log"'
check "a key for Anthropic is not passed on to the service" 'grep -q "api_key=\[\]" "$T/a/fake-env.log"'
check "its session is told which model to ask for" 'grep -q "model=deepseek-flash\[1m\]" "$T/a/fake-env.log"'
check "its session starts bare and never in auto mode" 'tail -1 "$T/a/fake-args.log" | grep -q -- "--bare" && tail -1 "$T/a/fake-args.log" | grep -q -- "--permission-mode default"'
a harness test agent > "$T/test-acp" 2>&1
check "an agent answers in the room format and keeps the conversation" 'grep -q "answers: *yes" "$T/test-acp" && grep -q "keeps to the room format: yes" "$T/test-acp" && grep -q "remembers the last turn: yes" "$T/test-acp"'
check "the agent is given no connectors and no file or terminal access" 'grep -q "session/new cwd=bench mcp=0" "$T/a/fake-acp.log" && grep -q "terminal=false" "$T/a/fake-acp.log" && grep -q "\"readTextFile\": false" "$T/a/fake-acp.log"'
check "the profile's model is chosen from what the agent offers" 'grep -q "set model=fake-large" "$T/a/fake-acp.log" && grep -q "model: *fake-large" "$T/test-acp"'
check "tokens the agent does not report are marked as estimated" 'grep -q "not reported per turn" "$T/test-acp"'
FAKE_ACP_USAGE=total a harness test agent > "$T/test-acp2" 2>&1
check "a running total of tokens becomes a number per turn" 'grep -q "reported, 1000 read in the first turn, 1500 in the second" "$T/test-acp2"'
FAKE_ACP_USAGE=turn a harness test agent > "$T/test-acp2b" 2>&1
check "tokens reported per turn are taken as they are, cached input included" 'grep -q "reported, 1000 read in the first turn, 1040 in the second" "$T/test-acp2b"'
FAKE_ACP_AUTH=1 a harness test agent > "$T/test-acp3" 2>&1
check "an agent without a login says so" 'grep -q "needs a login first" "$T/test-acp3"'
FAKE_ACP_FS=1 a harness test agent >/dev/null 2>&1
check "the agent is not offered file access through the client" 'grep -q "^fs -32601" "$T/a/fake-acp.log"'
a bench --harness agent > "$T/bench" 2>&1
check "bench measures a profile" 'grep -Eq "^ +[0-9]+ +room session, harness \"agent\"" "$T/bench"'

# A network of two: Mac on Claude Code, Heimserver on the agent
TICKET="$(a open | grep -o 'agora1[a-z0-9]*')"
b join "$TICKET" >/dev/null
wait_for 30 'a status | grep -q "join request"'; a approve Heimserver >/dev/null
wait_for 30 'b status | grep -q "Mac *online"'
b trust Mac --level 2 >/dev/null
b harness add acp --name agent --command "$AGENT" --local >/dev/null
b harness default agent >/dev/null
check "the default profile is marked" 'b harness list | grep -q "^\* agent"'

a discuss "Plan the release" --with Heimserver --project none > "$T/room1" 2>&1
check "a room of Claude Code and an agent reaches consensus" 'grep -q "FINAL: the agreed plan" "$T/room1"'
ROOM="$(ls "$T/b/rooms" | head -1)"
check "the agent's session ran in the room and got the rules once" '[ "$(grep -c "rules=1" "$T/b/fake-acp.log")" = 1 ] && grep -q "rules=0" "$T/b/fake-acp.log"'
check "what the agent says before a tool call is not its contribution" '! grep -rq "Let me look at the files" "$T/a/rooms" "$T/b/rooms"'
check "an agent only reads while the room discusses: its command is refused" 'grep -q "permission .*\"optionId\": \"reject\"" "$T/b/fake-acp.log" && ! grep -q "\"optionId\": \"allow\"" "$T/b/fake-acp.log"'
check "the room's statistics name the harness and say that its tokens are estimated" 'grep -q "\"harness\":\"agent\"" "$T/b/rooms/$ROOM/stats.json" && grep -q "\"tokens_estimated\":true" "$T/b/rooms/$ROOM/stats.json"'
check "peers shows which harness a peer runs on" 'wait_for 10 "a peers | grep -q \"Heimserver.*\\[agent\\]\""'

: > "$T/b/fake-acp.log"
b trust Mac --rights read >/dev/null
a discuss "Plan the next release" --with Heimserver --project none > "$T/room2" 2>&1
check "a room with a peer that only grants reading still closes" 'grep -q "FINAL: the agreed plan" "$T/room2"'
check "an agent on a machine that grants only reading may not run commands" 'grep -q "permission .*\"optionId\": \"reject\"" "$T/b/fake-acp.log" && ! grep -q "\"optionId\": \"allow\"" "$T/b/fake-acp.log"'
b trust Mac --rights full >/dev/null

# A room with a work phase: the agent does its package in a second session
a stop >/dev/null; sleep 1; export FAKE_SPLIT=1; a status >/dev/null; unset FAKE_SPLIT
wait_for 40 'a status | grep -q "Heimserver *online"'
: > "$T/b/fake-acp.log"
a discuss "Build it" --with Heimserver --project none > "$T/room2b" 2>&1
check "a room of Claude Code and an agent does the work it split" 'grep -q "closed: work finished;" "$T/room2b"'
check "the agent works in a second session, and there it may use its tools" '[ "$(grep -c "^start" "$T/b/fake-acp.log")" = 2 ] && grep -q "\"optionId\": \"allow\"" "$T/b/fake-acp.log" && grep -q "\"optionId\": \"reject\"" "$T/b/fake-acp.log"'
a stop >/dev/null; sleep 1; a status >/dev/null
wait_for 40 'a status | grep -q "Heimserver *online"'

# The opener on another model service
a discuss "Plan the docs" --with Heimserver --project none --harness deepseek --model fable > "$T/room3" 2>&1
check "a room opened on another model service reaches consensus" 'grep -q "FINAL: the agreed plan" "$T/room3"'
check "on another service the discussing session runs bare and may only read" 'tail -1 "$T/a/fake-args.log" | grep -q -- "--bare" && tail -1 "$T/a/fake-args.log" | grep -q -- "--tools Read --permission-mode default"'
check "Claude Code on Anthropic's service is started as before" 'grep -- "--permission-mode auto" "$T/a/fake-args.log" | grep -vq -- "--bare"'
check "the room's model is not asked of another service" '! tail -1 "$T/a/fake-args.log" | grep -q -- "--model" && grep -rq "keeps its own model" "$T/a/rooms"'
check "no key appears in anything the daemons wrote" '! grep -rq -e "$FAKE_SERVICE_KEY" -e "$ANTHROPIC_API_KEY" "$T/a" "$T/b"'
check "an unknown profile fails before anyone is invited" '! a discuss "X" --with Heimserver --project none --harness nothing >/dev/null 2>&1'

# A task handed to a peer on another model service: that session may work
b harness add deepseek --key env:FAKE_SERVICE_KEY >/dev/null; b harness default deepseek >/dev/null
a delegate "Tidy the notes" --with Heimserver --project none > "$T/delegate" 2>&1
check "a task on another service is done by a bare session in the default permission mode" 'grep -q "Did the task" "$T/delegate" && tail -1 "$T/b/fake-args.log" | grep -q -- "--bare" && tail -1 "$T/b/fake-args.log" | grep -q -- "--permission-mode default"'
check "it may use the shared memory and nothing else without asking" 'tail -1 "$T/b/fake-args.log" | grep -q -- "--allowedTools Bash(agora memory),Bash(agora memory \*),Bash(agora remember \*)"'
b harness add deepseek --name unchecked --key env:FAKE_SERVICE_KEY --permission-mode bypassPermissions > "$T/add2" 2>&1
b harness default unchecked >/dev/null
a delegate "Tidy the notes again" --with Heimserver --project none >/dev/null 2>&1
check "a permission mode is used only when the profile names one" 'tail -1 "$T/b/fake-args.log" | grep -q -- "--permission-mode bypassPermissions" && grep -q "without any check" "$T/add2" && grep -q -- "--permission-mode auto" "$T/a/fake-args.log"'
check "no key appears in anything the daemons wrote, after all of this" '! grep -rq -e "$FAKE_SERVICE_KEY" -e "$ANTHROPIC_API_KEY" "$T/a" "$T/b"'
check "an unknown permission mode is refused" '! a harness add deepseek --name x --key env:FAKE_SERVICE_KEY --permission-mode everything >/dev/null 2>&1'

# A project that refuses outside services
mkdir -p "$T/proj/.agora" "$T/clone/.agora"
(cd "$T/proj" && git init -q && git remote add origin https://example.org/team/guarded.git && touch .agora/no-external)
(cd "$T/proj" && a discuss "Plan" --with Heimserver --harness deepseek > "$T/room4" 2>&1)
check "an external profile is refused in a project that says so" 'grep -q "refuses sessions that send it to an outside service" "$T/room4"'
(cd "$T/proj" && a discuss "Plan" --with Heimserver > "$T/room5" 2>&1)
check "Claude Code itself still works there" 'grep -q "FINAL: the agreed plan" "$T/room5"'
(cd "$T/clone" && git init -q && git remote add origin https://example.org/team/guarded.git && touch .agora/no-external && b project add >/dev/null)
b harness add deepseek --key env:FAKE_SERVICE_KEY >/dev/null; b harness default deepseek >/dev/null
(cd "$T/proj" && a discuss "Plan" --with Heimserver > "$T/room6" 2>&1)
check "a peer on an external profile declines a room about such a project" 'grep -q "declined the room.*refuses sessions that send it to an outside service" "$T/room6"'

exit $fail
