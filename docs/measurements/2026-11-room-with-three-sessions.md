# Measurement: one room, three sessions, against one agent alone

November 2026. One run of one task. This shows what a room costs and how fast it is; it is not a benchmark, and it does not measure whether the room's answer is better.

## Setup

| | |
|---|---|
| Peers | `Mac` (macOS 13, Intel) opens the room; `Heimserver` (Linux container on another machine) joins; `Mac-2`, a second daemon on the Mac, is invited while the room runs |
| Model | Claude Sonnet (`--model sonnet`), Claude Code 2.1.328 on the Mac and 2.1.322 on Linux |
| Sessions | started by the daemon without connectors, built-in tools on, permission mode `auto` |
| Account | all three sessions ran on one account |
| Link | direct, 11 to 21 ms round trip; both machines were in one local network, so NAT traversal was not exercised |
| Project | each session worked in its own copy of this repository |
| Command | `agora discuss "<task>" --with Heimserver --turns 4`, then `agora invite <room> Mac-2` once the log had five messages |

The task:

> agora-mesh rooms do not survive a restart of the daemon that opened them (ARCHITECTURE.md section 13, src/room.rs, src/daemon.rs). Design how a room can resume after such a restart: what the daemon has to persist and when, how the Claude sessions are continued, and how the peers find the room again. Look at the code before you propose anything. Decide among yourselves and do not ask the human. This room only plans: do not split the work and do not change any files. The result is a concrete design of at most 20 lines that names the files and functions to change.

The baseline is one Sonnet session that got the same task in the same repository copy, started the same way (`claude -p … --strict-mcp-config --permission-mode auto`).

## Result

| | One agent alone | Room |
|---|---|---|
| Sessions | 1 | 3, on 2 machines |
| Wall time | 34 s | 104 s |
| Input tokens, cache reads included | 175,323 | 699,586 |
| Output tokens | 4,086 | 12,087 |
| Tokens in total | 179,409 | 711,673 |
| Outcome | a design | consensus on a design after 10 messages |

The room took 3.1 times as long and 4.0 times the tokens. In the baseline 77 % of the input tokens were cache reads; the room's statistics do not split that out.

### The room, peer by peer

| Peer | Turns | Input tokens | Per turn | Output tokens | Model time |
|---|---|---|---|---|---|
| Mac (opener) | 5 | 505,740 | 101,148 | 8,548 | 73 s |
| Heimserver | 3 | 162,588 | 54,196 | 3,195 | 28 s |
| Mac-2 (joined late) | 1 | 31,258 | 31,258 | 344 | 3 s |

The three model times add up to the wall time: peers take turns, so a room is as slow as the sum of its turns. The opener carries the most, because it writes the briefing, fills the gaps, and writes the final result. A turn costs far more than the fixed context of a session (about 24,000 tokens) because the agents read source files during their turns.

### Message order

| # | Peer | Kind | |
|---|---|---|---|
| 1 | Mac | BRIEF | briefing |
| 2 | Heimserver | KNOWN_MISSING | |
| 3 | Mac | ANSWER | |
| 4 | Heimserver | ANSWER | discussion starts |
| 5 | Mac | PROPOSAL | |
| 6 | Mac-2 | JOIN | invited 86 s after the start |
| 7 | Heimserver | CONSENSUS | |
| 8 | Mac | TURN | tells Mac-2 it is next |
| 9 | Mac-2 | CONSENSUS | got a summary of messages 1 to 5, not the log |
| 10 | Mac | CONSENSUS | room closes |

The late peer agreed with the proposal and added nothing of its own. The design the room produced is in [docs/design/room-resume.md](../design/room-resume.md).

## One question

`agora ask "Which crates does this project depend on directly according to Cargo.toml, and which iroh version does Cargo.lock pin?" --with Heimserver` returned the correct answer in 4 s. The answering peer used 55,678 tokens; the asking machine started no session and used none.

## Fixed context per turn

`agora bench` on the Mac, input tokens of a turn that says one word:

| Session | Tokens |
|---|---|
| default headless session (7 connectors, 188 tools), tools off | 195,258 |
| without connectors, tools off | 3,743 |
| room session: no connectors, built-in tools | 23,994 |

The room above had 9 model turns. Had its sessions been started the default way, each turn would have read about 191,500 tokens more, which adds up to about 2.4 million tokens instead of 0.7 million. That figure is calculated, not measured.

## What this run found

The first attempt at this measurement ended after seven messages with "turn budget used up" although every peer had turns left, and the late peer never spoke. The speaking order was tracked by position, which went wrong when a peer was inserted. It now goes by the last speaker, and `tests/e2e-everyday.sh` checks the order after a late join. The numbers above are from the run after that fix.
