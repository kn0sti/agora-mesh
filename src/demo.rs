//! `agora demo`: a whole room on one machine, without an account or tokens.
//!
//! Two daemons with their own data directories connect over the real network
//! code, open a real room and work on real git branches. Only the agents are
//! stand-ins: this binary, started in place of `claude`, answers from a script.
//! Nothing outside a temporary directory is read or changed.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

/// Set for a session that is a scripted agent; its value is the agent's name.
const AGENT: &str = "AGORA_DEMO_AGENT";
const DELAY: &str = "AGORA_DEMO_DELAY_MS";
const OPENER: &str = "Laptop";
const JOINER: &str = "Desktop";
const TOPIC: &str = "Add a /health endpoint to the service, with a test";
const ROOM_TIMEOUT: Duration = Duration::from_secs(180);

const SERVICE: &str = "def hello():\n    return 200, {\"message\": \"hello\"}\n\n\nROUTES = {\"/\": hello}\n";
const SERVICE_DONE: &str = "def hello():\n    return 200, {\"message\": \"hello\"}\n\n\ndef health():\n    return 200, {\"status\": \"ok\"}\n\n\nROUTES = {\"/\": hello, \"/health\": health}\n";
const TEST: &str = "from service import ROUTES\n\n\ndef test_health():\n    assert ROUTES[\"/health\"]() == (200, {\"status\": \"ok\"})\n";

/// The directory to remove when the demo ends, also after Ctrl-C.
static WORKDIR: OnceLock<PathBuf> = OnceLock::new();

/// True if this process was started by a demo daemon in place of `claude`.
pub fn is_agent() -> bool {
    std::env::var_os(AGENT).is_some() && std::env::args_os().nth(1).is_some_and(|a| a == "-p")
}

// The scripted agent ---------------------------------------------------------

/// Speaks the part of Claude Code's stream-json protocol a room needs.
pub fn agent() -> Result<()> {
    let me = std::env::var(AGENT)?;
    let delay = std::env::var(DELAY).ok().and_then(|d| d.parse().ok()).unwrap_or(0);
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        std::thread::sleep(Duration::from_millis(delay));
        let text = reply(&me, &line);
        let result = serde_json::json!({
            "type": "result",
            "is_error": false,
            "result": text,
            "usage": { "input_tokens": 0, "output_tokens": 0 },
        });
        writeln!(out, "{result}")?;
        out.flush()?;
    }
    Ok(())
}

fn reply(me: &str, line: &str) -> String {
    let split = |returns: &str, expects: &str| format!("SPLIT_WORK\n- {OPENER}: the endpoint in service.py{returns}\n- {JOINER}: its test in test_service.py{expects}");
    if line.contains("The work phase is over") {
        "Both packages are done, one branch each. Review the two branches, then merge them.".into()
    } else if line.contains("Work phase. Package #") {
        work(me, line)
    } else if line.contains("Write the briefing") {
        "Goal: a /health endpoint with a test. Known: routes live in the ROUTES table in service.py. There are no tests yet.".into()
    } else if line.contains("First line: KNOWN_MISSING") {
        "KNOWN_MISSING\nI can read service.py. I am missing what /health should return.".into()
    } else if line.contains("fill the gaps") {
        "ANSWER\nStatus 200 and the body {\"status\": \"ok\"}.".into()
    } else if line.contains("you are its critic") {
        "CRITIQUE\nWeak point: the split does not fix the response. If each of us guesses, the endpoint and its test can disagree. Write the exact return value into both packages.".into()
    } else if line.contains("Your turn") {
        if line.contains("· CRITIQUE") {
            split(", returning (200, {\"status\": \"ok\"})", ", expecting (200, {\"status\": \"ok\"})")
        } else if line.contains("· SPLIT_WORK") {
            "CONSENSUS\nAgreed. The response is fixed, and the two packages touch different files.".into()
        } else {
            split("", "")
        }
    } else {
        "PASS".into()
    }
}

/// Does this agent's package for real: a branch, a file, a commit, a push.
fn work(me: &str, line: &str) -> String {
    let Some(branch) = line.split("on the branch `").nth(1).and_then(|r| r.split('`').next()) else {
        return "BLOCKED\nThe package names no branch.".into();
    };
    let (file, content) = if me == OPENER { ("service.py", SERVICE_DONE) } else { ("test_service.py", TEST) };
    let done = (|| -> Result<String> {
        git(Path::new("."), &["checkout", "-q", "-b", branch])?;
        std::fs::write(file, content)?;
        git(Path::new("."), &["add", file])?;
        git(Path::new("."), &["commit", "-q", "-m", &format!("Package of {me}: {file}"), "-m", "Next: nothing"])?;
        git(Path::new("."), &["push", "-q", "origin", branch])?;
        git(Path::new("."), &["rev-parse", "--short", "HEAD"])
    })();
    match done {
        Ok(commit) => format!("DONE\nWrote {file}, committed {commit} and pushed {branch}."),
        Err(e) => format!("BLOCKED\n{e}"),
    }
}

