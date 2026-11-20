//! Profiles: which program runs a session on this machine, and which model
//! service that program talks to. See docs/design/other-harnesses.md.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::proto;
use crate::state::{home, write_private};

/// The profile every machine has without configuring anything.
pub const BUILTIN: &str = "claude";

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    /// "claude": Claude Code in its headless mode. "acp": an agent that speaks
    /// the Agent Client Protocol on stdin and stdout.
    pub kind: String,
    /// Program and arguments. Empty for kind "claude" means `claude`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Model to ask the harness for; its own default if none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Kind "claude": the permission mode of sessions that may work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// Prompts go to a service other than Anthropic's or this machine.
    #[serde(default, skip_serializing_if = "is_false")]
    pub external: bool,
    /// Environment of the session. A value `env:NAME`, `file:PATH` or
    /// `cmd:COMMAND` is looked up when a session starts, so no key is kept here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Kind "acp": the agent's own mode id for a rights level ("talk", "read", "full").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modes: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<String>,
    #[serde(default)]
    profile: BTreeMap<String, Profile>,
}

pub fn path() -> PathBuf {
    home().join("harness.toml")
}

fn load() -> Result<File> {
    match std::fs::read_to_string(path()) {
        Ok(text) => toml::from_str(&text).map_err(|e| anyhow!("{} is not valid: {e}", path().display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
        Err(e) => Err(anyhow!("could not read {}: {e}", path().display())),
    }
}

fn save(file: &File) -> Result<()> {
    std::fs::create_dir_all(home())?;
    write_private(&path(), &toml::to_string_pretty(file)?)
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn check(name: &str, p: &Profile) -> Result<()> {
    match p.kind.as_str() {
        "claude" => Ok(()),
        "acp" if p.command.is_empty() => bail!("profile \"{name}\" is of kind acp and needs a command"),
        "acp" => Ok(()),
        other => bail!("profile \"{name}\" has the unknown kind \"{other}\"; known kinds are claude and acp"),
    }
}

/// The profile a session uses: the one named, else `AGORA_HARNESS`, else the
/// default of harness.toml, else Claude Code as it is installed.
pub fn resolve(name: Option<&str>) -> Result<(String, Profile)> {
    let file = load()?;
    let wanted = name
        .map(str::to_string)
        .or_else(|| std::env::var("AGORA_HARNESS").ok().filter(|s| !s.is_empty()))
        .or_else(|| file.default.clone())
        .unwrap_or_else(|| BUILTIN.to_string());
    match file.profile.get(&wanted) {
        Some(p) => {
            check(&wanted, p)?;
            Ok((wanted, p.clone()))
        }
        None if wanted == BUILTIN => Ok((wanted, Profile { kind: "claude".into(), ..Profile::default() })),
        None => bail!("no harness profile named \"{wanted}\"; see `agora harness list`"),
    }
}

impl Profile {
    /// Claude Code pointed at a model service that is not Anthropic's.
    pub fn other_backend(&self) -> bool {
        self.kind == "claude" && self.env.contains_key("ANTHROPIC_BASE_URL")
    }

    /// The session's environment with every reference looked up.
    pub fn environment(&self) -> Result<Vec<(String, String)>> {
        self.env.iter().map(|(k, v)| Ok((k.clone(), lookup(v).map_err(|e| anyhow!("{k}: {e}"))?))).collect()
    }

    /// What peers are told about this profile.
    pub fn info(&self, name: &str) -> proto::Harness {
        let model = self.model.clone().or_else(|| self.env.get("ANTHROPIC_MODEL").cloned());
        proto::Harness {
            name: name.to_string(),
            kind: self.kind.clone(),
            model,
            external: self.external,
            // Only Claude Code on Anthropic's own service reports how full the usage limit is.
            capacity: self.kind == "claude" && !self.other_backend(),
        }
    }
}

/// This machine's default profile, as peers see it.
pub fn own_info() -> Option<proto::Harness> {
    resolve(None).ok().map(|(name, p)| p.info(&name))
}

/// Looks up `env:NAME`, `file:PATH` and `cmd:COMMAND`; anything else is taken
/// as it is. An error never contains the value.
fn lookup(value: &str) -> Result<String> {
    if let Some(name) = value.strip_prefix("env:") {
        return std::env::var(name).map_err(|_| anyhow!("the environment variable {name} is not set where this runs"));
    }
    if let Some(p) = value.strip_prefix("file:") {
        let path = match p.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
            None => PathBuf::from(p),
        };
        let text = std::fs::read_to_string(&path).map_err(|e| anyhow!("could not read {}: {e}", path.display()))?;
        let text = text.trim();
        if text.is_empty() {
            bail!("{} is empty", path.display());
        }
        return Ok(text.to_string());
    }
    if let Some(command) = value.strip_prefix("cmd:") {
        let (shell, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
        let out = std::process::Command::new(shell)
            .args([flag, command])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map_err(|e| anyhow!("could not run the key command: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || text.is_empty() {
            bail!("the key command failed or printed nothing");
        }
        return Ok(text);
    }
    Ok(value.to_string())
}

/// Whether sessions of an external profile are refused in `dir`: a file
/// `.agora/no-external` in it or in a directory above it says so.
pub fn refuses_external(dir: &Path) -> bool {
    dir.ancestors().any(|a| a.join(".agora").join("no-external").exists())
}

/// Fails if a session of this profile must not run in `dir`.
pub fn allowed_in(name: &str, profile: &Profile, dir: &Path) -> Result<()> {
    if profile.external && refuses_external(dir) {
        bail!("this project refuses sessions that send it to an outside service (.agora/no-external), and the harness profile \"{name}\" does");
    }
    Ok(())
}

// ------------------------------------------------------------------ presets

/// A profile this program knows how to write. The values are what each
/// vendor's documentation said on 2026-11-20; see docs/design/other-harnesses.md.
pub struct Preset {
    pub name: &'static str,
    pub about: &'static str,
    kind: &'static str,
    command: &'static [&'static str],
    /// Kind "claude": the Anthropic-compatible address, "" if the user has to give one.
    url: &'static str,
    /// The main model and the one for small background tasks; "" if the user has to name one.
    model: &'static str,
    small: &'static str,
    /// A key that is the same for everyone (local servers); "" if the user has one of their own.
    fixed_key: &'static str,
    extra: &'static [(&'static str, &'static str)],
    external: bool,
}

const fn backend(name: &'static str, about: &'static str, url: &'static str, model: &'static str, small: &'static str) -> Preset {
    Preset { name, about, kind: "claude", command: &[], url, model, small, fixed_key: "", extra: &[], external: true }
}

const fn agent(name: &'static str, about: &'static str, command: &'static [&'static str]) -> Preset {
    Preset { name, about, kind: "acp", command, url: "", model: "", small: "", fixed_key: "", extra: &[], external: true }
}

pub const PRESETS: &[Preset] = &[
    backend("deepseek", "Claude Code on DeepSeek", "https://api.deepseek.com/anthropic", "deepseek-flash[1m]", "deepseek-flash"),
    backend("glm", "Claude Code on Z.ai GLM", "https://api.z.ai/api/anthropic", "glm-5.3[1m]", "glm-5.3-flash[1m]"),
    backend("kimi", "Claude Code on Moonshot Kimi", "https://api.moonshot.ai/anthropic", "kimi-k3[1m]", "kimi-k3[1m]"),
    backend("minimax", "Claude Code on MiniMax", "https://api.minimax.io/anthropic", "MiniMax-M3.1-Flash[1m]", "MiniMax-M3.1-Flash[1m]"),
    backend("qwen", "Claude Code on Alibaba Model Studio; needs --url with your workspace address", "", "qwen3.7-max", "qwen3.6-flash"),
    backend("openrouter", "Claude Code on any model of OpenRouter; needs --model", "https://openrouter.ai/api", "", ""),
    backend("anthropic-compatible", "Claude Code on any service that speaks Anthropic's Messages API; needs --url and --model", "", "", ""),
    Preset { fixed_key: "ollama", external: false, ..backend("ollama", "Claude Code on a local Ollama; needs --model", "http://localhost:11434", "", "") },
    Preset {
        fixed_key: "lmstudio",
        external: false,
        extra: &[("CLAUDE_CODE_ATTRIBUTION_HEADER", "0")],
        ..backend("lmstudio", "Claude Code on a local LM Studio; needs --model", "http://localhost:1234", "", "")
    },
    agent("opencode", "OpenCode", &["opencode", "acp"]),
    agent("gemini", "Gemini CLI", &["gemini", "--acp"]),
    agent("qwen-code", "Qwen Code", &["qwen", "--acp"]),
    agent("kimi-code", "Kimi Code", &["kimi", "acp"]),
    agent("goose", "Goose", &["goose", "acp"]),
    agent("cline", "Cline", &["cline", "--acp"]),
    agent("copilot", "GitHub Copilot CLI", &["copilot", "--acp", "--stdio"]),
    agent("cursor", "Cursor CLI", &["agent", "acp"]),
    agent("minimax-code", "MiniMax Code", &["mcode", "acp"]),
    agent("kiro", "Kiro CLI", &["kiro-cli", "acp"]),
    agent("vibe", "Mistral Vibe", &["vibe-acp"]),
    agent("codex-acp", "Codex CLI through its adapter; needs npx", &["npx", "-y", "@agentclientprotocol/codex-acp"]),
    agent("acp", "any other agent that speaks the Agent Client Protocol; needs --command", &[]),
];

/// What `agora harness add` was given besides the preset.
#[derive(Default)]
pub struct AddOptions {
    pub name: Option<String>,
    pub key: Option<String>,
    pub url: Option<String>,
    pub model: Option<String>,
    pub small_model: Option<String>,
    pub command: Option<String>,
    pub permission_mode: Option<String>,
    pub local: bool,
}

/// The permission modes of Claude Code a profile may name.
const PERMISSION_MODES: [&str; 6] = ["auto", "default", "acceptEdits", "dontAsk", "plan", "bypassPermissions"];

/// The variables that tell Claude Code which model to ask for.
const MODEL_VARS: [&str; 5] =
    ["ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_FABLE_MODEL", "CLAUDE_CODE_SUBAGENT_MODEL"];

fn is_local(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or("");
    if authority.starts_with("[::1]") {
        return true;
    }
    matches!(authority.split(':').next().unwrap_or(""), "localhost" | "127.0.0.1")
}

/// Builds the profile for a preset. Returns its name, the profile and notes for the user.
pub fn build(preset: &Preset, opt: &AddOptions) -> Result<(String, Profile, Vec<String>)> {
    let name = opt.name.clone().unwrap_or_else(|| preset.name.to_string());
    if !valid_name(&name) {
        bail!("a profile name has at most 32 letters, digits, - or _");
    }
    let mut notes = Vec::new();
    let mut p = Profile { kind: preset.kind.to_string(), external: preset.external && !opt.local, ..Profile::default() };
    if preset.kind == "acp" {
        p.command = match &opt.command {
            Some(c) => c.split_whitespace().map(str::to_string).collect(),
            None => preset.command.iter().map(|s| s.to_string()).collect(),
        };
        if p.command.is_empty() {
            bail!("give the agent's command with --command, for example --command \"opencode acp\"");
        }
        p.model = opt.model.clone();
        if let Some(key) = &opt.key {
            let (var, source) = key.split_once('=').ok_or_else(|| anyhow!("for an agent, --key takes VARIABLE=SOURCE, for example GEMINI_API_KEY=env:GEMINI_API_KEY"))?;
            p.env.insert(var.to_string(), source.to_string());
        }
        if p.external {
            notes.push("Marked external, because this program cannot know where the agent sends its prompts. Add --local if its model runs on this machine.".to_string());
        }
        if opt.permission_mode.is_some() {
            bail!("--permission-mode is for profiles that run Claude Code; an agent is given or refused each tool by the rights a room has");
        }
        return Ok((name, p, notes));
    }
    match opt.permission_mode.as_deref() {
        Some(mode) if !PERMISSION_MODES.contains(&mode) => bail!("--permission-mode must be one of {}", PERMISSION_MODES.join(", ")),
        Some("bypassPermissions") => {
            p.permission_mode = opt.permission_mode.clone();
            notes.push("With bypassPermissions every command the model writes runs on this machine without any check.".to_string());
        }
        Some(_) => p.permission_mode = opt.permission_mode.clone(),
        None => notes.push(
            "Claude Code's auto mode cannot run its safety check on another model service: a session of this profile can discuss, read and run harmless commands, \
but commands that need the check are refused, so it cannot finish a work package. To let it work unchecked, add the profile again with --permission-mode bypassPermissions."
                .to_string(),
        ),
    }
    let url = opt.url.clone().unwrap_or_else(|| preset.url.to_string());
    if url.is_empty() {
        bail!("this preset needs the service's address: --url <address>");
    }
    let model = opt.model.clone().unwrap_or_else(|| preset.model.to_string());
    if model.is_empty() {
        bail!("this preset needs a model: --model <name>");
    }
    let small = opt.small_model.clone().unwrap_or_else(|| if opt.model.is_some() || preset.small.is_empty() { model.clone() } else { preset.small.to_string() });
    let key = match (&opt.key, preset.fixed_key) {
        (Some(k), _) => k.clone(),
        (None, "") => bail!("say where the key comes from: --key env:NAME, --key file:PATH or --key \"cmd:COMMAND\""),
        (None, fixed) => fixed.to_string(),
    };
    if !["env:", "file:", "cmd:"].iter().any(|s| key.starts_with(s)) && preset.fixed_key.is_empty() {
        bail!("a key is not written into the profile; name its source instead: --key env:NAME, --key file:PATH or --key \"cmd:COMMAND\"");
    }
    if let Some(var) = key.strip_prefix("env:") {
        notes.push(format!("The daemon reads {var} from its own environment, so it has to be set where the daemon starts. A file or a keychain command works everywhere."));
    }
    p.env.insert("ANTHROPIC_BASE_URL".into(), url.clone());
    p.env.insert("ANTHROPIC_AUTH_TOKEN".into(), key);
    for var in MODEL_VARS {
        p.env.insert(var.into(), model.clone());
    }
    p.env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), small);
    p.env.insert("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(), "1".into());
    for (k, v) in preset.extra {
        p.env.insert(k.to_string(), v.to_string());
    }
    if is_local(&url) {
        p.external = false;
    }
    if p.external {
        notes.push("Marked external: everything a session of this profile reads is sent to that service. A file `.agora/no-external` in a project keeps such sessions out of it.".to_string());
        notes.push("Flat-rate coding plans of most providers exclude automated use; a key billed by usage does not have that restriction.".to_string());
    }
    Ok((name, p, notes))
}

pub fn add(name: &str, profile: Profile) -> Result<()> {
    if name == "default" {
        bail!("a profile cannot be called \"default\"");
    }
    check(name, &profile)?;
    let mut file = load()?;
    file.profile.insert(name.to_string(), profile);
    save(&file)
}

pub fn remove(name: &str) -> Result<()> {
    let mut file = load()?;
    if file.profile.remove(name).is_none() {
        bail!("no harness profile named \"{name}\"");
    }
    if file.default.as_deref() == Some(name) {
        file.default = None;
    }
    save(&file)
}

pub fn set_default(name: &str) -> Result<()> {
    let mut file = load()?;
    if name != BUILTIN && !file.profile.contains_key(name) {
        bail!("no harness profile named \"{name}\"");
    }
    file.default = if name == BUILTIN && !file.profile.contains_key(name) { None } else { Some(name.to_string()) };
    save(&file)
}

/// One line per profile, the default marked.
pub fn list() -> Result<String> {
    let file = load()?;
    let default = std::env::var("AGORA_HARNESS").ok().filter(|s| !s.is_empty()).or_else(|| file.default.clone()).unwrap_or_else(|| BUILTIN.to_string());
    let mut out = String::new();
    let mut line = |name: &str, what: String| {
        out.push_str(&format!("{} {:<20} {what}\n", if name == default { "*" } else { " " }, name));
    };
    if !file.profile.contains_key(BUILTIN) {
        line(BUILTIN, "Claude Code as it is installed here".to_string());
    }
    for (name, p) in &file.profile {
        let what = match p.kind.as_str() {
            "acp" => format!("agent `{}`", p.command.join(" ")),
            _ if p.other_backend() => format!("Claude Code on {}", p.env["ANTHROPIC_BASE_URL"]),
            _ => "Claude Code".to_string(),
        };
        let model = p.info(name).model.map(|m| format!(", model {m}")).unwrap_or_default();
        let mode = p.permission_mode.as_deref().map(|m| format!(", permission mode {m}")).unwrap_or_default();
        line(name, format!("{what}{model}{mode}{}", if p.external { ", external" } else { "" }));
    }
    out.push_str("\n* is the profile this machine's sessions use. Presets: `agora harness presets`.");
    Ok(out)
}

pub fn presets() -> String {
    let mut out = String::from("Presets for `agora harness add <preset>`:\n\n");
    for p in PRESETS {
        out.push_str(&format!("  {:<22} {}\n", p.name, p.about));
    }
    out.push_str("\nA model service needs --key with the source of your key: env:NAME, file:PATH or \"cmd:COMMAND\".");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str) -> &'static Preset {
        PRESETS.iter().find(|p| p.name == name).unwrap()
    }

    #[test]
    fn a_backend_preset_never_holds_a_key() {
        let opt = AddOptions { key: Some("sk-secret".into()), ..AddOptions::default() };
        assert!(build(preset("deepseek"), &opt).is_err());
        let opt = AddOptions { key: Some("env:DEEPSEEK_API_KEY".into()), ..AddOptions::default() };
        let (name, p, _) = build(preset("deepseek"), &opt).unwrap();
        assert_eq!(name, "deepseek");
        assert_eq!(p.env["ANTHROPIC_AUTH_TOKEN"], "env:DEEPSEEK_API_KEY");
        assert_eq!(p.env["ANTHROPIC_MODEL"], "deepseek-flash[1m]");
        assert_eq!(p.env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "deepseek-flash");
        assert!(p.external && p.other_backend());
        assert!(!p.info("deepseek").capacity);
        // No permission mode unless asked for, and only one Claude Code knows.
        assert_eq!(p.permission_mode, None);
        let opt = AddOptions { key: Some("env:K".into()), permission_mode: Some("bypassPermissions".into()), ..AddOptions::default() };
        assert_eq!(build(preset("deepseek"), &opt).unwrap().1.permission_mode.as_deref(), Some("bypassPermissions"));
        let opt = AddOptions { key: Some("env:K".into()), permission_mode: Some("anything".into()), ..AddOptions::default() };
        assert!(build(preset("deepseek"), &opt).is_err());
        assert!(build(preset("opencode"), &AddOptions { permission_mode: Some("auto".into()), ..AddOptions::default() }).is_err());
    }

    #[test]
    fn local_services_are_not_external() {
        let opt = AddOptions { model: Some("qwen3".into()), ..AddOptions::default() };
        let (_, p, _) = build(preset("ollama"), &opt).unwrap();
        assert!(!p.external);
        assert_eq!(p.env["ANTHROPIC_AUTH_TOKEN"], "ollama");
        assert_eq!(p.env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "qwen3");
        // A remote address under a local preset is external again.
        let opt = AddOptions { model: Some("m".into()), url: Some("https://example.org/v1".into()), key: Some("env:K".into()), ..AddOptions::default() };
        assert!(build(preset("anthropic-compatible"), &opt).unwrap().1.external);
        assert!(is_local("http://127.0.0.1:1234") && !is_local("https://localhost.example.org"));
    }

    #[test]
    fn presets_say_what_is_missing() {
        assert!(build(preset("openrouter"), &AddOptions { key: Some("env:K".into()), ..AddOptions::default() }).is_err());
        assert!(build(preset("qwen"), &AddOptions { key: Some("env:K".into()), ..AddOptions::default() }).is_err());
        assert!(build(preset("acp"), &AddOptions::default()).is_err());
        let (_, p, _) = build(preset("opencode"), &AddOptions::default()).unwrap();
        assert_eq!(p.command, ["opencode", "acp"]);
        assert_eq!(p.kind, "acp");
    }

    #[test]
    fn a_profile_survives_the_file() {
        let (_, p, _) = build(preset("glm"), &AddOptions { key: Some("file:~/.keys/glm".into()), ..AddOptions::default() }).unwrap();
        let mut file = File { default: Some("glm".into()), ..File::default() };
        file.profile.insert("glm".into(), p.clone());
        let back: File = toml::from_str(&toml::to_string_pretty(&file).unwrap()).unwrap();
        assert_eq!(back.profile["glm"], p);
        assert_eq!(back.default.as_deref(), Some("glm"));
    }

    #[test]
    fn references_are_looked_up_and_errors_keep_quiet() {
        assert_eq!(lookup("plain").unwrap(), "plain");
        let e = lookup("env:AGORA_TEST_NO_SUCH_VARIABLE").unwrap_err().to_string();
        assert!(e.contains("AGORA_TEST_NO_SUCH_VARIABLE"));
        #[cfg(unix)]
        {
            assert_eq!(lookup("cmd:printf abc").unwrap(), "abc");
            assert!(lookup("cmd:exit 3").is_err());
        }
    }

    #[test]
    fn a_marker_file_refuses_external_profiles() {
        let dir = std::env::temp_dir().join(format!("agora-noext-{}", std::process::id()));
        let inner = dir.join("a").join("b");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(!refuses_external(&inner));
        std::fs::create_dir_all(dir.join(".agora")).unwrap();
        std::fs::write(dir.join(".agora").join("no-external"), "").unwrap();
        assert!(refuses_external(&inner));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
