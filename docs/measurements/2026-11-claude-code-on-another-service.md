# What Claude Code sends to another model service

2026-11-20. Claude Code 2.1.329 on macOS, started by `agora` on a harness profile made with `agora harness add anthropic-compatible --url http://127.0.0.1:<port> --model mock-model --key env:MOCK_KEY`. The service was a local stand-in that speaks Anthropic's Messages API, answers with a fixed text and logs every request. No real model service was involved, so this shows what Claude Code does, not what a provider does with it.

The machine was logged in to Claude with a subscription, and `ANTHROPIC_API_KEY` was set to a marker value in the environment of the command, to see whether either reaches the service.

## Credentials

Every request carried `Authorization: Bearer <the profile's key>` and nothing else: no `x-api-key` header, and no other bearer token. agora-mesh clears `ANTHROPIC_API_KEY` for a session whose profile names another address, unless the profile sets it itself.

## Requests

| Session | Requests per turn | Tools offered | System prompt | Request body |
|---|---|---|---|---|
| rights `talk` | 1 | 0 | 8,590 characters | 16,869 bytes |
| rights `full` | 1, plus 1 per tool call | 12 | 8,756 characters | 56,684 bytes |

- One request per turn. There were no background requests for titles, summaries or a second model.
- The model name arrived as the profile gave it.
- The second turn of a session was 120 to 214 bytes larger than the first: the conversation is kept, as on Anthropic's service.
- The requests carried `anthropic-beta: claude-code-20250219,interleaved-thinking-2025-05-14,context-management-…`. A service that rejects these needs `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` in the profile's environment, according to Claude Code's documentation; not tried here.

The body sizes are bytes, not tokens. For orientation: on Anthropic's service the same two kinds of session read 3,743 and 23,994 input tokens per turn (README, "Measured").

## Permission mode `auto`

The stand-in answered one turn with a call of the `Bash` tool.

| Command | Permission mode | Result |
|---|---|---|
| `echo agora-tool-test` | `auto` | ran |
| `curl -s http://127.0.0.1:<port>/probe` | `auto` | refused: "Auto mode is unavailable: its safety check does not run for this session, so Bash was not run. Other actions that need the check will be denied too" |
| the same | `bypassPermissions` | ran; the stand-in received the request |

No request for the safety check reached the stand-in. So on another model service a session in `auto` mode can answer, read and run commands Claude Code considers harmless, and nothing that needs the check. A work package ends with a commit and a push, which it cannot do. Which other tools count as needing the check was not tried.

This is why a profile for another service has no permission mode unless the user gives one with `--permission-mode`, and why `agora harness add` says so.

## Not measured

Tokens, cost, speed and the quality of answers on any real service; whether a real model keeps to the room's reply format; any agent over the Agent Client Protocol (that adapter has only run against the scripted agent in `tests/fake-acp.py`).
