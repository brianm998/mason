// SPDX-License-Identifier: GPL-3.0-or-later

//! Is a CI run that still answers "pending" actually moving? (s1043, decision 1036)
//!
//! A run whose jobs hang, or never get a runner, stays pending until `[ci] timeout_s`. The
//! daemon looks at the jobs on every poll; this module says when the same picture has been
//! seen for long enough to tell the owner. Pure: the clock is passed in as seconds.

/// One look at a pending run: when, what the jobs looked like, and whether any job is
/// not finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    /// Seconds on any clock that only goes forward.
    pub at_s: u64,
    /// The per-job states: counts and the running job names. Equal text, equal picture.
    pub signature: String,
    /// A job is queued or running, or the CI gave no job detail (a pending run is unfinished).
    pub unfinished: bool,
}

/// How long (seconds) the last look's signature has gone unchanged, when that is at least
/// `after_s` and the run is unfinished. `None`: not stuck, or `after_s == 0` (off).
pub fn stuck_for(looks: &[Look], after_s: u64) -> Option<u64> {
    if after_s == 0 {
        return None;
    }
    let last = looks.last()?;
    if !last.unfinished {
        return None;
    }
    let since = looks
        .iter()
        .rev()
        .take_while(|l| l.signature == last.signature)
        .last()?
        .at_s;
    let quiet = last.at_s.saturating_sub(since);
    (quiet >= after_s).then_some(quiet)
}

/// "45 min", "2 h 5 min", "30 s".
pub fn span(secs: u64) -> String {
    match secs {
        0..=89 => format!("{secs} s"),
        90..=5399 => format!("{} min", (secs + 30) / 60),
        _ => {
            let m = (secs + 30) / 60;
            if m % 60 == 0 {
                format!("{} h", m / 60)
            } else {
                format!("{} h {} min", m / 60, m % 60)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(at_s: u64, sig: &str, unfinished: bool) -> Look {
        Look {
            at_s,
            signature: sig.into(),
            unfinished,
        }
    }

    #[test]
    fn an_unchanged_signature_past_the_limit_is_stuck() {
        let l = [
            look(0, "a", true),
            look(600, "a", true),
            look(1800, "a", true),
        ];
        assert_eq!(stuck_for(&l, 1800), Some(1800));
        assert_eq!(stuck_for(&l, 1801), None);
    }

    #[test]
    fn a_changed_signature_resets_the_clock() {
        let l = [
            look(0, "a", true),
            look(1000, "b", true),
            look(1900, "b", true),
        ];
        assert_eq!(stuck_for(&l, 1800), None);
        assert_eq!(stuck_for(&l, 900), Some(900));
    }

    #[test]
    fn a_finished_run_is_never_stuck() {
        let l = [look(0, "a", false), look(5000, "a", false)];
        assert_eq!(stuck_for(&l, 1800), None);
    }

    #[test]
    fn queued_only_jobs_count_as_stuck() {
        let l = [look(0, "queued=2", true), look(4000, "queued=2", true)];
        assert_eq!(stuck_for(&l, 1800), Some(4000));
    }

    #[test]
    fn zero_turns_it_off() {
        let l = [look(0, "a", true), look(99999, "a", true)];
        assert_eq!(stuck_for(&l, 0), None);
        assert_eq!(stuck_for(&[], 10), None);
    }

    #[test]
    fn spans_read_plainly() {
        assert_eq!(span(30), "30 s");
        assert_eq!(span(1800), "30 min");
        assert_eq!(span(7500), "2 h 5 min");
    }
}
