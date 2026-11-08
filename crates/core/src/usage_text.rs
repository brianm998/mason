// SPDX-License-Identifier: GPL-3.0-or-later

//! The words used for LLM usage figures, so both monitors and the CLI say the
//! same thing. Pure: the clock arrives as an argument. Clock times are UTC
//! (decision 0045).

/// A length of time in the two largest useful units: "3 d 4 h", "2 h 14 m",
/// "12 min", "under a minute".
pub fn span(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} m")
    } else if m > 0 {
        format!("{m} min")
    } else {
        "under a minute".into()
    }
}

/// "as of 12 min ago"; "just now" under a minute. A reading stamped in the
/// future counts as just now.
pub fn age(now: u64, at: u64) -> String {
    let s = now.saturating_sub(at);
    if s < 60 {
        "just now".into()
    } else {
        format!("as of {} ago", span(s))
    }
}

fn clock(at: u64) -> String {
    let m = (at % 86400) / 60;
    format!("{:02}:{:02} UTC", m / 60, m % 60)
}

/// "resets in 2 h 14 m, at 15:00 UTC"; "reset time has passed" once it has.
pub fn resets(now: u64, at: u64) -> String {
    if at <= now {
        "reset time has passed".into()
    } else {
        format!("resets in {}, at {}", span(at - now), clock(at))
    }
}

/// The short form for the top bar: "resets 1 h 10 m".
pub fn resets_short(now: u64, at: u64) -> String {
    if at <= now {
        "reset due".into()
    } else {
        format!("resets {}", span(at - now))
    }
}

/// A trend over the readings of one window, oldest first, as a phrase:
/// "up 12 points over 6 h 0 m". `None` when fewer than two readings carry a percentage.
pub fn trend(points: &[(u64, f64)]) -> Option<String> {
    if points.len() < 2 {
        return None;
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    let delta = ((last.1 - first.1) * 100.0).round() as i64;
    let over = span(last.0.saturating_sub(first.0));
    Some(match delta {
        0 => format!("flat over {over}"),
        d if d > 0 => format!("up {d} points over {over}"),
        d => format!("down {} points over {over}", -d),
    })
}

/// "82%" from a 0..=1 fraction.
pub fn percent(u: f64) -> String {
    format!("{}%", (u * 100.0).round() as i64)
}

pub fn overage_off(why: &str) -> String {
    if why.is_empty() {
        "overage: off".into()
    } else {
        format!("overage: off ({why})")
    }
}

pub fn overage_in_use(now: u64, since: Option<u64>, cost: Option<f64>) -> String {
    let mut t = "overage: in use".to_string();
    if let Some(s) = since {
        t += &format!(" since {} ago", span(now.saturating_sub(s)));
    }
    if let Some(c) = cost {
        t += &format!(", cost ${c:.2}");
    }
    t
}

pub fn blocked(why: &str, now: u64, until: Option<u64>) -> String {
    match until {
        Some(u) if u > now => format!("blocked: {why}, until {} (in {})", clock(u), span(u - now)),
        Some(_) => format!("blocked: {why}, until its reset time (passed)"),
        None => format!("blocked: {why}, until unknown"),
    }
}

/// 12345 -> "12.3k", 2_400_000 -> "2.4M".
pub fn count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_use_two_units() {
        assert_eq!(span(30), "under a minute");
        assert_eq!(span(12 * 60), "12 min");
        assert_eq!(span(2 * 3600 + 14 * 60), "2 h 14 m");
        assert_eq!(span(3 * 86400 + 4 * 3600), "3 d 4 h");
    }

    #[test]
    fn age_and_resets() {
        assert_eq!(age(1000, 990), "just now");
        assert_eq!(age(1000, 1500), "just now");
        assert_eq!(age(1000 + 720, 1000), "as of 12 min ago");
        let at = 15 * 3600;
        assert_eq!(
            resets(at - (2 * 3600 + 14 * 60), at),
            "resets in 2 h 14 m, at 15:00 UTC"
        );
        assert_eq!(resets(at + 1, at), "reset time has passed");
        assert_eq!(resets_short(at - 4200, at), "resets 1 h 10 m");
    }

    #[test]
    fn trend_needs_two_points() {
        assert_eq!(trend(&[(0, 0.1)]), None);
        assert_eq!(
            trend(&[(0, 0.1), (6 * 3600, 0.22)]).unwrap(),
            "up 12 points over 6 h 0 m"
        );
        assert_eq!(
            trend(&[(0, 0.5), (3600, 0.5)]).unwrap(),
            "flat over 1 h 0 m"
        );
        assert!(trend(&[(0, 0.5), (600, 0.3)])
            .unwrap()
            .starts_with("down 20"));
    }

    #[test]
    fn overage_and_blocked_words() {
        assert_eq!(overage_off(""), "overage: off");
        assert_eq!(overage_off("org disabled"), "overage: off (org disabled)");
        assert_eq!(
            overage_in_use(7200, Some(0), Some(1.5)),
            "overage: in use since 2 h 0 m ago, cost $1.50"
        );
        assert!(blocked("5h window full", 0, Some(3600)).contains("until 01:00 UTC (in 1 h 0 m)"));
        assert!(blocked("x", 0, None).ends_with("until unknown"));
        assert_eq!(count(12345), "12.3k");
        assert_eq!(percent(0.824), "82%");
    }
}
