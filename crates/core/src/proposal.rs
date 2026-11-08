// SPDX-License-Identifier: GPL-3.0-or-later

//! The rules for proposals (Part 13), as pure functions: what counts as a
//! duplicate, what counts as evidence, what a source and severity decide, which
//! caps apply, where a proposal is routed, and how text is redacted before it
//! leaves the machine. No I/O and no clock: the daemon passes everything in.

use std::collections::BTreeSet;

/// Where a proposal came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// An agent working on a task, via `file_proposal`.
    Agent,
    /// Someone who is not the owner, through the tracker.
    Outside,
    /// The owner's own "Improve the pipeline" box or command.
    Owner,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Agent => "agent",
            Source::Outside => "outside",
            Source::Owner => "owner",
        }
    }
    pub fn parse(s: &str) -> Option<Source> {
        match s {
            "agent" => Some(Source::Agent),
            "outside" => Some(Source::Outside),
            "owner" => Some(Source::Owner),
            _ => None,
        }
    }
}

/// How much it matters. An agent says `blocks_task` when its own task cannot finish
/// without the fix; everything else it files is `normal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Normal,
    Blocks,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Normal => "normal",
            Severity::Blocks => "blocks",
        }
    }
    pub fn parse(s: &str) -> Option<Severity> {
        match s {
            "normal" => Some(Severity::Normal),
            "blocks" => Some(Severity::Blocks),
            _ => None,
        }
    }
}

/// What the policy decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Waits for the owner.
    Proposed,
    /// Runs.
    Queued,
}

/// The policy table: one disposition per source and severity. The defaults file
/// everything from agents and outsiders for the owner, except what blocks the
/// filing agent's own task, and run what the owner wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub agent_normal: Disposition,
    pub agent_blocks: Disposition,
    pub outside_normal: Disposition,
    pub outside_blocks: Disposition,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            agent_normal: Disposition::Proposed,
            agent_blocks: Disposition::Queued,
            outside_normal: Disposition::Proposed,
            outside_blocks: Disposition::Proposed,
        }
    }
}

impl Policy {
    pub fn decide(&self, source: Source, severity: Severity) -> Disposition {
        match (source, severity) {
            (Source::Owner, _) => Disposition::Queued,
            (Source::Agent, Severity::Normal) => self.agent_normal,
            (Source::Agent, Severity::Blocks) => self.agent_blocks,
            (Source::Outside, Severity::Normal) => self.outside_normal,
            (Source::Outside, Severity::Blocks) => self.outside_blocks,
        }
    }
}

pub fn parse_disposition(s: &str) -> Option<Disposition> {
    match s {
        "proposed" => Some(Disposition::Proposed),
        "queued" => Some(Disposition::Queued),
        _ => None,
    }
}

/// How many proposals one task, and one project in a day, may file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub per_task: u32,
    pub per_day: u32,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps {
            per_task: 3,
            per_day: 20,
        }
    }
}

impl Caps {
    /// `Err(why)` if one more filing would go over a cap.
    pub fn check(&self, by_this_task: u32, in_last_day: u32) -> Result<(), String> {
        if by_this_task >= self.per_task {
            return Err(format!(
                "this task has already filed {by_this_task} proposals and the limit is {}; put the rest in your final summary",
                self.per_task
            ));
        }
        if in_last_day >= self.per_day {
            return Err(format!(
                "this project has had {in_last_day} proposals in the last 24 hours and the limit is {}; try again tomorrow",
                self.per_day
            ));
        }
        Ok(())
    }
}

/// The words of a text, lower-cased, without punctuation and without words too
/// short or too common to say anything.
fn words(s: &str) -> BTreeSet<String> {
    const STOP: [&str; 14] = [
        "the", "a", "an", "and", "or", "of", "to", "in", "is", "on", "for", "with", "it", "this",
    ];
    s.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| !w.is_empty() && !STOP.contains(&w.as_str()))
        .collect()
}

/// A title in its normal form, for the dedupe key.
pub fn normalise(title: &str) -> String {
    words(title).into_iter().collect::<Vec<_>>().join(" ")
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    a.intersection(b).count() as f64 / a.union(b).count() as f64
}

/// Two proposals are the same if their titles share at least 60% of their words.
/// Titles that differ are compared again on the first 400 characters of the body,
/// so a reworded title over the same problem is still caught, but only when the
/// title also shares a good part of its words (at least 40%).
pub fn similar(title_a: &str, body_a: &str, title_b: &str, body_b: &str) -> bool {
    let (ta, tb) = (words(title_a), words(title_b));
    let t = jaccard(&ta, &tb);
    if t >= 0.6 {
        return true;
    }
    if t < 0.4 {
        return false;
    }
    let cut = |s: &str| s.chars().take(400).collect::<String>();
    jaccard(&words(&cut(body_a)), &words(&cut(body_b))) >= 0.6
}

