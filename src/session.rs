//! A session: one agent process on this machine that is fed one message per
//! turn and answers with one text. Two kinds of program can be behind it:
//! Claude Code in its headless mode, and any agent that speaks the Agent
//! Client Protocol. The room logic does not know which.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::watch;

use crate::harness::{self, Profile};
use crate::proto::{self, Capacity};
use crate::state::home;

const TURN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Returns when the session is told to stop.
async fn stopped(cancel: &mut watch::Receiver<bool>) {
    if cancel.wait_for(|stop| *stop).await.is_err() {
        // Nobody can stop it any more: never return.
        std::future::pending::<()>().await;
    }
}

pub struct Session {
    child: Child,
    cancel: watch::Receiver<bool>,
    /// Set once the process was killed; its output can no longer be trusted.
    dead: bool,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    /// Set for an agent that speaks the Agent Client Protocol.
    acp: Option<Acp>,
    /// The name of the harness profile the session runs on.
    pub harness: String,
    /// The model the session says it runs on.
    pub model: Option<String>,
    pub turns: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Input tokens of the most recent turn.
    pub last_in: u64,
    /// The harness did not report its tokens; the numbers come from text length.
    pub estimated: bool,
    /// What the most recent turn reported about the usage limit.
    pub capacity: Option<Capacity>,
    pub timeout: Duration,
    /// Time spent waiting for the model.
    pub busy: Duration,
    /// What the room should be told about how the session started.
    pub notes: Vec<String>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
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

pub fn is_limit(e: &anyhow::Error) -> bool {
    e.downcast_ref::<LimitReached>().is_some()
}

/// Whether an error text means a usage limit. The exact wording differs per
/// service and is not verified for any of them; the usual phrases are accepted.
fn limit_wording(text: &str) -> bool {
    let t = text.to_lowercase();
    ["usage limit", "rate limit", "rate_limit", "limit reached", "hit your limit", "quota", "too many requests"].iter().any(|p| t.contains(p))
}

/// What every session is told about the room it takes part in.
fn rules(me: &str, others: &str, rights: &str) -> String {
    let means = if rights == "talk" {
        "You have no tools in this session: you cannot read files or run commands, so do not write tool calls; say what you would have to look up."
    } else {
        "The peers share a memory: `agora memory` lists it, `agora memory show <id>` prints an entry, and \
`agora remember \"<text>\" --title \"<title>\" --importance high|normal|low` saves a fact every peer should keep."
    };
    format!(
        "You are \"{me}\", one of several equal peers in an agora-mesh room. The other peers ({others}) are \
coding agents on other machines with their own files and knowledge; a human may read along. \
Think for yourself: question weak points and do not agree out of politeness. \
Reply format: the first line is exactly one of PROPOSAL, CRITIQUE, QUESTION, ANSWER, SPLIT_WORK, PASS, CONSENSUS, NEED_HUMAN; \
your message follows on the next lines. Keep it short. Refer to earlier messages as #number instead of repeating them. \
Send CONSENSUS only when you accept the latest proposal as it stands; whoever made a proposal has agreed to it and need not say so again. \
Send NEED_HUMAN when a human has to decide. \
Send PASS when you have nothing to add. \
To divide work, send SPLIT_WORK with one line per package in the form `- <peer name>: <package>`; \
once every other peer answers CONSENSUS, all peers work on their packages at the same time. \
Until a message starts the work phase and names your branch, do not change files, commit or push, even when a package is clearly yours. \
{means} \
A room starts with a short briefing; during it you are told which first line to use instead, and nobody proposes or decides anything yet."
    )
}

/// Lets the session call `agora` itself, for the shared memory.
fn agora_on_path(cmd: &mut Command) {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        let mut parts = vec![dir];
        parts.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        if let Ok(path) = std::env::join_paths(parts) {
            cmd.env("PATH", path);
        }
    }
}

