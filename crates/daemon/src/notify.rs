// SPDX-License-Identifier: GPL-3.0-or-later

//! Notifications (Part 19): the dispatcher that turns live alerts into deliveries, the queue of
//! announcements for the one device that speaks, and the daemon's own speech when no monitor is
//! around. The decisions are in `mason_core::notify`; this does the sending and the remembering.

use crate::daemon::{now_ms, Daemon};
use mason_adapter_api::http::Curl;
use mason_adapter_api::notify_channels::{CurlMail, Email, Ntfy, Webhook};
use mason_adapter_api::notify_kit::{Message, Notifier, SendError};
use mason_core::notify::{
    self as n, AlertState, Attempt, Channel, Claim, Delivery, QuietHours, Routing, Rule,
};
use mason_core::{State, TaskId};
use mason_store::{AlertRow, AnnouncementRow, DeliveryRow};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::process::Command;
use std::sync::{Arc, Mutex};
use toml_edit::DocumentMut;

/// What the daemon remembers between ticks.
#[derive(Default)]
pub struct Runtime {
    /// Each unmuted monitor's latest claim to speak.
    claims: Mutex<Vec<Claim>>,
    speaker: Mutex<Option<String>>,
    /// The last time any monitor, muted or not, was heard from.
    last_claim: Mutex<Option<u64>>,
    landed_seen: Mutex<Option<BTreeSet<u64>>>,
    ticking: Mutex<()>,
    /// The speech thread is at work; the next tick leaves the announcements for it.
    speaking: Arc<std::sync::atomic::AtomicBool>,
    /// Held while the alert table is brought up to date.
    pub(crate) syncing: Mutex<()>,
    /// Test and exit-script switch (`POST /debug/store-writes`): the alert and delivery writes
    /// behave as if the store refused them, as a full disk does.
    refuse_writes: std::sync::atomic::AtomicBool,
    /// What the store could not take, kept here until it can.
    mem: Mutex<Mem>,
}

/// The alerts and deliveries held in memory while the store cannot be written.
#[derive(Default)]
struct Mem {
    /// The last write of the alert table failed. `alerts` is then the whole live list.
    degraded: bool,
    alerts: Vec<AlertRow>,
    /// Deliveries made but not recorded.
    sent: Vec<DeliveryRow>,
}

