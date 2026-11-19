//! Shared memory: one small set of entries per network, replicated to every peer.
//!
//! Every entry is signed by its author. When two peers change the same entry,
//! the one with the higher clock wins and the older version is kept in
//! `history/`. Entries live as Markdown files that agents and humans read
//! directly; `entries.json` next to them is what the daemon replicates.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use iroh::{EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::state::home;

pub const IMPORTANCE: [&str; 3] = ["high", "normal", "low"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemEntry {
    pub network: String,
    pub id: String,
    /// Logical clock; a change is accepted when (clock, author) is higher.
    pub clock: u64,
    /// Peer id of the author.
    pub author: String,
    pub author_name: String,
    pub importance: String,
    pub title: String,
    pub body: String,
    /// A deleted entry stays as a marker so the deletion replicates.
    #[serde(default)]
    pub deleted: bool,
    pub sig: String,
}

impl MemEntry {
    fn digest(&self) -> [u8; 32] {
        let text = format!(
            "agora-mem|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.network, self.id, self.clock, self.author, self.author_name, self.importance, self.deleted, self.title, self.body
        );
        *blake3::hash(text.as_bytes()).as_bytes()
    }

    pub fn sign(&mut self, key: &SecretKey) {
        self.sig = data_encoding::HEXLOWER.encode(&key.sign(&self.digest()).to_bytes());
    }

    /// Checks that the entry was signed by the peer named as its author.
    pub fn verify(&self) -> Result<()> {
        let author: EndpointId = self.author.parse().map_err(|_| anyhow!("entry {} has an invalid author", self.id))?;
        let bytes = data_encoding::HEXLOWER.decode(self.sig.as_bytes()).map_err(|_| anyhow!("entry {} has a damaged signature", self.id))?;
        let bytes: [u8; 64] = bytes.try_into().map_err(|_| anyhow!("entry {} has a damaged signature", self.id))?;
        author.verify(&self.digest(), &Signature::from_bytes(&bytes)).map_err(|_| anyhow!("entry {} is not signed by its author", self.id))
    }

    fn newer_than(&self, other: &MemEntry) -> bool {
        (self.clock, &self.author) > (other.clock, &other.author)
    }

    fn render(&self) -> String {
        format!(
            "---\ntitle: {}\nid: {}\nimportance: {}\nauthor: {}\nclock: {}\n---\n\n{}\n",
            self.title.replace('\n', " "),
            self.id,
            self.importance,
            self.author_name,
            self.clock,
            self.body.trim_end()
        )
    }
}

pub fn dir(network: &str) -> PathBuf {
    home().join("shared").join(network)
}

pub fn load(network: &str) -> BTreeMap<String, MemEntry> {
    std::fs::read_to_string(dir(network).join("entries.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(network: &str, entries: &BTreeMap<String, MemEntry>) -> Result<()> {
    let d = dir(network);
    std::fs::create_dir_all(&d)?;
    let tmp = d.join("entries.json.tmp");
    std::fs::write(&tmp, serde_json::to_string(entries)?)?;
    std::fs::rename(tmp, d.join("entries.json"))?;
    Ok(())
}

/// A file name that is safe on every platform.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    let out: String = out.chars().take(60).collect();
    if out.is_empty() { "note".to_string() } else { out }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Builds a new, signed version of an entry written on this machine.
pub fn compose(network: &str, key: &SecretKey, author_name: &str, title: &str, body: &str, importance: &str, deleted: bool) -> Result<MemEntry> {
    if !IMPORTANCE.contains(&importance) {
        bail!("importance must be one of high, normal, low");
    }
    let title = title.trim();
    if title.is_empty() {
        bail!("an entry needs a title or a text");
    }
    let clock = load(network).values().map(|e| e.clock).max().unwrap_or(0) + 1;
    let mut e = MemEntry {
        network: network.to_string(),
        id: slug(title),
        clock,
        author: key.public().to_string(),
        author_name: author_name.to_string(),
        importance: importance.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        deleted,
        sig: String::new(),
    };
    e.sign(key);
    Ok(e)
}

/// Stores an entry if it is newer than what this machine has. Returns whether it was.
/// `mirror` is an Obsidian vault folder to copy readable entries into.
pub fn apply(entry: &MemEntry, mirror: Option<&str>) -> Result<bool> {
    if !valid_id(&entry.id) || !IMPORTANCE.contains(&entry.importance.as_str()) {
        bail!("entry has an invalid id or importance");
    }
    let mut entries = load(&entry.network);
    let d = dir(&entry.network);
    if let Some(old) = entries.get(&entry.id) {
        if !entry.newer_than(old) {
            return Ok(false);
        }
        if !old.deleted {
            let h = d.join("history");
            std::fs::create_dir_all(&h)?;
            std::fs::write(h.join(format!("{}.{}.md", old.id, old.clock)), old.render())?;
        }
    }
    entries.insert(entry.id.clone(), entry.clone());
    save(&entry.network, &entries)?;
    write_files(&entry.network, &entries, entry, mirror)?;
    Ok(true)
}

/// Writes the readable file of one entry and the index of the network.
fn write_files(network: &str, entries: &BTreeMap<String, MemEntry>, changed: &MemEntry, mirror: Option<&str>) -> Result<()> {
    let d = dir(network);
    let archive = d.join("archive");
    std::fs::create_dir_all(&archive)?;
    let name = format!("{}.md", changed.id);
    let _ = std::fs::remove_file(d.join(&name));
    let _ = std::fs::remove_file(archive.join(&name));
    let mirror_dir = mirror.map(|m| Path::new(m).join("agora").join(network));
    if let Some(m) = &mirror_dir {
        let _ = std::fs::remove_file(m.join(&name));
    }
    if !changed.deleted {
        // Low importance is kept but stays out of the index.
        let target = if changed.importance == "low" { archive.join(&name) } else { d.join(&name) };
        std::fs::write(target, changed.render())?;
        if let (Some(m), true) = (&mirror_dir, changed.importance != "low") {
            std::fs::create_dir_all(m)?;
            std::fs::write(m.join(&name), changed.render())?;
        }
    }
    let mut index = String::from("# Shared memory\n\n");
    for e in entries.values().filter(|e| !e.deleted && e.importance != "low") {
        index.push_str(&format!("- [{}]({}.md) — {}, {}\n", e.title.replace('\n', " "), e.id, e.importance, e.author_name));
    }
    std::fs::write(d.join("INDEX.md"), index)?;
    Ok(())
}

/// The index of a network as text for `agora memory`.
pub fn list(network: &str) -> String {
    let entries = load(network);
    let mut out = String::new();
    for e in entries.values().filter(|e| !e.deleted) {
        out.push_str(&format!("{:<32} {:<7} {:<14} {}\n", e.id, e.importance, e.author_name, e.title.replace('\n', " ")));
    }
    if out.is_empty() { "no entries\n".to_string() } else { out }
}

/// True if `text` contains a term this machine never shares.
pub fn blocked<'a>(blocklist: &'a [String], text: &str) -> Option<&'a str> {
    let t = text.to_lowercase();
    blocklist.iter().map(String::as_str).find(|term| !term.trim().is_empty() && t.contains(&term.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SecretKey {
        SecretKey::from_bytes(&[7u8; 32])
    }

    fn entry(clock: u64) -> MemEntry {
        let mut e = MemEntry {
            network: "n".into(),
            id: "deploy".into(),
            clock,
            author: key().public().to_string(),
            author_name: "Mac".into(),
            importance: "high".into(),
            title: "Deploy".into(),
            body: "Use the script.".into(),
            deleted: false,
            sig: String::new(),
        };
        e.sign(&key());
        e
    }

    #[test]
    fn a_signed_entry_verifies_and_a_changed_one_does_not() {
        let e = entry(1);
        assert!(e.verify().is_ok());
        let mut forged = e.clone();
        forged.body = "Delete everything.".into();
        assert!(forged.verify().is_err());
        let mut stolen = e.clone();
        stolen.author = SecretKey::from_bytes(&[8u8; 32]).public().to_string();
        assert!(stolen.verify().is_err());
    }

    #[test]
    fn the_higher_clock_wins_and_ties_go_to_the_higher_author() {
        assert!(entry(2).newer_than(&entry(1)));
        assert!(!entry(1).newer_than(&entry(1)));
        let mut other = entry(1);
        other.author = "zzz".into();
        assert!(other.newer_than(&entry(1)));
    }

    #[test]
    fn slugs_are_safe_file_names() {
        assert_eq!(slug("Deploy: use ./run.sh (prod)!"), "deploy-use-run-sh-prod");
        assert_eq!(slug("  ÄÖÜ  "), "note");
        assert!(valid_id(&slug("Room 12ab: Plan the migration")));
        assert!(!valid_id("../etc/passwd"));
    }

    #[test]
    fn blocked_terms_are_found_without_regard_to_case() {
        let list = vec!["Weber".to_string(), " ".to_string()];
        assert_eq!(blocked(&list, "Angebot für WEBER GmbH"), Some("Weber"));
        assert_eq!(blocked(&list, "nothing here"), None);
    }
}