/// `claude` in the headless mode the daemon talks to, with the profile's
/// program and environment.
fn claude_command(profile: &Profile) -> Result<Command> {
    let mut cmd = match profile.command.split_first() {
        Some((program, args)) => {
            let mut cmd = Command::new(program);
            cmd.args(args);
            cmd
        }
        None => Command::new(std::env::var("AGORA_CLAUDE").unwrap_or_else(|_| "claude".into())),
    };
    cmd.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]);
    agora_on_path(&mut cmd);
    cmd.envs(profile.environment()?);
    if profile.other_backend() {
        // A key for Anthropic that happens to be in the daemon's environment
        // must not be sent to another service: only the profile's own count.
        if !profile.env.contains_key("ANTHROPIC_API_KEY") {
            cmd.env("ANTHROPIC_API_KEY", "");
        }
        if !profile.env.contains_key("ANTHROPIC_AUTH_TOKEN") {
            cmd.env_remove("ANTHROPIC_AUTH_TOKEN");
        }
    }
    Ok(cmd)
}

impl Session {
    /// Starts a session on the harness profile `harness` (this machine's
    /// default if none) in `dir`, with the rights "talk", "read" or "full".
    /// `model` is the model the room asks for.
    pub fn start(harness: Option<&str>, dir: &Path, me: &str, others: &str, rights: &str, model: Option<&str>, cancel: watch::Receiver<bool>) -> Result<Session> {
        Self::open(harness, dir, me, others, rights, model, cancel, Stdio::null())
    }

    #[allow(clippy::too_many_arguments)]
    fn open(harness: Option<&str>, dir: &Path, me: &str, others: &str, rights: &str, model: Option<&str>, cancel: watch::Receiver<bool>, stderr: Stdio) -> Result<Session> {
        let (name, profile) = harness::resolve(harness)?;
        harness::allowed_in(&name, &profile, dir)?;
        let rules = rules(me, others, rights);
        let mut notes = Vec::new();
        let mut session = if profile.kind == "acp" {
            let mut cmd = Command::new(&profile.command[0]);
            cmd.args(&profile.command[1..]);
            agora_on_path(&mut cmd);
            cmd.envs(profile.environment()?);
            if model.is_some() {
                notes.push(format!("the harness \"{name}\" does not take the room's model and keeps its own"));
            }
            let mut s = Self::spawn(cmd, dir, cancel, stderr).map_err(|e| anyhow!("could not start `{}`: {e}", profile.command.join(" ")))?;
            s.acp = Some(Acp::new(rules, rights, dir, &profile));
            s
        } else {
            let mut cmd = claude_command(&profile)?;
            // Connectors cost far more context than the whole discussion; see ARCHITECTURE.md, sections 5 ("Sessions") and 11.
            cmd.arg("--strict-mcp-config");
            cmd.args(["--append-system-prompt", &rules]);
            let other = profile.other_backend();
            if profile.bare() {
                // What Claude Code loads by itself (CLAUDE.md files above the
                // directory, memory, plugins) stays on this machine, and a
                // request is a tenth of the size; see
                // docs/measurements/2026-11-claude-code-on-another-service.md.
                cmd.arg("--bare");
            }
            // The auto mode sends its settings with every request (paths, the
            // user's permission rules, the state of the repository) and cannot
            // run its check on another service, so it is never used there.
            match rights {
                "talk" => {
                    cmd.args(["--tools", ""]);
                    if other {
                        cmd.args(["--permission-mode", "default"]);
                    }
                }
                "read" => {
                    // A bare session has no search tools.
                    cmd.args(["--tools", if profile.bare() { "Read" } else { "Read,Grep,Glob" }]);
                    if other {
                        cmd.args(["--permission-mode", "default"]);
                    }
                }
                _ => {
                    let mode = profile
                        .permission_mode
                        .clone()
                        .or_else(|| std::env::var("AGORA_PERMISSION_MODE").ok().filter(|m| !(other && m == "auto")))
                        .unwrap_or_else(|| if other { "default" } else { "auto" }.into());
                    cmd.args(["--permission-mode", &mode]);
                    if other {
                        // Without the auto mode nothing would let the session use
                        // the shared memory that the rules of the room tell it about.
                        cmd.args(["--allowedTools", "Bash(agora memory),Bash(agora memory *),Bash(agora remember *)"]);
                    }
                }
            }
            if let Ok(extra) = std::env::var("AGORA_CLAUDE_ARGS") {
                cmd.args(extra.split_whitespace());
            }
            // The room's model comes last, so it wins over a model named in the
            // environment. It names a Claude model, so it means nothing on another service.
            let room_model = model.filter(|m| proto::valid_model(m));
            let model = if profile.other_backend() {
                if room_model.is_some() {
                    notes.push(format!("the harness \"{name}\" runs on another model service and keeps its own model"));
                }
                profile.model.as_deref()
            } else {
                room_model.or(profile.model.as_deref())
            };
            if let Some(model) = model {
                cmd.args(["--model", model]);
            }
            Self::spawn(cmd, dir, cancel, stderr).map_err(|e| anyhow!("could not start claude: {e}"))?
        };
        session.harness = name;
        session.notes = notes;
        Ok(session)
    }

