//! Rooms: one topic, one fresh Claude session per participating machine.
//!
//! The opener's daemon numbers the messages and says who speaks next. That is
//! ordering, not authority: after the briefing all participants take equal turns.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, watch};

use crate::daemon::Shared;
use crate::proto::{self, KINDS, Member, Msg, Participant};
use crate::state::home;

const TURN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const JOIN_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a room waits for a human after NEED_HUMAN before it closes.
const PAUSE_TIMEOUT: Duration = Duration::from_secs(60 * 60);

pub enum RoomEvent {
    Remote(String, Msg),
    /// A human's message and the number of turns it grants every peer.
    Human(String, u32),
    Panic,
}

/// What `agora discuss` and `agora wait` wait for.
#[derive(Clone)]
pub enum RoomState {
    Running,
    /// A peer sent NEED_HUMAN; holds the question.
    Paused(String),
    Closed(Result<String, String>),
}

pub struct RoomHandle {
    pub topic: String,
    pub role: &'static str,
    pub tx: mpsc::Sender<RoomEvent>,
    /// Only rooms opened on this machine report their state here.
    pub state: Option<watch::Receiver<RoomState>>,
}

/// An invitation that waits for `agora accept`.
#[derive(Clone)]
pub struct Invite {
    pub room_id: String,
    pub topic: String,
    pub from: String,
    pub turn_budget: u32,
    pub participants: Vec<Participant>,
    pub rights: String,
}

#[derive(Clone)]
struct Entry {
    seq: u32,
    from: String,
    from_name: String,
    kind: String,
    text: String,
    turns_left: u32,
}

fn render(e: &Entry) -> String {
    if e.kind == "HUMAN" || e.kind == "BRIEF" || e.kind == "KNOWN_MISSING" {
        format!("[#{} {} · {}]\n{}\n", e.seq, e.from_name, e.kind, e.text)
    } else {
        format!("[#{} {} · {} · {} left]\n{}\n", e.seq, e.from_name, e.kind, e.turns_left, e.text)
    }
}

pub fn room_dir(room_id: &str) -> PathBuf {
    home().join("rooms").join(room_id)
}

fn append(dir: &Path, file: &str, text: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(file)) {
        let _ = writeln!(f, "{text}");
    }
}

// ------------------------------------------------------------------ session

/// A headless Claude Code process that is fed one user message per turn.
struct Session {
    _child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    turns: u32,
    tokens_in: u64,
    tokens_out: u64,
}

