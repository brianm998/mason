// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading the reset time a provider gave as text: `epoch 1793944800`,
//! `resets 3pm (America/Los_Angeles)`, `try again at 3:00 PM`.
//!
//! Pure. A time is turned into an epoch only when the text pins it down: an
//! epoch number, or a clock time in UTC/GMT/Z. A clock time with a named zone
//! (no zone database is shipped) or with no zone at all is [`ResetAt::Unknown`],
//! and the caller backs off instead of guessing (decision 0049).

const DAY: u64 = 86_400;

/// What the text says about when the limit lifts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResetAt {
    /// The limit lifts at this epoch second.
    At(u64),
    /// No usable time: back off and try again.
    Unknown,
}

/// A wall-clock time found in the text, and the zone named next to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clock {
    pub hour: u32,
    pub minute: u32,
    pub zone: Option<String>,
}

/// The first wall-clock time in `text`: `3pm`, `3:00 PM`, `15:30`.
pub fn clock(text: &str) -> Option<Clock> {
    let b: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() || (i > 0 && (b[i - 1].is_alphanumeric() || b[i - 1] == ':')) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let hour: u32 = b[i..j].iter().collect::<String>().parse().ok()?;
        let mut minute = 0;
        let mut colon = false;
        if j + 1 < b.len() && b[j] == ':' && b[j + 1].is_ascii_digit() {
            let mut k = j + 1;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            minute = b[j + 1..k].iter().collect::<String>().parse().ok()?;
            colon = true;
            j = k;
        }
        let mut k = j;
        while k < b.len() && b[k] == ' ' {
            k += 1;
        }
        let rest: String = b[k..].iter().take(2).collect::<String>().to_lowercase();
        let after = b.get(k + 2).copied();
        let ampm = (rest == "am" || rest == "pm") && !after.is_some_and(|c| c.is_alphanumeric());
        if !ampm && !colon {
            i = j.max(i + 1);
            continue;
        }
        let mut h = hour;
        if ampm {
            if !(1..=12).contains(&hour) {
                return None;
            }
            h = hour % 12 + if rest == "pm" { 12 } else { 0 };
            j = k + 2;
        } else if hour > 23 {
            return None;
        }
        if minute > 59 {
            return None;
        }
        let tail: String = b[j..].iter().collect();
        return Some(Clock {
            hour: h,
            minute,
            zone: zone_of(&tail),
        });
    }
    None
}

fn zone_of(tail: &str) -> Option<String> {
    if let (Some(a), Some(z)) = (tail.find('('), tail.find(')')) {
        if a < z {
            return Some(tail[a + 1..z].trim().to_string()).filter(|s| !s.is_empty());
        }
    }
    tail.split_whitespace()
        .next()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| is_utc(w))
        .map(String::from)
}

fn is_utc(z: &str) -> bool {
    matches!(z.to_ascii_uppercase().as_str(), "UTC" | "GMT" | "Z")
}

/// The reset time the text names, given `now` (epoch seconds).
pub fn parse(text: &str, now: u64) -> ResetAt {
    let t = text.trim();
    if let Some(p) = t.to_ascii_lowercase().find("epoch") {
        let digits: String = t[p + 5..]
            .trim_start_matches(|c: char| c == ':' || c == '=' || c.is_whitespace())
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        return match digits.parse::<u64>() {
            // Milliseconds are taken for seconds ten thousand years out.
            Ok(n) if n > 100_000_000_000 => ResetAt::At(n / 1000),
            Ok(n) if n > 0 => ResetAt::At(n),
            _ => ResetAt::Unknown,
        };
    }
    match clock(t) {
        Some(Clock {
            hour,
            minute,
            zone: Some(z),
        }) if is_utc(&z) => {
            let target = u64::from(hour) * 3600 + u64::from(minute) * 60;
            let today = now / DAY * DAY + target;
            ResetAt::At(if today > now { today } else { today + DAY })
        }
        _ => ResetAt::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_794_052_800; // 2026-11-07 12:00 UTC

    #[test]
    fn epoch_text_is_an_exact_time() {
        assert_eq!(parse("epoch 1793944800", NOW), ResetAt::At(1_793_944_800));
        assert_eq!(
            parse("resets epoch:1793944800", NOW),
            ResetAt::At(1_793_944_800)
        );
        assert_eq!(
            parse("epoch 1793944800000", NOW),
            ResetAt::At(1_793_944_800)
        );
        assert_eq!(parse("epoch soon", NOW), ResetAt::Unknown);
    }

    #[test]
    fn a_named_zone_is_read_but_not_resolved() {
        let t = "resets 3pm (America/Los_Angeles)";
        assert_eq!(
            clock(t),
            Some(Clock {
                hour: 15,
                minute: 0,
                zone: Some("America/Los_Angeles".into())
            })
        );
        assert_eq!(parse(t, NOW), ResetAt::Unknown);
    }

    #[test]
    fn try_again_at_has_no_zone() {
        let c = clock("try again at 3:00 PM").unwrap();
        assert_eq!((c.hour, c.minute, c.zone), (15, 0, None));
        assert_eq!(parse("try again at 3:00 PM", NOW), ResetAt::Unknown);
        assert_eq!(clock("3:00 pm").unwrap().hour, 15);
        assert_eq!(clock("12am").unwrap().hour, 0);
        assert_eq!(clock("12 PM").unwrap().hour, 12);
    }

    #[test]
    fn utc_clock_times_resolve_to_the_next_occurrence() {
        assert_eq!(parse("resets 3pm (UTC)", NOW), ResetAt::At(NOW + 3 * 3600));
        assert_eq!(
            parse("resets 9:30am (UTC)", NOW),
            ResetAt::At(NOW + 21 * 3600 + 1800)
        );
        assert_eq!(
            parse("try again at 3:00 PM GMT", NOW),
            ResetAt::At(NOW + 3 * 3600)
        );
    }

    #[test]
    fn garbage_is_unknown() {
        for t in [
            "",
            "later",
            "resets 13pm (UTC)",
            "in 5 minutes",
            "99:99 UTC",
            "epoch",
        ] {
            assert_eq!(parse(t, NOW), ResetAt::Unknown, "{t:?}");
        }
        assert_eq!(clock("no time here"), None);
    }
}