// Running the demo -----------------------------------------------------------

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).stdin(Stdio::null()).output().context("running git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct Style {
    on: bool,
}

impl Style {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.on { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
    }
    fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }
    fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }
    fn peer(&self, name: &str) -> String {
        let padded = format!("{name:<8}");
        self.paint(if name.starts_with(OPENER) { "36" } else { "35" }, &padded)
    }
}

/// One of the demo's two machines.
struct Peer {
    name: &'static str,
    home: PathBuf,
    root: PathBuf,
    delay: u64,
}

impl Peer {
    /// Git sees neither the user's settings (signing, hooks) nor a surrounding repository.
    fn isolate_git(&self, cmd: &mut Command) {
        cmd.env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
    }

    fn command(&self) -> Result<Command> {
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(&exe);
        cmd.env("AGORA_HOME", &self.home)
            .env("AGORA_CLAUDE", &exe)
            .env(AGENT, self.name)
            .env(DELAY, self.delay.to_string())
            .env_remove("AGORA_CLAUDE_ARGS")
            .env_remove("AGORA_PERMISSION_MODE");
        self.isolate_git(&mut cmd);
        Ok(cmd)
    }

    fn git(&self, dir: &Path, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("git");
        self.isolate_git(&mut cmd);
        let out = cmd.arg("-C").arg(dir).args(args).stdin(Stdio::null()).output().context("running git")?;
        if !out.status.success() {
            bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out: Output = self.command()?.args(args).stdin(Stdio::null()).stderr(Stdio::piped()).stdout(Stdio::piped()).output()?;
        if !out.status.success() {
            bail!("agora {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn log_tail(&self) -> String {
        let log = std::fs::read_to_string(self.home.join("daemon.log")).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        lines[lines.len().saturating_sub(8)..].join("\n")
    }
}

fn wait_until(what: &str, seconds: u64, mut ready: impl FnMut() -> bool) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        if ready() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    bail!("{what} did not happen within {seconds} s. The two local daemons find each other through iroh's public servers, so this needs an internet connection.")
}

fn online(status: &str, other: &str) -> bool {
    status.lines().any(|l| l.contains(other) && l.contains("online"))
}

fn remove_workdir() {
    let Some(dir) = WORKDIR.get() else { return };
    // On Windows a daemon that just stopped can hold its files for a moment.
    for _ in 0..10 {
        if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stops both daemons and removes the temporary directory, however the demo ends.
struct Cleanup<'a> {
    peers: &'a [Peer],
    keep: bool,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        for p in self.peers {
            let _ = p.run(&["stop"]);
        }
        if !self.keep {
            remove_workdir();
        }
    }
}

/// Stops the demo's daemons on Ctrl-C, so that none is left running.
fn on_interrupt(homes: Vec<PathBuf>, exe: PathBuf) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        if rt.block_on(tokio::signal::ctrl_c()).is_ok() {
            for home in homes {
                let _ = Command::new(&exe).arg("stop").env("AGORA_HOME", home).stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
            remove_workdir();
            std::process::exit(130);
        }
    });
}

