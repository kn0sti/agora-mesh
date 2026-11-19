# Contributing

agora-mesh is in early development. [ARCHITECTURE.md](ARCHITECTURE.md) describes what is built; its last section lists what is missing, which is a good place to look for something to do.

## Ground rules

- **Keep the description true.** A change in behaviour changes `ARCHITECTURE.md` in the same pull request. What is not built goes into its "Not built" section, not into the text above it.
- **No MCP.** Agents reach agora-mesh through the `agora` command only.
- **Measure token claims.** A statement about token use needs the command that produced the number and the Claude Code version it ran on.
- **Security-relevant changes** (rights, approval, what leaves a machine, anything that puts a string from a peer into a path or a command) need their own pull request and a note on the risk.
- Keep pull requests small and focused on one thing.

## Development

Rust, stable toolchain.

```
cargo build
cargo test                     # unit tests
tests/e2e-local.sh             # network: join, approval, reconnect, revoke
tests/e2e-rooms.sh             # rooms, budgets, work phase, takeover
tests/e2e-memory.sh            # shared memory
tests/e2e-everyday.sh          # ask, delegate, review, peers, late joiners, lost peers
```

The end-to-end scripts start two or three daemons on one machine and need network access, because the daemons find each other through iroh's public servers. They use no tokens: `tests/fake-claude.sh` stands in for Claude and answers from a script. Its switches (`FAKE_STUBBORN`, `FAKE_ASK`, `FAKE_SPLIT`, `FAKE_LIMIT`, `FAKE_DELAY`) are described at the top of the file.

To try something by hand without touching your own setup, give each daemon its own directory:

```
AGORA_HOME=/tmp/a target/debug/agora setup --name A
AGORA_HOME=/tmp/b target/debug/agora setup --name B
```

`AGORA_CLAUDE_ARGS="--model haiku"` keeps real sessions cheap while testing. The other environment variables are listed in `ARCHITECTURE.md`, section 2.

A change to how rooms behave should come with a check in one of the scripts. A claim that something works with real sessions should say so in the pull request, with what was run.

## Reporting security problems

Please do not open a public issue. Use GitHub's private vulnerability reporting on this repository.

## License

By contributing you agree that your contribution is licensed under the [MIT License](LICENSE).
