// SPDX-License-Identifier: GPL-3.0-or-later

//! The coordinator's side of running work on other Macs (`docs/multi-machine-nodes-plan.md`,
//! §3.2-§3.4): finding nodes (mDNS, plus `[nodes] static`), pairing with one, and asking each
//! paired node every few seconds whether it can take work. What the poll learns is kept in a
//! cache that placement reads (phase 2); this module never runs a job itself.
//!
//! A node found over mDNS is only *listed*. It is used once the owner has paired with it, and
//! after that every request is mutual TLS with the coordinator's certificate and the node's
//! pinned fingerprint (`node_client`).

use crate::config::{load_nodes, NodesCfg};
use crate::daemon::{now_ms, Daemon};
use crate::node_client::{ClientError, NodeClient};
use crate::node_serve::NODE_PROTO;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A node seen over mDNS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovered {
    pub nid: String,
    /// The host's name from its mDNS record ("studio.local." becomes "studio").
    pub name: String,
    /// IPv4 addresses first.
    pub addrs: Vec<String>,
    pub port: u16,
    pub fingerprint: String,
    pub node_proto: u32,
    pub version: String,
    pub arch: String,
    /// The mDNS instance, so a removal can find it.
    pub fullname: String,
}

/// A node's record, from the TXT keys of §3.2. `None` for a service that is not a node, or one
/// that lacks what we need to reach it.
pub fn parse_txt(
    txt: &BTreeMap<String, String>,
    hostname: &str,
    fullname: &str,
    addrs: &[std::net::IpAddr],
) -> Option<Discovered> {
    if txt.get("node").map(String::as_str) != Some("1") {
        return None;
    }
    let port: u16 = txt.get("nport")?.parse().ok().filter(|p| *p > 0)?;
    let fingerprint = txt.get("fp").filter(|f| !f.is_empty())?.clone();
    let nid = txt.get("nid").filter(|n| !n.is_empty())?.clone();
    let mut a: Vec<std::net::IpAddr> = addrs.to_vec();
    a.sort_by_key(|x| (x.is_ipv6(), *x));
    Some(Discovered {
        nid,
        name: hostname
            .trim_end_matches('.')
            .trim_end_matches(".local")
            .to_string(),
        addrs: a.iter().map(|x| x.to_string()).collect(),
        port,
        fingerprint,
        node_proto: txt
            .get("node_proto")
            .and_then(|p| p.parse().ok())
            .unwrap_or(0),
        version: txt.get("version").cloned().unwrap_or_default(),
        arch: txt.get("arch").cloned().unwrap_or_default(),
        fullname: fullname.to_string(),
    })
}

/// Where a paired node stands, from the last polls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeState {
    /// Paired, and not asked yet.
    NotPolled,
    Accepting,
    /// Reachable, and not taking work; the node's own words.
    NotAccepting(String),
    /// Did not answer the last `n` polls, fewer than the limit.
    Silent(u32),
    /// Did not answer `lost_after_polls` in a row.
    Lost(u32),
    /// The node no longer knows this coordinator (403).
    NotPaired,
    /// The node presented another certificate than the one paired.
    WrongCertificate,
    /// A different node protocol.
    Incompatible(String),
    /// macOS will not let MASON on this Mac reach the local network (§3.14).
    LocalNetworkDenied,
    /// macOS has a Local Network rule for `masond` that it cannot apply (s1118).
    LocalNetworkUnapplied,
}

impl NodeState {
    pub fn usable(&self) -> bool {
        matches!(self, NodeState::Accepting)
    }

    /// The words `mason node list` shows.
    pub fn text(&self) -> String {
        match self {
            NodeState::NotPolled => "not asked yet".into(),
            NodeState::Accepting => "accepting".into(),
            NodeState::NotAccepting(why) => format!("not accepting ({why})"),
            NodeState::Silent(n) => format!("no answer ({n} in a row)"),
            NodeState::Lost(n) => format!("lost ({n} polls without an answer)"),
            NodeState::NotPaired => "not paired (403): the node forgot this coordinator".into(),
            NodeState::WrongCertificate => {
                "its certificate is not the one paired; pair it again".into()
            }
            NodeState::Incompatible(m) => format!("incompatible ({m})"),
            NodeState::LocalNetworkDenied => "local network access denied by macOS (or no route to it): turn on Local Network for MASON in System Settings → Privacy & Security → Local Network; if it is already on, see docs/nodes.md".into(),
            NodeState::LocalNetworkUnapplied => crate::node_client::LOCAL_NETWORK_UNAPPLIED.into(),
        }
    }
}

/// What the last poll of one node left.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// The node's own status JSON, from the last answer.
    pub status: Option<Value>,
    /// Failed polls in a row.
    pub failures: u32,
    pub fault: Option<Fault>,
    pub polled_ms: u64,
    pub last_error: Option<String>,
}

/// A poll that got an answer, but not a good one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    NotPaired,
    WrongCertificate,
    Incompatible(String),
    LocalNetworkDenied,
    LocalNetworkUnapplied,
}

/// The state a view amounts to. Pure.
pub fn state_of(v: &View, lost_after: u32) -> NodeState {
    match &v.fault {
        Some(Fault::NotPaired) => return NodeState::NotPaired,
        Some(Fault::WrongCertificate) => return NodeState::WrongCertificate,
        Some(Fault::Incompatible(m)) => return NodeState::Incompatible(m.clone()),
        Some(Fault::LocalNetworkDenied) => return NodeState::LocalNetworkDenied,
        Some(Fault::LocalNetworkUnapplied) => return NodeState::LocalNetworkUnapplied,
        None => {}
    }
    if v.failures >= lost_after.max(1) {
        return NodeState::Lost(v.failures);
    }
    if v.failures > 0 {
        return NodeState::Silent(v.failures);
    }
    let Some(s) = &v.status else {
        return NodeState::NotPolled;
    };
    if s["accepting"].as_bool() == Some(true) {
        NodeState::Accepting
    } else {
        NodeState::NotAccepting(
            s["why_not"]
                .as_str()
                .unwrap_or("it did not say why")
                .to_string(),
        )
    }
}