    fn spawn(mut cmd: Command, dir: &Path, cancel: watch::Receiver<bool>, stderr: Stdio) -> Result<Session> {
        // Its own process group, so that stopping the session also stops the
        // commands it is running.
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr).kill_on_drop(true).spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        Ok(Session {
            child,
            cancel,
            dead: false,
            stdin,
            lines: BufReader::new(stdout).lines(),
            acp: None,
            harness: harness::BUILTIN.to_string(),
            model: None,
            turns: 0,
            tokens_in: 0,
            tokens_out: 0,
            last_in: 0,
            estimated: false,
            capacity: None,
            timeout: TURN_TIMEOUT,
            busy: Duration::ZERO,
            notes: Vec::new(),
        })
    }

    /// Sends one message and waits for the turn's final answer. The turn ends
    /// early when the room is stopped or the session takes too long; in both
    /// cases the process is killed, because a late answer would be taken for
    /// the answer to the next message.
    pub async fn turn(&mut self, text: &str) -> Result<String> {
        if self.dead {
            bail!("the session has ended");
        }
        let started = std::time::Instant::now();
        let limit = self.timeout;
        let mut cancel = self.cancel.clone();
        enum End {
            Answer(Result<String>),
            TimedOut,
            Stopped,
        }
        let end = {
            let answer = self.exchange(text);
            tokio::pin!(answer);
            tokio::select! {
                r = tokio::time::timeout(limit, &mut answer) => match r {
                    Ok(r) => End::Answer(r),
                    Err(_) => End::TimedOut,
                },
                _ = stopped(&mut cancel) => End::Stopped,
            }
        };
        self.busy += started.elapsed();
        match end {
            End::Answer(r) => r,
            End::TimedOut => {
                self.kill();
                bail!("the session did not answer within {} minutes", limit.as_secs() / 60)
            }
            End::Stopped => {
                self.kill();
                bail!("stopped by `agora panic`")
            }
        }
    }

    async fn exchange(&mut self, text: &str) -> Result<String> {
        if self.acp.is_some() {
            return self.acp_turn(text).await;
        }
        let msg = json!({"type": "user", "message": {"role": "user", "content": text}});
        self.write(&msg).await?;
        self.read_answer().await
    }

    async fn write(&mut self, v: &Value) -> Result<()> {
        if self.acp.is_some() {
            trace(">", &v.to_string());
        }
        self.stdin.write_all(format!("{v}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Ends the session and everything it started.
    fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // A negative pid addresses the process group the session leads.
            let _ = std::process::Command::new("kill").args(["-KILL", "--", &format!("-{pid}")]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        let _ = self.child.start_kill();
        self.dead = true;
    }

    /// Claude Code: reads the output up to the result of the current turn.
    async fn read_answer(&mut self) -> Result<String> {
        while let Some(line) = self.lines.next_line().await? {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["type"] == "rate_limit_event" {
                if let Some(c) = Capacity::from_event(&v["rate_limit_info"]) {
                    self.capacity = Some(c);
                }
                continue;
            }
            if v["type"] == "system" && v["subtype"] == "init" {
                if let Some(model) = v["model"].as_str() {
                    self.model = Some(model.to_string());
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
                if v["api_error_status"].as_u64() == Some(429) || limit_wording(&text) {
                    let resets_at = self.capacity.map(|c| c.resets_at).unwrap_or(0);
                    self.capacity = Some(Capacity { utilization: 1.0, resets_at, limited: true });
                    return Err(LimitReached.into());
                }
                bail!("the session reported an error: {text}");
            }
            return Ok(text);
        }
        self.dead = true;
        bail!("the session ended unexpectedly")
    }

    pub fn tokens(&self) -> u64 {
        self.tokens_in + self.tokens_out
    }

    pub fn stats(&self) -> String {
        let per_turn = if self.turns > 0 { self.tokens_in / self.turns as u64 } else { 0 };
        format!(
            "{} turns, {} input tokens ({} per turn), {} output tokens{}, {:.0} s model time",
            self.turns,
            self.tokens_in,
            per_turn,
            self.tokens_out,
            if self.estimated { " (estimated: the harness does not report them)" } else { "" },
            self.busy.as_secs_f64()
        )
    }
}

// --------------------------------------------------- Agent Client Protocol

/// What a session needs to remember to drive an agent over the Agent Client
/// Protocol, version 1: JSON-RPC 2.0, one message per line.
struct Acp {
    /// Sent with the first message. The agent keeps it in its context from then on.
    rules: Option<String>,
    rights: String,
    dir: PathBuf,
    model: Option<String>,
    /// The agent's own mode for this session's rights, if the profile names one.
    mode: Option<String>,
    session_id: String,
    next_id: u64,
    /// The agent's text in the current turn, and the part after its last tool call.
    full: String,
    tail: String,
    /// The context size the agent last reported.
    context: u64,
    /// What the agent reported last, and what that made for the turn.
    seen_in: u64,
    seen_out: u64,
    turn_in: u64,
    /// Whether the agent reports a running total of the session instead of
    /// the tokens of the turn. Found out on its second report.
    running_total: Option<bool>,
}

impl Acp {
    fn new(rules: String, rights: &str, dir: &Path, profile: &Profile) -> Acp {
        Acp {
            rules: Some(rules),
            rights: rights.to_string(),
            dir: std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()),
            model: profile.model.clone(),
            mode: profile.modes.get(rights).cloned(),
            session_id: String::new(),
            next_id: 1,
            full: String::new(),
            tail: String::new(),
            context: 0,
            seen_in: 0,
            seen_out: 0,
            turn_in: 0,
            running_total: None,
        }
    }

    /// The tokens of one turn from what the agent reported at its end.
    /// Agents differ: OpenCode reports the turn, the protocol's draft
    /// describes a total of the session. A total grows by about a whole
    /// context with every turn; the tokens of a turn stay close to those of
    /// the turn before. The second report tells the two apart, and the answer
    /// is kept for the session.
    fn share(&mut self, reported_in: u64, reported_out: u64) -> (u64, u64) {
        if self.running_total.is_none() && self.turn_in > 0 {
            self.running_total = Some(reported_in >= self.seen_in + self.turn_in / 2);
        }
        let turn = if self.running_total == Some(true) {
            (reported_in.saturating_sub(self.seen_in), reported_out.saturating_sub(self.seen_out))
        } else {
            (reported_in, reported_out)
        };
        self.seen_in = reported_in;
        self.seen_out = reported_out;
        self.turn_in = turn.0;
        turn
    }

    /// Answers the agent's question whether it may use a tool. The rights
    /// decide; the option is chosen by its kind, never by its label.
    fn permission(&self, params: &Value) -> Value {
        let kind = params["toolCall"]["kind"].as_str().unwrap_or("");
        let allow = match self.rights.as_str() {
            "talk" => false,
            "read" => matches!(kind, "read" | "search" | "think"),
            _ => true,
        };
        let wanted: [&str; 2] = if allow { ["allow_once", "allow_always"] } else { ["reject_once", "reject_always"] };
        let options = params["options"].as_array().cloned().unwrap_or_default();
        for kind in wanted {
            if let Some(o) = options.iter().find(|o| o["kind"] == kind) {
                return json!({"outcome": {"outcome": "selected", "optionId": o["optionId"]}});
            }
        }
        json!({"outcome": {"outcome": "cancelled"}})
    }

    fn update(&mut self, update: &Value) {
        match update["sessionUpdate"].as_str() {
            Some("agent_message_chunk") => {
                if let Some(t) = update["content"]["text"].as_str() {
                    self.full.push_str(t);
                    self.tail.push_str(t);
                }
            }
            // What the agent said before a tool call is not its answer.
            Some("tool_call") => self.tail.clear(),
            Some("usage_update") => {
                if let Some(n) = update["used"].as_u64() {
                    self.context = n;
                }
            }
            _ => {}
        }
    }
}

/// Appends one line of the conversation with an agent to the file named in
/// `AGORA_ACP_TRACE`, for finding out what an agent really sends. The file
/// holds the prompts and answers in full, so it is only written on request.
fn trace(direction: &str, line: &str) {
    use std::io::Write;
    let Some(path) = std::env::var_os("AGORA_ACP_TRACE") else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{direction} {line}");
    }
}

/// The config option of a category ("model", "mode") in what the agent offers.
fn config_option<'a>(options: &'a Value, category: &str) -> Option<&'a Value> {
    options.as_array()?.iter().find(|o| o["category"] == category || o["id"] == category)
}

