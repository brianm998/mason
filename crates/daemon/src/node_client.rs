// SPDX-License-Identifier: GPL-3.0-or-later

//! The coordinator's HTTPS client for talking to a node (`docs/multi-machine-nodes-plan.md`,
//! §3.3). The coordinator's own certificate (`lan::load_or_create_identity`) is presented as the
//! TLS **client certificate**: that is how a node knows it. The node's certificate is **pinned**:
//! the connection is refused unless its SHA-256 fingerprint is the one recorded at pairing (or
//! in mDNS). No token is sent anywhere.

use crate::lan::sha256_hex;
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
    /// The connection broke or the node spoke something else than HTTP.
    Protocol(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unreachable(m) | ClientError::Protocol(m) => write!(f, "{m}"),
            ClientError::WrongFingerprint { expected } => write!(
                f,
                "that node's certificate is not the one paired (expected fingerprint {}…); pair it again if it was reinstalled",
                &expected[..expected.len().min(12)]
            ),
        }
    }
}

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
        let tcp = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| ClientError::Unreachable(format!("{addr}: {e}")))?;
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