/// Evidence has to point at something: a path, a file with an extension, or a
/// `:<line>` reference, and be long enough to say something.
pub fn evidence_ok(text: &str) -> Result<(), String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(
            "evidence is empty: say what you saw, with the file and line or the output".into(),
        );
    }
    if t.chars().count() < 20 {
        return Err(
            "evidence is too short to check: give the file and line, or the output you saw".into(),
        );
    }
    if locators(t).is_empty() {
        return Err(
            "evidence names no file: give a path, `file.ext` or `file:line` that shows the problem"
                .into(),
        );
    }
    Ok(())
}

/// The path-like tokens in a text: they contain a `/`, or end in a short
/// extension, or carry a `:<line>` suffix.
pub fn locators(text: &str) -> Vec<String> {
    let mut out = vec![];
    for raw in text.split(|c: char| c.is_whitespace() || "`'\"()[],;<>".contains(c)) {
        let tok = raw.trim_matches(|c: char| ".:!?".contains(c));
        if tok.len() < 3 || tok.contains("://") {
            continue;
        }
        let (name, line) = match tok.rsplit_once(':') {
            Some((n, l)) if !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()) => (n, true),
            _ => (tok, false),
        };
        let has_ext = name.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
                && stem
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c))
        });
        let has_slash = name.contains('/') && name.chars().any(|c| c.is_ascii_alphanumeric());
        if has_ext || has_slash || (line && name.chars().any(|c| c.is_ascii_alphabetic())) {
            out.push(name.trim_start_matches("./").to_string());
        }
    }
    out
}

/// Where a proposal goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// The project the agent was working in.
    Project,
    /// MASON's own repository (the self project).
    Pipeline,
}

impl Dest {
    pub fn name(self) -> &'static str {
        match self {
            Dest::Project => "project",
            Dest::Pipeline => "pipeline",
        }
    }
}

/// Dest by the paths the evidence cites. It goes to the pipeline only if every
/// cited path is a file of MASON's repository and none exists in the filing
/// project; one path of the project's is enough to keep it there. With nothing
/// cited, or nothing recognised, it stays in the project.
pub fn route(
    cited: &[String],
    in_pipeline_repo: impl Fn(&str) -> bool,
    in_project_repo: impl Fn(&str) -> bool,
) -> Dest {
    if cited.is_empty() {
        return Dest::Project;
    }
    if cited.iter().any(|p| in_project_repo(p)) {
        return Dest::Project;
    }
    if cited.iter().all(|p| in_pipeline_repo(p)) {
        Dest::Pipeline
    } else {
        Dest::Project
    }
}

