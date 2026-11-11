// SPDX-License-Identifier: GPL-3.0-or-later

//! Notifications (Part 19), as decisions over values. The routing table of
//! conditions and channels, quiet hours, the one-reminder rule, the sentence a
//! speaker says, the sound that goes with it, and which of several monitors
//! speaks. Nothing here reads a clock or sends anything: time and the record of
//! what was already delivered come in as arguments.

use std::collections::BTreeMap;

pub const MINUTE_MS: u64 = 60_000;

/// A way to reach the owner. `Monitor` is the visual alert on an open monitor
/// and is never quiet, never muted and never escalated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    Monitor,
    Sound,
    Speech,
    Push,
    Email,
    Webhook,
}

pub const CHANNELS: &[Channel] = &[
    Channel::Monitor,
    Channel::Sound,
    Channel::Speech,
    Channel::Push,
    Channel::Email,
    Channel::Webhook,
];

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Channel::Monitor => "monitor",
            Channel::Sound => "sound",
            Channel::Speech => "speech",
            Channel::Push => "push",
            Channel::Email => "email",
            Channel::Webhook => "webhook",
        }
    }
    pub fn parse(s: &str) -> Option<Channel> {
        CHANNELS.iter().copied().find(|c| c.name() == s.trim())
    }
    /// Channels that leave the machine.
    pub fn remote(self) -> bool {
        matches!(self, Channel::Push | Channel::Email | Channel::Webhook)
    }
    /// Channels played by a monitor (or the daemon in its place).
    pub fn audible(self) -> bool {
        matches!(self, Channel::Sound | Channel::Speech)
    }
}

/// One channel for a condition and how long to wait before using it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub channel: Channel,
    pub after_ms: u64,
}

const fn rule(channel: Channel, after_min: u64) -> Rule {
    Rule {
        channel,
        after_ms: after_min * MINUTE_MS,
    }
}

/// The conditions Part 19 names, as the alert kinds that carry them.
pub const CONDITIONS: &[&str] = &[
    "needs_you",
    "failed",
    "landing",
    "daemon_down",
    "credentials",
    "provider_outage",
    "landed",
    "disk",
    "environment",
    "blocked",
    "host_lost",
    "runtime",
    "definition",
    "tracker",
    "provider_limit",
    "provider_overage",
    "provider_budget",
    "stalled",
];

/// Every alert kind the daemon raises. Add a new kind here when it is raised, and give it a route
/// in `Routing::defaults` or name it in `MONITOR_ONLY`: a test fails until one of the two is done.
pub const ALERT_KINDS: &[&str] = &[
    "needs_you",
    "failed",
    "landing",
    "landed",
    "daemon_down",
    "credentials",
    "provider_outage",
    "provider_limit",
    "provider_overage",
    "provider_budget",
    "provider_hold",
    "disk",
    "environment",
    "blocked",
    "host_lost",
    "runtime",
    "definition",
    "tracker",
    "paused",
    "debt",
    "stalled",
];

/// Kinds that are deliberately shown on the monitor and nothing else: the owner did these
/// themselves (`paused`, `provider_hold` set by hand) or they stop nothing (`debt`: a skipped
/// check still owed). Every other kind in `ALERT_KINDS` has a route that makes noise.
pub const MONITOR_ONLY: &[&str] = &["paused", "debt", "provider_hold"];

/// Kinds that mean the whole pipeline has stopped: their reminder repeats every
/// `Routing::repeat_ms` until the alert is acknowledged or clears. Every other kind is
/// reminded once.
pub const REPEATING: &[&str] = &["stalled", "disk"];

/// A daily window in local minutes of the day, `start` up to but not including
/// `end`; a window that ends before it starts runs over midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuietHours {
    pub start: u32,
    pub end: u32,
}

impl QuietHours {
    pub fn contains(&self, minute_of_day: u32) -> bool {
        let m = minute_of_day % 1440;
        if self.start == self.end {
            false
        } else if self.start < self.end {
            m >= self.start && m < self.end
        } else {
            m >= self.start || m < self.end
        }
    }
}

