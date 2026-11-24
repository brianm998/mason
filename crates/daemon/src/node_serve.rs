// SPDX-License-Identifier: GPL-3.0-or-later

//! The node side of running work on other Macs (`docs/multi-machine-nodes-plan.md`, §3.3-§3.5).
//!
//! A `masond` with `[node] serve = true` listens on a port of its own for **paired coordinators
//! only**. There are no tokens: each side's TLS certificate is its identity. The listener
//! *requires* a client certificate, takes any well-formed self-signed one through the handshake,
//! and then lets a request into `/node/*` only when the certificate's SHA-256 fingerprint is in
//! `<data>/node/coordinators.json`. Pairing (a code the owner reads off this machine and types on
//! the coordinator) is how a fingerprint gets there. A node keeps no secret: a fingerprint is
//! public, and the coordinator's private key never leaves the coordinator.
//!
//! A node has almost no configuration of its own. Its tunables arrive with each status poll and
//! live in memory; locally it has `[node] serve`, the owner's on/off switch (`node.toml`, read
//! live, off by default) and the list of coordinators pairing wrote.
//!
//! The listener is separate from the phone and monitor LAN listener, which is untouched.

use crate::config::{NodeSettings, OnSwitchOff};
use crate::daemon::{now_ms, Daemon};
use crate::lan::{self, sha256_hex};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use mason_core::access::{PairError, Pairing, Role};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_rustls::rustls::client::danger::HandshakeSignatureValid;
use tokio_rustls::rustls::crypto::{ring::default_provider, CryptoProvider};
use tokio_rustls::rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime,
};
use tokio_rustls::rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use tokio_rustls::rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

/// The port a node listens on when `[node]` names none.
pub const DEFAULT_NODE_PORT: u16 = 7425;

/// The node protocol. A coordinator and a node that disagree do not work together.
pub const NODE_PROTO: u32 = 1;

type Res = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn fail(code: u16, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
        Json(json!({"error": msg.into()})),
    )
}

/// A coordinator this node trusts: its certificate's fingerprint, and what the owner called it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coordinator {
    pub fingerprint: String,
    pub name: String,
    pub paired_at: u64,
}

/// What the running listener is.
#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub port: u16,
    pub fingerprint: String,
    pub nid: String,
    pub txt: BTreeMap<String, String>,
}

/// Everything a node keeps in memory.
pub struct NodeState {
    dir: PathBuf,
    pairing: Mutex<Pairing>,
    info: Mutex<Option<NodeInfo>>,
    coordinators: Mutex<Option<Vec<Coordinator>>>,
    /// The settings each coordinator last sent, by fingerprint. Memory only: a node restarted
    /// has the built-ins until the next poll.
    settings: Mutex<BTreeMap<String, NodeSettings>>,
}

impl NodeState {
    pub fn new(data_dir: &Path) -> Self {
        NodeState {
            dir: data_dir.join("node"),
            pairing: Mutex::new(Pairing::default()),
            info: Mutex::new(None),
            coordinators: Mutex::new(None),
            settings: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn info(&self) -> Option<NodeInfo> {
        self.info.lock().ok().and_then(|g| g.clone())
    }

    fn file(&self) -> PathBuf {
        self.dir.join("coordinators.json")
    }

    /// The paired coordinators, read from `coordinators.json` the first time.
    pub fn coordinators(&self) -> Vec<Coordinator> {
        let mut g = self.coordinators.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(|| read_coordinators(&self.file()))
            .clone()
    }

    pub fn is_paired(&self, fingerprint: &str) -> bool {
        !fingerprint.is_empty()
            && self
                .coordinators()
                .iter()
                .any(|c| c.fingerprint == fingerprint)
    }

    fn save(&self, list: &[Coordinator]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let v: Vec<Value> = list
            .iter()
            .map(
                |c| json!({"fingerprint": c.fingerprint, "name": c.name, "paired_at": c.paired_at}),
            )
            .collect();
        let tmp = self.dir.join("coordinators.json.tmp");
        lan::write_private(&tmp, Value::Array(v).to_string().as_bytes())?;
        std::fs::rename(&tmp, self.file())
    }

    /// Trust `fingerprint`, called `name`. The same fingerprint again renames it. A name already
    /// used by another fingerprint is refused.
    pub fn add_coordinator(&self, fingerprint: &str, name: &str, now: u64) -> Result<(), String> {
        let mut list = self.coordinators();
        if list
            .iter()
            .any(|c| c.name == name && c.fingerprint != fingerprint)
        {
            return Err(format!(
                "a coordinator called {name} is already paired; pair with another name"
            ));
        }
        list.retain(|c| c.fingerprint != fingerprint);
        list.push(Coordinator {
            fingerprint: fingerprint.to_string(),
            name: name.to_string(),
            paired_at: now,
        });
        self.save(&list).map_err(|e| e.to_string())?;
        *self.coordinators.lock().unwrap_or_else(|e| e.into_inner()) = Some(list);
        Ok(())
    }

    /// Stop trusting the coordinator called `name`. `false`: there is none.
    pub fn remove_coordinator(&self, name: &str) -> Result<bool, String> {
        let mut list = self.coordinators();
        let Some(gone) = list.iter().find(|c| c.name == name).cloned() else {
            return Ok(false);
        };
        list.retain(|c| c.fingerprint != gone.fingerprint);
        self.save(&list).map_err(|e| e.to_string())?;
        *self.coordinators.lock().unwrap_or_else(|e| e.into_inner()) = Some(list);
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&gone.fingerprint);
        Ok(true)
    }

    fn remember(&self, fingerprint: &str, s: NodeSettings) {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fingerprint.to_string(), s);
    }

