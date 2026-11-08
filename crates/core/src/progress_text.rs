// SPDX-License-Identifier: GPL-3.0-or-later

//! The words progress is shown in, the same in the CLI, the conformance run and (as
//! the same rules written again, checked against this one) both monitors.
//!
//! The rule for numbers: a percentage only from a real total (`reported` or `counted`),
//! "212 of 340 tests, 62%". With only a `history` total, the count and the typical
//! range, "1,240 lines, usually 1,800 to 2,400", and no percentage. With no total, the
//! count alone, "37 tool calls". A percentage is never estimated.

use crate::progress::{percent, Source};

/// `1240` as "1,240".
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A time span: "42 s", "9 min", "3h10". Always shown, never hidden.
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => {
            let m = (s % 3600) / 60;
            if m == 0 {
                format!("{}h", s / 3600)
            } else {
                format!("{}h{:02}", s / 3600, m)
            }
        }
    }
}

/// What a count and its total say, by the rule for numbers. Empty when there is nothing
/// to say yet (no count and no real total).
#[allow(clippy::too_many_arguments)]
pub fn count_text(
    unit: &str,
    done: u64,
    done_source: Source,
    total: u64,
    total_source: Source,
    total_low: u64,
) -> String {
    let unit_sp = if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    };
    match total_source {
        Source::Reported | Source::Counted if total > 0 => {
            let pct = percent(done, total).unwrap_or(0);
            format!("{} of {}{unit_sp}, {pct}%", group(done), group(total))
        }
        Source::History if done_source != Source::None => {
            let range = if total_low > 0 && total_low < total {
                format!("{} to {}", group(total_low), group(total))
            } else {
                group(total)
            };
            format!("{}{unit_sp}, usually {range}", group(done))
        }
        _ if done_source != Source::None => format!("{}{unit_sp}", group(done)),
        _ => String::new(),
    }
}

/// Where a figure came from, in words.
pub fn source_words(source: Source, samples: u32) -> String {
    match source {
        Source::None => String::new(),
        Source::Reported => "from the agent".into(),
        Source::Counted => "counted from output".into(),
        Source::History => format!("usually, from {samples} past runs"),
    }
}

/// "updated 4 s ago", counted against the daemon's clock. Empty when nothing moved yet.
pub fn updated_text(now_ms: u64, moved_ms: u64) -> String {
    if moved_ms == 0 {
        return String::new();
    }
    format!("updated {} ago", duration(now_ms.saturating_sub(moved_ms)))
}

/// "no output for 6 min", for a step quiet past its usual gap.
pub fn quiet_text(now_ms: u64, moved_ms: u64) -> String {
    format!(
        "no output for {}",
        duration(now_ms.saturating_sub(moved_ms))
    )
}

/// The time part of a progress line: the quiet text when stale, else "updated N s ago".
pub fn time_text(now_ms: u64, moved_ms: u64, stale_after_ms: u64) -> String {
    if crate::progress::is_stale(now_ms, moved_ms, stale_after_ms) {
        quiet_text(now_ms, moved_ms)
    } else {
        updated_text(now_ms, moved_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Source::*;

    #[test]
    fn numbers_are_grouped() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1240), "1,240");
        assert_eq!(group(1_234_567), "1,234,567");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0), "0 s");
        assert_eq!(duration(4_999), "4 s");
        assert_eq!(duration(60_000), "1 min");
        assert_eq!(duration(3_600_000), "1h");
        assert_eq!(duration(3_600_000 + 10 * 60_000), "1h10");
    }

    #[test]
    fn a_real_total_gives_a_percentage() {
        assert_eq!(
            count_text("tests", 212, Reported, 340, Reported, 0),
            "212 of 340 tests, 62%"
        );
        assert_eq!(
            count_text("lines", 5, Counted, 7, Counted, 0),
            "5 of 7 lines, 71%"
        );
        assert_eq!(count_text("", 3, Reported, 7, Reported, 0), "3 of 7, 42%");
        assert_eq!(
            count_text("tests", 400, Reported, 340, Reported, 0),
            "400 of 340 tests, 100%"
        );
    }

    #[test]
    fn a_typical_total_gives_the_range_and_no_percentage() {
        let t = count_text("lines", 1240, Counted, 2400, History, 1800);
        assert_eq!(t, "1,240 lines, usually 1,800 to 2,400");
        assert!(!t.contains('%'));
        assert_eq!(
            count_text("lines", 10, Counted, 2400, History, 0),
            "10 lines, usually 2,400"
        );
    }

    #[test]
    fn no_total_gives_the_count_only() {
        assert_eq!(
            count_text("tool calls", 37, Reported, 0, None, 0),
            "37 tool calls"
        );
        assert_eq!(count_text("", 0, None, 0, None, 0), "");
        // A total of 0 is not a real total.
        assert_eq!(count_text("tests", 3, Reported, 0, Reported, 0), "3 tests");
    }

    #[test]
    fn the_time_part() {
        assert_eq!(updated_text(10_000, 6_000), "updated 4 s ago");
        assert_eq!(updated_text(10_000, 0), "");
        assert_eq!(time_text(400_000, 40_000, 300_000), "no output for 6 min");
        assert_eq!(time_text(300_000, 40_000, 300_000), "updated 4 min ago");
        assert_eq!(source_words(History, 12), "usually, from 12 past runs");
        assert_eq!(source_words(Reported, 0), "from the agent");
        assert_eq!(source_words(Counted, 0), "counted from output");
    }
}