/// What the registry knows.
pub struct Nodes {
    inner: Mutex<Inner>,
    /// What building and sending a runtime to a node go through; tests stand in for it.
    io: Mutex<Option<Arc<dyn crate::node_fleet::FleetIo>>>,
}

#[derive(Default)]
struct Inner {
    discovered: BTreeMap<String, Discovered>,
    views: BTreeMap<String, View>,
    identity: Option<(Vec<u8>, Vec<u8>)>,
    /// The paired nodes as of the last poll (or add or remove), so a placement decision reads no
    /// store.
    paired: Vec<mason_store::NodeRow>,
    /// `[nodes]` as last read, by configuration directory, with the file's stamp.
    cfg: BTreeMap<std::path::PathBuf, (Option<(std::time::SystemTime, u64)>, NodesCfg)>,
}

impl Default for Nodes {
    fn default() -> Self {
        Nodes::new()
    }
}

impl Nodes {
    pub fn new() -> Self {
        Nodes {
            inner: Mutex::new(Inner::default()),
            io: Mutex::new(None),
        }
    }

    /// The paired nodes and what the last poll of each left.
    pub fn snapshot(&self) -> (Vec<mason_store::NodeRow>, BTreeMap<String, View>) {
        let g = self.lock();
        (g.paired.clone(), g.views.clone())
    }

    /// How a runtime is built and sent: the real way unless a test set another.
    pub fn io(&self) -> Arc<dyn crate::node_fleet::FleetIo> {
        self.io
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| Arc::new(crate::node_fleet::RealIo))
    }

    /// Stand in for building and sending (tests).
    pub fn set_io(&self, io: Arc<dyn crate::node_fleet::FleetIo>) {
        *self.io.lock().unwrap_or_else(|e| e.into_inner()) = Some(io);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn see(&self, n: Discovered) {
        self.lock().discovered.insert(n.nid.clone(), n);
    }

    pub fn forget_instance(&self, fullname: &str) {
        self.lock().discovered.retain(|_, n| n.fullname != fullname);
    }

    pub fn discovered(&self) -> Vec<Discovered> {
        self.lock().discovered.values().cloned().collect()
    }

    /// The paired nodes that are taking work and have what a run needs, as `(row, host, port,
    /// status)` with the address to reach each at, the coolest die first.
    pub fn usable(
        &self,
        cfg: &NodesCfg,
        suite_name: &str,
        command: &str,
    ) -> Vec<(mason_store::NodeRow, String, u16, Value)> {
        let g = self.lock();
        let mut out: Vec<(mason_store::NodeRow, String, u16, Value)> = g
            .paired
            .iter()
            .filter_map(|r| {
                let v = g.views.get(&r.nid)?;
                if !state_of(v, cfg.lost_after_polls).usable() {
                    return None;
                }
                let s = v.status.clone()?;
                if !covers_run(&s, suite_name, command) {
                    return None;
                }
                let (host, port) = endpoint(r, g.discovered.get(&r.nid));
                Some((r.clone(), host, port, s))
            })
            .collect();
        out.sort_by(|a, b| {
            let die = |s: &Value| s["thermal"]["die_c"].as_f64().unwrap_or(f64::MAX);
            die(&a.3)
                .partial_cmp(&die(&b.3))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.name.cmp(&b.0.name))
        });
        out
    }

    /// A client for the paired node `nid` whatever state it is in (a job it already has is
    /// followed to its end even when the node has since stopped taking new ones), with the
    /// node's architecture from its last status.
    pub fn client_for(
        &self,
        d: &Daemon,
        nid: &str,
    ) -> Option<(mason_store::NodeRow, NodeClient, String)> {
        let (row, host, port, arch) = {
            let g = self.lock();
            let row = g.paired.iter().find(|r| r.nid == nid)?.clone();
            let (host, port) = endpoint(&row, g.discovered.get(nid));
            let arch = g
                .views
                .get(nid)
                .and_then(|v| v.status.as_ref())
                .and_then(|s| s["arch"].as_str())
                .unwrap_or_default()
                .to_string();
            (row, host, port, arch)
        };
        let identity = self.identity(d).ok()?;
        let client = NodeClient::new(identity, &host, port, &row.fingerprint).ok()?;
        Some((row, client.with_timeout(Duration::from_secs(60)), arch))
    }

    pub fn view(&self, nid: &str) -> Option<View> {
        self.lock().views.get(nid).cloned()
    }

    fn set_view(&self, nid: &str, v: View) {
        self.lock().views.insert(nid.to_string(), v);
    }

    fn forget_view(&self, nid: &str) {
        self.lock().views.remove(nid);
    }

    fn set_paired(&self, rows: Vec<mason_store::NodeRow>) {
        self.lock().paired = rows;
    }

    /// `[nodes]` from `daemon.toml`, read again only when the file changed. A file that does not
    /// parse reads as no nodes.
    pub fn cfg(&self, config_dir: &std::path::Path) -> NodesCfg {
        let path = config_dir.join("daemon.toml");
        let stamp = std::fs::metadata(&path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        let mut g = self.lock();
        if let Some((s, c)) = g.cfg.get(config_dir) {
            if *s == stamp {
                return c.clone();
            }
        }
        let c = load_nodes(config_dir).unwrap_or_default();
        g.cfg.insert(config_dir.to_path_buf(), (stamp, c.clone()));
        c
    }

    fn identity(&self, d: &Daemon) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
        let mut g = self.lock();
        if let Some(i) = &g.identity {
            return Ok(i.clone());
        }
        let i = crate::lan::load_or_create_identity(&d.cfg.data_dir)?;
        g.identity = Some(i.clone());
        Ok(i)
    }
}

fn paired(d: &Daemon) -> Vec<mason_store::NodeRow> {
    d.store.list_nodes().unwrap_or_default()
}

