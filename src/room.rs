//! Rooms: one topic, one fresh Claude session per participating machine.
//!
//! The opener's daemon numbers the messages and says who speaks next. That is
//! ordering, not authority: after the briefing all participants take equal turns.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, watch};

use crate::daemon::Shared;
use crate::project;
use crate::proto::{self, Capacity, KINDS, Member, Msg, Participant};
use crate::state::home;

const TURN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const JOIN_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a joiner keeps a room without hearing from the opener.
const JOINER_IDLE: Duration = Duration::from_secs(3 * 60 * 60);
/// How long one work package may take.
const WORK_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How long a room waits for a human after NEED_HUMAN before it closes.
const PAUSE_TIMEOUT: Duration = Duration::from_secs(60 * 60);

pub enum RoomEvent {
    Remote(String, Msg),
    /// A human's message and the number of turns it grants every peer.
    Human(String, u32),
    /// A human invites another peer into the running room.
    Invite(Member),
    /// The link to this peer came up (again).
    LinkUp(String),
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
    pub project: Option<String>,
    pub mode: String,
    pub catch_up: Option<String>,
    pub seq: u32,
    pub lean: bool,
}

#[derive(Clone)]
struct Entry {
    seq: u32,
    from: String,
    from_name: String,
    kind: String,
    text: String,
    turns_left: u32,
    /// Who speaks after this message, if it says so.
    next: Option<String>,
}

fn discuss_msg(room_id: &str, e: &Entry) -> Msg {
    Msg::Discuss {
        room_id: room_id.to_string(),
        seq: e.seq,
        from: e.from.clone(),
        from_name: e.from_name.clone(),
        kind: e.kind.clone(),
        text: e.text.clone(),
        turns_left: e.turns_left,
        next: e.next.clone(),
    }
}

fn render(e: &Entry) -> String {
    if matches!(e.kind.as_str(), "HUMAN" | "BRIEF" | "KNOWN_MISSING" | "WORK" | "DONE" | "BLOCKED" | "HANDOVER" | "LIMIT" | "LEFT" | "JOIN" | "TURN") {
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
    /// Input tokens of the most recent turn.
    last_in: u64,
    /// What the most recent turn reported about the usage limit.
    capacity: Option<Capacity>,
    timeout: Duration,
    /// Time spent waiting for the model.
    busy: Duration,
}

/// The session's turn failed because the account is at its usage limit.
#[derive(Debug)]
struct LimitReached;

impl std::fmt::Display for LimitReached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "usage limit reached")
    }
}

impl std::error::Error for LimitReached {}

fn is_limit(e: &anyhow::Error) -> bool {
    e.downcast_ref::<LimitReached>().is_some()
}

/// Whether an error result means the usage limit. The exact wording of a real
/// limit error is not verified; status 429 and the usual phrases are accepted.
fn is_limit_error(v: &serde_json::Value, text: &str) -> bool {
    let t = text.to_lowercase();
    v["api_error_status"].as_u64() == Some(429) || ["usage limit", "rate limit", "limit reached", "hit your limit"].iter().any(|p| t.contains(p))
}

/// One line per high-importance memory entry the session has not been told about.
fn memory_notes(shared: &Shared, seen: &mut usize) -> String {
    shared.notices_since(seen).iter().map(|n| format!("[shared memory, new] {n}\n")).collect()
}

/// What a session is told when it starts a work package.
fn work_prompt(package: u32, title: &str, branch: &str, taken_from: Option<&str>, has_repo: bool) -> String {
    let takeover = match taken_from {
        Some(from) => format!(" This package was started by {from}, who reached a usage limit; continue their work."),
        None => String::new(),
    };
    let how = if has_repo {
        format!(
            "Do the work now in this repository on the branch `{branch}`: fetch origin and switch to it if it exists there, \
otherwise create it from the current HEAD. Commit your work on it and push it to origin if you are allowed to."
        )
    } else {
        "This machine has no clone of a shared repository for this room, so describe your result in the reply instead of committing.".to_string()
    };
    format!(
        "Work phase. Package #{package}: {title}\n{how}{takeover}\n\
Reply with the first line DONE or BLOCKED, then at most five lines: what you did, the branch, the last commit, and what is left."
    )
}

