// SPDX-License-Identifier: GPL-3.0-or-later

//! The coordinator's HTTPS client for talking to a node (`docs/multi-machine-nodes-plan.md`,
//! §3.3). The coordinator's own certificate (`lan::load_or_create_identity`) is presented as the
//! TLS **client certificate**: that is how a node knows it. The node's certificate is **pinned**:
//! the connection is refused unless its SHA-256 fingerprint is the one recorded at pairing (or
//! in mDNS). No token is sent anywhere.

use crate::lan::sha256_hex;
use crate::stable_app::Location;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{ring::default_provider, CryptoProvider};
use tokio_rustls::rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime,
};
use tokio_rustls::rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

/// What a node answered.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// The node's own words for a refusal (`{"error": ...}`), or the status.
    pub fn error_text(&self) -> String {
        self.json()["error"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| format!("HTTP {}", self.status))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// The node could not be reached or did not answer in time.
    Unreachable(String),
    /// The node presented a certificate other than the one pinned.
    WrongFingerprint { expected: String },
    /// macOS answered "no route to host" for a machine on the local network: what a process
    /// without Local Network permission gets (§3.14). A node that is simply off the network
    /// answers the same, so the words say both.
    LocalNetworkDenied(String),
    /// "No route to host" although the node answers ping, from an executable on an external
    /// disk: macOS has a Local Network rule it cannot apply (`stable_app.rs`).
    LocalNetworkUnapplied(String),
    /// The connection broke or the node spoke something else than HTTP.
    Protocol(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unreachable(m) | ClientError::Protocol(m) => write!(f, "{m}"),
            ClientError::LocalNetworkDenied(addr) => write!(
                f,
                "local network access denied by macOS for MASON ({addr}: no route to host; if the node is up, turn Local Network on for MASON in System Settings → Privacy & Security → Local Network; if it is already on, see docs/nodes.md)"
            ),
            ClientError::LocalNetworkUnapplied(addr) => {
                write!(f, "{addr}: {LOCAL_NETWORK_UNAPPLIED}")
            }
            ClientError::WrongFingerprint { expected } => write!(
                f,
                "that node's certificate is not the one paired (expected fingerprint {}…); pair it again if it was reinstalled",
                &expected[..expected.len().min(12)]
            ),
        }
    }
}

/// What a Local Network rule macOS cannot apply is called, wherever it is reported.
pub const LOCAL_NETWORK_UNAPPLIED: &str = "macOS has a Local Network rule for masond but cannot apply it (the binary lives on an external disk). Run MASON from the internal disk (see docs/nodes.md)";