impl Session {
    fn start(dir: &Path, me: &str, others: &str, rights: &str) -> Result<Session> {
        let rules = format!(
            "You are \"{me}\", one of several equal peers in an agora-mesh room. The other peers ({others}) are \
Claude Code agents on other machines with their own files and knowledge; a human may read along. \
Think for yourself: question weak points and do not agree out of politeness. \
Reply format: the first line is exactly one of PROPOSAL, CRITIQUE, QUESTION, ANSWER, SPLIT_WORK, PASS, CONSENSUS, NEED_HUMAN; \
your message follows on the next lines. Keep it short. Refer to earlier messages as #number instead of repeating them. \
Send CONSENSUS only when you accept the latest proposal as it stands. Send NEED_HUMAN when a human has to decide. \
Send PASS when you have nothing to add. \
A room starts with a short briefing; during it you are told which first line to use instead, and nobody proposes or decides anything yet."
        );
        let mut cmd = Command::new(std::env::var("AGORA_CLAUDE").unwrap_or_else(|_| "claude".into()));
        cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]);
        // Connectors cost far more context than the whole discussion; see ARCHITECTURE.md section 7.
        cmd.arg("--strict-mcp-config");
        cmd.args(["--append-system-prompt", &rules]);
        match rights {
            "talk" => {
                cmd.args(["--tools", ""]);
            }
            "read" => {
                cmd.args(["--tools", "Read,Grep,Glob"]);
            }
            _ => {
                let mode = std::env::var("AGORA_PERMISSION_MODE").unwrap_or_else(|_| "auto".into());
                cmd.args(["--permission-mode", &mode]);
            }
        }
        if let Ok(extra) = std::env::var("AGORA_CLAUDE_ARGS") {
            cmd.args(extra.split_whitespace());
        }
        let mut child = cmd
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow!("could not start claude: {e}"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        Ok(Session { _child: child, stdin, lines: BufReader::new(stdout).lines(), turns: 0, tokens_in: 0, tokens_out: 0 })
    }

    /// Sends one user message and waits for the turn's final answer.
    async fn turn(&mut self, text: &str) -> Result<String> {
        let msg = serde_json::json!({"type": "user", "message": {"role": "user", "content": text}});
        self.stdin.write_all(format!("{msg}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        let answer = tokio::time::timeout(TURN_TIMEOUT, async {
            while let Some(line) = self.lines.next_line().await? {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if v["type"] != "result" {
                    continue;
                }
                let u = &v["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                self.tokens_in += n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
                self.tokens_out += n("output_tokens");
                self.turns += 1;
                let text = v["result"].as_str().unwrap_or("").to_string();
                if v["is_error"].as_bool().unwrap_or(false) {
                    bail!("the Claude session reported an error: {text}");
                }
                return Ok(text);
            }
            bail!("the Claude session ended unexpectedly")
        })
        .await
        .map_err(|_| anyhow!("the Claude session did not answer within {} minutes", TURN_TIMEOUT.as_secs() / 60))??;
        Ok(answer)
    }

    fn tokens(&self) -> u64 {
        self.tokens_in + self.tokens_out
    }

    fn stats(&self) -> String {
        let per_turn = if self.turns > 0 { self.tokens_in / self.turns as u64 } else { 0 };
        format!("{} turns, {} input tokens ({} per turn), {} output tokens", self.turns, self.tokens_in, per_turn, self.tokens_out)
    }
}

// ------------------------------------------------------------------- opener

struct Opener {
    shared: Arc<Shared>,
    room_id: String,
    me: Participant,
    joiners: Vec<Participant>,
    log: Vec<Entry>,
    /// How much of the log our own session has seen.
    seen: usize,
    budget: HashMap<String, u32>,
    rx: mpsc::Receiver<RoomEvent>,
    session: Session,
    dir: PathBuf,
    /// Tokens each joiner's session reported so far.
    joiner_tokens: HashMap<String, u64>,
    token_budget: u64,
    state: watch::Sender<RoomState>,
}

