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
use axum::extract::{Path as UrlPath, RawQuery, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
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

/// The one route a coordinator uses to bring a node up to date. It is not versioned by
/// [`NODE_PROTO`]: a node on any protocol must be able to take a runtime from a coordinator on
/// any other, or a protocol bump would strand it until someone installed by hand (s1122).
pub const RUNTIME_ROUTE: &str = "/node/runtime";

/// The keys of the node's status that [`RUNTIME_ROUTE`] relies on, kept stable across
/// [`NODE_PROTO`] versions: what the node is (`arch`), what it runs (`runtime_sha`) and holds
/// (`runtimes`), and its protocol (`node_proto`, which says why it is incompatible). A node may
/// add to its status (`pending_runtime`, for a display only) but never drop or rename these.
pub const RUNTIME_STATUS_KEYS: [&str; 4] = ["node_proto", "arch", "runtime_sha", "runtimes"];

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
    /// The team the coordinator's own binary is signed by, pinned at pairing: a runtime it sends
    /// is installed only if it is signed by this team (§3.14). `None`: the coordinator is not
    /// signed, so it can send none.
    pub team_id: Option<String>,
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
    /// The jobs this node runs for its coordinators.
    pub jobs: Arc<crate::node_run::Jobs>,
    /// What the last `mason node doctor` found (start-up, every ten minutes, on demand).
    pub doctor: Mutex<Option<crate::node_doctor::Report>>,
    /// What mDNS did when the listener started.
    pub mdns: Mutex<crate::node_doctor::Mdns>,
    /// A runtime installed that this daemon is about to become. Running jobs do not hold it up:
    /// their commands run under step hosts that the new daemon finds again, as after any restart.
    pub pending_runtime: Mutex<Option<String>>,
    /// When the last takeover failed to start the new runtime (0: none did). The node serves
    /// under the runtime it has meanwhile, and tries again a minute later.
    pub takeover_failed_ms: std::sync::atomic::AtomicU64,
}

impl NodeState {
    pub fn new(data_dir: &Path) -> Self {
        NodeState {
            dir: data_dir.join("node"),
            pairing: Mutex::new(Pairing::default()),
            info: Mutex::new(None),
            coordinators: Mutex::new(None),
            settings: Mutex::new(BTreeMap::new()),
            jobs: Arc::new(crate::node_run::Jobs::new(data_dir)),
            doctor: Mutex::new(None),
            mdns: Mutex::new(crate::node_doctor::Mdns::Off),
            pending_runtime: Mutex::new(None),
            takeover_failed_ms: std::sync::atomic::AtomicU64::new(0),
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

    /// The coordinator with this certificate, if it is paired.
    pub fn coordinator(&self, fingerprint: &str) -> Option<Coordinator> {
        self.coordinators()
            .into_iter()
            .find(|c| !fingerprint.is_empty() && c.fingerprint == fingerprint)
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
                |c| json!({"fingerprint": c.fingerprint, "name": c.name, "paired_at": c.paired_at, "team_id": c.team_id}),
            )
            .collect();
        let tmp = self.dir.join("coordinators.json.tmp");
        lan::write_private(&tmp, Value::Array(v).to_string().as_bytes())?;
        std::fs::rename(&tmp, self.file())
    }

    /// Trust `fingerprint`, called `name`. The same fingerprint again renames it. A name already
    /// used by another fingerprint is refused.
    pub fn add_coordinator(
        &self,
        fingerprint: &str,
        name: &str,
        now: u64,
        team_id: Option<String>,
    ) -> Result<(), String> {
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
            team_id,
        });
        self.save(&list).map_err(|e| e.to_string())?;
        *self.coordinators.lock().unwrap_or_else(|e| e.into_inner()) = Some(list);
        Ok(())
    }

    /// Stop trusting the coordinator called `name`. `false`: there is none.
    pub fn remove_coordinator(&self, name: &str) -> Result<bool, String> {
        let name = coordinator_key(name);
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
                team_id: c["team_id"].as_str().map(String::from),
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
    /// Why the node holds off, apart from its owner's wishes: `mason node doctor` found something
    /// it cannot work with, or a runtime is installed and the node is taking over (seconds; its jobs carry on).
    pub held: Option<String>,
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
    if let Some(h) = &r.held {
        return Some(h.clone());
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
/// version in its name; [`xcode_app_version`] reads it from the app's Info.plist instead.
pub fn xcode_from_app_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix("Xcode")?.strip_suffix(".app")?;
    let v = rest.trim_start_matches(['_', '-', ' ']);
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.')).then(|| v.to_string())
}

/// The version of the Xcode app at `app`: the one in its name when there is one, else the
/// `CFBundleShortVersionString` in its `Contents/Info.plist` (a plain `Xcode.app`). Nothing when
/// the name is not an Xcode app's, or the plist is missing or does not hold a version.
pub fn xcode_app_version(app: &std::path::Path) -> Option<String> {
    let name = app.file_name()?.to_string_lossy().into_owned();
    if let Some(v) = xcode_from_app_name(&name) {
        return Some(v);
    }
    if !(name.starts_with("Xcode") && name.ends_with(".app")) {
        return None;
    }
    let plist = app.join("Contents/Info.plist");
    if !plist.is_file() {
        return None;
    }
    let bytes = std::fs::read(&plist).ok()?;
    let v = if bytes.starts_with(b"bplist") {
        // A binary plist: only macOS's PlistBuddy reads it.
        probe(
            "/usr/libexec/PlistBuddy",
            &[
                "-c",
                "Print :CFBundleShortVersionString",
                &plist.to_string_lossy(),
            ],
        )?
    } else {
        plist_string(
            &String::from_utf8_lossy(&bytes),
            "CFBundleShortVersionString",
        )?
    };
    let v = v.trim();
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.')).then(|| v.to_string())
}

/// The `<string>` value that follows `<key>key</key>` in an XML property list.
fn plist_string(xml: &str, key: &str) -> Option<String> {
    let tag = format!("<key>{key}</key>");
    let rest = xml[xml.find(&tag)? + tag.len()..].trim_start();
    let rest = rest.strip_prefix("<string>")?;
    Some(rest[..rest.find("</string>")?].to_string())
}

/// The Xcode versions of the apps in `dir` (and of `selected`, the app `xcode-select -p` names,
/// wherever it is), sorted and without duplicates.
pub fn xcode_versions(dir: &std::path::Path, selected: Option<&std::path::Path>) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| xcode_app_version(&e.path()))
        .collect();
    if let Some(app) = selected {
        v.extend(xcode_app_version(app));
    }
    v.sort();
    v.dedup();
    v
}

