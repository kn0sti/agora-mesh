# Architecture

This document describes agora-mesh as it is built. What is planned but missing is collected in [section 12](#12-not-built), so that nothing above it describes something that does not exist.

## 1. Goals and constraints

1. **Symmetric peers.** No master, no worker. Any peer may open a room, criticise, ask, or take work.
2. **No VPN, no cloud account, no port forwarding.** NAT traversal is part of the tool.
3. **Independent of the Claude account.** Each peer uses its own local `claude`. Peers may be on the same account or on different ones.
4. **No MCP.** Agents use the `agora` command through the shell. Nothing adds tool schemas to the context.
5. **Token protection.** Free discussion between agents is bounded by the daemon, not by the model's goodwill.
6. **Agent-first installation.** A user tells Claude Code to install the repository; `INSTALL.md` guides it.
7. **Runs wherever Claude Code runs.** That is the aim. Tested so far: macOS 13 on x64 and Linux on x64.

## 2. Components

| Component | Role |
|---|---|
| `agora` daemon | One per machine. Holds the identity key, the links to peers, the rooms and the shared memory. Starts and feeds room sessions. |
| `agora` command | Thin client. It talks to the local daemon over TCP on 127.0.0.1, with a token that only the user can read, and starts the daemon if it is not running. |
| Room session | An agent process the daemon starts for one room: Claude Code in its headless mode, or an agent that speaks the Agent Client Protocol (section 5, "Harness profiles"). |
| Plugin | The Claude Code plugin in this repository: a launcher for the command, a skill, a hook and a monitor (section 10). |

One Rust binary is both daemon and command. Source: `src/main.rs` (command), `daemon.rs` (links, control socket, commands), `room.rs` (rooms), `session.rs` (sessions, for both kinds of program), `harness.rs` (profiles and presets), `proto.rs` (messages), `memory.rs` (shared memory), `project.rs` (git remotes), `state.rs` (persistent state).

### Files

Everything lives under `~/.agora/` (`AGORA_HOME` names another directory, which also lets several daemons run on one machine).

| Path | Content |
|---|---|
| `state.json` | peer name, secret key, networks and their peers, registered projects, block list, settings; readable by the user only |
| `daemon.json`, `daemon.log` | port and token of the running daemon; its log |
| `harness.toml` | this machine's harness profiles and which one is the default (section 5) |
| `events.log` | one line per event (section 8) |
| `rooms/<id>/` | `log.md`, `result.md`, `closing.md`, `stats.json`, `timeline.tsv`, `members` of one room; removed 30 days after the last change |
| `shared/<network>/` | the shared memory of a network (section 8) |
| `bin/<version>/agora` | the binary the plugin's launcher downloaded |

### Environment

| Variable | Effect |
|---|---|
| `AGORA_HOME` | data directory |
| `AGORA_CLAUDE` | the `claude` executable to start for sessions |
| `AGORA_CLAUDE_ARGS` | further arguments for every session, for example `--model sonnet` |
| `AGORA_HARNESS` | the harness profile sessions run on, instead of the default in `harness.toml` |
| `AGORA_ACP_TRACE` | a file that receives every message exchanged with an agent over the Agent Client Protocol, for debugging |
| `AGORA_PERMISSION_MODE` | permission mode of sessions with full rights; default `auto` |
| `AGORA_LIMIT_AT` | utilization from which a peer counts as limited; default `0.97` |
| `AGORA_BIN`, `AGORA_RELEASE_BASE` | launcher only: use this binary, or download from this base URL |

The daemon reads these when it starts; a session inherits the daemon's environment.

## 3. Transport

**Decision: iroh 1.x.**

| | iroh | libp2p | Hyperswarm |
|---|---|---|---|
| Language | Rust | Go, Rust, JS | JavaScript only |
| Addressing | public key | peer ID plus DHT | topic in a DHT |
| If the direct connection fails | relay fallback, always connects | relay only coordinates; measured about 70 % success, about 40 % behind symmetric NAT | fallback not confirmed |
| Effort | low | high | low, needs Node |

The public key is the peer identity the handshake needs anyway, and the relay fallback is what makes "works everywhere" achievable. NAT traversal always needs a meeting point: agora-mesh uses iroh's public relays and address lookup (no account; traffic is end-to-end encrypted).

How it is used:

- One QUIC connection per pair of peers, with one bidirectional stream. Every message is a CBOR value with a four-byte length prefix, at most 1 MiB.
- Of two peers that know each other, the one with the smaller ID dials. A peer that is joining dials the peer whose network ID it was given.
- Each side sends a heartbeat every 15 s and drops a link that has been silent for 45 s. Missing links are dialled again every 10 s.
- `agora status` and `agora peers` show whether a peer is reached directly or through a relay, and the round-trip time.

Between a Mac and a Linux machine in one local network the link was direct with 11 to 21 ms round trip. Traversal of a real NAT between two networks has not been tested.

## 4. Network, identity, trust

- `agora setup --name <name>` creates an Ed25519 key pair. The public key is the peer ID.
- `agora open` creates a network and prints a network ID: the network's name, a secret, and the opener's peer ID.
- `agora join <network-id>` dials the opener and sends `PEER_HELLO` with a proof that it knows the secret, bound to its own peer ID. The opener keeps the request pending.
- Both machines show the same four words, derived from the two peer IDs. The humans compare them, then one runs `agora approve <name>`. A leaked network ID alone is therefore not enough to get in. `agora reject <name>` refuses.
- The approving peer sends the member list; every member learns of the new one, and the members connect to each other.
- A network is permanent. `agora revoke <name>` removes a peer: it is told, drops the network, and has to be approved again to return. Rooms that peer opened on this machine are stopped.
- The member list carries a version that goes up with every approval and revocation. When two members connect, the list with the higher version replaces the other. A peer that was away therefore cannot bring back a member that was revoked meanwhile, and it learns of members that joined.

**What a peer may do before it is approved:** ask to join, nothing else. Member lists, room messages and memory entries are processed only on a link that belongs to an approved member of that network.

**What comes from a peer is checked before it is used.** A network ID is handed over by someone else: the network's name in it must be plain hex before it is stored or becomes a directory name. Room ids must be alphanumeric. A peer's name for itself is reduced to letters, digits, `-`, `_` and `.`. No string from a peer reaches a shell or a git argument.

### Level and rights

Each machine decides for every peer what that peer may do here, with `agora approve --level --rights` or later `agora trust <name> --level --rights`. A member list received from another peer never changes these. A peer learned from a member list starts at level 1 with full rights.

| Setting | Values | Effect |
|---|---|---|
| level | 1, 2, 3 | from 2 on, this peer's invitations are accepted without `agora accept`; 3 has no further effect yet |
| rights | `full` | its rooms' sessions here run with `--permission-mode auto`: Claude Code's own safety classifier stays active, nothing else is restricted |
| | `read` | sessions get the tools `Read`, `Grep`, `Glob` only |
| | `talk` | sessions get no tools |

**A peer with full rights can make this machine run commands.** That is the purpose of the tool and its main risk. The warning is shown by `agora open` and `agora join`.

`agora panic` stops all rooms on this machine, also in the middle of a turn: every session runs in its own process group, and the whole group is killed, so a command the session was running ends with it. `agora stop` stops the daemon through its control socket, and with it every session.

## 5. Rooms

A room is one topic and a set of peers. The peer that opens it is the opener; its daemon numbers the messages and names the next speaker. That is ordering, not authority: after the briefing every peer takes equal turns.

Every participating machine keeps the room's log under `rooms/<id>/log.md`.

### Lifecycle

1. **Invite.** `agora discuss "<topic>" [--with a,b]` sends `ROOM_INVITE` to the named peers, or to all that are online. A machine takes part in at most two rooms at once. The opener repeats the invitation every 10 s until a peer has joined, and gives up after 120 s.
2. **Briefing.** The opener's session writes a briefing (`BRIEF`). Each joiner answers once with what it knows and what it lacks (`KNOWN_MISSING`). The opener fills the gaps (`ANSWER`). None of this counts against the budget.
3. **Discussion.** The joiners speak in the order they were invited, then the opener, and so on. The next speaker is always the next peer after the last speaker that still has turns. Default budget: 6 turns per peer (`--turns`).
4. **Close.** The room closes
   - when every peer still taking part answered `CONSENSUS` to the proposal that stands: consensus. The author of that proposal has agreed by making it and need not say so; without a proposal everyone has to. At least one peer must have said `CONSENSUS`. A new proposal makes every earlier answer stale: a `CONSENSUS` or a `PASS` was given to what stood before;
   - when the same holds with `PASS` counted too: nothing left to add;
   - when only one peer has turns left: turn budget used up;
   - when all sessions together have used more tokens than the token budget (default 5,000,000, `--tokens`; input tokens count in full, cache reads included);
   - when fewer than two peers remain.
5. **Result.** The opener's session writes the result for the human: the agreed result after a consensus, otherwise an overview of what is agreed, disputed, and left to the human. `agora discuss` prints it. Every machine keeps it as `result.md`.

A contribution starts with its kind on the first line. A session may send `PROPOSAL`, `CRITIQUE`, `QUESTION`, `ANSWER`, `SPLIT_WORK`, `PASS`, `CONSENSUS` or `NEED_HUMAN`; anything else counts as a proposal.

### Humans

- `agora watch <room>` follows a room; `agora rooms` lists rooms and open invitations.
- `agora say <room> "<text>"` adds a message as a human, on any participating machine. It does not change whose turn it is. `--turns N` grants every peer N more turns.
- `NEED_HUMAN` pauses the room. `agora discuss` returns with the question. After `agora say` the room goes on, and `agora wait <room>` returns the result. Without an answer the room closes with the overview after one hour.

### Rooms that change while they run

**Joining late.** `agora invite <room> <peer>` brings a peer into a running room. Its invitation carries what was said so far (everything if short, otherwise the briefing and the last six messages) and the number of the last message, so it receives only what follows. It speaks before the opener from then on.

**Lean rooms.** `agora discuss --lean` starts the discussion sessions without tools, which cuts the fixed context per turn from about 24,000 to about 3,700 tokens. The agents then cannot look anything up while they discuss. For the work phase each peer starts a second session with tools and gives it the agreed plan.

**Two sessions without `--lean`.** A peer whose harness nothing checks does the same on its own: it discusses with reading rights and works in a second session. That is every harness except Claude Code on Anthropic's service, where the `auto` mode checks actions. The reason is a measured one: models on other services, given tools and no check, did not wait for the work phase and pushed to `main` during the discussion ([measurement](docs/measurements/2026-11-claude-code-on-another-service.md), part 3). A question or a task (`agora ask`, `agora delegate`) is not a discussion and keeps the rights the peer granted.

**Lost links.** Every message of a room has a number, and every contribution of a joiner has a counter. A joiner that sees a gap, or whose link comes back, asks the opener to send again from the number it lacks (`RESEND`) and repeats its last contribution that was not confirmed. The opener ignores a contribution it has already used.

**Lost peers.** A peer that leaves, is revoked, does not answer within 15 minutes, or whose daemon restarted (the opener asks with `ROOM_PING` when the link is back) drops out of the room. The others go on; a running package of that peer goes to someone else. A session that does not finish a turn in time (15 minutes in the discussion, 60 for a package) is killed, because its late answer would be taken for the answer to the next message.

**Rooms that are gone.** A message for a room a daemon does not have is answered: a join or a contribution with `ROOM_CLOSE`, so that nobody waits for a room that has ended. An invitation that was never accepted is withdrawn when its room closes.

**Restarted opener.** A room does not survive a restart of the opener's daemon. When the joiners reach it again they are told the room is closed, so nobody waits. A joiner gives a room up after three hours without the opener.

### Sessions

The daemon starts a session as a child process and keeps it for the room:

```
claude -p --input-format stream-json --output-format stream-json --verbose \
       --strict-mcp-config --append-system-prompt "<rules of the room>" <rights>
```

Messages from peers are written to its standard input as user messages: one line per message, such as `[#7 Heimserver · CRITIQUE · 2 left]`, followed by the text. The text of the answer is the contribution, so talking costs no tool call.

- **No connectors.** `--strict-mcp-config` is what keeps a session affordable. A default headless session on the test machine read 195,258 input tokens per turn, a room session 23,994 (section 11).
- **Project.** A room is about the git repository `agora discuss` was called in (`--project <path>` names another, `--project none` none). The invitation carries its remote as `host/owner/repo`. Each machine starts the session in its own clone of that project: `agora project add|list|remove` registers clones, and the opener's is registered by itself. Without a clone the session runs in the room directory and is told that it cannot see the project's files.
- **The command.** The directory of the `agora` binary is put on the session's path, so an agent can use the shared memory and the other commands itself.
- **Model.** `--model <name>` on `agora discuss`, `ask` and `delegate` travels in the invitation, and every session of that room is started with it. Without it each machine uses its own default. A name from a peer must look like a model name before it becomes an argument of `claude`. `stats.json` records the model each session reports.
- **Streaming input** is documented for the Agent SDK only; with the command line it works as of Claude Code 2.1.328 and may change.

### Harness profiles

Which program a session runs in, and which model service that program talks to, is a **profile** on each machine. Profiles live in `harness.toml`; `agora harness add|list|presets|remove|default|test` edit and try them. A session uses the profile named with `--harness` on `agora discuss` (the opener's own session only), else `AGORA_HARNESS`, else the file's default, else `claude`: Claude Code as it is installed. A joiner always uses its own default, because only that machine knows what is installed and logged in there.

```toml
default = "deepseek"

[profile.deepseek]
kind = "claude"
external = true

[profile.deepseek.env]
ANTHROPIC_BASE_URL = "https://api.deepseek.com/anthropic"
ANTHROPIC_AUTH_TOKEN = "file:~/.keys/deepseek"
ANTHROPIC_MODEL = "deepseek-flash[1m]"

[profile.opencode]
kind = "acp"
command = ["opencode", "acp"]
external = true
```

A value `env:NAME`, `file:PATH` or `cmd:COMMAND` in `env` is looked up when a session starts, so the file holds no key; `agora harness add` refuses a key in place of its source. Peers are told a profile's name, kind, model and whether it is external, nothing else.

**Kind `claude`** is the session described above, with the profile's program, environment and permission mode. When the profile names another address (`ANTHROPIC_BASE_URL`), the session is started differently:

- **Bare.** `--bare` keeps Claude Code from loading `CLAUDE.md` files, memory, hooks and plugins, and leaves it three tools: `Bash`, `Edit` and `Read`. Started the usual way, a session sent a `CLAUDE.md` from above the project and its settings to the other service with every request, and a request was ten times the size ([measured](docs/measurements/2026-11-claude-code-on-another-service.md)). `bare = false` in the profile turns this off. With the rights `read` a bare session has `Read` only, because it has no search tools.
- **Never `auto`.** Claude Code cannot run the safety check of the `auto` mode there, and in that mode it sends the user's permission rules, paths and the state of the repository along. The session runs in `default`: it can read and run read-only commands and is refused the rest, so it cannot finish a work package. A profile added with `--permission-mode bypassPermissions` works, unchecked.
- **The shared memory.** `agora memory` and `agora remember` are allowed without asking, so that the session can use what the rules of the room tell it about. No other `agora` command is.
- **Credentials.** `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are taken from the profile only. One that the daemon's environment happens to hold is not passed on.
- **Model.** The room's `--model` is not used; it names a Claude model. The room's log notes that the peer keeps its own.

**Kind `acp`** drives an agent over the Agent Client Protocol, version 1: JSON-RPC 2.0, one message per line, on the agent's standard input and output.

1. `initialize` with protocol version 1 and without the client's file-system and terminal capabilities, so the agent uses its own tools. A request for them is answered with "method not found".
2. `session/new` with the working directory and no MCP servers.
3. The profile's `model` is set with `session/set_config_option`, if the agent offers a model option with a value of that name. A mode the profile names for the room's rights (`modes`) is set the same way, or with `session/set_mode`; if that fails, the session does not start.
4. The rules of the room are sent once, in front of the first message.
5. `session/prompt` per turn. The contribution is the text the agent sent after its last tool call in that turn.
6. `session/request_permission` is answered from the rights: refuse for `talk`, allow tools of the kinds `read`, `search` and `think` for `read`, allow for `full`. The option is chosen by its kind, never by its label. An agent that does not ask is not restrained by this: OpenCode ran its tools without a request in a real run. The presets for OpenCode, Qwen Code and Codex therefore name the agent's own read-only mode for `talk` and `read` (step 3); for other agents the two levels are only as strict as the agent asks.
7. An agent that is refused a tool may give up its turn (stop reason `cancelled`); Codex does. It is then told that the tool was refused and asked for its contribution, once.
8. Stopping a turn kills the agent's process group, as for Claude Code.

**What a harness reports** decides what the room can do with it:

| | Claude Code on Anthropic | Claude Code on another service | Agent over the protocol |
|---|---|---|---|
| Tokens per turn | yes | yes, as the service counts them | the `usage` of the turn's end if the agent sends one (OpenCode and Codex do), else the context size it reports (Qwen Code), else an estimate from text length; `stats.json` says `tokens_estimated` for the last two |
| How full the usage limit is | yes | no | no |
| "Limit reached" | status 429 or the usual wording | the same | the usual wording in the agent's error |

A peer that reports no usage limit shows "its harness reports no usage limit" in `agora peers`; work is not moved away from it in advance, and a limit error still hands its package to another peer.

Agents count differently in `usage`: OpenCode and Codex report the tokens of the turn, with cached input apart, while the protocol's draft describes a total of the session. The adapter decides on the second report, where a total has grown by about a whole context and the tokens of a turn have not, and keeps the answer for the session. `AGORA_ACP_TRACE=<file>` writes every message exchanged with an agent to a file, prompts and answers included, to see what an agent really sends ([three agents compared](docs/measurements/2026-11-real-agents.md)).

**External.** A profile whose prompts go to a service other than Anthropic's or the machine itself is marked `external`. In a directory with a file `.agora/no-external`, or below one, sessions of such a profile do not start: the opener's `agora discuss` fails before anyone is invited, and a joiner declines the room with that reason. This guards a project on the machine that holds the file. It does not stop a room's messages from reaching an external peer elsewhere.

## 6. Splitting work

**Capacity.** Claude Code prints a `rate_limit_event` with every turn: the utilization of the five-hour and the seven-day window and when each resets. The daemon keeps the tighter of the two as the peer's capacity, sends it to all peers (`STATUS`) and shows it in `agora status` and `agora peers`. A peer counts as limited when the event's status is not `allowed`, when utilization reaches 97 %, or when a turn fails with a limit error.

**Splitting.** A peer proposes a split with `SPLIT_WORK`, one line per package in the form `- <peer name>: <package>`. When the room then reaches consensus it enters the work phase. A later plain `PROPOSAL` replaces the split.

**Work phase.** Every package gets the branch `agora/<room>/<peer>`. The opener's daemon sends `WORK` to each owner; all peers work at the same time, one package per peer at a time, each in its own clone. A session is told to commit on its branch, to push it if it may, to run the checks that exist for what it changed, and to report `DONE` or `BLOCKED` with what it did and which checks ran. When all packages are reported, the opener's session writes the result for the human. The daemon does not run git itself and does not merge; one branch per peer is what keeps peers apart.

**Takeover.** When a peer reaches its usage limit, in the work phase or before a package starts, the package goes to the peer with capacity that has the least to do (`HANDOVER`), together with the branch name; the taker is told to fetch that branch and continue. If nobody has capacity left the package is reported as blocked. In the discussion a limited peer takes no further turns. If the opener's own session is limited, the overview is assembled from the log without a model.

The token budget applies to the discussion; work-phase tokens are only counted.

## 7. Everyday commands

A room is the expensive way to involve a peer. Three commands do less and cost less. None of them starts a session on the calling machine; only the peer spends tokens.

| Command | What happens on the peer |
|---|---|
| `agora ask "<question>" --with <peer>` | one turn that answers the question |
| `agora delegate "<task>" --with <peer>` | one long turn that carries out the task, on the branch `agora/<id>/<peer>` if it has a clone of the project, and reports `DONE` or `BLOCKED` |
| `agora review --with <peer> [--base <ref>]` | one turn that reviews the diff of the working tree against the base; the diff travels in the message (up to 150 kB), so nothing has to be pushed |

They use the invitation of a room with `mode` set to `ask` or `task`, so the same rule decides whether the peer starts at once or waits for `agora accept`.

`agora peers` lists everyone: reachable directly or through a relay, how many rooms each is working on (`BUSY`), and how much of its usage limit is used. A peer's limit is known once a session has run there.

## 8. Shared memory and events

Every network has one shared memory, replicated to all its peers, under `shared/<network>/`:

| Path | Content |
|---|---|
| `<id>.md` | one entry per file, with title, importance, author and clock in the front matter |
| `INDEX.md` | one line per entry of importance high or normal |
| `archive/<id>.md` | entries of importance low |
| `history/<id>.<clock>.md` | older versions of changed entries |
| `entries.json` | what the daemon replicates |

A peer's private Claude memory is never read or written.

**Writing.** `agora remember "<text>" --title "<title>" --importance high|normal|low`. The id is derived from the title, so writing the same title again replaces the entry. `agora memory` lists, `agora memory show <id>` prints, `agora forget <id>` removes on all peers.

| Importance | Effect |
|---|---|
| `high` | every peer records an event, and sessions running in rooms are told the title on their next turn; they fetch the text only if they need it |
| `normal` | listed in the index |
| `low` | archive only |

**Replication.** An entry carries a logical clock and is signed by its author with the peer key. A peer accepts an entry when the sender and the author are members of the network, the signature is valid, and (clock, author) is higher than what it has; the replaced version goes to `history/`. A clock more than a million ahead of the receiver's is refused. Accepted entries are passed on to all other connected members. When a link comes up, both sides send all their entries, which is how a peer that was offline or has just joined catches up.

The author shown for an entry is the name this machine knows that peer by, not what the entry says about itself.

**Room results.** When a room opened here closes with a result, the result is saved as `Room <id>: <topic>` with importance normal (`agora discuss --remember high|normal|low|none`).

**Block list.** `agora block add <term>` names text that must never leave this machine through the memory. `agora remember` and room results that contain a term are refused. Nothing else is checked against it: not what sessions say to each other in a room, and not what `agora ask`, `agora delegate` and `agora review` send to a peer, the diff of a review included.

**Obsidian.** `agora obsidian <vault folder>` connects the memory with an Obsidian vault in both directions.

- Outwards: new and changed entries of importance high and normal are written to `<folder>/agora/<network>/<id>.md`.
- Inwards: the daemon looks at that folder every three seconds. A note that is new or has changed becomes an entry on all peers, written by this machine. The title is the `title` of the front matter or the file name, the importance the `importance` of the front matter or normal. A note under another name than its id is renamed to `<id>.md`, with the front matter added.
- A note that reads back as the entry it mirrors is left alone, so entries from other peers are not imported again as changes.
- The block list applies: a note with a blocked term stays out of the memory. Deleting a note does not delete the entry; `agora forget <id>` does.

**Events.** The daemon appends join requests, invitations, paused and closed rooms and high-importance entries to `events.log`. `agora events` prints the last ones, `agora events --follow` each new one as it happens. `agora notify '<command>'` runs a shell command for every event, with the event in `AGORA_EVENT_KIND` and `AGORA_EVENT_TEXT`; that is the hook for a message to a phone.

## 9. Messages

Messages are authenticated by the QUIC connection: a link belongs to exactly one peer key. They are not signed one by one, with the exception of memory entries, which travel further than one link.

| Message | Fields | Purpose |
|---|---|---|
| `PEER_HELLO` | `peer_name`, `network`, `proof` | ask to join, or reconnect as a member |
| `PEER_PENDING` | | the request waits for a human |
| `PEER_APPROVE` | `peer_name`, `members`, `version` | admitted; the member list |
| `PEER_REJECT` | `reason` | refused |
| `MEMBERS` | `network`, `members`, `version` | the member list; a peer missing from a newer one has been revoked |
| `HEARTBEAT` | | keep-alive |
| `STATUS` | `capacity` | utilization, reset time, limited or not |
| `BUSY` | `rooms`, `harness` | how many rooms the sender is working on, and which harness profile its sessions run on |
| `ROOM_INVITE` | `room_id`, `topic`, `turn_budget`, `token_budget`, `project`, `participants`, `mode`, `catch_up`, `seq`, `lean` | invite into a room, or ask a question, or hand over a task |
| `ROOM_JOIN`, `ROOM_DECLINE` | `room_id`, `reason` | answer; a decline during a room means the peer leaves |
| `SAY` | `room_id`, `kind`, `text`, `tokens`, `extra_turns`, `package`, `n` | joiner to opener: a contribution, a human's message, or a report on a package |
| `DISCUSS_MSG` | `room_id`, `seq`, `from`, `from_name`, `kind`, `text`, `turns_left`, `next` | opener to joiners: a numbered message and who speaks next |
| `WORK` | `room_id`, `package`, `title`, `branch`, `taken_from` | work on this package now |
| `RESEND` | `room_id`, `from_seq` | send the room's messages again from this number |
| `ROOM_PING` | `room_id` | do you still have this room? |
| `ROOM_CLOSE` | `room_id`, `reason`, `result` | the room is closed |
| `MEM_PUT` | `entry` | a version of a shared-memory entry |

Besides the kinds a session may send (section 5), the daemon publishes `BRIEF`, `KNOWN_MISSING`, `HUMAN`, `JOIN`, `TURN`, `WORK`, `DONE`, `BLOCKED`, `HANDOVER`, `LIMIT` and `LEFT`.

The wire format does not affect token use: a model only ever sees the rendered line and the text.

## 10. Claude Code plugin

The repository is itself the plugin and its marketplace.

| Part | File | What it does |
|---|---|---|
| Manifest | `.claude-plugin/plugin.json`, `marketplace.json` | name, version, the hosts the launcher reaches |
| Launcher | `bin/agora`, `bin/agora.cmd` | on the Bash tool's path; on first use downloads the binary of the plugin's version for this platform from the GitHub release, checks it against `SHA256SUMS`, stores it under `~/.agora/bin/` |
| Skill | `skills/agora/SKILL.md` | tells a session when to use which command, to offer a peer's help on large tasks instead of imposing it, and what only the user decides; costs one line of context until it is used |
| Hook | `hooks/` | at session start runs `agora status`, which starts the daemon on a machine that is set up |
| Monitor | `monitors/monitors.json` | runs `agora events --follow`; Claude Code passes each line to the interactive session |

Nothing is written into the user's `CLAUDE.md`. Monitors are an experimental Claude Code feature and run in interactive sessions only. Channels are not used because they need an MCP server.

Releases: a tag `vX.Y.Z` makes `.github/workflows/release.yml` build the binary for seven targets, write `SHA256SUMS` and publish a GitHub release; the tag, `Cargo.toml` and `plugin.json` must carry the same version.

Verified: `claude plugin validate` passes; the launcher against a local stand-in for a release (download, reuse, tampered binary refused, missing checksum refused); the release workflow built all seven binaries for `v0.1.0` and `v0.2.0`; the plugin installs from the marketplace, and a session then finds `agora`, which downloads the released binary and runs it. The plugin adds about 130 tokens to every session. Not verified: the hook and the monitor in an interactive session, and every binary except the one for macOS on x64.

## 11. Measuring

`agora stats` lists the rooms this machine took part in: messages, turns, input tokens per turn, output tokens, seconds, and for rooms opened here the total over all peers. `timeline.tsv` records when each message of a room was published. `agora bench` measures the fixed context of one turn on the local machine, with real sessions on the smallest model:

| Session | Input tokens of a turn that says one word |
|---|---|
| default headless session (7 connectors, 188 tools), tools off | 195,258 |
| without connectors, tools off | 3,743 |
| room session: no connectors, built-in tools | 23,994 |

Sessions that load a large `CLAUDE.md` and memory, or read files during a turn, use more: 42,000 to 101,000 input tokens per turn in the measured rooms. The measurements are in [docs/measurements](docs/measurements/).

## 12. Not built

| Missing | Note |
|---|---|
| Resuming a room after the opener's daemon restarts | design in [docs/design/room-resume.md](docs/design/room-resume.md) |
| Running the daemon as a system service | it starts on demand and with the plugin's hook; it does not start at boot |
| A review phase after the work phase | each peer reviewing the other's branch would have caught part of what the measured run got wrong |
| A board per room and file reservations | a late joiner gets a summary of the log instead; one branch per peer keeps work apart |
| Knowledge levels beyond invitations | no peer card, no mirroring of a peer's Claude memory or other sources |
| A lifetime for networks | networks are permanent until a peer is revoked |
| A log of what remote peers triggered | room logs and `daemon.log` exist; there is no separate audit log |
| A protocol version, and signatures on room messages | peers must run compatible versions; room messages rely on the authenticated link |
| Replication that sends only what the other side lacks | everything is sent when a link comes up, which is fine for a few hundred short entries |
| Profiles for one's own and other people's machines | level and rights are set per peer by hand |
| Adapters of their own for Codex and Pi | both offer more than the Agent Client Protocol carries: Codex a real usage-limit signal, Pi the smallest start; see [docs/design/other-harnesses.md](docs/design/other-harnesses.md) |
| Several participants on one machine | one machine brings one session into a room; a strong and a cheap model side by side need two machines today |
| Telling each session which harness the others run on | the rules of a room call the others "coding agents"; `agora peers` shows the harness to the human |
| Lean start switches per agent | a Claude Code session is started without connectors and with only the tools it needs; for agents over the protocol only the connectors are left out |

Not tested: traversal of a real NAT between two networks; Windows; a real usage limit (it was simulated; utilization and reset time were read from real sessions); the work phase's takeover with real sessions; an agent writing to the shared memory on its own; peers on different accounts; the addresses of the providers the presets point at (sessions, rooms and work phases on other models ran through a reseller); the agents the presets name besides OpenCode, Qwen Code and Codex.

Known weak spots: a peer that is a member of two networks shared with the same peer uses one link, and member lists of the second network do not travel over it. An approved peer can still fill another peer's shared memory or its room queue; approval is the boundary, not a quota.