/// Accepts the server certificate whose fingerprint is `expected`, and no other.
#[derive(Debug)]
struct Pin {
    expected: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, tokio_rustls::rustls::Error> {
        if sha256_hex(end_entity.as_ref()) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(tokio_rustls::rustls::Error::General(
                "node fingerprint mismatch".into(),
            ))
        }
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

/// A client for one node: where it is, and the fingerprint it must present.
#[derive(Clone)]
pub struct NodeClient {
    host: String,
    port: u16,
    expected: String,
    cfg: Arc<ClientConfig>,
    timeout: Duration,
}

impl NodeClient {
    /// `identity` is the coordinator's certificate and key (DER). `expected` is the node's
    /// fingerprint, lower-case hex.
    pub fn new(
        identity: (Vec<u8>, Vec<u8>),
        host: &str,
        port: u16,
        expected: &str,
    ) -> std::io::Result<NodeClient> {
        let provider = Arc::new(default_provider());
        let cfg = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(std::io::Error::other)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pin {
                expected: expected.to_ascii_lowercase(),
                provider,
            }))
            .with_client_auth_cert(
                vec![CertificateDer::from(identity.0)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.1)),
            )
            .map_err(std::io::Error::other)?;
        Ok(NodeClient {
            host: host.to_string(),
            port,
            expected: expected.to_ascii_lowercase(),
            cfg: Arc::new(cfg),
            timeout: Duration::from_secs(15),
        })
    }

    /// The most a call may take, connecting included.
    pub fn with_timeout(mut self, t: Duration) -> NodeClient {
        self.timeout = t;
        self
    }

    /// One request. `body` is sent as JSON when it is given.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<Response, ClientError> {
        match body {
            Some(b) => {
                self.send(
                    method,
                    path,
                    Some("application/json"),
                    b.to_string().into_bytes(),
                )
                .await
            }
            None => self.send(method, path, None, Vec::new()).await,
        }
    }

    /// One request with a body of bytes (a git bundle), of the given kind.
    pub async fn send(
        &self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        payload: Vec<u8>,
    ) -> Result<Response, ClientError> {
        match tokio::time::timeout(self.timeout, self.once(method, path, content_type, payload))
            .await
        {
            Ok(r) => r,
            Err(_) => Err(ClientError::Unreachable(format!(
                "{}:{} did not answer",
                self.host, self.port
            ))),
        }
    }

    async fn once(
        &self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        payload: Vec<u8>,
    ) -> Result<Response, ClientError> {
        let addr = format!("{}:{}", self.host, self.port);
        let tcp = match tokio::net::TcpStream::connect(&addr).await {
            Ok(t) => t,
            Err(e) if denied_by_macos(&e, &self.host) => {
                let (code, host, a) = (e.raw_os_error(), self.host.clone(), addr.clone());
                let found = tokio::task::spawn_blocking(move || {
                    let ping = answers_ping(&host);
                    let location = if ping {
                        crate::stable_app::own_location()
                    } else {
                        Location::Unknown
                    };
                    local_network_error(code, &host, &a, ping, location)
                })
                .await;
                return Err(found
                    .ok()
                    .flatten()
                    .unwrap_or(ClientError::LocalNetworkDenied(addr)));
            }
            Err(e) => return Err(ClientError::Unreachable(format!("{addr}: {e}"))),
        };
        let name = ServerName::try_from(self.host.clone())
            .map_err(|_| ClientError::Unreachable(format!("{} is not a host name", self.host)))?;
        let tls = tokio_rustls::TlsConnector::from(self.cfg.clone())
            .connect(name, tcp)
            .await
            .map_err(|e| {
                if e.to_string().contains("fingerprint mismatch") {
                    ClientError::WrongFingerprint {
                        expected: self.expected.clone(),
                    }
                } else {
                    ClientError::Unreachable(format!("{addr}: {e}"))
                }
            })?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(tls))
                .await
                .map_err(|e| ClientError::Protocol(e.to_string()))?;
        tokio::spawn(conn);
        let mut req = hyper::Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::HOST, addr.as_str());
        if let Some(t) = content_type {
            req = req.header(hyper::header::CONTENT_TYPE, t);
        }
        let req = req
            .body(Full::new(Bytes::from(payload)))
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        let resp = sender.send_request(req).await.map_err(|e| {
            // A node that refuses our certificate at the TLS level shows up here, since in
            // TLS 1.3 the client finishes its handshake before the server has judged it.
            ClientError::Protocol(e.to_string())
        })?;
        let status = resp.status().as_u16();
        let body = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?
            .to_bytes()
            .to_vec();
        Ok(Response { status, body })
    }
}

