# agora-mesh

A peer-to-peer mesh for Claude Code. It connects Claude Code on two or more machines so that the agents on them work on one large task together: they plan it, question each other's ideas, split it, and build their parts at the same time, each on its own machine and its own branch.

No machine is the master. It is independent of the Claude account in use, needs no VPN and no cloud account, and does not use MCP.

> **Status: early development.** Everything described here works between a Mac and a Linux machine, and the repository builds from source. No release has been published yet, so the one-line installation as a plugin does not work yet. [ARCHITECTURE.md](ARCHITECTURE.md) describes what is built and lists what is not.

*agora-mesh is an independent community project. It is not affiliated with or endorsed by Anthropic.*

## What it is for

- **Large tasks, split and done in parallel.** Agents agree on a plan and on the interfaces between the parts, then each builds its part on its own branch `agora/<room>/<peer>`.
- **Agents that check each other.** In a room, peers propose, criticise and ask until they agree, or until they tell you what you have to decide.
- **Knowledge that sits on different machines.** Each peer has its own files, its own `CLAUDE.md` and memory, and its own access. A question goes to the machine that can answer it.
- **Capacity from more than one account.** Each peer runs its own `claude` with its own usage limit and context window. When a peer reaches its limit, another takes over its work.
- **A shared memory.** What one agent marks as important is on the other machines a second later, and sessions that are running are told.

## Measured

**Two agents build a stage of this project.** The task was the plugin wrapper and the release workflows of this repository, given as one paragraph to two Claude Sonnet agents on two machines. [Details and method](docs/measurements/2026-11-split-work.md).

| Seconds | |
|---|---|
| 0 to 49 | briefing, discussion, agreement on the interface between the parts, the split |
| 49 to 100 | both work at the same time, each in its own clone on its own branch |
| 108 | summary; two pushed branches, 11 files, no overlap, merged without a conflict |

The room used 889,880 tokens. One agent given the same task alone took 1,163 s and 1,859,692 tokens, but that is not a like-for-like speed-up: it ran far more checks on its own work, and the room's result needed four corrections in review. A room is fast at planning and splitting; its result still needs a review.

**A small task is cheaper alone.** For a design question, a room with three sessions took 104 s and 711,673 tokens where one agent took 34 s and 179,409 ([details](docs/measurements/2026-11-room-with-three-sessions.md)). For a plain question, `agora ask` got a correct answer from the other machine in 4 s with no session on the asking side.

**What a session costs before anyone says anything.** Agents use a small command instead of MCP tools, and the sessions agora-mesh starts carry no connectors. Input tokens of a turn that says one word, on the test machine (`agora bench` measures this on yours):

| Session | Tokens |
|---|---|
| default headless session (7 connectors, 188 tools) | 195,258 |
| session started by agora-mesh, with the built-in tools | 23,994 |
| the same without tools (`agora discuss --lean`) | 3,743 |

This compares headless sessions. An interactive session loads connector tools on demand and is not comparable.

## How it works

```
        Machine A                                   Machine B
 ┌──────────────────────┐                   ┌──────────────────────┐
 │ your Claude session  │                   │  (no session needed) │
 │   $ agora discuss …  │                   │                      │
 └──────────┬───────────┘                   └──────────────────────┘
            │ local socket
 ┌──────────▼───────────┐   encrypted QUIC  ┌──────────────────────┐
 │    agora daemon      │◄─────────────────►│    agora daemon      │
 │  identity · rooms ·  │  direct, or via   │  identity · rooms ·  │
 │  shared memory       │  relay fallback   │  shared memory       │
 └──────────┬───────────┘                   └──────────┬───────────┘
            │ stdin / stdout                           │ stdin / stdout
 ┌──────────▼───────────┐                   ┌──────────▼───────────┐
 │ room session         │   room messages   │ room session         │
 │ (fresh `claude -p`)  │◄ ─ ─ ─ ─ ─ ─ ─ ─ ►│ (fresh `claude -p`)  │
 └──────────────────────┘                   └──────────────────────┘
```

1. Each machine runs a small background daemon with its own key pair.
2. One machine opens a network; others join with the network ID. A human approves every join.
3. Any peer opens a room for a topic. Each invited daemon starts a fresh Claude session in its own clone of the project.
4. The opener briefs the others; then all take equal turns under a budget until they agree, split the work, or ask a human.
5. The result goes back to the session that asked, and into the shared memory.

## Security: read this first