/// Runs a room from invitation to result. Returns the text for the human who asked.
pub async fn run_opener(
    shared: Arc<Shared>,
    room_id: String,
    topic: String,
    joiners: Vec<Member>,
    turns: u32,
    token_budget: u64,
    rx: mpsc::Receiver<RoomEvent>,
    state: watch::Sender<RoomState>,
) -> Result<String> {
    let me = Participant { id: shared.me.clone(), name: shared.state.lock().unwrap().name.clone() };
    let joiners: Vec<Participant> = joiners.iter().map(|m| Participant { id: m.id.clone(), name: m.name.clone() }).collect();
    let dir = room_dir(&room_id);
    std::fs::create_dir_all(&dir)?;
    append(&dir, "log.md", &format!("# Room {room_id}\n\nTopic: {topic}\n"));

    let mut participants = vec![me.clone()];
    participants.extend(joiners.iter().cloned());
    for j in &joiners {
        let invite = Msg::RoomInvite { room_id: room_id.clone(), topic: topic.clone(), turn_budget: turns, token_budget, participants: participants.clone() };
        if !shared.send_to(&j.id, invite) {
            bail!("peer \"{}\" is offline", j.name);
        }
    }

    let others = joiners.iter().map(|j| j.name.as_str()).collect::<Vec<_>>().join(", ");
    let session = Session::start(&dir, &me.name, &others, "full")?;
    let budget = participants.iter().map(|p| (p.id.clone(), turns)).collect();
    let mut room = Opener { shared, room_id, me, joiners, log: vec![], seen: 0, budget, rx, session, dir, joiner_tokens: HashMap::new(), token_budget, state };

    let outcome = room.run(&topic).await;
    let (reason, result) = match &outcome {
        Ok((reason, result)) => (reason.clone(), result.clone()),
        Err(e) => ("error".to_string(), e.to_string()),
    };
    for j in &room.joiners {
        room.shared.send_to(&j.id, Msg::RoomClose { room_id: room.room_id.clone(), reason: reason.clone(), result: result.clone() });
    }
    let stats = room.session.stats();
    append(&room.dir, "log.md", &format!("\n---\nClosed: {reason}. This peer: {stats}."));
    std::fs::write(room.dir.join("result.md"), &result)?;
    let total = room.total_tokens();
    let closing = outcome.map(|(reason, result)| format!("{result}\n\n[room {} closed: {reason}; {} messages; this peer: {stats}; all peers: {total} tokens]", room.room_id, room.log.len()))?;
    // What `agora wait` prints when it is asked after the room has closed.
    std::fs::write(room.dir.join("closing.md"), &closing)?;
    Ok(closing)
}

impl Opener {
    async fn run(&mut self, topic: &str) -> Result<(String, String)> {
        self.wait_for_joins().await?;

        // Briefing: the opener states what is known, each joiner says what it lacks,
        // the opener fills the gaps. None of this counts against the budget.
        let names = self.joiners.iter().map(|j| j.name.clone()).collect::<Vec<_>>().join(", ");
        let brief = self
            .session
            .turn(&format!(
                "You opened this room. Topic: {topic}\n\nWrite the briefing for {names}: the goal, what is already known, \
and where relevant information can be found. This one message needs no kind on the first line."
            ))
            .await?;
        let first = self.joiners[0].id.clone();
        self.publish(&self.me.clone(), "BRIEF", &brief, Some(first));
        for i in 0..self.joiners.len() {
            let j = self.joiners[i].clone();
            let (_, text) = self.wait_say(&j.id).await?;
            let next = self.joiners.get(i + 1).map(|n| n.id.clone()).unwrap_or_else(|| self.me.id.clone());
            self.publish(&j, "KNOWN_MISSING", &text, Some(next));
        }
        let fill = self.own_turn("Briefing, last step: fill the gaps the others named, as far as you can. First line: ANSWER. Do not propose or decide anything yet; the discussion starts after this message.").await?;
        let (_, text) = proto::parse_kind(&fill, "ANSWER", &["ANSWER"]);
        let first = self.joiners[0].id.clone();
        self.publish(&self.me.clone(), "ANSWER", &text, Some(first));

        // Discussion: everyone takes equal turns.
        let mut order: Vec<Participant> = self.joiners.clone();
        order.push(self.me.clone());
        let mut last_kind: HashMap<String, String> = HashMap::new();
        let mut idx = 0;
        let reason = loop {
            if order.iter().all(|p| self.budget[&p.id] == 0) {
                break "turn budget used up";
            }
            let speaker = order[idx % order.len()].clone();
            idx += 1;
            if self.budget[&speaker.id] == 0 {
                continue;
            }
            let (kind, text) = if speaker.id == self.me.id {
                let left = self.budget[&speaker.id];
                let reply = self.own_turn(&format!("Your turn ({left} turns left).")).await?;
                proto::parse_kind(&reply, "PROPOSAL", &KINDS)
            } else {
                self.wait_say(&speaker.id).await?
            };
            *self.budget.get_mut(&speaker.id).unwrap() -= 1;
            let next = self.next_speaker(&order, idx);
            last_kind.insert(speaker.id.clone(), kind.clone());
            let mut done = if kind == "NEED_HUMAN" {
                Some("a human has to decide")
            } else if order.iter().all(|p| last_kind.get(&p.id).is_some_and(|k| k == "CONSENSUS")) {
                Some("consensus")
            } else if order.iter().all(|p| last_kind.get(&p.id).is_some_and(|k| k == "PASS" || k == "CONSENSUS")) {
                Some("nothing left to add")
            } else {
                None
            };
            if done.is_none() && self.total_tokens() > self.token_budget {
                done = Some("token budget used up");
            }
            self.publish(&speaker, &kind, &text, if done.is_some() { None } else { next });
            // NEED_HUMAN pauses the room. It goes on when a human answers and
            // closes with an overview when nobody does.
            if kind == "NEED_HUMAN" && self.wait_human(&speaker, &text, &order, idx).await? {
                continue;
            }
            if let Some(reason) = done {
                break reason;
            }
        };

        let ask = if reason == "consensus" {
            "The room reached consensus. Write the agreed result for the human who asked: complete and usable on its own. No kind on the first line."
        } else {
            "The room closes without consensus. Write an overview for the human who asked with three parts: agreed, disputed, and what the human has to decide. No kind on the first line."
        };
        let result = self.own_turn(ask).await?;
        Ok((reason.to_string(), result))
    }

