# Commands

Use the smallest command that does the job. The first four start no session on your machine; only the peer spends tokens.

| Command | |
|---|---|
| `agora peers` | who is reachable, what they are working on, which agent and model they run on, how much of their usage limit is used |
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
| `agora discuss … --model fable` | the model every Claude session of the room uses; default is each machine's own |
| `agora discuss … --turns N --tokens N --lean` | turns per peer; token budget; discuss without tools |
| `agora discuss … --approve` | wait for your go once the peers agreed on a split, before anyone starts working |
| `agora go <room>` | start the work as agreed; on a machine a package waits on: build it |
| `agora say <room> "<what to change>"` | instead of the go: send the split back to the peers |
| `agora cancel <room>` | close the room without the work; on a machine a package waits on: refuse it |
| `agora discuss … --work-minutes N --work-tokens N` | minutes one package may take (60); tokens of the work phase after which no further package starts |

The shared memory and what happened:

| Command | |
|---|---|
| `agora remember "<text>" --title "<title>" --importance high` | save a fact on all peers |
| `agora memory`, `agora memory show <id>`, `agora forget <id>` | list, read, remove |
| `agora block add "<term>"` | never let this text leave this machine: not into the shared memory, and not to a peer in a question, a task, a diff or a room |
| `agora obsidian <folder>` | connect the memory with an Obsidian vault, in both directions |
| `agora events`, `agora notify '<command>'` | what happened; run a command for every event, for example to message your phone |
| `agora stats`, `agora bench` | what the rooms cost; what a session costs on this machine |
| `agora harness` | which agent and which model this machine's sessions run on, see below |

[Back to the README](../README.md)