/// `"22:00"` to minutes of the day.
pub fn parse_clock(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// `"10m"`, `"1h"`, `"30s"` or bare seconds, as milliseconds.
pub fn parse_span(s: &str) -> Option<u64> {
    let s = s.trim();
    let (n, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => s.split_at(i),
        None => (s, "s"),
    };
    let n: u64 = n.parse().ok()?;
    let per = match unit.trim() {
        "ms" => 1,
        "s" | "" => 1000,
        "m" | "min" => MINUTE_MS,
        "h" => 60 * MINUTE_MS,
        _ => return None,
    };
    n.checked_mul(per)
}

/// The whole table: what each condition uses, quiet hours per channel, and the
/// single reminder's delay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Routing {
    pub by_kind: BTreeMap<String, Vec<Rule>>,
    pub quiet: BTreeMap<Channel, QuietHours>,
    /// When the one reminder is due, from the alert's start.
    pub reminder_ms: u64,
    /// How often the reminder of a `REPEATING` kind is repeated after the first; `0` is never.
    pub repeat_ms: u64,
    /// How many times a failing delivery is tried, and the wait between tries.
    pub max_attempts: u32,
    pub retry_ms: u64,
}

impl Default for Routing {
    fn default() -> Self {
        Routing::defaults()
    }
}

impl Routing {
    /// Part 19's table.
    pub fn defaults() -> Routing {
        use Channel::*;
        let mut by_kind = BTreeMap::new();
        let mut put = |k: &str, r: Vec<Rule>| {
            by_kind.insert(k.to_string(), r);
        };
        put(
            "needs_you",
            vec![
                rule(Monitor, 0),
                rule(Sound, 0),
                rule(Speech, 0),
                rule(Push, 10),
                rule(Email, 60),
            ],
        );
        let broke = vec![
            rule(Monitor, 0),
            rule(Sound, 0),
            rule(Speech, 0),
            rule(Push, 10),
        ];
        put("failed", broke.clone());
        // Conditions that stop work, or the pipeline: the same as `failed`.
        for k in [
            "disk",
            "environment",
            "blocked",
            "host_lost",
            "runtime",
            "definition",
            "tracker",
            "provider_limit",
            "provider_overage",
            "provider_budget",
            "daemon_bug",
            "numbers", // the stage and exit-script numbers are nearly used up
        ] {
            put(k, broke.clone());
        }
        put("landing", broke); // the base branch is frozen, or a landing is stuck
                               // `paused`, `debt` and `provider_hold` (see `MONITOR_ONLY`) are left unrouted on purpose:
                               // the owner did them, or they stop nothing. `rules` gives them the monitor alone.
        let everything = vec![
            rule(Monitor, 0),
            rule(Sound, 0),
            rule(Speech, 0),
            rule(Push, 0),
            rule(Email, 0),
            rule(Webhook, 0),
        ];
        for k in ["daemon_down", "credentials", "provider_outage", "stalled"] {
            put(k, everything.clone());
        }
        put("landed", vec![rule(Monitor, 0), rule(Speech, 0)]);
        Routing {
            by_kind,
            quiet: BTreeMap::new(),
            reminder_ms: 30 * MINUTE_MS,
            repeat_ms: 120 * MINUTE_MS,
            max_attempts: 3,
            retry_ms: MINUTE_MS,
        }
    }

    /// A condition the table does not name is shown on the monitor and nothing else.
    pub fn rules(&self, kind: &str) -> Vec<Rule> {
        self.by_kind
            .get(kind)
            .cloned()
            .unwrap_or_else(|| vec![rule(Channel::Monitor, 0)])
    }

    /// Replace a condition's channels (the owner's choice).
    pub fn set(&mut self, kind: &str, rules: Vec<Rule>) {
        self.by_kind.insert(kind.to_string(), rules);
    }

    /// Which reminder round `now` is in for an alert that began at `since_ms`: 0 is the first
    /// reminder; later rounds exist only for `REPEATING` kinds with a non-zero `repeat_ms`.
    pub fn reminder_round(&self, kind: &str, since_ms: u64, now_ms: u64) -> u64 {
        if self.repeat_ms == 0 || !REPEATING.contains(&kind) {
            return 0;
        }
        let first = since_ms.saturating_add(self.reminder_ms);
        now_ms.saturating_sub(first) / self.repeat_ms
    }

    pub fn is_quiet(&self, c: Channel, minute_of_day: u32) -> bool {
        c != Channel::Monitor
            && self
                .quiet
                .get(&c)
                .is_some_and(|q| q.contains(minute_of_day))
    }
}