/// The `.app` that `xcode-select -p` output (`/Applications/Xcode.app/Contents/Developer`) is in.
fn selected_xcode_app(printed: &str) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(printed.trim());
    path.ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "app"))
        .map(|a| a.to_path_buf())
}

/// The toolchains installed here, looked at at most every ten minutes.
pub fn toolchains() -> Value {
    // A test says what its node has, whatever the machine it runs on has.
    if let Some(v) = std::env::var("MASON_NODE_TOOLCHAINS")
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        return v;
    }
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
    let selected = probe("/usr/bin/xcode-select", &["-p"]).and_then(|t| selected_xcode_app(&t));
    let xcode = xcode_versions(std::path::Path::new("/Applications"), selected.as_deref());
    let java = probe("/usr/libexec/java_home", &["-V"])
        .map(|t| parse_java_home(&t))
        .unwrap_or_default();
    let mut v = json!({"rust": rust, "xcode": xcode, "java": java});
    // Only where it was asked: a node that does not say leaves `xcodebuild test` runs to the
    // coordinator's old rule (it has Xcode, so it may).
    if let Some(ok) = crate::node_doctor::testmanagerd_reachable() {
        v["xctest"] = if ok {
            json!(["testmanagerd"])
        } else {
            json!([])
        };
    }
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

/// What the node's machine says now, for the availability decision and the status.
struct Gathered {
    readings: Readings,
    cpu_pct: Option<u32>,
    die: Option<f32>,
    limited_ms: Option<u64>,
    speed: Option<u32>,
    pressure: Option<u32>,
    slots: serde_json::Map<String, Value>,
    disk_free_gb: Option<u64>,
    switch: Switch,
}

/// A daemon whose CPU is injected is a test's: it never reads the machine's real heat, disk or
/// signature.
pub fn measures_machine(d: &Daemon) -> bool {
    d.cfg.cpu.is_none() && std::env::var_os("MASON_CPU_FILE").is_none()
}

/// How long after a takeover that did not start the new runtime the node tries again.
const TAKEOVER_RETRY_MS: u64 = 60_000;

/// What the takeover to `sha` is doing, in words. Running jobs are not waited for; only those
/// still being set up are. Pure.
pub fn takeover_note(sha: &str, jobs_running: u32, settling: u32) -> String {
    let plural = |n: u32| if n == 1 { "" } else { "s" };
    let what = if settling > 0 {
        format!(
            "after {settling} job{} finish{} starting",
            plural(settling),
            if settling == 1 { "es" } else { "" }
        )
    } else if jobs_running > 0 {
        format!(
            "taking over now; {jobs_running} running job{} carr{} on",
            plural(jobs_running),
            if jobs_running == 1 { "ies" } else { "y" }
        )
    } else {
        "taking over now".to_string()
    };
    format!("updating to runtime {}: {what}", &sha[..sha.len().min(10)])
}

