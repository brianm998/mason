// SPDX-License-Identifier: GPL-3.0-or-later

//! The three channels that leave the machine: email over SMTP, ntfy push, and a generic webhook.
//! Each speaks through a seam ([`MailTransport`], [`crate::http::Http`]) so tests replay recorded
//! answers; production uses the system's `curl` with the credentials on its standard input,
//! never on a command line and never in an error.

use crate::http::{Http, Request};
use crate::netguard;
use crate::notify_kit::{one_line, Message, Notifier, SendError};
use crate::webhook;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

// ---- ntfy ------------------------------------------------------------------------------

/// Push through an ntfy server: `POST <server>/<topic>`, the sentence as the body, `Title` and
/// `Priority` as headers, a bearer token if the topic is protected.
pub struct Ntfy {
    pub server: String,
    pub topic: String,
    pub token: Option<String>,
    pub http: Arc<dyn Http>,
}

impl Notifier for Ntfy {
    fn name(&self) -> &'static str {
        "ntfy"
    }
    fn send(&self, m: &Message) -> Result<(), SendError> {
        let url = format!(
            "{}/{}",
            self.server.trim_end_matches('/'),
            self.topic.trim_matches('/')
        );
        let urgent =
            !m.reminder && matches!(m.kind.as_str(), "failed" | "daemon_down" | "credentials");
        let mut r = Request::post(&url, &m.body)
            .with("Title", &one_line(&m.title))
            .with("Priority", if urgent { "high" } else { "default" })
            .with("Tags", if m.reminder { "bell" } else { "warning" });
        if let Some(t) = &self.token {
            r = r.with("Authorization", &format!("Bearer {t}"));
        }
        classify(self.http.send(&r), &self.token)
    }
}

fn classify(
    r: Result<crate::http::Response, crate::http::Transport>,
    secret: &Option<String>,
) -> Result<(), SendError> {
    let scrub = |s: String| match secret {
        Some(t) if !t.is_empty() => s.replace(t.as_str(), "***"),
        _ => s,
    };
    match r {
        Err(t) => Err(SendError::Transient(scrub(t.0))),
        Ok(r) if (200..300).contains(&r.status) => Ok(()),
        Ok(r) if r.status == 429 || r.status == 408 || r.status >= 500 => Err(
            SendError::Transient(format!("the server answered {}", r.status)),
        ),
        Ok(r) => Err(SendError::Permanent(format!(
            "the server answered {}",
            r.status
        ))),
    }
}

// ---- webhook ---------------------------------------------------------------------------

/// The body a webhook receives.
pub fn webhook_body(m: &Message) -> String {
    serde_json::json!({
        "key": m.key, "kind": m.kind, "title": m.title, "text": m.body,
        "reminder": m.reminder, "since_ms": m.at_ms,
    })
    .to_string()
}

/// A generic webhook: JSON in the body, signed as `X-Mason-Signature: sha256=<hmac>` over the raw
/// body with the shared secret, so the receiver can check who sent it.
pub struct Webhook {
    pub url: String,
    pub secret: Option<String>,
    pub http: Arc<dyn Http>,
}

impl Notifier for Webhook {
    fn name(&self) -> &'static str {
        "webhook"
    }
    fn send(&self, m: &Message) -> Result<(), SendError> {
        let body = webhook_body(m);
        let mut r = Request::post(&self.url, &body).with("Content-Type", "application/json");
        if let Some(s) = &self.secret {
            let sig = webhook::hex(&webhook::hmac_sha256(s.as_bytes(), body.as_bytes()));
            r = r.with("X-Mason-Signature", &format!("sha256={sig}"));
        }
        classify(self.http.send(&r), &self.secret)
    }
}

// ---- email -----------------------------------------------------------------------------

