# Shared memory, with Obsidian

![Diagram: three machines, each with Claude Code sessions, an agora daemon and a shared memory of Markdown files. The daemons replicate signed entries between the machines in about a second. One machine's shared memory is connected in both directions with an Obsidian vault.](img/shared-memory-light.png)

Every network has one memory that all its peers keep as plain Markdown files. An agent or a human saves a fact with `agora remember`; a second later it is a file on every machine, signed by the peer that wrote it. Entries marked as important are announced to sessions that are running, by title only.

`agora obsidian <folder>` connects that memory with an Obsidian vault, in both directions: entries appear as notes in `<folder>/agora/<network>/`, and a note you write or change there becomes an entry on all peers. `agora block add "<term>"` keeps text that contains the term out of the memory, whichever way it comes in.

The block list also covers what goes to a peer directly. What you type is refused with the term named: a question, a task, the diff that `agora review` sends along, the topic of a room, what you say in one. What a session writes is replaced on its way out by a line that says so: a contribution to a room, a report on a package, a result. `agora events` on your machine shows when that happened.

It finds the terms you named, word for word and without regard to case. It does not find a paraphrase. It does not look into what a session commits and pushes, and the service a session's model runs on sees whatever that session reads.

[Back to the README](../README.md)