    fn total_tokens(&self) -> u64 {
        self.session.tokens() + self.joiner_tokens.values().sum::<u64>()
    }

    /// The first peer from position `idx` on that still has turns.
    fn next_speaker(&self, order: &[Participant], idx: usize) -> Option<String> {
        (0..order.len()).map(|k| &order[(idx + k) % order.len()]).find(|p| self.budget[&p.id] > 0).map(|p| p.id.clone())
    }

    fn grant(&mut self, turns: u32) {
        for left in self.budget.values_mut() {
            *left += turns;
        }
    }

    /// Waits for a human after NEED_HUMAN. True if one answered and the room goes on.
    async fn wait_human(&mut self, asker: &Participant, question: &str, order: &[Participant], idx: usize) -> Result<bool> {
        let _ = self.state.send(RoomState::Paused(format!("{} asks: {question}", asker.name)));
        loop {
            let ev = match tokio::time::timeout(PAUSE_TIMEOUT, self.rx.recv()).await {
                Ok(ev) => ev,
                Err(_) => {
                    let _ = self.state.send(RoomState::Running);
                    return Ok(false);
                }
            };
            let (who, text, extra) = match ev {
                Some(RoomEvent::Human(text, extra)) => (self.me.clone(), text, extra),
                Some(RoomEvent::Remote(p, Msg::Say { kind, text, extra_turns, .. })) if kind == "HUMAN" => {
                    match self.joiners.iter().find(|j| j.id == p).cloned() {
                        Some(j) => (j, text, extra_turns),
                        None => continue,
                    }
                }
                Some(other) => {
                    self.side_event(other)?;
                    continue;
                }
                None => bail!("room stopped"),
            };
            self.grant(extra);
            let next = self.next_speaker(order, idx);
            self.publish(&who, "HUMAN", &text, next);
            let _ = self.state.send(RoomState::Running);
            return Ok(true);
        }
    }

    async fn wait_for_joins(&mut self) -> Result<()> {
        let mut missing: Vec<String> = self.joiners.iter().map(|j| j.id.clone()).collect();
        while !missing.is_empty() {
            let ev = tokio::time::timeout(JOIN_TIMEOUT, self.rx.recv()).await.map_err(|_| anyhow!("the invited peers did not join within {}s; they may need to run `agora accept {}`", JOIN_TIMEOUT.as_secs(), self.room_id))?;
            match ev {
                Some(RoomEvent::Remote(p, Msg::RoomJoin { .. })) => missing.retain(|m| *m != p),
                Some(RoomEvent::Remote(p, Msg::RoomDecline { reason, .. })) if missing.contains(&p) => bail!("a peer declined the room: {reason}"),
                Some(RoomEvent::Panic) | None => bail!("room stopped"),
                _ => {}
            }
        }
        Ok(())
    }