/// Every value a select option offers, also inside groups.
fn option_values(option: &Value, out: &mut Vec<(String, String)>) {
    if let Some(list) = option["options"].as_array() {
        for o in list {
            if let Some(v) = o["value"].as_str() {
                out.push((v.to_string(), o["name"].as_str().unwrap_or(v).to_string()));
            }
            option_values(o, out);
        }
    }
}

/// The offered value that `wanted` means: the same text, or the only one that contains it.
fn pick_value(option: &Value, wanted: &str) -> Option<String> {
    let mut all = Vec::new();
    option_values(option, &mut all);
    let w = wanted.to_lowercase();
    if let Some((v, _)) = all.iter().find(|(v, n)| v.to_lowercase() == w || n.to_lowercase() == w) {
        return Some(v.clone());
    }
    let mut partial = all.iter().filter(|(v, n)| v.to_lowercase().contains(&w) || n.to_lowercase().contains(&w));
    match (partial.next(), partial.next()) {
        (Some((v, _)), None) => Some(v.clone()),
        _ => None,
    }
}

fn acp_error(method: &str, e: &Value) -> anyhow::Error {
    let message = e["message"].as_str().unwrap_or("no message");
    let detail = e["data"].as_str().map(|d| format!(" ({d})")).unwrap_or_default();
    if e["code"].as_i64() == Some(-32000) {
        return anyhow!("the agent needs a login first: start it once yourself and sign in ({message})");
    }
    if limit_wording(message) || limit_wording(&detail) {
        return LimitReached.into();
    }
    anyhow!("the agent answered {method} with an error: {message}{detail}")
}

