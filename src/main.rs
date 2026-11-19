//! `agora` — command and daemon of agora-mesh. See ARCHITECTURE.md.

mod daemon;
mod memory;
mod project;
mod proto;
mod room;
mod state;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};

use daemon::{DaemonInfo, Request, Response};
use state::State;

/// Prints a line. A reader that stops early (`agora status | grep -q ...`) is not an error.
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

#[derive(Parser)]
#[command(name = "agora", version, about = "Symmetric peer-to-peer mesh for Claude Code")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create this machine's identity
    Setup {
        /// Name other peers see, for example "MacBook"
        #[arg(long)]
        name: String,
    },
    /// Run the daemon in the foreground
    Daemon,
    /// Show identity, networks, peers and join requests
    Status,
    /// Open a new network and print its network ID
    Open,
    /// Ask to join a network
    Join { network_id: String },
    /// Approve a join request after comparing the four words
    Approve {
        name: String,
        /// Knowledge level 1, 2 or 3
        #[arg(long, default_value = "1")]
        level: String,
        /// full, read or talk
        #[arg(long, default_value = "full")]
        rights: String,
    },
    /// Reject a join request
    Reject { name: String },
    /// Remove a peer from its network
    Revoke { name: String },
    /// Set what a peer may do on this machine
    Trust {
        name: String,
        /// Knowledge level 1, 2 or 3; from 2 on, room invitations are accepted automatically
        #[arg(long, default_value = "1")]
        level: String,
        /// full, read or talk
        #[arg(long, default_value = "full")]
        rights: String,
    },
    /// Open a room, let the agents discuss, and print their result
    Discuss {
        topic: String,
        /// Peer names, comma-separated; default: every peer that is online
        #[arg(long, default_value = "")]
        with: String,
        /// Turns per peer after the briefing
        #[arg(long, default_value = "6")]
        turns: String,
        /// Token budget of the whole room, all peers together
        #[arg(long, default_value = "1000000")]
        tokens: String,
        /// Git repository the room is about; default: the one you are in. "none" for no project
        #[arg(long, default_value = "")]
        project: String,
        /// Importance of the result in the shared memory: high, normal, low or none
        #[arg(long, default_value = "normal")]
        remember: String,
        /// Discuss without tools to save tokens; a second session with tools does the work
        #[arg(long)]
        lean: bool,
    },
    /// Invite another peer into a running room you opened
    Invite { room: String, name: String },
    /// Ask one peer a single question; no session runs on this machine
    Ask {
        question: String,
        /// Peer name; default: the only peer that is online
        #[arg(long, default_value = "")]
        with: String,
        /// Git repository the question is about; default: the one you are in. "none" for no project
        #[arg(long, default_value = "")]
        project: String,
    },
    /// Hand a task to one peer to carry out; no session runs on this machine
    Delegate {
        task: String,
        /// Peer name; default: the only peer that is online
        #[arg(long, default_value = "")]
        with: String,
        /// Git repository the task is about; default: the one you are in. "none" for no project
        #[arg(long, default_value = "")]
        project: String,
    },
    /// All peers at a glance: reachable, busy, and how much of their usage limit is used
    Peers,
    /// Let a peer review your changes against a base branch (tracked files, committed or not)
    Review {
        #[arg(long, default_value = "")]
        with: String,
        /// What to diff against; default: the remote's default branch
        #[arg(long, default_value = "")]
        base: String,
    },
    /// Run a shell command for every event, for example to message your phone; "off" stops it
    Notify { command: String },
    /// Save a fact in the shared memory of all peers
    Remember {
        text: String,
        /// Short title; an entry with the same title is replaced
        #[arg(long, default_value = "")]
        title: String,
        /// high: running sessions are told at once; normal: in the index; low: archive only
        #[arg(long, default_value = "normal")]
        importance: String,
        #[arg(long, default_value = "")]
        network: String,
    },
    /// List the shared memory, or show or forget one entry
    Memory {
        #[command(subcommand)]
        action: Option<MemoryCmd>,
    },
    /// Remove an entry from the shared memory on all peers
    Forget { id: String },
    /// Terms that must never go into the shared memory from this machine
    Block {
        #[command(subcommand)]
        action: BlockCmd,
    },
    /// Mirror the shared memory into an Obsidian vault folder, or "off"
    Obsidian { path: String },
    /// Print what happened: join requests, invitations, paused and closed rooms, important memory entries
    Events {
        /// Keep running and print new events as they happen
        #[arg(long)]
        follow: bool,
    },
    /// Local clones a room session may work in
    Project {
        #[command(subcommand)]
        action: ProjectCmd,
    },
    /// Token use of the rooms this machine took part in
    Stats,
    /// Measure the fixed context of a session on this machine (uses real Claude sessions)
    Bench,
    /// Wait for the result of a room that paused for a human
    Wait { room: String },
    /// List running rooms and open invitations
    Rooms,
    /// Accept a room invitation
    Accept { room: String },
    /// Decline a room invitation
    Decline { room: String },
    /// Say something in a running room as a human
    Say {
        room: String,
        text: String,
        /// Grant every peer this many more turns
        #[arg(long, default_value = "0")]
        turns: String,
    },
    /// Follow a room's log
    Watch { room: String },
    /// Stop all rooms and their sessions on this machine
    Panic,
    /// Stop the daemon
    Stop,
}