/// The address to reach a paired node at: where mDNS sees it now (when its id and fingerprint
/// are the paired ones, so a new address after DHCP is followed), else where it was paired.
fn endpoint(row: &mason_store::NodeRow, seen: Option<&Discovered>) -> (String, u16) {
    match seen {
        Some(s) if s.fingerprint == row.fingerprint && !s.addrs.is_empty() => {
            (s.addrs[0].clone(), s.port)
        }
        _ => (row.host.clone(), row.port),
    }
}

/// Ask one paired node for its status, sending it the settings this coordinator has for it.
async fn poll_one(
    d: &Arc<Daemon>,
    cfg: &NodesCfg,
    row: &mason_store::NodeRow,
    identity: (Vec<u8>, Vec<u8>),
) -> View {
    let before = d.nodes.view(&row.nid).unwrap_or_default();
    let seen = d.nodes.lock().discovered.get(&row.nid).cloned();
    let (host, port) = endpoint(row, seen.as_ref());
    let mut v = View {
        polled_ms: now_ms(),
        ..before.clone()
    };
    let client = match NodeClient::new(identity, &host, port, &row.fingerprint) {
        Ok(c) => c.with_timeout(Duration::from_secs(10)),
        Err(e) => {
            v.failures += 1;
            v.last_error = Some(e.to_string());
            return v;
        }
    };
    let settings = cfg.settings_for(&row.name).to_json();
    match client
        .request("POST", "/node/status", Some(&settings))
        .await
    {
        Ok(r) if r.status == 200 => {
            let s = r.json();
            v.failures = 0;
            v.last_error = None;
            let proto = s["node_proto"].as_u64().unwrap_or(0);
            v.fault = (proto != u64::from(NODE_PROTO)).then(|| {
                Fault::Incompatible(format!(
                    "node protocol {proto} here, {NODE_PROTO} on this coordinator"
                ))
            });
            v.status = Some(s);
        }
        Ok(r) if r.status == 403 => {
            v.failures = 0;
            v.fault = Some(Fault::NotPaired);
            v.last_error = Some(r.error_text());
        }
        Ok(r) => {
            v.failures += 1;
            v.last_error = Some(r.error_text());
        }
        Err(ClientError::WrongFingerprint { .. }) => {
            v.failures = 0;
            v.fault = Some(Fault::WrongCertificate);
            v.last_error = Some("the node presented another certificate".into());
        }
        Err(ClientError::LocalNetworkDenied(a)) => {
            v.failures += 1;
            v.fault = Some(Fault::LocalNetworkDenied);
            v.last_error = Some(ClientError::LocalNetworkDenied(a).to_string());
        }
        Err(ClientError::LocalNetworkUnapplied(a)) => {
            v.failures += 1;
            v.fault = Some(Fault::LocalNetworkUnapplied);
            v.last_error = Some(ClientError::LocalNetworkUnapplied(a).to_string());
        }
        Err(e) => {
            v.failures += 1;
            v.fault = None;
            v.last_error = Some(e.to_string());
        }
    }
    v
}

/// Poll every paired node once, all at the same time. Public so a test can drive it.
pub async fn poll_once(d: &Arc<Daemon>) {
    let cfg = load_nodes(&d.cfg.config_dir).unwrap_or_default();
    let rows = paired(d);
    d.nodes.set_paired(rows.clone());
    let Ok(identity) = d.nodes.identity(d) else {
        return;
    };
    let mut set = tokio::task::JoinSet::new();
    for row in rows {
        let (d, cfg, id) = (d.clone(), cfg.clone(), identity.clone());
        set.spawn(async move {
            let v = poll_one(&d, &cfg, &row, id).await;
            (row.nid, v)
        });
    }
    while let Some(Ok((nid, v))) = set.join_next().await {
        set_capacity(d, &nid, &v, cfg.lost_after_polls);
        d.nodes.set_view(&nid, v);
    }
    push_runtimes(d, &cfg);
}

/// Does a node need the runtime `mine`? It is not running it, does not hold it (installed, and
/// waiting to take over) and is of an architecture there is a build for. Its protocol does not
/// matter: `PUT /node/runtime` is how a node on another protocol is brought up to date, so a
/// protocol bump must not strand it (s1122). Pure.
pub fn needs_runtime(status: &Value, mine: &str) -> bool {
    status["runtime_sha"].as_str() != Some(mine)
        && !status["runtimes"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(mine)))
        && crate::node_runtime::triple_for(status["arch"].as_str().unwrap_or("")).is_some()
}

/// After each poll, send every paired node that lacks the runtime this daemon runs the runtime
/// cross-built and signed for its architecture (`PUT /node/runtime`): the nodes an update could
/// not reach, and any that came back since. The node verifies it against the team it pinned at
/// pairing and takes over once its jobs have ended. One push at a time per node, and a failed one
/// is tried again after ten minutes (`mason node update` does not wait). Nothing happens without
/// a `sign_identity` in `update.toml`, nor while an update is under way (it sends its own).
fn push_runtimes(d: &Arc<Daemon>, cfg: &NodesCfg) {
    if !cfg.push_runtime || crate::node_fleet::update_under_way(d) {
        return;
    }
    let Some(mine) = d.cfg.runtime_sha.clone() else {
        return;
    };
    let Ok(ucfg) = crate::update::UpdateCfg::load(&d.cfg.config_dir) else {
        return;
    };
    if ucfg.sign_identity.is_none() {
        return;
    }
    for c in crate::node_fleet::candidates(d, &mine) {
        let _ = crate::node_fleet::start_push(d, &ucfg, &c, &mine, false);
    }
}

