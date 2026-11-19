# Architecture

This document is the specification agora-mesh is built against. It records decisions and their reasons. Items marked **unverified** are assumptions that the stage named next to them has to test.

## 1. Goals and constraints

1. **Symmetric peers.** No master, no worker. Any peer may open a room, criticise, ask, or take work.
2. **No VPN, no cloud account, no port forwarding.** NAT traversal is part of the tool.
3. **Independent of the Claude account.** Each peer uses its own local `claude`. Peers may be on the same account or on different ones.
4. **No MCP.** Agents use the `agora` command through the shell. Nothing adds tool schemas to the context.
5. **Token protection.** Free discussion between agents is bounded by the daemon, not by the model's goodwill.
6. **Agent-first installation.** A user tells Claude Code to install the repository; `INSTALL.md` guides it.
7. **Runs wherever Claude Code runs.** macOS 13+, Windows 10 1809+, Ubuntu 20.04+, Debian 10+, Alpine 3.19+; x64 and ARM64.

## 2. Components

| Component | Role |
|---|---|
| `agora` daemon | One per machine. Holds the identity key, peer connections, rooms, and the shared memory. Starts and feeds room sessions. |
| `agora` command | Thin client that talks to the local daemon over a local socket. This is what agents and humans call. |
| Room session | A fresh headless `claude -p` process started by the daemon for one room. |
| Plugin | Claude Code plugin that ships the command (`bin/`), a skill with usage instructions, hooks, and a monitor. |

Implementation language: Rust. Distribution: prebuilt binaries per platform, fetched on first session start into the plugin's persistent data directory and verified by checksum; local build as an alternative.

## 3. Transport

**Decision: iroh 1.x.**

| | iroh | libp2p | Hyperswarm |
|---|---|---|---|
| Language | Rust | Go, Rust, JS | JavaScript only |
| Addressing | public key | peer ID plus DHT | topic in a DHT |
| If the direct connection fails | relay fallback, always connects | relay only coordinates; measured about 70 % success, about 40 % behind symmetric NAT | fallback not confirmed |
| Parallel rooms | one QUIC stream per room | streams and GossipSub | one stream per connection |
| Effort | low | high | low, needs Node |

Reasons: the public key is the peer identity the handshake needs anyway; the relay fallback is what makes "works everywhere" achievable; QUIC streams map directly onto rooms.

NAT traversal always needs a meeting point. agora-mesh uses iroh's public relays by default (no account, traffic end-to-end encrypted). On a local network peers connect directly. A self-hosted relay is a configuration option.

**Unverified (stage 1):** behaviour across a real NAT between two home networks.

## 4. Network, identity, trust