    /// Numbers a contribution, records it and sends it to all joiners.
    fn publish(&mut self, from: &Participant, kind: &str, text: &str, next: Option<String>) {
        let from_name = if kind == "HUMAN" { format!("{} (human)", from.name) } else { from.name.clone() };
        let e = Entry {
            seq: self.log.len() as u32 + 1,
            from: from.id.clone(),
            from_name,
            kind: kind.to_string(),
            text: text.to_string(),
            turns_left: self.budget.get(&from.id).copied().unwrap_or(0),
        };
        append(&self.dir, "log.md", &render(&e));
        for j in &self.joiners {
            self.shared.send_to(
                &j.id,
                Msg::Discuss {
                    room_id: self.room_id.clone(),
                    seq: e.seq,
                    from: e.from.clone(),
                    from_name: e.from_name.clone(),
                    kind: e.kind.clone(),
                    text: e.text.clone(),
                    turns_left: e.turns_left,
                    next: next.clone(),
                },
            );
        }
        self.log.push(e);
    }

    /// Takes a turn with our own session: new messages since its last turn, then `ask`.
    async fn own_turn(&mut self, ask: &str) -> Result<String> {
        self.drain()?;
        let news: String = self.log[self.seen..].iter().filter(|e| e.from != self.me.id || e.kind == "HUMAN").map(render).collect::<Vec<_>>().join("\n");
        self.seen = self.log.len();
        let reply = self.session.turn(&format!("{news}\n{ask}")).await?;
        // Our own answer becomes the next log entry; the session already knows it.
        self.seen = self.log.len() + 1;
        Ok(reply)
    }

    /// Handles events that arrived while our session was busy.
    fn drain(&mut self) -> Result<()> {
        while let Ok(ev) = self.rx.try_recv() {
            self.side_event(ev)?;
        }
        Ok(())
    }

    /// Events that do not belong to the current speaker: humans joining in, stop.
    fn side_event(&mut self, ev: RoomEvent) -> Result<()> {
        match ev {
            RoomEvent::Human(text, extra) => {
                let me = self.me.clone();
                self.grant(extra);
                self.publish_human(&me, &text);
            }
            RoomEvent::Remote(p, Msg::Say { kind, text, extra_turns, .. }) if kind == "HUMAN" => {
                if let Some(j) = self.joiners.iter().find(|j| j.id == p).cloned() {
                    self.grant(extra_turns);
                    self.publish_human(&j, &text);
                }
            }
            RoomEvent::Remote(p, Msg::RoomDecline { reason, .. }) if self.joiners.iter().any(|j| j.id == p) => bail!("a peer left the room: {reason}"),
            RoomEvent::Panic => bail!("room stopped by `agora panic`"),
            _ => {}
        }
        Ok(())
    }

    fn publish_human(&mut self, from: &Participant, text: &str) {
        // A human's message does not change whose turn it is, so it names no next speaker.
        self.publish(from, "HUMAN", text, None);
    }

    async fn wait_say(&mut self, speaker: &str) -> Result<(String, String)> {
        loop {
            let ev = tokio::time::timeout(TURN_TIMEOUT, self.rx.recv()).await.map_err(|_| anyhow!("a peer did not answer within {} minutes", TURN_TIMEOUT.as_secs() / 60))?;
            match ev {
                Some(RoomEvent::Remote(p, Msg::Say { kind, text, tokens, .. })) if p == speaker && kind != "HUMAN" => {
                    self.joiner_tokens.insert(p, tokens);
                    return Ok((kind, text));
                }
                Some(ev) => self.side_event(ev)?,
                None => bail!("room stopped"),
            }
        }
    }
}

