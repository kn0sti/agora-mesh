#!/usr/bin/env python3
"""Stand-in for a coding agent that speaks the Agent Client Protocol, version 1,
on stdin and stdout. It answers like tests/fake-claude.sh, without using tokens.

  FAKE_ACP_TOOL=1     before it answers "Your turn" or works on a package it
                      says a sentence, calls a tool and asks for permission;
                      the outcome goes to the log
  FAKE_ACP_USAGE      "total": reports a running total of tokens with every
                      answer; "turn": reports the tokens of each turn;
                      "context" (default): reports only the context size;
                      "none": reports nothing
  FAKE_ACP_CANCEL=1   asks for permission to run a command during the connection
                      test and gives up its turn when refused, as Codex does;
                      it answers when it is asked again
  FAKE_ACP_AUTH=1     refuses to open a session without a login
  FAKE_ACP_FS=1       asks the client to read a file for it; the reply goes to the log

What it is started with and what it is asked goes to $AGORA_HOME/fake-acp.log.
"""
import json
import os
import sys

LOG = os.path.join(os.environ.get("AGORA_HOME", "."), "fake-acp.log")


def log(text):
    with open(LOG, "a") as f:
        f.write(text + "\n")


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)


def update(session, body):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": body}})


def say(session, text):
    # In two pieces, as a real agent streams.
    half = len(text) // 2
    for piece in (text[:half], text[half:]):
        update(session, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": piece}})


state = {"turns": 0, "said_apple": False, "tokens_in": 0, "tokens_out": 0, "next_id": 1000}


def ask_client(method, params):
    """Sends a request to the client and waits for its reply."""
    state["next_id"] += 1
    send({"jsonrpc": "2.0", "id": state["next_id"], "method": method, "params": params})
    while True:
        reply = read()
        if reply.get("id") == state["next_id"] and "method" not in reply:
            return reply


def answer_for(text):
    if "connection test" in text:
        state["said_apple"] = True
        return "ANSWER\napple"
    if "Which word did you reply with" in text:
        return "ANSWER\napple" if state["said_apple"] else "ANSWER\nI do not remember"
    if "Reply with only the word: " in text:
        return text.rsplit("Reply with only the word: ", 1)[1].strip()
    if "hands you a task" in text:
        return "DONE\nDid the task. cwd=" + os.path.basename(os.getcwd())
    if "asks you one question" in text:
        return "The deadline is Friday. cwd=" + os.path.basename(os.getcwd())
    if "The work phase is over" in text:
        return "WORKRESULT: all packages are reported."
    if "Work phase. Package #" in text:
        return "DONE\nDid the work. cwd=" + os.path.basename(os.getcwd())
    if "Write the briefing" in text:
        return "Goal: agree on a plan. Known: nothing yet."
    if "First line: KNOWN_MISSING" in text:
        return "KNOWN_MISSING\nI know the goal. I am missing the deadline. cwd=" + os.path.basename(os.getcwd())
    if "fill the gaps" in text:
        return "ANSWER\nThe deadline is Friday."
    if "reached consensus" in text:
        return "FINAL: the agreed plan."
    if "closes without consensus" in text:
        return "OVERVIEW: agreed nothing, disputed everything."
    if "Your turn" in text:
        state["turns"] += 1
        return "PROPOSAL\nDo it in two steps." if state["turns"] == 1 else "CONSENSUS\nAgreed."
    return "PASS"


def prompt(request):
    params = request["params"]
    session = params["sessionId"]
    text = "".join(block.get("text", "") for block in params["prompt"])
    log("prompt rules=%d chars=%d" % (text.count("one of several equal peers"), len(text)))
    if "was refused" in text and state.get("gave_up"):
        # Asked again after a refusal: answer what was asked before.
        text = state.pop("gave_up")
    elif os.environ.get("FAKE_ACP_CANCEL") == "1" and "connection test" in text:
        update(session, {"sessionUpdate": "tool_call", "toolCallId": "t9", "title": "write a file", "kind": "execute", "status": "pending"})
        reply = ask_client(
            "session/request_permission",
            {"sessionId": session, "toolCall": {"toolCallId": "t9", "kind": "execute"}, "options": [{"optionId": "yes", "kind": "allow_once"}, {"optionId": "cancel", "kind": "reject_once"}]},
        )
        if reply.get("result", {}).get("outcome", {}).get("optionId") != "yes":
            log("gave up its turn after a refusal")
            state["gave_up"] = text
            send({"jsonrpc": "2.0", "id": request["id"], "result": {"stopReason": "cancelled"}})
            return
    if os.environ.get("FAKE_ACP_TOOL") == "1" and ("Your turn" in text or "Work phase. Package #" in text):
        say(session, "Let me look at the files first. ")
        update(session, {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "run a command", "kind": "execute", "status": "pending"})
        reply = ask_client(
            "session/request_permission",
            {
                "sessionId": session,
                "toolCall": {"toolCallId": "t1", "kind": "execute", "title": "run a command"},
                "options": [
                    {"optionId": "allow", "name": "Yes", "kind": "allow_once"},
                    {"optionId": "reject", "name": "No", "kind": "reject_once"},
                ],
            },
        )
        log("permission " + json.dumps(reply.get("result", {}).get("outcome", {}), sort_keys=True))
        update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"})
    if os.environ.get("FAKE_ACP_FS") == "1":
        reply = ask_client("fs/read_text_file", {"sessionId": session, "path": "/etc/hosts"})
        log("fs " + json.dumps(reply.get("error", {}).get("code")))
    answer = answer_for(text)
    say(session, answer)
    result = {"stopReason": "end_turn"}
    usage = os.environ.get("FAKE_ACP_USAGE", "context")
    if usage == "total":
        state["tokens_in"] += 1000 + 500 * (state["tokens_in"] > 0)
        state["tokens_out"] += 20
        result["usage"] = {"totalTokens": state["tokens_in"] + state["tokens_out"], "inputTokens": state["tokens_in"], "outputTokens": state["tokens_out"]}
    elif usage == "turn":
        state["tokens_in"] = 1000 + 40 * (state["tokens_in"] > 0)
        result["usage"] = {"totalTokens": state["tokens_in"] + 5, "inputTokens": state["tokens_in"] - 300, "cachedReadTokens": 300, "outputTokens": 5}
    elif usage == "context":
        state["tokens_in"] += 700
        update(session, {"sessionUpdate": "usage_update", "used": state["tokens_in"], "size": 200000})
    send({"jsonrpc": "2.0", "id": request["id"], "result": result})


def main():
    log("start args=%s cwd=%s" % (" ".join(sys.argv[1:]), os.path.basename(os.getcwd())))
    while True:
        request = read()
        method = request.get("method")
        if method == "initialize":
            caps = request["params"].get("clientCapabilities", {})
            log("initialize version=%s fs=%s terminal=%s" % (request["params"].get("protocolVersion"), json.dumps(caps.get("fs", {}), sort_keys=True), json.dumps(caps.get("terminal"))))
            send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "fake-acp", "version": "1"}, "authMethods": []}})
        elif method == "session/new":
            if os.environ.get("FAKE_ACP_AUTH") == "1":
                send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32000, "message": "Authentication required"}})
                continue
            log("session/new cwd=%s mcp=%d" % (os.path.basename(request["params"]["cwd"]), len(request["params"]["mcpServers"])))
            options = [
                {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "fake-small", "options": [{"value": "fake-small", "name": "Small"}, {"value": "fake-large", "name": "Large"}]},
                {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "ask", "options": [{"value": "ask", "name": "Ask"}, {"value": "plan", "name": "Plan"}]},
            ]
            send({"jsonrpc": "2.0", "id": request["id"], "result": {"sessionId": "s1", "configOptions": options}})
        elif method == "session/set_config_option":
            log("set %s=%s" % (request["params"]["configId"], request["params"]["value"]))
            if request["params"]["value"] == "broken":
                send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32602, "message": "no such mode"}})
            else:
                send({"jsonrpc": "2.0", "id": request["id"], "result": {"configOptions": []}})
        elif method == "session/prompt":
            prompt(request)
        elif "id" in request and method:
            send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "method not found"}})


main()