/// Initial notice, or the one reminder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Delivery {
    Initial,
    Reminder,
}

impl Delivery {
    pub fn name(self) -> &'static str {
        match self {
            Delivery::Initial => "initial",
            Delivery::Reminder => "reminder",
        }
    }
}

/// What the record says about one alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlertState {
    pub since_ms: u64,
    pub acked: bool,
}

/// One try at delivering, from the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub channel: Channel,
    pub delivery: Delivery,
    pub ok: bool,
    /// The refusal will not pass by waiting: do not try again.
    pub permanent: bool,
    pub at_ms: u64,
}

/// What is due for a condition that is still true. Nothing for an acknowledged
/// alert; each channel's initial notice once its delay has passed and it is not
/// in quiet hours; and, once, a reminder on every channel that carried the
/// initial notice, `reminder_ms` after the start (and again every `repeat_ms` for the kinds in
/// `REPEATING`, only where the initial notice arrived). A delivery that failed is
/// tried again after `retry_ms`, up to `max_attempts` times, and then given up
/// on: there are no loops. A notice that was never due before the alert cleared
/// is never sent, because a cleared alert is not asked about.
pub fn due(
    r: &Routing,
    kind: &str,
    st: AlertState,
    done: &[Attempt],
    now_ms: u64,
    minute_of_day: u32,
) -> Vec<(Channel, Delivery)> {
    if st.acked {
        return vec![];
    }
    let round = r.reminder_round(kind, st.since_ms, now_ms);
    // Attempts at or after this time belong to the current reminder round (all of them in round 0).
    let round_start = if round == 0 {
        0
    } else {
        st.since_ms + r.reminder_ms + round * r.repeat_ms
    };
    let settled = |c: Channel, d: Delivery| -> Option<bool> {
        // Some(true): delivered. Some(false): given up on or waiting to retry. None: not tried.
        let from = if d == Delivery::Reminder {
            round_start
        } else {
            0
        };
        let tries: Vec<&Attempt> = done
            .iter()
            .filter(|a| a.channel == c && a.delivery == d && a.at_ms >= from)
            .collect();
        if tries.iter().any(|a| a.ok) {
            return Some(true);
        }
        if tries.iter().any(|a| a.permanent) {
            return Some(false);
        }
        let last = tries.iter().map(|a| a.at_ms).max()?;
        if tries.len() as u32 >= r.max_attempts || now_ms < last + r.retry_ms {
            return Some(false);
        }
        None
    };
    let mut out = vec![];
    let rules = r.rules(kind);
    for ru in &rules {
        if ru.channel == Channel::Monitor || r.is_quiet(ru.channel, minute_of_day) {
            continue;
        }
        if now_ms >= st.since_ms + ru.after_ms && settled(ru.channel, Delivery::Initial).is_none() {
            out.push((ru.channel, Delivery::Initial));
        }
    }
    if now_ms >= st.since_ms + r.reminder_ms {
        for ru in &rules {
            if ru.channel == Channel::Monitor || r.is_quiet(ru.channel, minute_of_day) {
                continue;
            }
            // Only where the initial notice arrived, and was due before the reminder was: an
            // escalation that only just went out is its own nudge.
            if ru.after_ms <= r.reminder_ms
                && settled(ru.channel, Delivery::Initial) == Some(true)
                && settled(ru.channel, Delivery::Reminder).is_none()
            {
                out.push((ru.channel, Delivery::Reminder));
            }
        }
    }
    out
}

// ---- what is said ----------------------------------------------------------------------

/// Which sound goes with an announcement. "Wants a decision" and "broke"
/// differ; everything else is a quiet tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sound {
    Decision,
    Broke,
    Info,
}

impl Sound {
    pub fn name(self) -> &'static str {
        match self {
            Sound::Decision => "decision",
            Sound::Broke => "broke",
            Sound::Info => "info",
        }
    }
    pub fn parse(s: &str) -> Option<Sound> {
        [Sound::Decision, Sound::Broke, Sound::Info]
            .into_iter()
            .find(|k| k.name() == s)
    }
}

