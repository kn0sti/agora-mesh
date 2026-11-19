# agora-mesh

> **Status: early development.** Stage 1 of the [roadmap](#roadmap) works: machines can form a network, with human approval, and stay connected. Agents cannot talk to each other yet; rooms come with stage 2. [ARCHITECTURE.md](ARCHITECTURE.md) is the specification.

A symmetric peer-to-peer mesh for Claude Code. It connects the Claude Code CLI on two or more machines so that the agents on them can talk as equals: plan together, criticise each other's ideas, split large tasks, review each other's code and keep a shared memory.

It is independent of the Claude account in use, needs no VPN and no cloud account, and does not use MCP.

*agora-mesh is an independent community project. It is not affiliated with or endorsed by Anthropic.*

## Why

- **Bundle capacity.** Each peer runs its own local `claude` with its own account, usage limits and context window. Two smaller plans, two people, or a work and a private account can work on one task together. When one peer hits its limit, another takes over its open work.
- **Peers, not a harness.** There is no master and no worker. Any machine can open a room, ask a question, object to a proposal or pick up part of a task.
- **Low fixed token cost.** Agents use a small command (`agora`) instead of MCP tools, and room sessions start without connectors. See [measured numbers](#token-cost).
- **Shared memory, live.** What one agent marks as important is on the other machines seconds later, and running sessions are told about it in one line.

### A note on plan arithmetic

Bundling does not make usage cheaper per unit. At the time of writing a Pro plan costs $20 and a Max plan $100 for five times the Pro allowance per window, so two Pro plans give two units for $40 and Max gives five for $100. Bundling is useful when the accounts exist anyway, or when you need more than one plan offers. Agents that discuss also use more tokens in total than one agent working alone.

Check the terms of your Claude plan before running several accounts. Sharing one account between people is not permitted by Anthropic's consumer terms.

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
3. Any peer opens a room for a topic. Each invited daemon starts a fresh Claude session.
4. The opener briefs the others, then all discuss as equals under a turn budget until they reach consensus or ask a human.
5. The result goes back to the session that asked, and the important parts go into the shared memory.

## Security: read this first

**A connected peer can make your machine run commands.** Room sessions are started with full rights by default, because agents that cannot read or change anything cannot collaborate. This applies to the person on the other machine and to anything that person's agent has read, such as a prepared web page.

Connect only machines and people you trust with a shell on your computer. Connections are permanent until you revoke the peer. `agora revoke <peer>` removes one peer; `agora panic` closes every room and stops every remotely started session.

## Comparison

| | agora-mesh | Built-in cross-session messaging | MCP-based peer tools | Agent teams |
|---|---|---|---|---|
| Across machines | yes | yes, with Remote Control | varies | no |
| Different accounts | yes | no, same account | varies | no |
| Traffic path | direct, relay fallback | Anthropic servers | varies | local |
| Tool schemas in context | none | built-in | yes | built-in |
| Shared memory | yes, live | no | some | task list |
| Equal peers | yes | yes | varies | lead and teammates |

## Token cost

Measured on one machine, headless `claude -p`, two turns, built-in tools off, smallest model:

| Room session started as | Input tokens per turn |
|---|---|
| default (7 connectors, 188 tools) | about 191,000 |
| without connectors (`--strict-mcp-config`) | about 3,700 |
| plus a short custom system prompt | about 650 |

These numbers apply to headless sessions. Interactive sessions load connector tools on demand, so they are not comparable. The cost of the built-in tools a full-rights room session needs is not measured yet. `agora bench` will measure the baseline on your own machine instead of quoting ours.

## Quickstart

The one-sentence installation through Claude Code ([INSTALL.md](INSTALL.md)) comes with stage 5. Until then, build from source with a stable Rust toolchain:

```
cargo build --release          # binary: target/release/agora
agora setup --name MacBook     # once per machine
agora open                     # machine A: prints a network ID
agora join <network-id>        # machine B
agora status                   # both: compare the four words
agora approve <name>           # machine A
```

`agora revoke <name>` removes a peer, `agora stop` stops the daemon. `tests/e2e-local.sh` runs the whole flow with three daemons on one machine.

## Roadmap

| Stage | Content | Done when |
|---|---|---|
| 0 ✓ | This repository and the design documents | all decisions are written down |
| 1 ✓ | Network: daemon, keys, join with approval, permanent connection | two machines reconnect on their own after a restart |
| 2 | Rooms: sessions without connectors, briefing, turn rules, watching and joining in | two agents present a plan they discussed |
| 3 | Capacity: status, work splitting, takeover on a full limit, one git branch per peer, token display | a work package moves to the other peer on a full limit |
| 4 | Shared memory: live sync, importance levels, closing notes, Obsidian, block list | an entry from A is visible on B seconds later |
| 5 | Plugin wrapper and prebuilt binaries for every platform | installation works with one sentence to Claude |
| 6 | Release: measured numbers, legal check, public repository | |

## License

[MIT](LICENSE)