impl Runtime {
    fn mem(&self) -> std::sync::MutexGuard<'_, Mem> {
        self.mem.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Make the alert and delivery writes fail (or work again). For tests and `--debug`.
pub fn refuse_writes(d: &Daemon, on: bool) {
    d.notify
        .refuse_writes
        .store(on, std::sync::atomic::Ordering::SeqCst);
}

fn refused(d: &Daemon) -> bool {
    d.notify
        .refuse_writes
        .load(std::sync::atomic::Ordering::SeqCst)
}

/// True while the alert table could not be written and the list is being served from memory.
pub fn store_unwritable(d: &Daemon) -> bool {
    d.notify.mem().degraded
}

/// The note shown beside the list while it is served from memory.
pub const UNWRITABLE_NOTE: &str = "MASON cannot write to its store (the disk may be full): this list is held in memory and will be saved when writes work again";

/// Deliveries made and not yet recorded in the store.
pub fn unrecorded_deliveries(d: &Daemon) -> usize {
    d.notify.mem().sent.len()
}

/// Write the alert table, or, if that fails, keep `now` in memory. Returns (raised, cleared).
pub(crate) fn sync_table(
    d: &Daemon,
    now: &[(String, String, String)],
) -> (Vec<String>, Vec<String>) {
    let at = now_ms();
    let held: Vec<(String, u64)> = d
        .notify
        .mem()
        .alerts
        .iter()
        .map(|a| (a.key.clone(), a.since))
        .collect();
    let r = if refused(d) {
        Err(mason_store::StoreError::WriterGone)
    } else {
        d.store.sync_alerts_since(now, at, &held)
    };
    let mut m = d.notify.mem();
    match r {
        Ok(v) => {
            m.degraded = false;
            m.alerts.clear();
            v
        }
        Err(e) => {
            if !m.degraded {
                tracing::warn!("cannot write the alert table: {e}; serving alerts from memory");
            }
            let table = d.store.alerts(true).unwrap_or_default();
            let old = std::mem::take(&mut m.alerts);
            m.alerts = now
                .iter()
                .map(|(key, kind, text)| {
                    let prior = old
                        .iter()
                        .chain(table.iter())
                        .find(|a| a.key == *key)
                        .cloned();
                    AlertRow {
                        key: key.clone(),
                        kind: kind.clone(),
                        text: text.clone(),
                        since: prior.as_ref().map_or(at, |a| a.since),
                        cleared_at: None,
                        acked_at: prior.as_ref().and_then(|a| a.acked_at),
                        reminded_at: prior.as_ref().and_then(|a| a.reminded_at),
                    }
                })
                .collect();
            m.degraded = true;
            (vec![], vec![])
        }
    }
}

/// The alerts to show and act on: the table, and while it cannot be written, what is true now
/// from memory in place of its live rows. Cleared rows come from the table only.
pub fn live_alerts(
    d: &Daemon,
    active_only: bool,
) -> Result<Vec<AlertRow>, mason_store::StoreError> {
    let mut rows = d.store.alerts(active_only)?;
    let m = d.notify.mem();
    if m.degraded {
        rows.retain(|r| r.cleared_at.is_some());
        rows.extend(m.alerts.iter().cloned());
        rows.sort_by(|a, b| (a.since, &a.key).cmp(&(b.since, &b.key)));
    }
    Ok(rows)
}

/// Put deliveries held in memory into the store, if it will take them.
fn flush_sent(d: &Daemon) {
    if refused(d) {
        return;
    }
    let mut m = d.notify.mem();
    let held = std::mem::take(&mut m.sent);
    for row in held {
        if d.store.add_delivery(&row).is_err() {
            m.sent.push(row);
        }
    }
}

// ---- settings --------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct EmailCfg {
    pub url: String,
    pub from: String,
    pub to: Vec<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub require_tls: bool,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub routing: Routing,
    pub daemon_speech: bool,
    pub speech_command: Vec<String>,
    pub preferred: Option<String>,
    /// Announcements older than this are dropped rather than spoken late.
    pub stale_ms: u64,
    /// How long the whole pipeline may go without an event or output before `stalled` is raised
    /// (`[stall] after`); 0 turns it off.
    pub stall_ms: u64,
    pub email: Option<EmailCfg>,
    pub ntfy: Option<(String, String, Option<String>)>,
    pub webhook: Option<(String, Option<String>)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            routing: Routing::defaults(),
            daemon_speech: false,
            speech_command: vec![],
            preferred: None,
            stale_ms: 10 * n::MINUTE_MS,
            stall_ms: 30 * n::MINUTE_MS,
            email: None,
            ntfy: None,
            webhook: None,
        }
    }
}

fn text(t: &dyn toml_edit::TableLike, k: &str) -> Option<String> {
    t.get(k).and_then(|v| v.as_str()).map(String::from)
}

/// A span written as a string (`"10m"`) or a number of seconds.
fn span(v: &toml_edit::Item, what: &str) -> Result<u64, String> {
    if let Some(s) = v.as_str() {
        n::parse_span(s).ok_or_else(|| format!("{what}: `{s}` is not a time like 10m, 1h or 30s"))
    } else if let Some(i) = v.as_integer() {
        u64::try_from(i)
            .ok()
            .and_then(|s| s.checked_mul(1000))
            .ok_or_else(|| format!("{what}: a number of seconds, not negative"))
    } else {
        Err(format!("{what}: a time like 10m, 1h or 30s"))
    }
}

/// A secret given in the file, in a file of its own, or in the environment.
fn secret(t: &dyn toml_edit::TableLike, k: &str) -> Result<Option<String>, String> {
    if let Some(v) = text(t, k) {
        return Ok(Some(v));
    }
    if let Some(var) = text(t, &format!("{k}_env")) {
        return Ok(std::env::var(var).ok());
    }
    if let Some(path) = text(t, &format!("{k}_file")) {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("{k}_file {path}: {e}"))?;
        return Ok(Some(raw.trim_end_matches(['\n', '\r']).to_string()));
    }
    Ok(None)
}

