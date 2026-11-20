# Design: resuming a room after the opener's daemon restarts

Status: proposed, not built. This text is the unedited result of an agora-mesh room: three Claude Sonnet sessions on two machines discussed the question and agreed on it. Line numbers and section numbers refer to the source at the time and no longer fit: since then ARCHITECTURE.md was rewritten (it has 12 sections, and the "Restarted opener" paragraph the last step mentions is in section 5), and `Session` moved from `room.rs` to `src/session.rs`. How the room ran and what it cost is in [docs/measurements/2026-11-room-with-three-sessions.md](../measurements/2026-11-room-with-three-sessions.md).

---

**Design: resuming a room after the opener's daemon restarts**

Today a restart loses the room. It lives only in memory, and `daemon.rs` (~597) answers joiners with `RoomClose` or `RoomDecline`. All three peers agreed on the design below.

1. **Snapshot (`room.rs`).** Add `struct Snapshot` (serde) and `Opener::save()`, which writes `room.json.tmp` and then renames it to `~/.agora/rooms/<id>/room.json`.
   - Contents: the `Opener` fields (`log`, `budget`, `joiner_tokens`, `split`, `plan`, `assigned`, `said`, `invited`, `last_next`, `Pkg` list with serde derives) and the stack locals of `Opener::run`.
   - Those locals (`order`, `idx`, `last_kind`, `previous`, `mode`) move into a struct `Loop`, so the snapshot and the running room read one state.
   - Also saved: `phase` (`Wait|Brief|Discuss|Work|Paused|Closed`), the next speaker, joiner ids, `work_dir` and the session ids.
2. **When to write.** `save()` is called at the end of `publish` (room.rs:966), on changes to `assigned`, `Pkg` or phase, and after every `Session::turn`.
   - The snapshot never holds a half-finished answer; the `session_id` goes in only after the `result` event.
   - `run_opener` sets `phase = Closed` when it writes `result.md`.
3. **Session continuation.** `Session::turn` (room.rs:254) reads `session_id` from the `result` event into a new `Session.id` field. A new `Session::resume(id, dir, …)` next to `Session::start` (room.rs:202) adds `--resume <id>` to the same arguments.
   - If the first turn fails, or `work_dir` differs from the saved one, it falls back to `Session::start` with `catch_up()` (room.rs:838) in the first prompt.
4. **Restore at startup.** `room::restore_all(shared)` is called in `daemon::run` (daemon.rs:270) after the state is loaded and before the first link.
   - It scans `rooms/*/room.json` with `phase != Closed`. For each room it mirrors the opener path at daemon.rs:1151: create the `RoomHandle` and watch, call `announce_rooms()`, and spawn `run_opener_restored(snapshot, rx, state_tx)`.
   - That function skips invitations and `wait_for_joins`, resumes the sessions, and enters `run` at the saved phase and speaker.
5. **Interrupted work.**
   - The turn that was running at the crash is run again. For the opener's own turn, resume the session and re-send the prompt.
   - A `Running` package becomes `Blocked` ("interrupted by restart"), and the opener's next turn decides about reassignment. Automatic reassignment is avoided because the joiner may already have committed part of it.
   - `Paused` restores as `Paused` with its question, and the 1-hour `PAUSE_TIMEOUT` starts again.
6. **How peers find the room.** No new messages are needed.
   - Once the room is back in `shared.rooms`, the existing `known` path in daemon.rs applies. A joiner's `LinkUp` (room.rs:1285) sends `Resend{last_seq+1}` and the opener answers from `log` (room.rs:1070).
   - The opener's `LinkUp` (room.rs:1078) sends `RoomPing`. A joiner that answers with `RoomJoin` or `Resend` counts as alive again, so the 15-minute lost-peers rule does not drop it.
   - `agora wait` works through the restored `RoomHandle` watch (daemon.rs ~1050).
7. **Failure and limits.**
   - A missing or corrupt snapshot gives today's behavior (`RoomClose` or `RoomDecline`) and a log line.
   - A joiner that already got `RoomDecline` before `restore_all` ran stays out, which is why `restore_all` runs first.
   - A joiner waits at most `JOINER_IDLE` (3 h, room.rs:25) for the opener.
   - Joiner restarts remain out of scope and are handled by the existing lost-peers rule.
8. **Before the build.** One real probe is required. It tests these four things:
   - `--resume` together with `--input-format stream-json`, `--append-system-prompt` and a different cwd;
   - whether the system prompt is doubled after a resume;
   - whether a new session id appears after a resume;
   - whether the stream-json `result` line carries `session_id`.
   My earlier test covered only plain `-p --output-format json` with the same cwd, and there the resume kept the context. If the probe fails, use `--session-id <uuid>` at `Session::start` plus the `catch_up()` fallback.
9. **Docs.** After the build, replace the "Restarted opener" paragraph in ARCHITECTURE.md §13.
