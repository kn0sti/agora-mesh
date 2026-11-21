# How it works

```
        Machine A                                   Machine B
 ┌──────────────────────┐                   ┌──────────────────────┐
 │ your agent session   │                   │  (no session needed) │
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
 │ (a fresh agent)      │◄ ─ ─ ─ ─ ─ ─ ─ ─ ►│ (a fresh agent)      │
 └──────────────────────┘                   └──────────────────────┘
```

1. Each machine runs a small background daemon with its own key pair.
2. One machine opens a network; others join with the network ID. A human approves every join.
3. Any peer opens a room for a topic. Each invited daemon starts a fresh session in its own clone of the project, on the agent and model that machine is set up for.
4. The opener briefs the others; then all take equal turns under a budget until they agree, split the work, or ask a human. Whoever makes a proposal has agreed to it; the room is agreed when every other peer says so. The first peer to answer a proposal is asked to look for its weakest point before it agrees.
5. If they agreed on a split, each peer does its package on its own branch, at the same time. With `--approve` the room first shows you the split and waits for your go.
6. The result goes back to the session that asked, and into the shared memory.

The full design is in [ARCHITECTURE.md](../ARCHITECTURE.md).

[Back to the README](../README.md)
