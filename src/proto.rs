//! Wire protocol: length-prefixed CBOR frames on a QUIC stream.

use anyhow::{Result, anyhow, bail};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"agora-mesh/0";
const MAX_FRAME: u32 = 1 << 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Member {
    pub name: String,
    pub id: String,
    /// Knowledge level 1..=3, see ARCHITECTURE.md section 4.
    pub level: u8,
    /// "full", "read" or "talk".
    pub rights: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Msg {
    #[serde(rename = "PEER_HELLO")]
    Hello { peer_name: String, network: String, proof: String },
    #[serde(rename = "PEER_PENDING")]
    Pending,
    #[serde(rename = "PEER_APPROVE")]
    Approve { peer_name: String, members: Vec<Member> },
    #[serde(rename = "PEER_REJECT")]
    Reject { reason: String },
    #[serde(rename = "MEMBERS")]
    Members { network: String, members: Vec<Member> },
    #[serde(rename = "HEARTBEAT")]
    Heartbeat,
    /// Opener to joiner: start a fresh session for this topic.
    #[serde(rename = "ROOM_INVITE")]
    RoomInvite {
        room_id: String,
        topic: String,
        turn_budget: u32,
        #[serde(default)]
        token_budget: u64,
        /// Normalized git remote of the project the room is about, if any.
        #[serde(default)]
        project: Option<String>,
        participants: Vec<Participant>,
    },
    #[serde(rename = "ROOM_JOIN")]
    RoomJoin { room_id: String },
    #[serde(rename = "ROOM_DECLINE")]
    RoomDecline { room_id: String, reason: String },
    /// Joiner to opener: a contribution that has no number yet.
    #[serde(rename = "SAY")]
    /// `tokens` is what the joiner's session has used in this room so far;
    /// `extra_turns` lets a human grant every peer more turns.
    Say {
        room_id: String,
        kind: String,
        text: String,
        #[serde(default)]
        tokens: u64,
        #[serde(default)]
        extra_turns: u32,
        /// Set when the contribution reports on a work package.
        #[serde(default)]
        package: Option<u32>,
    },
    /// Opener to joiner: work on this package now.
    #[serde(rename = "WORK")]
    Work { room_id: String, package: u32, title: String, branch: String, taken_from: Option<String> },
    /// How much of its usage limit a peer has used.
    #[serde(rename = "STATUS")]
    Status { capacity: Capacity },
    /// Opener to joiners: a numbered contribution. `next` names who speaks now.
    #[serde(rename = "DISCUSS_MSG")]
    Discuss { room_id: String, seq: u32, from: String, from_name: String, kind: String, text: String, turns_left: u32, next: Option<String> },
    #[serde(rename = "ROOM_CLOSE")]
    RoomClose { room_id: String, reason: String, result: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Participant {
    pub id: String,
    pub name: String,
}

/// A peer's remaining capacity, read from the rate-limit information Claude Code
/// prints with every turn.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Capacity {
    /// Used share of the tightest usage window, 0.0 to 1.0.
    pub utilization: f32,
    /// When that window resets, Unix seconds; 0 if unknown.
    pub resets_at: u64,
    pub limited: bool,
}

impl Capacity {
    /// Reads `rate_limit_info` of a `rate_limit_event`.
    pub fn from_event(info: &serde_json::Value) -> Option<Capacity> {
        let status = info["status"].as_str()?;
        let mut utilization = 0.0f32;
        let mut resets_at = info["resetsAt"].as_u64().unwrap_or(0);
        if let Some(windows) = info["unifiedWindows"].as_object() {
            for w in windows.values() {
                let u = w["utilization"].as_f64().unwrap_or(0.0) as f32;
                if u > utilization {
                    utilization = u;
                    resets_at = w["resetsAt"].as_u64().unwrap_or(resets_at);
                }
            }
        }
        let threshold = std::env::var("AGORA_LIMIT_AT").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.97);
        Some(Capacity { utilization, resets_at, limited: !status.starts_with("allowed") || utilization >= threshold })
    }

    pub fn describe(&self) -> String {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let reset = if self.resets_at > now {
            let m = (self.resets_at - now) / 60;
            format!(", resets in {}h{:02}m", m / 60, m % 60)
        } else {
            String::new()
        };
        if self.limited {
            format!("at its usage limit{reset}")
        } else {
            format!("{:.0} % of its limit used{reset}", self.utilization * 100.0)
        }
    }
}

/// Reads the packages of a SPLIT_WORK message: one line per package in the
/// form `- <peer name>: <package>`. Returns the index into `names` and the title.
pub fn parse_packages(text: &str, names: &[String]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches(|c: char| c == '-' || c == '*' || c == '.' || c == ')' || c.is_ascii_digit() || c == ' ');
        let Some((who, title)) = line.split_once(':') else { continue };
        let who = who.trim().trim_matches(|c| c == '*' || c == '`' || c == '"').to_lowercase();
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        if let Some(i) = names.iter().position(|n| n.to_lowercase() == who) {
            out.push((i, title.to_string()));
        }
    }
    out
}