    /// The settings in force: the strictest of what the paired coordinators last sent, or the
    /// built-ins when none has.
    pub fn effective_settings(&self) -> NodeSettings {
        let paired = self.coordinators();
        let g = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        let all: Vec<NodeSettings> = g
            .iter()
            .filter(|(fp, _)| paired.iter().any(|c| &c.fingerprint == *fp))
            .map(|(_, s)| *s)
            .collect();
        NodeSettings::strictest(&all)
    }
}

fn read_coordinators(path: &Path) -> Vec<Coordinator> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&text) else {
        return vec![];
    };
    a.iter()
        .filter_map(|c| {
            Some(Coordinator {
                fingerprint: c["fingerprint"].as_str()?.to_string(),
                name: c["name"].as_str()?.to_string(),
                paired_at: c["paired_at"].as_u64().unwrap_or(0),
            })
        })
        .collect()
}

/// This node's stable id: made once, kept in `<data>/node/id`, so that renaming the host does
/// not make it a new node.
pub fn node_id(data_dir: &Path) -> std::io::Result<String> {
    let dir = data_dir.join("node");
    let path = dir.join("id");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    std::fs::create_dir_all(&dir)?;
    let id: String = lan::random_bytes(8)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(&path, &id)?;
    Ok(id)
}

// ---- the owner's switch ---------------------------------------------------------------------

pub fn switch_path(d: &Daemon) -> PathBuf {
    d.cfg.config_dir.join("node.toml")
}

/// The owner's "accept work from other Macs" switch, as `node.toml` says now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Switch {
    pub on: bool,
    pub changed_by: Option<String>,
    pub changed_at: Option<u64>,
}

/// Read `node.toml`. A missing, unreadable or odd file is **off**: a node takes nothing until
/// the owner says so.
pub fn parse_switch(text: &str) -> Switch {
    let off = Switch {
        on: false,
        changed_by: None,
        changed_at: None,
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return off;
    };
    Switch {
        on: doc.get("accept").and_then(|v| v.as_bool()).unwrap_or(false),
        changed_by: doc
            .get("changed_by")
            .and_then(|v| v.as_str())
            .map(String::from),
        changed_at: doc
            .get("changed_at")
            .and_then(|v| v.as_integer())
            .and_then(|n| u64::try_from(n).ok()),
    }
}

pub fn read_switch(d: &Daemon) -> Switch {
    parse_switch(&std::fs::read_to_string(switch_path(d)).unwrap_or_default())
}

static EDITING: Mutex<()> = Mutex::new(());

/// Turn the switch on or off. The file's comments and other keys stay; who and when are in the
/// file and appended to `node-changes.log`.
pub fn set_switch(d: &Daemon, on: bool, who: &str) -> Result<Switch, (u16, String)> {
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = switch_path(d);
    let old = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err((500, format!("node.toml: {e}"))),
    };
    let mut doc: toml_edit::DocumentMut =
        old.parse().map_err(|e| (400, format!("node.toml: {e}")))?;
    let was = parse_switch(&old).on;
    let now = now_ms();
    doc["accept"] = toml_edit::value(on);
    doc["changed_by"] = toml_edit::value(who);
    doc["changed_at"] = toml_edit::value(i64::try_from(now).unwrap_or(i64::MAX));
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
        .open(d.cfg.config_dir.join("node-changes.log"))
    {
        let _ = writeln!(f, "{now} {who} accept {was} -> {on}");
    }
    Ok(Switch {
        on,
        changed_by: Some(who.to_string()),
        changed_at: Some(now),
    })
}

// ---- can this node take work? ---------------------------------------------------------------

