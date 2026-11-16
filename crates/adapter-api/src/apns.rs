// SPDX-License-Identifier: GPL-3.0-or-later

//! Push to a paired iPhone or iPad straight through Apple's push service (APNs), signed with the
//! owner's own key, no relay. The provider token is an ES256 JWT made with `ring` and kept for
//! 50 minutes (Apple wants it refreshed between 20 and 60). One HTTP/2 POST goes to each device
//! through the system `curl` ([`crate::http::Curl`]): the JWT travels in the config on curl's
//! standard input, never on a command line, and never in an error.

use crate::http::{Http, Request};
use crate::notify_kit::{one_line, Message, SendError};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use std::sync::{Arc, Mutex};

pub const PRODUCTION_HOST: &str = "https://api.push.apple.com";
pub const SANDBOX_HOST: &str = "https://api.sandbox.push.apple.com";

/// How long a provider token is reused before a new one is made.
pub const JWT_TTL_MS: u64 = 50 * 60 * 1000;

/// What a push needs from the alert beyond its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// The project the alert is about; becomes the `thread-id`, so alerts group by project.
    pub project: String,
    /// The task the alert is about, for the tap to open.
    pub task: Option<u64>,
}

/// One registered device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The device token, hex.
    pub token: String,
    /// `sandbox` or `production`.
    pub env: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushError {
    /// Apple says this device token is dead (410, or `BadDeviceToken`): forget it.
    Gone(String),
    Send(SendError),
}

/// The settings of the `apns` channel.
pub struct Apns {
    /// The `.p8` text (PEM PKCS#8).
    pub key_pem: String,
    pub key_id: String,
    pub team_id: String,
    /// The app's bundle id.
    pub topic: String,
    /// Replaces both Apple hosts (tests and exit scripts point it at a loopback fake).
    pub endpoint: Option<String>,
    /// Built with `Curl { http2: true, .. }` in production.
    pub http: Arc<dyn Http>,
}

fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

/// The DER inside a PEM block (`.p8` files hold one `PRIVATE KEY`).
pub fn pem_der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("-----"))
        .collect();
    if !pem.contains("PRIVATE KEY") || body.is_empty() {
        return Err("the key file is not a PEM private key (.p8)".into());
    }
    STANDARD
        .decode(body)
        .map_err(|_| "the key file is not valid base64".to_string())
}

/// The JWT header and claims, as the text that gets signed (`<header>.<claims>`).
pub fn signing_input(key_id: &str, team_id: &str, issued_at_s: u64) -> String {
    let header = serde_json::json!({"alg": "ES256", "kid": key_id}).to_string();
    let claims = serde_json::json!({"iss": team_id, "iat": issued_at_s}).to_string();
    format!("{}.{}", b64(header.as_bytes()), b64(claims.as_bytes()))
}

/// A signed provider token. `now_ms` is when it is made.
pub fn make_jwt(key_pem: &str, key_id: &str, team_id: &str, now_ms: u64) -> Result<String, String> {
    let der = pem_der(key_pem)?;
    let rng = SystemRandom::new();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng)
        .map_err(|_| "the key file is not a P-256 private key".to_string())?;
    let input = signing_input(key_id, team_id, now_ms / 1000);
    let sig = key
        .sign(&rng, input.as_bytes())
        .map_err(|_| "signing failed".to_string())?;
    Ok(format!("{input}.{}", b64(sig.as_ref())))
}

/// (SHA-256 of the key text, key id, team id, made at, token): the one token in use, so a burst
/// of alerts signs once. The key is held as a digest, not as text.
type Cached = (Vec<u8>, String, String, u64, String);
static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

/// The cached provider token, or a new one when it is 50 minutes old or for another key,
/// key id or team.
pub fn cached_jwt(
    key_pem: &str,
    key_id: &str,
    team_id: &str,
    now_ms: u64,
) -> Result<String, String> {
    let digest = ring::digest::digest(&ring::digest::SHA256, key_pem.as_bytes())
        .as_ref()
        .to_vec();
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((d, k, t, at, jwt)) = g.as_ref() {
        if *d == digest && k == key_id && t == team_id && now_ms >= *at && now_ms - *at < JWT_TTL_MS
        {
            return Ok(jwt.clone());
        }
    }
    let jwt = make_jwt(key_pem, key_id, team_id, now_ms)?;
    *g = Some((digest, key_id.into(), team_id.into(), now_ms, jwt.clone()));
    Ok(jwt)
}