/// Read `notify.toml`. Missing means the defaults.
pub fn parse(src: &str) -> Result<Settings, String> {
    let doc: DocumentMut = src.parse().map_err(|e| format!("notify.toml: {e}"))?;
    let mut s = Settings::default();
    for (section, item) in doc.iter() {
        let Some(t) = item.as_table_like() else {
            return Err(format!("notify.toml: `{section}` should be a table"));
        };
        match section {
            "routing" => {
                for (kind, rules) in t.iter() {
                    let rt = rules.as_table_like().ok_or_else(|| {
                        format!("routing.{kind} should be a table of channel = delay")
                    })?;
                    let mut out = vec![];
                    for (ch, delay) in rt.iter() {
                        let channel = Channel::parse(ch).ok_or_else(|| {
                            format!("routing.{kind}: unknown channel `{ch}` (monitor, sound, speech, push, email, webhook)")
                        })?;
                        out.push(Rule {
                            channel,
                            after_ms: span(delay, &format!("routing.{kind}.{ch}"))?,
                        });
                    }
                    if !out.iter().any(|r| r.channel == Channel::Monitor) {
                        out.insert(
                            0,
                            Rule {
                                channel: Channel::Monitor,
                                after_ms: 0,
                            },
                        );
                    }
                    s.routing.set(kind, out);
                }
            }
            "quiet" => {
                for (ch, v) in t.iter() {
                    let channel = Channel::parse(ch)
                        .ok_or_else(|| format!("quiet: unknown channel `{ch}`"))?;
                    let w = v.as_str().unwrap_or_default();
                    let (a, b) = w
                        .split_once('-')
                        .and_then(|(a, b)| Some((n::parse_clock(a)?, n::parse_clock(b)?)))
                        .ok_or_else(|| format!("quiet.{ch}: a window like \"22:00-07:00\""))?;
                    s.routing
                        .quiet
                        .insert(channel, QuietHours { start: a, end: b });
                }
            }
            "reminder" => {
                if let Some(v) = t.get("after") {
                    s.routing.reminder_ms = span(v, "reminder.after")?;
                }
                if let Some(v) = t.get("retry") {
                    s.routing.retry_ms = span(v, "reminder.retry")?;
                }
                if let Some(v) = t.get("attempts").and_then(|v| v.as_integer()) {
                    s.routing.max_attempts = u32::try_from(v).unwrap_or(3).clamp(1, 10);
                }
            }
            "speech" => {
                s.daemon_speech = t.get("daemon").and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(c) = t.get("command") {
                    s.speech_command = match (c.as_str(), c.as_array()) {
                        (Some(one), _) => vec![one.to_string()],
                        (_, Some(a)) => a
                            .iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect(),
                        _ => {
                            return Err(
                                "speech.command: a program, or a list of program and arguments"
                                    .into(),
                            )
                        }
                    };
                }
                s.preferred = text(t, "preferred_client");
                if let Some(v) = t.get("stale") {
                    s.stale_ms = span(v, "speech.stale")?;
                }
            }
            "stall" => {
                if let Some(v) = t.get("after") {
                    s.stall_ms = span(v, "stall.after")?;
                }
            }
            "email" => {
                let need = |k: &str| text(t, k).ok_or_else(|| format!("email: `{k}` is required"));
                let to: Vec<String> = match t.get("to") {
                    Some(v) if v.is_str() => vec![v.as_str().unwrap_or_default().to_string()],
                    Some(v) => v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    None => vec![],
                };
                if to.is_empty() {
                    return Err("email: `to` is required".into());
                }
                let tls = text(t, "tls").unwrap_or_else(|| "starttls".into());
                let url = need("server")?;
                let url = if url.contains("://") {
                    url
                } else if tls == "tls" {
                    format!("smtps://{url}")
                } else {
                    format!("smtp://{url}")
                };
                s.email = Some(EmailCfg {
                    url,
                    from: need("from")?,
                    to,
                    user: text(t, "user"),
                    password: secret(t, "password")?,
                    require_tls: tls == "starttls",
                });
            }
            "ntfy" => {
                s.ntfy = Some((
                    text(t, "server").unwrap_or_else(|| "https://ntfy.sh".into()),
                    text(t, "topic").ok_or("ntfy: `topic` is required")?,
                    secret(t, "token")?,
                ));
            }
            "webhook" => {
                s.webhook = Some((
                    text(t, "url").ok_or("webhook: `url` is required")?,
                    secret(t, "secret")?,
                ));
            }
            other => return Err(format!("notify.toml: unknown section `{other}`")),
        }
    }
    Ok(s)
}

pub fn load(d: &Daemon) -> Result<Settings, String> {
    match std::fs::read_to_string(d.cfg.config_dir.join("notify.toml")) {
        Ok(t) => parse(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("notify.toml: {e}")),
    }
}

/// The channel for a name, if it is configured.
pub fn notifier(s: &Settings, c: Channel) -> Option<Box<dyn Notifier>> {
    let http: Arc<dyn mason_adapter_api::http::Http> = Arc::new(Curl::default());
    match c {
        Channel::Email => s.email.as_ref().map(|e| {
            Box::new(Email {
                url: e.url.clone(),
                from: e.from.clone(),
                to: e.to.clone(),
                user: e.user.clone(),
                password: e.password.clone(),
                require_tls: e.require_tls,
                transport: Arc::new(CurlMail::default()),
            }) as Box<dyn Notifier>
        }),
        Channel::Push => s.ntfy.as_ref().map(|(server, topic, token)| {
            Box::new(Ntfy {
                server: server.clone(),
                topic: topic.clone(),
                token: token.clone(),
                http,
            }) as Box<dyn Notifier>
        }),
        Channel::Webhook => s.webhook.as_ref().map(|(url, secret)| {
            Box::new(Webhook {
                url: url.clone(),
                secret: secret.clone(),
                http,
            }) as Box<dyn Notifier>
        }),
        _ => None,
    }
}

