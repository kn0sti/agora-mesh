# Other agents and other models

A peer does not have to run Claude Code on a Claude model. Each machine chooses a **harness profile** for its own sessions; a room does not care which one a peer is on.

| | Example | What runs |
|---|---|---|
| Claude Code on another model service | `agora harness add deepseek --key file:~/.keys/deepseek` | Claude Code, pointed at the service's Anthropic-compatible address. Presets: `deepseek`, `glm`, `kimi`, `minimax`, `qwen`, `openrouter`, `ollama`, `lmstudio`, and `anthropic-compatible` for any other. |
| Another coding agent | `agora harness add opencode` | An agent that speaks the [Agent Client Protocol](https://agentclientprotocol.com). Presets: `opencode`, `gemini`, `qwen-code`, `kimi-code`, `goose`, `cline`, `copilot`, `cursor`, `minimax-code`, `kiro`, `vibe`, `codex-acp`, and `acp` for any other. |

```
agora harness presets            # what `add` knows
agora harness test deepseek      # two real turns: does it answer, keep to the room's format, report tokens?
agora harness default deepseek   # this machine's sessions now run on it
agora bench --harness deepseek   # what one turn costs there
agora peers                      # shows which harness each peer runs on
```

Of the two ways, Claude Code on another model is the lean one: it starts bare and reads a few hundred tokens per turn, where an agent brings its own system prompt and tools.

![Bar chart of the size of one request that Claude Code sends to another model service. Started the usual way: 17,259 bytes without tools and 57,118 with tools. Started bare, as agora-mesh does: 2,189 and 5,539 bytes. The usual way also sent a CLAUDE.md from above the project, the path of the home directory, some 85 permission rules, the state of the repository and nine of twelve tools.](img/what-leaves-light.png)

Read this before you rely on it:

- **Presets are from documentation.** The addresses and commands in the presets are what each vendor's documentation said on 2026-11-20. What ran for real is in [the table above](other-agents.md#models-and-agents-that-ran); everything on other models went through a reseller, not through the providers' own addresses. `agora harness test` is there so that you find out on your machine.
- **Keys.** A profile names where a key comes from (`env:NAME`, `file:PATH` or `cmd:COMMAND`, for example a keychain), never the key. A key for Anthropic in the environment is not passed on to another service.
- **What a session on another service sends.** Claude Code is started bare there: without `CLAUDE.md` files, memory, hooks and plugins, and never in the `auto` permission mode. Started the usual way it sent a `CLAUDE.md` from above the project, the path of the home directory, the user's permission rules and the state of the repository with every request. What still goes out besides the conversation is an identifier of the installation. The price: such a session reads no `CLAUDE.md` at all, also not the project's, and has the tools `Bash`, `Edit` and `Read` only.
- **Working needs a decision.** A session on another service can discuss, read, run read-only commands and use the shared memory. It cannot change files, so it cannot finish a work package. `--permission-mode bypassPermissions` on `agora harness add` lets it work, and then every command that model writes runs unchecked.
- **Why such a peer only reads while a room discusses.** Given tools and no check, two models did their packages during the discussion and pushed to `main`, also after the rules of the room told them not to. So every harness except Claude Code on Anthropic's service discusses with reading rights and does its package in a second session that starts with the work phase. After that change the same models ended with one branch per peer and `main` untouched.
- **Rights of an agent.** The rights of a room (everything, reading only, nothing) reach an agent in two ways: its requests for permission are answered from them, and for OpenCode, Qwen Code and Codex the preset puts the agent into its own read-only mode. OpenCode used its tools without asking in a real run, and Codex tried to start its package during the discussion and was stopped by its read-only mode. For an agent without such a preset the rights are only as strict as the agent asks.
- **Tokens.** An agent that does not report its tokens is counted by its context size or by text length, and `agora stats` says so. No harness but Claude Code on Anthropic's service reports how full a usage limit is.
- **Where your prompts go.** A profile on an outside service is marked external, and everything its sessions read is sent there. A file `.agora/no-external` in a project keeps such sessions out of it on that machine. What is said in a room reaches the service of every peer in it; `agora peers` shows who is external.
- **Errors that look like answers.** A reseller answered one model's requests with a sentence saying that the key may not use that model, sent as an ordinary answer. Nothing can tell such a sentence from a contribution; it went into the room until the other model asked for a human.
- **Terms of the providers.** Flat-rate coding plans of most providers exclude automated use; keys billed by usage do not. Anthropic documents pointing Claude Code at another address and says it does not support other models there.

The plan behind this, with what each agent and provider offers: [design/other-harnesses.md](design/other-harnesses.md).

## Models and agents that ran

| Runs in | Model | A session | A room | A work phase |
|---|---|---|---|---|
| Claude Code | Claude Sonnet, Claude Fable | yes | yes | yes |
| Claude Code, pointed at another model | deepseek-v4.1-flash | yes | yes | yes |
| | glm-5.3-flash | yes | yes | yes |
| | qwen3.8-flash | yes | yes, without tools | no |
| | minimax-m3.1-flash | yes | yes, without tools | no |
| OpenCode 1.19.6 | deepseek-v4.1-flash, glm-5.3-flash | yes | yes, with Claude Code | yes, with Claude Code |
| Qwen Code 0.28.0 | qwen3.8-flash | yes | no | no |
| Codex, through its adapter 2.8.0 | gpt-6-luna | yes | yes, with Claude Code | yes, with Claude Code |
| Hermes Agent (git 15806b3) | deepseek-v4.1-flash | yes | yes, with Claude Code, its tools turned off | no |

Claude Code on another model, OpenCode and Codex have also shared one room and built one package each, on three branches ([details](measurements/2026-11-real-agents.md)).

"Yes" means it ran on a real machine and ended with a usable result. The Claude rows ran between a Mac and a Linux machine; the others between two daemons on one Mac, with the models reached through a reseller. How to set a peer up for another model or agent is under [other agents and other models](other-agents.md).

## Hermes Agent as a peer

[Hermes Agent](https://github.com/NousResearch/hermes-agent) speaks the Agent Client Protocol with `hermes acp`, so it joins through the `acp` preset:

```
agora harness add acp --name hermes --command "hermes acp"
agora harness test hermes
agora harness default hermes
```

Hermes reads its model and key from its own configuration. **Turn its tools off unless you mean to give them to the room.** There is no read-only preset for Hermes, and it only asks for permission for commands it considers dangerous, so the rights of a room do not hold it back. This is the configuration of the tested run, in `~/.hermes/config.yaml`:

```yaml
platform_toolsets:
  acp: []
agent:
  disabled_toolsets: [terminal, code_execution, file, web, browser, delegation, memory, skills, session_search, todo, vision]
```

Tested on 2026-11-21 with Hermes at git `15806b3`: `agora harness test` passed (answers, keeps to the room format, remembers the last turn), and a room with Claude Code reached consensus on a design question in 8 messages, 135 s and 24,439 tokens, both on `deepseek-v4.1-flash` through a reseller. A work phase with Hermes has not been run.

[Back to the README](../README.md)
