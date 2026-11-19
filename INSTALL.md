# Installation guide for Claude Code

This file is written for a Claude Code agent that a user asked to install agora-mesh. Follow the steps in order and show the user what you do.

> **Status: planning.** There is nothing to install yet. If a user asks you to install agora-mesh now, tell them so and point them to the roadmap in `README.md`. Do not improvise an installation.

The steps below describe the intended flow once stage 5 is released.

## Rules for the installing agent

- Run only the commands listed here. If a step fails, stop and show the user the error.
- Never confirm the security warning yourself. The user has to read and confirm it.
- Do not write to the global `CLAUDE.md`. agora-mesh ships its instructions as a skill.
- Do not connect to any network or approve any peer on your own initiative.

## Steps

1. **Check the platform.** Supported: macOS 13+, Windows 10 1809+, Ubuntu 20.04+, Debian 10+, Alpine 3.19+; x64 or ARM64. On Windows, Git for Windows must be installed.
2. **Install the plugin.**
   ```
   claude plugin install agora-mesh --marketplace kn0sti/agora-mesh
   ```
3. **Show the security warning** from the "Security" section of `README.md` to the user in full and ask them to confirm it in their own words.
4. **Ask the user** for:
   - a peer name for this machine, for example `MacBook` or `Heimserver`;
   - whether this machine should run the daemon permanently as a system service (recommended for machines that are always on) or only while a session is open.
5. **Start the daemon** with `agora setup --name <peer-name>` and report the output.
6. **Check** with `agora status`. It should show the peer name and no networks.
7. **Explain the next step** to the user: `agora open` on one machine, `agora join <network-id>` on the other, then `agora approve <name>` on the first after comparing the four words shown on both screens.