pub fn run(fast: bool, keep: bool) -> Result<()> {
    if Command::new("git").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
        bail!("the demo needs git, and git was not found");
    }
    let style = Style { on: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() };
    let root = std::env::temp_dir().join(format!("agora-demo-{}-{:08x}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir(&root).with_context(|| format!("creating {}", root.display()))?;
    // Canonical, because git reports the real path (on macOS the temp dir is a
    // link). Not on Windows: there it is a `\\?\` path, which git cannot read.
    let root = if cfg!(windows) { root } else { root.canonicalize()? };
    let _ = WORKDIR.set(root.clone());
    std::fs::write(root.join("gitconfig"), "[user]\n\tname = agora demo\n\temail = demo@example.invalid\n[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n")?;

    let delay = if fast { 0 } else { 1200 };
    let peers = [
        Peer { name: OPENER, home: root.join("laptop"), root: root.clone(), delay },
        Peer { name: JOINER, home: root.join("desktop"), root: root.clone(), delay },
    ];
    on_interrupt(peers.iter().map(|p| p.home.clone()).collect(), std::env::current_exe()?);
    let cleanup = Cleanup { peers: &peers, keep };
    let result = play(&peers, &root, &style, fast);
    if let Err(e) = &result {
        eprintln!("\nThe demo stopped: {e}");
        for p in &peers {
            eprintln!("--- daemon log of {}\n{}", p.name, p.log_tail());
        }
    }
    drop(cleanup);
    result?;
    if keep {
        println!("{}", style.dim(&format!("Kept: {}", root.display())));
    }
    Ok(())
}

fn play(peers: &[Peer; 2], root: &Path, style: &Style, fast: bool) -> Result<()> {
    let [a, b] = peers;
    let pause = |ms: u64| {
        if !fast {
            std::thread::sleep(Duration::from_millis(ms));
        }
    };
    let step = |n: u32, text: &str| println!("\n{} {}", style.bold(&format!("[{n}/4]")), style.bold(text));

    println!("{}", style.bold("agora demo: two agents take one task from plan to pushed branches"));
    println!("{}", style.dim("The daemons, the encrypted link, the room and git are real. The agents are scripted: no model, no account, no tokens."));
    println!("{}", style.dim("Everything happens in a temporary folder and is removed afterwards."));

    step(1, "Two peers connect, both on this machine");
    for p in peers {
        p.command()?.args(["setup", "--name", p.name]).stdout(Stdio::null()).status()?;
    }
    let ticket = a.run(&["open"])?.split_whitespace().find(|w| w.starts_with("agora1")).map(str::to_string).ok_or_else(|| anyhow!("no network ID"))?;
    b.run(&["join", &ticket])?;
    let mut request = String::new();
    wait_until("the join request", 60, || {
        request = a.run(&["status"]).unwrap_or_default();
        request.contains("join request")
    })?;
    let words = request.lines().find(|l| l.contains("join request")).and_then(|l| l.split("words: ").nth(1)).unwrap_or("").trim().to_string();
    println!("  {} asks to join. Both screens show the same four words: {}", style.peer(JOINER).trim_end(), style.bold(&words));
    pause(900);
    a.run(&["approve", JOINER, "--level", "2"])?;
    wait_until("the connection", 60, || {
        online(&a.run(&["status"]).unwrap_or_default(), JOINER) && online(&b.run(&["status"]).unwrap_or_default(), OPENER)
    })?;
    b.run(&["trust", OPENER, "--level", "2"])?;
    println!("  The demo approves it for you. Normally this is your decision.");
    println!("  The link is encrypted end to end, and neither side is in charge.");
    pause(600);

    step(2, "Each peer has its own clone of the project");
    let origin = root.join("origin.git");
    let (clone_a, clone_b) = (root.join("clone-laptop"), root.join("clone-desktop"));
    let git_in = |dir: &Path, args: &[&str]| a.git(dir, args);
    let (origin_s, clone_a_s, clone_b_s) = (origin.to_string_lossy(), clone_a.to_string_lossy(), clone_b.to_string_lossy());
    git_in(root, &["init", "-q", "--bare", &origin_s])?;
    git_in(root, &["clone", "-q", &origin_s, &clone_a_s])?;
    std::fs::write(clone_a.join("service.py"), SERVICE)?;
    git_in(&clone_a, &["add", "service.py"])?;
    git_in(&clone_a, &["commit", "-q", "-m", "A small service"])?;
    git_in(&clone_a, &["push", "-q", "origin", "HEAD"])?;
    git_in(root, &["clone", "-q", &origin_s, &clone_b_s])?;
    b.run(&["project", "add", &clone_b_s])?;
    println!("  Task: {}", style.bold(TOPIC));
    pause(600);

    step(3, "They brief each other, argue, split the work and build it");
    let output = root.join("discuss.out");
    let mut child = a
        .command()?
        .args(["discuss", TOPIC, "--with", JOINER, "--turns", "4", "--remember", "none"])
        .current_dir(&clone_a)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&output)?)
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
    let mut shown = 0;
    let status = loop {
        shown = show_log(&a.home, shown, style);
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > ROOM_TIMEOUT {
            let _ = child.kill();
            bail!("the room did not finish within {} s", ROOM_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    show_log(&a.home, shown, style);
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            use std::io::Read;
            let _ = e.read_to_string(&mut err);
        }
        bail!("the room failed: {}", err.trim());
    }
    let closing = std::fs::read_to_string(&output)?;
    if !closing.contains("closed: work finished;") {
        bail!("the room did not finish its work:\n{}", closing.trim());
    }

    step(4, "The result is in git");
    let refs = git_in(&origin, &["for-each-ref", "--format=%(refname:short)", "refs/heads/agora"])?;
    let branches: Vec<&str> = refs.lines().collect();
    if branches.len() != 2 {
        bail!("expected two pushed branches, found {}", branches.len());
    }
    for branch in &branches {
        let commit = git_in(&origin, &["log", "-1", "--format=%h %s", branch])?;
        println!("  {}  {}", style.bold(branch), commit);
    }
    git_in(&clone_a, &["fetch", "-q", "origin"])?;
    git_in(&clone_a, &["checkout", "-q", "--detach", "origin/main"])?;
    for branch in &branches {
        git_in(&clone_a, &["merge", "-q", "--no-edit", &format!("origin/{branch}")])?;
    }
    let files = git_in(&clone_a, &["ls-files"])?;
    if !files.lines().any(|f| f == "test_service.py") || !std::fs::read_to_string(clone_a.join("service.py"))?.contains("/health") {
        bail!("the merged branches do not hold both packages");
    }
    println!("  Both branches merge without a conflict.");

    println!("\n{}", style.bold("That was a room."));
    println!("With real agents they read your code, disagree for real, and the plan is theirs.");
    println!("A room costs more than one agent, so it is for tasks large enough to split.\n");
    println!("Next, on two machines:");
    println!("  claude plugin install agora-mesh --marketplace kn0sti/agora-mesh");
    println!("  agora setup --name <this-machine>");
    println!("  agora open              {}", style.dim("# on one machine"));
    println!("  agora join <network-id> {}", style.dim("# on the other"));
    println!("{}", style.dim("A peer you connect can run commands on your machine. Connect only machines you trust."));
    Ok(())
}