/// Take out what must not reach a public place: each client name, each client
/// path root (and the home directory), whatever looks like a file path of a client
/// project, and anything that looks like a secret. Whole lines carrying a client's
/// code (fenced blocks) are dropped. Returns the text and how many things went.
pub fn redact(text: &str, client_names: &[String], client_roots: &[String]) -> (String, usize) {
    let mut n = 0;
    let mut out = String::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            if fenced {
                out.push_str("[code block removed]\n");
                n += 1;
            }
            continue;
        }
        if fenced {
            continue;
        }
        let mut l = line.to_string();
        for r in client_roots.iter().filter(|r| r.len() >= 3) {
            n += l.matches(r.as_str()).count();
            l = l.replace(r.as_str(), "[client-path]");
        }
        for name in client_names.iter().filter(|c| !c.trim().is_empty()) {
            let lower = l.to_lowercase();
            let needle = name.to_lowercase();
            let mut res = String::new();
            let mut at = 0;
            while let Some(i) = lower[at..].find(&needle) {
                res.push_str(&l[at..at + i]);
                res.push_str("[client]");
                at += i + needle.len();
                n += 1;
            }
            res.push_str(&l[at..]);
            // Lower-casing can shift byte offsets for rare characters; fall back whole.
            l = if lower.len() == l.len() {
                res
            } else {
                l.replace(name.as_str(), "[client]")
            };
        }
        let words: Vec<String> = l
            .split(' ')
            .map(|w| {
                let t = w.trim_matches(|c: char| "`'\"(),;".contains(c));
                let secret = t.len() >= 20
                    && t.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
                    && t.chars().any(|c| c.is_ascii_digit())
                    && t.chars().any(|c| c.is_ascii_alphabetic());
                if secret || t.starts_with("ghp_") || t.starts_with("sk-") {
                    n += 1;
                    "[secret]".to_string()
                } else if t.contains("[client-path]") {
                    // The rest of a client path (its file and folders) goes with the root.
                    "[client-path]".to_string()
                } else if w
                    .trim_matches(|c: char| "`'\",;".contains(c))
                    .ends_with("()")
                    && t.len() > 1
                {
                    // A call such as `chargeCard()` is client code, not prose.
                    n += 1;
                    "[code]".to_string()
                } else if t.starts_with("/Users/")
                    || t.starts_with("/home/")
                    || t.starts_with("/Volumes/")
                {
                    n += 1;
                    "[path]".to_string()
                } else {
                    w.to_string()
                }
            })
            .collect();
        out.push_str(&words.join(" "));
        out.push('\n');
    }
    (out.trim_end().to_string(), n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_title_is_a_duplicate_and_different_is_not() {
        assert!(similar(
            "Flaky test in the landing queue",
            "",
            "flaky test in landing queue!",
            ""
        ));
        assert!(!similar(
            "Flaky test in the landing queue",
            "x",
            "Document the CLI flags",
            "y"
        ));
    }

    #[test]
    fn reworded_title_with_the_same_body_is_caught() {
        let body =
            "the landing queue test fails one run in ten because it sleeps instead of waiting";
        assert!(similar(
            "Landing queue flaky timing",
            body,
            "Flaky landing queue slow test",
            body
        ));
        assert!(!similar(
            "Landing queue flaky timing",
            body,
            "Flaky landing queue slow test",
            "different words entirely here"
        ));
    }

    #[test]
    fn evidence_needs_a_locator() {
        assert!(evidence_ok("").is_err());
        assert!(evidence_ok("   \n ").is_err());
        assert!(evidence_ok("it seems to be broken somehow").is_err());
        assert!(evidence_ok("a.b").is_err(), "too short");
        assert!(evidence_ok("crates/daemon/src/jobs.rs:120 unwrap on a closed channel").is_ok());
        assert!(evidence_ok("the panic message came from main.rs when run twice").is_ok());
        assert!(evidence_ok("see https://example.com for details and more details").is_err());
        assert!(evidence_ok("the output at src:42 shows it dying at once").is_ok());
    }

    #[test]
    fn policy_defaults_and_overrides() {
        let p = Policy::default();
        assert_eq!(
            p.decide(Source::Agent, Severity::Normal),
            Disposition::Proposed
        );
        assert_eq!(
            p.decide(Source::Agent, Severity::Blocks),
            Disposition::Queued
        );
        assert_eq!(
            p.decide(Source::Outside, Severity::Blocks),
            Disposition::Proposed
        );
        assert_eq!(
            p.decide(Source::Owner, Severity::Normal),
            Disposition::Queued
        );
        let q = Policy {
            agent_normal: Disposition::Queued,
            ..p
        };
        assert_eq!(
            q.decide(Source::Agent, Severity::Normal),
            Disposition::Queued
        );
    }

    #[test]
    fn caps_stop_at_the_limit() {
        let c = Caps::default();
        assert!(c.check(2, 19).is_ok());
        assert!(c.check(3, 0).unwrap_err().contains("limit is 3"));
        assert!(c.check(0, 20).unwrap_err().contains("limit is 20"));
    }

    #[test]
    fn routing_follows_the_cited_paths() {
        let mason = |p: &str| p.starts_with("crates/");
        let proj = |p: &str| p.starts_with("app/");
        let c = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(route(&c(&["crates/a.rs"]), mason, proj), Dest::Pipeline);
        assert_eq!(
            route(&c(&["crates/a.rs", "app/x.swift"]), mason, proj),
            Dest::Project
        );
        assert_eq!(route(&c(&["app/x.swift"]), mason, proj), Dest::Project);
        assert_eq!(route(&c(&[]), mason, proj), Dest::Project);
        assert_eq!(route(&c(&["other/file.rs"]), mason, proj), Dest::Project);
    }

    #[test]
    fn redaction_removes_names_roots_code_and_secrets() {
        let text = "Acme Corp's checkout in /Users/me/git/acme/app/Pay.swift fails\n```\nlet secretFn = 1\n```\ntoken ghp_abcdefghijklmnopqrstuvwx1234 here";
        let (r, n) = redact(text, &["Acme Corp".into()], &["/Users/me/git/acme".into()]);
        assert!(!r.to_lowercase().contains("acme"), "{r}");
        assert!(!r.contains("secretFn") && !r.contains("ghp_"), "{r}");
        assert!(r.contains("[code block removed]"));
        assert!(n >= 4);
    }

    #[test]
    fn redaction_takes_the_whole_client_path_and_calls() {
        let (r, _) = redact(
            "see /work/acme/app/Pay.swift where chargeCard() wraps",
            &[],
            &["/work/acme".into()],
        );
        assert!(!r.contains("Pay.swift") && !r.contains("chargeCard"), "{r}");
        assert!(r.contains("[client-path]") && r.contains("[code]"), "{r}");
    }
}