#[derive(Subcommand)]
enum MemoryCmd {
    /// List all entries
    List,
    /// Print one entry
    Show { id: String },
    /// Remove an entry on all peers
    Forget { id: String },
}

#[derive(Subcommand)]
enum BlockCmd {
    Add { term: String },
    List,
    Remove { term: String },
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Register a clone; default: the repository you are in
    Add { path: Option<String> },
    /// List registered clones
    List,
    /// Forget a registered clone
    Remove { path: String },
}

fn cwd() -> String {
    std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Setup { name } => setup(&name),
        Cmd::Daemon => tokio::runtime::Runtime::new()?.block_on(daemon::run()),
        Cmd::Stop => stop(),
        Cmd::Status => call("status", &[]),
        Cmd::Open => call("open", &[]),
        Cmd::Join { network_id } => call("join", &[("ticket", network_id)]),
        Cmd::Approve { name, level, rights } => call("approve", &[("name", name), ("level", level), ("rights", rights)]),
        Cmd::Reject { name } => call("reject", &[("name", name)]),
        Cmd::Revoke { name } => call("revoke", &[("name", name)]),
        Cmd::Discuss { topic, with, turns, tokens, project, remember, lean } => call(
            "discuss",
            &[
                ("topic", topic),
                ("with", with),
                ("turns", turns),
                ("tokens", tokens),
                ("project", project),
                ("remember", remember),
                ("lean", if lean { "1".into() } else { String::new() }),
                ("cwd", cwd()),
            ],
        ),
        Cmd::Invite { room, name } => call("invite", &[("room", room), ("name", name)]),
        Cmd::Ask { question, with, project } => call("ask", &[("question", question), ("with", with), ("project", project), ("cwd", cwd())]),
        Cmd::Delegate { task, with, project } => call("delegate", &[("question", task), ("with", with), ("project", project), ("cwd", cwd())]),
        Cmd::Peers => call("peers", &[]),
        Cmd::Review { with, base } => review(with, base),
        Cmd::Notify { command } => call("notify", &[("command", command)]),
        Cmd::Remember { text, title, importance, network } => {
            call("remember", &[("text", text), ("title", title), ("importance", importance), ("network", network)])
        }
        Cmd::Memory { action } => match action.unwrap_or(MemoryCmd::List) {
            MemoryCmd::List => call("memory", &[("action", "list".into())]),
            MemoryCmd::Show { id } => call("memory", &[("action", "show".into()), ("id", id)]),
            MemoryCmd::Forget { id } => call("memory", &[("action", "forget".into()), ("id", id)]),
        },
        Cmd::Forget { id } => call("memory", &[("action", "forget".into()), ("id", id)]),
        Cmd::Block { action } => match action {
            BlockCmd::Add { term } => call("block", &[("action", "add".into()), ("term", term)]),
            BlockCmd::List => call("block", &[("action", "list".into())]),
            BlockCmd::Remove { term } => call("block", &[("action", "remove".into()), ("term", term)]),
        },
        Cmd::Obsidian { path } => call("obsidian", &[("path", path)]),
        Cmd::Events { follow } => events(follow),
        Cmd::Project { action } => match action {
            ProjectCmd::Add { path } => call("project", &[("action", "add".into()), ("path", path.unwrap_or_else(cwd))]),
            ProjectCmd::List => call("project", &[("action", "list".into())]),
            ProjectCmd::Remove { path } => call("project", &[("action", "remove".into()), ("path", path)]),
        },
        Cmd::Stats => stats(),
        Cmd::Bench => {
            say!("Measuring with real Claude sessions on the smallest model; this takes about a minute.");
            say!("{}", tokio::runtime::Runtime::new()?.block_on(room::bench())?);
            Ok(())
        }
        Cmd::Wait { room } => call("wait", &[("room", room)]),
        Cmd::Trust { name, level, rights } => call("trust", &[("name", name), ("level", level), ("rights", rights)]),
        Cmd::Rooms => call("rooms", &[]),
        Cmd::Accept { room } => call("accept", &[("room", room)]),
        Cmd::Decline { room } => call("decline", &[("room", room)]),
        Cmd::Say { room, text, turns } => call("say", &[("room", room), ("text", text), ("turns", turns)]),
        Cmd::Panic => call("panic", &[]),
        Cmd::Watch { room } => watch(&room),
    }
}