// ---- time ------------------------------------------------------------------------------

/// Local minutes since midnight. `MASON_TEST_MINUTE_OF_DAY` stands in for the clock in tests.
pub fn minute_of_day() -> u32 {
    if let Some(m) = std::env::var("MASON_TEST_MINUTE_OF_DAY")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return m;
    }
    Command::new("date")
        .arg("+%H:%M")
        .output()
        .ok()
        .and_then(|o| n::parse_clock(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or(12 * 60)
}

// ---- what to say -----------------------------------------------------------------------

/// How to name the thing an alert is about, and the trouble, for the sentence.
fn subject_and_detail(d: &Daemon, key: &str, kind: &str, text: &str) -> (String, String) {
    let id = key
        .split_once(':')
        .and_then(|(p, rest)| {
            matches!(p, "question" | "failed" | "environment" | "landed").then_some(rest)
        })
        .and_then(|r| r.parse::<u64>().ok());
    if let (Some(id), Ok(w)) = (id, d.world()) {
        if let Some(t) = w.tasks.get(&TaskId(id)) {
            let subject = format!("{} {}", t.title, id);
            let detail = match kind {
                "needs_you" => crate::attention::card_of(d, t)
                    .map(|(c, _)| c.ask)
                    .unwrap_or_default(),
                "failed" => t.failure.clone().unwrap_or_default(),
                _ => String::new(),
            };
            return (subject, detail);
        }
    }
    ("MASON".into(), text.to_string())
}

// ---- the dispatcher --------------------------------------------------------------------

fn attempts(rows: &[DeliveryRow]) -> Vec<Attempt> {
    rows.iter()
        .filter_map(|r| {
            Some(Attempt {
                channel: Channel::parse(&r.channel)?,
                delivery: if r.delivery == "reminder" {
                    Delivery::Reminder
                } else {
                    Delivery::Initial
                },
                ok: r.ok,
                permanent: !r.ok && r.detail.starts_with("permanent:"),
                at_ms: r.at,
            })
        })
        .collect()
}

/// `alert:<key>:<since>:<delivery>:<flags>` (the key itself may hold colons) as
/// (key, since, delivery, flags); flags are two digits, sound then speech.
fn parse_alert_key(k: &str) -> Option<(String, u64, String, String)> {
    k.strip_prefix("alert:")?;
    let mut parts = k.rsplitn(4, ':');
    let flags = parts.next()?;
    let delivery = parts.next()?;
    let since = parts.next()?.parse().ok()?;
    let rest = parts.next()?.strip_prefix("alert:").unwrap_or_default();
    Some((rest.to_string(), since, delivery.into(), flags.into()))
}

/// Playing is delivery: record the sound and speech an alert's announcement carried, now that
/// something has taken it. `speech_only` is for the daemon, which has no way to play a sound.
fn record_played(d: &Daemon, rows: &[AnnouncementRow], speech_only: bool, now: u64) {
    for r in rows {
        let Some((key, since, delivery, flags)) = parse_alert_key(&r.key) else {
            continue;
        };
        let dl = if delivery == "reminder" {
            Delivery::Reminder
        } else {
            Delivery::Initial
        };
        let f = flags.as_bytes();
        for (c, on) in [
            (Channel::Sound, f.first() == Some(&b'1') && !speech_only),
            (Channel::Speech, f.get(1) == Some(&b'1')),
        ] {
            if on {
                record(d, &key, since, c, dl, Ok(()), now);
                if delivery == "catchup" {
                    // One announcement stood for the whole history of the alert.
                    record(d, &key, since, c, Delivery::Reminder, Ok(()), now);
                }
            }
        }
    }
}

/// An announcement about an alert is dropped if, before anyone heard it, the alert cleared or was
/// acknowledged: nobody wants to be told about something that has already gone.
fn prune(d: &Daemon, now: u64) {
    let pending = d.store.pending_announcements(200).unwrap_or_default();
    if pending.iter().all(|p| !p.key.starts_with("alert:")) {
        return;
    }
    let live: Vec<_> = live_alerts(d, true).unwrap_or_default();
    for p in pending.iter().filter(|p| p.key.starts_with("alert:")) {
        let (key, since, _, _) = parse_alert_key(&p.key).unwrap_or_default();
        let wanted = live
            .iter()
            .any(|a| a.key == key && a.since == since && a.acked_at.is_none());
        if !wanted {
            let _ = d.store.expire_announcement(p.id, now);
        }
    }
}

/// One pass over the live alerts: send what is due, queue what is to be said, speak for the
/// daemon if no monitor is around. Run after `sync_alerts`, so it sees what is true now.
pub fn tick(d: &Arc<Daemon>) {
    tick_at(d, now_ms());
}

/// `tick` at a given time in milliseconds (tests drive the clock through this).
pub fn tick_at(d: &Arc<Daemon>, now: u64) {
    let Ok(_one_at_a_time) = d.notify.ticking.try_lock() else {
        return;
    };
    let settings = match load(d) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("{e}; using the default routing");
            Settings::default()
        }
    };
