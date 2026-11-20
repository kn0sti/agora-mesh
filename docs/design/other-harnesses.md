# Design: peers on other harnesses and other models

Status: plan, nothing of it is built. Researched on 2026-11-20.

Today every peer runs Claude Code on a Claude model. This plan lets a peer run another coding agent (Codex, OpenCode, Gemini CLI, Qwen Code, Kimi Code, and others) or run on another model (DeepSeek, GLM, Kimi, Qwen, MiniMax, a local model), without giving up what makes a room cheap: sessions that start lean and stay alive.

"Harness" here means the program that runs the agent loop on a machine: Claude Code, Codex CLI, OpenCode. "Backend" means the model service that program talks to.

## 1. Where the facts come from

Three research passes read the vendors' documentation and repositories on 2026-11-20. Most pages were read through a summarising fetcher, so flag names and field names below are as reported, not as tested. Two things were checked on a real installation: the `codex exec` flags and the existence of `codex app-server` (Codex CLI 0.153.4, where the app server is marked experimental). Everything else must be checked against `--help` and a real run before it is relied on. Prices change; re-read them at the source.

## 2. What a room needs from a session

The room logic in `src/room.rs` uses a session through a small surface. This is the contract every harness adapter has to meet:

| Need | Today (Claude Code) | Required? |
|---|---|---|
| start in a directory, with the room rules and a rights level (`talk`, `read`, `work`) | flags `--append-system-prompt`, `--tools`, `--permission-mode` | yes |
| send one message, get the final answer text | one line on stdin, the `result` event | yes |
| stop a running turn and everything it started | kill the process group | yes |
| tokens in and out per turn | `result.usage` | no, see section 6 |
| the model the session runs on | `system/init` | no |
| how full the usage limit is, and when it resets | `rate_limit_event` | no |
| a recognisable "limit reached" error | `is_limit_error` | no |
| stays alive between turns | yes | no, but one-shot costs tokens, see section 7 |

The optional rows become **capabilities** a peer reports. The room adapts to what is missing instead of pretending.

## 3. What exists

### 3.1 One protocol covers most harnesses

The Agent Client Protocol (ACP, agentclientprotocol.com, started by Zed, now governed jointly with JetBrains) lets a client drive a coding agent as a subprocess: JSON-RPC 2.0, one message per line, over stdin and stdout. Version 1 is stable and actively maintained; version 2 is an alpha draft that the project itself says not to ship by default.

Harnesses that speak ACP themselves, with the command the ACP registry or the vendor documents:

| Harness | Command | Note |
|---|---|---|
| OpenCode | `opencode acp` | MIT, any provider, also local models |
| Gemini CLI | `gemini --acp` | API key or enterprise only, see section 8 |
| Qwen Code | `qwen --acp` | Apache-2.0 |
| Kimi Code | `kimi acp` | MIT |
| Goose | `goose acp` | Apache-2.0 |
| Cline | `cline --acp` | Apache-2.0 |
| GitHub Copilot CLI | `copilot --acp --stdio` | public preview |
| Cursor CLI | `agent acp` | hidden command |
| MiniMax Code | `mcode acp` | MIT |
| Kiro CLI, Mistral Vibe, Droid, Auggie, Junie and about thirty more | see the registry | |

Through an adapter package: Claude Code (`claude-agent-acp`) and Codex (`codex-acp`), both maintained in the ACP organisation. Not available: Aider, Crush.

The registry at `cdn.agentclientprotocol.com/registry/v1/latest/registry.json` lists launch commands in machine-readable form.

What ACP version 1 gives and does not give, measured against section 2:

- **Given:** sessions with many turns (`session/prompt` on one `sessionId`), cancel (`session/cancel`), choosing model and mode (`session/set_config_option`), permission requests the client answers (`session/request_permission`), no MCP servers unless the client passes some.
- **Not given, final answer:** there is no final-answer field. The client has to assemble the text from `agent_message_chunk` updates.
- **Not given, tokens per turn:** the stable part reports only the context size of the session (`usage_update`). Tokens per turn exist as an unstable field (`PromptResponse.usage`) that some agents already fill.
- **Not given, limits:** no field and no error code for rate limits or quota.

