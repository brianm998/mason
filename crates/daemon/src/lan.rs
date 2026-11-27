// SPDX-License-Identifier: GPL-3.0-or-later

//! LAN access (Part 16): a second listener, off unless the owner turns it on, that serves the
//! same API over TLS to paired devices only. Loopback is untouched and stays trusted.
//!
//! Every request on this listener must carry a device token (`Authorization: Bearer`); the
//! token's role decides what it may do (`mason_core::access`), and the `x-mason-as` header is
//! replaced, never trusted. Tokens are random, kept only as SHA-256, and revocable at once.
//! The certificate is self-signed; clients pin its SHA-256 fingerprint, which is also in the
//! mDNS record and in the pairing URL. Codes and tokens are never logged.

use crate::daemon::{now_ms, Daemon};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use mason_core::access::{decide, Access, PairError, Pairing, Role};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path as FsPath;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The port the LAN listener uses when `[lan] enabled = true` names none.
pub const DEFAULT_LAN_PORT: u16 = 7424;

/// The mDNS service type.
pub const SERVICE_TYPE: &str = "_mason._tcp.local.";

/// Who a request on the LAN listener came from.
#[derive(Clone, Debug)]
pub struct Peer {
    pub device: u64,
    pub name: String,
    pub role: Role,
}

/// What the running listener is, for `GET /lan` and the pairing URL.
#[derive(Clone, Debug)]
pub struct Info {
    pub port: u16,
    pub fingerprint: String,
    pub instance: String,
    pub txt: BTreeMap<String, String>,
}

pub struct LanState {
    pairing: Mutex<Pairing>,
    info: Mutex<Option<Info>>,
    /// Ids of revoked devices, so open sockets can close.
    pub revoked: tokio::sync::broadcast::Sender<u64>,
}

impl LanState {
    pub fn new() -> Self {
        LanState {
            pairing: Mutex::new(Pairing::default()),
            info: Mutex::new(None),
            revoked: tokio::sync::broadcast::channel(64).0,
        }
    }
    pub fn info(&self) -> Option<Info> {
        self.info.lock().ok().and_then(|g| g.clone())
    }
}

impl Default for LanState {
    fn default() -> Self {
        Self::new()
    }
}

type Res = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn fail(code: u16, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
        Json(json!({"error": msg.into()})),
    )
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn random_bytes(n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf)
}

fn new_token() -> std::io::Result<String> {
    let b = random_bytes(32)?;
    Ok(format!(
        "mt_{}",
        b.iter().map(|x| format!("{x:02x}")).collect::<String>()
    ))
}

/// A fresh eight-digit code, uniform (values above the largest multiple of 10^8 are redrawn).
pub(crate) fn new_code() -> std::io::Result<String> {
    const LIMIT: u64 = u64::MAX - (u64::MAX % 100_000_000);
    loop {
        let b = random_bytes(8)?;
        let n = u64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
        if n < LIMIT {
            return Ok(format!("{:08}", n % 100_000_000));
        }
    }
}

pub(crate) fn write_private(path: &FsPath, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new();
    f.write(true).create(true).truncate(true);
    let mut f = mason_platform::fs::owner_only(&mut f).open(path)?;
    f.write_all(bytes)
}

/// The self-signed certificate and key (DER), made once and kept in the data directory.
pub fn load_or_create_identity(data_dir: &FsPath) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let dir = data_dir.join("lan");
    std::fs::create_dir_all(&dir)?;
    let (cp, kp) = (dir.join("cert.der"), dir.join("key.der"));
    if let (Ok(c), Ok(k)) = (std::fs::read(&cp), std::fs::read(&kp)) {
        return Ok((c, k));
    }
    let host = hostname();
    let cert = rcgen::generate_simple_self_signed(vec![format!("{host}.local"), host])
        .map_err(|e| std::io::Error::other(format!("cannot make the certificate: {e}")))?;
    let c = cert
        .serialize_der()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let k = cert.serialize_private_key_der();
    write_private(&kp, &k)?;
    write_private(&cp, &c)?;
    Ok((c, k))
}

/// The `.local` name this machine answers to, without the suffix. On macOS that is the current
/// `LocalHostName` (which moves when the system renames the Mac), not `gethostname`.
pub fn hostname() -> String {
    #[cfg(target_os = "macos")]
    let local = local_host_name();
    #[cfg(not(target_os = "macos"))]
    let local = None;
    pick_hostname(local, mason_platform::host::name())
}

