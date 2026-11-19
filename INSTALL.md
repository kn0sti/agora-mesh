# Installation guide for Claude Code

This file is written for a Claude Code agent that a user asked to install agora-mesh. Follow the steps in order and show the user what you do.

This installs the plugin. The first call of `agora` downloads the binary for this platform from the GitHub release `v<plugin version>` and checks it against `SHA256SUMS`. Needs `curl` or `wget` and `sha256sum` or `shasum` (macOS has `shasum`).

## Rules for the installing agent

- Run only the commands listed here. If a step fails, stop and show the user the error.
- Never confirm the security warning yourself. The user has to read and confirm it.
- Do not write to the global `CLAUDE.md`. agora-mesh ships its instructions as a skill.
- Do not connect to any network or approve any peer on your own initiative.

## Steps

1. **Check the platform.** Supported: macOS 13+, Windows 10 1809+, Ubuntu 20.04+, Debian 10+, Alpine 3.19+; x64 or ARM64. On Windows, Git for Windows must be installed; the Windows launcher (`bin/agora.cmd`) is best-effort and untested.
2. **Install the plugin.**
   ```
   claude plugin marketplace add kn0sti/agora-mesh
   claude plugin install agora-mesh@agora-mesh
   ```
   If the user asked for the one-liner form, `claude plugin install agora-mesh --marketplace kn0sti/agora-mesh` is equivalent. The plugin adds `agora` to the PATH of the Bash tool, a SessionStart hook that starts the daemon on a set-up machine, and a monitor that shows `agora events --follow`. Start a new session afterwards so these load.
3. **Show the security warning** from the "Security" section of `README.md` to the user in full and ask them to confirm it in their own words.
4. **Ask the user** for:
   - a peer name for this machine, for example `MacBook` or `Heimserver`;
   - whether this machine should run the daemon permanently as a system service (recommended for machines that are always on) or only while a session is open.
5. **Set up** with `agora setup --name <peer-name>` and report the output. The first call downloads the binary; if the download or checksum check fails, stop and show the error. The daemon starts by itself on the next call and at every session start. Developers can set `AGORA_BIN` to a local `cargo build` binary to skip the download.
6. **Check** with `agora status`. It should show the peer name and no networks.
7. **Explain the next step** to the user: `agora open` on one machine, `agora join <network-id>` on the other, then `agora approve <name>` on the first after comparing the four words shown on both screens.
