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

use crate::project;
use crate::proto::{self, ALPN, Member, Msg, Ticket};
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
    /// Invitations waiting for `agora accept`.
    invites: Mutex<HashMap<String, Invite>>,
    /// Join requests waiting for a human, by nickname.
    pending: Mutex<HashMap<String, PendingJoin>>,
    dialing: Mutex<HashSet<String>>,
    redial: Notify,
}

impl Shared {
    /// Queues a message for an approved, connected peer. False if it is offline.
    pub fn send_to(&self, peer: &str, msg: Msg) -> bool {
        match self.conns.lock().unwrap().get(peer) {
            Some(tx) => tx.try_send(msg).is_ok(),
            None => false,
        }
    }
}

pub fn secret_key_from_hex(hex: &str) -> Result<SecretKey> {
    let bytes = data_encoding::HEXLOWER.decode(hex.as_bytes()).map_err(|_| anyhow!("secret key in state.json is damaged"))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("secret key has the wrong length"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

pub async fn run() -> Result<()> {
    let state = State::load()?.ok_or_else(|| anyhow!("not set up yet: run `agora setup --name <peer-name>` first"))?;
    let secret = secret_key_from_hex(&state.secret_key)?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
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
        invites: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        dialing: Mutex::new(HashSet::new()),
        redial: Notify::new(),
    });
    eprintln!("agora daemon running, peer id {}", shared.me);

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

    let (tx, rx) = mpsc::channel(32);
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
    let (mut send, recv) = conn.open_bi().await.map_err(|e| anyhow!("{e}"))?;
    let hello = Msg::Hello { peer_name: name, network: network.into(), proof: proto::proof(&secret, &shared.me) };
    proto::write_msg(&mut send, &hello).await?;

    let (tx, rx) = mpsc::channel(32);
    let result = run_link(shared, peer, network, &tx, rx, send, recv).await;
    cleanup_link(shared, peer, &tx);
    conn.close(0u32.into(), b"bye");
    result
}

fn cleanup_link(shared: &Shared, peer: &str, tx: &mpsc::Sender<Msg>) {
    let mut conns = shared.conns.lock().unwrap();
    if conns.get(peer).is_some_and(|t| t.same_channel(tx)) {
        conns.remove(peer);
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
        Msg::RoomInvite { room_id, topic, turn_budget, participants, project, .. } => {
            if is_live_member(shared, peer, tx) {
                on_invite(shared, peer, room_id, topic, turn_budget, participants, project);
            }
            Ok(true)
        }
        Msg::RoomJoin { ref room_id }
        | Msg::RoomDecline { ref room_id, .. }
        | Msg::Say { ref room_id, .. }
        | Msg::Discuss { ref room_id, .. }
        | Msg::RoomClose { ref room_id, .. } => {
            if is_live_member(shared, peer, tx) {
                if let Some(room) = shared.rooms.lock().unwrap().get(room_id) {
                    let _ = room.tx.try_send(RoomEvent::Remote(peer.to_string(), msg.clone()));
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
fn on_invite(
    shared: &Arc<Shared>,
    peer: &str,
    room_id: String,
    topic: String,
    turn_budget: u32,
    participants: Vec<proto::Participant>,
    project: Option<String>,
) {
    let Some(m) = member(shared, peer) else { return };
    if !room_id.chars().all(|c| c.is_ascii_alphanumeric()) || room_id.is_empty() || room_id.len() > 32 {
        return;
    }
    let invite = Invite { room_id: room_id.clone(), topic, from: peer.to_string(), turn_budget, participants, rights: m.rights.clone(), project };
    let busy = shared.rooms.lock().unwrap().len() >= MAX_ROOMS;
    if busy {
        shared.send_to(peer, Msg::RoomDecline { room_id, reason: format!("already in {MAX_ROOMS} rooms") });
    } else if m.level >= 2 {
        start_joiner(shared, invite);
    } else {
        eprintln!("room invitation {room_id} from \"{}\": {}", m.name, invite.topic);
        shared.invites.lock().unwrap().insert(room_id, invite);
    }
}

fn start_joiner(shared: &Arc<Shared>, invite: Invite) {
    let (tx, rx) = mpsc::channel(64);
    let id = invite.room_id.clone();
    shared.rooms.lock().unwrap().insert(id.clone(), RoomHandle { topic: invite.topic.clone(), role: "joined", tx, state: None });
    let shared = shared.clone();
    tokio::spawn(async move {
        if let Err(e) = room::run_joiner(shared.clone(), invite, rx).await {
            eprintln!("room {id} ended: {e}");
        }
        shared.rooms.lock().unwrap().remove(&id);
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
    } else if ["discuss", "wait", "say"].contains(&req.cmd.as_str()) {
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
    if shared.rooms.lock().unwrap().len() >= MAX_ROOMS {
        bail!("already in {MAX_ROOMS} rooms");
    }
    // The project is the git repository the command was called in, unless told otherwise.
    let wanted = req.args.get("project").cloned().unwrap_or_default();
    let project = if wanted == "none" {
        None
    } else {
        let start = if wanted.is_empty() { req.args.get("cwd").cloned().unwrap_or_default() } else { wanted.clone() };
        let found = project::locate(std::path::Path::new(&start));
        if found.is_none() && !wanted.is_empty() {
            bail!("{wanted} is not a git repository with an origin remote");
        }
        found
    };
    if let Some((_, dir)) = &project {
        remember_project(shared, &dir.to_string_lossy())?;
    }
    let id = data_encoding::HEXLOWER.encode(&rand::random::<[u8; 4]>());
    let (tx, rx) = mpsc::channel(64);
    let (state_tx, state_rx) = watch::channel(RoomState::Running);
    shared.rooms.lock().unwrap().insert(id.clone(), RoomHandle { topic: topic.clone(), role: "opened", tx, state: Some(state_rx.clone()) });
    eprintln!("room {id} opened: {topic}");
    // The room outlives this request: it may pause for a human and go on later.
    let (sh, rid) = (shared.clone(), id.clone());
    tokio::spawn(async move {
        let result = room::run_opener(sh.clone(), rid.clone(), topic, joiners, turns, tokens, project, rx, state_tx.clone()).await;
        let _ = state_tx.send(RoomState::Closed(result.map_err(|e| e.to_string())));
        sh.rooms.lock().unwrap().remove(&rid);
    });
    wait_room(&id, state_rx).await
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

fn status(shared: &Shared) -> String {
    let state = shared.state.lock().unwrap();
    let conns = shared.conns.lock().unwrap();
    let pending = shared.pending.lock().unwrap();
    let mut out = format!("peer \"{}\"  id {}\n", state.name, shared.me);
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
                    let link = if conns.contains_key(&m.id) { "online" } else { "offline" };
                    out.push_str(&format!("  {:<16} {:<8} level {} rights {}\n", m.name, link, m.level, m.rights));
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
    let r = rooms(shared);
    if r != "no rooms\n" {
        out.push_str(&r);
    }
    out
}
