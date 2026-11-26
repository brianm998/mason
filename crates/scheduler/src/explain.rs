// SPDX-License-Identifier: GPL-3.0-or-later

use crate::plan::{Kind, ThermalHold};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitCause {
    /// Every unit of the resource is held.
    Full { resource: String },
    /// A request ahead of this one needs the resource and is still waiting.
    Behind { resource: String },
    /// The whole machine is wanted, or held.
    Exclusive,
    /// Starting beside other work needs more free memory than there is.
    Memory { free_mb: u64, floor_mb: u64 },
    /// Free disk is below the floor: no compile and no LLM session starts.
    Disk { free_mb: u64, floor_mb: u64 },
    /// A compile does not start while the machine's CPU use is at or above the limit.
    Cpu { busy_pct: u32, max_pct: u32 },
    /// Another compile started moments ago and does not show in the CPU reading
    /// yet. `until` is when the next one may start.
    Settling { until: u64 },
    /// The build disk has been answering slowly and a compile is already running.
    DiskStall { slow_ms: u64, max_ms: u64 },
    /// macOS's `syspolicyd` is busy checking new programs and a compile is already running.
    Policy { busy_pct: u32, max_pct: u32 },
    /// The CPU is too hot (see `hold`; for `Speed`, held below `min_pct` of its speed) and
    /// machine work is already running.
    Thermal {
        speed_pct: u32,
        min_pct: u32,
        hold: ThermalHold,
    },
    /// Needs more of the resource than its limit, so it cannot run until the
    /// limit is raised.
    TooBig {
        resource: String,
        need: u32,
        limit: u32,
    },
    /// Names a resource nobody declared.
    Unknown { resource: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ahead {
    /// The request's id in the ledger.
    pub id: u64,
    pub label: String,
    /// It asks for the whole machine.
    pub exclusive: bool,
    /// `None` for an interactive caller.
    pub steps: Option<u32>,
    /// Why this one is waiting itself, when that is a pause or a gate and not a slot.
    pub cause: Option<WaitCause>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldBy {
    /// The holder's id in the ledger.
    pub id: u64,
    pub label: String,
    pub since: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wait {
    pub cause: WaitCause,
    pub kind: Option<Kind>,
    /// 1 is next in line.
    pub position: usize,
    /// In rank order.
    pub behind: Vec<Ahead>,
    pub held_by: Vec<HeldBy>,
    /// The request this one may not pass: the first waiter ahead that holds it back by the
    /// passing rule (an exclusive request, or the one that has set aside what this needs).
    /// `None` when what holds it back is a holder, a gate or its own size.
    pub blocker: Option<Ahead>,
    /// How long this request has waited, when that is past `order.max_wait_s`
    /// and it has therefore moved ahead of other pipeline work.
    pub aged_ms: Option<u64>,
    /// An exclusive request that has waited past `order.exclusive_max_wait_s`: how long it has
    /// waited. It gave up its freeze, so other work runs again until the machine drains (s552).
    pub thawed_ms: Option<u64>,
    /// `thawed_ms` is for a converted request that froze `compile` only (s681).
    pub thawed_compile: bool,
    /// For a GitHub runner pool: (units held, limit), for "4 of 4 in use" (#700).
    pub pool: Option<(u32, u32)>,
    /// For a runner pool: how many requests ahead of this one are landing runs, checks, test
    /// steps and debt payments, in that order (2026-11-21).
    pub runs_ahead: [u32; 4],
}

pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// "4 min", "30 s", "1 h 5 min": how long ago `since_ms` was.
pub fn since(now_ms: u64, since_ms: u64) -> String {
    let s = now_ms.saturating_sub(since_ms) / 1000;
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => match (s / 3600, (s % 3600) / 60) {
            (h, 0) => format!("{h} h"),
            (h, m) => format!("{h} h {m} min"),
        },
    }
}

fn noun(resource: &str, kind: Option<Kind>) -> String {
    match (kind, resource.strip_prefix("llm:")) {
        (Some(Kind::Llm), Some(account)) => format!("LLM session ({account})"),
        (Some(Kind::Machine), _) => format!("{resource} slot"),
        _ => resource.to_string(),
    }
}

/// Most characters of an interactive command the sentence shows.
pub const COMMAND_LIMIT: usize = 40;

/// Most characters of a command's description the monitor shows before the ellipsis.
pub const DESCRIPTION_LIMIT: usize = 49;

/// A short, one-line description of a command: its first meaningful line with whitespace
/// collapsed, a leading `bash -c`/`sh -c` wrapper and its quotes dropped, ` …` after a first line
/// that has more lines behind it, and at most `limit` characters (counted as characters) before
/// an ellipsis when cut. It never contains a newline.
pub fn describe_command(cmd: &str, limit: usize) -> String {
    let mut text = cmd.trim();
    loop {
        let mut next = text;
        for shell in ["bash", "sh", "zsh"] {
            if let Some(rest) = text.strip_prefix(shell) {
                let rest = rest.trim_start();
                for flag in ["-lc", "-c"] {
                    if let Some(r) = rest.strip_prefix(flag) {
                        if r.starts_with(char::is_whitespace) {
                            next = r.trim_start();
                        }
                    }
                }
            }
        }
        if next.len() == text.len() {
            break;
        }
        text = next;
    }
    let wrapped = text.starts_with(['\'', '"']);
    let text = if wrapped {
        let q = &text[..1];
        let inner = &text[1..];
        inner.strip_suffix(q).unwrap_or(inner).trim()
    } else {
        text
    };
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("");
    let mut one = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if lines.next().is_some() {
        one.push_str(" …");
    }
    if one.chars().count() <= limit {
        return one;
    }
    let cut: String = one.chars().take(limit).collect();
    format!("{}…", cut.trim_end())
}

/// A label split for the monitor: what to show (at most `DESCRIPTION_LIMIT` characters plus an
/// ellipsis, one line) and the full command (empty for a task's own label like `#88`).
/// `interactive `<cmd>`` shows the description of the command; `task N `<cmd>`` shows
/// `task N <description>`.
pub fn label_parts(label: &str) -> (String, String) {
    if let Some(cmd) = label
        .strip_prefix("interactive `")
        .map(|r| r.strip_suffix('`').unwrap_or(r))
    {
        return (describe_command(cmd, DESCRIPTION_LIMIT), cmd.to_string());
    }
    if let Some(rest) = label.strip_prefix("task ") {
        if let Some((n, cmd)) = rest.split_once(" `") {
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                let cmd = cmd.strip_suffix('`').unwrap_or(cmd);
                let d = describe_command(cmd, DESCRIPTION_LIMIT - 6 - n.len());
                return (format!("task {n} {d}"), cmd.to_string());
            }
        }
    }
    (label.to_string(), String::new())
}

/// A holder's label as the sentence shows it. An interactive label is
/// "interactive `<command>`" with the command described in at most `COMMAND_LIMIT`
/// characters, with an ellipsis when cut. Other labels (#88) are shown whole.
fn short_label(label: &str) -> String {
    let Some(rest) = label.strip_prefix("interactive `") else {
        return label.to_string();
    };
    let cmd = rest.strip_suffix('`').unwrap_or(rest);
    format!("interactive `{}`", describe_command(cmd, COMMAND_LIMIT))
}

/// What the resource is called in totals: its own name (`compile`, `test-suite`, `llm:claude`,
/// `device:ios-simulator`).
pub fn resource_class(resource: &str) -> String {
    if resource.trim().is_empty() {
        return "other".into();
    }
    resource.to_string()
}

impl Wait {
    /// Why it waits, as a code a client can switch on: `full`, `behind`, `behind-exclusive`,
    /// `exclusive`, `cpu`, `settling`, `disk-stall`, `syspolicyd`, `memory`, `disk-floor`,
    /// `too-big`, `unknown`. Taken from the cause and the blocker, never from the sentence.
    pub fn reason_code(&self) -> &'static str {
        let behind_exclusive = self.blocker.as_ref().is_some_and(|b| b.exclusive);
        match &self.cause {
            WaitCause::Full { .. } => "full",
            WaitCause::Behind { .. } | WaitCause::Exclusive if behind_exclusive => {
                "behind-exclusive"
            }
            WaitCause::Behind { .. } => "behind",
            WaitCause::Exclusive => "exclusive",
            WaitCause::Memory { .. } => "memory",
            WaitCause::Disk { .. } => "disk-floor",
            WaitCause::Cpu { .. } => "cpu",
            WaitCause::Settling { .. } => "settling",
            WaitCause::DiskStall { .. } => "disk-stall",
            WaitCause::Policy { .. } => "syspolicyd",
            WaitCause::Thermal { .. } => "thermal",
            WaitCause::TooBig { .. } => "too-big",
            WaitCause::Unknown { .. } => "unknown",
        }
    }

    /// What this wait is for, taken from its cause and not from the sentence: the resource
    /// (`compile`, `llm:claude`, `exclusive`, ...) or the gate (`cpu`, `disk-stall`, `disk-floor`,
    /// `memory`).
    pub fn class(&self) -> String {
        cause_class(&self.cause)
    }

    /// Held by the CPU gate or its settle pause. Decided on the cause, never on the text.
    pub fn is_cpu(&self) -> bool {
        matches!(
            self.cause,
            WaitCause::Cpu { .. } | WaitCause::Settling { .. }
        )
    }
}

/// The one function from a wait's cause to its class. Never empty: a resource with no name is
/// `other`. A settling compile is waiting on CPU use, so it totals under `cpu`.
pub fn cause_class(cause: &WaitCause) -> String {
    match cause {
        WaitCause::Full { resource }
        | WaitCause::Behind { resource }
        | WaitCause::TooBig { resource, .. }
        | WaitCause::Unknown { resource } => resource_class(resource),
        WaitCause::Exclusive => "exclusive".into(),
        WaitCause::Memory { .. } => "memory".into(),
        WaitCause::Disk { .. } => "disk-floor".into(),
        WaitCause::Cpu { .. } | WaitCause::Settling { .. } => "cpu".into(),
        WaitCause::DiskStall { .. } => "disk-stall".into(),
        WaitCause::Policy { .. } => "syspolicyd".into(),
        WaitCause::Thermal { .. } => "thermal".into(),
    }
}

/// Why a request is paused by a gate, for naming it in the wait behind it.
fn reason(cause: &WaitCause, now_ms: u64) -> Option<String> {
    match cause {
        WaitCause::Settling { until } => Some(format!(
            "letting the last compile show in CPU use, {} s",
            until.saturating_sub(now_ms).div_ceil(1000)
        )),
        WaitCause::Cpu { busy_pct, max_pct } => Some(format!("CPU {busy_pct}% > {max_pct}%")),
        WaitCause::DiskStall { .. } => Some("the build disk is slow".into()),
        WaitCause::Policy { .. } => Some("macOS is busy checking new programs".into()),
        WaitCause::Thermal {
            speed_pct,
            min_pct,
            hold,
        } => Some(thermal_short(*speed_pct, *min_pct, *hold)),
        WaitCause::Disk { .. } => Some("disk is low".into()),
        WaitCause::Memory { .. } => Some("memory is low".into()),
        _ => None,
    }
}

/// The sentence for a thermal hold: the condition that is failing and what releases it.
/// `speed_pct` and `min_pct` are only named when the speed is that condition.
pub fn thermal_sentence(speed_pct: u32, min_pct: u32, hold: ThermalHold) -> String {
    match hold {
        ThermalHold::Speed => format!(
            "waiting: the CPU is held to {speed_pct}% of its speed for heat (work starts at {min_pct}% or more)"
        ),
        ThermalHold::Die { c, max_c } => format!(
            "waiting: the CPU die is at {c} °C, at or over the {max_c} °C limit (work starts again below {max_c} °C)"
        ),
        ThermalHold::PowerLimited { ms } => format!(
            "waiting: macOS has power-limited the CPU for {} s (work starts again once it is not limited)",
            ms / 1000
        ),
        ThermalHold::Ceiling(h) => format!(
            "waiting: the CPU die is at {} °C, over the {} °C ceiling (work starts again at {} °C or below)",
            h.die_c, h.ceiling_c, h.ceiling_c
        ),
    }
}

/// [`thermal_sentence`] in a few words, for where it is named inside another line.
pub fn thermal_short(speed_pct: u32, min_pct: u32, hold: ThermalHold) -> String {
    match hold {
        ThermalHold::Speed => format!("CPU speed {speed_pct}%, starts at {min_pct}% or more"),
        ThermalHold::Die { c, max_c } => format!("die {c} °C, starts below {max_c} °C"),
        ThermalHold::PowerLimited { ms } => {
            format!("power limited {} s, starts when lifted", ms / 1000)
        }
        ThermalHold::Ceiling(h) => {
            format!("die {} °C, ceiling {} °C", h.die_c, h.ceiling_c)
        }
    }
}

/// The sentence for one wait, like "Waiting for compile slot: 2nd in line,
/// held by #91 for 4 min". Who is ahead is not named: `mason resources` lists it.
pub fn explain(w: &Wait, now_ms: u64) -> String {
    let mut text = explain_cause(w, now_ms);
    if let Some(ms) = w.thawed_ms {
        let (what, scope) = if w.thawed_compile {
            ("converted exclusive request", "compile")
        } else {
            ("exclusive request", "the machine")
        };
        text.push_str(&format!(
            "; {what} waited {}: work runs again until {scope} drains",
            since(ms, 0)
        ));
    }
    match w.aged_ms {
        Some(ms) => format!(
            "{text}; waited {}: ahead of work as far along",
            since(ms, 0)
        ),
        None => text,
    }
}

fn explain_cause(w: &Wait, now_ms: u64) -> String {
    let queue = |w: &Wait, resource: &str| {
        let mut parts = vec![format!("{} in line", ordinal(w.position))];
        let holders = |w: &Wait| {
            let shown: Vec<String> = w
                .held_by
                .iter()
                .take(2)
                .map(|h| format!("{} for {}", short_label(&h.label), since(now_ms, h.since)))
                .collect();
            let more = w.held_by.len().saturating_sub(2);
            let mut s = shown.join(" and ");
            if more > 0 {
                s.push_str(&format!(" and {more} more"));
            }
            s
        };
        if let Some(b) = &w.blocker {
            let mut s = format!(
                "behind {}{}",
                if b.exclusive {
                    "an exclusive request "
                } else {
                    ""
                },
                short_label(&b.label)
            );
            if let Some(reason) = b.cause.as_ref().and_then(|c| reason(c, now_ms)) {
                s.push_str(&format!(" ({reason})"));
            }
            parts.push(s);
            if !w.held_by.is_empty() {
                parts.push(format!("which waits for {}", holders(w)));
            }
        } else if !w.held_by.is_empty() {
            parts.push(format!("held by {}", holders(w)));
        } else if let Some(first) = w.behind.first() {
            // No holder: the queue waits behind a paused request, not for a slot.
            let mut s = format!("behind {}", short_label(&first.label));
            if let Some(reason) = first.cause.as_ref().and_then(|c| reason(c, now_ms)) {
                s.push_str(&format!(" ({reason})"));
            }
            parts.push(s);
        }
        format!(
            "Waiting for {}: {}",
            noun(resource, w.kind),
            parts.join(", ")
        )
    };
    match &w.cause {
        WaitCause::Full { resource } | WaitCause::Behind { resource }
            if crate::plan::is_gha_pool(resource) || resource == crate::plan::GHA_DEBT =>
        {
            let what = if resource == crate::plan::GHA_MACOS {
                "a GitHub macOS runner"
            } else if resource == crate::plan::GHA_DEBT {
                "a GitHub runner (debt payments are capped)"
            } else {
                "a GitHub runner"
            };
            let mut text = format!("waiting for {what}");
            if let Some((used, limit)) = w.pool {
                text.push_str(&format!(": {used} of {limit} in use"));
            }
            let labels: Vec<String> = w.held_by.iter().map(|h| short_label(&h.label)).collect();
            if !labels.is_empty() {
                text.push_str(&format!(" ({})", labels.join(", ")));
            }
            if w.position > 1 {
                text.push_str(&format!(", {} in line", ordinal(w.position)));
            }
            let [l, c, t, dbt] = w.runs_ahead;
            let mut ahead = vec![];
            for (n, one, many) in [
                (l, "landing run", "landing runs"),
                (c, "check", "checks"),
                (t, "test step", "test steps"),
                (dbt, "debt payment", "debt payments"),
            ] {
                if n > 0 {
                    ahead.push(format!("{n} {}", if n == 1 { one } else { many }));
                }
            }
            if let Some(last) = ahead.pop() {
                let list = if ahead.is_empty() {
                    last
                } else {
                    format!("{} and {last}", ahead.join(", "))
                };
                text.push_str(&format!(": {list} ahead"));
            }
            text
        }
        WaitCause::Full { resource } | WaitCause::Behind { resource } => queue(w, resource),
        WaitCause::Exclusive => {
            let mut s = queue(w, "the whole machine");
            s = s.replacen("Waiting for the whole machine", "Waiting for the machine", 1);
            s
        }
        WaitCause::Memory { free_mb, floor_mb } => format!(
            "Waiting for free memory: {free_mb} MB free, {floor_mb} MB needed to start beside other work"
        ),
        WaitCause::Disk { free_mb, floor_mb } => format!(
            "Waiting: disk has {} GB free, floor {} GB",
            free_mb / 1024,
            floor_mb / 1024
        ),
        WaitCause::Cpu { busy_pct, max_pct } => {
            format!("compile waiting: CPU {busy_pct}% > {max_pct}%")
        }
        WaitCause::DiskStall { slow_ms, max_ms } => format!(
            "compile waiting: the build disk took {:.1} s to answer (limit {:.1} s)",
            *slow_ms as f64 / 1000.0,
            *max_ms as f64 / 1000.0
        ),
        WaitCause::Policy { busy_pct, max_pct } => format!(
            "compile waiting: syspolicyd is at {busy_pct}% CPU checking new programs (limit {max_pct}%)"
        ),
        WaitCause::Thermal {
            speed_pct,
            min_pct,
            hold,
        } => thermal_sentence(*speed_pct, *min_pct, *hold),
        WaitCause::Settling { until } => format!(
            "compile waiting: letting the last compile show in CPU use ({} s)",
            until.saturating_sub(now_ms).div_ceil(1000)
        ),
        WaitCause::TooBig {
            resource,
            need,
            limit,
        } => format!(
            "Cannot start: needs {need} of {} but the limit is {limit}",
            noun(resource, w.kind)
        ),
        WaitCause::Unknown { resource } => {
            format!("Cannot start: `{resource}` is not a declared resource")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `%` figure in `text`.
    fn percentages(text: &str) -> Vec<u64> {
        text.split('%')
            .rev()
            .skip(1)
            .filter_map(|before| {
                let digits =
                    before.len() - before.trim_end_matches(|c: char| c.is_ascii_digit()).len();
                before[before.len() - digits..].parse().ok()
            })
            .collect()
    }

    #[test]
    fn a_thermal_hold_names_its_condition_and_never_a_threshold_above_100_percent() {
        let ceiling = crate::plan::HeatCeiling {
            die_c: 101,
            ceiling_c: 99,
        };
        // (speed, minimum, hold, what the sentence and the short form must both name)
        let cases: [(u32, u32, ThermalHold, &[&str]); 4] = [
            (27, 80, ThermalHold::Speed, &["27%", "80%"]),
            (
                100,
                35,
                ThermalHold::Die { c: 97, max_c: 90 },
                &["97 °C", "below 90 °C"],
            ),
            (
                100,
                35,
                ThermalHold::PowerLimited { ms: 45_000 },
                &["power", "limited", "45 s"],
            ),
            (100, 35, ThermalHold::Ceiling(ceiling), &["101 °C", "99 °C"]),
        ];
        for (speed, min, hold, named) in cases {
            for text in [
                thermal_sentence(speed, min, hold),
                thermal_short(speed, min, hold),
            ] {
                for n in named {
                    assert!(text.contains(n), "{hold:?}: {text} lacks {n}");
                }
                let p = percentages(&text);
                assert!(p.iter().all(|n| *n <= 100), "{text}");
                assert_eq!(p.is_empty(), hold != ThermalHold::Speed, "{text}");
            }
        }
        assert_eq!(percentages("at 101% or more, 7%"), [7, 101]);
    }

    #[test]
    fn every_wait_cause_has_a_class() {
        let r = |n: &str| n.to_string();
        let causes = [
            WaitCause::Full {
                resource: r("compile"),
            },
            WaitCause::Full { resource: r("") },
            WaitCause::Behind {
                resource: r("test-suite"),
            },
            WaitCause::Behind {
                resource: r("llm:claude"),
            },
            WaitCause::Exclusive,
            WaitCause::Memory {
                free_mb: 1,
                floor_mb: 2,
            },
            WaitCause::Disk {
                free_mb: 1,
                floor_mb: 2,
            },
            WaitCause::Cpu {
                busy_pct: 90,
                max_pct: 50,
            },
            WaitCause::Settling { until: 5 },
            WaitCause::DiskStall {
                slow_ms: 3000,
                max_ms: 1000,
            },
            WaitCause::Policy {
                busy_pct: 640,
                max_pct: 300,
            },
            WaitCause::TooBig {
                resource: r("compile"),
                need: 5,
                limit: 2,
            },
            WaitCause::TooBig {
                resource: r(" "),
                need: 5,
                limit: 2,
            },
            WaitCause::Unknown { resource: r("") },
        ];
        for c in causes {
            assert!(!cause_class(&c).is_empty(), "{c:?}");
        }
        assert_eq!(cause_class(&WaitCause::Settling { until: 1 }), "cpu");
    }

    /// The structured wait and its sentence are made from one `Wait`: for every cause the
    /// reason code is the one expected and the sentence carries the same facts.
    #[test]
    fn the_sentence_and_the_structured_wait_agree_for_every_cause() {
        let r = |n: &str| n.to_string();
        let held = vec![HeldBy {
            id: 7,
            label: "#91".into(),
            since: 0,
        }];
        let excl_ahead = Ahead {
            id: 3,
            exclusive: true,
            label: "interactive `starpipe heavy`".into(),
            steps: None,
            cause: Some(WaitCause::Exclusive),
        };
        let plain_ahead = Ahead {
            id: 4,
            exclusive: false,
            label: "#289".into(),
            steps: Some(2),
            cause: None,
        };
        let mk = |cause: WaitCause, blocker: Option<Ahead>, held_by: Vec<HeldBy>| Wait {
            cause,
            kind: Some(Kind::Machine),
            position: 3,
            behind: blocker.clone().into_iter().collect(),
            held_by,
            blocker,
            aged_ms: None,
            thawed_ms: None,
            thawed_compile: false,
            pool: None,
            runs_ahead: [0; 4],
        };
        // (wait, the code, a fragment the sentence must have)
        let cases = [
            (
                mk(
                    WaitCause::Full {
                        resource: r("compile"),
                    },
                    None,
                    held.clone(),
                ),
                "full",
                "held by #91",
            ),
            (
                mk(
                    WaitCause::Behind {
                        resource: r("compile"),
                    },
                    Some(plain_ahead.clone()),
                    vec![],
                ),
                "behind",
                "behind #289",
            ),
            (
                mk(WaitCause::Exclusive, Some(excl_ahead.clone()), held.clone()),
                "behind-exclusive",
                "behind an exclusive request interactive `starpipe heavy`, which waits for #91",
            ),
            (
                mk(WaitCause::Exclusive, None, held.clone()),
                "exclusive",
                "held by #91",
            ),
            (
                mk(
                    WaitCause::Memory {
                        free_mb: 1,
                        floor_mb: 2,
                    },
                    None,
                    vec![],
                ),
                "memory",
                "1 MB free",
            ),
            (
                mk(
                    WaitCause::Disk {
                        free_mb: 1024,
                        floor_mb: 2048,
                    },
                    None,
                    vec![],
                ),
                "disk-floor",
                "1 GB free",
            ),
            (
                mk(
                    WaitCause::Cpu {
                        busy_pct: 90,
                        max_pct: 50,
                    },
                    None,
                    vec![],
                ),
                "cpu",
                "CPU 90% > 50%",
            ),
            (
                mk(WaitCause::Settling { until: 5_000 }, None, vec![]),
                "settling",
                "5 s",
            ),
            (
                mk(
                    WaitCause::DiskStall {
                        slow_ms: 3000,
                        max_ms: 1000,
                    },
                    None,
                    vec![],
                ),
                "disk-stall",
                "3.0 s",
            ),
            (
                mk(
                    WaitCause::Policy {
                        busy_pct: 640,
                        max_pct: 300,
                    },
                    None,
                    vec![],
                ),
                "syspolicyd",
                "640%",
            ),
            (
                mk(
                    WaitCause::TooBig {
                        resource: r("compile"),
                        need: 5,
                        limit: 2,
                    },
                    None,
                    vec![],
                ),
                "too-big",
                "needs 5",
            ),
            (
                mk(WaitCause::Unknown { resource: r("zzz") }, None, vec![]),
                "unknown",
                "`zzz`",
            ),
        ];
        for (w, code, fragment) in cases {
            assert_eq!(w.reason_code(), code, "{:?}", w.cause);
            let s = explain(&w, 0);
            assert!(s.contains(fragment), "{code}: {s}");
        }
    }

    #[test]
    fn ordinals() {
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111]
            .iter()
            .map(|n| ordinal(*n))
            .collect();
        assert_eq!(
            got,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st",
                "111th"
            ]
        );
    }

    #[test]
    fn durations() {
        assert_eq!(since(10_000, 4_000), "6 s");
        assert_eq!(since(240_000, 0), "4 min");
        assert_eq!(since(3_600_000, 0), "1 h");
        assert_eq!(since(3_900_000, 0), "1 h 5 min");
        assert_eq!(since(0, 5_000), "0 s");
    }

    fn wait(behind: Vec<Ahead>, holder: &str) -> Wait {
        Wait {
            cause: WaitCause::Full {
                resource: "compile".into(),
            },
            kind: Some(Kind::Machine),
            position: behind.len() + 1,
            behind,
            blocker: None,
            held_by: vec![HeldBy {
                id: 0,
                label: holder.into(),
                since: 0,
            }],
            aged_ms: None,
            thawed_ms: None,
            thawed_compile: false,
            pool: None,
            runs_ahead: [0; 4],
        }
    }

    #[test]
    fn an_aged_wait_says_how_long_and_that_it_goes_ahead_of_work_as_far_along() {
        let mut w = wait(vec![], "#91");
        w.aged_ms = Some(30 * 60_000);
        assert_eq!(
            explain(&w, 0),
            "Waiting for compile slot: 1st in line, held by #91 for 0 s; waited 30 min: ahead of work as far along"
        );
    }

    #[test]
    fn the_sentence_from_the_design() {
        let w = wait(
            vec![Ahead {
                id: 0,
                exclusive: false,
                label: "#88".into(),
                steps: Some(2),
                cause: None,
            }],
            "#91",
        );
        assert_eq!(
            explain(&w, 240_000),
            "Waiting for compile slot: 2nd in line, held by #91 for 4 min"
        );
    }

    #[test]
    fn behind_a_paused_request_with_no_holder() {
        let w = Wait {
            cause: WaitCause::Behind {
                resource: "compile".into(),
            },
            kind: Some(Kind::Machine),
            position: 4,
            blocker: None,
            behind: vec![Ahead {
                id: 0,
                exclusive: false,
                label: "#289".into(),
                steps: Some(2),
                cause: Some(WaitCause::Settling { until: 37_000 }),
            }],
            held_by: vec![],
            aged_ms: None,
            thawed_ms: None,
            thawed_compile: false,
            pool: None,
            runs_ahead: [0; 4],
        };
        let s = explain(&w, 30_000);
        assert_eq!(
            s,
            "Waiting for compile slot: 4th in line, behind #289 (letting the last compile show in CPU use, 7 s)"
        );
        assert!(!s.contains("held by"));
    }

    #[test]
    fn interactive_waiters_ahead_are_not_named() {
        let ahead = |c: char| Ahead {
            id: 0,
            exclusive: false,
            label: format!("interactive `{}`", c.to_string().repeat(300)),
            steps: None,
            cause: None,
        };
        let w = wait(vec![ahead('a'), ahead('b'), ahead('c')], "#91");
        let s = explain(&w, 240_000);
        assert_eq!(
            s,
            "Waiting for compile slot: 4th in line, held by #91 for 4 min"
        );
        assert!(!s.contains("aaaa") && !s.contains("bbbb") && !s.contains("cccc"));
        assert!(!s.contains("behind"));
    }

    #[test]
    fn a_long_interactive_holder_is_cut_and_a_short_one_is_whole() {
        let long = format!("interactive `{}`", "x".repeat(300));
        let s = explain(&wait(vec![], &long), 240_000);
        let want = format!(
            "held by interactive `{}…` for 4 min",
            "x".repeat(COMMAND_LIMIT)
        );
        assert!(s.ends_with(&want), "{s}");
        let s = explain(&wait(vec![], "interactive `cargo build`"), 240_000);
        assert!(
            s.ends_with("held by interactive `cargo build` for 4 min"),
            "{s}"
        );
    }

    #[test]
    fn describe_command_cases() {
        let script = "bash -c python3 - <<'EOF'\np=\"crates/a.rs\"\nprint(p)\nEOF";
        let d = describe_command(script, DESCRIPTION_LIMIT);
        assert_eq!(d, "python3 - <<'EOF' …");
        assert!(!d.contains('\n'));
        assert_eq!(describe_command("cargo  build", 49), "cargo build");
        let long = "y".repeat(500);
        let d = describe_command(&long, 49);
        assert!(d.chars().count() <= 50 && d.ends_with('…'), "{d}");
        assert_eq!(
            describe_command("bash -c 'cargo test -p x'", 49),
            "cargo test -p x"
        );
        assert_eq!(describe_command("sh -c \"ls -l\"", 49), "ls -l");
        let multi = "é".repeat(100);
        let d = describe_command(&multi, 49);
        assert_eq!(d.chars().count(), 50);
        assert_eq!(describe_command(&"é".repeat(49), 49), "é".repeat(49));
    }

    #[test]
    fn label_parts_split_commands_from_task_labels() {
        assert_eq!(label_parts("#88"), ("#88".into(), String::new()));
        let (l, c) = label_parts("interactive `cargo build`");
        assert_eq!((l.as_str(), c.as_str()), ("cargo build", "cargo build"));
        let (l, c) = label_parts("task 5 `a\nb`");
        assert_eq!((l.as_str(), c.as_str()), ("task 5 a …", "a\nb"));
    }
}