fn setup(name: &str) -> Result<()> {
    if let Some(existing) = State::load()? {
        bail!("already set up as \"{}\" in {}", existing.name, state::home().display());
    }
    if name.trim().is_empty() || name.len() > 32 || name.chars().any(|c| c.is_control() || c.is_whitespace()) {
        bail!("the name must be 1 to 32 characters without spaces");
    }
    let key: [u8; 32] = rand::random();
    let public = iroh::SecretKey::from_bytes(&key).public();
    State { name: name.to_string(), secret_key: data_encoding::HEXLOWER.encode(&key), networks: vec![], projects: vec![], blocklist: vec![], obsidian: None, notify: None }.save()?;
    say!("Set up peer \"{name}\" with id {public}");
    say!("Data directory: {}", state::home().display());
    Ok(())
}

fn read_info() -> Option<DaemonInfo> {
    serde_json::from_str(&std::fs::read_to_string(DaemonInfo::path()).ok()?).ok()
}

fn connect() -> Option<(TcpStream, DaemonInfo)> {
    let info = read_info()?;
    let stream = TcpStream::connect_timeout(&([127, 0, 0, 1], info.port).into(), Duration::from_secs(2)).ok()?;
    Some((stream, info))
}

/// Connects to the daemon and starts it first if it is not running.
fn connect_or_start() -> Result<(TcpStream, DaemonInfo)> {
    if let Some(c) = connect() {
        return Ok(c);
    }
    if State::load()?.is_none() {
        bail!("not set up yet: run `agora setup --name <peer-name>` first");
    }
    let _ = std::fs::remove_file(DaemonInfo::path());
    let log = std::fs::OpenOptions::new().create(true).append(true).open(state::home().join("daemon.log"))?;
    std::process::Command::new(std::env::current_exe()?)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .context("starting the daemon")?;
    for _ in 0..100 {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(c) = connect() {
            return Ok(c);
        }
    }
    bail!("the daemon did not start; see {}", state::home().join("daemon.log").display())
}

fn call(cmd: &str, args: &[(&str, String)]) -> Result<()> {
    let (mut stream, info) = connect_or_start()?;
    let args: HashMap<String, String> = args.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let req = Request { token: info.token, cmd: cmd.to_string(), args };
    stream.write_all(format!("{}\n", serde_json::to_string(&req)?).as_bytes())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let resp: Response = serde_json::from_str(&line).map_err(|_| anyhow!("the daemon gave no valid answer"))?;
    if resp.ok {
        say!("{}", resp.text.trim_end());
        Ok(())
    } else {
        bail!("{}", resp.text)
    }
}

/// Sends the difference between the working tree and a base branch to a peer
/// and prints its findings.
fn review(with: String, base: String) -> Result<()> {
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git").args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let base = if base.is_empty() {
        git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "main".into())
    } else {
        base
    };
    let mut diff = git(&["diff", &base]).ok_or_else(|| anyhow!("could not diff against {base}; run this inside a git repository"))?;
    if diff.trim().is_empty() {
        bail!("nothing to review: no difference to {base}");
    }
    const MAX: usize = 150_000;
    let mut note = String::new();
    if diff.len() > MAX {
        let mut cut = MAX;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
        note = format!("\nThe diff was cut off after {MAX} bytes; say that your review covers only the part you saw.");
    }
    let question = format!(
        "Review this change (diff against {base}). You have your own clone of the project for context, but the change is not applied there; \
judge what the diff shows. Report findings ordered by severity, one per line as `file:line - problem - suggestion`. \
Report only what you are confident is wrong or risky, and say plainly if you find nothing.{note}\n\n```diff\n{diff}\n```"
    );
    call("ask", &[("question", question), ("with", with), ("project", String::new()), ("cwd", cwd())])
}