impl Session {
    fn acp_mut(&mut self) -> &mut Acp {
        self.acp.as_mut().expect("an ACP session")
    }

    /// Sends a request and reads until its answer arrives. Whatever the agent
    /// sends in between (updates, its own requests) is handled on the way.
    async fn acp_call(&mut self, method: &str, params: Value) -> Result<Value> {
        let acp = self.acp_mut();
        let id = acp.next_id;
        acp.next_id += 1;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await?;
        while let Some(line) = self.lines.next_line().await? {
            trace("<", &line);
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("method").is_some() {
                self.acp_incoming(&v).await?;
                continue;
            }
            if v["id"].as_u64() != Some(id) {
                continue;
            }
            return match v.get("error").filter(|e| !e.is_null()) {
                Some(e) => Err(acp_error(method, e)),
                None => Ok(v["result"].clone()),
            };
        }
        self.dead = true;
        bail!("the agent ended unexpectedly")
    }

    async fn acp_incoming(&mut self, v: &Value) -> Result<()> {
        let id = v.get("id").filter(|i| !i.is_null()).cloned();
        match (v["method"].as_str().unwrap_or(""), id) {
            ("session/update", _) => self.acp_mut().update(&v["params"]["update"]),
            ("session/request_permission", Some(id)) => {
                let result = self.acp_mut().permission(&v["params"]);
                self.write(&json!({"jsonrpc": "2.0", "id": id, "result": result})).await?;
            }
            // Reading and writing files and running commands for the agent are
            // not offered: it uses its own tools, like every other harness.
            (_, Some(id)) => self.write(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})).await?,
            _ => {}
        }
        Ok(())
    }

    /// The handshake: version, a new session without connectors, model and mode.
    async fn acp_open(&mut self) -> Result<()> {
        let init = self
            .acp_call(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
                    "clientInfo": {"name": "agora-mesh", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        if let Some(version) = init["protocolVersion"].as_u64().filter(|v| *v != 1) {
            bail!("the agent speaks version {version} of the Agent Client Protocol; agora-mesh speaks version 1");
        }
        let dir = self.acp_mut().dir.clone();
        let new = self.acp_call("session/new", json!({"cwd": dir, "mcpServers": []})).await?;
        let Some(session_id) = new["sessionId"].as_str().map(str::to_string) else { bail!("the agent opened no session") };
        self.acp_mut().session_id = session_id.clone();
        let options = new["configOptions"].clone();

        if let Some(wanted) = self.acp_mut().model.clone() {
            let found = config_option(&options, "model").and_then(|o| Some((o["id"].as_str()?.to_string(), pick_value(o, &wanted)?)));
            match found {
                Some((config, value)) => {
                    self.acp_call("session/set_config_option", json!({"sessionId": session_id, "configId": config, "value": value})).await?;
                    self.model = Some(value);
                }
                None => self.notes.push(format!("the agent offers no model \"{wanted}\" and keeps its own")),
            }
        }
        if self.model.is_none() {
            let current = config_option(&options, "model").and_then(|o| o["currentValue"].as_str()).or_else(|| new["models"]["currentModelId"].as_str());
            self.model = current.map(str::to_string);
        }
        // A mode the profile names for these rights is a promise about what the
        // session may do, so failing to set it ends the session.
        if let Some(mode) = self.acp_mut().mode.clone() {
            match config_option(&options, "mode").and_then(|o| o["id"].as_str()) {
                Some(config) => self.acp_call("session/set_config_option", json!({"sessionId": session_id, "configId": config, "value": mode})).await?,
                None => self.acp_call("session/set_mode", json!({"sessionId": session_id, "modeId": mode})).await?,
            };
        }
        Ok(())
    }

    async fn acp_turn(&mut self, text: &str) -> Result<String> {
        if self.acp_mut().session_id.is_empty() {
            self.acp_open().await?;
        }
        let acp = self.acp_mut();
        acp.full.clear();
        acp.tail.clear();
        let text = match acp.rules.take() {
            Some(rules) => format!("{rules}\n\n{text}"),
            None => text.to_string(),
        };
        let session_id = acp.session_id.clone();
        let result = self.acp_call("session/prompt", json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]})).await?;

        let acp = self.acp.as_mut().expect("an ACP session");
        let answer = if acp.tail.trim().is_empty() { acp.full.trim() } else { acp.tail.trim() }.to_string();
        let usage = &result["usage"];
        let (tokens_in, tokens_out) = if usage.is_object() {
            let n = |k: &str| usage[k].as_u64().unwrap_or(0);
            // Input that came from the cache is reported apart from the rest.
            acp.share(n("inputTokens") + n("cachedReadTokens") + n("cachedWriteTokens"), n("outputTokens"))
        } else {
            self.estimated = true;
            let read = if acp.context > 0 { acp.context } else { text.len() as u64 / 4 };
            (read, answer.len() as u64 / 4)
        };
        self.last_in = tokens_in;
        self.tokens_in += tokens_in;
        self.tokens_out += tokens_out;
        self.turns += 1;
        match result["stopReason"].as_str() {
            Some("cancelled") => bail!("the agent's turn was cancelled"),
            Some("refusal") if answer.is_empty() => bail!("the agent refused to answer"),
            _ => Ok(answer),
        }
    }
}