/// Is `host` a machine on the local network (a private or link-local address, or a `.local` name)?
pub fn is_lan_host(host: &str) -> bool {
    match host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => a.is_private() || a.is_link_local(),
        Ok(std::net::IpAddr::V6(a)) => {
            let s = a.segments()[0];
            (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
        Err(_) => host.ends_with(".local"),
    }
}

/// Does this connect error look like macOS refusing Local Network access? A process without the
/// permission gets `EHOSTUNREACH` (65, "No route to host") connecting to a machine on the LAN.
pub fn denied_by_macos(e: &std::io::Error, host: &str) -> bool {
    cfg!(target_os = "macos") && e.raw_os_error() == Some(MACOS_EHOSTUNREACH) && is_lan_host(host)
}

/// `EHOSTUNREACH` on macOS.
const MACOS_EHOSTUNREACH: i32 = 65;

/// Which Local Network failure a macOS connect error is, if it is one: `EHOSTUNREACH` to a
/// machine on the LAN. A node that answers ping while this executable is on an external disk is
/// the rule macOS cannot apply; anything else may be a denial or a node that is off. Pure.
pub fn local_network_error(
    code: Option<i32>,
    host: &str,
    addr: &str,
    answers_ping: bool,
    executable: Location,
) -> Option<ClientError> {
    if code != Some(MACOS_EHOSTUNREACH) || !is_lan_host(host) {
        return None;
    }
    Some(if answers_ping && executable == Location::External {
        ClientError::LocalNetworkUnapplied(addr.to_string())
    } else {
        ClientError::LocalNetworkDenied(addr.to_string())
    })
}

/// Does `host` answer one ping? A ping that cannot run counts as no answer.
fn answers_ping(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    // A leading `-` would be read as an option.
    if host.starts_with('-') {
        return false;
    }
    let program = if host.contains(':') {
        "/sbin/ping6"
    } else {
        "/sbin/ping"
    };
    mason_landing::proc::output(
        std::process::Command::new(program).args(["-c", "1", host]),
        Duration::from_secs(3),
    )
    .is_ok_and(|o| o.status.success())
}

/// Run `f` to its end from a thread that is not a tokio worker: the daemon's blocking code
/// (a step's thread) talks to nodes through this. From inside a runtime it runs on a helper
/// thread, since a runtime cannot be started inside another.
pub fn block_on<F>(f: F) -> F::Output
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    let rt = RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("node-client")
            .build()
            .expect("a runtime for node requests")
    });
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| {
            s.spawn(|| rt.block_on(f))
                .join()
                .unwrap_or_else(|e| std::panic::resume_unwind(e))
        })
    } else {
        rt.block_on(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lan_host_is_a_private_or_link_local_address_or_a_local_name() {
        for h in [
            "192.168.1.40",
            "10.0.0.7",
            "172.16.5.5",
            "169.254.1.2",
            "studio.local",
            "fd00::1",
            "fe80::1",
            "[fe80::1]",
        ] {
            assert!(is_lan_host(h), "{h}");
        }
        for h in ["127.0.0.1", "8.8.8.8", "example.com", "::1"] {
            assert!(!is_lan_host(h), "{h}");
        }
    }

    #[test]
    fn no_route_to_a_private_address_that_pings_from_an_external_disk_is_an_unapplied_rule() {
        let of = |code, host: &str, ping, at| local_network_error(code, host, "a:1", ping, at);
        let unapplied = of(Some(65), "10.10.10.5", true, Location::External).unwrap();
        assert_eq!(unapplied, ClientError::LocalNetworkUnapplied("a:1".into()));
        let m = unapplied.to_string();
        assert!(m.contains(LOCAL_NETWORK_UNAPPLIED), "{m}");
        assert!(!m.contains("turn Local Network on"), "{m}");
        // No ping answer, or an executable that is not known to be external: not that claim.
        for (ping, at) in [
            (false, Location::External),
            (true, Location::Internal),
            (true, Location::Unknown),
        ] {
            assert_eq!(
                of(Some(65), "10.10.10.5", ping, at),
                Some(ClientError::LocalNetworkDenied("a:1".into()))
            );
        }
        // Other errors, and other hosts, are left alone.
        assert_eq!(of(Some(61), "10.10.10.5", true, Location::External), None);
        assert_eq!(of(None, "10.10.10.5", true, Location::External), None);
        assert_eq!(of(Some(65), "8.8.8.8", true, Location::External), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn no_route_to_a_lan_host_is_what_a_macos_denial_looks_like_and_nothing_else_is() {
        let no_route = std::io::Error::from_raw_os_error(65);
        let refused = std::io::Error::from_raw_os_error(61);
        assert!(denied_by_macos(&no_route, "192.168.1.40"));
        assert!(denied_by_macos(&no_route, "studio.local"));
        assert!(
            !denied_by_macos(&no_route, "8.8.8.8"),
            "the internet is not the local network"
        );
        assert!(
            !denied_by_macos(&refused, "192.168.1.40"),
            "a refused connection is a node that is not listening"
        );
        let m = ClientError::LocalNetworkDenied("192.168.1.40:7425".into()).to_string();
        assert!(m.starts_with("local network access denied by macOS"), "{m}");
        assert!(
            m.contains("System Settings → Privacy & Security → Local Network"),
            "{m}"
        );
    }
}
