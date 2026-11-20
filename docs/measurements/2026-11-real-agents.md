# Real agents over the Agent Client Protocol

2026-11-20, macOS. The adapter for the Agent Client Protocol had only met a scripted agent. Here it drives three real ones: OpenCode, Qwen Code and Codex. `AGORA_ACP_TRACE=<file>` wrote down every message; the shapes below are read from those traces.

OpenCode and Qwen Code ran on models of a reseller (OpenAI-compatible address), Codex on a ChatGPT login.

## One session each

`agora harness test`, two turns, rights `talk`:

| | OpenCode 1.19.6 | Qwen Code 0.28.0 | Codex, adapter 2.8.0 |
|---|---|---|---|
| Started with | `npx -y opencode-ai acp` | `npx -y @qwen-code/qwen-code --acp` | `npx -y @agentclientprotocol/codex-acp` |
| Model | deepseek-v4.1-flash | qwen3.8-flash | gpt-6-luna |
| Answers, keeps to the room's format, remembers the turn before | yes, yes, yes | yes, yes, yes | yes, yes, yes |
| Input tokens, first and second turn | 11,543 and 11,577 | 15,074 and 15,119 | 17,845 and 17,916 |
| Time for both turns | 24.6 s | 30.0 s | 7.1 s |

For comparison: a bare Claude Code session on another service read 350 to 750 input tokens in such a turn. An agent brings its own system prompt and tools, and nothing was done here to make it smaller.

## What the agents send

All three speak protocol version 1 and offer a model and a mode as config options, as the adapter expects. They differ in the rest:

| | OpenCode | Qwen Code | Codex |
|---|---|---|---|
| Tokens at the end of a turn (`usage`) | yes, the tokens of the turn | no | yes, the tokens of the turn |
| Context size (`usage_update`) | no | yes | yes |
| Cached input | apart, as `cachedReadTokens` | | apart, as `cachedReadTokens` |
| Modes | `build`, `plan` | `plan`, `default`, `auto-edit`, `auto`, `yolo` | `read-only`, `workspace-write`, `agent`, `agent-full-access` |
| Mode it starts in | `build` | `auto` | `agent` |
| Asked for permission before using a tool | no (in `build`) | not seen | yes, in `read-only`: allow once, allow always, or refuse |
| Usage limit | no | no | no; `_meta.quota` holds token counts only |

Two things in the adapter came out of this:

- **Tokens.** The protocol's draft describes `usage` as a total of the session, and the adapter read it so. OpenCode and Codex report the tokens of the turn; the first run showed 10,275 tokens for the first turn and 45 for the second. The adapter now tells the two ways of counting apart on the second report: a total grows by about a whole context, the tokens of a turn stay close to those of the turn before.
- **Modes.** OpenCode used its tools without asking, so answering permission requests does not restrain it. The presets for these three agents now set the agent's own read-only mode for the rights `talk` and `read` (`plan`, `plan`, `read-only`); each accepted it in a real run. For an agent without such a preset the rights are only as strict as the agent asks.

## A room of two harnesses

Two daemons on one machine. One ran Claude Code, bare, on deepseek-v4.1-flash; the other ran OpenCode on glm-5.3-flash. `--turns 3`, no project, full rights.

| | |
|---|---|
| Messages | 8: brief, known and missing, answer, proposal, critique, answer, consensus, consensus |
| Result | consensus: a reply to a bug reporter in six sentences and what the maintainers check first |
| Tokens | 83,413 in total: 29,754 on the Claude Code side, 53,659 on the OpenCode side |
| Time | 156 s |
| Tools | OpenCode read a file and ran two commands; the Claude Code session saved the result with `agora remember` |

## A work phase of two harnesses

Rooms with a work phase on other models, and what they changed in a room, are in [2026-11-claude-code-on-another-service.md](2026-11-claude-code-on-another-service.md), part 3. Since then an agent, too, only reads while a room discusses and works in a second session. This run used that: the same task as there (a small shell script and a page of documentation for it), each peer in its own clone of a throwaway repository, Claude Code (bare, `bypassPermissions`) on one side and OpenCode on the other, both on deepseek-v4.1-flash.

| | |
|---|---|
| Messages | 9: brief, known and missing, answer, proposal, split, consensus, work, done, done |
| OpenCode's sessions | two: one in its mode `plan` for the discussion, in which it ran five commands and changed nothing; one in its usual mode for the work, in which it wrote the file, committed and pushed |
| Branches in the origin | `agora/<room>/mac` with `docs/USAGE.md` from Claude Code, `agora/<room>/server` with `bin/greet.sh` from OpenCode; `main` untouched |
| The script | prints `Hello, world!` and `Hello, Ada!` |
| Tokens | 88,874 in total |
| Time | 422 s |

## A work phase with Codex

