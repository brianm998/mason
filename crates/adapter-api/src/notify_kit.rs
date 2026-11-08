// SPDX-License-Identifier: GPL-3.0-or-later

//! What a notification channel is (Part 19) and the conformance kit every channel passes.
//! A [`Notifier`] sends one [`Message`] and says how it failed: a failure that may pass
//! ([`SendError::Transient`]: no route, a busy server) is tried again by the dispatcher, a
//! refusal that will not ([`SendError::Permanent`]: bad credentials, an unknown topic) is not,
//! and neither carries a secret in its text.

/// What to tell the owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// The alert's key, so a receiver can de-duplicate.
    pub key: String,
    /// The condition (`needs_you`, `failed`, ...).
    pub kind: String,
    pub title: String,
    pub body: String,
    /// The one reminder, not the first notice.
    pub reminder: bool,
    /// When the alert began, in ms.
    pub at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    Transient(String),
    Permanent(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Transient(s) => write!(f, "{s} (may pass)"),
            SendError::Permanent(s) => write!(f, "{s} (will not pass by waiting)"),
        }
    }
}

pub trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;
    fn send(&self, m: &Message) -> Result<(), SendError>;
}

/// How a conformance double should answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Accept,
    /// The server is failing (5xx, a rate limit, a mid-send error).
    ServerError,
    /// The server refuses us (bad credentials, no such topic).
    Rejected,
    /// Nobody answered.
    Unreachable,
}

/// A message every conformance run sends: awkward on purpose.
pub fn sample() -> Message {
    Message {
        key: "question:204".into(),
        kind: "needs_you".into(),
        title: "Web API 204 needs a decision\r\nInjected: header".into(),
        body: "Web API 204 needs a decision on token format.\nSecond line with \"quotes\" and a \\ backslash.".into(),
        reminder: false,
        at_ms: 1_700_000_000_000,
    }
}

/// The rules every channel keeps. `make` builds the channel against a double that answers as
/// told; `secret` is the credential the channel was configured with.
pub fn conformance(
    make: &dyn Fn(Outcome) -> Box<dyn Notifier>,
    secret: &str,
) -> Result<(), String> {
    let m = sample();
    let ok = make(Outcome::Accept);
    ok.send(&m)
        .map_err(|e| format!("{}: an accepted message failed: {e}", ok.name()))?;
    let mut reminder = m.clone();
    reminder.reminder = true;
    ok.send(&reminder)
        .map_err(|e| format!("{}: a reminder failed: {e}", ok.name()))?;
    for (outcome, want_transient) in [
        (Outcome::ServerError, true),
        (Outcome::Rejected, false),
        (Outcome::Unreachable, true),
    ] {
        let n = make(outcome);
        match n.send(&m) {
            Ok(()) => return Err(format!("{}: {outcome:?} reported success", n.name())),
            Err(e) => {
                let transient = matches!(e, SendError::Transient(_));
                if transient != want_transient {
                    return Err(format!(
                        "{}: {outcome:?} was classified as {e:?}, expected {}",
                        n.name(),
                        if want_transient {
                            "transient"
                        } else {
                            "permanent"
                        }
                    ));
                }
                if !secret.is_empty() && e.to_string().contains(secret) {
                    return Err(format!(
                        "{}: the error text leaks the secret: {e}",
                        n.name()
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Header and subject text on one line.
pub fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