/// `POST /nodes/update {name?, all?}`: send the runtime this coordinator runs to a node, or to
/// every node, now, whatever the retry wait says (`mason node update`). It answers when the
/// sends have started; `GET /nodes` shows how each goes.
pub async fn update(
    axum::extract::State(d): axum::extract::State<Arc<Daemon>>,
    axum::Json(b): axum::Json<Value>,
) -> Res {
    let name = b["name"].as_str().map(String::from);
    let all = b["all"].as_bool().unwrap_or(false);
    if name.is_none() && !all {
        return Err(fail(400, "which node? `mason node update NAME` or `--all`"));
    }
    let Some(mine) = d.cfg.runtime_sha.clone() else {
        return Err(fail(
            409,
            "this daemon is not running from a runtime (`mason update` first): there is nothing to send",
        ));
    };
    if crate::node_fleet::update_under_way(&d) {
        return Err(fail(
            409,
            "an update is under way; it sends the nodes their runtime itself (`mason update status`)",
        ));
    }
    let ucfg = crate::update::UpdateCfg::load(&d.cfg.config_dir).map_err(|e| fail(409, e))?;
    if ucfg.sign_identity.is_none() {
        return Err(fail(
            409,
            "update.toml has no `sign_identity`: a node takes only a runtime signed by a team",
        ));
    }
    let (rows, views) = d.nodes.snapshot();
    let chosen: Vec<&mason_store::NodeRow> = rows
        .iter()
        .filter(|r| all || name.as_deref() == Some(r.name.as_str()))
        .collect();
    if chosen.is_empty() {
        return Err(match name {
            Some(n) => fail(404, format!("no paired node is called {n}")),
            None => fail(404, "no node is paired"),
        });
    }
    let out: Vec<Value> = chosen
        .iter()
        .map(|r| {
            let status = views.get(&r.nid).and_then(|v| v.status.clone());
            let result = |started: bool, state: String, note: String| {
                json!({"name": r.name, "started": started, "state": state, "note": note})
            };
            let Some(status) = status else {
                return result(
                    false,
                    crate::node_fleet::Fleet::WillUpdateWhenBack.text(),
                    "it has not answered yet; it is sent the runtime when it does".into(),
                );
            };
            if !needs_runtime(&status, &mine) {
                let f = crate::node_fleet::fleet_state(Some(&status), true, &mine, None);
                return result(false, f.text(), "it has the runtime already".into());
            }
            let c = crate::node_fleet::Candidate {
                nid: r.nid.clone(),
                name: r.name.clone(),
                arch: status["arch"].as_str().unwrap_or_default().to_string(),
            };
            match crate::node_fleet::start_push(&d, &ucfg, &c, &mine, true) {
                Ok(()) => result(
                    true,
                    crate::node_fleet::Fleet::Building(c.arch.clone()).text(),
                    "sending; `mason node list` shows how it goes".into(),
                ),
                Err(why) => result(false, "not started".into(), why),
            }
        })
        .collect();
    Ok(axum::Json(json!({"runtime": mine, "nodes": out})))
}

/// The broker resource a node's units are leased from.
pub fn resource(nid: &str) -> String {
    format!("node:{nid}")
}

/// The units this coordinator may hold on a node: what the node has free plus what this
/// coordinator holds there already, so the limit means the node's real capacity. A node that is
/// not taking work keeps what is held and grants one more at most, so a step that queued for it
/// is let through to find that out (and fall back) instead of waiting for a node that is gone.
fn set_capacity(d: &Arc<Daemon>, nid: &str, v: &View, lost_after: u32) {
    let name = resource(nid);
    let held = d.broker.used(&name);
    let free = v
        .status
        .as_ref()
        .map(|s| {
            let max = s["jobs"]["max"].as_u64().unwrap_or(0);
            let running = s["jobs"]["running"].as_u64().unwrap_or(0);
            max.saturating_sub(running) as u32
        })
        .unwrap_or(0);
    let units = if state_of(v, lost_after).usable() {
        free + held
    } else {
        held
    };
    d.broker.set_pool_units(&name, units.max(1));
}

/// Does a node with this status have what a run of `suite` needs? The toolchain names the suite
/// is known by (`remote::toolchains`) must be installed, and so must the Xcode version its
/// command names.
pub fn covers(status: &Value, suite: Option<&mason_testing::Suite>) -> bool {
    match suite {
        Some(s) => covers_run(status, &s.name, &s.command),
        None => true,
    }
}

/// [`covers`] for a run known by its suite's name and its command.
pub fn covers_run(status: &Value, suite_name: &str, command: &str) -> bool {
    let tc = &status["toolchains"];
    let has = |k: &str| tc[k].as_array().is_some_and(|a| !a.is_empty());
    for t in crate::remote::toolchains(suite_name) {
        let key = if t == "swift" { "xcode" } else { t };
        if !has(key) {
            return false;
        }
    }
    match crate::remote::xcode_requested(command) {
        Some(v) => tc["xcode"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(v.as_str()))),
        None => true,
    }
}

/// The node with the most room for a run of `project`, if any: where a run that waited too long
/// for GitHub goes instead.
pub fn best_room(
    d: &Daemon,
    project: &str,
    suite: Option<&mason_testing::Suite>,
) -> Option<crate::place::NodeRoom> {
    let (rooms, _) = rooms_for(d, project, suite);
    crate::place::best_node(&rooms).cloned()
}

/// What needs saying about the nodes, as `(key, kind, text)` the way the alert table takes it
/// (`attention::alerts_now`): a node that went away, one that cannot work with this coordinator.
/// An alert is true exactly as long as its condition.
pub fn alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let cfg = d.nodes.cfg(&d.cfg.config_dir);
    if !cfg.enabled {
        return vec![];
    }
    let mut out = {
        let g = d.nodes.lock();
        alerts_for(&g.paired, &g.views, cfg.lost_after_polls)
    };
    out.extend(crate::node_fleet::alerts(d));
    out
}

