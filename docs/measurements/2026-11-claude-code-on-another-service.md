# Claude Code on another model service

2026-11-20. Claude Code 2.1.329 on macOS, started by `agora` on a harness profile. Three parts: what Claude Code sends, looked at with a local stand-in for a model service; real rooms on four other models; and rooms with a work phase, which changed how a room works.

## 1. What Claude Code sends

The service was a local stand-in that speaks Anthropic's Messages API, answers with a fixed text and keeps every request. The machine was logged in to Claude with a subscription, `ANTHROPIC_API_KEY` was set to a marker value in the environment, and a `CLAUDE.md` with a marker lay in the directory above the session's.

### Started the usual way

This is how agora-mesh started such a session at first: like a session on Anthropic's service.

| | Rights `talk` | Rights `full` |
|---|---|---|
| Request body | 17,259 bytes | 57,118 bytes |
| Tools offered | 0 | 12 (`Agent`, `Bash`, `Edit`, `Read`, `Write`, `Skill`, `Workflow` and others) |
| System prompt | 8,591 characters | 8,756 characters |

Every request carried, besides the prompt:

- the `CLAUDE.md` from the directory above the session's, which is how personal notes above a project reach a service that was never meant to see them,
- the section about the memory, with the path of the home directory,
- a block `safeguards` of about 6,900 bytes: the working directory, the home directory, the paths of the settings files, some 85 permission rules, and the state of the repository. Claude Code sends it in the permission mode `auto`, which was this machine's default.

### Started bare

Now a session on another service is started with `--bare` and never in the mode `auto`:

| | Rights `talk` | Rights `full` |
|---|---|---|
| Request body | 2,189 bytes | 5,539 bytes |
| Tools offered | 0 | 3 (`Bash`, `Edit`, `Read`) |
| System prompt | 1,464 characters, the rules of the room | the same |

None of the three items above is sent any more. What remains besides the rules and the conversation is `metadata.user_id`: an identifier of the installation (64 characters) and of the session. The account identifier in it is empty.

The price: a bare session reads no `CLAUDE.md` at all, also not the project's, and has no search tools. With the rights `read` it has the tool `Read` only.

### Credentials, and requests per turn

Every request carried `Authorization: Bearer <the profile's key>` and nothing else: no `x-api-key`, no token of the subscription. One request per turn, plus one per tool call; no background requests. The model name arrived as the profile gave it.

### Permission modes

The stand-in answered a turn with a call of the `Bash` tool.

| Command | Permission mode | Result |
|---|---|---|
| `echo agora-tool-test` | `auto` or `default` | ran |
| `curl -s http://127.0.0.1:<port>/probe` | `auto` | refused: "Auto mode is unavailable: its safety check does not run for this session, so Bash was not run." |
| the same | `bypassPermissions` | ran |
| `agora memory`, `agora remember …` | `default`, with the two commands allowed by agora-mesh | ran |
| `agora peers` | `default` | refused: "This command requires approval" |

So `auto` buys nothing on another service and costs the `safeguards` block with every request. A session there runs in `default`: it can answer, read, run read-only commands and use the shared memory. It cannot change files, so it cannot finish a work package, unless the profile was added with `--permission-mode bypassPermissions`.

## 2. Real runs on other models

Through a reseller's Anthropic-compatible address, with a profile made by `agora harness add anthropic-compatible`. The key stayed on another machine; the profile held the command that fetches it.

### One session each

`agora harness test`, two turns:

| Model | Answers | Keeps to the room's format | Remembers the turn before | Input tokens, first and second turn | Time |
|---|---|---|---|---|---|
| deepseek-v4.1-flash | yes | yes | yes | 396, 428 | 14.3 s |
| glm-5.3-flash | yes | yes | yes | 344, 408 | 10.8 s |
| qwen3.8-flash | yes | yes | yes | 736, 505 | 22.4 s |
| minimax-m3.1-flash | yes | yes | yes | 528, 587 | 6.8 s |

No model reported a usage limit; none is expected to.

### Two rooms

Two daemons on one machine, each on another model, `--turns 3`, no project.