### 3.2 Harnesses with a better interface of their own

| Harness | Interface | Why it is worth an adapter of its own |
|---|---|---|
| Codex CLI | `codex app-server`, JSON-RPC over stdio: `thread/start`, `turn/start`, `turn/interrupt` | the most used harness; reports tokens per turn (`thread/tokenUsage/updated`) and is the only one besides Claude Code with a real quota signal (`account/rateLimits/read`: percent used, reset time) |
| Pi | `pi --mode rpc`, JSON lines: `prompt`, `abort`, `get_session_stats` | smallest start of all measured harnesses, about 1,900 tokens; reports tokens and cost per message |
| Amp, Antigravity CLI, Qwen Code | stream-JSON input and output modelled on Claude Code's | a variant of the existing adapter rather than a new one; Qwen's input mode is documented as under construction |

### 3.3 Other models without another harness

DeepSeek, Z.ai (GLM), Moonshot (Kimi), Alibaba (Qwen), MiniMax, Xiaomi (MiMo), OpenRouter, Ollama and LM Studio all offer an endpoint that speaks Anthropic's Messages API. Their own documentation tells Claude Code users to set environment variables, for example for DeepSeek:

```
ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic
ANTHROPIC_AUTH_TOKEN=<key>
ANTHROPIC_MODEL=deepseek-flash[1m]
ANTHROPIC_DEFAULT_OPUS_MODEL=deepseek-flash[1m]
ANTHROPIC_DEFAULT_SONNET_MODEL=deepseek-flash[1m]
ANTHROPIC_DEFAULT_HAIKU_MODEL=deepseek-flash
CLAUDE_CODE_SUBAGENT_MODEL=deepseek-flash
```

agora-mesh already starts `claude` with the daemon's environment. So a peer on a cheap model needs no new adapter, only a way to name such a set of variables and apply it to the sessions it starts. This is the shortest route to DeepSeek and the other Chinese providers.

Prices per million tokens as the providers listed them on 2026-11-20 (input / output):

| Provider | Model | Input | Output | Endpoint |
|---|---|---|---|---|
| DeepSeek | deepseek-flash | $0.30 | $1.20 | `https://api.deepseek.com/anthropic` |
| DeepSeek | deepseek-v4-pro | $1.32 | $3.96 | same |
| Z.ai | GLM-5.3-Flash | $0.15 | $0.50 | `https://api.z.ai/api/anthropic` |
| Z.ai | GLM-5.3 | $1.40 | $4.40 | same |
| MiniMax | MiniMax-M3.1-Flash | $0.15 | $0.60 | `https://api.minimax.io/anthropic` |
| Alibaba | qwen3.7-plus | $0.40 | $1.60 | per workspace and region, Frankfurt available |
| Xiaomi | mimo-v2.6-pro | $0.435 | $0.87 | `https://api.xiaomimimo.com/anthropic` |
| Moonshot | kimi-k3 | $3.00 | $15.00 | `https://api.moonshot.ai/anthropic` |
| Ollama, LM Studio | local | 0 | 0 | `http://localhost:11434`, `http://localhost:1234` |

DeepSeek halves its prices outside peak hours and bills cached input at $0.006. DeepSeek, Moonshot, Alibaba and MiniMax document that this endpoint returns `usage` with cache fields; for Z.ai no statement was found; Ollama returns input and output tokens only.

What is known to differ from Anthropic's own API: no provider documents the rate-limit headers Claude Code reads, so no `rate_limit_event` is to be expected. The cost Claude Code prints is its own estimate at Anthropic prices and will be wrong. Alibaba notes that the permission mode `auto` can fail on its plan endpoint, because the classifier behind it expects a Claude model. Web search and image input depend on the provider.

## 4. Design

### 4.1 Three kinds of adapter behind one interface

`Session` in `src/room.rs` becomes an interface with the surface of section 2, and the present code becomes its first implementation. No room logic moves.