/// `LocalHostName` if there is one, else the `gethostname` answer, else `mason`; no `.local`.
pub(crate) fn pick_hostname(local: Option<String>, gethost: Option<String>) -> String {
    let clean = |h: String| h.trim().trim_end_matches(".local").to_string();
    local
        .map(clean)
        .filter(|h| !h.is_empty())
        .or_else(|| gethost.map(clean).filter(|h| !h.is_empty()))
        .unwrap_or_else(|| "mason".into())
}

/// `scutil --get LocalHostName`, cached briefly: it is read on request paths.
#[cfg(target_os = "macos")]
fn local_host_name() -> Option<String> {
    use std::time::Instant;
    static CACHE: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, v)) = g.as_ref() {
        if t.elapsed() < Duration::from_secs(30) {
            return v.clone();
        }
    }
    let v = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    *g = Some((Instant::now(), v.clone()));
    v
}

fn tls_config(
    cert: Vec<u8>,
    key: Vec<u8>,
) -> std::io::Result<Arc<tokio_rustls::rustls::ServerConfig>> {
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio_rustls::rustls::{crypto::ring::default_provider, ServerConfig};
    let cfg = ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(std::io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .map_err(std::io::Error::other)?;
    Ok(Arc::new(cfg))
}

/// The record advertised over mDNS (TXT).
pub fn txt_record(fingerprint: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("proto".to_string(), mason_protocol::PROTOCOL.to_string()),
        ("fp".to_string(), fingerprint.to_string()),
        ("version".to_string(), env!("CARGO_PKG_VERSION").to_string()),
    ])
}

/// A live advertisement. Kept for the life of the process (callers `std::mem::forget` it).
pub(crate) enum Advertisement {
    #[cfg(target_os = "macos")]
    System(crate::dnssd::Registration),
    #[cfg(not(target_os = "macos"))]
    Own(#[allow(dead_code)] mdns_sd::ServiceDaemon),
}

/// The `ServiceInfo` for platforms that run their own responder. Its host name is ours
/// (`mason-<first 8 of the fingerprint>.local.`), never the machine's, so no one is ever told
/// this process answers for the machine's own `.local` name.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn service_info(info: &Info) -> Result<mdns_sd::ServiceInfo, String> {
    let fp8: String = info.fingerprint.chars().take(8).collect();
    let props: std::collections::HashMap<String, String> = info.txt.clone().into_iter().collect();
    mdns_sd::ServiceInfo::new(
        SERVICE_TYPE,
        &info.instance,
        &format!("mason-{fp8}.local."),
        "",
        info.port,
        props,
    )
    .map(|s| s.enable_addr_auto())
    .map_err(|e| e.to_string())
}

/// Advertise `_mason._tcp`. macOS registers through the system responder (null host, so the
/// record names the Mac's real `.local` name); elsewhere `mdns-sd` runs with its own host name.
pub(crate) fn advertise(info: &Info) -> Result<Advertisement, String> {
    #[cfg(target_os = "macos")]
    {
        crate::dnssd::register(&info.instance, info.port, &info.txt).map(Advertisement::System)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mdns = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
        mdns.register(service_info(info)?)
            .map_err(|e| e.to_string())?;
        Ok(Advertisement::Own(mdns))
    }
}

/// Accept TLS connections on one listener and serve the shared router on them.
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
            let _ = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(tls), TowerToHyperService::new(app))
                .await;
        });
    }
}

/// Bind the LAN listener(s) and serve them in the background. Returns the port it got.
pub async fn start(d: Arc<Daemon>) -> std::io::Result<u16> {
    let (cert, key) = load_or_create_identity(&d.cfg.data_dir)?;
    let fingerprint = sha256_hex(&cert);
    let acceptor = tokio_rustls::TlsAcceptor::from(tls_config(cert, key)?);
    // One listener per bind address. With port 0 the first one picks the port, the rest use it.
    let mut listeners = Vec::new();
    let mut port = d.cfg.lan.port;
    for addr in &d.cfg.lan.bind {
        let l = tokio::net::TcpListener::bind((*addr, port)).await?;
        port = l.local_addr()?.port();
        listeners.push(l);
    }
    let info = Info {
        port,
        fingerprint: fingerprint.clone(),
        instance: format!("MASON on {}", hostname()),
        txt: txt_record(&fingerprint),
    };
    if let Ok(mut g) = d.lan.info.lock() {
        *g = Some(info.clone());
    }
    match advertise(&info) {
        // Kept for the life of the process.
        Ok(m) => std::mem::forget(m), // an `Advertisement`
        Err(e) => tracing::warn!("mDNS is not advertising: {e}"),
    }
    let app = router(d.clone());
    tracing::info!(
        port,
        bind = ?d.cfg.lan.bind,
        "masond listening on the LAN (TLS, paired devices only)"
    );
    for listener in listeners {
        tokio::spawn(serve(listener, acceptor.clone(), app.clone()));
    }
    Ok(port)
}

