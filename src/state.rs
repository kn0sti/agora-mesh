//! Persistent state of one daemon: identity, networks, peers.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::proto::Member;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NetStatus {
    Active,
    /// We sent a join request and wait for a human on the other side.
    Joining,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Network {
    pub id: String,
    pub secret: String,
    pub status: NetStatus,
    /// Peer we dial while joining.
    pub opener: String,
    /// All other members (never contains ourselves).
    pub peers: Vec<Member>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub name: String,
    pub secret_key: String,
    #[serde(default)]
    pub networks: Vec<Network>,
    /// Local clones a room session may be started in, see project.rs.
    #[serde(default)]
    pub projects: Vec<String>,
    /// Terms that must never be written to the shared memory from this machine.
    #[serde(default)]
    pub blocklist: Vec<String>,
    /// Obsidian vault folder that mirrors the shared memory.
    #[serde(default)]
    pub obsidian: Option<String>,
}

/// Data directory. `AGORA_HOME` overrides it, which lets two daemons run on one machine.
pub fn home() -> PathBuf {
    match std::env::var_os("AGORA_HOME") {
        Some(p) => PathBuf::from(p),
        None => dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".agora"),
    }
}

impl State {
    pub fn path() -> PathBuf {
        home().join("state.json")
    }

    pub fn load() -> Result<Option<State>> {
        let p = Self::path();
        if !p.exists() {
            return Ok(None);
        }
        let s = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Ok(Some(serde_json::from_str(&s).with_context(|| format!("parsing {}", p.display()))?))
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(home())?;
        let p = Self::path();
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    pub fn network_mut(&mut self, id: &str) -> Option<&mut Network> {
        self.networks.iter_mut().find(|n| n.id == id)
    }

    pub fn network(&self, id: &str) -> Option<&Network> {
        self.networks.iter().find(|n| n.id == id)
    }
}