/// [`alerts`] from what the poller knows. Pure.
pub fn alerts_for(
    rows: &[mason_store::NodeRow],
    views: &BTreeMap<String, View>,
    lost_after: u32,
) -> Vec<(String, String, String)> {
    rows.iter()
        .filter_map(|r| {
            let v = views.get(&r.nid)?;
            let (what, text) = match state_of(v, lost_after) {
                NodeState::Lost(_) => ("lost", format!("node {} went away", r.name)),
                NodeState::Incompatible(why) => (
                    "incompatible",
                    format!(
                        "node {} is incompatible with this coordinator ({why})",
                        r.name
                    ),
                ),
                NodeState::LocalNetworkDenied => (
                    "local-network",
                    "macOS denies MASON the local network, so node ".to_string() + &r.name + " cannot be reached (System Settings → Privacy & Security → Local Network)",
                ),
                NodeState::LocalNetworkUnapplied => (
                    "local-network",
                    format!(
                        "node {} cannot be reached: {}",
                        r.name,
                        crate::node_client::LOCAL_NETWORK_UNAPPLIED
                    ),
                ),
                NodeState::WrongCertificate => (
                    "certificate",
                    format!(
                        "node {} presents another certificate than the one paired",
                        r.name
                    ),
                ),
                _ => return None,
            };
            Some((format!("node:{}:{what}", r.nid), "node".to_string(), text))
        })
        .collect()
}

/// The nodes that could take a run of `project` (of `suite`, when it is known) now, and whether
/// `[nodes] prefer = "github"`. Empty unless `[nodes] enabled` and the project may use nodes.
pub fn rooms_for(
    d: &Daemon,
    project: &str,
    suite: Option<&mason_testing::Suite>,
) -> (Vec<crate::place::NodeRoom>, bool) {
    let cfg = d.nodes.cfg(&d.cfg.config_dir);
    let prefer_github = cfg.prefer == crate::config::Prefer::Github;
    if !cfg.enabled || !cfg.allows(project) {
        return (vec![], prefer_github);
    }
    let (rows, views) = {
        let g = d.nodes.lock();
        (g.paired.clone(), g.views.clone())
    };
    let rooms = rows
        .iter()
        .filter_map(|r| {
            let v = views.get(&r.nid)?;
            let state = state_of(v, cfg.lost_after_polls);
            if !state.usable() {
                return None;
            }
            let s = v.status.as_ref()?;
            if !covers(s, suite) {
                return None;
            }
            let name = resource(&r.nid);
            let free = match d.broker.limit(&name) {
                Some(l) => l.saturating_sub(d.broker.used(&name)),
                None => 0,
            };
            (free > 0).then(|| crate::place::NodeRoom {
                nid: r.nid.clone(),
                name: r.name.clone(),
                free,
                warm: s["mirrors"][project].is_object(),
                die_c: s["thermal"]["die_c"].as_f64().map(|c| c as f32),
            })
        })
        .collect();
    (rooms, prefer_github)
}

/// Start the browse thread and the poller. The poller wakes every `[nodes] status_every_s`
/// (read afresh each time) and does nothing while no node is paired.
pub fn start(d: &Arc<Daemon>) {
    let weak = Arc::downgrade(d);
    let _ = std::thread::Builder::new()
        .name("nodes-poller".into())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            loop {
                let Some(d) = weak.upgrade() else {
                    return;
                };
                rt.block_on(poll_once(&d));
                let every = load_nodes(&d.cfg.config_dir)
                    .map_or(5, |c| c.status_every_s)
                    .max(1);
                drop(d);
                std::thread::sleep(Duration::from_secs(every));
            }
        });
    if std::env::var_os("MASON_NO_MDNS").is_some() {
        return;
    }
    let weak = Arc::downgrade(d);
    let _ = std::thread::Builder::new()
        .name("nodes-browse".into())
        .spawn(move || browse(weak));
}

fn browse(weak: std::sync::Weak<Daemon>) {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    let Ok(mdns) = ServiceDaemon::new() else {
        tracing::warn!("mDNS is not browsing for nodes");
        return;
    };
    let Ok(rx) = mdns.browse(crate::lan::SERVICE_TYPE) else {
        return;
    };
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(5));
        let Some(d) = weak.upgrade() else {
            return;
        };
        match ev {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let txt: BTreeMap<String, String> = info
                    .get_properties()
                    .iter()
                    .map(|p| (p.key().to_string(), p.val_str().to_string()))
                    .collect();
                let addrs: Vec<std::net::IpAddr> = info.get_addresses().iter().copied().collect();
                if let Some(n) = parse_txt(&txt, info.get_hostname(), info.get_fullname(), &addrs) {
                    d.nodes.see(n);
                }
            }
            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => d.nodes.forget_instance(&fullname),
            Ok(_) => {}
            Err(_) if rx.is_disconnected() => return,
            Err(_) => {}
        }
    }
}

// ---- the owner's side, on loopback ----------------------------------------------------------

type Res = Result<axum::Json<Value>, (axum::http::StatusCode, axum::Json<Value>)>;

fn fail(code: u16, msg: impl Into<String>) -> (axum::http::StatusCode, axum::Json<Value>) {
    (
        axum::http::StatusCode::from_u16(code).unwrap_or(axum::http::StatusCode::BAD_REQUEST),
        axum::Json(json!({"error": msg.into()})),
    )
}

/// One row of `mason node list`.
fn row_json(row: &mason_store::NodeRow, v: &View, cfg: &NodesCfg, goal: Option<&str>) -> Value {
    let state = state_of(v, cfg.lost_after_polls);
    let s = v.status.clone().unwrap_or(Value::Null);
    // Where its update stands against the commit the fleet is moving to (`node_fleet`).
    let update = goal.map(|g| {
        crate::node_fleet::fleet_state(
            v.status.as_ref(),
            v.failures == 0,
            g,
            crate::node_fleet::record_of(&row.nid).as_ref(),
        )
    });
    json!({
        "update": update.as_ref().map(|f| f.text()),
        "update_state": update.as_ref().map(|f| f.key()),
        "runtime_sha": s["runtime_sha"], "node_proto": s["node_proto"],
        "name": row.name, "nid": row.nid, "host": row.host, "port": row.port,
        "paired": true, "paired_at": row.paired_at,
        "state": state.text(), "usable": state.usable(),
        "accepting": s["accepting"], "why_not": s["why_not"],
        "arch": s["arch"], "os": s["os"], "cores": s["cores"], "version": s["version"],
        "cpu_pct": s["cpu_pct"], "other_cpu_pct": s["other_cpu_pct"],
        "thermal": s["thermal"], "slots": s["slots"], "jobs": s["jobs"],
        "disk_free_gb": s["disk_free_gb"], "toolchains": s["toolchains"],
        "settings": cfg.settings_for(&row.name).to_json(),
        "polled_ms": v.polled_ms, "last_error": v.last_error,
    })
}