<<<<<<< HEAD
    let now = now_ms();
    flush_sent(d);
=======
>>>>>>> origin/develop
    prune(d, now);
    let minute = if settings.routing.quiet.is_empty() {
        0
    } else {
        minute_of_day()
    };
    for a in live_alerts(d, true).unwrap_or_default() {
        let st = AlertState {
            since_ms: a.since,
            acked: a.acked_at.is_some(),
        };
        let mut rows = d.store.deliveries_for(&a.key, a.since).unwrap_or_default();
        rows.extend(
            d.notify
                .mem()
                .sent
                .iter()
                .filter(|r| r.alert == a.key && r.since == a.since)
                .cloned(),
        );
        let todo = n::due(
            &settings.routing,
            &a.kind,
            st,
            &attempts(&rows),
            now,
            minute,
        );
        if todo.is_empty() {
            continue;
        }
        let (subject, detail) = subject_and_detail(d, &a.key, &a.kind, &a.text);
        for delivery in [Delivery::Initial, Delivery::Reminder] {
            let these: Vec<Channel> = todo
                .iter()
                .filter(|x| x.1 == delivery)
                .map(|x| x.0)
                .collect();
            if these.is_empty() {
                continue;
            }
            let reminder = delivery == Delivery::Reminder;
            let line = n::sentence(&a.kind, &subject, &detail, reminder);
            let (sound, speech) = (
                these.contains(&Channel::Sound),
                these.contains(&Channel::Speech),
            );
            if sound || speech {
                let key = format!(
                    "alert:{}:{}:{}:{}{}",
                    a.key,
                    a.since,
                    delivery.name(),
                    u8::from(sound),
                    u8::from(speech)
                );
<<<<<<< HEAD
                // The queue lives in the store. If it cannot be written nothing is recorded as
                // said, and the same line is queued, once, when writes work again.
                let queued = !refused(d)
                    && d.store
                        .enqueue_announcement(&key, &a.kind, &line, sound, speech, now)
                        .is_ok();
                if queued {
                    for (c, on) in [(Channel::Sound, sound), (Channel::Speech, speech)] {
                        if on {
                            record(d, &a.key, a.since, c, delivery, Ok(()), now);
                        }
                    }
                }
=======
                // Queuing is not delivery: it is recorded when a monitor or the daemon plays it.
                let _ = d
                    .store
                    .enqueue_announcement(&key, &a.kind, &line, sound, speech, now);
>>>>>>> origin/develop
            }
            for c in these.into_iter().filter(|c| c.remote()) {
                let m = Message {
                    key: a.key.clone(),
                    kind: a.kind.clone(),
                    title: format!("MASON: {subject}"),
                    body: line.clone(),
                    reminder,
                    at_ms: a.since,
                };
                let r = match notifier(&settings, c) {
                    Some(nt) => nt.send(&m),
                    None => Err(SendError::Permanent(format!(
                        "{} is not configured in notify.toml",
                        c.name()
                    ))),
                };
                record(d, &a.key, a.since, c, delivery, r, now);
            }
            if reminder {
                let _ = d.store.mark_reminded(&a.key, now);
            }
        }
    }
    announce_landed(d, &settings, now);
    daemon_speech(d, &settings, now);
}

