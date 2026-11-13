// SPDX-License-Identifier: GPL-3.0-or-later

use crate::plan::Kind;

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
    pub label: String,
    /// `None` for an interactive caller.
    pub steps: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldBy {
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
    /// How long this request has waited, when that is past `order.max_wait_s`
    /// and it has therefore moved ahead of other pipeline work.
    pub aged_ms: Option<u64>,
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

/// A holder's label as the sentence shows it. An interactive label is
/// "interactive `<whole command>`"; it is cut so the command takes at most
/// `COMMAND_LIMIT` characters, with an ellipsis when cut. Other labels (#88)
/// are shown whole.
fn short_label(label: &str) -> String {
    let Some(rest) = label.strip_prefix("interactive `") else {
        return label.to_string();
    };
    let cmd = rest.strip_suffix('`').unwrap_or(rest);
    if cmd.chars().count() <= COMMAND_LIMIT {
        return label.to_string();
    }
    let cut: String = cmd.chars().take(COMMAND_LIMIT).collect();
    format!("interactive `{}…`", cut.trim_end())
}

/// What the resource is called in totals: its own name (`compile`, `test-suite`, `llm:claude`,
/// `device:ios-simulator`).
pub fn resource_class(resource: &str) -> String {
    resource.to_string()
}

impl Wait {
    /// What this wait is for, taken from its cause and not from the sentence: the resource
    /// (`compile`, `llm:claude`, `exclusive`, ...) or the gate (`cpu`, `disk-stall`, `disk-floor`,
    /// `memory`).
    pub fn class(&self) -> String {
        match &self.cause {
            WaitCause::Full { resource }
            | WaitCause::Behind { resource }
            | WaitCause::TooBig { resource, .. }
            | WaitCause::Unknown { resource } => resource_class(resource),
            WaitCause::Exclusive => "exclusive".into(),
            WaitCause::Memory { .. } => "memory".into(),
            WaitCause::Disk { .. } => "disk-floor".into(),
            WaitCause::Cpu { .. } | WaitCause::Settling { .. } => "cpu".into(),
            WaitCause::DiskStall { .. } => "disk-stall".into(),
        }
    }
}

/// The sentence for one wait, like "Waiting for compile slot: 2nd in line,
/// held by #91 for 4 min". Who is ahead is not named: `mason resources` lists it.
pub fn explain(w: &Wait, now_ms: u64) -> String {
    let text = explain_cause(w, now_ms);
    match w.aged_ms {
        Some(ms) => format!(
            "{text}; waited {}: first in line once the machine is free",
            since(ms, 0)
        ),
        None => text,
    }
}

fn explain_cause(w: &Wait, now_ms: u64) -> String {
    let queue = |w: &Wait, resource: &str| {
        let mut parts = vec![format!("{} in line", ordinal(w.position))];
        if !w.held_by.is_empty() {
            let shown: Vec<String> = w
                .held_by
                .iter()
                .take(2)
                .map(|h| format!("{} for {}", short_label(&h.label), since(now_ms, h.since)))
                .collect();
            let more = w.held_by.len().saturating_sub(2);
            let mut s = format!("held by {}", shown.join(" and "));
            if more > 0 {
                s.push_str(&format!(" and {more} more"));
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
            held_by: vec![HeldBy {
                label: holder.into(),
                since: 0,
            }],
            aged_ms: None,
        }
    }

    #[test]
    fn an_aged_wait_says_how_long_and_that_it_is_first_in_line() {
        let mut w = wait(vec![], "#91");
        w.aged_ms = Some(30 * 60_000);
        assert_eq!(
            explain(&w, 0),
            "Waiting for compile slot: 1st in line, held by #91 for 0 s; waited 30 min: first in line once the machine is free"
        );
    }

    #[test]
    fn the_sentence_from_the_design() {
        let w = wait(
            vec![Ahead {
                label: "#88".into(),
                steps: Some(2),
            }],
            "#91",
        );
        assert_eq!(
            explain(&w, 240_000),
            "Waiting for compile slot: 2nd in line, held by #91 for 4 min"
        );
    }

    #[test]
    fn interactive_waiters_ahead_are_not_named() {
        let ahead = |c: char| Ahead {
            label: format!("interactive `{}`", c.to_string().repeat(300)),
            steps: None,
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
}