1. **`claude`**: the present adapter. New: it applies a named set of environment variables and flags, which is all a backend like DeepSeek needs.
2. **`acp`**: a generic ACP client. One implementation, many harnesses.
3. **`codex`** and **`pi`**: native adapters where the harness offers more than ACP carries. They share the JSON-lines request and response plumbing with the ACP adapter.

A fourth kind, a command template that starts the program anew for every turn (`aider --message {text}`), covers what has no session interface at all. It comes last, because it is the most expensive per turn and the least reliable.

### 4.2 Profiles

A profile names a harness, how to start it, and its backend. Profiles live in `~/.agora/harness.toml`; the commands `agora harness add`, `list`, `test` and `default` edit it, so nobody has to write it by hand.

```toml
default = "claude"

[deepseek]
kind = "claude"
model = "deepseek-flash[1m]"
env = { ANTHROPIC_BASE_URL = "https://api.deepseek.com/anthropic", ANTHROPIC_AUTH_TOKEN = "env:DEEPSEEK_API_KEY" }
permission_mode = "acceptEdits"
external = true

[opencode]
kind = "acp"
command = ["opencode", "acp"]

[codex]
kind = "codex"
```

- **Presets.** The binary ships presets for the providers of section 3.3 and the harnesses of section 3.1 with every variable the provider's documentation lists. `agora harness add deepseek` asks only for where the key comes from.
- **Secrets.** A profile never holds a key. It names an environment variable (`env:NAME`) or a command that prints the key (`cmd:...`, for a system keychain). Profiles are local; nothing of them except name, kind and model is sent to peers.
- **`agora harness test <name>`** starts a session, sends one message, and prints what worked: answer received, tokens reported or not, limit signal or not, tokens of the first turn. This is how a profile's capabilities are found out rather than assumed.
- **Which profile a session uses.** Each machine decides for itself: its default profile, or `AGORA_HARNESS`, or `--harness` on the command that opens or joins. The opener cannot choose a peer's harness, because only the peer knows what is installed and logged in there.

### 4.3 What peers tell each other

The greeting between peers and the room invitation gain optional fields: harness name, kind, model, and the capability flags `tokens`, `capacity`, `persistent`. Old peers ignore them; new peers treat their absence as "Claude Code, everything supported". `agora peers` shows harness and model next to the name, and "unknown" where a harness reports no capacity.

`--model` on `agora discuss` is a name only one kind of harness understands. It keeps travelling in the invitation; a peer whose profile does not know the name keeps its own model and says so in the room's notes.

The room rules sent to every session say today that the other peers "are Claude Code agents". They will name each peer's harness and model instead, so that an agent knows whom it is talking to.

### 4.4 The ACP adapter in detail

Written by hand on the existing process and line handling: the client needs six methods, and the official Rust crate went through four major versions in five months and brings its own process and I/O layer. The schema crate can supply the types.

1. `initialize` with protocol version 1 and **no** file system or terminal capability, so the agent uses its own tools, as in every other harness.
2. `session/new` with the working directory and `mcpServers: []`.
3. Model and mode through `session/set_config_option`, read from the options the agent returned. The mode is chosen from the rights level: the agent's read-only or plan mode for `talk` and `read`, its auto-approving mode for `work`.
4. The room rules: as the client system prompt where the agent supports it (stable in ACP since 2026-11-06), otherwise as the first part of the first message. Either way they are sent once.
5. `session/prompt` per turn. The answer is the text of the `agent_message_chunk` updates **after the last tool call** of the turn, which matches what Claude Code's `result` carries. This rule needs testing against each harness.
6. `session/request_permission` is answered from the rights level: reject for `talk`, allow only reading kinds for `read`, allow for `work`. Chosen by the option's kind, never by its label.
7. Cancel: `session/cancel`, then the pending permission requests are answered as cancelled, then the process group is killed if the turn does not end within a few seconds. `agora panic` keeps its guarantee.
8. Tokens: the unstable per-turn field where present, otherwise the growth of `usage_update.used`, otherwise unknown.

