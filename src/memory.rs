//! Shared memory: one small set of entries per network, replicated to every peer.
//!
//! Every entry is signed by its author. When two peers change the same entry,
//! the one with the higher clock wins and the older version is kept in
//! `history/`. Entries live as Markdown files that agents and humans read
//! directly; `entries.json` next to them is what the daemon replicates.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, anyhow, bail};
use iroh::{EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::proto::valid_network_id;
use crate::state::home;

pub const IMPORTANCE: [&str; 3] = ["high", "normal", "low"];

/// How far ahead of this machine's highest clock a received entry may be. A
/// peer that was offline for a long time is ahead by what it wrote meanwhile.
const MAX_CLOCK_JUMP: u64 = 1_000_000;

/// One writer at a time: entries arrive on several links at once, and every
/// change reads, modifies and rewrites the whole store.
static STORE: Mutex<()> = Mutex::new(());

/// Looks up the name this machine knows a peer by.
pub type Names<'a> = &'a dyn Fn(&str) -> String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemEntry {
    pub network: String,
    pub id: String,
    /// Logical clock; a change is accepted when (clock, author) is higher.
    pub clock: u64,
    /// Peer id of the author.
    pub author: String,
    /// What the author calls itself. Never shown: the name this machine knows
    /// the author by is, because a peer could put anything here.
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
    /// What the signature covers. Every field is prefixed with its length, so
    /// that text cannot move from one field into the next.
    fn digest(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"agora-mem-2");
        let clock = self.clock.to_string();
        let deleted = if self.deleted { "1" } else { "0" };
        for field in [&self.network, &self.id, &clock, &self.author, &self.author_name, &self.importance, deleted, &self.title, &self.body] {
            h.update(&(field.len() as u64).to_be_bytes());
            h.update(field.as_bytes());
        }
        *h.finalize().as_bytes()
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

    fn render(&self, names: Names) -> String {
        format!(
            "---\ntitle: {}\nid: {}\nimportance: {}\nauthor: {}\nclock: {}\n---\n\n{}\n",
            self.title.replace('\n', " "),
            self.id,
            self.importance,
            names(&self.author).replace('\n', " "),
            self.clock,
            self.body.trim_end()
        )
    }
}

/// The directory of a network's memory. The network id comes from a network
/// ID a peer handed over, so it is checked before it becomes part of a path.
pub fn dir(network: &str) -> Result<PathBuf> {
    if !valid_network_id(network) {
        bail!("invalid network id");
    }
    Ok(home().join("shared").join(network))
}

/// The entries of a network. An unreadable store is an error, so that it is
/// not mistaken for an empty one and overwritten.
fn load_strict(network: &str) -> Result<BTreeMap<String, MemEntry>> {
    let path = dir(network)?.join("entries.json");
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = std::fs::read_to_string(&path)?;
    serde_json::from_str(&text).map_err(|e| anyhow!("{} is damaged ({e}); move it away to start over", path.display()))
}

/// The entries of a network for reading; empty if there are none or the store is unreadable.
pub fn load(network: &str) -> BTreeMap<String, MemEntry> {
    let _guard = STORE.lock().unwrap();
    load_strict(network).unwrap_or_default()
}

fn save(network: &str, entries: &BTreeMap<String, MemEntry>) -> Result<()> {
    let d = dir(network)?;
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
    let out: String = out.trim_end_matches('-').chars().take(60).collect();
    if out.is_empty() {
        // A title without a single ASCII letter or digit: tell such titles apart.
        format!("note-{}", &blake3::hash(title.as_bytes()).to_hex()[..8])
    } else {
        out
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Writes an entry from this machine: next clock, signature, storage. `id`
/// names an existing entry to change; without it the id comes from the title.
#[allow(clippy::too_many_arguments)]
pub fn write_local(
    network: &str,
    key: &SecretKey,
    author_name: &str,
    id: Option<&str>,
    title: &str,
    body: &str,
    importance: &str,
    deleted: bool,
    mirror: Option<&str>,
    names: Names,
) -> Result<MemEntry> {
    if !IMPORTANCE.contains(&importance) {
        bail!("importance must be one of high, normal, low");
    }
    let title = title.trim();
    if title.is_empty() {
        bail!("an entry needs a title or a text");
    }
    let _guard = STORE.lock().unwrap();
    let mut entries = load_strict(network)?;
    let clock = entries.values().map(|e| e.clock).max().unwrap_or(0).checked_add(1).ok_or_else(|| anyhow!("the memory's clock is exhausted"))?;
    let mut e = MemEntry {
        network: network.to_string(),
        id: id.map(str::to_string).unwrap_or_else(|| slug(title)),
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
    if !store(&mut entries, &e, mirror, names)? {
        bail!("the entry was not saved: a newer version exists");
    }
    Ok(e)
}

/// Stores an entry a peer sent if it is newer than what this machine has.
/// Returns whether it was. `mirror` is an Obsidian vault folder.
pub fn apply(entry: &MemEntry, mirror: Option<&str>, names: Names) -> Result<bool> {
    let _guard = STORE.lock().unwrap();
    let mut entries = load_strict(&entry.network)?;
    let highest = entries.values().map(|e| e.clock).max().unwrap_or(0);
    if entry.clock > highest.saturating_add(MAX_CLOCK_JUMP) {
        bail!("entry {} has a clock far ahead of this machine's", entry.id);
    }
    store(&mut entries, entry, mirror, names)
}

fn store(entries: &mut BTreeMap<String, MemEntry>, entry: &MemEntry, mirror: Option<&str>, names: Names) -> Result<bool> {
    if !valid_id(&entry.id) || !IMPORTANCE.contains(&entry.importance.as_str()) {
        bail!("entry has an invalid id or importance");
    }
    let d = dir(&entry.network)?;
    if let Some(old) = entries.get(&entry.id) {
        if !entry.newer_than(old) {
            return Ok(false);
        }
        if !old.deleted {
            let h = d.join("history");
            std::fs::create_dir_all(&h)?;
            std::fs::write(h.join(format!("{}.{}.md", old.id, old.clock)), old.render(names))?;
        }
    }
    entries.insert(entry.id.clone(), entry.clone());
    save(&entry.network, entries)?;
    write_files(&d, entries, entry, mirror, names)?;
    Ok(true)
}

/// Writes the readable file of one entry and the index of the network.
fn write_files(d: &Path, entries: &BTreeMap<String, MemEntry>, changed: &MemEntry, mirror: Option<&str>, names: Names) -> Result<()> {
    let archive = d.join("archive");
    std::fs::create_dir_all(&archive)?;
    let name = format!("{}.md", changed.id);
    let _ = std::fs::remove_file(d.join(&name));
    let _ = std::fs::remove_file(archive.join(&name));
    let mirror_dir = mirror.map(|m| Path::new(m).join("agora").join(&changed.network));
    if let Some(m) = &mirror_dir {
        let _ = std::fs::remove_file(m.join(&name));
    }
    if !changed.deleted {
        // Low importance is kept but stays out of the index.
        let target = if changed.importance == "low" { archive.join(&name) } else { d.join(&name) };
        std::fs::write(target, changed.render(names))?;
        if let (Some(m), true) = (&mirror_dir, changed.importance != "low") {
            std::fs::create_dir_all(m)?;
            std::fs::write(m.join(&name), changed.render(names))?;
        }
    }
    let mut index = String::from("# Shared memory\n\n");
    for e in entries.values().filter(|e| !e.deleted && e.importance != "low") {
        index.push_str(&format!("- [{}]({}.md) — {}, {}\n", e.title.replace('\n', " "), e.id, e.importance, names(&e.author).replace('\n', " ")));
    }
    std::fs::write(d.join("INDEX.md"), index)?;
    Ok(())
}

/// The index of a network as text for `agora memory`.
pub fn list(network: &str, names: Names) -> String {
    let entries = load(network);
    let mut out = String::new();
    for e in entries.values().filter(|e| !e.deleted) {
        out.push_str(&format!("{:<32} {:<7} {:<14} {}\n", e.id, e.importance, names(&e.author), e.title.replace('\n', " ")));
    }
    if out.is_empty() { "no entries\n".to_string() } else { out }
}

/// A note a human wrote in the Obsidian folder.
#[derive(Debug, PartialEq)]
pub struct Note {
    pub title: String,
    pub importance: String,
    pub body: String,
}

/// Reads a Markdown note. A front matter block may name `title` and
/// `importance`; without one the title is the file name and the importance
/// normal. The files this module writes itself parse back to their entry.
pub fn parse_note(text: &str, file_stem: &str) -> Note {
    let mut title = file_stem.trim().to_string();
    let mut importance = "normal".to_string();
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                let Some((key, value)) = line.split_once(':') else { continue };
                let value = value.trim();
                match key.trim() {
                    "title" if !value.is_empty() => title = value.to_string(),
                    "importance" if IMPORTANCE.contains(&value) => importance = value.to_string(),
                    _ => {}
                }
            }
            body = &rest[end + 4..];
        }
    }
    Note { title, importance, body: body.trim().to_string() }
}

/// Where a network's entries are mirrored inside an Obsidian vault folder.
pub fn mirror_dir(mirror: &str, network: &str) -> PathBuf {
    Path::new(mirror).join("agora").join(network)
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
            network: "0a1b2c".into(),
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
    fn text_cannot_move_between_signed_fields() {
        let mut a = entry(1);
        a.title = "a|b".into();
        a.body = "c".into();
        let mut b = entry(1);
        b.title = "a".into();
        b.body = "b|c".into();
        assert_ne!(a.digest(), b.digest());
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
        assert!(slug("  ÄÖÜ  ").starts_with("note-"));
        assert_ne!(slug("ÄÖÜ"), slug("日本語"));
        assert!(valid_id(&slug("Room 12ab: Plan the migration")));
        assert!(valid_id(&slug("日本語")));
        assert!(!valid_id("../etc/passwd"));
    }

    #[test]
    fn a_network_id_that_is_not_hex_never_becomes_a_path() {
        assert!(dir("0a1b2c3d4e5f").is_ok());
        for bad in ["../../.claude", "/etc", "a/b", "", "x y", "..", "0a1b2c3d4e5f0a1b2c3d4e5f0a1b2c3d4e5f"] {
            assert!(dir(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_note_from_obsidian_is_read_with_or_without_front_matter() {
        assert_eq!(parse_note("Backups run at night.\n", "Backup plan"), Note { title: "Backup plan".into(), importance: "normal".into(), body: "Backups run at night.".into() });
        let with = "---\ntitle: Deploy: the rules\nimportance: high\ntags: [x]\n---\n\nUse CI.\n";
        assert_eq!(parse_note(with, "deploy"), Note { title: "Deploy: the rules".into(), importance: "high".into(), body: "Use CI.".into() });
        // An unknown importance and an unfinished front matter do not break the note.
        assert_eq!(parse_note("---\nimportance: urgent\n---\nx", "n").importance, "normal");
        assert_eq!(parse_note("---\ntitle: lost\nno end", "n").title, "n");
    }

    #[test]
    fn a_mirrored_entry_reads_back_as_itself() {
        let e = entry(3);
        let note = parse_note(&e.render(&|_| "Mac".to_string()), "deploy");
        assert_eq!((note.title, note.importance, note.body), (e.title, e.importance, e.body));
    }

    #[test]
    fn blocked_terms_are_found_without_regard_to_case() {
        let list = vec!["Weber".to_string(), " ".to_string()];
        assert_eq!(blocked(&list, "Angebot für WEBER GmbH"), Some("Weber"));
        assert_eq!(blocked(&list, "nothing here"), None);
    }
}