/// What the availability decision looks at: plain readings, so that it is a pure function.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Readings {
    pub switch_on: bool,
    pub jobs_running: u32,
    /// Why the machine counts as hot ("die 96 °C"), if it does.
    pub hot: Option<String>,
    /// CPU used by anything but this node's own jobs. `None`: not known (never blocks).
    pub other_cpu_pct: Option<u32>,
    /// Free units of the `compile` slot, from the node's own broker. `None`: not known.
    pub compile_free: Option<u32>,
    pub disk_free_gb: Option<u64>,
}

/// Why this node cannot take a job now, in plain words, or `None` when it can. The first rule
/// that fails is named; the order is the plan's (§3.4): switch, heat, the owner's own work,
/// a free slot, disk. A reading that is not known never refuses.
pub fn why_not(r: &Readings, s: &NodeSettings) -> Option<String> {
    if !r.switch_on {
        return Some(match (r.jobs_running, s.on_switch_off) {
            (0, _) => "the owner turned remote work off".to_string(),
            (n, OnSwitchOff::Drain) => format!(
                "the owner turned remote work off (draining: {n} job{} left)",
                if n == 1 { "" } else { "s" }
            ),
            (n, OnSwitchOff::Cancel) => format!(
                "the owner turned remote work off (cancelling {n} job{})",
                if n == 1 { "" } else { "s" }
            ),
        });
    }
    if let Some(h) = &r.hot {
        return Some(format!("too hot: {h}"));
    }
    if let Some(c) = r.other_cpu_pct.filter(|c| *c > s.max_other_cpu_pct) {
        return Some(format!(
            "CPU {c} % from other work (limit {} %)",
            s.max_other_cpu_pct
        ));
    }
    if r.jobs_running >= s.max_jobs {
        return Some(format!("all {} job slots are in use", s.max_jobs));
    }
    if r.compile_free == Some(0) {
        return Some("no compile slot is free".into());
    }
    if let Some(g) = r.disk_free_gb.filter(|g| *g < s.min_disk_free_gb) {
        return Some(format!(
            "only {g} GB of disk free (needs {})",
            s.min_disk_free_gb
        ));
    }
    None
}

/// Other work's share of the CPU: the whole machine's use less what this node's own jobs use.
pub fn other_cpu(total_pct: Option<u32>, jobs_pct: u32) -> Option<u32> {
    total_pct.map(|t| t.saturating_sub(jobs_pct))
}

fn pressure_now() -> Option<u32> {
    static CACHE: Mutex<Option<(std::time::Instant, Option<u32>)>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *g {
        if at.elapsed() < Duration::from_secs(5) {
            return v;
        }
    }
    let v = crate::thermal::pressure_level();
    *g = Some((std::time::Instant::now(), v));
    v
}

/// Why the machine is hot, from the broker's thermal gate, the die limit a coordinator set, and
/// the macOS pressure level (the only source on Apple Silicon).
fn hot_now(
    d: &Daemon,
    s: &NodeSettings,
    die: Option<f32>,
    pressure: Option<u32>,
) -> Option<String> {
    if let Some(why) = d.broker.thermal_smc().2 {
        return Some(why);
    }
    if let Some(c) = die.filter(|c| s.max_die_c > 0 && *c >= s.max_die_c as f32) {
        return Some(format!("die {} °C", c.round() as u32));
    }
    pressure
        .filter(|p| *p >= crate::thermal::PRESSURE_HOT)
        .map(|p| format!("thermal pressure level {p}"))
}

fn probe(cmd: &str, args: &[&str]) -> Option<String> {
    let o = std::process::Command::new(cmd).args(args).output().ok()?;
    let mut t = String::from_utf8_lossy(&o.stdout).into_owned();
    t.push_str(&String::from_utf8_lossy(&o.stderr));
    Some(t)
}