/// `GET /nodes`: the paired nodes with their state, then the nodes seen but not paired.
pub async fn list(axum::extract::State(d): axum::extract::State<Arc<Daemon>>) -> Res {
    let cfg = load_nodes(&d.cfg.config_dir).unwrap_or_default();
    let rows = paired(&d);
    let goal = crate::node_fleet::goal(&d);
    let mut out: Vec<Value> = rows
        .iter()
        .map(|r| {
            row_json(
                r,
                &d.nodes.view(&r.nid).unwrap_or_default(),
                &cfg,
                goal.as_deref(),
            )
        })
        .collect();
    for n in d.nodes.discovered() {
        if rows.iter().any(|r| r.nid == n.nid) {
            continue;
        }
        let compat = n.node_proto == NODE_PROTO;
        out.push(json!({
            "name": n.name, "nid": n.nid, "host": n.addrs.first(), "port": n.port,
            "paired": false, "fingerprint": n.fingerprint, "arch": n.arch, "version": n.version,
            "state": if compat { "found over mDNS, not paired".to_string() }
                     else { format!("found over mDNS, incompatible (node protocol {} there, {NODE_PROTO} here)", n.node_proto) },
            "usable": false,
        }));
    }
    for h in &cfg.static_hosts {
        let (host, port) = h.rsplit_once(':').unwrap_or((h, "0"));
        if rows
            .iter()
            .any(|r| r.host == host && r.port.to_string() == port)
        {
            continue;
        }
        out.push(json!({
            "name": h, "host": host, "port": port.parse::<u16>().unwrap_or(0), "paired": false,
            "state": "listed in [nodes] static, not paired", "usable": false,
        }));
    }
    Ok(axum::Json(json!({
        "enabled": cfg.enabled, "prefer": match cfg.prefer {
            crate::config::Prefer::Nodes => "nodes", crate::config::Prefer::Github => "github" },
        "nodes": out,
    })))
}

/// The team this daemon's own executable is signed by, which a node pins at pairing: a runtime
/// this coordinator sends it must be signed by the same team. `None` when it is not signed by one
/// (a development build); such a coordinator can send no runtime.
fn own_team() -> Option<String> {
    static TEAM: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    TEAM.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|e| crate::node_doctor::team_of(&e))
    })
    .clone()
}

/// `POST /nodes/add {target?, host?, port?, fp?, code, name?}`: pair with a node. `target` names
/// a node mDNS has found (its host name or id); `host` and `fp` name one that is not found.
pub async fn add(
    axum::extract::State(d): axum::extract::State<Arc<Daemon>>,
    axum::Json(b): axum::Json<Value>,
) -> Res {
    let code = b["code"].as_str().unwrap_or_default().to_string();
    if code.is_empty() {
        return Err(fail(
            400,
            "the pairing code is needed (`mason node pair-code` on the node)",
        ));
    }
    let (host, port, fp) = if let Some(t) = b["target"].as_str() {
        let found: Vec<Discovered> = d
            .nodes
            .discovered()
            .into_iter()
            .filter(|n| n.name == t || n.nid == t)
            .collect();
        match found.as_slice() {
            [n] => match n.addrs.first() {
                Some(a) => (a.clone(), n.port, n.fingerprint.clone()),
                None => return Err(fail(409, format!("mDNS found {t} but gave no address for it"))),
            },
            [] => {
                return Err(fail(
                    404,
                    format!("no node called {t:?} was found over mDNS; use `--host H:PORT --fp FINGERPRINT`"),
                ))
            }
            _ => return Err(fail(409, format!("more than one node is called {t:?}; use its id"))),
        }
    } else {
        let h = b["host"].as_str().unwrap_or_default();
        let (host, port) = h
            .rsplit_once(':')
            .and_then(|(h, p)| Some((h.to_string(), p.parse::<u16>().ok()?)))
            .filter(|_| crate::config::valid_host_port(h))
            .ok_or_else(|| fail(400, "`host` must be host:port"))?;
        let fp = b["fp"].as_str().unwrap_or_default().to_ascii_lowercase();
        if fp.len() != 64 || !fp.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(fail(
                400,
                "`fp` must be the node's fingerprint (64 hex digits, printed by `mason node pair-code`)",
            ));
        }
        (host, port, fp)
    };
    let identity = d
        .nodes
        .identity(&d)
        .map_err(|e| fail(500, format!("no certificate: {e}")))?;
    let client =
        NodeClient::new(identity, &host, port, &fp).map_err(|e| fail(500, e.to_string()))?;
    let me = crate::node_serve::coordinator_name(&crate::lan::hostname());
    let r = client
        .request(
            "POST",
            "/node-pair",
            Some(&json!({"code": code, "name": me, "team_id": own_team()})),
        )
        .await
        .map_err(|e| fail(502, e.to_string()))?;
    if r.status != 200 {
        return Err(fail(
            if r.status == 400 || r.status == 410 || r.status == 429 || r.status == 409 {
                r.status
            } else {
                502
            },
            r.error_text(),
        ));
    }
    let reply = r.json();
    let nid = reply["nid"].as_str().unwrap_or_default().to_string();
    if nid.is_empty() {
        return Err(fail(502, "the node did not say who it is"));
    }
    let name = b["name"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .map(|n| n.trim().to_string())
        .or_else(|| reply["name"].as_str().map(String::from))
        .unwrap_or_else(|| nid.clone());
    let row = mason_store::NodeRow {
        nid: nid.clone(),
        name: name.clone(),
        host,
        port,
        fingerprint: fp,
        paired_at: now_ms(),
    };
    d.store
        .put_node(&row)
        .map_err(|e| fail(500, e.to_string()))?;
    d.nodes.forget_view(&nid);
    d.nodes.set_paired(paired(&d));
    // Ask it at once, so `mason node list` right after has an answer.
    let d2 = d.clone();
    poll_once(&d2).await;
    tracing::info!(node = %name, "paired with a node");
    Ok(axum::Json(
        json!({"paired": name, "nid": nid, "host": row.host, "port": row.port}),
    ))
}