pub fn sound_for(kind: &str) -> Sound {
    match kind {
        "needs_you" => Sound::Decision,
        "failed" | "daemon_bug" | "landing" | "daemon_down" | "credentials" | "provider_outage"
        | "stalled" | "environment" | "runtime" | "definition" | "disk" | "blocked"
        | "host_lost" | "tracker" | "provider_limit" | "provider_overage" | "provider_budget"
        | "git_stuck" | "numbers" => Sound::Broke,
        _ => Sound::Info,
    }
}

fn one_line(s: &str, max: usize) -> String {
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s
        .trim_end_matches(['.', ':', ';', ',', '?', '!'])
        .to_string();
    if s.chars().count() <= max {
        s
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{}…", cut.trim_end())
    }
}

/// The one sentence: the task, what is wrong, what to do.
/// `subject` is how the task is named ("Web API 204"); `detail` is the question
/// or the reason.
pub fn sentence(kind: &str, subject: &str, detail: &str, reminder: bool) -> String {
    let subject = one_line(subject, 60);
    let detail = one_line(detail, 100);
    let text = match kind {
        "needs_you" if detail.is_empty() => {
            format!("{subject} needs a decision; open the card and answer it")
        }
        "needs_you" => format!("{subject} needs a decision on {detail}"),
        "approval" => format!("{subject} is waiting for your approval; approve or reject it"),
        "proposal" => {
            format!("{subject} is proposed and waiting for your approval; approve or reject it")
        }
        "failed" if detail.is_empty() => format!("{subject} failed; open it to retry or cancel"),
        "failed" => format!("{subject} failed because {detail}; open it to retry or cancel"),
        "landing" => format!("{subject}: {detail}; open the landing queue to see why"),
        "disk" if detail.is_empty() => "MASON is stopped: disk is nearly full".into(),
        "disk" => format!("MASON is stopped: disk is nearly full, {detail}"),
        "landed" => format!("{subject} landed"),
        "daemon_down" => "MASON has lost its daemon; check that masond is running".into(),
        "credentials" => {
            format!("{subject}: the agent is not logged in; log in again, work resumes by itself")
        }
        "provider_outage" => format!("{subject}: the provider is down; work resumes by itself"),
        _ if detail.is_empty() => format!("{subject} needs a look"),
        _ => format!("{subject}: {detail}; open it to look"),
    };
    let text = if reminder {
        format!("Reminder: {text}")
    } else {
        text
    };
    format!("{}.", text.trim_end_matches('.'))
}

// ---- who speaks ------------------------------------------------------------------------

/// A client that asked to speak, and when it last did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub client: String,
    pub at_ms: u64,
}

/// How long a speaker may be silent before another takes over.
pub const TAKEOVER_MS: u64 = 30_000;

/// Which client speaks. The current speaker keeps the job while it keeps
/// claiming; if it stops for `TAKEOVER_MS` the most recently active other
/// client takes over. With no current speaker the owner's preferred client wins
/// when it is active, otherwise the most recently active one. `claims` holds
/// every unmuted client's latest claim.
pub fn elect(
    claims: &[Claim],
    now_ms: u64,
    current: Option<&str>,
    preferred: Option<&str>,
) -> Option<String> {
    let fresh: Vec<&Claim> = claims
        .iter()
        .filter(|c| now_ms.saturating_sub(c.at_ms) < TAKEOVER_MS)
        .collect();
    if let Some(cur) = current {
        if fresh.iter().any(|c| c.client == cur) {
            return Some(cur.to_string());
        }
    }
    if let Some(p) = preferred {
        if fresh.iter().any(|c| c.client == p) {
            return Some(p.to_string());
        }
    }
    fresh
        .iter()
        .max_by(|a, b| a.at_ms.cmp(&b.at_ms).then_with(|| b.client.cmp(&a.client)))
        .map(|c| c.client.clone())
}