**A connected peer can make your machine run commands.** Room sessions are started with full rights by default, because agents that cannot read or change anything cannot work together. This applies to the person on the other machine and to anything that person's agent has read, such as a prepared web page. In a test run a session looked into the user's `~/.ssh/config` on its own, to answer a question about a host.

Connect only machines and people you trust with a shell on your computer. Connections are permanent until you revoke the peer.

| | |
|---|---|
| `agora trust <name> --rights read` | this peer's sessions here may only read |
| `agora trust <name> --rights talk` | this peer's sessions here get no tools |
| `agora revoke <name>` | remove a peer |
| `agora panic` | stop every room and its session on this machine |

## Install

Once the first release is published, tell your Claude Code

> Install https://github.com/kn0sti/agora-mesh and help me set it up.

or install the plugin yourself with `/plugin install agora-mesh --marketplace kn0sti/agora-mesh`; [INSTALL.md](INSTALL.md) has the steps. Until then, build from source with a stable Rust toolchain:

```
cargo build --release          # binary: target/release/agora
```

Connect two machines:

```
agora setup --name MacBook     # once per machine
agora open                     # machine A: prints a network ID
agora join <network-id>        # machine B
agora status                   # both: compare the four words
agora approve <name>           # machine A
```

## Use

Use the smallest command that does the job. The first four start no session on your machine; only the peer spends tokens.

| Command | |
|---|---|
| `agora peers` | who is reachable, what they are working on, how much of their usage limit is used |
| `agora ask "<question>" --with <peer>` | one question to one peer |
| `agora delegate "<task>" --with <peer>` | hand a task to a peer; it reports done or blocked |
| `agora review --with <peer>` | a peer reviews your changes; the diff travels with the request, nothing has to be pushed |
| `agora discuss "<topic>"` | a room: the peers discuss, and split and do the work if they agree on a split |

Inside a git repository these commands are about that project, and every peer works in its own clone (`agora project add` registers one).

While a room runs:

| Command | |
|---|---|
| `agora rooms`, `agora watch <room>` | list rooms and invitations; follow one live |
| `agora accept <room>` | take an invitation, unless `agora trust <name> --level 2` lets that peer's invitations through |
| `agora say <room> "<text>"` | say something as a human, or answer when the agents ask you |
| `agora wait <room>` | the result of a room that paused for your answer |
| `agora invite <room> <peer>` | bring another peer into the running room |
| `agora discuss … --turns N --tokens N --lean` | turns per peer; token budget; discuss without tools |

The shared memory and what happened:

| Command | |
|---|---|
| `agora remember "<text>" --title "<title>" --importance high` | save a fact on all peers |
| `agora memory`, `agora memory show <id>`, `agora forget <id>` | list, read, remove |
| `agora block add "<term>"` | never let this text into the shared memory from this machine |
| `agora obsidian <folder>` | mirror the entries into an Obsidian vault |
| `agora events`, `agora notify '<command>'` | what happened; run a command for every event, for example to message your phone |
| `agora stats`, `agora bench` | what the rooms cost; what a session costs on this machine |

## A note on plan arithmetic

Bundling accounts does not make usage cheaper per unit. At the time of writing a Pro plan costs $20 and a Max plan $100 for five times the Pro allowance per window, so two Pro plans give two units for $40 and Max gives five for $100. Bundling helps when the accounts exist anyway, for two people working together, or when one plan is not enough. Machines on the same account share one limit.

Check the terms of your Claude plan before running several accounts. Sharing one account between people is not permitted by Anthropic's consumer terms.

## Compared with

| | agora-mesh | Built-in cross-session messaging | MCP-based peer tools | Agent teams |
|---|---|---|---|---|
| Across machines | yes | yes, with Remote Control | varies | no |
| Different accounts | yes | no, same account | varies | no |
| Traffic path | direct, relay fallback | Anthropic servers | varies | local |
| Tool schemas in context | none | built-in | yes | built-in |
| Shared memory | yes | no | some | task list |
| Equal peers | yes | yes | varies | lead and teammates |

## Status

| Works | Not yet |
|---|---|
| network with human approval, permanent links | a published release, so installation as a plugin |
| rooms: briefing, equal turns, budgets, pause for a human | resuming a room after its opener's daemon restarts |
| splitting work, parallel work on branches, takeover at a usage limit | a review phase after the work phase |
| `ask`, `delegate`, `review`, `peers` | Windows and NAT traversal are untested |
| shared memory with live sync, Obsidian mirror, block list | the daemon as a system service |

The full list of what is missing and untested is in [ARCHITECTURE.md](ARCHITECTURE.md#12-not-built). Development and tests: [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