/// `claude` in the headless mode the daemon talks to.
fn claude_command() -> Command {
    let mut cmd = Command::new(std::env::var("AGORA_CLAUDE").unwrap_or_else(|_| "claude".into()));
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]);
    // Let the session call `agora` itself, for the shared memory.
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let mut parts = vec![dir];
        parts.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        if let Ok(path) = std::env::join_paths(parts) {
            cmd.env("PATH", path);
        }
    }
    cmd
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
To divide work, send SPLIT_WORK with one line per package in the form `- <peer name>: <package>`; \
once every peer answers CONSENSUS, all peers work on their packages at the same time. \
The peers share a memory: `agora memory` lists it, `agora memory show <id>` prints an entry, and \
`agora remember \"<text>\" --title \"<title>\" --importance high|normal|low` saves a fact every peer should keep. \
A room starts with a short briefing; during it you are told which first line to use instead, and nobody proposes or decides anything yet."
        );
        let mut cmd = claude_command();
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
        Self::spawn(cmd, dir)
    }

    fn spawn(mut cmd: Command, dir: &Path) -> Result<Session> {
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
        Ok(Session { _child: child, stdin, lines: BufReader::new(stdout).lines(), turns: 0, tokens_in: 0, tokens_out: 0, last_in: 0, capacity: None, timeout: TURN_TIMEOUT, busy: Duration::ZERO })
    }

    /// Sends one user message and waits for the turn's final answer.
    async fn turn(&mut self, text: &str) -> Result<String> {
        let started = std::time::Instant::now();
        let msg = serde_json::json!({"type": "user", "message": {"role": "user", "content": text}});
        self.stdin.write_all(format!("{msg}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        let limit = self.timeout;
        let answer = tokio::time::timeout(limit, async {
            while let Some(line) = self.lines.next_line().await? {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if v["type"] == "rate_limit_event" {
                    if let Some(c) = Capacity::from_event(&v["rate_limit_info"]) {
                        self.capacity = Some(c);
                    }
                    continue;
                }
                if v["type"] != "result" {
                    continue;
                }
                let u = &v["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                self.last_in = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
                self.tokens_in += self.last_in;
                self.tokens_out += n("output_tokens");
                self.turns += 1;
                let text = v["result"].as_str().unwrap_or("").to_string();
                if v["is_error"].as_bool().unwrap_or(false) {
                    if is_limit_error(&v, &text) {
                        let resets_at = self.capacity.map(|c| c.resets_at).unwrap_or(0);
                        self.capacity = Some(Capacity { utilization: 1.0, resets_at, limited: true });
                        return Err(LimitReached.into());
                    }
                    bail!("the Claude session reported an error: {text}");
                }
                return Ok(text);
            }
            bail!("the Claude session ended unexpectedly")
        })
        .await
        .map_err(|_| anyhow!("the Claude session did not answer within {} minutes", limit.as_secs() / 60))??;
        self.busy += started.elapsed();
        Ok(answer)
    }

    fn tokens(&self) -> u64 {
        self.tokens_in + self.tokens_out
    }

    fn stats(&self) -> String {
        let per_turn = if self.turns > 0 { self.tokens_in / self.turns as u64 } else { 0 };
        format!("{} turns, {} input tokens ({} per turn), {} output tokens, {:.0} s model time", self.turns, self.tokens_in, per_turn, self.tokens_out, self.busy.as_secs_f64())
    }
}

fn write_stats(
    dir: &Path,
    room_id: &str,
    topic: &str,
    role: &str,
    reason: &str,
    messages: usize,
    session: &Session,
    all: Option<(u64, serde_json::Map<String, serde_json::Value>)>,
    took: Duration,
) {
    let closed_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut v = serde_json::json!({
        "room": room_id, "topic": topic, "role": role, "reason": reason, "messages": messages, "closed_at": closed_at,
        "turns": session.turns, "tokens_in": session.tokens_in, "tokens_out": session.tokens_out,
        "model_s": session.busy.as_secs(), "duration_s": took.as_secs(),
    });
    if let Some((total, peers)) = all {
        v["all_peers_tokens"] = total.into();
        v["peers"] = peers.into();
    }
    let _ = std::fs::write(dir.join("stats.json"), v.to_string());
}

/// Measures what one turn costs on this machine before anyone has said
/// anything: a default headless session, one without connectors, and a room
/// session. Uses real Claude sessions on the smallest model.
pub async fn bench() -> Result<String> {
    let dir = home().join("bench");
    std::fs::create_dir_all(&dir)?;
    let variants: [(&str, &[&str]); 3] = [
        ("default headless session, tools off", &["--tools", ""]),
        ("without connectors, tools off", &["--strict-mcp-config", "--tools", ""]),
        ("room session (no connectors, built-in tools)", &["--strict-mcp-config"]),
    ];
    let mut out = String::from("Input tokens of one turn that only says one word, measured on this machine:\n\n");
    for (name, args) in variants {
        let mut cmd = claude_command();
        cmd.args(["--model", "haiku", "--no-session-persistence"]);
        cmd.args(args);
        let mut s = Session::spawn(cmd, &dir)?;
        s.turn("Reply with only the word: one").await?;
        // Connectors finish loading after the first turn; the second turn shows the real cost.
        tokio::time::sleep(Duration::from_secs(10)).await;
        s.turn("Reply with only the word: two").await?;
        out.push_str(&format!("  {:>9}  {name}\n", s.last_in));
    }
    out.push_str("\nA session reads this again on every turn; after the first turn most of it comes from the cache.");
    Ok(out)
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
    /// Peers that reported a usage limit in this room.
    limited: HashSet<String>,
    /// Packages of the latest SPLIT_WORK proposal: owner id and title.
    split: Option<Vec<(String, String)>>,
    has_project: bool,
    /// How many shared-memory notices our session has been told about.
    notice_idx: usize,
    topic: String,
    turns: u32,
    project_url: Option<String>,
    /// Peers invited into the running room that have not joined yet.
    invited: Vec<Participant>,
    /// Peers that joined late and wait to be added to the speaking order.
    arrivals: Vec<Participant>,
    /// Who the last message named as the next speaker.
    last_next: Option<String>,
    /// Number of the last contribution accepted from each joiner.
    said: HashMap<String, u32>,
    /// Work a joiner has been sent and not reported on yet.
    assigned: HashMap<String, Msg>,
    lean: bool,
    /// In a lean room: the session with tools that does this machine's packages.
    work_session: Option<Session>,
    work_dir: PathBuf,
    others: String,
    /// Text of the agreed SPLIT_WORK message.
    plan: String,
}

#[derive(Clone, Copy, PartialEq)]
enum PkgState {
    Open,
    Running,
    Done,
    Blocked,
}

struct Pkg {
    id: u32,
    owner: String,
    title: String,
    branch: String,
    state: PkgState,
    /// Name of the peer that started the package, after a handover.
    taken_from: Option<String>,
}

/// Runs a room from invitation to result. Returns the text for the human who asked.
pub async fn run_opener(
    shared: Arc<Shared>,
    room_id: String,
    topic: String,
    joiners: Vec<Member>,
    turns: u32,
    token_budget: u64,
    lean: bool,
    project: Option<(String, PathBuf)>,
    rx: mpsc::Receiver<RoomEvent>,
    state: watch::Sender<RoomState>,
) -> Result<String> {
    let started = std::time::Instant::now();
    let me = Participant { id: shared.me.clone(), name: shared.state.lock().unwrap().name.clone() };
    let joiners: Vec<Participant> = joiners.iter().map(|m| Participant { id: m.id.clone(), name: m.name.clone() }).collect();
    let dir = room_dir(&room_id);
    std::fs::create_dir_all(&dir)?;
    append(&dir, "log.md", &format!("# Room {room_id}\n\nTopic: {topic}\n"));

    let mut participants = vec![me.clone()];
    participants.extend(joiners.iter().cloned());
    for j in &joiners {
        let invite = Msg::RoomInvite { room_id: room_id.clone(), topic: topic.clone(), turn_budget: turns, token_budget, project: project.as_ref().map(|(url, _)| url.clone()), participants: participants.clone(), mode: String::new(), catch_up: None, seq: 0, lean };
        if !shared.send_to(&j.id, invite) {
            bail!("peer \"{}\" is offline", j.name);
        }
    }

    let others = joiners.iter().map(|j| j.name.as_str()).collect::<Vec<_>>().join(", ");
    // The session works in the project's clone; log and result stay in the room directory.
    let work_dir = project.as_ref().map(|(_, d)| d.clone()).unwrap_or_else(|| dir.clone());
    // A lean room discusses without tools; see `work_turn` for the work phase.
    let session = Session::start(&work_dir, &me.name, &others, if lean { "talk" } else { "full" })?;
    let budget = participants.iter().map(|p| (p.id.clone(), turns)).collect();
    let notice_idx = shared.notices_len();
    let mut room = Opener {
        shared,
        room_id,
        me,
        joiners,
        log: vec![],
        seen: 0,
        budget,
        rx,
        session,
        dir,
        joiner_tokens: HashMap::new(),
        token_budget,
        state,
        limited: HashSet::new(),
        split: None,
        has_project: project.is_some(),
        notice_idx,
        topic: topic.clone(),
        turns,
        project_url: project.as_ref().map(|(url, _)| url.clone()),
        invited: vec![],
        arrivals: vec![],
        last_next: None,
        said: HashMap::new(),
        assigned: HashMap::new(),
        lean,
        work_session: None,
        work_dir,
        others,
        plan: String::new(),
    };

    let brief_topic = match &project {
        Some((url, d)) => format!("{topic}\nProject: {url}. You are in its local clone at {}.", d.display()),
        None => topic.clone(),
    };
    let outcome = room.run(&brief_topic).await;
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
    let peers: serde_json::Map<String, serde_json::Value> =
        room.joiners.iter().map(|j| (j.name.clone(), room.joiner_tokens.get(&j.id).copied().unwrap_or(0).into())).collect();
    write_stats(&room.dir, &room.room_id, &topic, "opened", &reason, room.log.len(), &room.session, Some((total, peers)), started.elapsed());
    let closing = outcome.map(|(reason, result)| format!("{result}\n\n[room {} closed: {reason}; {} messages; this peer: {stats}; all peers: {total} tokens; took {}s]", room.room_id, room.log.len(), started.elapsed().as_secs()))?;
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
            let (kind, text) = self.wait_say(&j.id).await?;
            if kind == "SKIP" {
                bail!("{} left the room during the briefing", j.name);
            }
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
        let mut previous: Option<String> = None;
        let reason = loop {
            // Peers that joined late speak before the opener from now on.
            for p in std::mem::take(&mut self.arrivals) {
                order.insert(order.len() - 1, p);
            }
            if order.iter().all(|p| self.budget[&p.id] == 0) {
                break "turn budget used up";
            }
            let speaker = order[idx % order.len()].clone();
            idx += 1;
            if self.budget[&speaker.id] == 0 {
                continue;
            }
            // Only one peer has turns left: it would be talking to itself.
            if previous.as_deref() == Some(speaker.id.as_str()) {
                break "turn budget used up";
            }
            previous = Some(speaker.id.clone());
            let (kind, text) = if speaker.id == self.me.id {
                let left = self.budget[&speaker.id];
                match self.own_turn(&format!("Your turn ({left} turns left).")).await {
                    Ok(reply) => proto::parse_kind(&reply, "PROPOSAL", &KINDS),
                    Err(e) if is_limit(&e) => ("LIMIT".to_string(), "usage limit reached".to_string()),
                    Err(e) => return Err(e),
                }
            } else {
                // Tell the speaker it is its turn if the last message did not.
                if self.last_next.as_deref() != Some(speaker.id.as_str()) {
                    let me = self.me.clone();
                    self.publish(&me, "TURN", &format!("{} is next.", speaker.name), Some(speaker.id.clone()));
                }
                self.wait_say(&speaker.id).await?
            };
            if kind == "SKIP" {
                // The speaker left; go on with the others if two remain.
                if order.iter().filter(|p| !self.limited.contains(&p.id)).count() < 2 {
                    break "a peer left the room";
                }
                continue;
            }
            *self.budget.get_mut(&speaker.id).unwrap() -= 1;
            match kind.as_str() {
                // A peer at its usage limit takes no further turns.
                "LIMIT" => {
                    self.limited.insert(speaker.id.clone());
                    self.budget.insert(speaker.id.clone(), 0);
                }
                "SPLIT_WORK" => {
                    let names: Vec<String> = order.iter().map(|p| p.name.clone()).collect();
                    let found = proto::parse_packages(&text, &names);
                    self.plan = text.clone();
                    self.split = if found.is_empty() { None } else { Some(found.into_iter().map(|(i, t)| (order[i].id.clone(), t)).collect()) };
                }
                "PROPOSAL" => self.split = None,
                _ => {}
            }
            let next = self.next_speaker(&order, idx);
            last_kind.insert(speaker.id.clone(), kind.clone());
            let active: Vec<&Participant> = order.iter().filter(|p| !self.limited.contains(&p.id)).collect();
            let mut done = if kind == "NEED_HUMAN" {
                Some("a human has to decide")
            } else if active.len() < 2 {
                Some("a peer reached its usage limit")
            } else if active.iter().all(|p| last_kind.get(&p.id).is_some_and(|k| k == "CONSENSUS")) {
                Some("consensus")
            } else if active.iter().all(|p| last_kind.get(&p.id).is_some_and(|k| k == "PASS" || k == "CONSENSUS")) {
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

        // If the peers agreed on a split, they now do the work.
        let mut reason = reason.to_string();
        if reason == "consensus" {
            if let Some(split) = self.split.clone() {
                reason = self.work_phase(split).await?;
            }
        }
        let ask = if reason.starts_with("work") {
            "The work phase is over. Write the result for the human who asked: per package what was done and on which branch, what is blocked or left, and how to review and merge it. No kind on the first line."
        } else if reason == "consensus" {
            "The room reached consensus. Write the agreed result for the human who asked: complete and usable on its own. No kind on the first line."
        } else {
            "The room closes without consensus. Write an overview for the human who asked with three parts: agreed, disputed, and what the human has to decide. No kind on the first line."
        };
        let result = match self.own_turn(ask).await {
            Ok(r) => r,
            // Without capacity left there is no model to write the summary.
            Err(e) if is_limit(&e) => self.plain_overview(),
            Err(e) => return Err(e),
        };
        Ok((reason, result))
    }

    /// The last message of every peer, for when our own session cannot summarize.
    fn plain_overview(&self) -> String {
        let mut out = String::from("This machine's session is at its usage limit, so this overview was assembled without a model. Last message of each peer:\n");
        let mut order = self.joiners.clone();
        order.push(self.me.clone());
        for p in order {
            if let Some(e) = self.log.iter().rev().find(|e| e.from == p.id && e.kind != "HUMAN") {
                out.push_str(&format!("\n{}", render(e)));
            }
        }
        out
    }

    fn participant(&self, id: &str) -> Participant {
        self.joiners.iter().find(|j| j.id == id).cloned().unwrap_or_else(|| self.me.clone())
    }

    fn at_limit(&self, id: &str) -> bool {
        self.limited.contains(id) || self.shared.capacity_of(id).is_some_and(|c| c.limited)
    }

    /// Gives package `i` to the peer with capacity that has the least to do.
    fn reassign(&mut self, pkgs: &mut [Pkg], i: usize, why: &str) {
        let old = self.participant(&pkgs[i].owner);
        let mut all = self.joiners.clone();
        all.push(self.me.clone());
        let load = |id: &str| pkgs.iter().filter(|p| p.owner == id && matches!(p.state, PkgState::Open | PkgState::Running)).count();
        let taker = all.iter().filter(|p| p.id != old.id && !self.at_limit(&p.id)).min_by_key(|p| load(&p.id)).cloned();
        let me = self.me.clone();
        match taker {
            Some(t) => {
                pkgs[i].owner = t.id.clone();
                pkgs[i].taken_from = Some(old.name.clone());
                pkgs[i].state = PkgState::Open;
                self.publish(&me, "HANDOVER", &format!("#{} {} goes from {} to {}: {} {why}. Branch: {}", pkgs[i].id, pkgs[i].title, old.name, t.name, old.name, pkgs[i].branch), None);
            }
            None => {
                pkgs[i].state = PkgState::Blocked;
                self.publish(&me, "BLOCKED", &format!("#{} {}: {} {why} and no other peer has capacity left.", pkgs[i].id, pkgs[i].title, old.name), None);
            }
        }
    }

    /// The peers work on the agreed packages at the same time, each on its own
    /// branch. A package whose owner reaches its usage limit moves to another peer.
    async fn work_phase(&mut self, split: Vec<(String, String)>) -> Result<String> {
        let mut pkgs: Vec<Pkg> = split
            .into_iter()
            .enumerate()
            .map(|(i, (owner, title))| {
                let branch = format!("agora/{}/{}", self.room_id, self.participant(&owner).name.to_lowercase());
                Pkg { id: i as u32 + 1, owner, title, branch, state: PkgState::Open, taken_from: None }
            })
            .collect();
        let list = pkgs.iter().map(|p| format!("#{} {}: {} (branch {})", p.id, self.participant(&p.owner).name, p.title, p.branch)).collect::<Vec<_>>().join("\n");
        let me = self.me.clone();
        self.publish(&me, "WORK", &format!("Work phase. Packages:\n{list}"), None);
        self.session.timeout = WORK_TIMEOUT;
        loop {
            for i in 0..pkgs.len() {
                if pkgs[i].state == PkgState::Open && self.at_limit(&pkgs[i].owner) {
                    self.reassign(&mut pkgs, i, "is at its usage limit");
                }
            }
            // Start one package per idle peer.
            let mut mine = None;
            for i in 0..pkgs.len() {
                let owner = pkgs[i].owner.clone();
                if pkgs[i].state != PkgState::Open || pkgs.iter().any(|p| p.owner == owner && p.state == PkgState::Running) {
                    continue;
                }
                pkgs[i].state = PkgState::Running;
                if owner == self.me.id {
                    mine = Some(i);
                } else {
                    let p = &pkgs[i];
                    let work = Msg::Work { room_id: self.room_id.clone(), package: p.id, title: p.title.clone(), branch: p.branch.clone(), taken_from: p.taken_from.clone() };
                    self.assigned.insert(owner.clone(), work.clone());
                    self.shared.send_to(&owner, work);
                }
            }
            if !pkgs.iter().any(|p| p.state == PkgState::Running) {
                break;
            }
            let (i, kind, text) = if let Some(i) = mine {
                let prompt = work_prompt(pkgs[i].id, &pkgs[i].title, &pkgs[i].branch, pkgs[i].taken_from.as_deref(), self.has_project);
                match self.work_turn(&prompt).await {
                    Ok(reply) => {
                        let (kind, text) = proto::parse_kind(&reply, "DONE", &["DONE", "BLOCKED"]);
                        (i, kind, text)
                    }
                    Err(e) if is_limit(&e) => (i, "LIMIT".to_string(), "usage limit reached".to_string()),
                    Err(e) => return Err(e),
                }
            } else {
                let (peer, id, kind, text) = self.wait_report().await?;
                if kind == "SKIP" {
                    // A peer left: its running package goes to someone else.
                    for i in 0..pkgs.len() {
                        if pkgs[i].state == PkgState::Running && pkgs[i].owner != self.me.id && self.limited.contains(&pkgs[i].owner) {
                            let gone = pkgs[i].owner.clone();
                            self.assigned.remove(&gone);
                            self.reassign(&mut pkgs, i, "left the room");
                        }
                    }
                    continue;
                }
                match pkgs.iter().position(|p| p.id == id && p.owner == peer && p.state == PkgState::Running) {
                    Some(i) => (i, kind, text),
                    None => continue,
                }
            };
            let owner = self.participant(&pkgs[i].owner);
            self.assigned.remove(&owner.id);
            let head = format!("#{} {}", pkgs[i].id, pkgs[i].title);
            match kind.as_str() {
                "LIMIT" => {
                    self.limited.insert(owner.id.clone());
                    self.publish(&owner, "LIMIT", &format!("{head}: {text}"), None);
                    self.reassign(&mut pkgs, i, "reached its usage limit");
                }
                "BLOCKED" => {
                    pkgs[i].state = PkgState::Blocked;
                    self.publish(&owner, "BLOCKED", &format!("{head}\n{text}"), None);
                }
                _ => {
                    pkgs[i].state = PkgState::Done;
                    self.publish(&owner, "DONE", &format!("{head}\n{text}"), None);
                }
            }
        }
        self.session.timeout = TURN_TIMEOUT;
        let all_done = pkgs.iter().all(|p| p.state == PkgState::Done);
        Ok(if all_done { "work finished" } else { "work finished with open packages" }.to_string())
    }

    /// Our own package. In a lean room the discussion session has no tools, so a
    /// second session with tools does the work and is told the agreed plan.
    async fn work_turn(&mut self, prompt: &str) -> Result<String> {
        if !self.lean {
            return self.own_turn(prompt).await;
        }
        if self.work_session.is_none() {
            self.work_session = Some(Session::start(&self.work_dir, &self.me.name, &self.others, "full")?);
        }
        let ws = self.work_session.as_mut().unwrap();
        ws.timeout = WORK_TIMEOUT;
        let reply = ws.turn(&format!("You take part in a room with {}. The room agreed on this plan:\n{}\n\n{prompt}", self.others, self.plan)).await;
        if let Some(c) = ws.capacity {
            self.shared.report_capacity(c);
        }
        reply
    }

    /// True if this is a contribution we have not used yet.
    fn fresh(&mut self, peer: &str, n: u32) -> bool {
        if n == 0 {
            return true;
        }
        let last = self.said.entry(peer.to_string()).or_insert(0);
        if n <= *last {
            return false;
        }
        *last = n;
        true
    }

    /// Takes a peer out of the room: it speaks no more and gets no more work.
    fn drop_peer(&mut self, id: &str, why: &str) {
        if !self.limited.insert(id.to_string()) {
            return;
        }
        self.budget.insert(id.to_string(), 0);
        let p = self.participant(id);
        self.publish(&p, "LEFT", why, None);
    }

    /// What a peer that joins late is told instead of the whole log.
    fn catch_up(&self) -> String {
        let all: Vec<String> = self.log.iter().map(render).collect();
        if all.iter().map(String::len).sum::<usize>() <= 6000 || all.len() <= 7 {
            return all.join("\n");
        }
        format!("{}\n[... {} messages left out ...]\n\n{}", all[0], all.len() - 7, all[all.len() - 6..].join("\n"))
    }

    /// Waits for a joiner's report on a work package.
    async fn wait_report(&mut self) -> Result<(String, u32, String, String)> {
        let gone = self.limited.len();
        loop {
            let ev = tokio::time::timeout(WORK_TIMEOUT, self.rx.recv()).await.map_err(|_| anyhow!("a peer did not report on its package within {} minutes", WORK_TIMEOUT.as_secs() / 60))?;
            match ev {
                Some(RoomEvent::Remote(p, Msg::Say { kind, text, tokens, package: Some(id), n, .. })) if kind != "HUMAN" => {
                    if !self.fresh(&p, n) {
                        continue;
                    }
                    self.joiner_tokens.insert(p.clone(), tokens);
                    return Ok((p, id, kind, text));
                }
                Some(ev) => {
                    self.side_event(ev)?;
                    if self.limited.len() != gone {
                        return Ok((String::new(), 0, "SKIP".to_string(), String::new()));
                    }
                }
                None => bail!("room stopped"),
            }
        }
    }

    fn total_tokens(&self) -> u64 {
        self.session.tokens() + self.work_session.as_ref().map_or(0, |s| s.tokens()) + self.joiner_tokens.values().sum::<u64>()
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
        crate::daemon::event("room-paused", &format!("{}: {} asks a human (agora say {} \"<answer>\")", self.room_id, asker.name, self.room_id));
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

    /// The invitation into this room, for the first joiners and for retries.
    fn invitation(&self) -> Msg {
        let mut participants = vec![self.me.clone()];
        participants.extend(self.joiners.iter().cloned());
        Msg::RoomInvite {
            room_id: self.room_id.clone(),
            topic: self.topic.clone(),
            turn_budget: self.turns,
            token_budget: self.token_budget,
            project: self.project_url.clone(),
            participants,
            mode: String::new(),
            catch_up: None,
            seq: 0,
            lean: self.lean,
        }
    }

    async fn wait_for_joins(&mut self) -> Result<()> {
        let mut missing: Vec<String> = self.joiners.iter().map(|j| j.id.clone()).collect();
        let deadline = std::time::Instant::now() + JOIN_TIMEOUT;
        while !missing.is_empty() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                bail!("the invited peers did not join within {}s; they may need to run `agora accept {}`", JOIN_TIMEOUT.as_secs(), self.room_id);
            }
            let ev = match tokio::time::timeout(left.min(Duration::from_secs(10)), self.rx.recv()).await {
                Ok(ev) => ev,
                Err(_) => {
                    // An invitation or its answer may have been lost with a link: ask again.
                    for id in &missing {
                        self.shared.send_to(id, self.invitation());
                    }
                    continue;
                }
            };
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
            next: next.clone(),
        };
        append(&self.dir, "log.md", &render(&e));
        for j in &self.joiners {
            // A peer that is offline right now asks for what it missed when it is back.
            self.shared.send_to(&j.id, discuss_msg(&self.room_id, &e));
        }
        self.log.push(e);
        if next.is_some() {
            self.last_next = next;
        }
    }

    /// Takes a turn with our own session: new messages since its last turn, then `ask`.
    async fn own_turn(&mut self, ask: &str) -> Result<String> {
        self.drain()?;
        let mut news: String = self.log[self.seen..].iter().filter(|e| e.from != self.me.id || e.kind == "HUMAN").map(render).collect::<Vec<_>>().join("\n");
        self.seen = self.log.len();
        news = format!("{}{news}", memory_notes(&self.shared, &mut self.notice_idx));
        let reply = self.session.turn(&format!("{news}\n{ask}")).await;
        if let Some(c) = self.session.capacity {
            self.shared.report_capacity(c);
        }
        let reply = reply?;
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
            RoomEvent::Remote(p, Msg::Say { kind, text, extra_turns, n, .. }) if kind == "HUMAN" => {
                if let Some(j) = self.joiners.iter().find(|j| j.id == p).cloned() {
                    if self.fresh(&p, n) {
                        self.grant(extra_turns);
                        self.publish_human(&j, &text);
                    }
                }
            }
            // A joiner that leaves, or whose daemon lost the room, drops out; the others go on.
            RoomEvent::Remote(p, Msg::RoomDecline { reason, .. }) => {
                if self.joiners.iter().any(|j| j.id == p) {
                    self.drop_peer(&p, &format!("left the room: {reason}"));
                } else {
                    self.invited.retain(|i| i.id != p);
                }
            }
            RoomEvent::Invite(m) => {
                let known = m.id == self.me.id || self.joiners.iter().chain(self.invited.iter()).any(|x| x.id == m.id);
                if !known {
                    let new = Participant { id: m.id.clone(), name: m.name.clone() };
                    let mut participants = vec![self.me.clone()];
                    participants.extend(self.joiners.iter().cloned());
                    participants.push(new.clone());
                    let invite = Msg::RoomInvite {
                        room_id: self.room_id.clone(),
                        topic: self.topic.clone(),
                        turn_budget: self.turns,
                        token_budget: self.token_budget,
                        project: self.project_url.clone(),
                        participants,
                        mode: String::new(),
                        catch_up: Some(self.catch_up()),
                        seq: self.log.len() as u32,
                        lean: self.lean,
                    };
                    if self.shared.send_to(&new.id, invite) {
                        self.invited.push(new);
                    }
                }
            }
            RoomEvent::Remote(p, Msg::RoomJoin { .. }) => {
                if let Some(i) = self.invited.iter().position(|x| x.id == p) {
                    let new = self.invited.remove(i);
                    self.budget.insert(new.id.clone(), self.turns);
                    self.joiners.push(new.clone());
                    self.arrivals.push(new.clone());
                    self.publish(&new, "JOIN", "joined the room.", None);
                }
            }
            // A joiner missed messages, usually because the link dropped.
            RoomEvent::Remote(p, Msg::Resend { from_seq, .. }) if self.joiners.iter().any(|j| j.id == p) => {
                for e in self.log.iter().filter(|e| e.seq >= from_seq) {
                    self.shared.send_to(&p, discuss_msg(&self.room_id, e));
                }
                if let Some(work) = self.assigned.get(&p) {
                    self.shared.send_to(&p, work.clone());
                }
            }
            RoomEvent::LinkUp(p) if self.joiners.iter().any(|j| j.id == p) => {
                self.shared.send_to(&p, Msg::RoomPing { room_id: self.room_id.clone() });
            }
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
            let ev = match tokio::time::timeout(TURN_TIMEOUT, self.rx.recv()).await {
                Ok(ev) => ev,
                Err(_) => {
                    // A peer that does not answer drops out instead of ending the room.
                    self.drop_peer(speaker, &format!("did not answer within {} minutes", TURN_TIMEOUT.as_secs() / 60));
                    return Ok(("SKIP".to_string(), String::new()));
                }
            };
            match ev {
                Some(RoomEvent::Remote(p, Msg::Say { kind, text, tokens, package: None, n, .. })) if p == speaker && kind != "HUMAN" => {
                    if !self.fresh(&p, n) {
                        continue;
                    }
                    self.joiner_tokens.insert(p, tokens);
                    return Ok((kind, text));
                }
                Some(ev) => {
                    self.side_event(ev)?;
                    if self.limited.contains(speaker) {
                        return Ok(("SKIP".to_string(), String::new()));
                    }
                }
                None => bail!("room stopped"),
            }
        }
    }
}

/// Asks one peer one question. No session runs on the asking machine, so the
/// question costs tokens only where it is answered.
pub async fn run_ask(
    shared: Arc<Shared>,
    room_id: String,
    question: String,
    peer: Member,
    project: Option<(String, PathBuf)>,
    mode: &str,
    mut rx: mpsc::Receiver<RoomEvent>,
) -> Result<String> {
    let started = std::time::Instant::now();
    let me = Participant { id: shared.me.clone(), name: shared.state.lock().unwrap().name.clone() };
    let other = Participant { id: peer.id.clone(), name: peer.name.clone() };
    let dir = room_dir(&room_id);
    std::fs::create_dir_all(&dir)?;
    append(&dir, "log.md", &format!("# Question {room_id} to {}\n\n{question}\n", other.name));
    let invite = Msg::RoomInvite {
        room_id: room_id.clone(),
        topic: question.clone(),
        turn_budget: 1,
        token_budget: 0,
        project: project.map(|(url, _)| url),
        participants: vec![me, other.clone()],
        mode: mode.to_string(),
        catch_up: None,
        seq: 0,
        lean: false,
    };
    if !shared.send_to(&other.id, invite) {
        bail!("peer \"{}\" is offline", other.name);
    }
    let mut joined = false;
    loop {
        let wait = match (joined, mode) {
            (false, _) => JOIN_TIMEOUT,
            (true, "task") => WORK_TIMEOUT,
            (true, _) => TURN_TIMEOUT,
        };
        let ev = tokio::time::timeout(wait, rx.recv()).await.map_err(|_| {
            if joined {
                anyhow!("{} did not answer within {} minutes", other.name, wait.as_secs() / 60)
            } else {
                anyhow!("{} did not take the question within {}s; it may need `agora accept {room_id}` there", other.name, wait.as_secs())
            }
        })?;
        match ev {
            Some(RoomEvent::Remote(p, Msg::RoomJoin { .. })) if p == other.id => joined = true,
            Some(RoomEvent::Remote(p, Msg::RoomDecline { reason, .. })) if p == other.id => bail!("{} declined: {reason}", other.name),
            Some(RoomEvent::Remote(p, Msg::Say { kind, text, tokens, .. })) if p == other.id => {
                append(&dir, "log.md", &format!("[{} · {kind}]\n{text}\n", other.name));
                shared.send_to(&other.id, Msg::RoomClose { room_id: room_id.clone(), reason: "answered".into(), result: String::new() });
                if kind != "ANSWER" {
                    bail!("{}: {text}", other.name);
                }
                let did = if mode == "task" { "reported back after" } else { "answered in" };
                let closing = format!("{text}\n\n[{} {did} {}s using {tokens} tokens; none were used on this machine]", other.name, started.elapsed().as_secs());
                std::fs::write(dir.join("result.md"), &text)?;
                std::fs::write(dir.join("closing.md"), &closing)?;
                return Ok(closing);
            }
            Some(RoomEvent::Panic) | None => bail!("stopped"),
            _ => {}
        }
    }
}

// ------------------------------------------------------------------- joiner

/// Takes part in a room another peer opened.
pub async fn run_joiner(shared: Arc<Shared>, invite: Invite, mut rx: mpsc::Receiver<RoomEvent>) -> Result<()> {
    let started = std::time::Instant::now();
    let me = shared.me.clone();
    let my_name = shared.state.lock().unwrap().name.clone();
    let dir = room_dir(&invite.room_id);
    std::fs::create_dir_all(&dir)?;
    append(&dir, "log.md", &format!("# Room {}\n\nTopic: {}\n", invite.room_id, invite.topic));
    let others = invite.participants.iter().filter(|p| p.id != me).map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
    // Work in the local clone of the room's project if this machine has one.
    let clone = invite.project.as_ref().and_then(|url| {
        let projects = shared.state.lock().unwrap().projects.clone();
        project::resolve(&projects, url)
    });
    let project_note = match (&invite.project, &clone) {
        (Some(url), Some(d)) => format!("\nProject: {url}. You are in its local clone at {}.", d.display()),
        (Some(url), None) => format!("\nProject: {url}. This machine has no registered clone of it, so you cannot look at its files; say so."),
        _ => String::new(),
    };
    let has_clone = clone.is_some();
    let work_dir = clone.unwrap_or_else(|| dir.clone());
    let discuss_rights = if invite.lean { "talk" } else { invite.rights.as_str() };
    let mut session = match Session::start(&work_dir, &my_name, &others, discuss_rights) {
        Ok(s) => s,
        Err(e) => {
            shared.send_to(&invite.from, Msg::RoomDecline { room_id: invite.room_id.clone(), reason: e.to_string() });
            return Err(e);
        }
    };
    shared.send_to(&invite.from, Msg::RoomJoin { room_id: invite.room_id.clone() });

    // A single question or a handed-over task: one turn, then this session is done.
    if invite.mode == "ask" || invite.mode == "task" {
        let asker = invite.participants.iter().find(|p| p.id == invite.from).map(|p| p.name.clone()).unwrap_or_default();
        let prompt = if invite.mode == "task" {
            session.timeout = WORK_TIMEOUT;
            let how = if has_clone {
                format!(" Work on the branch `agora/{}/{}`: create it from the current HEAD, commit on it, and push it to origin if you are allowed to.", invite.room_id, my_name.to_lowercase())
            } else {
                String::new()
            };
            format!(
                "{asker} hands you a task to carry out on this machine.{project_note}{how} \
Reply with the first line DONE or BLOCKED, then a short report: what you did, where the result is, and what is left.\n\nTask: {}",
                invite.topic
            )
        } else {
            format!(
                "{asker} asks you one question. Answer it from what this machine knows and can look up. Be brief, and say so if you cannot know. \
This answer needs no kind on the first line.{project_note}\n\nQuestion: {}",
                invite.topic
            )
        };
        let turn = session.turn(&prompt).await;
        if let Some(c) = session.capacity {
            shared.report_capacity(c);
        }
        let (kind, text) = match turn {
            Ok(r) => ("ANSWER".to_string(), r),
            Err(e) if is_limit(&e) => ("LIMIT".to_string(), "this peer is at its usage limit".to_string()),
            Err(e) => ("BLOCKED".to_string(), format!("the session failed: {e}")),
        };
        append(&dir, "log.md", &format!("[{my_name} · {kind}]\n{text}\n"));
        std::fs::write(dir.join("result.md"), &text)?;
        shared.send_to(&invite.from, Msg::Say { room_id: invite.room_id.clone(), kind, text, tokens: session.tokens(), extra_turns: 0, package: None, n: 1 });
        write_stats(&dir, &invite.room_id, &invite.topic, "answered", "answered", 1, &session, None, started.elapsed());
        return Ok(());
    }

    let mut news: Vec<Entry> = vec![];
    let mut notice_idx = shared.notices_len();
    let mut my_left = invite.turn_budget;
    // Numbers of the last message received and the last contribution sent, so
    // that a dropped link neither loses nor repeats anything.
    let mut last_seq = invite.seq;
    let mut said = 0u32;
    let mut pending: Option<Msg> = None;
    let mut reports: HashMap<u32, Msg> = HashMap::new();
    let mut catch_up = invite.catch_up.clone();
    let mut plan = String::new();
    let mut work_session: Option<Session> = None;
    let room = invite.room_id.clone();
    let say = |said: &mut u32, kind: String, text: String, tokens: u64, extra_turns: u32, package: Option<u32>| {
        *said += 1;
        Msg::Say { room_id: room.clone(), kind, text, tokens, extra_turns, package, n: *said }
    };
    let outcome: Result<String> = loop {
        let ev = match tokio::time::timeout(JOINER_IDLE, rx.recv()).await {
            Ok(ev) => ev,
            Err(_) => break Ok("the opener did not come back".into()),
        };
        let used = |session: &Session, work: &Option<Session>| session.tokens() + work.as_ref().map_or(0, |s| s.tokens());
        match ev {
            // Back in touch with the opener: ask for what we missed, repeat what it may have missed.
            Some(RoomEvent::LinkUp(p)) if p == invite.from => {
                shared.send_to(&invite.from, Msg::Resend { room_id: invite.room_id.clone(), from_seq: last_seq + 1 });
                if let Some(m) = &pending {
                    shared.send_to(&invite.from, m.clone());
                }
            }
            Some(RoomEvent::Remote(p, Msg::Discuss { seq, from, from_name, kind, text, turns_left, next, .. })) if p == invite.from => {
                if seq <= last_seq {
                    continue;
                }
                if seq > last_seq + 1 {
                    shared.send_to(&invite.from, Msg::Resend { room_id: invite.room_id.clone(), from_seq: last_seq + 1 });
                    continue;
                }
                last_seq = seq;
                let e = Entry { seq, from, from_name, kind, text, turns_left, next: next.clone() };
                append(&dir, "log.md", &render(&e));
                if e.kind == "SPLIT_WORK" {
                    plan = e.text.clone();
                }
                if e.from == me && e.kind != "HUMAN" {
                    my_left = e.turns_left;
                    pending = None;
                } else {
                    news.push(e);
                }
                if next.as_deref() != Some(me.as_str()) || (news.is_empty() && catch_up.is_none()) {
                    continue;
                }
                let briefing = news.iter().any(|e| e.kind == "BRIEF");
                let ask = if briefing {
                    format!("Topic: {}{project_note}\n\nBriefing: your turn. First line: KNOWN_MISSING. Say briefly what you already know about this and what you are missing to take part. Do not propose anything yet; the discussion starts after the opener has answered.", invite.topic)
                } else if let Some(before) = catch_up.take() {
                    format!("Topic: {}{project_note}\n\nYou join this room while it is running. What was said before you joined:\n{before}\n\nYour turn ({my_left} turns left).", invite.topic)
                } else {
                    format!("Your turn ({my_left} turns left).")
                };
                let prompt = format!("{}{}\n{ask}", memory_notes(&shared, &mut notice_idx), news.iter().map(render).collect::<Vec<_>>().join("\n"));
                news.clear();
                let turn = session.turn(&prompt).await;
                if let Some(c) = session.capacity {
                    shared.report_capacity(c);
                }
                let (kind, text) = match turn {
                    Ok(reply) if briefing => proto::parse_kind(&reply, "KNOWN_MISSING", &["KNOWN_MISSING"]),
                    Ok(reply) => proto::parse_kind(&reply, "PROPOSAL", &KINDS),
                    Err(e) if is_limit(&e) => ("LIMIT".to_string(), "usage limit reached".to_string()),
                    Err(e) => {
                        shared.send_to(&invite.from, Msg::RoomDecline { room_id: invite.room_id.clone(), reason: e.to_string() });
                        break Err(e);
                    }
                };
                let m = say(&mut said, kind, text, used(&session, &work_session), 0, None);
                pending = Some(m.clone());
                shared.send_to(&invite.from, m);
            }
            Some(RoomEvent::Remote(p, Msg::Work { package, title, branch, taken_from, .. })) if p == invite.from => {
                // Asked again for a package we already reported on: repeat the report.
                if let Some(m) = reports.get(&package) {
                    shared.send_to(&invite.from, m.clone());
                    continue;
                }
                let task = work_prompt(package, &title, &branch, taken_from.as_deref(), has_clone);
                // In a lean room the discussion session has no tools; a second one does the work.
                if invite.lean && work_session.is_none() {
                    work_session = Session::start(&work_dir, &my_name, &others, &invite.rights).ok();
                }
                let prompt = if work_session.is_some() {
                    format!("You take part in a room with {others}. The room agreed on this plan:\n{plan}\n\n{task}")
                } else {
                    format!("{}\n{task}", news.iter().map(render).collect::<Vec<_>>().join("\n"))
                };
                news.clear();
                let worker = work_session.as_mut().unwrap_or(&mut session);
                worker.timeout = WORK_TIMEOUT;
                let turn = worker.turn(&prompt).await;
                if let Some(c) = worker.capacity {
                    shared.report_capacity(c);
                }
                let (kind, text) = match turn {
                    Ok(r) => proto::parse_kind(&r, "DONE", &["DONE", "BLOCKED"]),
                    Err(e) if is_limit(&e) => ("LIMIT".to_string(), "usage limit reached".to_string()),
                    Err(e) => ("BLOCKED".to_string(), format!("the session failed: {e}")),
                };
                let m = say(&mut said, kind, text, used(&session, &work_session), 0, Some(package));
                reports.insert(package, m.clone());
                pending = Some(m.clone());
                shared.send_to(&invite.from, m);
            }
            Some(RoomEvent::Remote(p, Msg::RoomClose { reason, result, .. })) if p == invite.from => {
                std::fs::write(dir.join("result.md"), &result)?;
                break Ok(reason);
            }
            Some(RoomEvent::Human(text, extra_turns)) => {
                let m = say(&mut said, "HUMAN".to_string(), text, used(&session, &work_session), extra_turns, None);
                shared.send_to(&invite.from, m);
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
    let messages = std::fs::read_to_string(dir.join("log.md")).map(|l| l.lines().filter(|x| x.starts_with("[#")).count()).unwrap_or(0);
    write_stats(&dir, &invite.room_id, &invite.topic, "joined", &reason, messages, &session, None, started.elapsed());
    outcome.map(|_| ())
}
