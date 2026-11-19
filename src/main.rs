//! `agora` — command and daemon of agora-mesh. See ARCHITECTURE.md.

mod daemon;
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
        Cmd::Discuss { topic, with, turns, tokens, project } => {
            call("discuss", &[("topic", topic), ("with", with), ("turns", turns), ("tokens", tokens), ("project", project), ("cwd", cwd())])
        }
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
    State { name: name.to_string(), secret_key: data_encoding::HEXLOWER.encode(&key), networks: vec![], projects: vec![] }.save()?;
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
    say!("{:<9} {:<7} {:>4} {:>5} {:>9} {:>8} {:>10}  {}", "room", "role", "msgs", "turns", "in/turn", "out", "all peers", "closed because");
    let (mut sum_in, mut sum_out) = (0u64, 0u64);
    for v in &rows {
        let n = |k: &str| v[k].as_u64().unwrap_or(0);
        let per_turn = if n("turns") > 0 { n("tokens_in") / n("turns") } else { 0 };
        let all = v["all_peers_tokens"].as_u64().map(|x| x.to_string()).unwrap_or_else(|| "-".into());
        say!(
            "{:<9} {:<7} {:>4} {:>5} {:>9} {:>8} {:>10}  {}",
            v["room"].as_str().unwrap_or("?"),
            v["role"].as_str().unwrap_or("?"),
            n("messages"),
            n("turns"),
            per_turn,
            n("tokens_out"),
            all,
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