/// A monitor has claimed. An alert that is live, unacknowledged and never heard (its
/// announcement expired, or none was ever taken) gets one announcement now, however old it is:
/// the key makes it once per alert. Returns nothing; the announcement is taken by the caller.
fn catch_up(d: &Daemon, settings: &Settings, now: u64) {
    let stale_before = now.saturating_sub(settings.stale_ms);
    let pending = d.store.pending_announcements(200).unwrap_or_default();
    let minute = if settings.routing.quiet.is_empty() {
        0
    } else {
        minute_of_day()
    };
    for a in d.store.alerts(true).unwrap_or_default() {
        if a.acked_at.is_some() {
            continue;
        }
        let rows = d.store.deliveries_for(&a.key, a.since).unwrap_or_default();
        let heard = rows
            .iter()
            .any(|r| r.ok && matches!(r.channel.as_str(), "sound" | "speech"));
        if heard {
            continue;
        }
        // A fresh announcement for this run is already waiting: it will be handed out as is.
        let prefix = format!("alert:{}:{}:", a.key, a.since);
        if pending
            .iter()
            .any(|p| p.key.starts_with(&prefix) && p.created_at >= stale_before)
        {
            continue;
        }
        let audible = |c: Channel| {
            settings.routing.rules(&a.kind).iter().any(|r| {
                r.channel == c
                    && now >= a.since + r.after_ms
                    && !settings.routing.is_quiet(c, minute)
            })
        };
        let (sound, speech) = (audible(Channel::Sound), audible(Channel::Speech));
        if !(sound || speech) {
            continue;
        }
        let reminder = now >= a.since + settings.routing.reminder_ms;
        let (subject, detail) = subject_and_detail(d, &a.key, &a.kind, &a.text);
        let line = n::sentence(&a.kind, &subject, &detail, reminder);
        let key = format!(
            "alert:{}:{}:catchup:{}{}",
            a.key,
            a.since,
            u8::from(sound),
            u8::from(speech)
        );
        let _ = d
            .store
            .enqueue_announcement(&key, &a.kind, &line, sound, speech, now);
        if reminder {
            let _ = d.store.mark_reminded(&a.key, now);
        }
    }
}

fn record(
    d: &Daemon,
    key: &str,
    since: u64,
    c: Channel,
    dl: Delivery,
    r: Result<(), SendError>,
    now: u64,
) {
    let (ok, detail) = match r {
        Ok(()) => (true, String::new()),
        Err(SendError::Transient(s)) => (false, format!("transient: {s}")),
        Err(SendError::Permanent(s)) => (false, format!("permanent: {s}")),
    };
    if !ok {
        tracing::warn!(alert = key, channel = c.name(), "delivery failed: {detail}");
    }
    let row = DeliveryRow {
        alert: key.into(),
        since,
        channel: c.name().into(),
        delivery: dl.name().into(),
        ok,
        detail,
        at: now,
    };
    // Not recorded means it would be sent again next pass: keep it here until it can be.
    if refused(d) || d.store.add_delivery(&row).is_err() {
        d.notify.mem().sent.push(row);
    }
}

/// "Landed" is an event, not a condition that holds: a short spoken line, once per task, and
/// nothing for what had landed before the daemon started looking.
fn announce_landed(d: &Daemon, s: &Settings, now: u64) {
    let Ok(w) = d.world() else { return };
    let landed: BTreeSet<u64> = w
        .tasks
        .values()
        .filter(|t| t.landed.is_some() || t.state == State::Landed)
        .map(|t| t.id.0)
        .collect();
    let mut seen = d
        .notify
        .landed_seen
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(before) = seen.as_ref() {
        let speaks = s
            .routing
            .rules("landed")
            .iter()
            .any(|r| r.channel.audible());
        for id in landed.difference(before) {
            if let (true, Some(t)) = (speaks, w.tasks.get(&TaskId(*id))) {
                let line = n::sentence("landed", &format!("{} {}", t.title, id), "", false);
                let sound = s
                    .routing
                    .rules("landed")
                    .iter()
                    .any(|r| r.channel == Channel::Sound);
                let _ = d.store.enqueue_announcement(
                    &format!("landed:{id}"),
                    "landed",
                    &line,
                    sound,
                    true,
                    now,
                );
            }
        }
    }
    *seen = Some(landed);
}