/// `POST /nodes/remove {name}`.
pub async fn remove(
    axum::extract::State(d): axum::extract::State<Arc<Daemon>>,
    axum::Json(b): axum::Json<Value>,
) -> Res {
    let name = b["name"].as_str().unwrap_or_default().to_string();
    let nid = paired(&d)
        .into_iter()
        .find(|r| r.name == name)
        .map(|r| r.nid);
    match d.store.remove_node(&name) {
        Ok(true) => {
            if let Some(n) = nid {
                d.nodes.forget_view(&n);
            }
            d.nodes.set_paired(paired(&d));
            Ok(axum::Json(json!({"removed": name})))
        }
        Ok(false) => Err(fail(404, format!("no paired node called {name:?}"))),
        Err(e) => Err(fail(500, e.to_string())),
    }
}

/// `[nodes] enabled`, written into `daemon.toml` with its comments kept, and noted in
/// `nodes-changes.log`.
pub fn set_enabled(d: &Daemon, on: bool, who: &str) -> Result<bool, (u16, String)> {
    static EDITING: Mutex<()> = Mutex::new(());
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = d.cfg.config_dir.join("daemon.toml");
    let old = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err((500, format!("daemon.toml: {e}"))),
    };
    let mut doc: toml_edit::DocumentMut = old
        .parse()
        .map_err(|e| (400, format!("daemon.toml: {e}")))?;
    let was = crate::config::parse_nodes(&old)
        .map_err(|e| (400, e))?
        .enabled;
    if doc.get("nodes").is_none() {
        doc["nodes"] = toml_edit::table();
    }
    doc["nodes"]["enabled"] = toml_edit::value(on);
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).map_err(|e| (500, format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        (500, format!("{}: {e}", path.display()))
    })?;
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(d.cfg.config_dir.join("nodes-changes.log"))
    {
        let _ = writeln!(f, "{} {who} enabled {was} -> {on}", now_ms());
    }
    Ok(was)
}

