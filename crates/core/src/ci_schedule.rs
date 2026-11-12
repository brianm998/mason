// SPDX-License-Identifier: GPL-3.0-or-later

//! The rules of a scheduled CI run (s175, decision 0170): when a slow workflow is worth
//! dispatching, how its run is followed (rerun the failed jobs once), and how the jobs are
//! judged and grouped for filing. No I/O and no clock: runs, files and times come in as values.

/// A followed run that has not moved after this long is dropped (3 days).
pub const GIVE_UP_MS: u64 = 3 * 24 * 3_600_000;

const DAY_MS: i64 = 86_400_000;

// ---- when ----------------------------------------------------------------------------------

/// `"23:00"` as minutes after midnight.
pub fn parse_at(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60 && s.trim().len() <= 5).then_some(h * 60 + m)
}

/// Is a daily job at `at_min` (local minutes after midnight) due? It is when its latest start
/// time has passed since it last ran. `tz_min` is the local offset from UTC in minutes.
pub fn due_at(last_run_ms: u64, now_ms: u64, at_min: u32, tz_min: i32) -> bool {
    let off = i64::from(tz_min) * 60_000;
    let local_now = now_ms as i64 + off;
    let mut start = local_now.div_euclid(DAY_MS) * DAY_MS + i64::from(at_min) * 60_000;
    if start > local_now {
        start -= DAY_MS;
    }
    (last_run_ms as i64) < start - off
}

// ---- the path rule ---------------------------------------------------------------------------

/// `on.pull_request.paths` of a workflow file: the one list of what the workflow covers. A small
/// indentation-aware reader of the block-list form. An empty or missing list is an error:
/// reading it as "everything is relevant" would dispatch every night regardless.
pub fn relevant_globs(yml: &str) -> Result<Vec<String>, String> {
    let mut path: Vec<(usize, String)> = vec![];
    let mut found = vec![];
    for raw in yml.lines() {
        let line = raw.split(" #").next().unwrap_or("").trim_end();
        let text = line.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if let Some(item) = text.strip_prefix("- ") {
            if path
                .iter()
                .map(|(_, k)| k.as_str())
                .eq(["on", "pull_request", "paths"])
            {
                found.push(item.trim().trim_matches(['\'', '"']).to_string());
            }
            continue;
        }
        while path.last().is_some_and(|(i, _)| *i >= indent) {
            path.pop();
        }
        path.push((
            indent,
            text.split(':').next().unwrap_or("").trim().to_string(),
        ));
    }
    if found.is_empty() {
        return Err("no on.pull_request.paths list".into());
    }
    Ok(found)
}

fn glob(g: &[u8], p: &[u8]) -> bool {
    match g.first() {
        None => p.is_empty(),
        Some(b'*') if g.starts_with(b"**") => {
            let rest = &g[2..];
            // `a/**/b` also matches `a/b`.
            if rest.first() == Some(&b'/') && glob(&rest[1..], p) {
                return true;
            }
            (0..=p.len()).any(|i| glob(rest, &p[i..]))
        }
        Some(b'*') => {
            let rest = &g[1..];
            for i in 0..=p.len() {
                if glob(rest, &p[i..]) {
                    return true;
                }
                if i < p.len() && p[i] == b'/' {
                    break;
                }
            }
            false
        }
        Some(b'?') => !p.is_empty() && p[0] != b'/' && glob(&g[1..], &p[1..]),
        Some(c) => p.first() == Some(c) && glob(&g[1..], &p[1..]),
    }
}

/// GitHub path-filter semantics: `**` crosses `/`, `*` does not.
pub fn path_matches(path: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| glob(g.as_bytes(), path.as_bytes()))
}

// ---- deciding --------------------------------------------------------------------------------

/// One run of the scheduled workflow on the base branch, as the decision needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunInfo {
    pub id: u64,
    pub sha: String,
    pub created_ms: u64,
    pub completed: bool,
    pub conclusion: Option<String>,
    pub event: String,
    /// The run's display title: the run-name the dispatch gave it.
    pub title: String,
    pub attempt: u32,
}

