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
}

/// The state a view amounts to. Pure.
pub fn state_of(v: &View, lost_after: u32) -> NodeState {
    match &v.fault {
        Some(Fault::NotPaired) => return NodeState::NotPaired,
        Some(Fault::WrongCertificate) => return NodeState::WrongCertificate,
        Some(Fault::Incompatible(m)) => return NodeState::Incompatible(m.clone()),
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
        }
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
fn row_json(row: &mason_store::NodeRow, v: &View, cfg: &NodesCfg) -> Value {
    let state = state_of(v, cfg.lost_after_polls);
    let s = v.status.clone().unwrap_or(Value::Null);
    json!({
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
    let mut out: Vec<Value> = rows
        .iter()
        .map(|r| row_json(r, &d.nodes.view(&r.nid).unwrap_or_default(), &cfg))
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
    let me = crate::lan::hostname();
    let r = client
        .request(
            "POST",
            "/node-pair",
            Some(&json!({"code": code, "name": me})),
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
        assert!(NodeState::Accepting.usable() && !NodeState::Lost(7).usable());
        assert!(NodeState::NotPaired.text().contains("403"));
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