/// Why the node holds off apart from its owner's wishes: what `mason node doctor` found, or a
/// runtime that is about to take over. The hold lasts seconds (jobs keep running through the
/// takeover), and not at all after a takeover that failed: the node then serves as it was.
fn held_now(d: &Daemon, jobs_running: u32) -> Option<String> {
    let failed = d
        .node
        .takeover_failed_ms
        .load(std::sync::atomic::Ordering::SeqCst)
        != 0;
    if let Some(sha) = d
        .node
        .pending_runtime
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .filter(|_| !failed)
    {
        return Some(takeover_note(&sha, jobs_running, d.node.jobs.settling()));
    }
    d.node
        .doctor
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|r| r.problem())
}

fn gather(d: &Daemon, s: &NodeSettings, jobs_running: u32) -> Gathered {
    let switch = read_switch(d);
    let cpu = d.cpu_reading().usage.busy_permille;
    let cpu_pct = (cpu >= 0).then(|| (cpu as u32).div_ceil(10).min(100));
    let (die, limited_ms, _) = d.broker.thermal_smc();
    let speed = d.broker.thermal_read().map(|(p, _)| p);
    // A daemon whose CPU is injected is a test's: it never reads the machine's real heat.
    let pressure = if measures_machine(d) {
        pressure_now()
    } else {
        None
    };
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
    // What this node's own jobs use is not "other work": the whole machine's CPU less the CPU of
    // the jobs' process trees.
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as u32;
    let other = other_cpu(cpu_pct, d.node.jobs.cpu_pct(cores));
    Gathered {
        readings: Readings {
            switch_on: switch.on,
            jobs_running,
            hot,
            other_cpu_pct: other,
            compile_free: slot("compile").map(|(f, _)| f),
            disk_free_gb,
            held: held_now(d, jobs_running),
        },
        cpu_pct,
        die,
        limited_ms,
        speed,
        pressure,
        slots,
        disk_free_gb,
        switch,
    }
}

/// What this node reports (§3.4): the same JSON for a coordinator's poll and `mason node status`.
/// `coord` is the asking coordinator's directory name, for what the node keeps for it.
pub fn status_value(d: &Daemon, s: &NodeSettings, coord: Option<&str>) -> Value {
    let jobs_running = d.node.jobs.running();
    let g = gather(d, s, jobs_running);
    let why = why_not(&g.readings, s);
    let projects: serde_json::Map<String, Value> = d
        .projects()
        .values()
        .map(|p| (p.id.clone(), json!({"remote": p.remote})))
        .collect();
    let mirrors: serde_json::Map<String, Value> = coord
        .map(|c| d.node.jobs.warm(c).into_iter().collect())
        .unwrap_or_default();
    json!({
        "node_proto": d.cfg.node.proto,
        "version": env!("CARGO_PKG_VERSION"),
        "nid": node_id(&d.cfg.data_dir).unwrap_or_default(),
        "name": lan::hostname(),
        "arch": arch(),
        "os": os_version(),
        "cores": std::thread::available_parallelism().map_or(0, |n| n.get()),
        "accepting": why.is_none(),
        "why_not": why,
        "switch": {"on": g.switch.on, "changed_by": g.switch.changed_by, "changed_at": g.switch.changed_at},
        "cpu_pct": g.cpu_pct,
        "other_cpu_pct": g.readings.other_cpu_pct,
        "thermal": {
            "limit_pct": g.speed, "die_c": g.die, "power_limited_ms": g.limited_ms,
            "pressure": g.pressure, "hot": g.readings.hot.is_some(),
        },
        "slots": g.slots,
        "jobs": {"running": jobs_running, "max": s.max_jobs},
        "disk_free_gb": g.disk_free_gb,
        "toolchains": toolchains(),
        "projects": projects,
        "mirrors": mirrors,
        "settings": s.to_json(),
        "runtime_sha": d.cfg.runtime_sha,
        "runtimes": installed_runtimes(&d.cfg.data_dir),
        // A runtime that arrived and is taking over (a display; not part of the contract).
        "pending_runtime": d.node.pending_runtime.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        "doctor": d.node.doctor.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|r| r.to_json()),
    })
}

/// The runtimes installed here (complete ones), by commit.
fn installed_runtimes(data_dir: &Path) -> Vec<String> {
    let rt = crate::update::Runtimes::new(data_dir);
    std::fs::read_dir(&rt.root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| crate::node_run::valid_sha(n) && rt.is_ready(n))
        .collect()
}