/// `POST /nodes/use {on}`: `mason node use on|off`.
pub async fn use_nodes(
    axum::extract::State(d): axum::extract::State<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    axum::Json(b): axum::Json<Value>,
) -> Res {
    let on = b["on"]
        .as_bool()
        .ok_or_else(|| fail(400, "`on` must be true or false"))?;
    let who = headers
        .get("x-mason-as")
        .and_then(|v| v.to_str().ok())
        .filter(|w| !w.is_empty())
        .unwrap_or("owner")
        .to_string();
    let was = set_enabled(&d, on, &who).map_err(|(c, m)| fail(c, m))?;
    Ok(axum::Json(
        json!({"enabled": on, "was": was, "changed_by": who}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn good() -> BTreeMap<String, String> {
        txt(&[
            ("proto", "1"),
            ("fp", "ab"),
            ("version", "0.1.0"),
            ("node", "1"),
            ("nport", "7425"),
            ("node_proto", "1"),
            ("nid", "n1"),
            ("arch", "arm64"),
        ])
    }

    #[test]
    fn a_node_record_is_read_from_its_txt_keys() {
        let addrs: Vec<std::net::IpAddr> =
            vec!["fe80::1".parse().unwrap(), "10.0.0.7".parse().unwrap()];
        let n = parse_txt(
            &good(),
            "studio.local.",
            "MASON node on studio._mason._tcp.local.",
            &addrs,
        )
        .unwrap();
        assert_eq!(n.name, "studio");
        assert_eq!(
            (n.nid.as_str(), n.port, n.fingerprint.as_str()),
            ("n1", 7425, "ab")
        );
        assert_eq!(n.addrs, ["10.0.0.7", "fe80::1"], "IPv4 first");
        assert_eq!(
            (n.node_proto, n.arch.as_str(), n.version.as_str()),
            (1, "arm64", "0.1.0")
        );
    }

    #[test]
    fn a_service_that_is_not_a_node_or_lacks_a_key_is_not_listed() {
        let mut phone = good();
        phone.remove("node");
        assert!(parse_txt(&phone, "x.local.", "f", &[]).is_none());
        for key in ["nport", "fp", "nid"] {
            let mut t = good();
            t.remove(key);
            assert!(parse_txt(&t, "x.local.", "f", &[]).is_none(), "{key}");
        }
        let mut t = good();
        t.insert("nport".into(), "0".into());
        assert!(parse_txt(&t, "x.local.", "f", &[]).is_none());
        t.insert("nport".into(), "abc".into());
        assert!(parse_txt(&t, "x.local.", "f", &[]).is_none());
        // A missing protocol is 0, which no coordinator speaks.
        let mut t = good();
        t.remove("node_proto");
        assert_eq!(parse_txt(&t, "x.local.", "f", &[]).unwrap().node_proto, 0);
    }

    #[test]
    fn what_a_poll_left_becomes_a_state() {
        let ok = |accepting: bool, why: Option<&str>| View {
            status: Some(json!({"accepting": accepting, "why_not": why})),
            ..View::default()
        };
        assert_eq!(state_of(&View::default(), 6), NodeState::NotPolled);
        assert_eq!(state_of(&ok(true, None), 6), NodeState::Accepting);
        assert_eq!(
            state_of(&ok(false, Some("the owner turned remote work off")), 6),
            NodeState::NotAccepting("the owner turned remote work off".into())
        );
        let silent = |n| View {
            failures: n,
            ..ok(true, None)
        };
        assert_eq!(state_of(&silent(2), 6), NodeState::Silent(2));
        assert_eq!(state_of(&silent(5), 6), NodeState::Silent(5));
        assert_eq!(state_of(&silent(6), 6), NodeState::Lost(6));
        assert_eq!(state_of(&silent(9), 6), NodeState::Lost(9));
        let fault = |f| View {
            fault: Some(f),
            ..ok(true, None)
        };
        assert_eq!(state_of(&fault(Fault::NotPaired), 6), NodeState::NotPaired);
        assert_eq!(
            state_of(&fault(Fault::WrongCertificate), 6),
            NodeState::WrongCertificate
        );
        assert!(matches!(
            state_of(&fault(Fault::Incompatible("1 vs 2".into())), 6),
            NodeState::Incompatible(_)
        ));
        assert_eq!(
            state_of(&fault(Fault::LocalNetworkDenied), 6),
            NodeState::LocalNetworkDenied
        );
        assert!(NodeState::LocalNetworkDenied
            .text()
            .starts_with("local network access denied by macOS"));
        assert!(NodeState::LocalNetworkDenied
            .text()
            .contains("Local Network"));
        let unapplied = state_of(&fault(Fault::LocalNetworkUnapplied), 6);
        assert_eq!(unapplied, NodeState::LocalNetworkUnapplied);
        assert!(!unapplied.usable());
        assert_eq!(
            unapplied.text(),
            crate::node_client::LOCAL_NETWORK_UNAPPLIED
        );
        assert!(NodeState::Accepting.usable() && !NodeState::Lost(7).usable());
        assert!(NodeState::NotPaired.text().contains("403"));
    }

    #[test]
    fn a_node_needs_the_runtime_this_coordinator_runs_when_it_has_neither_running_nor_installed() {
        let sha = "a".repeat(40);
        let st = |proto: u64, running: Option<&str>, installed: &[&str], arch: &str| json!({"node_proto": proto, "runtime_sha": running, "runtimes": installed, "arch": arch});
        assert!(needs_runtime(&st(1, None, &[], "arm64"), &sha));
        assert!(needs_runtime(&st(1, Some("b"), &["c"], "x86_64"), &sha));
        assert!(
            !needs_runtime(&st(1, Some(&sha), &[], "arm64"), &sha),
            "running it"
        );
        assert!(
            !needs_runtime(&st(1, None, &[&sha], "arm64"), &sha),
            "installed, waiting to take over"
        );
        assert!(
            !needs_runtime(&st(1, None, &[], "riscv"), &sha),
            "no build for that"
        );
    }

    /// s1122: a protocol bump must not strand a node. Whatever protocol it reports, a node that
    /// lacks the runtime is sent it, and one that runs or holds it is not.
    #[test]
    fn a_node_on_an_older_protocol_that_lacks_the_runtime_is_sent_it() {
        let sha = "a".repeat(40);
        let st = |proto: u64, running: Option<&str>, installed: &[&str]| json!({"node_proto": proto, "runtime_sha": running, "runtimes": installed, "arch": "arm64"});
        let bumped = u64::from(NODE_PROTO) + 1;
        for proto in [0, u64::from(NODE_PROTO), bumped] {
            assert!(
                needs_runtime(&st(proto, Some("old"), &["old"]), &sha),
                "protocol {proto} lacking it"
            );
            assert!(
                !needs_runtime(&st(proto, Some(&sha), &[&sha]), &sha),
                "protocol {proto} running it"
            );
            assert!(
                !needs_runtime(&st(proto, Some("old"), &["old", &sha]), &sha),
                "protocol {proto} holding it"
            );
        }
    }

    #[test]
    fn a_node_that_went_away_or_cannot_be_worked_with_is_an_alert_only_while_it_is_so() {
        let row = |nid: &str, name: &str| mason_store::NodeRow {
            nid: nid.into(),
            name: name.into(),
            host: "h".into(),
            port: 1,
            fingerprint: "f".into(),
            paired_at: 0,
        };
        let ok = View {
            status: Some(json!({"accepting": true})),
            ..View::default()
        };
        let lost = View {
            failures: 6,
            ..ok.clone()
        };
        let old = View {
            fault: Some(Fault::Incompatible("protocol 1 vs 2".into())),
            ..ok.clone()
        };
        let rows = [row("n1", "studio"), row("n2", "mini"), row("n3", "old")];
        let views = BTreeMap::from([
            ("n1".to_string(), lost),
            ("n2".to_string(), ok),
            ("n3".to_string(), old),
        ]);
        let a = alerts_for(&rows, &views, 6);
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0].0, "node:n1:lost");
        assert_eq!(a[0].2, "node studio went away");
        assert!(a[1].2.contains("node old is incompatible") && a[1].2.contains("protocol 1 vs 2"));
        // Fewer misses than the limit is not news; nothing known is not either.
        let silent = View {
            failures: 5,
            ..View::default()
        };
        assert!(
            alerts_for(&rows[..1], &BTreeMap::from([("n1".to_string(), silent)]), 6).is_empty()
        );
        assert!(alerts_for(&rows, &BTreeMap::new(), 6).is_empty());
    }

    #[test]
    fn a_paired_node_is_followed_to_its_new_address_only_when_it_is_the_same_node() {
        let row = mason_store::NodeRow {
            nid: "n1".into(),
            name: "studio".into(),
            host: "10.0.0.7".into(),
            port: 7425,
            fingerprint: "ab".into(),
            paired_at: 0,
        };
        let seen = |fp: &str, addrs: &[&str]| Discovered {
            nid: "n1".into(),
            name: "studio".into(),
            addrs: addrs.iter().map(|a| a.to_string()).collect(),
            port: 9000,
            fingerprint: fp.into(),
            node_proto: 1,
            version: String::new(),
            arch: String::new(),
            fullname: String::new(),
        };
        assert_eq!(endpoint(&row, None), ("10.0.0.7".into(), 7425));
        assert_eq!(
            endpoint(&row, Some(&seen("ab", &["10.0.0.9"]))),
            ("10.0.0.9".into(), 9000)
        );
        // Another certificate at that id, or no address: stay where it was paired.
        assert_eq!(
            endpoint(&row, Some(&seen("zz", &["10.0.0.9"]))),
            ("10.0.0.7".into(), 7425)
        );
        assert_eq!(
            endpoint(&row, Some(&seen("ab", &[]))),
            ("10.0.0.7".into(), 7425)
        );
    }
}