### 4.5 Several participants on one machine

Once a session's profile is a property of the participant rather than of the machine, one machine can bring two participants into a room: `agora discuss "..." --with mac,mac:deepseek`. That gives a planning room of a strong and a cheap model on a single computer, which is the cheapest way to get a second opinion. It changes who counts as a participant (peer id plus profile), so it is its own step.

## 5. Order of work

| Step | What | Brings | Size |
|---|---|---|---|
| 1 | Session interface, capabilities, the optional fields between peers, neutral wording in the rules. No change in behaviour; all present tests stay as they are. | the base | small |
| 2 | Profiles for the `claude` adapter, presets, `agora harness add/list/test/default` | DeepSeek, GLM, Kimi, Qwen, MiniMax, MiMo, anything on OpenRouter, local models through Ollama and LM Studio | small |
| 3 | ACP adapter | OpenCode, Gemini CLI, Qwen Code, Kimi Code, Goose, Cline, Copilot CLI, Cursor CLI, MiniMax Code, and Codex through `codex-acp` | medium |
| 4 | Native Codex adapter | Codex with tokens per turn and a real limit signal, so handover on a full limit works there too | medium |
| 5 | Pi adapter, several participants per machine, command-template adapter | the leanest harness, mixed rooms on one machine, Aider and Crush | medium |

Steps 2 and 3 together cover every harness and provider named in this document except Aider and Crush. Each step ends with a measurement in `docs/measurements/` and with the README's support table saying what was actually run and what was only built.

**Tests.** The end-to-end tests drive a fake `claude`. Step 3 adds a fake ACP agent of the same kind (a script that speaks the six methods and can be told to ask for permission, to stream in pieces, to call a tool before answering, to hang). Step 2 extends the fake `claude` to record its environment, so a test can show that a profile's variables arrive and that a key never appears in the room log, the peer greeting or `stats.json`. Real runs are needed on top, per profile, with `agora harness test`.

## 6. When a harness reports less

- **No tokens.** The room budget counts turns only for that peer; `agora stats` shows an estimate from text length and marks it as an estimate. A budget in tokens still stops the room on the peers that do report.
- **No capacity.** `agora peers` shows "unknown". Work is not moved away from such a peer in advance.
- **No recognisable limit error.** Each adapter has its own patterns for "limit reached" (HTTP 429, quota wording). Where none matches, the error is treated like any other failed session: the peer leaves the room and its package is reassigned, as today.
- **Not persistent** (command-template adapter only). Every turn resends the room's history. The adapter sends the compact, numbered history the room already keeps, not the harness's own transcript.

## 7. Keeping it lean

The rule that made a Claude Code turn cost 24,000 instead of 195,000 tokens applies to every harness: start without connectors, with only the tools the rights level needs, without the user's instruction files, and keep the session alive so the provider's cache carries the prefix.

What a third-party benchmark measured for the first call of an empty headless session (testdino.com, tests of 8 October 2026; not checked here), and what the documentation offers to reduce it:

| Harness | First call | Switches |
|---|---|---|
| Pi | about 1,900 | `--no-tools`, `--tools <list>`, `--no-context-files`, `--no-skills`, `--no-extensions` |
| Aider | about 2,500 | `--map-tokens 0`; caching is off unless `--cache-prompts` |
| Cline | about 5,400 | `-s` replaces the system prompt, `--thinking none` |
| Gemini CLI | about 9,300 | `tools.core` allow list, `-e none`, `GEMINI_SYSTEM_MD` |
| Cursor CLI | about 13,200 | none documented |
| OpenCode (version 1) | about 13,400 | `OPENCODE_DISABLE_PROJECT_CONFIG=1`, deny rules remove tools |
| Qwen Code | about 13,700 | `--exclude-tools`, `--system-prompt` |
| Codex CLI | about 14,800 | `--ignore-user-config`, `-c web_search="disabled"`, `--disable <feature>`, `-c model_reasoning_effort=low` |
| Copilot CLI | about 17,100 | `--disable-builtin-mcps`, `--no-custom-instructions`, `--available-tools` |
| Kiro CLI | about 31,800 | none documented |