/// The LAN listener's routes: pairing is open (a code is the credential); everything else is
/// behind the token check.
pub fn router(d: Arc<Daemon>) -> Router {
    let guarded = crate::http::router(d.clone())
        .layer(axum::middleware::from_fn_with_state(d.clone(), guard));
    Router::new()
        .route("/pair", post(pair))
        .with_state(d)
        .merge(guarded)
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "this daemon only talks to paired devices; pair with a code"})),
    )
        .into_response()
}

fn bearer(h: &axum::http::HeaderMap) -> Option<String> {
    h.get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// The name an owner token acts as.
pub fn owner_identity(d: &Daemon) -> String {
    d.cfg
        .owner_names
        .first()
        .cloned()
        .unwrap_or_else(|| "owner".into())
}

async fn guard(State(d): State<Arc<Daemon>>, mut req: Request, next: Next) -> Response {
    let Some(token) = bearer(req.headers()) else {
        return unauthorized();
    };
    let hash = sha256_hex(token.as_bytes());
    let d2 = d.clone();
    let found = d.pool.run(move || d2.store.find_device(&hash)).await;
    let Ok(Ok(Some(dev))) = found else {
        return unauthorized();
    };
    let Some(role) = Role::parse(&dev.role) else {
        return unauthorized();
    };
    if let Access::Refuse(code, why) = decide(role, req.method().as_str(), req.uri().path()) {
        return fail(code, why).into_response();
    }
    let now = now_ms();
    if dev.last_seen.is_none_or(|t| now.saturating_sub(t) > 30_000) {
        let (d2, id) = (d.clone(), dev.id);
        let _ = d.pool.run(move || d2.store.touch_device(id, now)).await;
    }
    let as_who = match role {
        Role::Owner => owner_identity(&d),
        Role::Viewer => format!("viewer:{}", dev.id),
    };
    req.headers_mut().remove("x-mason-as");
    if let Ok(v) = HeaderValue::from_str(&as_who) {
        req.headers_mut().insert("x-mason-as", v);
    }
    req.extensions_mut().insert(Peer {
        device: dev.id,
        name: dev.name,
        role,
    });
    next.run(req).await
}

/// Turn a code into a token. Open to any LAN client; the code is the credential.
async fn pair(State(d): State<Arc<Daemon>>, Json(b): Json<Value>) -> Res {
    let code = b["code"].as_str().unwrap_or_default().to_string();
    let given = b["name"].as_str().unwrap_or_default().to_string();
    let now = now_ms();
    let role = {
        let mut p = d
            .lan
            .pairing
            .lock()
            .map_err(|_| fail(500, "internal error"))?;
        p.attempt(&code, now)
    };
    let role = role.map_err(|e| {
        let status = match e {
            PairError::Malformed | PairError::Wrong => 400,
            PairError::Expired => 410,
            PairError::Locked => 429,
        };
        fail(status, e.message())
    })?;
    let token = new_token().map_err(|_| fail(500, "no randomness available"))?;
    let hash = sha256_hex(token.as_bytes());
    let name: String = given.trim().chars().take(60).collect();
    let name = if name.is_empty() {
        "device".to_string()
    } else {
        name
    };
    let (d2, n2) = (d.clone(), name.clone());
    let id = d
        .pool
        .run(move || d2.store.create_device(&n2, role.as_str(), &hash, now))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?
        .map_err(|_| fail(500, "could not record the device"))?;
    tracing::info!(device = id, name = %name, role = role.as_str(), "device paired");
    let fp = d.lan.info().map(|i| i.fingerprint).unwrap_or_default();
    Ok(Json(
        json!({"token": token, "role": role.as_str(), "device": id, "fingerprint": fp}),
    ))
}

// ---- routes of the main API (loopback, or an owner token) ---------------------------------

/// The names the phone may use: `<host>.local` first, then the configured extras.
pub fn reach_hosts(extra: &[String]) -> Vec<String> {
    let mut v = vec![format!("{}.local", hostname())];
    v.extend(extra.iter().cloned());
    v
}

/// The `mason://pair` URL: `host=` is the first name (older clients read only it), then one
/// `alt=` for each other.
pub fn pair_url(hosts: &[String], port: u16, fingerprint: &str, code: &str) -> String {
    let mut url = format!(
        "mason://pair?host={}&port={port}&fp={fingerprint}&code={code}",
        hosts[0]
    );
    for h in &hosts[1..] {
        url.push_str("&alt=");
        url.push_str(h);
    }
    url
}

/// `GET /lan`: whether the listener is up, and what it advertises.
pub async fn status(State(d): State<Arc<Daemon>>) -> Res {
    Ok(Json(match d.lan.info() {
        None => json!({"enabled": false}),
        Some(i) => json!({
            "enabled": true, "port": i.port, "fingerprint": i.fingerprint,
            "instance": i.instance, "service": SERVICE_TYPE, "txt": i.txt,
            "hosts": reach_hosts(&d.cfg.lan.hosts),
            "bind": d.cfg.lan.bind.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
        }),
    }))
}

/// `POST /pair/codes`: make a code for a new device (a trusted client only; the LAN guard keeps
/// viewers out). The response carries the `mason://pair` URL a QR code shows.
pub async fn issue_code(State(d): State<Arc<Daemon>>, body: Option<Json<Value>>) -> Res {
    let role = body
        .and_then(|b| b.0["role"].as_str().map(String::from))
        .unwrap_or_default();
    let role = if role.is_empty() {
        Role::Owner
    } else {
        Role::parse(&role).ok_or_else(|| fail(400, "role is `owner` or `viewer`"))?
    };
    let Some(info) = d.lan.info() else {
        return Err(fail(
            409,
            "LAN access is off; set `[lan] enabled = true` in daemon.toml or start masond with --lan",
        ));
    };
    let code = new_code().map_err(|_| fail(500, "no randomness available"))?;
    d.lan
        .pairing
        .lock()
        .map_err(|_| fail(500, "internal error"))?
        .issue(code.clone(), role, now_ms());
    let hosts = reach_hosts(&d.cfg.lan.hosts);
    let host = hosts[0].clone();
    let url = pair_url(&hosts, info.port, &info.fingerprint, &code);
    Ok(Json(json!({
        "code": code, "role": role.as_str(), "expires_in_s": mason_core::access::CODE_TTL_MS / 1000,
        "host": host, "hosts": hosts, "port": info.port, "fingerprint": info.fingerprint, "url": url,
    })))
}

pub async fn devices(State(d): State<Arc<Daemon>>) -> Res {
    let d2 = d.clone();
    let rows = d
        .pool
        .run(move || d2.store.list_devices())
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?
        .map_err(|e| fail(500, e.to_string()))?;
    Ok(Json(json!({"devices": rows.iter().map(|r| json!({
        "id": r.id, "name": r.name, "role": r.role, "created_at": r.created_at,
        "last_seen": r.last_seen, "revoked_at": r.revoked_at,
    })).collect::<Vec<_>>()})))
}

/// The calling device's own row, and nothing else: the LAN guard names the caller. A request
/// that did not come over the LAN has no device to speak for.
fn own_device(peer: Option<axum::Extension<Peer>>) -> Result<u64, (StatusCode, Json<Value>)> {
    peer.map(|p| p.0.device)
        .ok_or_else(|| fail(403, "only a paired device can register its own push token"))
}

/// `POST /devices/self/push {token, env}`: this device wants alerts pushed to it (APNs).
pub async fn set_push(
    State(d): State<Arc<Daemon>>,
    peer: Option<axum::Extension<Peer>>,
    Json(b): Json<Value>,
) -> Res {
    let id = own_device(peer)?;
    let token = b["token"].as_str().unwrap_or_default().to_string();
    let env = b["env"].as_str().unwrap_or_default().to_string();
    let d2 = d.clone();
    d.pool
        .run(move || crate::notify::set_own_push(&d2, id, Some((&token, &env))))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?
        .map_err(|e| fail(400, e))?;
    Ok(Json(json!({"device": id, "push": true})))
}

/// `DELETE /devices/self/push`: this device wants no more pushes.
pub async fn clear_push(State(d): State<Arc<Daemon>>, peer: Option<axum::Extension<Peer>>) -> Res {
    let id = own_device(peer)?;
    let d2 = d.clone();
    d.pool
        .run(move || crate::notify::set_own_push(&d2, id, None))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?
        .map_err(|e| fail(500, e))?;
    Ok(Json(json!({"device": id, "push": false})))
}

pub async fn revoke(State(d): State<Arc<Daemon>>, Path(id): Path<u64>) -> Res {
    let (d2, now) = (d.clone(), now_ms());
    let done = d
        .pool
        .run(move || d2.store.revoke_device(id, now))
        .await
        .map_err(|_| fail(503, "the daemon is busy; try again"))?
        .map_err(|e| fail(500, e.to_string()))?;
    if !done {
        return Err(fail(404, format!("no live device #{id}")));
    }
    let _ = d.lan.revoked.send(id);
    tracing::info!(device = id, "device revoked");
    Ok(Json(json!({"revoked": id})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_is_made_once_and_the_key_is_private() {
        let d = tempfile::tempdir().unwrap();
        let (c1, k1) = load_or_create_identity(d.path()).unwrap();
        let (c2, k2) = load_or_create_identity(d.path()).unwrap();
        assert_eq!((&c1, &k1), (&c2, &k2));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(d.path().join("lan/key.der"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(sha256_hex(&c1).len(), 64);
    }

    #[test]
    fn the_pairing_url_has_an_alt_for_each_extra_host() {
        let hosts = vec!["a.local".to_string(), "m.ts.net".into(), "100.1.2.3".into()];
        let u = pair_url(&hosts, 7424, "ff", "12345678");
        assert_eq!(
            u,
            "mason://pair?host=a.local&port=7424&fp=ff&code=12345678&alt=m.ts.net&alt=100.1.2.3"
        );
        let one = pair_url(&hosts[..1], 1, "ff", "12345678");
        assert!(!one.contains("alt="));
        let r = reach_hosts(&["x.ts.net".into()]);
        assert!(r[0].ends_with(".local") && r[1] == "x.ts.net");
    }

    #[test]
    fn sha256_matches_the_known_answer() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_txt_record_has_proto_fp_and_version() {
        let t = txt_record("ff");
        assert_eq!(t["fp"], "ff");
        assert!(t.contains_key("proto") && t.contains_key("version"));
    }

    #[test]
    fn codes_are_eight_digits_and_tokens_are_long() {
        for _ in 0..20 {
            assert!(mason_core::access::well_formed(&new_code().unwrap()));
        }
        assert_eq!(new_token().unwrap().len(), 3 + 64);
        assert_ne!(new_token().unwrap(), new_token().unwrap());
    }

    #[test]
    fn the_bearer_header_is_read() {
        let mut h = axum::http::HeaderMap::new();
        assert_eq!(bearer(&h), None);
        h.insert("authorization", "Bearer abc".parse().unwrap());
        assert_eq!(bearer(&h).as_deref(), Some("abc"));
        h.insert("authorization", "Basic abc".parse().unwrap());
        assert_eq!(bearer(&h), None);
    }
}

#[cfg(test)]
mod hostname_tests {
    use super::*;

    #[test]
    fn the_local_host_name_wins_and_loses_its_suffix() {
        let h = pick_hostname(Some("Marmot-827.local".into()), Some("Marmot-807".into()));
        assert_eq!(h, "Marmot-827");
        let h = pick_hostname(Some("Marmot-827".into()), None);
        assert_eq!(h, "Marmot-827");
    }

    #[test]
    fn it_falls_back_to_gethostname() {
        assert_eq!(pick_hostname(None, Some("box.local".into())), "box");
        assert_eq!(pick_hostname(Some(" ".into()), Some("box".into())), "box");
        assert_eq!(pick_hostname(None, None), "mason");
    }

    #[test]
    fn the_service_host_is_never_the_machines_name() {
        let info = Info {
            port: 7425,
            fingerprint: "0123456789abcdef".into(),
            instance: "MASON node on box".into(),
            txt: txt_record("0123456789abcdef"),
        };
        let svc = service_info(&info).unwrap();
        assert_eq!(svc.get_hostname(), "mason-01234567.local.");
        assert_ne!(
            svc.get_hostname().trim_end_matches('.'),
            format!("{}.local", hostname())
        );
    }
}