- Each daemon generates an Ed25519 key pair on first start. The public key is the peer ID.
- `agora open` creates a network and prints a network ID (a ticket containing the opener's peer ID and a network secret).
- `agora join <network-id> --name <nickname>` asks to join. A member has to approve with `agora approve <nickname>`. Both screens show the same four words derived from the two keys; the humans compare them, so a leaked network ID alone is not enough.
- **Lifetime: permanent by default.** A network lasts until a peer is revoked. Daemons reconnect on their own after a restart. A lifetime (`5h`, `24h`, `7d`) is optional.
- `agora revoke <peer>` removes a peer. `agora panic` closes all rooms and stops all remotely started sessions.
- Every action a remote peer triggers is written to a local audit log.

### Rights

Room sessions run with **full rights by default**. The warning is shown when a network is opened, when it is joined, and when a peer is approved; a human confirms it, never the installing agent. Per peer the level can be lowered to `talk` (no tools) or `read` (read-only tools).

### Knowledge levels per peer

| Level | What the peer receives |
|---|---|
| 1 (default for new peers) | peer card: role, capabilities, reachable repositories; plus entries explicitly shared |
| 2 | additionally a mirror of the Claude memory directory |
| 3 | additionally configured external sources such as an Obsidian vault, as an index |

A block list (paths and tags) names content that never leaves the machine at any level.

Level and rights are local policy. Each machine decides them for every peer with `agora approve --level --rights` or later with `agora trust <peer> --level --rights`; a member list received from another peer never changes them. A peer learned from a member list starts at level 1 with full rights.

## 5. Rooms

A room belongs to one topic or task and has three parts:

- **Log.** Append-only, numbered, signed messages.
- **Board.** A small document with the current state: plan, decisions, open questions, who does what. A session that joins late or is woken reads the board and the log entries after its read marker, never the whole log.
- **Reservations.** Which peer is working on which files.

Code stays in git. A room names its project by git remote URL; each peer resolves that to its local clone, or uses a neutral room directory. Each peer works on its own branch; the room carries references to branches and commits. There is no live sync of working trees.

### Lifecycle

1. **Invite.** The opener sends `ROOM_INVITE`. Invitations from peers at level 2 or higher are accepted automatically; others need a confirmation. At most two rooms per peer run at once.
2. **Briefing.** The opener states topic, goal and known facts and points to shared-memory entries. Each joiner answers once with what it already knows and what it lacks. The opener fills the gaps, at most two turns per joiner. Briefing turns do not count against the budget.
3. **Discussion.** All participants have equal rights. Two peers alternate; three or more go round-robin and may pass. Default budget: six turns per peer.
4. **Close.** The room closes when all participants have sent `CONSENSUS` for the same proposal hash, when all pass in one round, or when the budget is used up. Without consensus the opener's session writes an "agreed / disputed" overview and the room reports `NEED_HUMAN`.
5. **Closing notes.** The opener's session writes the result and decisions to the shared memory (high importance), a summary as a note (normal), and the raw log to a local archive that expires after 30 days (low).

`NEED_HUMAN` pauses the room. `agora discuss` returns with the question; a human on any participating machine answers with `agora say <room> "<answer>"`, optionally granting more turns with `--turns N`, and `agora wait <room>` returns the result. If nobody answers within an hour the room closes with the overview.

Joiners report the tokens their session has used with every contribution. When the sum over all peers passes the room's token budget (default 1,000,000, `agora discuss --tokens`), the room closes with the overview.

The opener's daemon assigns the message numbers. This is ordering, not authority. If it drops out, the room pauses until it returns.

### Token protection

Enforced by the daemon:

- turn budget and a token budget per room, taken from the usage figures in the session output;
- every message must carry a type; pure acknowledgements are not a type;
- messages refer to earlier ones by number, and board changes are sent as changed lines only;
- `NEED_HUMAN` pauses the room; a human may grant more turns;
- an idle timeout closes abandoned rooms.

## 6. Wire protocol

Frames are CBOR with a length prefix, one QUIC stream per room plus a control stream per peer. `agora log --json` prints any stream as NDJSON for debugging. The wire format does not affect token use: the model only sees the rendering, one header line such as `[Heimserver · CRITIQUE · 5 left]` followed by the text.

Common envelope fields: `v` (protocol version), `type`, `from` (peer ID), `seq` (room messages), `ts`, `sig` (Ed25519 over the frame).

| Message | Fields | Purpose |
|---|---|---|
| `PEER_HELLO` | `peer_name`, `pub_key`, `network`, `ttl_expires?` | join request |
| `PEER_APPROVE` / `PEER_REJECT` | `peer`, `level`, `rights`, `members` | answer, signed member list |
| `PEER_REVOKE` | `peer` | remove a peer |
| `CONTEXT_EXCHANGE` | `level`, `card`, `entries[]` | peer card and shared entries, both directions |
| `MEM_PUT` | `id`, `clock`, `importance`, `title`, `body_ref` | shared-memory entry |
| `ROOM_INVITE` | `room_id`, `topic`, `project?`, `turn_budget`, `token_budget` | open a room |
| `ROOM_JOIN` / `ROOM_DECLINE` | `room_id` | answer |
| `DISCUSS_MSG` | `room_id`, `kind`, `re?`, `turns_left`, `text` | room contribution |
| `BOARD_PATCH` | `room_id`, `base`, `lines` | change to the board |
| `STATUS` | `capacity`, `limited_until?`, `rooms` | capacity of a peer |
| `HANDOVER` | `room_id`, `package`, `branch`, `state` | pass a work package |
| `ROOM_CLOSE` | `room_id`, `reason`, `result_ref?` | close a room |
| `HEARTBEAT` | | keep-alive |

`DISCUSS_MSG.kind` is one of `BRIEF`, `KNOWN_MISSING`, `PROPOSAL`, `CRITIQUE`, `QUESTION`, `ANSWER`, `SPLIT_WORK`, `PASS`, `CONSENSUS`, `NEED_HUMAN`.

## 7. Claude Code integration

### Room sessions

The daemon starts a room session as a child process:

```
claude -p --input-format stream-json --output-format stream-json --verbose --strict-mcp-config …
```

Peer messages are written to its standard input as user messages. The text of the turn's answer is the room contribution, so talking costs no tool call. The first line of the answer carries the message kind.

Tested on Claude Code 2.1.328: two messages sent 12 seconds apart to one process arrived and the context was kept. The documentation describes streaming input for the Agent SDK only, so this may change.

`--strict-mcp-config` is mandatory. A default headless session loads every connected connector; measured cost is about 191,000 input tokens per turn against about 3,700 without.

Measured in stage 2 with the built-in tools enabled (smallest model): about 25,500 input tokens per turn in an empty room directory, and 42,000 to 53,000 per turn on machines whose session loads a large `CLAUDE.md` and memory and uses tools during the turn. That context is the peer's own knowledge, which is the point of asking it, but it is the main cost of a room.

Rights map to the session as follows: `full` starts it with `--permission-mode auto` (Claude Code's own safety classifier stays active; `AGORA_PERMISSION_MODE` overrides the mode), `read` with `--tools Read,Grep,Glob`, `talk` with no tools. `AGORA_CLAUDE_ARGS` appends further arguments, for example a model.

A room's project is the git repository `agora discuss` was called in (`--project <path>` names another, `--project none` none). The invitation carries its remote in the normalized form `host/owner/repo`. Each machine keeps a list of local clones (`agora project add|list|remove`; the opener's clone is added by itself) and starts the session in the clone whose `origin` matches. Without a match the session runs in a neutral directory per room and is told that it cannot see the project's files. Log and result always stay in the room directory under `~/.agora/rooms/`.

In the current implementation room messages travel on the peer link instead of a stream per room, and a room has one message more than section 6 lists: `SAY`, a joiner's contribution before the opener has numbered it.

### Interactive sessions

- The session the human types in calls `agora discuss "<topic>" --with <peer>` as a background command and receives the final plan. It pays for the result, not for the discussion.
- The plugin's monitor prints one line per event (invitation, room result, new high-importance memory entry); Claude Code delivers each line to the running session as a notification. Through it an interactive session can also take part in a room directly.
- Hooks start the daemon at session start and react to changed memory files.
- `agora watch <room>` shows a room live; `agora say <room> "<text>"` lets the human take part. Where tmux exists the daemon can also open a pane.

Monitors are marked experimental and run in interactive sessions only. Channels are not used because they require an MCP server.

### Instructions

Usage instructions ship as a skill, which costs one line of context until it is needed. Nothing is written into the global `CLAUDE.md`.

## 8. Capacity bundling

**Capacity.** Claude Code prints a `rate_limit_event` with every turn: the utilization of the five-hour and the seven-day window and when each resets. The daemon keeps the tighter of the two as the peer's capacity, sends it to all peers as `STATUS` when it changes, and shows it in `agora status`. A peer counts as limited when the event's status is not `allowed`, when utilization reaches 97 % (`AGORA_LIMIT_AT`), or when a turn fails with a limit error.

**Splitting.** A peer proposes a split with `SPLIT_WORK`, one line per package in the form `- <peer name>: <package>`. When all peers answer `CONSENSUS`, the room enters the work phase. A later plain `PROPOSAL` replaces the split.

**Work phase.** Every package gets the branch `agora/<room>/<peer>`. The opener's daemon sends `WORK` to each owner; all peers work at the same time, one package per peer at a time, each in its own clone of the project. A session commits on its branch and pushes it if it may. It reports `DONE` or `BLOCKED` with a short summary. When all packages are reported, the opener's session writes the result for the human. The daemon does not run git itself, does not merge, and does not reserve files; one branch per peer is what keeps peers apart.

**Takeover.** When a peer reaches its usage limit, in the work phase or before a package starts, the package goes to the peer with capacity that has the least to do (`HANDOVER`), together with the branch name; the taker fetches that branch and continues. If nobody has capacity left the package is reported as blocked. In the discussion a limited peer takes no further turns, and the room closes when fewer than two peers remain. If the opener's own session is limited, the overview is assembled from the log without a model.

The token budget applies to the discussion only; work-phase tokens are counted in the statistics.

**Unverified:** the wording and status of a real limit error. Utilization and reset time were read from real sessions; a full limit was only simulated in tests (status 429 and the usual phrases are accepted). Takeover between machines also needs the limited peer to have pushed its branch, which it cannot do once it is limited mid-turn.

## 9. Shared memory

- One directory per network, in the same format as Claude's memory (one fact per file, plus an index). A peer's private memory is never overwritten.
- Each peer keeps a signed append-only log of its changes. Peers merge by union; when two change the same entry, the later logical clock wins and the older version is kept.
- A session writes with `agora remember "<text>" --importance high|normal|low`.
  - `high`: running sessions of all peers receive one line with the title at once; the content is fetched on demand.
  - `normal`: appears in the index, visible at the next session or room start.
  - `low`: archive only.
- Adapters mirror the shared directory into an Obsidian vault folder and add a single pointer line to the Claude memory index.

**Open (stage 4):** own replication as described, or iroh's document sync.

## 10. Measuring

`agora stats` lists the rooms this machine took part in: messages, turns, input tokens per turn, output tokens, and for rooms opened here the total over all peers. `agora bench` measures the fixed context of one turn on the local machine for a default headless session, one without connectors, and a room session; it uses real sessions on the smallest model. Published comparisons state the method and compare like with like.

## 11. Open points

| Point | Stage |
|---|---|
| NAT traversal between two real home networks | 1 |
| Behaviour at a real usage limit (only simulated so far) | 3 |
| Work phase and takeover with real sessions and a real repository | 3 |
| Replication of the shared memory | 4 |
| Plugin monitor and Windows behaviour in practice | 5 |
| Original wording of Anthropic's terms on several accounts | 6 |