/// Drop the cached token (Apple refused it).
pub fn forget_jwt() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The notification body: the alert, the project as `thread-id`, and a `mason` dictionary with
/// the task and condition for the tap to open.
pub fn payload(m: &Message, ctx: &Context) -> String {
    let mut mason = serde_json::json!({"kind": m.kind, "key": m.key});
    if let Some(t) = ctx.task {
        mason["task"] = t.into();
    }
    serde_json::json!({
        "aps": {
            "alert": {"title": one_line(&m.title), "body": m.body},
            "sound": "default",
            "thread-id": ctx.project,
        },
        "mason": mason,
    })
    .to_string()
}

impl Apns {
    fn url(&self, t: &Target) -> Result<String, SendError> {
        if t.token.is_empty() || !t.token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(SendError::Permanent("the device token is not hex".into()));
        }
        let host = match (&self.endpoint, t.env.as_str()) {
            (Some(e), _) => e.trim_end_matches('/').to_string(),
            (None, "sandbox") => SANDBOX_HOST.to_string(),
            (None, "production") => PRODUCTION_HOST.to_string(),
            (None, other) => {
                return Err(SendError::Permanent(format!(
                    "the device's push environment is `{other}`, not sandbox or production"
                )))
            }
        };
        Ok(format!("{host}/3/device/{}", t.token))
    }

    /// Send one alert to one device.
    pub fn send_to(
        &self,
        t: &Target,
        m: &Message,
        ctx: &Context,
        now_ms: u64,
    ) -> Result<(), PushError> {
        let url = self.url(t).map_err(PushError::Send)?;
        let jwt = cached_jwt(&self.key_pem, &self.key_id, &self.team_id, now_ms)
            .map_err(|e| PushError::Send(SendError::Permanent(e)))?;
        let req = Request::post(&url, &payload(m, ctx))
            .with("authorization", &format!("bearer {jwt}"))
            .with("apns-topic", &one_line(&self.topic))
            .with("apns-push-type", "alert")
            .with("apns-priority", if m.reminder { "5" } else { "10" })
            .with("content-type", "application/json");
        let scrub = |s: String| s.replace(&jwt, "***");
        let resp = self
            .http
            .send(&req)
            .map_err(|e| PushError::Send(SendError::Transient(scrub(e.0))))?;
        let reason = serde_json::from_str::<serde_json::Value>(&resp.body)
            .ok()
            .and_then(|v| v["reason"].as_str().map(String::from))
            .unwrap_or_default();
        match resp.status {
            200..=299 => Ok(()),
            410 => Err(PushError::Gone(if reason.is_empty() {
                "Unregistered".into()
            } else {
                reason
            })),
            400 if matches!(reason.as_str(), "BadDeviceToken" | "DeviceTokenNotForTopic") => {
                Err(PushError::Gone(reason))
            }
            403 if reason == "ExpiredProviderToken" => {
                forget_jwt();
                Err(PushError::Send(SendError::Transient(
                    "Apple said the provider token expired".into(),
                )))
            }
            408 | 429 | 500..=599 => Err(PushError::Send(SendError::Transient(format!(
                "APNs answered {} {reason}",
                resp.status
            )))),
            s => Err(PushError::Send(SendError::Permanent(format!(
                "APNs answered {s} {reason}"
            )))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Curl, Recorded};
    use ring::signature::{KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};

    /// A fresh P-256 key as the PEM text of a `.p8`, and its public key.
    fn key() -> (String, Vec<u8>) {
        let rng = SystemRandom::new();
        let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der.as_ref(), &rng).unwrap();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            STANDARD.encode(der.as_ref())
        );
        (pem, pair.public_key().as_ref().to_vec())
    }

    #[test]
    fn two_keys_with_the_same_kid_and_team_each_get_their_own_jwt() {
        let (pem_a, pub_a) = key();
        let (pem_b, pub_b) = key();
        let ja = cached_jwt(&pem_a, "SAMEKID123", "SAMETEAM12", 1_000_000).unwrap();
        let jb = cached_jwt(&pem_b, "SAMEKID123", "SAMETEAM12", 1_000_000).unwrap();
        assert_ne!(ja, jb);
        for (jwt, public) in [(&ja, &pub_a), (&jb, &pub_b)] {
            let (input, sig) = jwt.rsplit_once('.').unwrap();
            let sig = URL_SAFE_NO_PAD.decode(sig).unwrap();
            UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
                .verify(input.as_bytes(), &sig)
                .unwrap();
        }
    }

    fn msg() -> Message {
        Message {
            key: "question:204".into(),
            kind: "needs_you".into(),
            title: "MASON: Web API 204".into(),
            body: "Web API 204 needs a decision.".into(),
            reminder: false,
            at_ms: 1,
        }
    }

    fn apns(rec: Arc<Recorded>, pem: &str) -> Apns {
        Apns {
            key_pem: pem.into(),
            key_id: "ABC123DEFG".into(),
            team_id: "TEAM123456".into(),
            topic: "com.example.mason".into(),
            endpoint: Some("http://127.0.0.1:1".into()),
            http: rec,
        }
    }

    fn script(status: u16, body: &str) -> Arc<Recorded> {
        Arc::new(
            Recorded::from_json(&format!(
                r#"[{{"method":"POST","url":"/3/device/","repeat":true,"response":{{"status":{status},"body":{}}}}}]"#,
                serde_json::json!(body)
            ))
            .unwrap(),
        )
    }

    #[test]
    fn the_jwt_has_the_header_and_claims_apple_wants_and_its_signature_verifies() {
        let (pem, public) = key();
        let jwt = make_jwt(&pem, "ABC123DEFG", "TEAM123456", 1_700_000_000_123).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let json = |s: &str| -> serde_json::Value {
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(s).unwrap()).unwrap()
        };
        assert_eq!(
            json(parts[0]),
            serde_json::json!({"alg": "ES256", "kid": "ABC123DEFG"})
        );
        assert_eq!(
            json(parts[1]),
            serde_json::json!({"iss": "TEAM123456", "iat": 1_700_000_000u64})
        );
        let sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        assert_eq!(sig.len(), 64, "raw r||s, not DER");
        let signed = format!("{}.{}", parts[0], parts[1]);
        let pk = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public);
        pk.verify(signed.as_bytes(), &sig).expect("signature");
        assert!(pk.verify(b"other", &sig).is_err());
    }

    #[test]
    fn a_bad_key_file_is_named_and_a_token_is_reused_until_it_is_old() {
        assert!(make_jwt("hello", "k", "t", 0).unwrap_err().contains("PEM"));
        assert!(make_jwt(
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----",
            "k",
            "t",
            0
        )
        .is_err());
        let (pem, _) = key();
        let a = cached_jwt(&pem, "cache-kid", "T", 1_000_000).unwrap();
        let b = cached_jwt(&pem, "cache-kid", "T", 1_000_000 + JWT_TTL_MS - 1).unwrap();
        assert_eq!(a, b);
        let c = cached_jwt(&pem, "cache-kid", "T", 1_000_000 + JWT_TTL_MS + 5000).unwrap();
        assert_ne!(a, c);
        let d = cached_jwt(&pem, "other-kid", "T", 1_000_000 + JWT_TTL_MS + 5000).unwrap();
        assert_ne!(c, d);
    }

    #[test]
    fn the_payload_carries_the_alert_the_thread_and_what_the_tap_opens() {
        let ctx = Context {
            project: "mason".into(),
            task: Some(204),
        };
        let v: serde_json::Value = serde_json::from_str(&payload(&msg(), &ctx)).unwrap();
        assert_eq!(v["aps"]["alert"]["title"], "MASON: Web API 204");
        assert_eq!(v["aps"]["alert"]["body"], "Web API 204 needs a decision.");
        assert_eq!(v["aps"]["thread-id"], "mason");
        assert_eq!(v["mason"]["task"], 204);
        assert_eq!(v["mason"]["kind"], "needs_you");
    }

    #[test]
    fn it_posts_to_the_right_host_with_the_jwt_in_a_header_and_topic() {
        let (pem, public) = key();
        let rec = script(200, "");
        let a = apns(rec.clone(), &pem);
        let t = Target {
            token: "ab12cd".into(),
            env: "sandbox".into(),
        };
        a.send_to(&t, &msg(), &Context::default(), 5_000_000_000)
            .unwrap();
        let r = &rec.requests()[0];
        assert_eq!(r.url, "http://127.0.0.1:1/3/device/ab12cd");
        let h = |k: &str| {
            r.headers
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(h("apns-topic").as_deref(), Some("com.example.mason"));
        assert_eq!(h("apns-push-type").as_deref(), Some("alert"));
        let auth = h("authorization").unwrap();
        let jwt = auth.strip_prefix("bearer ").unwrap();
        let p: Vec<&str> = jwt.split('.').collect();
        let sig = URL_SAFE_NO_PAD.decode(p[2]).unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public)
            .verify(format!("{}.{}", p[0], p[1]).as_bytes(), &sig)
            .expect("the JWT on the wire verifies");
        // Without an override, each environment has its own Apple host.
        let mut real = apns(script(200, ""), &pem);
        real.endpoint = None;
        assert_eq!(
            real.url(&t).unwrap(),
            "https://api.sandbox.push.apple.com/3/device/ab12cd"
        );
        let prod = Target {
            env: "production".into(),
            ..t.clone()
        };
        assert_eq!(
            real.url(&prod).unwrap(),
            "https://api.push.apple.com/3/device/ab12cd"
        );
        let bad = Target {
            token: "../x".into(),
            ..t
        };
        assert!(real.url(&bad).is_err());
    }

    #[test]
    fn the_curl_config_carries_the_jwt_and_nothing_else_does() {
        let (pem, _) = key();
        let rec = script(200, "");
        let a = apns(rec.clone(), &pem);
        let t = Target {
            token: "ab12cd".into(),
            env: "sandbox".into(),
        };
        a.send_to(&t, &msg(), &Context::default(), 9_000_000_000)
            .unwrap();
        let req = rec.requests()[0].clone();
        let jwt = req
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap()
            .1
            .clone();
        let jwt = jwt.strip_prefix("bearer ").unwrap().to_string();
        let cfg = Curl {
            timeout_s: 5,
            http2: true,
        }
        .config(&req)
        .unwrap();
        assert!(cfg.contains(&jwt), "the config on stdin has the JWT");
        assert!(cfg.lines().any(|l| l == "http2"));
        // The command line is fixed text: no field of the request is on it.
        let args = ["-sS", "--max-time", "5", "-D", "<file>", "-K", "-"];
        assert!(args.iter().all(|a| !a.contains(&jwt)));
        // And nothing that holds the JWT is an error text.
        let bad = apns(script(403, r#"{"reason":"InvalidProviderToken"}"#), &pem);
        let e = bad
            .send_to(&t, &msg(), &Context::default(), 9_000_000_000)
            .unwrap_err();
        assert!(!format!("{e:?}").contains(&jwt));
    }

    #[test]
    fn apples_answers_are_told_apart() {
        let (pem, _) = key();
        let t = Target {
            token: "ab12cd".into(),
            env: "sandbox".into(),
        };
        let go = |status, body: &str| {
            apns(script(status, body), &pem).send_to(&t, &msg(), &Context::default(), 7_000_000_000)
        };
        assert!(go(200, "").is_ok());
        assert!(matches!(
            go(410, r#"{"reason":"Unregistered"}"#),
            Err(PushError::Gone(_))
        ));
        assert!(matches!(go(410, ""), Err(PushError::Gone(_))));
        assert!(matches!(
            go(400, r#"{"reason":"BadDeviceToken"}"#),
            Err(PushError::Gone(_))
        ));
        assert!(matches!(
            go(400, r#"{"reason":"PayloadEmpty"}"#),
            Err(PushError::Send(SendError::Permanent(_)))
        ));
        assert!(matches!(
            go(429, r#"{"reason":"TooManyRequests"}"#),
            Err(PushError::Send(SendError::Transient(_)))
        ));
        assert!(matches!(
            go(503, ""),
            Err(PushError::Send(SendError::Transient(_)))
        ));
    }
}