/// Kinds an agent may put on the first line of a contribution.
pub const KINDS: [&str; 8] = ["PROPOSAL", "CRITIQUE", "QUESTION", "ANSWER", "SPLIT_WORK", "PASS", "CONSENSUS", "NEED_HUMAN"];

/// Splits a session's answer into kind and text. Falls back to `default`
/// when the first line is not one of the allowed kinds.
pub fn parse_kind(reply: &str, default: &str, allowed: &[&str]) -> (String, String) {
    let reply = reply.trim();
    let (first, rest) = reply.split_once('\n').unwrap_or((reply, ""));
    let (head, tail) = first.split_once(':').unwrap_or((first, ""));
    let head = head.trim().trim_matches(|c| c == '*' || c == '#' || c == '`').trim().to_uppercase().replace(' ', "_");
    if allowed.contains(&head.as_str()) {
        let text = format!("{}\n{}", tail.trim(), rest).trim().to_string();
        (head, text)
    } else {
        (default.to_string(), reply.to_string())
    }
}

pub async fn write_msg(send: &mut SendStream, msg: &Msg) -> Result<()> {
    let mut buf = Vec::new();
    ciborium::into_writer(msg, &mut buf)?;
    send.write_all(&(buf.len() as u32).to_be_bytes()).await.map_err(|e| anyhow!("{e}"))?;
    send.write_all(&buf).await.map_err(|e| anyhow!("{e}"))?;
    Ok(())
}

pub async fn read_msg(recv: &mut RecvStream) -> Result<Msg> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await.map_err(|e| anyhow!("{e}"))?;
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        bail!("frame too large: {len}");
    }
    let mut buf = vec![0u8; len as usize];
    recv.read_exact(&mut buf).await.map_err(|e| anyhow!("{e}"))?;
    Ok(ciborium::from_reader(buf.as_slice())?)
}

/// Proof that a joiner knows the network secret, bound to its own id.
pub fn proof(secret: &str, joiner_id: &str) -> String {
    blake3::hash(format!("agora-join|{secret}|{joiner_id}").as_bytes()).to_hex().to_string()
}

const WORDS: [&str; 64] = [
    "amber", "anchor", "apple", "arrow", "badge", "bamboo", "beacon", "birch", "bridge", "cabin",
    "candle", "canyon", "cedar", "cloud", "cobalt", "comet", "copper", "coral", "delta", "desert",
    "ember", "falcon", "fern", "flint", "forest", "garnet", "glacier", "harbor", "hazel", "island",
    "ivory", "jade", "kettle", "lagoon", "lantern", "maple", "marble", "meadow", "nickel", "oasis",
    "olive", "onyx", "orchid", "pebble", "pine", "plum", "quartz", "raven", "reef", "river",
    "saffron", "salmon", "silver", "spruce", "thistle", "timber", "tulip", "umber", "valley",
    "velvet", "walnut", "willow", "winter", "zephyr",
];

