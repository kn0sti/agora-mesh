# Contributing

agora-mesh is in the planning stage. The most useful contributions right now are comments on [ARCHITECTURE.md](ARCHITECTURE.md): open an issue and name the section.

## Ground rules

- **The specification comes first.** A change in behaviour starts with a change to `ARCHITECTURE.md`.
- **No MCP.** Agents reach agora-mesh through the `agora` command only.
- **Measure token claims.** A statement about token use needs the command that produced the number and the Claude Code version it ran on.
- **Security-relevant changes** (rights, approval, what leaves a machine) need their own pull request and a note on the risk.
- Keep pull requests small and focused on one thing.

## Development

Rust, stable toolchain. Build and test instructions follow with stage 1.

## Reporting security problems

Please do not open a public issue. Use GitHub's private vulnerability reporting on this repository.

## License

By contributing you agree that your contribution is licensed under the [MIT License](LICENSE).
