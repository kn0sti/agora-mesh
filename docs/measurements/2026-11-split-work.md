# Measurement: two agents plan, split and build stage 5

November 2026. One run of one task, and one run of the same task by a single agent. The two runs did not do the same amount of checking, so the times are not a speed-up figure; read the section "What the two runs are not" before quoting any number.

## Setup

| | |
|---|---|
| Peers | `Mac` (macOS 13, Intel) opens the room; `Heimserver` (Linux container on another machine) joins |
| Model | Claude Sonnet (`--model sonnet`) on both |
| Sessions | started by the daemon without connectors, built-in tools on, permission mode `auto` |
| Link | direct, about 12 ms round trip, same local network |
| Project | each peer worked in its own clone of this repository and pushed its own branch |
| Command | `agora discuss "<task>" --with Heimserver --turns 4` |

The task was stage 5 of this project's roadmap, in two parts: (A) GitHub Actions workflows that build and release the binary for seven targets and run the tests, (B) the Claude Code plugin wrapper with manifest, marketplace file, a launcher that downloads the binary and checks it against `SHA256SUMS`, a hook, a monitor, and a rewritten `INSTALL.md`. The agents were asked to agree on the interface between the parts first, then to split the work and do it, each on its own branch.

## What happened

| Seconds | Message | |
|---|---|---|
| 0 to 28 | 1 to 3 | briefing |
| 28 to 49 | 4 to 9 | discussion: question, proposal, the split, consensus |
| 49 | 10 | work phase starts, both peers at once |
| 71 | 11 | Mac reports part A done (22 s) |
| 100 | 12 | Heimserver reports part B done (51 s) |
| 108 | | the opener's summary; room closes |

The work phase took 51 s. One after the other, the two packages would have taken 73 s. The agents agreed on asset names, checksum format and version scheme before they split, and the two branches touched no common file and merged without a conflict.

| Peer | Turns | Input tokens | Output tokens | Model time | Delivered |
|---|---|---|---|---|---|
| Mac | 7 | 373,117 | 8,576 | 65 s | `ci.yml`, `release.yml`: 98 lines |
| Heimserver | 5 | 499,889 | 8,298 | 64 s | plugin wrapper: 7 files, 121 lines |
| Together | 12 | 873,006 | 16,874 | | 889,880 tokens in total |

From the task to two pushed branches: 108 seconds.

## The same task, one agent

One Sonnet session got both parts in a fresh clone, started the same way.

| | Room | One agent |
|---|---|---|
| Wall time | 108 s | 1,163 s |
| Tokens in total | 889,880 | 1,859,692 |
| Model steps | 12 turns | 34 |
| Result | 11 files, 219 lines, two branches | 11 files, 279 lines, one branch |
| Checks the agents ran | shellcheck, JSON parse, a dry run of the launcher | `claude plugin validate`, launcher against a local server, the hook, the monitor, all end-to-end test scripts |
| Corrections a reviewer made afterwards | 4 | not reviewed in the same depth |

## What the two runs are not

They are not the same work done at two speeds. The single agent spent most of its 19 minutes checking: it ran the plugin validator and the project's end-to-end tests, which take minutes by themselves, and so avoided three of the four problems below. The room's agents reported after far less checking.

What a reviewer had to correct in the room's result:

1. The release workflow used a `macos-13` runner, which GitHub no longer offers.
2. The CI workflow passed a relative path to test scripts that change directory.
3. The plugin manifest declared the monitor at the top level instead of under `experimental`.
4. The launcher stored the binary in a different place depending on who called it.

The agents also put the plugin part on the machine that had no plugin examples to look at, although the task said where those were, so the monitor file's format was a guess. It turned out right.

What the run does show:

- Two agents on two machines can take a task from a paragraph of text to an agreed interface, a split, and two non-overlapping branches in under two minutes.
- The split bought 22 s of 73 s in the work phase. The gain grows with the size of the packages; the fixed cost of the room is the minute of briefing and discussion.
- Speed came with less verification. A room's result still needs a review. The work instruction now tells every session to run the checks that exist for what it changed before it reports, and to say which ones ran.

After the review fixes, `claude plugin validate` passes, the launcher passes eight checks against a local stand-in for a release (download, reuse, tampered binary refused, missing checksum refused), and a real Claude Code session with the plugin loaded finds `agora` on its path and the skill `agora-mesh:agora`. The workflows have not run on GitHub yet.