/// Prints events.log: the last 20 lines, or with `follow` every new line as it
/// is written. The plugin's monitor runs this to tell a session what happened.
fn events(follow: bool) -> Result<()> {
    let path = state::home().join("events.log");
    let show = |line: &str| {
        let mut parts = line.splitn(3, '\t');
        if let (Some(_), Some(kind), Some(text)) = (parts.next(), parts.next(), parts.next()) {
            say!("[agora] {kind}: {text}");
        }
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if !follow {
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            say!("no events yet");
        }
        lines[lines.len().saturating_sub(20)..].iter().for_each(|l| show(l));
        return Ok(());
    }
    let mut shown = text.len();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if text.len() > shown {
            text[shown..].lines().for_each(show);
            shown = text.len();
        }
    }
}

/// Prints what the rooms on this machine cost, oldest first.
fn stats() -> Result<()> {
    let mut rows: Vec<serde_json::Value> = Vec::new();
    if let Ok(dirs) = std::fs::read_dir(state::home().join("rooms")) {
        for d in dirs.flatten() {
            if let Ok(text) = std::fs::read_to_string(d.path().join("stats.json")) {
                if let Ok(v) = serde_json::from_str(&text) {
                    rows.push(v);
                }
            }
        }
    }
    if rows.is_empty() {
        say!("no closed rooms on this machine");
        return Ok(());
    }
    rows.sort_by_key(|v| v["closed_at"].as_u64().unwrap_or(0));
    say!("{:<9} {:<8} {:>4} {:>5} {:>9} {:>8} {:>10} {:>5}  {}", "room", "role", "msgs", "turns", "in/turn", "out", "all peers", "secs", "closed because");
    let (mut sum_in, mut sum_out) = (0u64, 0u64);
    for v in &rows {
        let n = |k: &str| v[k].as_u64().unwrap_or(0);
        let per_turn = if n("turns") > 0 { n("tokens_in") / n("turns") } else { 0 };
        let all = v["all_peers_tokens"].as_u64().map(|x| x.to_string()).unwrap_or_else(|| "-".into());
        say!(
            "{:<9} {:<8} {:>4} {:>5} {:>9} {:>8} {:>10} {:>5}  {}",
            v["room"].as_str().unwrap_or("?"),
            v["role"].as_str().unwrap_or("?"),
            n("messages"),
            n("turns"),
            per_turn,
            n("tokens_out"),
            all,
            n("duration_s"),
            v["reason"].as_str().unwrap_or("?")
        );
        sum_in += n("tokens_in");
        sum_out += n("tokens_out");
    }
    say!("\nThis machine's sessions in {} room(s): {sum_in} input tokens, {sum_out} output tokens.", rows.len());
    say!("\"in/turn\" is the context a session reads on every turn; \"all peers\" is known only for rooms opened here.");
    Ok(())
}

/// Prints a room's log as it grows, until the room has a result.
fn watch(room: &str) -> Result<()> {
    let dir = room::room_dir(room);
    let log = dir.join("log.md");
    if !log.exists() {
        bail!("no room {room} on this machine");
    }
    let mut shown = 0;
    loop {
        let done = dir.join("result.md").exists();
        let text = std::fs::read_to_string(&log)?;
        if text.len() > shown {
            print!("{}", &text[shown..]);
            std::io::stdout().flush()?;
            shown = text.len();
        }
        if done {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn stop() -> Result<()> {
    let Some(info) = read_info() else {
        say!("The daemon is not running.");
        return Ok(());
    };
    #[cfg(unix)]
    {
        std::process::Command::new("kill").arg("-INT").arg(info.pid.to_string()).status()?;
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new("taskkill").args(["/PID", &info.pid.to_string(), "/F"]).status()?;
        let _ = std::fs::remove_file(DaemonInfo::path());
    }
    say!("Stopped the daemon (pid {}).", info.pid);
    Ok(())
}