// ------------------------------------------------------------ measurements

fn never() -> watch::Receiver<bool> {
    // The sender is dropped, so the session can only end by itself.
    watch::channel(false).1
}

/// Measures what one turn costs on this machine before anyone has said
/// anything. Uses real sessions.
pub async fn bench(harness: Option<&str>) -> Result<String> {
    let dir = home().join("bench");
    std::fs::create_dir_all(&dir)?;
    let (name, profile) = harness::resolve(harness)?;
    let mut out = String::from("Input tokens of one turn that only says one word, measured on this machine:\n\n");
    if profile.kind == "claude" && !profile.other_backend() {
        // Claude Code itself: also show what the room's way of starting it saves.
        let variants: [(&str, &[&str]); 3] = [
            ("default headless session, tools off", &["--tools", ""]),
            ("without connectors, tools off", &["--strict-mcp-config", "--tools", ""]),
            ("room session (no connectors, built-in tools)", &["--strict-mcp-config"]),
        ];
        for (label, args) in variants {
            let mut cmd = claude_command(&profile)?;
            cmd.args(["--model", "haiku", "--no-session-persistence"]);
            cmd.args(args);
            let mut s = Session::spawn(cmd, &dir, never(), Stdio::null()).map_err(|e| anyhow!("could not start claude: {e}"))?;
            s.turn("Reply with only the word: one").await?;
            // Connectors finish loading after the first turn; the second turn shows the real cost.
            tokio::time::sleep(Duration::from_secs(10)).await;
            s.turn("Reply with only the word: two").await?;
            out.push_str(&format!("  {:>9}  {label}\n", s.last_in));
        }
    } else {
        let variants: &[(&str, &str)] = if profile.kind == "acp" { &[("full", "room session")] } else { &[("talk", "room session without tools"), ("full", "room session with tools")] };
        for (rights, label) in variants {
            let mut s = Session::start(Some(&name), &dir, "bench", "nobody", rights, None, never())?;
            s.turn("Reply with only the word: one").await?;
            s.turn("Reply with only the word: two").await?;
            let how = if s.estimated { " (estimated: the harness does not report its tokens)" } else { "" };
            out.push_str(&format!("  {:>9}  {label}, harness \"{name}\"{how}\n", s.last_in));
        }
    }
    out.push_str("\nA session reads this again on every turn; where the service caches, most of it comes from the cache after the first turn.");
    Ok(out)
}