/// Why a job cannot start now: the availability rules with the jobs that run, or `None`.
pub fn no_room_for_job(d: &Daemon, s: &NodeSettings, running: u32) -> Option<String> {
    why_not(&gather(d, s, running).readings, s)
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
            Ok(m) => {
                m.keep();
                *d.node.mdns.lock().unwrap_or_else(|e| e.into_inner()) =
                    crate::node_doctor::Mdns::Advertising;
            }
            Err(e) => {
                tracing::warn!("mDNS is not advertising this node: {e}");
                *d.node.mdns.lock().unwrap_or_else(|e| e.into_inner()) =
                    crate::node_doctor::Mdns::Failed(e);
            }
        }
    }
    d.node.jobs.resume(&d);
    spawn_upkeep(&d);
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
        .route("/node/mirror/{project}/has", get(mirror_has))
        .route("/node/mirror/{project}", put(mirror_put))
        .route("/node/mirror/{project}/base", post(mirror_base))
        .route("/node/mirrors", get(mirror_list))
        .route("/node/jobs", post(job_start))
        .route("/node/jobs/{id}", get(job_get).delete(job_cancel))
        .route("/node/jobs/{id}/log", get(job_log))
        .route("/node/jobs/{id}/artifact", get(job_artifact))
        .route("/node/jobs/{id}/ack", post(job_ack))
        .route(RUNTIME_ROUTE, put(runtime_put))
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

/// The longest coordinator name a node keeps, in characters.
const COORDINATOR_NAME_MAX: usize = 60;

/// The name a node files a coordinator under: trimmed, and when longer than
/// `COORDINATOR_NAME_MAX` characters cut at a character boundary with a short hash of the whole
/// name at the end, so two long names that share a prefix stay distinct. Pairing and every lookup
/// by name go through this, so the name a coordinator sends always finds the entry it made.
pub fn coordinator_key(name: &str) -> String {
    let name = name.trim();
    if name.chars().count() <= COORDINATOR_NAME_MAX {
        return name.to_string();
    }
    // FNV-1a: stable across runs and platforms, which `DefaultHasher` does not promise.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let head: String = name.chars().take(COORDINATOR_NAME_MAX - 9).collect();
    format!("{head}-{:08x}", h & 0xffff_ffff)
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
    let name = coordinator_key(b["name"].as_str().unwrap_or_default());
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
    // The team the coordinator is signed by, which a runtime it sends must be signed by too.
    let team = b["team_id"]
        .as_str()
        .filter(|t| !t.is_empty() && t.len() <= 20 && t.bytes().all(|c| c.is_ascii_alphanumeric()))
        .map(String::from);
    d.node
        .add_coordinator(&fp, &name, now, team)
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
        .run(move || {
            status_value(
                &d2,
                &d2.node.effective_settings(),
                Some(&crate::node_run::fp16(&fp)),
            )
        })
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    Ok(Json(v))
}

// ---- jobs: mirrors and runs, for a paired coordinator ------------------------------------------

fn coord_of(fp: &str) -> String {
    crate::node_run::fp16(fp)
}

fn project_ok(p: &str) -> Result<(), (StatusCode, Json<Value>)> {
    if crate::node_run::safe_name(p) {
        Ok(())
    } else {
        Err(fail(
            400,
            "`project` must be letters, digits, `.`, `_` and `-`",
        ))
    }
}

/// `GET /node/mirror/{project}/has?c=<sha>&c=<sha>`: which of those commits the mirror has.
async fn mirror_has(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    UrlPath(project): UrlPath<String>,
    RawQuery(q): RawQuery,
) -> Res {
    project_ok(&project)?;
    let shas: Vec<String> = q
        .unwrap_or_default()
        .split('&')
        .filter_map(|kv| kv.strip_prefix("c="))
        .map(String::from)
        .take(64)
        .collect();
    let d2 = d.clone();
    let have = d
        .pool
        .run(move || d2.node.jobs.has(&coord_of(&fp), &project, &shas))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    Ok(Json(json!({"has": have})))
}