// ------------------------------------------------------------------- joiner

/// Takes part in a room another peer opened.
pub async fn run_joiner(shared: Arc<Shared>, invite: Invite, mut rx: mpsc::Receiver<RoomEvent>) -> Result<()> {
    let me = shared.me.clone();
    let my_name = shared.state.lock().unwrap().name.clone();
    let dir = room_dir(&invite.room_id);
    std::fs::create_dir_all(&dir)?;
    append(&dir, "log.md", &format!("# Room {}\n\nTopic: {}\n", invite.room_id, invite.topic));
    let others = invite.participants.iter().filter(|p| p.id != me).map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
    let mut session = match Session::start(&dir, &my_name, &others, &invite.rights) {
        Ok(s) => s,
        Err(e) => {
            shared.send_to(&invite.from, Msg::RoomDecline { room_id: invite.room_id.clone(), reason: e.to_string() });
            return Err(e);
        }
    };
    shared.send_to(&invite.from, Msg::RoomJoin { room_id: invite.room_id.clone() });

    let mut news: Vec<Entry> = vec![];
    let mut my_left = invite.turn_budget;
    let outcome: Result<String> = loop {
        match rx.recv().await {
            Some(RoomEvent::Remote(p, Msg::Discuss { seq, from, from_name, kind, text, turns_left, next, .. })) if p == invite.from => {
                let e = Entry { seq, from, from_name, kind, text, turns_left };
                append(&dir, "log.md", &render(&e));
                if e.from == me && e.kind != "HUMAN" {
                    my_left = e.turns_left;
                } else {
                    news.push(e);
                }
                if next.as_deref() != Some(me.as_str()) || news.is_empty() {
                    continue;
                }
                let briefing = news.iter().any(|e| e.kind == "BRIEF");
                let ask = if briefing {
                    format!("Topic: {}\n\nBriefing: your turn. First line: KNOWN_MISSING. Say briefly what you already know about this and what you are missing to take part. Do not propose anything yet; the discussion starts after the opener has answered.", invite.topic)
                } else {
                    format!("Your turn ({my_left} turns left).")
                };
                let prompt = format!("{}\n{ask}", news.iter().map(render).collect::<Vec<_>>().join("\n"));
                news.clear();
                let reply = match session.turn(&prompt).await {
                    Ok(r) => r,
                    Err(e) => {
                        shared.send_to(&invite.from, Msg::RoomDecline { room_id: invite.room_id.clone(), reason: e.to_string() });
                        break Err(e);
                    }
                };
                let (kind, text) = if briefing { proto::parse_kind(&reply, "KNOWN_MISSING", &["KNOWN_MISSING"]) } else { proto::parse_kind(&reply, "PROPOSAL", &KINDS) };
                shared.send_to(&invite.from, Msg::Say { room_id: invite.room_id.clone(), kind, text, tokens: session.tokens(), extra_turns: 0 });
            }
            Some(RoomEvent::Remote(p, Msg::RoomClose { reason, result, .. })) if p == invite.from => {
                std::fs::write(dir.join("result.md"), &result)?;
                break Ok(reason);
            }
            Some(RoomEvent::Human(text, extra_turns)) => {
                shared.send_to(&invite.from, Msg::Say { room_id: invite.room_id.clone(), kind: "HUMAN".into(), text, tokens: session.tokens(), extra_turns });
            }
            Some(RoomEvent::Panic) | None => {
                shared.send_to(&invite.from, Msg::RoomDecline { room_id: invite.room_id.clone(), reason: "stopped on the peer's machine".into() });
                break Ok("stopped".into());
            }
            _ => {}
        }
    };
    let reason = match &outcome {
        Ok(r) => r.clone(),
        Err(e) => format!("error: {e}"),
    };
    append(&dir, "log.md", &format!("\n---\nClosed: {reason}. This peer: {}.", session.stats()));
    outcome.map(|_| ())
}
