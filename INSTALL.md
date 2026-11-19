# Installation guide for Claude Code

This file is written for a Claude Code agent that a user asked to install agora-mesh. Follow the steps in order and show the user what you do.

> **No release has been published yet.** Until the repository has a release, the launcher has nothing to download: step 5 fails with "download failed". If that happens, tell the user that agora-mesh has to be built from source for now (`cargo build --release`, then set `AGORA_BIN` to `target/release/agora`), and stop.

Installing the plugin adds the `agora` command, a skill, a hook that starts the daemon when a session starts, and a monitor that tells the session about events. The first call of `agora` downloads the binary for this platform from the GitHub release `v<plugin version>` and checks it against `SHA256SUMS`. That needs `curl` or `wget`, and `sha256sum` or `shasum`.

## Rules for the installing agent

- Run only the commands listed here. If a step fails, stop and show the user the error.
- Never confirm the security warning yourself. The user has to read and confirm it.
- Do not write to the global `CLAUDE.md`. agora-mesh ships its instructions as a skill.
- Do not connect to any network or approve any peer on your own initiative.

## Steps

1. **Check the platform.** Tested: macOS on x64 and Linux on x64. Built but untested: macOS and Linux on ARM64, Linux with musl, Windows on x64. On Windows, Git for Windows must be installed, and the launcher `bin/agora.cmd` has never been run; say so to the user.
2. **Install the plugin.**
   ```
   claude plugin install agora-mesh --marketplace kn0sti/agora-mesh
   ```
   Tell the user to start a new session afterwards, so that the command, the hook and the monitor load.
3. **Show the security warning** from the "Security" section of `README.md` to the user in full and ask them to confirm it in their own words.
4. **Ask the user** for a name for this machine, for example `MacBook` or `Heimserver`. One word, no spaces.
5. **Set up** with `agora setup --name <name>` and report the output. This first call downloads the binary; if the download or the checksum check fails, stop and show the error.
6. **Check** with `agora status`. It should show the name and "no networks". The daemon now runs; it starts again with every Claude Code session and with every `agora` command, but not when the machine boots.
7. **Explain the next step** to the user: `agora open` on one machine, `agora join <network-id>` on the other, compare the four words that `agora status` shows on both, then `agora approve <name>` on the first.