/// `PUT /node/mirror/{project}`: a git bundle, added to the project's mirror. Refused, with what
/// is missing, when the mirror lacks a commit the bundle builds on.
async fn mirror_put(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    UrlPath(project): UrlPath<String>,
    body: axum::body::Body,
) -> Res {
    use http_body_util::BodyExt;
    use std::io::Write;
    project_ok(&project)?;
    let dir = d.cfg.data_dir.join("node").join("incoming");
    std::fs::create_dir_all(&dir).map_err(|e| fail(500, e.to_string()))?;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let file = dir.join(format!(
        "{}-{}-{}.bundle",
        coord_of(&fp),
        now_ms(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let mut out = std::fs::File::create(&file).map_err(|e| fail(500, e.to_string()))?;
    let mut body = body;
    let mut size = 0u64;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| {
            let _ = std::fs::remove_file(&file);
            fail(400, e.to_string())
        })?;
        if let Some(data) = frame.data_ref() {
            size += data.len() as u64;
            if size > 8 * 1024 * 1024 * 1024 {
                let _ = std::fs::remove_file(&file);
                return Err(fail(413, "the bundle is too large"));
            }
            out.write_all(data).map_err(|e| {
                let _ = std::fs::remove_file(&file);
                fail(500, e.to_string())
            })?;
        }
    }
    drop(out);
    let (d2, f2) = (d.clone(), file.clone());
    let r = d
        .pool
        .run(move || d2.node.jobs.add_bundle(&coord_of(&fp), &project, &f2))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    let _ = std::fs::remove_file(&file);
    match r {
        Ok(()) => Ok(Json(json!({"added": size}))),
        Err(e) => Err((
            StatusCode::from_u16(412).unwrap_or(StatusCode::BAD_REQUEST),
            Json(json!({"error": e, "missing": true})),
        )),
    }
}

/// `POST /node/mirror/{project}/base {name, sha}`: set `origin/<name>` of the project's mirror
/// (`mason node sync`, after the bundle that carries `sha`). The answer says where it is.
async fn mirror_base(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    UrlPath(project): UrlPath<String>,
    Json(b): Json<Value>,
) -> Res {
    project_ok(&project)?;
    let name = b["name"].as_str().unwrap_or_default().to_string();
    let sha = b["sha"].as_str().unwrap_or_default().to_string();
    if name.is_empty() || !crate::node_run::valid_sha(&sha) {
        return Err(fail(400, "`name` and a full commit id `sha` are needed"));
    }
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || {
            let coord = coord_of(&fp);
            let jobs = &d2.node.jobs;
            let at = jobs.sync_base(&coord, &project, &name, &sha)?;
            Ok::<_, String>(json!({
                "sha": at, "path": jobs.mirror_path(&coord, &project).to_string_lossy(),
            }))
        })
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    r.map(Json).map_err(|e| fail(409, e))
}

/// `GET /node/mirrors`: the mirrors this node keeps for the asking coordinator.
async fn mirror_list(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || d2.node.jobs.mirrors(&coord_of(&fp)))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    Ok(Json(json!({"mirrors": v})))
}

/// `POST /node/jobs {id, project, sha, run, limit_s, lease_s}`: start a job, if there is room
/// for it now. `409 {why}` if not; `412` if the commit is not in the mirror.
async fn job_start(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    Json(b): Json<Value>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    use crate::node_run::StartError;
    let req = crate::node_run::StartReq::parse(&b).map_err(|e| fail(400, e))?;
    let coord = coord_of(&fp);
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || {
            let s = d2.node.effective_settings();
            let jobs = d2.node.jobs.clone();
            let d3 = d2.clone();
            jobs.start(&d2, &coord, req, &s, &|running| {
                no_room_for_job(&d3, &s, running)
            })
            .map(|h| h.job.lock().map(|j| j.view()).unwrap_or(Value::Null))
        })
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    match r {
        Ok(v) => Ok((StatusCode::CREATED, Json(json!({"job": v})))),
        Err(StartError::Busy(why)) => Err(fail(409, why)),
        Err(StartError::Missing(sha)) => Err((
            StatusCode::from_u16(412).unwrap_or(StatusCode::BAD_REQUEST),
            Json(json!({"error": "the commit is not in the mirror", "missing": sha})),
        )),
        Err(StartError::Bad(e)) => Err(fail(400, e)),
        Err(StartError::Io(e)) => Err(fail(500, e)),
    }
}

/// `GET /node/jobs/{id}`: the job's state. Asking renews its lease.
async fn job_get(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    UrlPath(id): UrlPath<String>,
) -> Res {
    let h = d
        .node
        .jobs
        .handle(&coord_of(&fp), &id)
        .ok_or_else(|| fail(404, "no such job"))?;
    d.node.jobs.renew(&h);
    let v = h.job.lock().map(|j| j.view()).unwrap_or(Value::Null);
    Ok(Json(json!({"job": v})))
}

/// `GET /node/jobs/{id}/log?from=N`: the output from byte `N`, raw.
async fn job_log(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    UrlPath(id): UrlPath<String>,
    RawQuery(q): RawQuery,
) -> Response {
    let from = q
        .unwrap_or_default()
        .split('&')
        .find_map(|kv| kv.strip_prefix("from=").and_then(|v| v.parse::<u64>().ok()))
        .unwrap_or(0);
    let coord = coord_of(&fp);
    if d.node.jobs.handle(&coord, &id).is_none() {
        return fail(404, "no such job").into_response();
    }
    let d2 = d.clone();
    let bytes = d
        .pool
        .run(move || d2.node.jobs.log(&coord, &id, from))
        .await
        .unwrap_or_default();
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response()
}