/// How the message gets to the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailRequest {
    /// `smtp://host:port` or `smtps://host:port`.
    pub url: String,
    pub from: String,
    pub to: Vec<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    /// Refuse to continue unless the connection is encrypted (STARTTLS on `smtp://`).
    pub require_tls: bool,
    /// The whole message, headers and body.
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailError {
    /// The server refused us (login, a recipient): waiting will not help.
    Refused(String),
    /// No answer, or it went wrong on the way.
    Failed(String),
}

pub trait MailTransport: Send + Sync {
    fn deliver(&self, r: &MailRequest) -> Result<(), MailError>;
}

pub struct Email {
    pub url: String,
    pub from: String,
    pub to: Vec<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub require_tls: bool,
    pub transport: Arc<dyn MailTransport>,
}

/// The RFC 5322 text for a message. Header values are held to one line so no field can inject another.
pub fn mail_text(from: &str, to: &[String], m: &Message) -> String {
    let subject = format!(
        "{}{}",
        if m.reminder { "Reminder: " } else { "" },
        one_line(&m.title)
    );
    let mut s = format!(
        "From: {}\r\nTo: {}\r\nSubject: {}\r\nContent-Type: text/plain; charset=utf-8\r\nX-Mason-Alert: {}\r\n\r\n",
        one_line(from),
        to.iter().map(|t| one_line(t)).collect::<Vec<_>>().join(", "),
        subject,
        one_line(&m.key),
    );
    for line in m.body.lines() {
        // (curl dot-stuffs the upload, so a line that is a lone dot is safe.)
        s.push_str(&line.replace('\0', ""));
        s.push_str("\r\n");
    }
    s
}

impl Notifier for Email {
    fn name(&self) -> &'static str {
        "email"
    }
    fn send(&self, m: &Message) -> Result<(), SendError> {
        let scrub = |s: String| {
            let mut s = s;
            for secret in [&self.password, &self.user].into_iter().flatten() {
                if !secret.is_empty() {
                    s = s.replace(secret.as_str(), "***");
                }
            }
            s
        };
        let req = MailRequest {
            url: self.url.clone(),
            from: one_line(&self.from),
            to: self.to.iter().map(|t| one_line(t)).collect(),
            user: self.user.clone(),
            password: self.password.clone(),
            require_tls: self.require_tls,
            message: mail_text(&self.from, &self.to, m),
        };
        self.transport.deliver(&req).map_err(|e| match e {
            MailError::Refused(s) => SendError::Permanent(scrub(s)),
            MailError::Failed(s) => SendError::Transient(scrub(s)),
        })
    }
}

/// `curl smtp://...`, with the settings (and the password) on stdin as a config file and the
/// message in a private temporary file that holds no secret.
pub struct CurlMail {
    pub timeout_s: u32,
}

impl Default for CurlMail {
    fn default() -> Self {
        CurlMail { timeout_s: 30 }
    }
}