/// Should the daemon speak itself? Only when speech is on, and no monitor,
/// muted or not, has been heard from within the takeover time.
pub fn daemon_should_speak(enabled: bool, last_claim_ms: Option<u64>, now_ms: u64) -> bool {
    enabled && last_claim_ms.is_none_or(|t| now_ms.saturating_sub(t) >= TAKEOVER_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(since: u64) -> AlertState {
        AlertState {
            since_ms: since,
            acked: false,
        }
    }

    fn attempt(c: Channel, d: Delivery, ok: bool, at: u64) -> Attempt {
        Attempt {
            channel: c,
            delivery: d,
            ok,
            permanent: false,
            at_ms: at,
        }
    }

    #[test]
    fn an_initial_notice_that_expired_unheard_is_not_delivered() {
        let r = Routing::defaults();
        // Queued and dropped: nothing was recorded, so the sound is still due, and no reminder
        // for a channel that never carried the initial notice, however late it is.
        let late = 2 * 60 * MINUTE_MS;
        let got = due(&r, "failed", st(0), &[], late, 0);
        assert!(got.contains(&(Channel::Sound, Delivery::Initial)));
        assert!(got.contains(&(Channel::Speech, Delivery::Initial)));
        assert!(!got.iter().any(|x| x.1 == Delivery::Reminder));
        // Heard later: only then does the reminder follow.
        let heard = [attempt(Channel::Sound, Delivery::Initial, true, late)];
        let got = due(&r, "failed", st(0), &heard, late, 0);
        assert!(got.contains(&(Channel::Sound, Delivery::Reminder)));
        assert!(!got.contains(&(Channel::Speech, Delivery::Reminder)));
    }

    #[test]
    fn every_raised_kind_is_routed_or_named_monitor_only() {
        let r = Routing::defaults();
        for k in ALERT_KINDS {
            let routed = r.by_kind.contains_key(*k);
            let mon = MONITOR_ONLY.contains(k);
            assert!(routed != mon, "{k} must be routed xor named monitor-only");
            if mon {
                assert_eq!(r.rules(k), vec![rule(Channel::Monitor, 0)], "{k}");
                assert!(!CONDITIONS.contains(k), "{k}");
            } else {
                assert!(CONDITIONS.contains(k), "{k} missing from CONDITIONS");
            }
        }
        for k in CONDITIONS {
            assert!(ALERT_KINDS.contains(k), "{k} not in ALERT_KINDS");
        }
    }

    #[test]
    fn disk_makes_sound_and_speech_at_once_and_pushes_after_ten_minutes() {
        assert_eq!(
            channels("disk"),
            vec![
                (Channel::Monitor, 0),
                (Channel::Sound, 0),
                (Channel::Speech, 0),
                (Channel::Push, 10)
            ]
        );
        assert_eq!(sound_for("disk"), Sound::Broke);
        let r = Routing::defaults();
        let now = 5_000_000;
        let s = st(now);
        let first = due(&r, "disk", s, &[], now, 0);
        assert_eq!(
            first,
            vec![
                (Channel::Sound, Delivery::Initial),
                (Channel::Speech, Delivery::Initial)
            ]
        );
        let later = due(&r, "disk", s, &[], now + 10 * MINUTE_MS, 0);
        assert!(later.contains(&(Channel::Push, Delivery::Initial)));
    }

    fn channels(kind: &str) -> Vec<(Channel, u64)> {
        Routing::defaults()
            .rules(kind)
            .iter()
            .map(|r| (r.channel, r.after_ms / MINUTE_MS))
            .collect()
    }

    #[test]
    fn the_defaults_are_part_19s_table() {
        use Channel::*;
        assert_eq!(
            channels("needs_you"),
            vec![
                (Monitor, 0),
                (Sound, 0),
                (Speech, 0),
                (Push, 10),
                (Email, 60)
            ]
        );
        for k in ["failed", "landing"] {
            assert_eq!(
                channels(k),
                vec![(Monitor, 0), (Sound, 0), (Speech, 0), (Push, 10)]
            );
        }
        for k in ["daemon_down", "credentials", "provider_outage", "stalled"] {
            let c: Vec<Channel> = channels(k).into_iter().map(|x| x.0).collect();
            assert_eq!(c, vec![Monitor, Sound, Speech, Push, Email, Webhook]);
            assert!(channels(k).iter().all(|x| x.1 == 0));
        }
        assert_eq!(channels("landed"), vec![(Monitor, 0), (Speech, 0)]);
        assert_eq!(channels("something_else"), vec![(Monitor, 0)]);
    }

    #[test]
    fn an_alert_is_delivered_once_per_channel_and_waits_for_its_delay() {
        let r = Routing::defaults();
        let now = 1_000;
        let d = due(&r, "needs_you", st(now), &[], now, 12 * 60);
        assert_eq!(
            d,
            vec![
                (Channel::Sound, Delivery::Initial),
                (Channel::Speech, Delivery::Initial)
            ]
        );
        let done = vec![
            attempt(Channel::Sound, Delivery::Initial, true, now),
            attempt(Channel::Speech, Delivery::Initial, true, now),
        ];
        assert!(due(&r, "needs_you", st(now), &done, now + 5 * MINUTE_MS, 0).is_empty());
        // Push follows after ten minutes, email after an hour.
        let d = due(&r, "needs_you", st(now), &done, now + 10 * MINUTE_MS, 0);
        assert_eq!(d, vec![(Channel::Push, Delivery::Initial)]);
    }

    #[test]
    fn one_reminder_then_silence() {
        let r = Routing::defaults();
        let mut done = vec![
            attempt(Channel::Sound, Delivery::Initial, true, 0),
            attempt(Channel::Speech, Delivery::Initial, true, 0),
            attempt(Channel::Push, Delivery::Initial, true, 10 * MINUTE_MS),
        ];
        assert!(due(&r, "needs_you", st(0), &done, r.reminder_ms - 1, 0).is_empty());
        let at = r.reminder_ms;
        let d = due(&r, "needs_you", st(0), &done, at, 0);
        assert_eq!(
            d,
            vec![
                (Channel::Sound, Delivery::Reminder),
                (Channel::Speech, Delivery::Reminder),
                (Channel::Push, Delivery::Reminder)
            ]
        );
        for (c, _) in &d {
            done.push(attempt(*c, Delivery::Reminder, true, at));
        }
        // The hour-mark email goes once; it is not reminded, and nothing else repeats.
        let later = due(&r, "needs_you", st(0), &done, 100 * MINUTE_MS, 0);
        assert_eq!(later, vec![(Channel::Email, Delivery::Initial)]);
        done.push(attempt(
            Channel::Email,
            Delivery::Initial,
            true,
            100 * MINUTE_MS,
        ));
        assert!(due(&r, "needs_you", st(0), &done, 100_000 * MINUTE_MS, 0).is_empty());
    }

    fn heard(
        kind: &str,
        r: &Routing,
        done: &mut Vec<Attempt>,
        at: u64,
    ) -> Vec<(Channel, Delivery)> {
        let d = due(r, kind, st(0), done, at, 0);
        for (c, dl) in &d {
            done.push(attempt(*c, *dl, true, at));
        }
        d
    }

    #[test]
    fn stalled_repeats_at_each_interval_and_failed_does_not() {
        let r = Routing::defaults();
        assert_eq!(r.repeat_ms, 120 * MINUTE_MS);
        for kind in ["stalled", "disk"] {
            let mut done = vec![];
            let first = heard(kind, &r, &mut done, 0);
            assert!(first.iter().all(|x| x.1 == Delivery::Initial) && !first.is_empty());
            heard(kind, &r, &mut done, 10 * MINUTE_MS); // the push escalation
            let at = r.reminder_ms;
            let rem = heard(kind, &r, &mut done, at);
            assert!(
                !rem.is_empty() && rem.iter().all(|x| x.1 == Delivery::Reminder),
                "{kind}"
            );
            // Nothing between rounds.
            assert!(due(&r, kind, st(0), &done, at + r.repeat_ms - 1, 0).is_empty());
            for k in 1..4u64 {
                let t = at + k * r.repeat_ms;
                let again = heard(kind, &r, &mut done, t);
                assert_eq!(again.len(), rem.len(), "{kind} round {k}");
                assert!(heard(kind, &r, &mut done, t + 1).is_empty());
            }
        }
        let mut done = vec![];
        heard("failed", &r, &mut done, 0);
        heard("failed", &r, &mut done, 10 * MINUTE_MS);
        assert!(!heard("failed", &r, &mut done, r.reminder_ms).is_empty());
        assert!(due(
            &r,
            "failed",
            st(0),
            &done,
            r.reminder_ms + 5 * r.repeat_ms,
            0
        )
        .is_empty());
    }

    #[test]
    fn a_repeat_goes_only_where_the_first_notice_arrived() {
        let r = Routing::defaults();
        let done = vec![attempt(Channel::Sound, Delivery::Initial, true, 0)];
        let d = due(&r, "stalled", st(0), &done, r.reminder_ms + r.repeat_ms, 0);
        assert!(d.contains(&(Channel::Sound, Delivery::Reminder)));
        assert!(d
            .iter()
            .all(|x| x.1 == Delivery::Initial || x.0 == Channel::Sound));
    }

    #[test]
    fn repeats_stop_when_acknowledged_and_wait_out_quiet_hours() {
        let mut r = Routing::defaults();
        let mut done = vec![];
        heard("stalled", &r, &mut done, 0);
        heard("stalled", &r, &mut done, r.reminder_ms);
        let t = r.reminder_ms + r.repeat_ms;
        let acked = AlertState {
            since_ms: 0,
            acked: true,
        };
        assert!(due(&r, "stalled", acked, &done, t, 0).is_empty());
        r.quiet.insert(
            Channel::Sound,
            QuietHours {
                start: 22 * 60,
                end: 7 * 60,
            },
        );
        let d = due(&r, "stalled", st(0), &done, t, 23 * 60);
        assert!(d.iter().all(|x| x.0 != Channel::Sound));
        let d = due(&r, "stalled", st(0), &done, t, 12 * 60);
        assert!(d.contains(&(Channel::Sound, Delivery::Reminder)));
    }

    #[test]
    fn repeat_zero_is_one_reminder_then_silence() {
        let mut r = Routing::defaults();
        r.repeat_ms = 0;
        let mut done = vec![];
        heard("stalled", &r, &mut done, 0);
        assert!(!heard("stalled", &r, &mut done, r.reminder_ms).is_empty());
        assert!(due(&r, "stalled", st(0), &done, 100_000 * MINUTE_MS, 0).is_empty());
    }

    #[test]
    fn acknowledging_stops_everything() {
        let r = Routing::defaults();
        let s = AlertState {
            since_ms: 0,
            acked: true,
        };
        assert!(due(&r, "daemon_down", s, &[], 10_000_000, 0).is_empty());
    }

    #[test]
    fn quiet_hours_defer_but_do_not_drop_and_never_hide_the_monitor() {
        let mut r = Routing::defaults();
        r.quiet.insert(
            Channel::Sound,
            QuietHours {
                start: 22 * 60,
                end: 7 * 60,
            },
        );
        r.quiet.insert(
            Channel::Monitor,
            QuietHours {
                start: 0,
                end: 1439,
            },
        );
        assert!(!r.is_quiet(Channel::Monitor, 100));
        let night = due(&r, "needs_you", st(0), &[], 0, 23 * 60);
        assert_eq!(night, vec![(Channel::Speech, Delivery::Initial)]);
        let morning = due(&r, "needs_you", st(0), &[], 8 * MINUTE_MS * 60, 8 * 60);
        assert!(morning.contains(&(Channel::Sound, Delivery::Initial)));
    }

    #[test]
    fn quiet_window_edges() {
        let q = QuietHours {
            start: 1320,
            end: 420,
        };
        assert!(q.contains(1320) && q.contains(0) && q.contains(419));
        assert!(!q.contains(420) && !q.contains(720) && !q.contains(1319));
        let day = QuietHours {
            start: 60,
            end: 120,
        };
        assert!(day.contains(60) && !day.contains(120));
        assert!(!QuietHours { start: 5, end: 5 }.contains(5));
    }

    #[test]
    fn a_failing_delivery_is_retried_a_few_times_then_dropped() {
        let r = Routing::defaults();
        let mut done = vec![];
        let mut sent = 0;
        let mut now = 0;
        while now < 100 * MINUTE_MS {
            let d = due(&r, "daemon_down", st(0), &done, now, 0);
            for (c, dl) in d {
                if c == Channel::Webhook && dl == Delivery::Initial {
                    sent += 1;
                    done.push(attempt(c, dl, false, now));
                } else {
                    done.push(attempt(c, dl, true, now));
                }
            }
            now += 30_000;
        }
        assert_eq!(sent, r.max_attempts);
    }

    #[test]
    fn a_refusal_that_will_not_pass_is_not_tried_again() {
        let r = Routing::defaults();
        let mut a = attempt(Channel::Push, Delivery::Initial, false, 0);
        a.permanent = true;
        let d = due(&r, "daemon_down", st(0), &[a], 100 * MINUTE_MS, 0);
        assert!(!d.contains(&(Channel::Push, Delivery::Initial)));
    }

    #[test]
    fn spans_and_clock_times_parse() {
        assert_eq!(parse_span("10m"), Some(600_000));
        assert_eq!(parse_span("1h"), Some(3_600_000));
        assert_eq!(parse_span("30"), Some(30_000));
        assert_eq!(parse_span("2s"), Some(2000));
        assert_eq!(parse_span("soon"), None);
        assert_eq!(parse_clock("22:30"), Some(1350));
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("7"), None);
    }

    #[test]
    fn decision_and_broke_sound_different() {
        assert_ne!(sound_for("needs_you"), sound_for("failed"));
        assert_eq!(sound_for("needs_you"), Sound::Decision);
        assert_eq!(sound_for("failed"), Sound::Broke);
        assert_eq!(sound_for("landed"), Sound::Info);
    }

    #[test]
    fn an_approval_is_worded_as_waiting_for_approval_not_for_an_answer() {
        assert_eq!(
            sentence("approval", "Web API 204", "", false),
            "Web API 204 is waiting for your approval; approve or reject it."
        );
        assert_eq!(
            sentence("proposal", "Web API 204", "", true),
            "Reminder: Web API 204 is proposed and waiting for your approval; approve or reject it."
        );
    }

    #[test]
    fn the_sentence_names_the_task_the_trouble_and_the_next_step() {
        assert_eq!(
            sentence("needs_you", "Web API 204", "token format?", false),
            "Web API 204 needs a decision on token format."
        );
        let s = sentence("failed", "Task 7", "the build is red", false);
        assert_eq!(
            s,
            "Task 7 failed because the build is red; open it to retry or cancel."
        );
        assert_eq!(sentence("landed", "Task 7", "", false), "Task 7 landed.");
        assert!(sentence("needs_you", "Task 7", "x", true).starts_with("Reminder: "));
        let long = "word ".repeat(100);
        assert!(sentence("needs_you", "T", &long, false).chars().count() < 200);
        assert_eq!(
            sentence("failed", "T", "a.\n\nb", false)
                .matches('.')
                .count(),
            2
        );
    }

    #[test]
    fn one_client_speaks_and_another_takes_over_after_thirty_seconds() {
        let a = |t| Claim {
            client: "a".into(),
            at_ms: t,
        };
        let b = |t| Claim {
            client: "b".into(),
            at_ms: t,
        };
        // The most recently used wins when nobody speaks yet.
        assert_eq!(
            elect(&[a(1000), b(2000)], 3000, None, None).as_deref(),
            Some("b")
        );
        // The speaker keeps the job while it claims, even if the other is newer.
        assert_eq!(
            elect(&[a(1000), b(2000)], 3000, Some("a"), None).as_deref(),
            Some("a")
        );
        // Silent for 30 s: the other takes over.
        let now = 1000 + TAKEOVER_MS;
        assert_eq!(
            elect(&[a(1000), b(now - 1)], now, Some("a"), None).as_deref(),
            Some("b")
        );
        // Preferred wins when no one has the job.
        assert_eq!(
            elect(&[a(1000), b(2000)], 3000, None, Some("a")).as_deref(),
            Some("a")
        );
        // Nobody fresh, nobody speaks.
        assert_eq!(elect(&[a(0)], TAKEOVER_MS + 1, Some("a"), None), None);
        assert_eq!(elect(&[], 5, None, None), None);
    }

    #[test]
    fn the_daemon_speaks_only_when_no_monitor_is_around() {
        assert!(!daemon_should_speak(false, None, 100));
        assert!(daemon_should_speak(true, None, 100));
        assert!(!daemon_should_speak(true, Some(90_000), 100_000));
        assert!(daemon_should_speak(true, Some(60_000), 100_000));
    }

    #[test]
    fn a_full_disk_is_a_condition_that_makes_a_sound_and_speaks() {
        assert!(CONDITIONS.contains(&"disk"));
        let channels: Vec<Channel> = Routing::defaults()
            .rules("disk")
            .iter()
            .map(|r| r.channel)
            .collect();
        assert!(channels.contains(&Channel::Sound) && channels.contains(&Channel::Speech));
        assert_eq!(
            sentence("disk", "MASON", "9 tasks waiting", false),
            "MASON is stopped: disk is nearly full, 9 tasks waiting."
        );
    }
}
