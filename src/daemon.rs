//! The background daemon: peer connections, join approval, local control socket.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use iroh::endpoint::{RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointId, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, watch};

use crate::memory::{self, MemEntry};
use crate::project;
use crate::proto::{self, ALPN, Capacity, Member, Msg, Ticket};
use crate::room::{self, Invite, RoomEvent, RoomHandle, RoomState};
use crate::state::{NetStatus, Network, State, home};

const HEARTBEAT: Duration = Duration::from_secs(15);
const DEAD_AFTER: Duration = Duration::from_secs(45);
const REDIAL: Duration = Duration::from_secs(10);
const MAX_ROOMS: usize = 2;

/// What the `agora` command reads to find the running daemon.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

impl DaemonInfo {
    pub fn path() -> std::path::PathBuf {
        home().join("daemon.json")
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub token: String,
    pub cmd: String,
    #[serde(default)]
    pub args: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub text: String,
}

struct PendingJoin {
    id: String,
    network: String,
    tx: mpsc::Sender<Msg>,
}

pub struct Shared {
    pub me: String,
    endpoint: Endpoint,
    pub state: Mutex<State>,
    /// Approved peers we currently hold a live link to.
    conns: Mutex<HashMap<String, mpsc::Sender<Msg>>>,
    /// Rooms running on this machine.
    rooms: Mutex<HashMap<String, RoomHandle>>,
    /// Last known capacity per peer id, our own included.
    capacity: Mutex<HashMap<String, Capacity>>,
    /// How many rooms each other peer said it is working on.
    activity: Mutex<HashMap<String, u32>>,
    /// Signs this machine's shared-memory entries.
    secret: SecretKey,
    /// Open connections by peer id, to report how a peer is reached.
    links: Mutex<HashMap<String, iroh::endpoint::Connection>>,
    /// High-importance memory entries, for sessions that are running.
    notices: Mutex<Vec<String>>,
    /// Invitations waiting for `agora accept`.
    invites: Mutex<HashMap<String, Invite>>,
    /// Join requests waiting for a human, by nickname.
    pending: Mutex<HashMap<String, PendingJoin>>,
    dialing: Mutex<HashSet<String>>,
    redial: Notify,
}

impl Shared {
    /// Records this machine's capacity and tells every connected peer.
    pub fn report_capacity(&self, capacity: Capacity) {
        let changed = self.capacity.lock().unwrap().insert(self.me.clone(), capacity) != Some(capacity);
        if changed {
            for tx in self.conns.lock().unwrap().values() {
                let _ = tx.try_send(Msg::Status { capacity });
            }
        }
    }

    /// Tells every connected peer how many rooms run here. Call after a change.
    pub fn announce_rooms(&self) {
        let rooms = self.rooms.lock().unwrap().len() as u32;
        for tx in self.conns.lock().unwrap().values() {
            let _ = tx.try_send(Msg::Busy { rooms });
        }
    }

    pub fn notices_len(&self) -> usize {
        self.notices.lock().unwrap().len()
    }

    /// Notices a session has not seen yet; moves its marker forward.
    pub fn notices_since(&self, seen: &mut usize) -> Vec<String> {
        let all = self.notices.lock().unwrap();
        let new = all.get(*seen..).map(<[String]>::to_vec).unwrap_or_default();
        *seen = all.len();
        new
    }

    pub fn capacity_of(&self, peer: &str) -> Option<Capacity> {
        self.capacity.lock().unwrap().get(peer).copied()
    }