/// With speech on and no monitor around, say what is waiting, one at a time, with the
/// configured command.
fn daemon_speech(d: &Daemon, s: &Settings, now: u64) {
    use std::sync::atomic::Ordering;
    let last = *d
        .notify
        .last_claim
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if s.speech_command.is_empty() || !n::daemon_should_speak(s.daemon_speech, last, now) {
        return;
    }
    // Still saying the last batch: leave the rest queued rather than wait for it.
    if d.notify.speaking.swap(true, Ordering::SeqCst) {
        return;
    }
    let rows = d
        .store
        .take_announcements("daemon", now.saturating_sub(s.stale_ms), 20, now)
        .unwrap_or_default();
    record_played(
        d,
        &rows
            .iter()
            .filter(|r| r.speech)
            .cloned()
            .collect::<Vec<_>>(),
        true,
        now,
    );
    let lines: Vec<String> = rows
        .into_iter()
        .filter(|r| r.speech)
        .map(|r| r.sentence)
        .collect();
    if lines.is_empty() {
        d.notify.speaking.store(false, Ordering::SeqCst);
        return;
    }
    let command = s.speech_command.clone();
    let limit = d.cfg.speech_timeout;
    let flag = d.notify.speaking.clone();
    let spawned = std::thread::Builder::new().name("speech".into()).spawn({
        let flag = flag.clone();
        move || {
            for line in &lines {
                say(&command, line, limit);
            }
            flag.store(false, Ordering::SeqCst);
        }
    });
    if spawned.is_err() {
        flag.store(false, Ordering::SeqCst);
    }
}

/// One sentence through the speech command; a command that outlives `limit` is killed.
fn say(command: &[String], sentence: &str, limit: std::time::Duration) {
    let Some((prog, args)) = command.split_first() else {
        return;
    };
    let mut child = match Command::new(prog).args(args).arg(sentence).spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("daemon speech: cannot run {prog}: {e}");
            return;
        }
    };
    let end = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if std::time::Instant::now() < end => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!("daemon speech: {prog} did not finish in {limit:?}; killed it");
                return;
            }
            Err(e) => {
                tracing::warn!("daemon speech: {prog}: {e}");
                return;
            }
        }
    }
}

// ---- who speaks ------------------------------------------------------------------------

fn announcement_json(r: &AnnouncementRow) -> Value {
    json!({
        "id": r.id, "key": r.key, "kind": r.kind, "sentence": r.sentence,
        "sound": r.sound, "speech": r.speech,
        "sound_name": n::sound_for(&r.kind).name(),
        "created_at": r.created_at, "taken_at": r.taken_at, "taken_by": r.taken_by,
        "played": r.played(),
    })
}

/// A monitor asks whether it is the one that speaks. If it is, it is given what is waiting, in
/// order; if not, nothing, and the announcements wait for the one that is. A muted monitor is
/// counted as present (the daemon stays quiet too) but never speaks.
pub fn claim(d: &Daemon, client: &str, muted: bool) -> Value {
    claim_at(d, client, muted, now_ms())
}

/// `claim` at a given time in milliseconds (tests drive the clock through this).
pub fn claim_at(d: &Daemon, client: &str, muted: bool, now: u64) -> Value {
    let settings = load(d).unwrap_or_default();
    *d.notify
        .last_claim
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(now);
    let mut claims = d.notify.claims.lock().unwrap_or_else(|e| e.into_inner());
    claims.retain(|c| c.client != client && now.saturating_sub(c.at_ms) < n::TAKEOVER_MS * 4);
    if !muted {
        claims.push(Claim {
            client: client.into(),
            at_ms: now,
        });
    }
    let mut speaker = d.notify.speaker.lock().unwrap_or_else(|e| e.into_inner());
    *speaker = n::elect(
        &claims,
        now,
        speaker.as_deref(),
        settings.preferred.as_deref(),
    );
    let me = !muted && speaker.as_deref() == Some(client);
    let rows = if me {
        catch_up(d, &settings, now);
        d.store
            .take_announcements(client, now.saturating_sub(settings.stale_ms), 20, now)
            .unwrap_or_default()
    } else {
        vec![]
    };
    record_played(d, &rows, false, now);
    json!({
        "speaker": me,
        "speaker_id": *speaker,
        "announcements": rows.iter().map(announcement_json).collect::<Vec<_>>(),
    })
}

/// Acknowledge an alert: its noise stops; it stays visible while its condition holds.
pub fn ack(d: &Daemon, key: &str) -> Result<bool, String> {
    d.store.ack_alert(key, now_ms()).map_err(|e| e.to_string())
}

/// Send a test through one channel, so the owner knows it works.
pub fn test(d: &Arc<Daemon>, channel: &str) -> Result<Value, String> {
    let c = Channel::parse(channel).ok_or_else(|| format!("no channel {channel:?}"))?;
    let s = load(d)?;
    let now = now_ms();
    match c {
        Channel::Monitor => Ok(
            json!({"channel": "monitor", "note": "the monitor shows the alerts list; nothing to send"}),
        ),
        Channel::Sound | Channel::Speech => {
            let line = "This is a test of MASON's voice.";
            d.store
                .enqueue_announcement(&format!("test:{now}"), "info", line, true, true, now)
                .map_err(|e| e.to_string())?;
            Ok(json!({"channel": channel, "queued": line}))
        }
        _ => {
            let nt = notifier(&s, c)
                .ok_or_else(|| format!("{channel} is not configured in notify.toml"))?;
            let m = Message {
                key: "test".into(),
                kind: "info".into(),
                title: "MASON: test".into(),
                body: "This is a test notification from MASON.".into(),
                reminder: false,
                at_ms: now,
            };
            nt.send(&m).map_err(|e| e.to_string())?;
            Ok(json!({"channel": channel, "sent": true}))
        }
    }
}

