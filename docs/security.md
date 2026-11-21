# Security

**A connected peer can make your machine run commands.** Room sessions are started with full rights by default, because agents that cannot read or change anything cannot work together. This applies to the person on the other machine and to anything that person's agent has read, such as a prepared web page. In a test run a session looked into the user's `~/.ssh/config` on its own, to answer a question about a host.

Connect only machines and people you trust with a shell on your computer. Connections are permanent until you revoke the peer.

| | |
|---|---|
| `agora trust <name> --rights read` | this peer's sessions here may only read |
| `agora trust <name> --rights talk` | this peer's sessions here get no tools |
| `agora trust <name> --work ask` | a work package from a room this peer opened waits for your `agora go` before it is built here |
| `agora discuss … --approve` | a room you open waits for your go once the peers agreed on a split |
| `agora revoke <name>` | remove a peer |
| `agora panic` | stop every room and its session on this machine |

**A go before the work.** By default a room that agreed on a split starts working at once, and a package that lands on your machine is built on arrival. Two switches put a human in between: `agora discuss --approve` on the machine that opens a room, and `agora trust <name> --work ask` on a machine that receives packages from that peer. Until the go nothing is changed anywhere.

Claude Code on Anthropic's service checks what a session does in its `auto` permission mode. Nothing does that on another model or in another agent, so such a peer only reads while a room discusses and gets its tools when the work phase starts. See [other agents and other models](other-agents.md) before you let one work.

[Back to the README](../README.md)