/// `1.96.0-x86_64-apple-darwin (default)` lines of `rustup toolchain list`: the versions.
pub fn parse_rustup(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let name = l.split_whitespace().next()?;
            let ver = name.split('-').next()?;
            ver.chars()
                .next()
                .filter(|c| c.is_ascii_digit())
                .map(|_| ver.to_string())
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The major versions in `/usr/libexec/java_home -V`'s lines (`    21.0.5 (arm64) "..."`).
pub fn parse_java_home(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let w = l.split_whitespace().next()?;
            let major = w.split('.').next()?;
            (major.chars().all(|c| c.is_ascii_digit()) && !major.is_empty())
                .then(|| major.to_string())
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// `Xcode_16.4.app`, `Xcode-16.4.app` or `Xcode16.4.app`: `16.4`. A plain `Xcode.app` has no
/// version in its name and is not listed.
pub fn xcode_from_app_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix("Xcode")?.strip_suffix(".app")?;
    let v = rest.trim_start_matches(['_', '-', ' ']);
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.')).then(|| v.to_string())
}

/// The toolchains installed here, looked at at most every ten minutes.
pub fn toolchains() -> Value {
    static CACHE: Mutex<Option<(std::time::Instant, Value)>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = &*g {
        if at.elapsed() < Duration::from_secs(600) {
            return v.clone();
        }
    }
    let rust = probe("rustup", &["toolchain", "list"])
        .map(|t| parse_rustup(&t))
        .unwrap_or_default();
    let xcode: Vec<String> = std::fs::read_dir("/Applications")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| xcode_from_app_name(&e.file_name().to_string_lossy()))
        .collect();
    let java = probe("/usr/libexec/java_home", &["-V"])
        .map(|t| parse_java_home(&t))
        .unwrap_or_default();
    let v = json!({"rust": rust, "xcode": xcode, "java": java});
    *g = Some((std::time::Instant::now(), v.clone()));
    v
}

fn os_version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        probe("/usr/bin/sw_vers", &["-productVersion"])
            .map(|t| format!("macOS {}", t.trim()))
            .unwrap_or_else(|| std::env::consts::OS.to_string())
    })
    .clone()
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        a => a,
    }
}

/// What this node reports (§3.4): the same JSON for a coordinator's poll and `mason node status`.
pub fn status_value(d: &Daemon, s: &NodeSettings) -> Value {
    let switch = read_switch(d);
    let jobs_running = 0u32;
    let cpu = d.cpu_reading().usage.busy_permille;
    let cpu_pct = (cpu >= 0).then(|| (cpu as u32).div_ceil(10).min(100));
    let other = other_cpu(cpu_pct, 0);
    let (die, limited_ms, _) = d.broker.thermal_smc();
    let speed = d.broker.thermal_read().map(|(p, _)| p);
    // A daemon whose CPU is injected is a test's: it never reads the machine's real heat.
    let measures = d.cfg.cpu.is_none() && std::env::var_os("MASON_CPU_FILE").is_none();
    let pressure = if measures { pressure_now() } else { None };
    let hot = hot_now(d, s, die, pressure);
    let slot = |name: &str| {
        d.broker.limit(name).map(|limit| {
            let used = d.broker.used(name);
            (limit.saturating_sub(used), limit)
        })
    };
    let mut slots = serde_json::Map::new();
    for name in ["compile", "test-suite"] {
        if let Some((free, limit)) = slot(name) {
            slots.insert(name.into(), json!({"free": free, "limit": limit}));
        }
    }
    let disk_free_gb = match &d.cfg.disk {
        Some(src) => src.free_mb(&d.cfg.data_dir),
        None => crate::disk::DiskSource::free_mb(&crate::disk::Statvfs, &d.cfg.data_dir),
    }
    .map(|mb| mb / 1024);
    let readings = Readings {
        switch_on: switch.on,
        jobs_running,
        hot: hot.clone(),
        other_cpu_pct: other,
        compile_free: slot("compile").map(|(f, _)| f),
        disk_free_gb,
    };
    let why = why_not(&readings, s);
    let projects: serde_json::Map<String, Value> = d
        .projects()
        .values()
        .map(|p| (p.id.clone(), json!({"remote": p.remote})))
        .collect();
    json!({
        "node_proto": NODE_PROTO,
        "version": env!("CARGO_PKG_VERSION"),
        "nid": node_id(&d.cfg.data_dir).unwrap_or_default(),
        "name": lan::hostname(),
        "arch": arch(),
        "os": os_version(),
        "cores": std::thread::available_parallelism().map_or(0, |n| n.get()),
        "accepting": why.is_none(),
        "why_not": why,
        "switch": {"on": switch.on, "changed_by": switch.changed_by, "changed_at": switch.changed_at},
        "cpu_pct": cpu_pct,
        "other_cpu_pct": other,
        "thermal": {
            "limit_pct": speed, "die_c": die, "power_limited_ms": limited_ms,
            "pressure": pressure, "hot": hot.is_some(),
        },
        "slots": slots,
        "jobs": {"running": jobs_running, "max": s.max_jobs},
        "disk_free_gb": disk_free_gb,
        "toolchains": toolchains(),
        "projects": projects,
        "settings": s.to_json(),
    })
}

// ---- the listener ---------------------------------------------------------------------------

/// The fingerprint of the client certificate a connection presented, put in each request's
/// extensions by the connection loop. Empty if there was none.
#[derive(Clone, Debug)]
pub struct PeerFp(pub String);