/// What the Settings page and `mason notify status` show.
pub fn status(d: &Daemon) -> Value {
    let (s, err) = match load(d) {
        Ok(s) => (s, None),
        Err(e) => (Settings::default(), Some(e)),
    };
    let rules = |k: &str| -> Vec<Value> {
        s.routing
            .rules(k)
            .iter()
            .map(|r| json!({"channel": r.channel.name(), "after_s": r.after_ms / 1000}))
            .collect()
    };
    json!({
        "error": err,
        "routing": n::CONDITIONS.iter().map(|k| json!({"condition": k, "rules": rules(k)})).collect::<Vec<_>>(),
        "reminder_after_s": s.routing.reminder_ms / 1000,
        "daemon_speech": s.daemon_speech && !s.speech_command.is_empty(),
        "configured": {"email": s.email.is_some(), "push": s.ntfy.is_some(), "webhook": s.webhook.is_some()},
        "speaker": *d.notify.speaker.lock().unwrap_or_else(|e| e.into_inner()),
        "last_played": d.store.last_played().unwrap_or_default(),
        "deliveries": d.store.recent_audible_deliveries(20).unwrap_or_default().iter().map(|r| json!({
            "alert": r.alert, "since": r.since, "channel": r.channel, "delivery": r.delivery, "ok": r.ok, "at": r.at,
        })).collect::<Vec<_>>(),
        "recent": d.store.announcements(10).unwrap_or_default().iter().map(announcement_json).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_settings_are_the_defaults() {
        let s = parse("").unwrap();
        assert_eq!(s.routing, Routing::defaults());
        assert!(!s.daemon_speech);
    }

    #[test]
    fn stall_after_defaults_to_thirty_minutes_and_zero_is_off() {
        assert_eq!(parse("").unwrap().stall_ms, 30 * n::MINUTE_MS);
        assert_eq!(parse("[stall]\nafter = \"2s\"\n").unwrap().stall_ms, 2000);
        assert_eq!(parse("[stall]\nafter = \"0\"\n").unwrap().stall_ms, 0);
        assert_eq!(parse("[stall]\nafter = 0\n").unwrap().stall_ms, 0);
    }

    #[test]
    fn overrides_quiet_hours_and_channels_parse() {
        let s = parse(
            r#"
[routing.failed]
sound = "0"
push = "5m"
[quiet]
sound = "22:00-07:00"
push = "23:30-06:00"
[reminder]
after = "45m"
[speech]
daemon = true
command = ["say", "-v", "Samantha"]
[ntfy]
topic = "mason"
token_env = "NO_SUCH_VAR_FOR_MASON_TEST"
[email]
server = "mail.example.test:587"
from = "mason@example.test"
to = "me@example.test"
user = "mason"
password = "pw"
"#,
        )
        .unwrap();
        let r = s.routing.rules("failed");
        assert_eq!(r[0].channel, Channel::Monitor);
        assert_eq!(
            r.iter()
                .find(|x| x.channel == Channel::Push)
                .unwrap()
                .after_ms,
            300_000
        );
        assert!(r.iter().all(|x| x.channel != Channel::Speech));
        assert!(s.routing.is_quiet(Channel::Sound, 23 * 60));
        assert_eq!(s.routing.reminder_ms, 45 * n::MINUTE_MS);
        assert_eq!(s.speech_command, vec!["say", "-v", "Samantha"]);
        assert_eq!(s.ntfy.unwrap().0, "https://ntfy.sh");
        let e = s.email.unwrap();
        assert_eq!(e.url, "smtp://mail.example.test:587");
        assert!(e.require_tls);
    }

    #[test]
    fn mistakes_are_named() {
        assert!(parse("[routing.failed]\nfax = \"0\"")
            .unwrap_err()
            .contains("fax"));
        assert!(parse("[quiet]\nsound = \"late\"")
            .unwrap_err()
            .contains("22:00-07:00"));
        assert!(parse("[email]\nserver = \"x\"")
            .unwrap_err()
            .contains("required"));
        assert!(parse("[loud]\n").unwrap_err().contains("loud"));
    }
}