/// Runs two turns on a profile and says what worked, so that its abilities
/// are found out and not assumed. Uses a real session; the agent's own error
/// output goes to the terminal.
pub async fn try_out(harness: Option<&str>) -> Result<String> {
    let dir = home().join("bench");
    std::fs::create_dir_all(&dir)?;
    let (name, profile) = harness::resolve(harness)?;
    let started = std::time::Instant::now();
    let mut s = Session::open(Some(&name), &dir, "test", "nobody", "talk", None, never(), Stdio::inherit())?;
    let first = s.turn("This is a connection test. Reply with ANSWER on the first line and only the word apple on the second.").await?;
    let first_in = s.last_in;
    let second = s.turn("Which word did you reply with a moment ago? Reply with ANSWER on the first line and only that word on the second.").await?;
    let (kind, _) = proto::parse_kind(&first, "", &proto::KINDS);
    let yes_no = |b: bool| if b { "yes" } else { "no" };
    let mut out = format!("harness \"{name}\" ({})\n", profile.kind);
    out.push_str(&format!("  answers:                 {}\n", yes_no(first.to_lowercase().contains("apple"))));
    out.push_str(&format!("  keeps to the room format: {}\n", yes_no(kind == "ANSWER")));
    out.push_str(&format!("  remembers the last turn: {}\n", yes_no(second.to_lowercase().contains("apple"))));
    out.push_str(&format!("  model:                   {}\n", s.model.as_deref().unwrap_or("not reported")));
    out.push_str(&format!(
        "  tokens:                  {}, {first_in} read in the first turn, {} in the second\n",
        if s.estimated { "not reported per turn (the size of the agent's context where it reports one, else estimated from text length)" } else { "reported" },
        s.last_in
    ));
    out.push_str(&format!("  usage limit:             {}\n", s.capacity.map(|c| c.describe()).unwrap_or_else(|| "not reported".into())));
    out.push_str(&format!("  time for both turns:     {:.1} s", started.elapsed().as_secs_f64()));
    for note in &s.notes {
        out.push_str(&format!("\n  note: {note}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acp(rights: &str) -> Acp {
        Acp::new(String::new(), rights, Path::new("."), &Profile::default())
    }

    fn ask(kind: &str) -> Value {
        json!({"toolCall": {"toolCallId": "t1", "kind": kind}, "options": [
            {"optionId": "yes-always", "kind": "allow_always", "name": "Reject"},
            {"optionId": "yes", "kind": "allow_once", "name": "Reject"},
            {"optionId": "no", "kind": "reject_once", "name": "Allow"},
        ]})
    }

    #[test]
    fn rights_decide_what_an_agent_may_do() {
        // The labels lie on purpose: only the kind of an option counts.
        assert_eq!(acp("full").permission(&ask("execute"))["outcome"]["optionId"], "yes");
        assert_eq!(acp("talk").permission(&ask("read"))["outcome"]["optionId"], "no");
        assert_eq!(acp("read").permission(&ask("read"))["outcome"]["optionId"], "yes");
        assert_eq!(acp("read").permission(&ask("edit"))["outcome"]["optionId"], "no");
        assert_eq!(acp("read").permission(&ask("execute"))["outcome"]["optionId"], "no");
        // A tool of unknown kind is not a reading tool.
        assert_eq!(acp("read").permission(&json!({"toolCall": {}, "options": ask("x")["options"]}))["outcome"]["optionId"], "no");
        // Nothing to refuse with: the request is cancelled, never allowed.
        let only_allow = json!({"toolCall": {"kind": "edit"}, "options": [{"optionId": "yes", "kind": "allow_once"}]});
        assert_eq!(acp("talk").permission(&only_allow)["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn the_answer_is_what_follows_the_last_tool_call() {
        let mut a = acp("full");
        let say = |t: &str| json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": t}});
        a.update(&say("Let me look. "));
        a.update(&json!({"sessionUpdate": "tool_call", "toolCallId": "t1"}));
        a.update(&json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"}));
        a.update(&say("ANSWER\n"));
        a.update(&say("four"));
        a.update(&json!({"sessionUpdate": "usage_update", "used": 1234, "size": 200000}));
        assert_eq!(a.tail, "ANSWER\nfour");
        assert_eq!(a.full, "Let me look. ANSWER\nfour");
        assert_eq!(a.context, 1234);
    }

    #[test]
    fn tokens_of_a_turn_whichever_way_the_agent_counts() {
        // As OpenCode reported them in a real run: the tokens of each turn.
        let mut a = acp("full");
        assert_eq!(a.share(11264, 4), (11264, 4));
        assert_eq!(a.share(11301, 5), (11301, 5));
        assert_eq!(a.share(11950, 40), (11950, 40));
        // A running total of the session.
        let mut a = acp("full");
        assert_eq!(a.share(10000, 20), (10000, 20));
        assert_eq!(a.share(20400, 45), (10400, 25));
        assert_eq!(a.share(31000, 80), (10600, 35));
        // Decided on the second report and kept: a total that later grows slowly stays a total.
        assert_eq!(a.share(31200, 90), (200, 10));
    }

    #[test]
    fn a_model_is_picked_from_what_the_agent_offers() {
        let option = json!({"id": "model", "category": "model", "currentValue": "a/small", "options": [
            {"value": "a/small", "name": "Small"},
            {"group": "big", "options": [{"value": "b/large-1", "name": "Large"}, {"value": "b/huge", "name": "Huge"}]},
        ]});
        assert_eq!(pick_value(&option, "b/huge").as_deref(), Some("b/huge"));
        assert_eq!(pick_value(&option, "LARGE").as_deref(), Some("b/large-1"));
        assert_eq!(pick_value(&option, "small").as_deref(), Some("a/small"));
        // "a" fits several: no guess.
        assert_eq!(pick_value(&option, "a"), None);
        assert_eq!(pick_value(&option, "missing"), None);
        assert!(config_option(&json!([option]), "model").is_some());
        assert!(config_option(&json!([option]), "mode").is_none());
    }

    #[test]
    fn limit_errors_are_recognised() {
        assert!(is_limit(&acp_error("session/prompt", &json!({"code": -32603, "message": "429 Too Many Requests"}))));
        assert!(is_limit(&acp_error("session/prompt", &json!({"code": -32603, "message": "Internal error", "data": "You exceeded your current quota"}))));
        assert!(!is_limit(&acp_error("session/prompt", &json!({"code": -32603, "message": "Internal error"}))));
        assert!(acp_error("session/new", &json!({"code": -32000, "message": "Authentication required"})).to_string().contains("login"));
    }
}