/// Prints the log entries of the demo's room from entry `from` on. Returns how many there are.
fn show_log(home: &Path, from: usize, style: &Style) -> usize {
    let Some(log) = std::fs::read_dir(home.join("rooms")).ok().and_then(|mut d| d.next()).and_then(|e| e.ok()).map(|e| e.path().join("log.md")) else {
        return from;
    };
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let entries = parse_log(&text);
    for (name, kind, body) in entries.iter().skip(from) {
        println!("\n  {} {}", style.peer(name), style.bold(kind));
        for line in body.lines().filter(|l| !l.trim().is_empty()) {
            println!("           {line}");
        }
    }
    let _ = std::io::stdout().flush();
    entries.len().max(from)
}

/// Splits a room's log into (speaker, kind, text).
fn parse_log(log: &str) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in log.lines() {
        if line == "---" {
            break;
        }
        if let Some(head) = line.strip_prefix("[#").and_then(|l| l.strip_suffix(']')) {
            let mut parts = head.split(" · ");
            let name = parts.next().unwrap_or("").split_once(' ').map(|x| x.1).unwrap_or("").to_string();
            let kind = parts.next().unwrap_or("").to_string();
            out.push((name, kind, String::new()));
        } else if let Some(last) = out.last_mut() {
            last.2.push_str(line);
            last.2.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_room_log_is_split_into_entries() {
        let log = "# Room x\n\nTopic: t\n[#1 Laptop · BRIEF]\nGoal.\n[#2 Desktop · CRITIQUE · 3 left]\nWeak point.\nSecond line.\n\n---\nClosed: done.";
        let entries = parse_log(log);
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].0.as_str(), entries[0].1.as_str()), ("Laptop", "BRIEF"));
        assert_eq!((entries[1].0.as_str(), entries[1].1.as_str()), ("Desktop", "CRITIQUE"));
        assert!(entries[1].2.contains("Second line."));
        assert!(!entries[1].2.contains("Closed"));
    }

    #[test]
    fn the_script_answers_each_step_of_a_room() {
        assert!(reply(OPENER, "Write the briefing for Desktop").starts_with("Goal"));
        assert!(reply(JOINER, "Briefing: your turn. First line: KNOWN_MISSING.").starts_with("KNOWN_MISSING"));
        assert!(reply(JOINER, "Your turn (3 turns left).").starts_with("SPLIT_WORK"));
        assert!(reply(OPENER, "· SPLIT_WORK] Your turn (3 turns left). you are its critic").starts_with("CRITIQUE"));
        assert!(reply(JOINER, "· CRITIQUE] Your turn (2 turns left).").contains("200"));
        assert!(reply(OPENER, "· SPLIT_WORK] Your turn (2 turns left).").starts_with("CONSENSUS"));
    }
}