/// Four words both humans compare before approving. Same on both sides.
pub fn words(id_a: &str, id_b: &str) -> String {
    let (a, b) = if id_a <= id_b { (id_a, id_b) } else { (id_b, id_a) };
    let h = blake3::hash(format!("agora-words|{a}|{b}").as_bytes());
    h.as_bytes()[..4].iter().map(|x| WORDS[(*x & 63) as usize]).collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Ticket {
    pub network: String,
    pub secret: String,
    pub opener: String,
}

impl Ticket {
    pub fn encode(&self) -> String {
        let mut buf = Vec::new();
        ciborium::into_writer(self, &mut buf).expect("ticket encodes");
        format!("agora1{}", data_encoding::BASE32_NOPAD.encode(&buf).to_lowercase())
    }

    pub fn decode(s: &str) -> Result<Self> {
        let body = s.trim().strip_prefix("agora1").ok_or_else(|| anyhow!("not a network ID (must start with agora1)"))?;
        let buf = data_encoding::BASE32_NOPAD.decode(body.to_uppercase().as_bytes()).map_err(|_| anyhow!("network ID is damaged"))?;
        Ok(ciborium::from_reader(buf.as_slice())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_roundtrip() {
        let t = Ticket { network: "n1".into(), secret: "s".into(), opener: "abc".into() };
        let d = Ticket::decode(&t.encode()).unwrap();
        assert_eq!((d.network, d.secret, d.opener), ("n1".into(), "s".into(), "abc".into()));
        assert!(Ticket::decode("nonsense").is_err());
    }

    #[test]
    fn words_are_symmetric_and_bound_to_ids() {
        assert_eq!(words("a", "b"), words("b", "a"));
        assert_ne!(words("a", "b"), words("a", "c"));
        assert_eq!(words("a", "b").split(' ').count(), 4);
    }

    #[test]
    fn kind_is_read_from_the_first_line() {
        assert_eq!(parse_kind("CRITIQUE\nno", "PROPOSAL", &KINDS), ("CRITIQUE".into(), "no".into()));
        assert_eq!(parse_kind("**need human**: decide\nplease", "PROPOSAL", &KINDS), ("NEED_HUMAN".into(), "decide\nplease".into()));
        assert_eq!(parse_kind("I think so", "PROPOSAL", &KINDS), ("PROPOSAL".into(), "I think so".into()));
        assert_eq!(parse_kind("CONSENSUS", "PROPOSAL", &["KNOWN_MISSING"]), ("PROPOSAL".into(), "CONSENSUS".into()));
    }

    #[test]
    fn packages_are_read_from_split_work() {
        let names = vec!["Mac".to_string(), "Heimserver".to_string()];
        let text = "We split it:\n- Mac: backend API\n* **heimserver**: frontend\n2. Nobody: ignored\n- Mac:";
        assert_eq!(parse_packages(text, &names), vec![(0, "backend API".to_string()), (1, "frontend".to_string())]);
        assert!(parse_packages("no packages here", &names).is_empty());
    }

    #[test]
    fn capacity_is_read_from_a_rate_limit_event() {
        let info = serde_json::json!({"status": "allowed", "resetsAt": 10, "unifiedWindows": {
            "five_hour": {"utilization": 0.1, "resetsAt": 10}, "seven_day": {"utilization": 0.42, "resetsAt": 99}}});
        assert_eq!(Capacity::from_event(&info), Some(Capacity { utilization: 0.42, resets_at: 99, limited: false }));
        let full = serde_json::json!({"status": "rejected", "resetsAt": 10});
        assert!(Capacity::from_event(&full).unwrap().limited);
        let near = serde_json::json!({"status": "allowed", "unifiedWindows": {"five_hour": {"utilization": 0.99, "resetsAt": 5}}});
        assert!(Capacity::from_event(&near).unwrap().limited);
    }

    #[test]
    fn proof_depends_on_secret_and_id() {
        assert_ne!(proof("s1", "id"), proof("s2", "id"));
        assert_ne!(proof("s1", "id"), proof("s1", "other"));
    }
}