/// `GET /node/jobs/{id}/artifact`: the result and the results files of a job that has ended.
async fn job_artifact(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    UrlPath(id): UrlPath<String>,
) -> Res {
    let (d2, coord) = (d.clone(), coord_of(&fp));
    let r = d
        .pool
        .run(move || d2.node.jobs.artifact(&coord, &id))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    r.map(Json).map_err(|e| fail(409, e))
}

/// `DELETE /node/jobs/{id}`: cancel.
async fn job_cancel(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    UrlPath(id): UrlPath<String>,
) -> Res {
    if d.node
        .jobs
        .cancel(&coord_of(&fp), &id, "cancelled by its coordinator")
    {
        Ok(Json(json!({"cancelled": id})))
    } else {
        Err(fail(404, "no such job"))
    }
}

/// `POST /node/jobs/{id}/ack`: the result was collected; the node may delete the job.
async fn job_ack(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    UrlPath(id): UrlPath<String>,
) -> Res {
    if d.node.jobs.ack(&coord_of(&fp), &id) {
        Ok(Json(json!({"acked": id})))
    } else {
        Err(fail(404, "no such finished job"))
    }
}

/// Run the doctor now and keep what it found. A daemon that is a test's has no machine to judge
/// and keeps nothing.
pub fn run_doctor(d: &Arc<Daemon>) -> crate::node_doctor::Report {
    let mdns = d
        .node
        .mdns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let exe = std::env::current_exe().unwrap_or_default();
    let allow = std::env::var_os("MASON_NODE_ALLOW_UNSIGNED").is_some();
    let inputs =
        crate::node_doctor::gather(&exe, &d.cfg.data_dir, d.node.info().is_some(), mdns, allow);
    let r = crate::node_doctor::evaluate(&inputs, now_ms());
    if measures_machine(d) {
        *d.node.doctor.lock().unwrap_or_else(|e| e.into_inner()) = Some(r.clone());
    }
    r
}

/// `PUT /node/runtime`: a runtime the coordinator built and signed for this node. It is installed
/// only if every executable of it verifies and is signed by the team the coordinator was signed
/// by when it paired; this daemon then takes over once its running jobs have ended.
async fn runtime_put(
    State(d): State<Arc<Daemon>>,
    Extension(PeerFp(fp)): Extension<PeerFp>,
    body: axum::body::Body,
) -> Res {
    use http_body_util::BodyExt;
    let Some(team) = d.node.coordinator(&fp).and_then(|c| c.team_id) else {
        return Err(fail(
            412,
            "this coordinator was not signed by a team when it paired, so a runtime from it cannot be checked; install the runtime by hand (`mason node export-runtime`) or pair again with a signed coordinator",
        ));
    };
    let mut bytes: Vec<u8> = Vec::new();
    let mut body = body;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| fail(400, e.to_string()))?;
        if let Some(data) = frame.data_ref() {
            if (bytes.len() + data.len()) as u64 > crate::node_runtime::MAX_BYTES {
                return Err(fail(413, "the runtime is too large"));
            }
            bytes.extend_from_slice(data);
        }
    }
    let d2 = d.clone();
    let installed = d
        .pool
        .run(move || -> Result<String, String> {
            let (sha, files) = crate::node_runtime::unpack(&bytes)?;
            crate::node_runtime::install(
                &d2.cfg.data_dir,
                &sha,
                &files,
                &team,
                &crate::node_runtime::real_codesign,
            )?;
            Ok(sha)
        })
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?
        .map_err(|e| fail(422, e))?;
    // Takes over at once, its jobs carrying on under their step hosts, when this daemon runs from a runtime (a development build has
    // none to replace).
    let takes_over = d.cfg.runtime_sha.as_deref().is_some_and(|r| r != installed);
    if takes_over {
        *d.node
            .pending_runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(installed.clone());
    }
    tracing::info!(runtime = %installed, takes_over, "installed a runtime from its coordinator");
    Ok(Json(
        json!({"installed": installed, "takes_over": takes_over}),
    ))
}

/// `POST /node/doctor`: check everything now (`mason node doctor`).
pub async fn doctor(State(d): State<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let r = d
        .pool
        .run(move || run_doctor(&d2))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?;
    Ok(Json(r.to_json()))
}

