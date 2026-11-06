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

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The sentence for one wait, like "Waiting for compile slot: 2nd in line,
/// behind #88 (2 steps from landing), held by #91 for 4 min".
pub fn explain(w: &Wait, now_ms: u64) -> String {
    let queue = |w: &Wait, resource: &str| {
        let mut parts = vec![format!("{} in line", ordinal(w.position))];
        if !w.behind.is_empty() {
            let shown: Vec<String> = w
                .behind
                .iter()
                .take(2)
                .map(|a| match a.steps {
                    Some(n) => format!("{} ({} from landing)", a.label, plural(n, "step", "steps")),
                    None => format!("{} (interactive)", a.label),
                })
                .collect();
            let more = w.behind.len().saturating_sub(2);
            let mut s = format!("behind {}", shown.join(" and "));
            if more > 0 {
                s.push_str(&format!(" and {more} more"));
            }
            parts.push(s);
        }
        if !w.held_by.is_empty() {
            let shown: Vec<String> = w
                .held_by
                .iter()
                .take(2)
                .map(|h| format!("{} for {}", h.label, since(now_ms, h.since)))
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

    #[test]
    fn the_sentence_from_the_design() {
        let w = Wait {
            cause: WaitCause::Full {
                resource: "compile".into(),
            },
            kind: Some(Kind::Machine),
            position: 2,
            behind: vec![Ahead {
                label: "#88".into(),
                steps: Some(2),
            }],
            held_by: vec![HeldBy {
                label: "#91".into(),
                since: 0,
            }],
        };
        assert_eq!(
            explain(&w, 240_000),
            "Waiting for compile slot: 2nd in line, behind #88 (2 steps from landing), held by #91 for 4 min"
        );
    }
}