fn q(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The curl config for a request (kept apart so a test can check what is, and is not, in it).
pub fn curl_config(r: &MailRequest, message_file: &str) -> Result<String, String> {
    for v in std::iter::once(&r.url)
        .chain(std::iter::once(&r.from))
        .chain(r.to.iter())
        .chain(r.user.iter())
        .chain(r.password.iter())
    {
        if v.contains(['\n', '\r', '"']) {
            return Err("a mail setting contains a newline or a quote".into());
        }
    }
    let mut c = format!(
        "url = \"{}\"\nmail-from = \"<{}>\"\nupload-file = \"{}\"\nsilent\nshow-error\n",
        q(&r.url),
        q(&r.from),
        q(message_file)
    );
    for t in &r.to {
        c.push_str(&format!("mail-rcpt = \"<{}>\"\n", q(t)));
    }
    if let Some(u) = &r.user {
        c.push_str(&format!(
            "user = \"{}:{}\"\n",
            q(u),
            q(r.password.as_deref().unwrap_or(""))
        ));
    }
    if r.require_tls {
        c.push_str("ssl-reqd\n");
    }
    Ok(c)
}

static MAIL_SEQ: Mutex<u64> = Mutex::new(0);

impl MailTransport for CurlMail {
    fn deliver(&self, r: &MailRequest) -> Result<(), MailError> {
        netguard::check(&r.url).map_err(|e| MailError::Failed(e.to_string()))?;
        let n = {
            let mut g = MAIL_SEQ.lock().unwrap_or_else(|e| e.into_inner());
            *g += 1;
            *g
        };
        let file = std::env::temp_dir().join(format!("mason-mail-{}-{n}", std::process::id()));
        write_private(&file, r.message.as_bytes()).map_err(|e| MailError::Failed(e.to_string()))?;
        let result = (|| {
            let cfg = curl_config(r, &file.display().to_string()).map_err(MailError::Failed)?;
            let mut child = Command::new("curl")
                .args(["--max-time", &self.timeout_s.to_string(), "-K", "-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| MailError::Failed(format!("cannot run curl: {e}")))?;
            child
                .stdin
                .take()
                .ok_or_else(|| MailError::Failed("no stdin".into()))?
                .write_all(cfg.as_bytes())
                .map_err(|e| MailError::Failed(e.to_string()))?;
            let out = child
                .wait_with_output()
                .map_err(|e| MailError::Failed(e.to_string()))?;
            if out.status.success() {
                return Ok(());
            }
            let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
            // 67: login denied; 69-ish and 55 with a 5xx are refusals; the rest may pass.
            let refused = out.status.code() == Some(67)
                || text.contains("535")
                || text.to_ascii_lowercase().contains("authentication")
                || text.contains("550")
                || text.contains("553");
            Err(if refused {
                MailError::Refused(text)
            } else {
                MailError::Failed(text)
            })
        })();
        let _ = std::fs::remove_file(&file);
        result
    }
}

fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Recorded;
    use crate::notify_kit::{conformance, sample, Outcome};

    fn script(o: Outcome, url: &str, method: &str) -> Arc<Recorded> {
        let entry = match o {
            Outcome::Accept => format!(
                r#"[{{"method":"{method}","url":"{url}","repeat":true,"response":{{"status":200}}}}]"#
            ),
            Outcome::ServerError => format!(
                r#"[{{"method":"{method}","url":"{url}","repeat":true,"response":{{"status":503}}}}]"#
            ),
            Outcome::Rejected => format!(
                r#"[{{"method":"{method}","url":"{url}","repeat":true,"response":{{"status":403}}}}]"#
            ),
            Outcome::Unreachable => format!(
                r#"[{{"method":"{method}","url":"{url}","repeat":true,"transport_error":"connection refused by hunter2"}}]"#
            ),
        };
        Arc::new(Recorded::from_json(&entry).expect("script"))
    }

    #[test]
    fn ntfy_passes_conformance_and_sends_what_ntfy_expects() {
        let secret = "hunter2";
        let make = |o| -> Box<dyn Notifier> {
            Box::new(Ntfy {
                server: "http://127.0.0.1:1".into(),
                topic: "mason".into(),
                token: Some(secret.into()),
                http: script(o, "/mason", "POST"),
            })
        };
        conformance(&make, secret).expect("conformance");
        let rec = script(Outcome::Accept, "/mason", "POST");
        let n = Ntfy {
            server: "http://127.0.0.1:1/".into(),
            topic: "mason".into(),
            token: Some(secret.into()),
            http: rec.clone(),
        };
        n.send(&sample()).unwrap();
        let r = &rec.requests()[0];
        assert_eq!(r.url, "http://127.0.0.1:1/mason");
        assert!(r
            .body
            .as_deref()
            .unwrap()
            .starts_with("Web API 204 needs a decision"));
        let h = |k: &str| {
            r.headers
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            h("Title").as_deref(),
            Some("Web API 204 needs a decision Injected: header")
        );
        assert_eq!(h("Authorization").as_deref(), Some("Bearer hunter2"));
        assert!(!r.headers.iter().any(|(_, v)| v.contains('\n')));
    }

    #[test]
    fn webhook_passes_conformance_and_its_signature_verifies() {
        let secret = "hunter2";
        let make = |o| -> Box<dyn Notifier> {
            Box::new(Webhook {
                url: "http://127.0.0.1:1/hook".into(),
                secret: Some(secret.into()),
                http: script(o, "/hook", "POST"),
            })
        };
        conformance(&make, secret).expect("conformance");
        let rec = script(Outcome::Accept, "/hook", "POST");
        Webhook {
            url: "http://127.0.0.1:1/hook".into(),
            secret: Some(secret.into()),
            http: rec.clone(),
        }
        .send(&sample())
        .unwrap();
        let r = &rec.requests()[0];
        let sig = r
            .headers
            .iter()
            .find(|(k, _)| k == "X-Mason-Signature")
            .unwrap()
            .1
            .clone();
        let body = r.body.clone().unwrap();
        assert!(webhook::verify(secret, body.as_bytes(), &sig));
        assert!(!webhook::verify("wrong", body.as_bytes(), &sig));
        assert!(!webhook::verify(secret, b"{}", &sig));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["key"], "question:204");
        assert_eq!(v["reminder"], false);
    }

    struct FakeMail {
        outcome: Outcome,
        sent: Mutex<Vec<MailRequest>>,
    }
    impl MailTransport for FakeMail {
        fn deliver(&self, r: &MailRequest) -> Result<(), MailError> {
            self.sent.lock().unwrap().push(r.clone());
            match self.outcome {
                Outcome::Accept => Ok(()),
                Outcome::ServerError => Err(MailError::Failed("451 try later for hunter2".into())),
                Outcome::Rejected => Err(MailError::Refused("535 login hunter2 denied".into())),
                Outcome::Unreachable => Err(MailError::Failed("could not connect".into())),
            }
        }
    }

    fn email(t: Arc<dyn MailTransport>) -> Email {
        Email {
            url: "smtp://127.0.0.1:2525".into(),
            from: "mason@example.test".into(),
            to: vec!["me@example.test".into()],
            user: Some("mason".into()),
            password: Some("hunter2".into()),
            require_tls: false,
            transport: t,
        }
    }

    #[test]
    fn email_passes_conformance_and_never_leaks_the_password() {
        let make = |o| -> Box<dyn Notifier> {
            Box::new(email(Arc::new(FakeMail {
                outcome: o,
                sent: Mutex::new(vec![]),
            })))
        };
        conformance(&make, "hunter2").expect("conformance");
    }

    #[test]
    fn the_message_cannot_inject_headers_and_the_password_stays_out_of_the_message() {
        let fake = Arc::new(FakeMail {
            outcome: Outcome::Accept,
            sent: Mutex::new(vec![]),
        });
        email(fake.clone()).send(&sample()).unwrap();
        let sent = fake.sent.lock().unwrap()[0].clone();
        let head = sent.message.split("\r\n\r\n").next().unwrap();
        assert!(head.contains("Subject: Web API 204 needs a decision Injected: header"));
        assert_eq!(
            head.lines().filter(|l| l.starts_with("Injected")).count(),
            0
        );
        assert!(!sent.message.contains("hunter2"));
        assert!(sent.message.ends_with("\r\n"));
    }

    #[test]
    fn the_curl_config_carries_the_password_and_nothing_else_does() {
        let r = MailRequest {
            url: "smtp://127.0.0.1:2525".into(),
            from: "a@b.test".into(),
            to: vec!["c@d.test".into()],
            user: Some("mason".into()),
            password: Some("hunter2".into()),
            require_tls: true,
            message: "x".into(),
        };
        let cfg = curl_config(&r, "/tmp/msg").unwrap();
        assert!(cfg.contains("user = \"mason:hunter2\""));
        assert!(cfg.contains("ssl-reqd") && cfg.contains("mail-rcpt = \"<c@d.test>\""));
        let mut bad = r.clone();
        bad.to = vec!["x@y\nRCPT".into()];
        assert!(curl_config(&bad, "/tmp/msg").is_err());
    }
}