/// Keep the node's jobs in order: leases, old results, and the owner's switch when the
/// coordinator chose `cancel`.
fn spawn_upkeep(d: &Arc<Daemon>) {
    let weak = Arc::downgrade(d);
    let _ = std::thread::Builder::new()
        .name("node-upkeep".into())
        .spawn(move || {
            let mut next_doctor = 0u64;
            loop {
            std::thread::sleep(Duration::from_secs(2));
            let Some(d) = weak.upgrade() else {
                return;
            };
            // The start-up probe, then every ten minutes: a node that was healthy and loses a
            // grant says so before a coordinator finds out with a job.
            if measures_machine(&d) && now_ms() >= next_doctor {
                next_doctor = now_ms() + 600_000;
                let r = run_doctor(&d);
                // A signature `codesign` could not read in time (a busy `syspolicyd`) is not
                // kept: ask again in half a minute rather than hold the node off for ten.
                if r.checks.iter().any(|c| {
                    c.name == "signature" && c.detail.starts_with("could not read masond's")
                }) {
                    next_doctor = now_ms() + 30_000;
                }
            }
            // A runtime that arrived: this daemon becomes it at once. The running jobs do not
            // wait and are not touched: their commands run under step hosts, and the new daemon
            // takes the jobs up again (`Jobs::resume`). Only a job still being set up is waited
            // for. A takeover that did not start is tried again after a minute.
            let pending = d
                .node
                .pending_runtime
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(sha) = pending {
                let failed = d
                    .node
                    .takeover_failed_ms
                    .load(std::sync::atomic::Ordering::SeqCst);
                let due = failed == 0 || now_ms().saturating_sub(failed) >= TAKEOVER_RETRY_MS;
                if due && d.node.jobs.settling() == 0 {
                    tracing::info!(runtime = %sha, jobs = d.node.jobs.running(), "taking over the runtime a coordinator sent");
                    // Returns only if it could not.
                    let _ = crate::update::take_over(&d, &sha);
                    d.node
                        .takeover_failed_ms
                        .store(now_ms(), std::sync::atomic::Ordering::SeqCst);
                }
            }
            d.node.jobs.tick(now_ms());
            if !read_switch(&d).on
                && d.node.effective_settings().on_switch_off == OnSwitchOff::Cancel
            {
                d.node.jobs.cancel_all("the owner turned remote work off");
            }
            }
        });
}

// ---- the owner's side, on loopback ----------------------------------------------------------