    /// Queues a message for an approved, connected peer. False if it is offline.
    pub fn send_to(&self, peer: &str, msg: Msg) -> bool {
        match self.conns.lock().unwrap().get(peer) {
            Some(tx) => tx.try_send(msg).is_ok(),
            None => false,
        }
    }
}

/// The command of `agora notify`, if any.
static NOTIFY: Mutex<Option<String>> = Mutex::new(None);

/// Runs the user's notify command for an event, without waiting for it.
fn notify(kind: &str, text: &str) {
    let Some(cmd) = NOTIFY.lock().unwrap().clone() else { return };
    let mut c = if cfg!(windows) { std::process::Command::new("cmd") } else { std::process::Command::new("sh") };
    c.arg(if cfg!(windows) { "/C" } else { "-c" }).arg(cmd);
    c.env("AGORA_EVENT_KIND", kind).env("AGORA_EVENT_TEXT", text);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Ok(mut child) = c.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Appends a line to events.log. `agora events` prints it; the plugin's monitor
/// passes it to the session a human is working in.
pub fn event(kind: &str, text: &str) {
    notify(kind, text);
    use std::io::Write;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(home().join("events.log")) {
        let _ = writeln!(f, "{now}\t{kind}\t{}", text.replace('\n', " "));
    }
}

/// Removes rooms whose log has not changed for 30 days.
fn prune_rooms() {
    let Ok(dirs) = std::fs::read_dir(home().join("rooms")) else { return };
    for d in dirs.flatten() {
        let old = std::fs::metadata(d.path().join("log.md")).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        if old.is_some_and(|age| age > Duration::from_secs(30 * 24 * 3600)) {
            let _ = std::fs::remove_dir_all(d.path());
        }
    }
}

/// Stores a memory entry if it is new, announces high importance, and passes it on.
fn accept_entry(shared: &Shared, entry: &MemEntry, from: Option<&str>) -> Result<bool> {
    let (mirror, peers) = {
        let state = shared.state.lock().unwrap();
        let peers: Vec<String> = state.network(&entry.network).map(|n| n.peers.iter().map(|m| m.id.clone()).collect()).unwrap_or_default();
        (state.obsidian.clone(), peers)
    };
    if !memory::apply(entry, mirror.as_deref())? {
        return Ok(false);
    }
    if entry.importance == "high" && !entry.deleted {
        let line = format!("{} (by {}; read it with `agora memory show {}`)", entry.title, entry.author_name, entry.id);
        shared.notices.lock().unwrap().push(line.clone());
        event("memory", &line);
    }
    for p in peers {
        if Some(p.as_str()) != from {
            shared.send_to(&p, Msg::MemPut { entry: entry.clone() });
        }
    }
    Ok(true)
}

/// Checks an entry a peer sent before storing it.
fn receive_entry(shared: &Shared, peer: &str, entry: &MemEntry) -> Result<()> {
    {
        let state = shared.state.lock().unwrap();
        let net = state.network(&entry.network).filter(|n| n.status == NetStatus::Active).ok_or_else(|| anyhow!("unknown network"))?;
        if !net.peers.iter().any(|m| m.id == peer) {
            bail!("the sender is not a member of that network");
        }
        if entry.author != shared.me && !net.peers.iter().any(|m| m.id == entry.author) {
            bail!("the author is not a member of that network");
        }
    }
    entry.verify()?;
    accept_entry(shared, entry, Some(peer))?;
    Ok(())
}

/// A link to a peer came up: tell the rooms, and send the peer every entry of
/// the networks we share.
fn sync_memory(shared: &Arc<Shared>, peer: &str) {
    for room in shared.rooms.lock().unwrap().values() {
        let _ = room.tx.try_send(RoomEvent::LinkUp(peer.to_string()));
    }
    let Some(tx) = shared.conns.lock().unwrap().get(peer).cloned() else { return };
    let networks: Vec<String> =
        shared.state.lock().unwrap().networks.iter().filter(|n| n.peers.iter().any(|m| m.id == peer)).map(|n| n.id.clone()).collect();
    let rooms = shared.rooms.lock().unwrap().len() as u32;
    let capacity = shared.capacity_of(&shared.me);
    tokio::spawn(async move {
        // What the peer shows for us in `agora peers`.
        let _ = tx.send(Msg::Busy { rooms }).await;
        if let Some(capacity) = capacity {
            let _ = tx.send(Msg::Status { capacity }).await;
        }
        for n in networks {
            for entry in memory::load(&n).into_values() {
                if tx.send(Msg::MemPut { entry }).await.is_err() {
                    return;
                }
            }
        }
    });
}

/// The network a command means: the named one, or the only one this machine is in.
fn pick_network(shared: &Shared, wanted: Option<&String>) -> Result<String> {
    let state = shared.state.lock().unwrap();
    let active: Vec<&Network> = state.networks.iter().filter(|n| n.status == NetStatus::Active).collect();
    match wanted.filter(|w| !w.is_empty()) {
        Some(w) => active.iter().find(|n| n.id == *w).map(|n| n.id.clone()).ok_or_else(|| anyhow!("no network {w}")),
        None => match active.as_slice() {
            [] => bail!("this machine is in no network yet"),
            [one] => Ok(one.id.clone()),
            _ => bail!("this machine is in several networks; name one with --network"),
        },
    }
}

/// Writes an entry from this machine: block list, signature, storage, replication.
fn remember(shared: &Shared, network: &str, title: &str, body: &str, importance: &str, deleted: bool) -> Result<MemEntry> {
    let (name, blocked) = {
        let state = shared.state.lock().unwrap();
        (state.name.clone(), memory::blocked(&state.blocklist, &format!("{title}\n{body}")).map(str::to_string))
    };
    if let Some(term) = blocked {
        bail!("not saved: the text contains \"{term}\", which is on this machine's block list");
    }
    let entry = memory::compose(network, &shared.secret, &name, title, body, importance, deleted)?;
    accept_entry(shared, &entry, None)?;
    Ok(entry)
}

pub fn secret_key_from_hex(hex: &str) -> Result<SecretKey> {
    let bytes = data_encoding::HEXLOWER.decode(hex.as_bytes()).map_err(|_| anyhow!("secret key in state.json is damaged"))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("secret key has the wrong length"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

pub async fn run() -> Result<()> {
    let state = State::load()?.ok_or_else(|| anyhow!("not set up yet: run `agora setup --name <peer-name>` first"))?;
    let secret = secret_key_from_hex(&state.secret_key)?;
    *NOTIFY.lock().unwrap() = state.notify.clone();
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret.clone())
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| anyhow!("binding the network endpoint failed: {e}"))?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let token = data_encoding::HEXLOWER.encode(&rand::random::<[u8; 16]>());
    let info = DaemonInfo { port: listener.local_addr()?.port(), token: token.clone(), pid: std::process::id() };
    write_private(&DaemonInfo::path(), &serde_json::to_string(&info)?)?;

    let shared = Arc::new(Shared {
        me: endpoint.id().to_string(),
        endpoint: endpoint.clone(),
        state: Mutex::new(state),
        conns: Mutex::new(HashMap::new()),
        rooms: Mutex::new(HashMap::new()),
        capacity: Mutex::new(HashMap::new()),
        activity: Mutex::new(HashMap::new()),
        secret,
        links: Mutex::new(HashMap::new()),
        notices: Mutex::new(Vec::new()),
        invites: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        dialing: Mutex::new(HashSet::new()),
        redial: Notify::new(),
    });
    eprintln!("agora daemon running, peer id {}", shared.me);
    prune_rooms();

    tokio::spawn(accept_loop(shared.clone()));
    tokio::spawn(dial_loop(shared.clone()));

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let shared = shared.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_control(shared, stream, &token).await {
                        eprintln!("control request failed: {e}");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    let _ = std::fs::remove_file(DaemonInfo::path());
    endpoint.close().await;
    Ok(())
}

fn write_private(path: &std::path::Path, content: &str) -> Result<()> {
    std::fs::create_dir_all(home())?;
    std::fs::write(path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- peer links

async fn accept_loop(shared: Arc<Shared>) {
    while let Some(incoming) = shared.endpoint.accept().await {
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_incoming(shared, incoming).await {
                eprintln!("incoming connection ended: {e}");
            }
        });
    }
}

async fn handle_incoming(shared: Arc<Shared>, incoming: iroh::endpoint::Incoming) -> Result<()> {
    let conn = incoming.await.map_err(|e| anyhow!("{e}"))?;
    let peer = conn.remote_id().to_string();
    shared.links.lock().unwrap().insert(peer.clone(), conn.clone());
    let (mut send, mut recv) = conn.accept_bi().await.map_err(|e| anyhow!("{e}"))?;

    let Msg::Hello { peer_name, network, proof } = proto::read_msg(&mut recv).await? else {
        bail!("peer {peer} did not start with PEER_HELLO");
    };

    // Decide under the lock, answer after releasing it.
    enum Decision {
        Reject(&'static str),
        Member(Vec<Member>),
        Pending,
    }
    let decision = {
        let state = shared.state.lock().unwrap();
        match state.network(&network) {
            None => Decision::Reject("unknown network"),
            Some(net) if net.status != NetStatus::Active => Decision::Reject("this peer is not a member yet"),
            Some(net) if proto::proof(&net.secret, &peer) != proof => Decision::Reject("wrong network secret"),
            Some(net) if net.peers.iter().any(|m| m.id == peer) => Decision::Member(all_members(&shared.me, &state, net)),
            Some(_) => Decision::Pending,
        }
    };

    let (tx, rx) = mpsc::channel(256);
    match decision {
        Decision::Reject(reason) => {
            proto::write_msg(&mut send, &Msg::Reject { reason: reason.into() }).await?;
            let _ = send.finish();
            // Give the peer a moment to read the answer before the connection drops.
            let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
            return Ok(());
        }
        Decision::Member(members) => {
            let my_name = shared.state.lock().unwrap().name.clone();
            proto::write_msg(&mut send, &Msg::Approve { peer_name: my_name, members }).await?;
            shared.conns.lock().unwrap().insert(peer.clone(), tx.clone());
            sync_memory(&shared, &peer);
        }
        Decision::Pending => {
            let mut nick = peer_name.clone();
            {
                let state = shared.state.lock().unwrap();
                let mut pending = shared.pending.lock().unwrap();
                pending.retain(|_, p| p.id != peer);
                let taken = pending.contains_key(&nick) || state.networks.iter().any(|n| n.peers.iter().any(|m| m.name == nick));
                if taken {
                    nick = format!("{nick}-{}", &peer[..6.min(peer.len())]);
                }
                pending.insert(nick.clone(), PendingJoin { id: peer.clone(), network: network.clone(), tx: tx.clone() });
            }
            eprintln!("join request from \"{nick}\" ({}), words: {}", short(&peer), proto::words(&shared.me, &peer));
            event("join-request", &format!("\"{nick}\" wants to join; words: {} (agora approve {nick})", proto::words(&shared.me, &peer)));
            proto::write_msg(&mut send, &Msg::Pending).await?;
        }
    }

    let result = run_link(&shared, &peer, &network, &tx, rx, send, recv).await;
    cleanup_link(&shared, &peer, &tx);
    conn.close(0u32.into(), b"bye");
    result
}

async fn dial_loop(shared: Arc<Shared>) {
    loop {
        for (peer, network) in dial_targets(&shared) {
            if !shared.dialing.lock().unwrap().insert(peer.clone()) {
                continue;
            }
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Err(e) = dial(&shared, &peer, &network).await {
                    eprintln!("link to {} ended: {e}", short(&peer));
                }
                shared.dialing.lock().unwrap().remove(&peer);
            });
        }
        tokio::select! {
            _ = tokio::time::sleep(REDIAL) => {}
            _ = shared.redial.notified() => {}
        }
    }
}

/// Whom we should be connected to but are not. For an approved pair the peer with
/// the smaller id dials, so the two never open duplicate links.
fn dial_targets(shared: &Shared) -> Vec<(String, String)> {
    let state = shared.state.lock().unwrap();
    let conns = shared.conns.lock().unwrap();
    let mut out = Vec::new();
    for net in &state.networks {
        match net.status {
            NetStatus::Joining => out.push((net.opener.clone(), net.id.clone())),
            NetStatus::Active => {
                for m in &net.peers {
                    if shared.me < m.id && !conns.contains_key(&m.id) {
                        out.push((m.id.clone(), net.id.clone()));
                    }
                }
            }
        }
    }
    out.retain(|(p, _)| !conns.contains_key(p));
    out
}

async fn dial(shared: &Arc<Shared>, peer: &str, network: &str) -> Result<()> {
    let id: EndpointId = peer.parse().map_err(|_| anyhow!("invalid peer id {peer}"))?;
    let (name, secret) = {
        let state = shared.state.lock().unwrap();
        let net = state.network(network).ok_or_else(|| anyhow!("network is gone"))?;
        (state.name.clone(), net.secret.clone())
    };
    let conn = shared.endpoint.connect(id, ALPN).await.map_err(|e| anyhow!("{e}"))?;
    shared.links.lock().unwrap().insert(peer.to_string(), conn.clone());
    let (mut send, recv) = conn.open_bi().await.map_err(|e| anyhow!("{e}"))?;
    let hello = Msg::Hello { peer_name: name, network: network.into(), proof: proto::proof(&secret, &shared.me) };
    proto::write_msg(&mut send, &hello).await?;

    let (tx, rx) = mpsc::channel(256);
    let result = run_link(shared, peer, network, &tx, rx, send, recv).await;
    cleanup_link(shared, peer, &tx);
    conn.close(0u32.into(), b"bye");
    result
}

fn cleanup_link(shared: &Shared, peer: &str, tx: &mpsc::Sender<Msg>) {
    let mut conns = shared.conns.lock().unwrap();
    if conns.get(peer).is_some_and(|t| t.same_channel(tx)) {
        conns.remove(peer);
        shared.links.lock().unwrap().remove(peer);
    }
    shared.pending.lock().unwrap().retain(|_, p| !p.tx.same_channel(tx));
}

/// One live link: forwards queued messages, sends heartbeats, applies what the peer says.
async fn run_link(
    shared: &Arc<Shared>,
    peer: &str,
    network: &str,
    tx: &mpsc::Sender<Msg>,
    mut rx: mpsc::Receiver<Msg>,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<()> {
    let mut beat = tokio::time::interval(HEARTBEAT);
    loop {
        tokio::select! {
            msg = tokio::time::timeout(DEAD_AFTER, proto::read_msg(&mut recv)) => {
                let msg = msg.map_err(|_| anyhow!("no heartbeat for {}s", DEAD_AFTER.as_secs()))??;
                if !apply(shared, peer, network, tx, msg)? {
                    return Ok(());
                }
            }
            out = rx.recv() => {
                let Some(out) = out else { return Ok(()) };
                let last = matches!(out, Msg::Reject { .. });
                proto::write_msg(&mut send, &out).await?;
                if last {
                    let _ = send.finish();
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    return Ok(());
                }
            }
            _ = beat.tick() => proto::write_msg(&mut send, &Msg::Heartbeat).await?,
        }
    }
}

/// Applies one message from a peer. Returns false when the link should end.
fn apply(shared: &Arc<Shared>, peer: &str, network: &str, tx: &mpsc::Sender<Msg>, msg: Msg) -> Result<bool> {
    match msg.clone() {
        Msg::Heartbeat | Msg::Pending | Msg::Hello { .. } => Ok(true),
        Msg::Approve { members, .. } => {
            // Only the peer we asked may admit us, and only into the network we asked for.
            set_members(shared, peer, network, members, true)?;
            shared.conns.lock().unwrap().insert(peer.to_string(), tx.clone());
            shared.redial.notify_one();
            sync_memory(shared, peer);
            Ok(true)
        }
        Msg::Members { network: n, members } => {
            if n != network {
                return Ok(true);
            }
            let still_member = members.iter().any(|m| m.id == shared.me);
            if still_member {
                set_members(shared, peer, network, members, false)?;
                shared.redial.notify_one();
            } else {
                let mut state = shared.state.lock().unwrap();
                state.networks.retain(|x| x.id != network);
                state.save()?;
                eprintln!("removed from network {network} by {}", short(peer));
            }
            Ok(still_member)
        }
        Msg::MemPut { entry } => {
            if is_live_member(shared, peer, tx) {
                if let Err(e) = receive_entry(shared, peer, &entry) {
                    eprintln!("memory entry from {} ignored: {e}", short(peer));
                }
            }
            Ok(true)
        }
        Msg::Busy { rooms } => {
            if is_live_member(shared, peer, tx) {
                shared.activity.lock().unwrap().insert(peer.to_string(), rooms);
            }
            Ok(true)
        }
        Msg::Status { capacity } => {
            if is_live_member(shared, peer, tx) {
                shared.capacity.lock().unwrap().insert(peer.to_string(), capacity);
            }
            Ok(true)
        }
        Msg::RoomInvite { room_id, topic, turn_budget, participants, project, mode, catch_up, seq, lean, .. } => {
            if is_live_member(shared, peer, tx) {
                let invite = Invite { room_id, topic, from: peer.to_string(), turn_budget, participants, rights: String::new(), project, mode, catch_up, seq, lean };
                on_invite(shared, peer, invite);
            }
            Ok(true)
        }
        Msg::RoomJoin { ref room_id }
        | Msg::RoomDecline { ref room_id, .. }
        | Msg::Say { ref room_id, .. }
        | Msg::Discuss { ref room_id, .. }
        | Msg::Work { ref room_id, .. }
        | Msg::Resend { ref room_id, .. }
        | Msg::RoomPing { ref room_id }
        | Msg::RoomClose { ref room_id, .. } => {
            if is_live_member(shared, peer, tx) {
                let known = match shared.rooms.lock().unwrap().get(room_id) {
                    Some(room) => {
                        let _ = room.tx.try_send(RoomEvent::Remote(peer.to_string(), msg.clone()));
                        true
                    }
                    None => false,
                };
                // After a restart this daemon no longer has the room. Say so,
                // so that the other side does not wait for it.
                if !known {
                    match msg {
                        Msg::Resend { .. } => {
                            let dir = room::room_dir(room_id);
                            let result = std::fs::read_to_string(dir.join("result.md")).unwrap_or_else(|_| "The room no longer exists on the opener's machine; its daemon may have restarted.".into());
                            shared.send_to(peer, Msg::RoomClose { room_id: room_id.clone(), reason: "closed".into(), result });
                        }
                        Msg::RoomPing { .. } => {
                            shared.send_to(peer, Msg::RoomDecline { room_id: room_id.clone(), reason: "its daemon restarted and lost the room".into() });
                        }
                        _ => {}
                    }
                }
            }
            Ok(true)
        }
        Msg::Reject { reason } => {
            let mut state = shared.state.lock().unwrap();
            let joining = state.network(network).is_some_and(|n| n.status == NetStatus::Joining);
            if joining {
                state.networks.retain(|x| x.id != network);
                state.save()?;
            }
            eprintln!("rejected by {}: {reason}", short(peer));
            Ok(false)
        }
    }
}

/// True if this link belongs to an approved peer. Room messages on a link that
/// still waits for approval are ignored.
fn is_live_member(shared: &Shared, peer: &str, tx: &mpsc::Sender<Msg>) -> bool {
    shared.conns.lock().unwrap().get(peer).is_some_and(|t| t.same_channel(tx))
}

fn member(shared: &Shared, id: &str) -> Option<Member> {
    let state = shared.state.lock().unwrap();
    state.networks.iter().flat_map(|n| n.peers.iter()).find(|m| m.id == id).cloned()
}

/// A peer invites us into a room. Peers we gave level 2 or higher start a session
/// at once; all others wait for `agora accept`.
fn on_invite(shared: &Arc<Shared>, peer: &str, mut invite: Invite) {
    let Some(m) = member(shared, peer) else { return };
    let room_id = invite.room_id.clone();
    if !room_id.chars().all(|c| c.is_ascii_alphanumeric()) || room_id.is_empty() || room_id.len() > 32 {
        return;
    }
    // The opener asks again when it has not heard from us: answer, do not join twice.
    if shared.rooms.lock().unwrap().contains_key(&room_id) {
        shared.send_to(peer, Msg::RoomJoin { room_id });
        return;
    }
    if shared.invites.lock().unwrap().contains_key(&room_id) {
        return;
    }
    invite.rights = m.rights.clone();
    let busy = shared.rooms.lock().unwrap().len() >= MAX_ROOMS;
    if busy {
        shared.send_to(peer, Msg::RoomDecline { room_id, reason: format!("already in {MAX_ROOMS} rooms") });
    } else if m.level >= 2 {
        start_joiner(shared, invite);
    } else {
        eprintln!("room invitation {room_id} from \"{}\": {}", m.name, invite.topic);
        event("invitation", &format!("\"{}\" invites to room {room_id}: {} (agora accept {room_id})", m.name, invite.topic));
        shared.invites.lock().unwrap().insert(room_id, invite);
    }
}

fn start_joiner(shared: &Arc<Shared>, invite: Invite) {
    let (tx, rx) = mpsc::channel(64);
    let id = invite.room_id.clone();
    shared.rooms.lock().unwrap().insert(id.clone(), RoomHandle { topic: invite.topic.clone(), role: "joined", tx, state: None });
    eprintln!("joining room {id}: {}", invite.topic.lines().next().unwrap_or(""));
    shared.announce_rooms();
    let shared = shared.clone();
    tokio::spawn(async move {
        if let Err(e) = room::run_joiner(shared.clone(), invite, rx).await {
            eprintln!("room {id} ended: {e}");
        }
        shared.rooms.lock().unwrap().remove(&id);
        shared.announce_rooms();
    });
}

/// Replaces our member list for a network with the one a peer sent.
/// `admit` is true for PEER_APPROVE, which may turn a joining network active.
fn set_members(shared: &Shared, from: &str, network: &str, members: Vec<Member>, admit: bool) -> Result<()> {
    let mut state = shared.state.lock().unwrap();
    let me = shared.me.clone();
    let Some(net) = state.network_mut(network) else { return Ok(()) };
    let trusted = match net.status {
        NetStatus::Joining => admit && net.opener == from,
        NetStatus::Active => net.peers.iter().any(|m| m.id == from),
    };
    if !trusted || !members.iter().any(|m| m.id == from) {
        bail!("ignoring member list from {} (not entitled)", short(from));
    }
    // Level and rights are local policy: keep what this machine decided for peers
    // it already knows, and start new ones at the defaults.
    let known: HashMap<String, (u8, String)> = net.peers.iter().map(|m| (m.id.clone(), (m.level, m.rights.clone()))).collect();
    net.peers = members
        .into_iter()
        .filter(|m| m.id != me)
        .map(|mut m| {
            let (level, rights) = known.get(&m.id).cloned().unwrap_or((1, "full".into()));
            m.level = level;
            m.rights = rights;
            m
        })
        .collect();
    net.status = NetStatus::Active;
    state.save()
}

fn all_members(me: &str, state: &State, net: &Network) -> Vec<Member> {
    let mut v = vec![Member { name: state.name.clone(), id: me.to_string(), level: 1, rights: "full".into() }];
    v.extend(net.peers.iter().cloned());
    v
}

fn short(id: &str) -> &str {
    &id[..10.min(id.len())]
}

/// Sends the current member list of a network to every connected member.
fn broadcast_members(shared: &Shared, network: &str) {
    let state = shared.state.lock().unwrap();
    let Some(net) = state.network(network) else { return };
    let msg = Msg::Members { network: network.into(), members: all_members(&shared.me, &state, net) };
    let conns = shared.conns.lock().unwrap();
    for m in &net.peers {
        if let Some(tx) = conns.get(&m.id) {
            let _ = tx.try_send(msg.clone());
        }
    }
}

// ------------------------------------------------------------ local control

async fn serve_control(shared: Arc<Shared>, stream: tokio::net::TcpStream, token: &str) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let req: Request = serde_json::from_str(&line).context("malformed control request")?;
    let resp = if req.token != token {
        Response { ok: false, text: "wrong control token".into() }
    } else if ["discuss", "wait", "say", "ask", "delegate"].contains(&req.cmd.as_str()) {
        match waiting_command(&shared, &req).await {
            Ok(text) => Response { ok: true, text },
            Err(e) => Response { ok: false, text: e.to_string() },
        }
    } else {
        match command(&shared, &req) {
            Ok(text) => Response { ok: true, text },
            Err(e) => Response { ok: false, text: e.to_string() },
        }
    };
    write.write_all(format!("{}\n", serde_json::to_string(&resp)?).as_bytes()).await?;
    Ok(())
}

const WARNING: &str = "WARNING: an approved peer can make this machine run commands. Connect only\n\
machines and people you trust with a shell on this computer. The connection is\n\
permanent until you run `agora revoke <peer>`.";

fn command(shared: &Arc<Shared>, req: &Request) -> Result<String> {
    let arg = |k: &str| req.args.get(k).cloned().ok_or_else(|| anyhow!("missing argument: {k}"));
    match req.cmd.as_str() {
        "status" => Ok(status(shared)),
        "peers" => Ok(peers(shared)),
        "open" => {
            let mut state = shared.state.lock().unwrap();
            let net = Network {
                id: data_encoding::HEXLOWER.encode(&rand::random::<[u8; 6]>()),
                secret: data_encoding::HEXLOWER.encode(&rand::random::<[u8; 16]>()),
                status: NetStatus::Active,
                opener: shared.me.clone(),
                peers: vec![],
            };
            let ticket = Ticket { network: net.id.clone(), secret: net.secret.clone(), opener: shared.me.clone() }.encode();
            let id = net.id.clone();
            state.networks.push(net);
            state.save()?;
            Ok(format!("{WARNING}\n\nNetwork {id} opened. On the other machine run:\n\n  agora join {ticket}\n\nThen approve the request here with `agora approve <name>`."))
        }
        "join" => {
            let t = Ticket::decode(&arg("ticket")?)?;
            if t.opener == shared.me {
                bail!("this is your own network");
            }
            let mut state = shared.state.lock().unwrap();
            if state.network(&t.network).is_some() {
                bail!("already a member of, or joining, network {}", t.network);
            }
            let words = proto::words(&shared.me, &t.opener);
            state.networks.push(Network { id: t.network.clone(), secret: t.secret, status: NetStatus::Joining, opener: t.opener, peers: vec![] });
            state.save()?;
            shared.redial.notify_one();
            Ok(format!("{WARNING}\n\nJoin request for network {} sent. Compare these four words with the other screen\nbefore the request is approved there:\n\n  {words}", t.network))
        }
        "approve" => {
            let name = arg("name")?;
            let level: u8 = req.args.get("level").map(|s| s.parse()).transpose().map_err(|_| anyhow!("level must be 1, 2 or 3"))?.unwrap_or(1);
            let rights = req.args.get("rights").cloned().unwrap_or_else(|| "full".into());
            if !(1..=3).contains(&level) || !["full", "read", "talk"].contains(&rights.as_str()) {
                bail!("level must be 1, 2 or 3 and rights one of full, read, talk");
            }
            let p = shared.pending.lock().unwrap().remove(&name).ok_or_else(|| anyhow!("no join request from \"{name}\" (see `agora status`)"))?;
            let members = {
                let mut state = shared.state.lock().unwrap();
                let net = state.network_mut(&p.network).ok_or_else(|| anyhow!("network is gone"))?;
                net.peers.retain(|m| m.id != p.id);
                net.peers.push(Member { name: name.clone(), id: p.id.clone(), level, rights: rights.clone() });
                state.save()?;
                all_members(&shared.me, &state, state.network(&p.network).unwrap())
            };
            shared.conns.lock().unwrap().insert(p.id.clone(), p.tx.clone());
            let my_name = shared.state.lock().unwrap().name.clone();
            p.tx.try_send(Msg::Approve { peer_name: my_name, members }).map_err(|_| anyhow!("the peer disconnected; it will ask again"))?;
            broadcast_members(shared, &p.network);
            sync_memory(shared, &p.id);
            Ok(format!("Approved \"{name}\" (level {level}, rights {rights}). Words were: {}", proto::words(&shared.me, &p.id)))
        }
        "reject" => {
            let name = arg("name")?;
            let p = shared.pending.lock().unwrap().remove(&name).ok_or_else(|| anyhow!("no join request from \"{name}\""))?;
            let _ = p.tx.try_send(Msg::Reject { reason: "rejected by the user".into() });
            Ok(format!("Rejected \"{name}\"."))
        }
        "revoke" => {
            let name = arg("name")?;
            let (network, id, rest) = {
                let mut state = shared.state.lock().unwrap();
                let me_member = Member { name: state.name.clone(), id: shared.me.clone(), level: 1, rights: "full".into() };
                let net = state.networks.iter_mut().find(|n| n.peers.iter().any(|m| m.name == name)).ok_or_else(|| anyhow!("no peer named \"{name}\""))?;
                let id = net.peers.iter().find(|m| m.name == name).unwrap().id.clone();
                net.peers.retain(|m| m.id != id);
                let mut rest = vec![me_member];
                rest.extend(net.peers.iter().cloned());
                let network = net.id.clone();
                state.save()?;
                (network, id, rest)
            };
            // Tell the revoked peer (its list no longer contains it), then drop the link.
            if let Some(tx) = shared.conns.lock().unwrap().remove(&id) {
                let _ = tx.try_send(Msg::Members { network: network.clone(), members: rest });
            }
            broadcast_members(shared, &network);
            Ok(format!("Revoked \"{name}\". It has to be approved again to rejoin."))
        }
        "trust" => {
            let name = arg("name")?;
            let level: u8 = arg("level")?.parse().map_err(|_| anyhow!("level must be 1, 2 or 3"))?;
            let rights = arg("rights")?;
            if !(1..=3).contains(&level) || !["full", "read", "talk"].contains(&rights.as_str()) {
                bail!("level must be 1, 2 or 3 and rights one of full, read, talk");
            }
            let mut state = shared.state.lock().unwrap();
            let m = state.networks.iter_mut().flat_map(|n| n.peers.iter_mut()).find(|m| m.name == name).ok_or_else(|| anyhow!("no peer named \"{name}\""))?;
            m.level = level;
            m.rights = rights.clone();
            state.save()?;
            Ok(format!("\"{name}\" now has level {level} and rights {rights} on this machine."))
        }
        "remember" => {
            let text = arg("text")?;
            let network = pick_network(shared, req.args.get("network"))?;
            let title = match req.args.get("title").filter(|t| !t.trim().is_empty()) {
                Some(t) => t.clone(),
                None => text.lines().next().unwrap_or("").chars().take(60).collect(),
            };
            let importance = req.args.get("importance").cloned().unwrap_or_else(|| "normal".into());
            let e = remember(shared, &network, &title, &text, &importance, false)?;
            Ok(format!("Saved \"{}\" as {} ({}) in network {network}.", e.title, e.id, e.importance))
        }
        "memory" => {
            let action = arg("action")?;
            let networks: Vec<String> = {
                let state = shared.state.lock().unwrap();
                state.networks.iter().filter(|n| n.status == NetStatus::Active).map(|n| n.id.clone()).collect()
            };
            if action == "list" {
                if networks.is_empty() {
                    bail!("this machine is in no network yet");
                }
                return Ok(networks.iter().map(|n| format!("network {n}  ({})\n{}", memory::dir(n).display(), memory::list(n))).collect::<Vec<_>>().join("\n"));
            }
            let id = arg("id")?;
            let found = networks.iter().find_map(|n| memory::load(n).remove(&id).filter(|e| !e.deleted));
            let e = found.ok_or_else(|| anyhow!("no entry {id} (see `agora memory`)"))?;
            match action.as_str() {
                "show" => Ok(format!("{} ({}, by {})\n\n{}", e.title, e.importance, e.author_name, e.body)),
                "forget" => {
                    remember(shared, &e.network, &e.title, "", &e.importance, true)?;
                    Ok(format!("Forgot {id} on all peers."))
                }
                other => bail!("unknown memory action: {other}"),
            }
        }
        "block" => {
            let action = arg("action")?;
            let mut state = shared.state.lock().unwrap();
            match action.as_str() {
                "list" => Ok(if state.blocklist.is_empty() { "the block list is empty".into() } else { state.blocklist.join("\n") }),
                "add" => {
                    let term = arg("term")?;
                    if term.trim().is_empty() {
                        bail!("the term is empty");
                    }
                    if !state.blocklist.contains(&term) {
                        state.blocklist.push(term.clone());
                        state.save()?;
                    }
                    Ok(format!("\"{term}\" will never be written to the shared memory from this machine."))
                }
                "remove" => {
                    let term = arg("term")?;
                    state.blocklist.retain(|t| *t != term);
                    state.save()?;
                    Ok(format!("Removed \"{term}\" from the block list."))
                }
                other => bail!("unknown block action: {other}"),
            }
        }
        "notify" => {
            let cmd = arg("command")?;
            let mut state = shared.state.lock().unwrap();
            state.notify = if cmd == "off" { None } else { Some(cmd.clone()) };
            state.save()?;
            *NOTIFY.lock().unwrap() = state.notify.clone();
            Ok(match &state.notify {
                Some(c) => format!("Every event now runs: {c}\nThe event is in $AGORA_EVENT_KIND and $AGORA_EVENT_TEXT."),
                None => "Events no longer run a command.".into(),
            })
        }
        "obsidian" => {
            let path = arg("path")?;
            let mut state = shared.state.lock().unwrap();
            if path == "off" {
                state.obsidian = None;
                state.save()?;
                return Ok("The shared memory is no longer mirrored.".into());
            }
            if !std::path::Path::new(&path).is_dir() {
                bail!("{path} is not a folder");
            }
            state.obsidian = Some(path.clone());
            state.save()?;
            Ok(format!("New and changed entries are mirrored to {path}/agora/<network>/."))
        }
        "project" => {
            let action = arg("action")?;
            if action == "list" {
                let state = shared.state.lock().unwrap();
                if state.projects.is_empty() {
                    return Ok("no projects registered".into());
                }
                return Ok(state.projects.iter().map(|p| match project::locate(std::path::Path::new(p)) {
                    Some((url, _)) => format!("{p}  ({url})"),
                    None => format!("{p}  (no longer a git repository with an origin)"),
                }).collect::<Vec<_>>().join("\n"));
            }
            let path = arg("path")?;
            match action.as_str() {
                "add" => {
                    let (url, dir) = project::locate(std::path::Path::new(&path)).ok_or_else(|| anyhow!("{path} is not a git repository with an origin remote"))?;
                    remember_project(shared, &dir.to_string_lossy())?;
                    Ok(format!("Registered {} for project {url}.", dir.display()))
                }
                "remove" => {
                    let mut state = shared.state.lock().unwrap();
                    let before = state.projects.len();
                    state.projects.retain(|p| *p != path && *p != path.trim_end_matches('/'));
                    state.save()?;
                    Ok(if state.projects.len() < before { format!("Removed {path}.") } else { format!("{path} was not registered.") })
                }
                other => bail!("unknown project action: {other}"),
            }
        }
        "rooms" => Ok(rooms(shared)),
        "accept" => {
            let id = arg("room")?;
            let invite = shared.invites.lock().unwrap().remove(&id).ok_or_else(|| anyhow!("no invitation for room {id}"))?;
            start_joiner(shared, invite);
            Ok(format!("Joined room {id}. Follow it with `agora watch {id}`."))
        }
        "decline" => {
            let id = arg("room")?;
            let invite = shared.invites.lock().unwrap().remove(&id).ok_or_else(|| anyhow!("no invitation for room {id}"))?;
            shared.send_to(&invite.from, Msg::RoomDecline { room_id: id.clone(), reason: "declined by the user".into() });
            Ok(format!("Declined room {id}."))
        }
        "invite" => {
            let id = arg("room")?;
            let name = arg("name")?;
            let m = {
                let state = shared.state.lock().unwrap();
                let conns = shared.conns.lock().unwrap();
                state.networks.iter().flat_map(|n| n.peers.iter()).find(|m| m.name == name && conns.contains_key(&m.id)).cloned()
            };
            let m = m.ok_or_else(|| anyhow!("no peer named \"{name}\" is online"))?;
            let rooms = shared.rooms.lock().unwrap();
            let room = rooms.get(&id).filter(|r| r.role == "opened").ok_or_else(|| anyhow!("no room {id} opened on this machine"))?;
            room.tx.try_send(RoomEvent::Invite(m)).map_err(|_| anyhow!("the room is not taking requests right now"))?;
            Ok(format!("Invited \"{name}\" into room {id}. It gets a summary of what was said and speaks when its turn comes."))
        }
        "panic" => {
            let rooms = shared.rooms.lock().unwrap();
            for room in rooms.values() {
                let _ = room.tx.try_send(RoomEvent::Panic);
            }
            let n = rooms.len();
            shared.invites.lock().unwrap().clear();
            Ok(format!("Stopped {n} room(s) and their sessions on this machine."))
        }
        other => bail!("unknown command: {other}"),
    }
}

/// Commands that wait for something to happen in a room.
async fn waiting_command(shared: &Arc<Shared>, req: &Request) -> Result<String> {
    let arg = |k: &str| req.args.get(k).cloned().ok_or_else(|| anyhow!("missing argument: {k}"));
    match req.cmd.as_str() {
        "discuss" => discuss(shared, req).await,
        "ask" | "delegate" => {
            let mode = if req.cmd == "delegate" { "task" } else { "ask" };
            let question = arg("question")?;
            let peer = {
                let state = shared.state.lock().unwrap();
                let conns = shared.conns.lock().unwrap();
                let online: Vec<&Member> = state.networks.iter().flat_map(|n| n.peers.iter()).filter(|m| conns.contains_key(&m.id)).collect();
                match req.args.get("with").filter(|w| !w.is_empty()) {
                    Some(name) => online.iter().find(|m| m.name == *name).map(|m| (*m).clone()).ok_or_else(|| anyhow!("no peer named \"{name}\" is online"))?,
                    None => match online.as_slice() {
                        [] => bail!("no peer is online"),
                        [one] => (*one).clone(),
                        _ => bail!("several peers are online; name one with --with"),
                    },
                }
            };
            let project = project_of(shared, req)?;
            let id = data_encoding::HEXLOWER.encode(&rand::random::<[u8; 4]>());
            let (tx, rx) = mpsc::channel(64);
            let role = if mode == "task" { "delegated" } else { "asking" };
            shared.rooms.lock().unwrap().insert(id.clone(), RoomHandle { topic: question.clone(), role, tx, state: None });
            shared.announce_rooms();
            let result = room::run_ask(shared.clone(), id.clone(), question, peer, project, mode, rx).await;
            shared.rooms.lock().unwrap().remove(&id);
            shared.announce_rooms();
            result
        }
        "wait" => {
            let id = arg("room")?;
            let state = shared.rooms.lock().unwrap().get(&id).and_then(|r| r.state.clone());
            match state {
                Some(rx) => wait_room(&id, rx).await,
                None => {
                    let dir = room::room_dir(&id);
                    std::fs::read_to_string(dir.join("closing.md"))
                        .or_else(|_| std::fs::read_to_string(dir.join("result.md")))
                        .map_err(|_| anyhow!("no room {id} with a result on this machine"))
                }
            }
        }
        "say" => {
            let id = arg("room")?;
            let extra: u32 = req.args.get("turns").map(|s| s.parse()).transpose().map_err(|_| anyhow!("turns must be a number"))?.unwrap_or(0);
            let (tx, state) = {
                let rooms = shared.rooms.lock().unwrap();
                let room = rooms.get(&id).ok_or_else(|| anyhow!("no running room {id}"))?;
                (room.tx.clone(), room.state.clone())
            };
            tx.try_send(RoomEvent::Human(arg("text")?, extra)).map_err(|_| anyhow!("the room is not taking messages right now"))?;
            // If the room was waiting for this answer, return once it runs again,
            // so a following `agora wait` does not see the old question.
            if let Some(mut state) = state {
                let paused = matches!(*state.borrow(), RoomState::Paused(_));
                if paused {
                    let _ = tokio::time::timeout(Duration::from_secs(3), state.changed()).await;
                }
            }
            Ok("Sent.".into())
        }
        other => bail!("unknown command: {other}"),
    }
}

/// Waits until a room opened here pauses for a human or closes.
async fn wait_room(id: &str, mut rx: watch::Receiver<RoomState>) -> Result<String> {
    loop {
        let state = rx.borrow_and_update().clone();
        match state {
            RoomState::Running => {
                if rx.changed().await.is_err() {
                    bail!("room {id} ended without a result");
                }
            }
            RoomState::Paused(question) => {
                return Ok(format!(
                    "[room {id} is paused: a human has to decide]\n{question}\n\nAnswer with `agora say {id} \"<answer>\"` (add `--turns N` to grant more turns), then run `agora wait {id}` for the result."
                ));
            }
            RoomState::Closed(Ok(text)) => return Ok(text),
            RoomState::Closed(Err(e)) => bail!("{e}"),
        }
    }
}

/// Opens a room and waits until it pauses or closes. Returns the text for the human.
async fn discuss(shared: &Arc<Shared>, req: &Request) -> Result<String> {
    let topic = req.args.get("topic").cloned().ok_or_else(|| anyhow!("missing topic"))?;
    let turns: u32 = req.args.get("turns").map(|s| s.parse()).transpose().map_err(|_| anyhow!("turns must be a number"))?.unwrap_or(6);
    if !(1..=30).contains(&turns) {
        bail!("turns must be between 1 and 30");
    }
    let lean = req.args.get("lean").is_some_and(|v| v == "1");
    let tokens: u64 = req.args.get("tokens").map(|s| s.parse()).transpose().map_err(|_| anyhow!("tokens must be a number"))?.unwrap_or(1_000_000);
    let with = req.args.get("with").cloned().unwrap_or_default();
    let mut joiners = Vec::new();
    {
        let state = shared.state.lock().unwrap();
        let conns = shared.conns.lock().unwrap();
        let all: Vec<&Member> = state.networks.iter().flat_map(|n| n.peers.iter()).collect();
        let wanted: Vec<&str> = with.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        if wanted.is_empty() {
            joiners.extend(all.iter().filter(|m| conns.contains_key(&m.id)).map(|m| (*m).clone()));
        }
        for name in wanted {
            let m = all.iter().find(|m| m.name == name).ok_or_else(|| anyhow!("no peer named \"{name}\""))?;
            if !conns.contains_key(&m.id) {
                bail!("peer \"{name}\" is offline");
            }
            joiners.push((*m).clone());
        }
    }
    if joiners.is_empty() {
        bail!("no peer is online to discuss with");
    }
    let remember_as = req.args.get("remember").cloned().unwrap_or_else(|| "normal".into());
    if remember_as != "none" && !memory::IMPORTANCE.contains(&remember_as.as_str()) {
        bail!("--remember must be one of high, normal, low, none");
    }
    let network = {
        let state = shared.state.lock().unwrap();
        state.networks.iter().find(|n| n.peers.iter().any(|m| m.id == joiners[0].id)).map(|n| n.id.clone()).unwrap_or_default()
    };
    if shared.rooms.lock().unwrap().len() >= MAX_ROOMS {
        bail!("already in {MAX_ROOMS} rooms");
    }
    let project = project_of(shared, req)?;
    let id = data_encoding::HEXLOWER.encode(&rand::random::<[u8; 4]>());
    let (tx, rx) = mpsc::channel(64);
    let (state_tx, state_rx) = watch::channel(RoomState::Running);
    shared.rooms.lock().unwrap().insert(id.clone(), RoomHandle { topic: topic.clone(), role: "opened", tx, state: Some(state_rx.clone()) });
    eprintln!("room {id} opened: {topic}");
    shared.announce_rooms();
    // The room outlives this request: it may pause for a human and go on later.
    let (sh, rid) = (shared.clone(), id.clone());
    let note_topic = topic.clone();
    tokio::spawn(async move {
        let result = room::run_opener(sh.clone(), rid.clone(), topic, joiners, turns, tokens, lean, project, rx, state_tx.clone()).await;
        if let Ok(text) = &result {
            event("room-closed", &format!("{rid}: {note_topic} (agora wait {rid})"));
            // The result goes into the shared memory so every peer keeps it.
            if remember_as != "none" {
                let title = format!("Room {rid}: {}", note_topic.chars().take(60).collect::<String>());
                if let Err(e) = remember(&sh, &network, &title, text, &remember_as, false) {
                    eprintln!("room {rid}: result not saved to the shared memory: {e}");
                }
            }
        }
        let _ = state_tx.send(RoomState::Closed(result.map_err(|e| e.to_string())));
        sh.rooms.lock().unwrap().remove(&rid);
        sh.announce_rooms();
    });
    wait_room(&id, state_rx).await
}

/// The project a command is about: the git repository it was called in, unless
/// `project` names another or says "none". The clone is registered on the way.
fn project_of(shared: &Shared, req: &Request) -> Result<Option<(String, std::path::PathBuf)>> {
    let wanted = req.args.get("project").cloned().unwrap_or_default();
    if wanted == "none" {
        return Ok(None);
    }
    let start = if wanted.is_empty() { req.args.get("cwd").cloned().unwrap_or_default() } else { wanted.clone() };
    let found = project::locate(std::path::Path::new(&start));
    if found.is_none() && !wanted.is_empty() {
        bail!("{wanted} is not a git repository with an origin remote");
    }
    if let Some((_, dir)) = &found {
        remember_project(shared, &dir.to_string_lossy())?;
    }
    Ok(found)
}

fn remember_project(shared: &Shared, dir: &str) -> Result<()> {
    let mut state = shared.state.lock().unwrap();
    if !state.projects.iter().any(|p| p == dir) {
        state.projects.push(dir.to_string());
        state.save()?;
    }
    Ok(())
}

fn rooms(shared: &Shared) -> String {
    let mut out = String::new();
    for (id, r) in shared.rooms.lock().unwrap().iter() {
        out.push_str(&format!("room {id}  {}  {}\n", r.role, r.topic));
    }
    for (id, i) in shared.invites.lock().unwrap().iter() {
        let from = i.participants.iter().find(|p| p.id == i.from).map(|p| p.name.as_str()).unwrap_or("?");
        out.push_str(&format!("invitation {id} from \"{from}\": {}  (agora accept {id})\n", i.topic));
    }
    if out.is_empty() {
        out.push_str("no rooms\n");
    }
    out
}

/// Everyone in this machine's networks at a glance: reachable or not, what
/// it is working on, and how much of its usage limit is left.
fn peers(shared: &Shared) -> String {
    let state = shared.state.lock().unwrap();
    let conns = shared.conns.lock().unwrap();
    let links = shared.links.lock().unwrap();
    let capacity = shared.capacity.lock().unwrap();
    let activity = shared.activity.lock().unwrap();
    let limit = |id: &str| capacity.get(id).map(|c| c.describe()).unwrap_or_else(|| "limit unknown until a session has run there".into());
    let busy = |n: u32| match n {
        0 => "idle".to_string(),
        1 => "working on 1 room".to_string(),
        n => format!("working on {n} rooms"),
    };
    let mine = shared.rooms.lock().unwrap().len() as u32;
    let mut out = format!("{:<16} {:<22} {:<20} {}\n", state.name, "this machine", busy(mine), limit(&shared.me));
    let mut seen = HashSet::new();
    for m in state.networks.iter().flat_map(|n| n.peers.iter()) {
        if !seen.insert(m.id.clone()) {
            continue;
        }
        if !conns.contains_key(&m.id) {
            out.push_str(&format!("{:<16} offline\n", m.name));
            continue;
        }
        let path = links.get(&m.id).map(|c| {
            let paths = c.paths();
            let direct = paths.iter().any(|p| p.is_ip());
            let rtt = paths.iter().map(|p| p.rtt()).min();
            format!("online ({}{})", if direct { "direct" } else { "relay" }, rtt.map(|r| format!(", {} ms", r.as_millis())).unwrap_or_default())
        });
        let work = busy(activity.get(&m.id).copied().unwrap_or(0));
        out.push_str(&format!("{:<16} {:<22} {:<20} {}\n", m.name, path.unwrap_or_else(|| "online".into()), work, limit(&m.id)));
    }
    if seen.is_empty() {
        out.push_str("no peers yet: open a network with `agora open`\n");
    }
    out
}

fn status(shared: &Shared) -> String {
    let state = shared.state.lock().unwrap();
    let conns = shared.conns.lock().unwrap();
    let pending = shared.pending.lock().unwrap();
    let links = shared.links.lock().unwrap();
    // How a peer is reached: a direct path, or only through a relay.
    let path = |id: &str| {
        links.get(id).map(|c| {
            let paths = c.paths();
            let direct = paths.iter().any(|p| p.is_ip());
            let rtt = paths.iter().map(|p| p.rtt()).min();
            format!(" ({}{})", if direct { "direct" } else { "relay" }, rtt.map(|r| format!(", {} ms", r.as_millis())).unwrap_or_default())
        })
        .unwrap_or_default()
    };
    let capacity = shared.capacity.lock().unwrap();
    let cap = |id: &str| capacity.get(id).map(|c| format!("  {}", c.describe())).unwrap_or_default();
    let mut out = format!("peer \"{}\"  id {}{}\n", state.name, shared.me, cap(&shared.me));
    if state.networks.is_empty() {
        out.push_str("no networks\n");
    }
    for net in &state.networks {
        match net.status {
            NetStatus::Joining => out.push_str(&format!(
                "network {}  waiting for approval, words: {}\n",
                net.id,
                proto::words(&shared.me, &net.opener)
            )),
            NetStatus::Active => {
                out.push_str(&format!("network {}  {} peer(s)\n", net.id, net.peers.len()));
                for m in &net.peers {
                    let link = if conns.contains_key(&m.id) { format!("online{}", path(&m.id)) } else { "offline".to_string() };
                    out.push_str(&format!("  {:<16} {:<8} level {} rights {}{}\n", m.name, link, m.level, m.rights, cap(&m.id)));
                }
            }
        }
    }
    for (name, p) in pending.iter() {
        out.push_str(&format!("join request: \"{name}\" for network {}, words: {}\n", p.network, proto::words(&shared.me, &p.id)));
    }
    drop(pending);
    drop(conns);
    drop(state);
    drop(capacity);
    drop(links);
    let r = rooms(shared);
    if r != "no rooms\n" {
        out.push_str(&r);
    }
    out
}