For comparison, measured in this project: a Claude Code session as agora-mesh starts it reads 23,994 tokens per turn with tools and 3,743 without.

Consequences for the design:

- Every preset carries the lean switches of its harness, and the rights level `talk` removes tools wherever the harness allows it.
- `agora bench --harness <name>` measures the real cost per turn of a profile on this machine. The README shows only numbers measured that way.
- Persistent sessions matter more on cheap backends than on Claude: DeepSeek bills a cached input token at a fiftieth of an uncached one. A harness driven one shot at a time gives that up.
- The room protocol itself (numbered messages, references instead of repetition, a briefing that sends only what is missing) is independent of the harness and stays as it is.

## 8. Terms and data

These are findings to show users, not legal advice.

- **Flat-rate coding plans of the model providers mostly exclude this use.** Alibaba's Coding Plan and Token Plan forbid "automated scripts" and "non-interactive" use; Volcengine's plan is for coding tools only; Z.ai's plan is for "officially supported tools" and forbids calling the API from one's own systems; MiniMax describes its plan as for "individual, interactive developer use". Keys billed by usage carry no such restriction. Presets therefore point at the usage-billed endpoints, and `agora harness add` says so when a plan endpoint is entered.
- **Anthropic documents the mechanism but not the use.** Its gateway documentation says any endpoint in a supported API format works, and that Anthropic "doesn't support routing Claude Code to non-Claude models through any gateway". Unsupported, not prohibited, as far as the pages read say.
- **Google.** Gemini CLI no longer serves free or Google AI Pro accounts (since 18 June 2026); it works with a paid API key or Vertex. The consumer successor, Antigravity CLI, documents headless use for scripting but says that wrapping it "inside third-party coding agents or applications" is not supported on consumer plans. A Gemini peer should use an API key.
- **OpenAI.** The pricing page lists `codex exec` and scriptable workflows as a plan feature; the documentation recommends an API key for automation. No prohibition was found; the consumer terms page could not be read.
- **Where prompts go.** DeepSeek processes in China; Z.ai and Moonshot name Singapore; Alibaba offers Frankfurt with an EU scope and says it never trains on the data; the consumer policies of DeepSeek, Z.ai and Moonshot list model training as a purpose. A room sends its messages, and whatever files its sessions read, to the backend of every peer in it.

Because of the last point, a profile whose backend is outside the user's control is marked `external = true`, and a project can refuse such peers: a file `.agora/no-external` in a repository makes the daemon decline to start a session of an external profile there, and to tell the room why. That is a guard against mistakes, not a security boundary; the peer on the other machine enforces its own.

## 9. Risks

- **ACP details differ per harness.** The answer-assembly rule, the names of modes, what a permission request looks like: each needs a real run. The capability test in `agora harness test` exists for this reason.
- **Moving targets.** OpenCode's version 2 broke its server API, Kimi CLI was replaced by Kimi Code, Gemini CLI changed its audience, all within 2026. Presets will age. They are data, not code, so they can be corrected in a release without touching an adapter.
- **Cheap models and the room format.** A room depends on the agent putting one of eight words on the first line. Weaker models may not keep to it. The parser already treats an unknown first line as a plain contribution; whether rooms still converge needs measuring, and may need a one-line reminder per turn for some profiles.
- **Permissions.** Each harness has its own idea of sandbox and approval. The rights levels `talk` and `read` are only as strong as the mode the harness offers; where a harness has no read-only mode, the level cannot be promised and the profile must say so.
- **Support load.** Every profile is one more thing that can break without this project changing. The support table lists per profile: built, run once, run regularly.

## 10. Open questions

1. TOML for the profile file adds a dependency; JSON would not. TOML is easier to read and to correct by hand.
2. Should a room's opener be able to require capabilities, for example "only peers that report tokens"?
3. Whether presets should be fetched from the repository at run time, so that a changed endpoint does not need a new release. It would add a network request and a trust question.