/// Is this run one the schedule dispatched? With a run-name, its title is that; without one,
/// any manual dispatch counts.
pub fn is_mine(r: &RunInfo, run_name: &str) -> bool {
    if run_name.is_empty() {
        r.event == "workflow_dispatch"
    } else {
        r.title == run_name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub dispatch: bool,
    pub reason: String,
}

fn short(sha: &str) -> &str {
    sha.get(..10).unwrap_or(sha)
}

/// Dispatch or skip. `runs` are the workflow's runs on the base branch; `globs` is `None` when the
/// schedule has no path rule (any change is relevant); `changed(sha)` lists the files that differ
/// from `sha` to the tip, `None` when the commit is not known locally.
pub fn decide(
    runs: &[RunInfo],
    run_name: &str,
    tip: &str,
    globs: Option<&[String]>,
    changed: &dyn Fn(&str) -> Option<Vec<String>>,
) -> Decision {
    let mut mine: Vec<&RunInfo> = runs.iter().filter(|r| is_mine(r, run_name)).collect();
    mine.sort_by_key(|r| std::cmp::Reverse(r.id));
    let out = |dispatch, reason: String| Decision { dispatch, reason };
    if let Some(r) = mine.iter().find(|r| !r.completed) {
        return out(false, format!("run {} is already in flight", r.id));
    }
    let finished = |r: &&&RunInfo| {
        r.completed && matches!(r.conclusion.as_deref(), Some("success" | "failure"))
    };
    if let Some(r) = mine.iter().find(finished) {
        if r.sha == tip {
            return out(
                false,
                format!(
                    "the tip {} was already tested (run {}: {})",
                    short(tip),
                    r.id,
                    r.conclusion.as_deref().unwrap_or("?")
                ),
            );
        }
    }
    let Some(green) = mine
        .iter()
        .find(|r| r.completed && r.conclusion.as_deref() == Some("success"))
    else {
        return out(true, "no green run yet".into());
    };
    let Some(files) = changed(&green.sha) else {
        return out(
            true,
            format!(
                "the last green run's commit {} is not in the repository; cannot tell what changed",
                short(&green.sha)
            ),
        );
    };
    let hits: Vec<&String> = files
        .iter()
        .filter(|f| globs.is_none_or(|g| path_matches(f, g)))
        .collect();
    match hits.first() {
        None => out(
            false,
            format!(
                "nothing relevant changed since the last green run {} ({} files, none match)",
                short(&green.sha),
                files.len()
            ),
        ),
        Some(first) => out(
            true,
            format!(
                "{} relevant file(s) changed since the last green run {}, e.g. {first}",
                hits.len(),
                short(&green.sha)
            ),
        ),
    }
}

// ---- grouping job names ------------------------------------------------------------------------

/// How a job name splits into the part that is filed once (the group) and the leg it ran on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GroupBy {
    /// Every job is its own group.
    #[default]
    Job,
    /// `<group>/<leg>` or `<leg> · <group>`: split at the first separator.
    Split { group_first: bool, sep: String },
}

impl GroupBy {
    /// Read a template such as `<group>/<leg>`.
    pub fn parse(t: &str) -> Result<GroupBy, String> {
        let (g, l) = (t.find("<group>"), t.find("<leg>"));
        let (Some(g), Some(l)) = (g, l) else {
            return Err(
                "group_by is a template with <group> and <leg>, such as \"<group>/<leg>\"".into(),
            );
        };
        let (first, second) = (g.min(l), g.max(l));
        let (a, b) = if g < l { (7, 5) } else { (5, 7) };
        let sep = &t[first + a..second];
        if first != 0 || second + b != t.len() || sep.is_empty() {
            return Err("group_by must be <group><separator><leg> or the other way round, with nothing else around them".into());
        }
        Ok(GroupBy::Split {
            group_first: g < l,
            sep: sep.to_string(),
        })
    }

    /// `(leg, group)` of a job name. A name with no separator is its own group on no leg.
    pub fn split(&self, name: &str) -> (String, String) {
        match self {
            GroupBy::Job => (String::new(), name.trim().to_string()),
            GroupBy::Split { group_first, sep } => match name.split_once(sep.as_str()) {
                Some((a, b)) if *group_first => (b.trim().to_string(), a.trim().to_string()),
                Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
                None => (String::new(), name.trim().to_string()),
            },
        }
    }
}

// ---- judging the jobs ------------------------------------------------------------------------------

/// A job that failed on the first attempt, captured before the rerun replaces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    pub job: String,
    pub conclusion: String,
    pub log: String,
    /// Why it looks like infrastructure, if it does.
    pub infra: Option<String>,
}