| | Room 1 | Room 2 |
|---|---|---|
| Models | deepseek-v4.1-flash and glm-5.3-flash | qwen3.8-flash and minimax-m3.1-flash |
| Rights | full, permission mode `default` | `--lean`: no tools |
| Messages | 8 | 9 |
| Course | brief, known and missing, answer, proposal, critique, proposal, consensus, consensus | brief, known and missing, answer, four proposals, consensus, consensus |
| Result | consensus | consensus |
| Tokens, both peers | 78,379 | 21,515 |
| Input tokens per turn, opener | 6,798 | 1,665 |
| Time | 457 s | 240 s |

Both rooms ended with a usable result: a five-step plan for adding a `--json` option to a command without breaking scripts, and a six-point checklist for moving a web service with under five minutes of downtime. Every message carried a valid kind on its first line.

For comparison, measured earlier on Claude: a planning room of two sessions on Claude Fable took 49 seconds and 248,909 tokens. The rooms here used a third and a tenth of the tokens, on models that cost a fraction per token, and took five to nine times as long. In room 2 one model answered in 23 seconds of model time in total and the other in 216.

Room 1 was run before the two memory commands were allowed: its sessions tried `agora remember`, were refused, and said so in the result.

The reseller's balance went down by less than one US dollar over all of this; it does not show smaller steps.

## 3. Rooms with a work phase

Two daemons, each in its own clone of a throwaway repository with a bare origin, each on another model, the profiles added with `--permission-mode bypassPermissions`. The task: one peer writes a small shell script, the other a page of documentation for it; split the work and do it.

### What went wrong first

| Run | Models | What happened |
|---|---|---|
| 1 | deepseek-v4.1-flash, glm-5.3-flash | The peers agreed on a split and then did not wait for the work phase. Each did its package inside its next discussion turn, committed on `main` and pushed to the origin's `main`; one of them then edited the other's file. The result was correct. The room closed as "consensus" after 735 s, with no work phase and no branches. |
| 2 | the same, after the rules said "do not change files, commit or push until a message starts the work phase" | The same, except that each pushed a branch of its own choosing instead of `main`. The rule was not kept. 1,280 s, "turn budget used up". |
| 3 | deepseek-v4.1-flash twice, `--lean` | No tools in the discussion, so nothing could be changed early. But after one peer proposed the split and the other answered CONSENSUS, the proposer made a new proposal instead of saying CONSENSUS too, and the turns ran out. One session wrote a tool call as plain text into its contribution. |

Two rules of a room did not survive these models: that a session with tools waits for the work phase, and that the author of a proposal confirms it with a CONSENSUS of its own.

### What was changed

- **A proposal is its author's agreement.** A room has reached consensus when every *other* peer answered CONSENSUS to the proposal that stands. This also ends every room one message earlier.
- **A harness that nothing checks only reads while the room discusses.** Claude Code on Anthropic's service checks actions in its `auto` mode. On another service, and for agents over the Agent Client Protocol, the discussion session gets reading rights, and the work is done by a second session with tools that starts with the work phase and is told the agreed plan, as in a lean room.
- **A session without tools is told so**, instead of being told about commands it cannot run.

### After the change

| | Run 4 | Run 5 |
|---|---|---|
| Models | deepseek-v4.1-flash twice | deepseek-v4.1-flash and glm-5.3-flash |
| Messages | 8: brief, known and missing, answer, split, consensus, work, done, done | 9: the same with one more answer |
| Branches in the origin | `agora/<room>/mac` with `bin/greet.sh`, `agora/<room>/server` with `docs/USAGE.md`; `main` untouched | the same |
| The script | prints `Hello, world!` and `Hello, Ada!` | the same |
| Tokens, both peers | 75,152 | 84,554 |
| Time | 351 s | 619 s |

Both rooms closed with "work finished" and a result for the human that names the two branches.

One more thing from run 3's first attempt: the reseller answered one model's requests with a sentence saying that the key may not use that model, sent as an ordinary answer and not as an error. Neither Claude Code nor agora-mesh can tell such a sentence from a contribution; it went into the room three times before the other model asked for a human.

## Not measured

- The services of the providers themselves. The reseller may answer differently than DeepSeek, Z.ai, Alibaba or MiniMax do directly, and the presets for those were not run.
- The larger models of these providers, and how good the results are compared with Claude's. Two rooms on small topics say that the format holds, not that the plans are as good.
- A task larger than two small files, and a package that needs a review.
- Agents over the Agent Client Protocol are in [2026-11-real-agents.md](2026-11-real-agents.md).
