//! `agora` — command and daemon of agora-mesh. See ARCHITECTURE.md.

mod daemon;
mod proto;
mod state;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};

use daemon::{DaemonInfo, Request, Response};
use state::State;

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
    /// Stop the daemon
    Stop,
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
    State { name: name.to_string(), secret_key: data_encoding::HEXLOWER.encode(&key), networks: vec![] }.save()?;
    println!("Set up peer \"{name}\" with id {public}");
    println!("Data directory: {}", state::home().display());
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
        println!("{}", resp.text.trim_end());
        Ok(())
    } else {
        bail!("{}", resp.text)
    }
}

fn stop() -> Result<()> {
    let Some(info) = read_info() else {
        println!("The daemon is not running.");
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
    println!("Stopped the daemon (pid {}).", info.pid);
    Ok(())
}