The same task and set-up, with Codex (through its adapter, on a ChatGPT login) in place of OpenCode, and Claude Code (bare, `bypassPermissions`) on deepseek-v4.1-flash. It took four attempts, and three of them found something.

| Attempt | What happened | What changed |
|---|---|---|
| 1 | Codex had nothing to add on its first turn and sent PASS. The other peer then proposed the split, and the room closed at once with "nothing left to add": the pass from before the proposal was counted as an answer to it. | A new proposal makes every earlier answer stale. This was a fault in version 0.2.0. With three peers it could close a room as agreed although one of them had only agreed to an earlier proposal. |
| 2 | Codex proposed how the script should behave, the other peer agreed, and the room closed with a plan and no work. | The rules of a room now say that a room asked to do work needs an agreed split: a plan one accepts is answered with a split that takes it in. |
| 3 | After the split, Codex tried to write the file, commit and push inside the discussion. Its read-only mode made it ask first, the request was refused, and Codex gave up its turn. The adapter took that for a failure and the peer left the room. | An agent that gives up its turn after a refusal is told what happened and asked for its contribution, once. |
| 4 | Brief, known and missing, answer, pass, split, consensus, work, done, done. Codex again tried to start early, was refused, was asked again and answered with consensus. | |

The fourth attempt:

| | |
|---|---|
| Branches in the origin | `agora/<room>/mac` with `docs/USAGE.md` from Claude Code, `agora/<room>/server` with `bin/greet.sh` from Codex; `main` untouched |
| The script | prints `Hello, world!` and `Hello, Ada!` |
| Tokens | 278,760 in total, 245,924 of them on the Codex side: its working session read its whole context again with every command, most of it from the cache |
| Time | 190 s |

This is the third kind of agent that started the work before the room had agreed on it, after two models on Claude Code. What stopped it was not the rule in the text but the session's rights.

## A room of three harnesses

Three daemons on one Mac, each with its own clone of one repository: Claude Code (bare, `bypassPermissions`) on deepseek-v4.1-flash as the opener, OpenCode on the same model, and Codex through its adapter on gpt-6-luna. The task names three things that do not depend on each other: a script, a file on how to call it, and a test for the script. Script: `real-three.sh`.

| | First run, version 0.3.0 | Second run, version 0.4.0 |
|---|---|---|
| Until the work started | 73 s, 7 messages | 82 s, 9 messages |
| What the room said at the end | "work finished" | "work finished" |
| Branches in the origin | two of three: the script from OpenCode, the usage file from Codex | three: the script from OpenCode, the usage file from Codex, the test from Claude Code |
| Put together | the script works; there is no test | the script works, and the test one harness wrote passes against the script another wrote |
| `main` | untouched | untouched |
| Time, tokens of all peers | 471 s, 351,958 | 611 s, 350,153 |

**What the first run found.** The session on Claude Code answered its package with one line of text and a tool call written out as text (`Bash<arg_key>command</arg_key>…`). It ran nothing and committed nothing. A reply to a package that named no kind counted as `DONE`, so the room closed with "work finished", and the result told the human to merge a branch that did not exist. Two things changed in version 0.4.0:

- A reply that starts with neither `DONE` nor `BLOCKED` is no report. The session is asked once more; after that the package is blocked.
- To every `DONE` the daemon of the reporting machine adds what git says there about the package's branch.

**The second run.** Every session reported, so asking again was not needed; that path is covered by the scripted tests only. The three lines from git read `Checked on Mac: agora/<room>/mac is at deb2f51 … and pushed`, and the same for the other two, and they were true: the origin held the three branches. Two peers went further than asked: the one with the test fetched the other's branch and ran its test against that script, and the one with the script fetched the test and ran it.

The result for the human still got one thing wrong. It said that the clones had no shared remote and that each branch existed in its owner's clone only, which a peer had assumed during the discussion. Three lines from git said otherwise. The session that writes the result is now told that those lines come from git and that it should go by them where a report or an earlier message disagrees.

**A third run** with that change ended the same way as the second, three branches and a test that passes, in 325 s with 404,312 tokens, and its result reads "All three branches are pushed to `origin`; all daemon lines agree with the reports." One run does not show that the sentence is what made the difference.

A room like this is slow for what it produces: three one-file packages took five to ten minutes and 350,000 to 400,000 tokens, most of them in the working sessions of the two agents that bring their own context. It shows that three harnesses can share a room, not that they should for a task of this size.

## Not measured

- Other agents the presets name: Gemini CLI, Kimi Code, Goose, Cline, Copilot CLI, Cursor CLI, MiniMax Code, Kiro, Mistral Vibe. Their commands come from documentation.
- A work phase done by Qwen Code, and a task larger than two small files.
- A handover between real sessions: a real usage limit cannot be produced on demand, so the note for the taker has only met scripted sessions.
- Stopping a real agent in the middle of a turn.