/// `GET /node/status`: the same JSON, from this machine, for `mason node status`.
pub async fn local_status(State(d): State<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let v = d
        .pool
        .run(move || {
            let mut v = status_value(&d2, &d2.node.effective_settings(), None);
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
    // By default running jobs finish (`drain`); a coordinator that set `cancel` has them stopped.
    let cancelled = if !on && d.node.effective_settings().on_switch_off == OnSwitchOff::Cancel {
        d.node.jobs.cancel_all("the owner turned remote work off")
    } else {
        0
    };
    Ok(Json(
        json!({"accept": sw.on, "changed_by": sw.changed_by, "changed_at": sw.changed_at, "cancelled": cancelled}),
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
    let name = coordinator_key(b["name"].as_str().unwrap_or_default());
    match d.node.remove_coordinator(&name) {
        Ok(true) => Ok(Json(json!({"removed": name}))),
        Ok(false) => Err(fail(404, format!("no coordinator called {name:?}"))),
        Err(e) => Err(fail(500, e)),
    }
}

/// `POST /node/stop-jobs`: cancel every running job now.
pub async fn stop_jobs(State(d): State<Arc<Daemon>>) -> Res {
    let n = d
        .node
        .jobs
        .cancel_all("the owner stopped the jobs on this machine");
    Ok(Json(json!({"cancelled": n})))
}

/// `GET /node/jobs`: the jobs this node knows.
pub async fn jobs(State(d): State<Arc<Daemon>>) -> Res {
    Ok(Json(json!({"jobs": d.node.jobs.list()})))
}

#[cfg(test)]
mod tests {
    /// Running jobs are never a reason to wait; only a job still being set up is, and the note
    /// says which it is.
    #[test]
    fn the_takeover_note_does_not_wait_for_running_jobs() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            takeover_note(sha, 0, 0),
            "updating to runtime 0123456789: taking over now"
        );
        assert_eq!(
            takeover_note(sha, 9, 0),
            "updating to runtime 0123456789: taking over now; 9 running jobs carry on"
        );
        assert_eq!(
            takeover_note(sha, 1, 0),
            "updating to runtime 0123456789: taking over now; 1 running job carries on"
        );
        assert_eq!(
            takeover_note(sha, 3, 1),
            "updating to runtime 0123456789: after 1 job finishes starting"
        );
        assert!(!takeover_note(sha, 9, 2).contains("waiting for 9"));
    }

    use super::*;

    fn ok() -> Readings {
        Readings {
            switch_on: true,
            jobs_running: 0,
            hot: None,
            other_cpu_pct: Some(10),
            compile_free: Some(3),
            disk_free_gb: Some(400),
            held: None,
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
                "held by what the doctor found",
                Readings {
                    held: Some("masond is signed ad hoc; privacy grants will not hold".into()),
                    ..ok()
                },
                Some("masond is signed ad hoc"),
            ),
            (
                "the switch is named before what the doctor found",
                Readings {
                    switch_on: false,
                    held: Some("x".into()),
                    ..ok()
                },
                Some("turned remote work off"),
            ),
            (
                "what the doctor found is named before heat",
                Readings {
                    held: Some("held first".into()),
                    hot: Some("die 99 °C".into()),
                    ..ok()
                },
                Some("held first"),
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

    fn fake_xcode(dir: &std::path::Path, app: &str, plist_version: Option<&str>) {
        let c = dir.join(app).join("Contents");
        std::fs::create_dir_all(&c).unwrap();
        if let Some(v) = plist_version {
            std::fs::write(
                c.join("Info.plist"),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                     <key>CFBundleShortVersionString</key><string>{v}</string></dict></plist>\n"
                ),
            )
            .unwrap();
        }
    }

    #[test]
    fn a_plain_xcode_app_is_listed_by_the_version_in_its_plist() {
        let d = tempfile::tempdir().unwrap();
        fake_xcode(d.path(), "Xcode.app", Some("27.0"));
        assert_eq!(xcode_versions(d.path(), None), ["27.0"]);
    }

    #[test]
    fn a_version_in_the_app_name_wins_and_versions_are_not_repeated() {
        let d = tempfile::tempdir().unwrap();
        fake_xcode(d.path(), "Xcode_16.4.app", Some("99.9"));
        fake_xcode(d.path(), "Xcode.app", Some("16.4"));
        fake_xcode(d.path(), "Safari.app", Some("18.0"));
        let selected = d.path().join("Xcode.app");
        assert_eq!(xcode_versions(d.path(), Some(&selected)), ["16.4"]);
    }

    #[test]
    fn a_missing_or_broken_plist_gives_no_version() {
        let d = tempfile::tempdir().unwrap();
        fake_xcode(d.path(), "Xcode.app", None);
        assert!(xcode_versions(d.path(), None).is_empty());
        let c = d.path().join("Xcode_beta.app/Contents");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(c.join("Info.plist"), "not a plist").unwrap();
        assert!(xcode_versions(d.path(), None).is_empty());
        assert!(xcode_versions(&d.path().join("absent"), None).is_empty());
    }

    #[test]
    fn the_selected_xcode_app_is_found_in_the_xcode_select_path() {
        assert_eq!(
            selected_xcode_app("/Applications/Xcode.app/Contents/Developer\n"),
            Some(std::path::PathBuf::from("/Applications/Xcode.app"))
        );
        assert_eq!(
            selected_xcode_app("/Library/Developer/CommandLineTools\n"),
            None
        );
    }

    #[test]
    fn a_coordinator_name_over_sixty_characters_is_found_again_by_the_name_it_sent() {
        let dir = tempfile::tempdir().unwrap();
        let long_a = format!("{}-A2C80B34A8C3", "x".repeat(49));
        let long_b = format!("{}-B2C80B34A8C3", "x".repeat(49));
        assert_eq!(long_a.chars().count(), 62);
        assert_eq!(coordinator_key(&long_a).chars().count(), 60);
        assert_ne!(coordinator_key(&long_a), coordinator_key(&long_b));
        assert_eq!(
            coordinator_key(&format!(" {long_a} ")),
            coordinator_key(&long_a)
        );
        assert_eq!(coordinator_key("imac"), "imac");
        let n = NodeState::new(dir.path());
        n.add_coordinator("aa", &coordinator_key(&long_a), 1, None)
            .unwrap();
        n.add_coordinator("bb", &coordinator_key(&long_b), 2, None)
            .unwrap();
        assert!(n.remove_coordinator(&long_a).unwrap());
        assert_eq!(n.coordinators().len(), 1);
        assert!(!n.remove_coordinator(&long_a).unwrap());
        assert!(n.remove_coordinator(&long_b).unwrap());
    }

    #[test]
    fn coordinators_are_kept_in_a_private_file_and_hold_no_secret() {
        let dir = tempfile::tempdir().unwrap();
        let n = NodeState::new(dir.path());
        assert!(n.coordinators().is_empty() && !n.is_paired("aa"));
        n.add_coordinator("aa", "imac", 5, None).unwrap();
        assert!(n.is_paired("aa") && !n.is_paired("bb") && !n.is_paired(""));
        // A name used by another fingerprint is refused; the same fingerprint may rename.
        assert!(n.add_coordinator("bb", "imac", 6, None).is_err());
        n.add_coordinator("aa", "desk", 7, Some("TEAM111111".into()))
            .unwrap();
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
        n.add_coordinator("aa", "one", 1, None).unwrap();
        n.add_coordinator("bb", "two", 2, None).unwrap();
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