/// The same job on the rerun.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Second {
    pub job: String,
    pub done: bool,
    pub conclusion: Option<String>,
    pub log: String,
    pub infra: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Red,
    Flaky,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Red on both attempts.
    Red,
    /// Red, then green.
    Flaky,
    /// Cancelled or judged infrastructure: rerun, never filed.
    Infra(String),
    /// The rerun did not finish for this job.
    Unfinished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judged {
    pub job: String,
    pub outcome: Outcome,
    pub first_log: String,
    pub second_log: String,
}

fn green(c: &str) -> bool {
    matches!(c, "success" | "neutral" | "skipped")
}

/// Judge each job that failed on the first attempt by what it did on the rerun.
pub fn classify(first: &[Failed], second: &[Second]) -> Vec<Judged> {
    first
        .iter()
        .map(|f| {
            let s = second.iter().find(|s| s.job == f.job);
            let outcome = match s {
                None => Outcome::Unfinished,
                Some(s) if !s.done => Outcome::Unfinished,
                _ if f.conclusion == "cancelled" => Outcome::Infra("cancelled".into()),
                Some(s) => {
                    let c = s.conclusion.as_deref().unwrap_or("");
                    if c == "cancelled" {
                        Outcome::Infra("cancelled on the rerun".into())
                    } else if !green(c) {
                        match &s.infra {
                            Some(why) => Outcome::Infra(why.clone()),
                            None => Outcome::Red,
                        }
                    } else if let Some(why) = &f.infra {
                        Outcome::Infra(why.clone())
                    } else {
                        Outcome::Flaky
                    }
                }
            };
            Judged {
                job: f.job.clone(),
                outcome,
                first_log: f.log.clone(),
                second_log: s.map(|s| s.log.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leg {
    pub leg: String,
    pub job: String,
    pub first_log: String,
    pub second_log: String,
}

/// What is filed once: all the legs of one group that went the same way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: Kind,
    pub group: String,
    pub legs: Vec<Leg>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grouped {
    /// Reds first, then flaky, each in the order the jobs were listed.
    pub groups: Vec<Group>,
    pub infra: usize,
    pub unfinished: usize,
}

pub fn group(judged: &[Judged], by: &GroupBy) -> Grouped {
    let mut out = Grouped::default();
    for kind in [Kind::Red, Kind::Flaky] {
        for j in judged {
            let want = matches!(
                (&j.outcome, kind),
                (Outcome::Red, Kind::Red) | (Outcome::Flaky, Kind::Flaky)
            );
            if !want {
                continue;
            }
            let (leg, g) = by.split(&j.job);
            let leg = Leg {
                leg,
                job: j.job.clone(),
                first_log: j.first_log.clone(),
                second_log: j.second_log.clone(),
            };
            match out
                .groups
                .iter_mut()
                .find(|x| x.kind == kind && x.group == g)
            {
                Some(x) => x.legs.push(leg),
                None => out.groups.push(Group {
                    kind,
                    group: g,
                    legs: vec![leg],
                }),
            }
        }
    }
    for j in judged {
        match j.outcome {
            Outcome::Infra(_) => out.infra += 1,
            Outcome::Unfinished => out.unfinished += 1,
            _ => {}
        }
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub file: Vec<Group>,
    /// Already named by an open task or issue: not filed again.
    pub covered: Vec<Group>,
    /// Over the cap.
    pub left: Vec<Group>,
}

/// Choose what to file: a covered group is skipped without using the cap; at most `max` of the
/// rest are filed, reds before flakes (the order `groups` is in).
pub fn select(groups: Vec<Group>, max: usize, covered: &dyn Fn(&Group) -> bool) -> Selection {
    let mut s = Selection::default();
    let mut ordered = groups;
    ordered.sort_by_key(|g| g.kind == Kind::Flaky);
    for g in ordered {
        if covered(&g) {
            s.covered.push(g);
        } else if s.file.len() < max {
            s.file.push(g);
        } else {
            s.left.push(g);
        }
    }
    s
}

// ---- following a run ---------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Dispatched; the run has not shown up in the list yet.
    Locate,
    /// Waiting for the first attempt to finish.
    Await1,
    /// The failed jobs were rerun; waiting for the second attempt.
    Await2,
}

/// The run being followed, kept between ticks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Follow {
    pub run_id: u64,
    pub phase: Phase,
    /// When it was dispatched.
    pub at: u64,
    /// The runs that existed before the dispatch (to tell the new one apart).
    pub before: Vec<u64>,
    pub failures: Vec<Failed>,
}

/// What the host says about the followed run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub completed: bool,
    pub attempt: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do yet.
    Wait,
    /// Never moved within the limit: forget it.
    Drop,
    /// Somebody reran it by hand: the first attempt's failures are gone.
    ByHand,
    /// The first attempt finished: look at its jobs, rerun the failed ones.
    Check,
    /// The rerun finished: judge and file.
    Classify,
}

pub fn step(f: &Follow, run: Option<Seen>, now: u64) -> Step {
    let stale = || {
        if now.saturating_sub(f.at) > GIVE_UP_MS {
            Step::Drop
        } else {
            Step::Wait
        }
    };
    match (f.phase, run) {
        (_, None)
        | (
            _,
            Some(Seen {
                completed: false, ..
            }),
        ) => stale(),
        (Phase::Locate, Some(_)) => Step::Wait,
        (Phase::Await1, Some(s)) if s.attempt != 1 => Step::ByHand,
        (Phase::Await1, Some(_)) => Step::Check,
        (Phase::Await2, Some(s)) if s.attempt < 2 => stale(),
        (Phase::Await2, Some(_)) => Step::Classify,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: u64, sha: &str, done: bool, concl: &str, title: &str) -> RunInfo {
        RunInfo {
            id,
            sha: sha.into(),
            completed: done,
            conclusion: (!concl.is_empty()).then(|| concl.to_string()),
            event: "workflow_dispatch".into(),
            title: title.into(),
            attempt: 1,
            ..Default::default()
        }
    }

    fn globs() -> Vec<String> {
        vec!["src/**".into(), "Cargo.toml".into()]
    }

    #[test]
    fn decide_follows_the_rule() {
        let g = globs();
        let files = |v: &'static [&'static str]| {
            move |_: &str| Some(v.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        let d = |runs: &[RunInfo], tip: &str, f: &dyn Fn(&str) -> Option<Vec<String>>| {
            decide(runs, "Full", tip, Some(&g), f)
        };
        // in flight: skip
        let r = [
            run(2, "a", false, "", "Full"),
            run(1, "z", true, "success", "Full"),
        ];
        assert!(!d(&r, "b", &files(&["src/x.rs"])).dispatch);
        // the newest finished run was on the tip: skip, even when red
        let r = [
            run(2, "tip", true, "failure", "Full"),
            run(1, "z", true, "success", "Full"),
        ];
        let x = d(&r, "tip", &files(&["src/x.rs"]));
        assert!(!x.dispatch && x.reason.contains("already tested"), "{x:?}");
        // never green: dispatch
        let r = [run(1, "a", true, "failure", "Full")];
        assert!(d(&r, "tip", &files(&[])).dispatch);
        assert!(d(&[], "tip", &files(&[])).dispatch);
        // a cancelled run tested nothing: not "the tip was tested"
        let r = [run(2, "tip", true, "cancelled", "Full")];
        assert!(d(&r, "tip", &files(&[])).dispatch);
        // another workflow run (other run-name) is not this schedule's
        let r = [
            run(2, "tip", false, "", "Subset"),
            run(1, "z", true, "success", "Full"),
        ];
        assert!(!d(&r, "tip2", &files(&["docs/a.md"])).dispatch);
        // changed files outside the globs: skip; inside: dispatch
        let r = [run(1, "green", true, "success", "Full")];
        let x = d(&r, "tip", &files(&["docs/a.md", "README.md"]));
        assert!(!x.dispatch && x.reason.contains("none match"), "{x:?}");
        let x = d(&r, "tip", &files(&["docs/a.md", "src/deep/b.rs"]));
        assert!(x.dispatch && x.reason.contains("src/deep/b.rs"), "{x:?}");
        // the green commit unknown here: cannot tell, dispatch
        assert!(d(&r, "tip", &|_| None).dispatch);
        // no path rule: any change counts, no change does not
        assert!(decide(&r, "Full", "tip", None, &files(&["x"])).dispatch);
        assert!(!decide(&r, "Full", "tip", None, &files(&[])).dispatch);
    }

    #[test]
    fn globs_are_read_from_a_workflow_and_an_empty_list_is_refused() {
        let yml = "name: smoke\non:\n  pull_request:\n    paths:\n      - 'src/**'  # code\n      - \"Cargo.toml\"\n  push:\n    paths:\n      - other/**\njobs:\n  a:\n    steps:\n      - run: x\n";
        assert_eq!(relevant_globs(yml).unwrap(), vec!["src/**", "Cargo.toml"]);
        assert!(relevant_globs("on:\n  pull_request:\n    branches: [main]\n").is_err());
        assert!(relevant_globs("on:\n  push:\n    paths:\n      - a\n").is_err());
        assert!(relevant_globs("").is_err());
    }

    #[test]
    fn globs_follow_github_semantics() {
        let m = |g: &str, p: &str| path_matches(p, &[g.to_string()]);
        assert!(m("src/**", "src/a/b.rs"));
        assert!(m("src/*.rs", "src/a.rs"));
        assert!(!m("src/*.rs", "src/a/b.rs"));
        assert!(m("a/**/b", "a/b"));
        assert!(m("a/**/b", "a/x/y/b"));
        assert!(m("**/*.md", "docs/x/y.md"));
        assert!(!m("src/**", "docs/src/a"));
        assert!(m("Cargo.toml", "Cargo.toml"));
        assert!(!m("Cargo.toml", "Cargo.lock"));
        assert!(m("a?c", "abc") && !m("a?c", "a/c"));
    }

    #[test]
    fn at_times_and_the_daily_due_rule() {
        assert_eq!(parse_at("23:00"), Some(1380));
        assert_eq!(parse_at("0:05"), Some(5));
        for bad in ["24:00", "23:60", "2300", "", "a:b", "23:00:00"] {
            assert_eq!(parse_at(bad), None, "{bad}");
        }
        let h = 3_600_000u64;
        let day = 24 * h;
        // UTC: 23:00 on day 10 is 10*day + 23h.
        let at = 23 * 60;
        // last ran the morning of day 10, now 22:59: not due; 23:00: due
        assert!(!due_at(
            10 * day + 8 * h,
            10 * day + 22 * h + 59 * 60_000,
            at,
            0
        ));
        assert!(due_at(10 * day + 8 * h, 10 * day + 23 * h, at, 0));
        // ran at 23:01 itself: not due again until tomorrow
        assert!(!due_at(
            10 * day + 23 * h + 60_000,
            11 * day + 22 * h,
            at,
            0
        ));
        assert!(due_at(10 * day + 23 * h + 60_000, 11 * day + 23 * h, at, 0));
        // a long-asleep job runs once, then waits
        assert!(due_at(3 * day, 10 * day + 5 * h, at, 0));
        // at +02:00, 23:00 local is 21:00 UTC
        assert!(!due_at(10 * day, 10 * day + 20 * h, at, 120));
        assert!(due_at(10 * day, 10 * day + 21 * h, at, 120));
        // at -05:00, 23:00 local is 04:00 UTC the next day
        assert!(!due_at(10 * day + 5 * h, 11 * day + 3 * h, at, -300));
        assert!(due_at(10 * day + 3 * h, 11 * day + 4 * h, at, -300));
    }

    #[test]
    fn job_names_split_by_the_template() {
        let by = GroupBy::parse("<group>/<leg>").unwrap();
        assert_eq!(by.split("feat-a/linux"), ("linux".into(), "feat-a".into()));
        assert_eq!(by.split("lonely"), ("".into(), "lonely".into()));
        let by = GroupBy::parse("<leg> · <group>").unwrap();
        assert_eq!(
            by.split("mac · shard 1: a"),
            ("mac".into(), "shard 1: a".into())
        );
        for bad in [
            "",
            "<group>",
            "x<group>/<leg>",
            "<group><leg>",
            "<group>/<leg>y",
        ] {
            assert!(GroupBy::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(GroupBy::Job.split("a/b"), ("".into(), "a/b".into()));
    }

    fn f(job: &str, c: &str) -> Failed {
        Failed {
            job: job.into(),
            conclusion: c.into(),
            log: format!("first {job}"),
            infra: None,
        }
    }

    fn s(job: &str, c: &str) -> Second {
        Second {
            job: job.into(),
            done: !c.is_empty(),
            conclusion: (!c.is_empty()).then(|| c.to_string()),
            log: format!("second {job}"),
            infra: None,
        }
    }

    #[test]
    fn jobs_are_red_flaky_infrastructure_or_unfinished() {
        let first = [
            f("a/x", "failure"),
            f("a/y", "failure"),
            f("b/x", "failure"),
            f("c/x", "cancelled"),
            f("d/x", "failure"),
            f("e/x", "failure"),
        ];
        let mut e = s("e/x", "failure");
        e.infra = Some("runner or network trouble".into());
        let second = [
            s("a/x", "failure"),
            s("a/y", "failure"),
            s("b/x", "success"),
            s("c/x", "success"),
            s("d/x", ""),
            e,
        ];
        let j = classify(&first, &second);
        let kinds: Vec<&Outcome> = j.iter().map(|j| &j.outcome).collect();
        assert_eq!(kinds[0], &Outcome::Red);
        assert_eq!(kinds[1], &Outcome::Red);
        assert_eq!(kinds[2], &Outcome::Flaky);
        assert!(matches!(kinds[3], Outcome::Infra(_)), "cancelled first");
        assert_eq!(kinds[4], &Outcome::Unfinished);
        assert!(matches!(kinds[5], Outcome::Infra(_)));
        let g = group(&j, &GroupBy::parse("<group>/<leg>").unwrap());
        assert_eq!((g.infra, g.unfinished), (2, 1));
        assert_eq!(g.groups.len(), 2);
        assert_eq!(
            (g.groups[0].kind, g.groups[0].group.as_str()),
            (Kind::Red, "a")
        );
        assert_eq!(g.groups[0].legs.len(), 2, "legs grouped");
        assert_eq!(g.groups[0].legs[0].second_log, "second a/x");
        assert_eq!(
            (g.groups[1].kind, g.groups[1].group.as_str()),
            (Kind::Flaky, "b")
        );
        // red, then green after an infrastructure-looking first failure is not flaky
        let mut inf = f("z/x", "failure");
        inf.infra = Some("network".into());
        let j = classify(&[inf], &[s("z/x", "success")]);
        assert!(matches!(j[0].outcome, Outcome::Infra(_)));
    }

    fn grp(kind: Kind, name: &str) -> Group {
        Group {
            kind,
            group: name.into(),
            legs: vec![],
        }
    }

    #[test]
    fn the_cap_keeps_reds_first_and_covered_groups_do_not_use_it() {
        let groups = vec![
            grp(Kind::Flaky, "f1"),
            grp(Kind::Red, "r1"),
            grp(Kind::Flaky, "f2"),
            grp(Kind::Red, "r2"),
            grp(Kind::Red, "r3"),
        ];
        let sel = select(groups, 2, &|g| g.group == "r1");
        let names = |v: &[Group]| v.iter().map(|g| g.group.clone()).collect::<Vec<_>>();
        assert_eq!(names(&sel.covered), ["r1"]);
        assert_eq!(names(&sel.file), ["r2", "r3"]);
        assert_eq!(names(&sel.left), ["f1", "f2"]);
    }

    #[test]
    fn a_followed_run_moves_and_is_dropped_after_three_days() {
        let mut fl = Follow {
            run_id: 5,
            phase: Phase::Await1,
            at: 1_000,
            before: vec![],
            failures: vec![],
        };
        let done = |a| {
            Some(Seen {
                completed: true,
                attempt: a,
            })
        };
        let going = Some(Seen {
            completed: false,
            attempt: 1,
        });
        assert_eq!(step(&fl, going, 2_000), Step::Wait);
        assert_eq!(step(&fl, done(1), 2_000), Step::Check);
        assert_eq!(step(&fl, done(2), 2_000), Step::ByHand);
        let late = 1_000 + GIVE_UP_MS + 1;
        assert_eq!(step(&fl, going, 1_000 + GIVE_UP_MS), Step::Wait);
        assert_eq!(step(&fl, going, late), Step::Drop);
        assert_eq!(step(&fl, None, late), Step::Drop);
        assert_eq!(step(&fl, None, 2_000), Step::Wait);
        fl.phase = Phase::Await2;
        assert_eq!(
            step(&fl, done(1), 2_000),
            Step::Wait,
            "the rerun has not started"
        );
        assert_eq!(step(&fl, done(1), late), Step::Drop);
        assert_eq!(step(&fl, done(2), late), Step::Classify);
        fl.phase = Phase::Locate;
        assert_eq!(step(&fl, None, late), Step::Drop);
    }
}