/// Takes any client certificate through the handshake; the guard decides by fingerprint after.
/// Only the signature is checked, so the peer does hold the key of the certificate it sent.
#[derive(Debug)]
struct AnyClientCert {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClientCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, tokio_rustls::rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        tokio_rustls::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, tokio_rustls::rustls::Error> {
        tokio_rustls::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn server_config(
    cert: Vec<u8>,
    key: Vec<u8>,
) -> std::io::Result<Arc<tokio_rustls::rustls::ServerConfig>> {
    use tokio_rustls::rustls::ServerConfig;
    let provider = Arc::new(default_provider());
    let cfg = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(std::io::Error::other)?
        .with_client_cert_verifier(Arc::new(AnyClientCert { provider }))
        .with_single_cert(
            vec![CertificateDer::from(cert)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .map_err(std::io::Error::other)?;
    Ok(Arc::new(cfg))
}

/// The record advertised over mDNS for a node (the phone listener's keys, plus the node's).
pub fn txt_record(fingerprint: &str, nid: &str, port: u16, arch: &str) -> BTreeMap<String, String> {
    let mut t = lan::txt_record(fingerprint);
    t.insert("node".into(), "1".into());
    t.insert("nport".into(), port.to_string());
    t.insert("node_proto".into(), NODE_PROTO.to_string());
    t.insert("nid".into(), nid.to_string());
    t.insert("arch".into(), arch.to_string());
    t
}

/// Accept TLS connections and serve the node's routes on them, each request carrying the
/// fingerprint of the certificate its connection presented.
async fn serve(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    app: Router,
) {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder;
    use hyper_util::service::TowerToHyperService;
    loop {
        let tcp = match listener.accept().await {
            Ok((t, _)) => t,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let (acceptor, app) = (acceptor.clone(), app.clone());
        tokio::spawn(async move {
            let Ok(Ok(tls)) =
                tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await
            else {
                return;
            };
            let fp = tls
                .get_ref()
                .1
                .peer_certificates()
                .and_then(|c| c.first())
                .map(|c| sha256_hex(c.as_ref()))
                .unwrap_or_default();
            let app = app.layer(Extension(PeerFp(fp)));
            let _ = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(tls), TowerToHyperService::new(app))
                .await;
        });
    }
}

/// Bind the node listener(s) and serve them in the background. Returns the port it got.
pub async fn start(d: Arc<Daemon>) -> std::io::Result<u16> {
    let (cert, key) = lan::load_or_create_identity(&d.cfg.data_dir)?;
    let fingerprint = sha256_hex(&cert);
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config(cert, key)?);
    let nid = node_id(&d.cfg.data_dir)?;
    let mut listeners = Vec::new();
    let mut port = d.cfg.node.port;
    for addr in &d.cfg.node.bind {
        let l = tokio::net::TcpListener::bind((*addr, port)).await?;
        port = l.local_addr()?.port();
        listeners.push(l);
    }
    let txt = txt_record(&fingerprint, &nid, port, arch());
    let info = NodeInfo {
        port,
        fingerprint: fingerprint.clone(),
        nid,
        txt: txt.clone(),
    };
    if let Ok(mut g) = d.node.info.lock() {
        *g = Some(info);
    }
    // Tests turn the advertisement off: multicast is unreliable where they run.
    if std::env::var_os("MASON_NO_MDNS").is_none() {
        let adv = lan::Info {
            port,
            fingerprint,
            instance: format!("MASON node on {}", lan::hostname()),
            txt,
        };
        match lan::advertise(&adv) {
            // Kept for the life of the process.
            Ok(m) => std::mem::forget(m),
            Err(e) => tracing::warn!("mDNS is not advertising this node: {e}"),
        }
    }
    let app = router(d.clone());
    tracing::info!(port, bind = ?d.cfg.node.bind, "masond serving as a node (TLS, paired coordinators only)");
    for listener in listeners {
        tokio::spawn(serve(listener, acceptor.clone(), app.clone()));
    }
    Ok(port)
}

/// The node listener's routes. Pairing is open to any client certificate (a code is the
/// credential); everything under `/node/` is behind the fingerprint check.
pub fn router(d: Arc<Daemon>) -> Router {
    let guarded = Router::new()
        .route("/node/status", post(remote_status))
        .layer(axum::middleware::from_fn_with_state(d.clone(), guard))
        .with_state(d.clone());
    Router::new()
        .route("/node-pair", post(pair))
        .with_state(d)
        .merge(guarded)
}

async fn guard(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    req: Request,
    next: Next,
) -> Response {
    if !d.node.is_paired(&fp) {
        return fail(
            403,
            "this node does not know that coordinator; pair it with a code (`mason node add`)",
        )
        .into_response();
    }
    next.run(req).await
}

/// `POST /node-pair {code, name}`: a coordinator that has the code. Its certificate, which the
/// handshake gave us, is what gets trusted.
async fn pair(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    Json(b): Json<Value>,
) -> Res {
    if fp.is_empty() {
        return Err(fail(400, "no client certificate"));
    }
    let code = b["code"].as_str().unwrap_or_default().to_string();
    let name: String = b["name"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(60)
        .collect();
    if name.is_empty() {
        return Err(fail(400, "the coordinator needs a name"));
    }
    let now = now_ms();
    d.node
        .pairing
        .lock()
        .map_err(|_| fail(500, "internal error"))?
        .attempt(&code, now)
        .map_err(|e| {
            let status = match e {
                PairError::Malformed | PairError::Wrong => 400,
                PairError::Expired => 410,
                PairError::Locked => 429,
            };
            fail(status, e.message())
        })?;
    d.node
        .add_coordinator(&fp, &name, now)
        .map_err(|e| fail(409, e))?;
    tracing::info!(coordinator = %name, "coordinator paired");
    let info = d.node.info();
    Ok(Json(json!({
        "nid": info.as_ref().map(|i| i.nid.clone()),
        "name": lan::hostname(),
        "fingerprint": info.map(|i| i.fingerprint),
        "node_proto": NODE_PROTO,
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

/// `POST /node/status {settings}`: the coordinator's settings for this node, remembered in
/// memory, and the node's state under the strictest of every coordinator's.
async fn remote_status(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    body: Option<Json<Value>>,
) -> Res {
    let sent = match body {
        Some(Json(v)) if !v.is_null() => NodeSettings::from_json(&v).map_err(|e| fail(400, e))?,
        _ => NodeSettings::default(),
    };
    d.node.remember(&fp, sent);
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || status_value(&d2, &d2.node.effective_settings()))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    Ok(Json(v))
}

// ---- the owner's side, on loopback ----------------------------------------------------------

/// `GET /node/status`: the same JSON, from this machine, for `mason node status`.
pub async fn local_status(State(d): State<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let mut v = status_value(&d2, &d2.node.effective_settings());
            v["serving"] = json!(d2.node.info().is_some());
            v["coordinators"] = json!(d2
                .node
                .coordinators()
                .iter()
                .map(|c| json!({"name": c.name, "paired_at": c.paired_at}))
                .collect::<Vec<_>>());
            v
        })
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    Ok(Json(v))
}

/// `POST /node/pair-code`: a code for one coordinator, with the fingerprint it should expect.
pub async fn issue_code(State(d): State<Arc<Daemon>>) -> Res {
    let Some(info) = d.node.info() else {
        return Err(fail(
            409,
            "this daemon is not serving as a node; set `[node] serve = true` in daemon.toml and restart masond",
        ));
    };
    let code = lan::new_code().map_err(|_| fail(500, "no randomness available"))?;
    d.node
        .pairing
        .lock()
        .map_err(|_| fail(500, "internal error"))?
        .issue(code.clone(), Role::Owner, now_ms());
    Ok(Json(json!({
        "code": code,
        "expires_in_s": mason_core::access::CODE_TTL_MS / 1000,
        "fingerprint": info.fingerprint,
        "port": info.port,
        "host": format!("{}.local", lan::hostname()),
        "nid": info.nid,
    })))
}

/// `POST /node/accept {on}`: the owner's switch. Turning it off stops new jobs at once.
pub async fn accept(
    State(d): State<Arc<Daemon>>,
    headers: axum::http::HeaderMap,
    Json(b): Json<Value>,
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
    let sw = set_switch(&d, on, &who).map_err(|(c, m)| fail(c, m))?;
    Ok(Json(
        json!({"accept": sw.on, "changed_by": sw.changed_by, "changed_at": sw.changed_at}),
    ))
}

/// `GET /node/coordinators`.
pub async fn coordinators(State(d): State<Arc<Daemon>>) -> Res {
    Ok(Json(
        json!({"coordinators": d.node.coordinators().iter().map(|c| json!({
        "name": c.name, "fingerprint": c.fingerprint, "paired_at": c.paired_at,
    })).collect::<Vec<_>>()}),
    ))
}

/// `POST /node/coordinators/remove {name}`.
pub async fn coordinator_remove(State(d): State<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let name = b["name"].as_str().unwrap_or_default().to_string();
    match d.node.remove_coordinator(&name) {
        Ok(true) => Ok(Json(json!({"removed": name}))),
        Ok(false) => Err(fail(404, format!("no coordinator called {name:?}"))),
        Err(e) => Err(fail(500, e)),
    }
}

/// `POST /node/stop-jobs`: cancel every running job now. Nothing runs here until phase 2.
pub async fn stop_jobs(State(_d): State<Arc<Daemon>>) -> Res {
    Ok(Json(json!({"cancelled": 0})))
}

/// `GET /node/jobs`: the jobs running here. None until phase 2.
pub async fn jobs(State(_d): State<Arc<Daemon>>) -> Res {
    Ok(Json(json!({"jobs": []})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Readings {
        Readings {
            switch_on: true,
            jobs_running: 0,
            hot: None,
            other_cpu_pct: Some(10),
            compile_free: Some(3),
            disk_free_gb: Some(400),
        }
    }

    #[test]
    fn the_availability_table() {
        let s = NodeSettings::default(); // 2 jobs, 50 % other CPU, 50 GB disk, drain
        let rows: Vec<(&str, Readings, Option<&str>)> = vec![
            ("all clear", ok(), None),
            (
                "switch off",
                Readings {
                    switch_on: false,
                    ..ok()
                },
                Some("the owner turned remote work off"),
            ),
            (
                "switch off, one job draining",
                Readings {
                    switch_on: false,
                    jobs_running: 1,
                    ..ok()
                },
                Some("draining: 1 job left"),
            ),
            (
                "switch off, two jobs draining",
                Readings {
                    switch_on: false,
                    jobs_running: 2,
                    ..ok()
                },
                Some("draining: 2 jobs left"),
            ),
            (
                "hot",
                Readings {
                    hot: Some("die 96 °C".into()),
                    ..ok()
                },
                Some("too hot: die 96 °C"),
            ),
            (
                "other CPU high",
                Readings {
                    other_cpu_pct: Some(85),
                    ..ok()
                },
                Some("CPU 85 % from other work (limit 50 %)"),
            ),
            (
                "other CPU exactly at the limit",
                Readings {
                    other_cpu_pct: Some(50),
                    ..ok()
                },
                None,
            ),
            (
                "jobs at the limit",
                Readings {
                    jobs_running: 2,
                    ..ok()
                },
                Some("all 2 job slots are in use"),
            ),
            (
                "no compile slot",
                Readings {
                    compile_free: Some(0),
                    ..ok()
                },
                Some("no compile slot is free"),
            ),
            (
                "disk low",
                Readings {
                    disk_free_gb: Some(12),
                    ..ok()
                },
                Some("only 12 GB of disk free (needs 50)"),
            ),
            (
                "nothing known never refuses",
                Readings {
                    switch_on: true,
                    ..Readings::default()
                },
                None,
            ),
            (
                "the first failing rule is named: switch before heat",
                Readings {
                    switch_on: false,
                    hot: Some("x".into()),
                    ..ok()
                },
                Some("turned remote work off"),
            ),
            (
                "heat before CPU",
                Readings {
                    hot: Some("x".into()),
                    other_cpu_pct: Some(99),
                    ..ok()
                },
                Some("too hot"),
            ),
        ];
        for (name, r, want) in rows {
            let got = why_not(&r, &s);
            match want {
                None => assert_eq!(got, None, "{name}"),
                Some(w) => assert!(
                    got.as_deref().is_some_and(|g| g.contains(w)),
                    "{name}: {got:?}"
                ),
            }
        }
    }

    #[test]
    fn a_coordinators_settings_change_the_decision() {
        let s = NodeSettings {
            max_jobs: 1,
            max_other_cpu_pct: 20,
            min_disk_free_gb: 500,
            ..NodeSettings::default()
        };
        let r = Readings {
            other_cpu_pct: Some(30),
            ..ok()
        };
        assert!(why_not(&r, &s).unwrap().contains("limit 20 %"));
        let r = Readings {
            jobs_running: 1,
            ..ok()
        };
        assert!(why_not(&r, &s).unwrap().contains("all 1 job slots"));
        assert!(why_not(&ok(), &s).unwrap().contains("needs 500"));
    }

    #[test]
    fn other_cpu_is_the_machines_less_the_nodes_own_jobs() {
        assert_eq!(other_cpu(Some(70), 60), Some(10));
        assert_eq!(other_cpu(Some(10), 60), Some(0));
        assert_eq!(other_cpu(None, 60), None);
    }

    #[test]
    fn the_switch_file_defaults_to_off() {
        assert!(!parse_switch("").on);
        assert!(!parse_switch("accept = \"yes\"\n").on);
        assert!(!parse_switch("not toml [[[").on);
        let s = parse_switch("accept = true\nchanged_by = \"brian\"\nchanged_at = 5\n");
        assert_eq!(
            s,
            Switch {
                on: true,
                changed_by: Some("brian".into()),
                changed_at: Some(5)
            }
        );
    }

    #[test]
    fn the_txt_record_marks_a_node_and_names_its_port() {
        let t = txt_record("ab", "n1", 7425, "arm64");
        assert_eq!(t["node"], "1");
        assert_eq!(t["nport"], "7425");
        assert_eq!(t["node_proto"], NODE_PROTO.to_string());
        assert_eq!(t["nid"], "n1");
        assert_eq!(t["arch"], "arm64");
        // The phone listener's keys are still there.
        assert_eq!(t["fp"], "ab");
        assert!(t.contains_key("proto") && t.contains_key("version"));
    }

    #[test]
    fn toolchain_lists_are_read() {
        assert_eq!(
            parse_rustup("1.96.0-x86_64-apple-darwin (default)\nstable-aarch64-apple-darwin\n1.80.1-aarch64-apple-darwin\n"),
            ["1.80.1", "1.96.0"]
        );
        assert_eq!(
            parse_java_home("Matching Java Virtual Machines (2):\n    21.0.5 (arm64) \"Oracle\" - \"OpenJDK 21.0.5\" /x\n    17.0.2 (arm64) \"x\" - \"y\" /z\n"),
            ["17", "21"]
        );
        assert_eq!(
            xcode_from_app_name("Xcode_16.4.app").as_deref(),
            Some("16.4")
        );
        assert_eq!(
            xcode_from_app_name("Xcode-15.2.app").as_deref(),
            Some("15.2")
        );
        assert_eq!(xcode_from_app_name("Xcode.app"), None);
        assert_eq!(xcode_from_app_name("Xcode_beta.app"), None);
        assert_eq!(xcode_from_app_name("Safari.app"), None);
    }

    #[test]
    fn coordinators_are_kept_in_a_private_file_and_hold_no_secret() {
        let dir = tempfile::tempdir().unwrap();
        let n = NodeState::new(dir.path());
        assert!(n.coordinators().is_empty() && !n.is_paired("aa"));
        n.add_coordinator("aa", "imac", 5).unwrap();
        assert!(n.is_paired("aa") && !n.is_paired("bb") && !n.is_paired(""));
        // A name used by another fingerprint is refused; the same fingerprint may rename.
        assert!(n.add_coordinator("bb", "imac", 6).is_err());
        n.add_coordinator("aa", "desk", 7).unwrap();
        assert_eq!(n.coordinators().len(), 1);
        // A fresh state reads it back.
        let again = NodeState::new(dir.path());
        assert_eq!(again.coordinators()[0].name, "desk");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("node/coordinators.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let text = std::fs::read_to_string(dir.path().join("node/coordinators.json")).unwrap();
        assert!(text.contains("\"aa\"") && !text.contains("token") && !text.contains("max_jobs"));
        assert!(!again.remove_coordinator("nobody").unwrap());
        assert!(again.remove_coordinator("desk").unwrap());
        assert!(!again.is_paired("aa"));
        assert!(NodeState::new(dir.path()).coordinators().is_empty());
    }

    #[test]
    fn settings_are_kept_per_paired_coordinator_and_in_memory_only() {
        let dir = tempfile::tempdir().unwrap();
        let n = NodeState::new(dir.path());
        assert_eq!(n.effective_settings(), NodeSettings::default());
        n.add_coordinator("aa", "one", 1).unwrap();
        n.add_coordinator("bb", "two", 2).unwrap();
        n.remember(
            "aa",
            NodeSettings {
                max_jobs: 5,
                ..NodeSettings::default()
            },
        );
        n.remember(
            "bb",
            NodeSettings {
                max_jobs: 3,
                ..NodeSettings::default()
            },
        );
        // Settings from a fingerprint that is not paired do not count.
        n.remember(
            "zz",
            NodeSettings {
                max_jobs: 1,
                ..NodeSettings::default()
            },
        );
        assert_eq!(n.effective_settings().max_jobs, 3);
        n.remove_coordinator("two").unwrap();
        assert_eq!(n.effective_settings().max_jobs, 5);
        // Nothing of the settings is on disk.
        let text = std::fs::read_to_string(dir.path().join("node/coordinators.json")).unwrap();
        assert!(!text.contains("max_jobs"));
        assert_eq!(
            NodeState::new(dir.path()).effective_settings(),
            NodeSettings::default()
        );
    }

    #[test]
    fn the_node_id_is_made_once() {
        let dir = tempfile::tempdir().unwrap();
        let a = node_id(dir.path()).unwrap();
        assert_eq!(a.len(), 16);
        assert_eq!(a, node_id(dir.path()).unwrap());
    }
}
