// SPDX-License-Identifier: GPL-3.0-or-later

//! Secrets and owner-banned words in what a change adds.
//!
//! The same shape as `markers`, and called beside it: the submit validator, `run_check` and the
//! landing job. Only the lines a change *adds* count.
//!
//! *Secrets* are a short built-in list of shapes that are rarely anything else. A line that
//! really is not one ends with a `mason:allow-secret` comment; landing records each such line
//! in the batch's log.
//!
//! *Banned words* come from files the owner keeps outside every repository, read at each scan:
//! `<config>/banned-words.txt` (`MASON_BANNED_WORDS` names another file in its place, for
//! tests) and `<config>/banned-words/<project>.txt`. One entry a line: a literal, or `re:` and
//! a regular expression, either matched without regard to case; `#` starts a comment line. The
//! comment that allows a secret does not allow a banned word.
//!
//! Nothing this module returns holds the matched text: every match in a quoted line is masked,
//! so neither a secret nor an entry of the list reaches a reply, a log or a task's feedback.

use crate::markers::{added_lines, cut, diff_args};
use mason_landing::git;
use regex::{Regex, RegexBuilder};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The comment that ends a line which looks like a secret and is not one.
pub const ALLOW: &str = "mason:allow-secret";

/// The environment variable that names a file to read in place of the global list.
pub const LIST_ENV: &str = "MASON_BANNED_WORDS";

/// The batch's file that records the lines allowed by [`ALLOW`].
pub const ALLOWED_LOG: &str = "content-scan.log";

/// How many hits a message names before it says "and N more".
const NAMED: usize = 6;

/// What stands for matched text in a quoted line.
const MASK: &str = "[…]";

/// The secret shapes. A group named `v` is the secret itself; otherwise the whole match is.
const SECRETS: &[(&str, &str)] = &[
    (
        "private key",
        r"-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY(?: BLOCK)?-----",
    ),
    ("GitHub token", r"\b(?:ghp|gho)_[A-Za-z0-9]{36,}"),
    ("GitHub token", r"\bgithub_pat_[A-Za-z0-9_]{36,}"),
    ("Anthropic API key", r"\bsk-ant-[A-Za-z0-9_-]{20,}"),
    ("AWS access key id", r"\bAKIA[0-9A-Z]{16}\b"),
    ("Slack token", r"\bxox[abpr]-[0-9][A-Za-z0-9-]{9,}"),
    (
        ASSIGNED,
        r#"(?i)(?:password|secret|token)["']?\s*(?::=|=>|==|=|:)\s*(?:"(?P<v>[^"\s]{16,})"|'(?P<w>[^'\s]{16,})')"#,
    ),
];

/// The generic shape, which needs a second look at its value.
const ASSIGNED: &str = "password, secret or token assignment";

/// Is a quoted value long enough to match [`ASSIGNED`] something that could be a secret? A
/// reference (`env:NAME`), a placeholder (`${…}`, `<…>`, `{}`) or a plain name is not.
fn looks_secret(v: &str) -> bool {
    !["env:", "file:", "keychain:"]
        .iter()
        .any(|p| v.starts_with(p))
        && !v.contains(['$', '{', '}', '<', '>'])
        && v.chars().any(|c| c.is_ascii_digit())
        && v.chars().any(|c| c.is_ascii_alphabetic())
}

/// Where the lists are: the daemon's configuration folder and the project's name.
#[derive(Clone, Debug, Default)]
pub struct Source {
    pub config_dir: PathBuf,
    pub project: String,
}

impl Source {
    pub fn new(config_dir: &Path, project: &str) -> Source {
        Source {
            config_dir: config_dir.to_path_buf(),
            project: project.to_string(),
        }
    }

    /// The patterns and the lists as they are now.
    pub fn rules(&self) -> Rules {
        let own = !self.config_dir.as_os_str().is_empty();
        let global = std::env::var_os(LIST_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| own.then(|| self.config_dir.join("banned-words.txt")));
        let project = (own && !self.project.is_empty()).then(|| {
            self.config_dir
                .join("banned-words")
                .join(format!("{}.txt", self.project))
        });
        Rules::from_files(global.into_iter().chain(project))
    }
}

/// What a scan looks for.
pub struct Rules {
    secrets: Vec<(&'static str, Regex)>,
    banned: Vec<Regex>,
    /// Lists, or entries of them, that could not be read. Named by file and line, never quoted.
    pub problems: Vec<String>,
}

impl Rules {
    /// The built-in patterns and the banned words of `files`. A file that does not exist is an
    /// empty list.
    pub fn from_files(files: impl IntoIterator<Item = PathBuf>) -> Rules {
        let secrets = SECRETS
            .iter()
            .filter_map(|(name, re)| Some((*name, Regex::new(re).ok()?)))
            .collect();
        let mut rules = Rules {
            secrets,
            banned: vec![],
            problems: vec![],
        };
        for file in files {
            match std::fs::read_to_string(&file) {
                Ok(text) => rules.add_list(&file, &text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => rules
                    .problems
                    .push(format!("{} cannot be read: {e}", file.display())),
            }
        }
        rules
    }

    fn add_list(&mut self, file: &Path, text: &str) {
        for (n, line) in text.lines().enumerate() {
            let line = line.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let pattern = match line.strip_prefix("re:") {
                Some(re) => re.trim().to_string(),
                None => regex::escape(line),
            };
            let built = RegexBuilder::new(&pattern).case_insensitive(true).build();
            match built {
                Ok(re) if !pattern.is_empty() => self.banned.push(re),
                // The error would quote the entry.
                _ => self.problems.push(format!(
                    "{} line {} is not a usable regular expression",
                    file.display(),
                    n + 1
                )),
            }
        }
    }

    /// The secret on a line, if any: its name and where the secrets are.
    fn secrets_in(&self, line: &str) -> (Option<&'static str>, Vec<Range<usize>>) {
        let (mut first, mut at) = (None, vec![]);
        for (name, re) in &self.secrets {
            for c in re.captures_iter(line) {
                let value = c.name("v").or_else(|| c.name("w"));
                if *name == ASSIGNED && !value.is_some_and(|v| looks_secret(v.as_str())) {
                    continue;
                }
                if let Some(m) = value.or_else(|| c.get(0)) {
                    first = first.or(Some(*name));
                    at.push(m.range());
                }
            }
        }
        (first, at)
    }

    fn banned_in(&self, line: &str) -> Vec<Range<usize>> {
        self.banned
            .iter()
            .flat_map(|re| re.find_iter(line))
            .filter(|m| !m.is_empty())
            .map(|m| m.range())
            .collect()
    }
}

/// What a hit is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Secret(&'static str),
    Banned,
}

/// One added line that holds a secret or a banned word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub file: String,
    /// Its line number in the changed file.
    pub line: u32,
    pub kind: Kind,
    /// The line with every match masked, cut short.
    pub text: String,
}

/// An added line that looks like a secret and says it is allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub file: String,
    pub line: u32,
    pub what: &'static str,
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub hits: Vec<Hit>,
    pub allowed: Vec<Allowed>,
}

/// Does the line end with a comment that says [`ALLOW`]?
fn is_allowed(line: &str) -> bool {
    let mut t = line.trim_end();
    for close in ["*/", "-->"] {
        t = t.strip_suffix(close).unwrap_or(t).trim_end();
    }
    t.strip_suffix(ALLOW).is_some_and(|before| {
        let before = before.trim_end();
        ["//", "#", "--", "/*", "<!--", ";"]
            .iter()
            .any(|lead| before.ends_with(lead))
    })
}

/// `line` with every range replaced by [`MASK`]; ranges that overlap are one.
fn masked(line: &str, mut at: Vec<Range<usize>>) -> String {
    at.sort_by_key(|r| r.start);
    let (mut out, mut done) = (String::new(), 0);
    for r in at {
        if r.start >= done {
            out.push_str(&line[done..r.start]);
            out.push_str(MASK);
        }
        done = done.max(r.end);
    }
    out.push_str(&line[done..]);
    out
}

/// What the lines a unified diff adds hold.
pub fn in_diff(rules: &Rules, diff: &str) -> Scan {
    let mut scan = Scan::default();
    added_lines(diff, |file, line, text| {
        let (secret, mut at) = rules.secrets_in(text);
        let banned = rules.banned_in(text);
        if secret.is_none() && banned.is_empty() {
            return;
        }
        let has_banned = !banned.is_empty();
        at.extend(banned);
        let shown = cut(&masked(text, at));
        let mut hit = |kind| {
            scan.hits.push(Hit {
                file: file.to_string(),
                line,
                kind,
                text: shown.clone(),
            })
        };
        if has_banned {
            hit(Kind::Banned);
        }
        if let Some(what) = secret {
            if is_allowed(text) {
                scan.allowed.push(Allowed {
                    file: file.to_string(),
                    line,
                    what,
                });
            } else {
                hit(Kind::Secret(what));
            }
        }
    });
    scan
}

/// What `head` adds since it left the base: the diff of `base_ref...head`.
pub fn added_since(
    rules: &Rules,
    dir: &Path,
    base_ref: &str,
    head: &str,
) -> Result<Scan, git::GitError> {
    let range = format!("{base_ref}...{head}");
    Ok(in_diff(rules, &git::run(dir, &diff_args(&range))?))
}

/// What a worktree adds since it left `base_ref`, committed or not (tracked files). Measured
/// from the merge base, so a line the base has since removed is not the change's.
pub fn added_in_worktree(rules: &Rules, dir: &Path, base_ref: &str) -> Result<Scan, git::GitError> {
    let fork = git::run(dir, &["merge-base", base_ref, "HEAD"])?;
    Ok(in_diff(rules, &git::run(dir, &diff_args(fork.trim()))?))
}

fn and_more(total: usize) -> Option<usize> {
    total.checked_sub(NAMED).filter(|n| *n > 0)
}

/// `file:line` for each hit, the first few, for a reason or a log line.
pub fn places(found: &[Hit]) -> String {
    let list: Vec<String> = found
        .iter()
        .take(NAMED)
        .map(|h| format!("{}:{}", h.file, h.line))
        .collect();
    match and_more(found.len()) {
        Some(n) => format!("{} and {n} more", list.join(", ")),
        None => list.join(", "),
    }
}

/// What to tell the agent: where the hits are, and what to do about them.
pub fn refusal(found: &[Hit]) -> String {
    let list: Vec<String> = found
        .iter()
        .take(NAMED)
        .map(|h| {
            let what = match h.kind {
                Kind::Secret(what) => what,
                Kind::Banned => "banned word",
            };
            format!("{}:{} ({what}) `{}`", h.file, h.line, h.text)
        })
        .collect();
    let more = and_more(found.len()).map_or_else(String::new, |n| format!(" (and {n} more)"));
    let mut text = format!(
        "the change adds text that must not reach git: {}{more}. Matches are shown as {MASK}. \
         Remove each one, and rewrite the commits that added it so it is not in the branch's \
         history either, then commit again.",
        list.join("; ")
    );
    if found.iter().any(|h| matches!(h.kind, Kind::Secret(_))) {
        text.push_str(&format!(
            " Never commit a real secret. A line that only looks like one (a made-up fixture) \
             may end with a comment saying `{ALLOW}`."
        ));
    }
    if found.iter().any(|h| h.kind == Kind::Banned) {
        text.push_str(
            " A banned word is on a list the owner keeps outside the repository; say it another \
             way. No comment allows it.",
        );
    }
    text
}

/// Records in `<log_dir>/content-scan.log` every line the tasks' branches add that
/// [`ALLOW`] let through, so the owner can see what a batch carried. Reporting only: the
/// verdict on each task was reached before it joined the batch. Returns what it wrote.
pub fn note_allowed<'a>(
    rules: &Rules,
    repo: &Path,
    base_ref: &str,
    tasks: impl IntoIterator<Item = (u64, &'a str)>,
    log_dir: &Path,
) -> String {
    let mut text = String::new();
    for (task, branch) in tasks {
        match added_since(rules, repo, base_ref, branch) {
            Ok(scan) => {
                for a in scan.allowed {
                    text.push_str(&format!(
                        "task #{task}: {}:{} ({}) allowed by {ALLOW}\n",
                        a.file, a.line, a.what
                    ));
                }
            }
            Err(e) => text.push_str(&format!("task #{task}: not scanned: {e}\n")),
        }
    }
    if !text.is_empty() {
        tracing::warn!("content scan: {}", text.trim_end());
        let written = std::fs::create_dir_all(log_dir)
            .and_then(|()| std::fs::write(log_dir.join(ALLOWED_LOG), &text));
        if let Err(e) = written {
            tracing::warn!("content scan log not written: {e}");
        }
    }
    text
}

/// `masond contentscan --repo D --base REF [--config-dir C] [--project P]`: the scan of what
/// `HEAD` adds, by hand. Exit 0: nothing; 1: hits; 2: it could not look.
pub fn cli(args: &[String]) -> i32 {
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    };
    let (Some(repo), Some(base)) = (value("--repo"), value("--base")) else {
        eprintln!("usage: masond contentscan --repo D --base REF [--config-dir C] [--project P]");
        return 2;
    };
    let config = value("--config-dir").map_or_else(
        || {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".config/mason"))
                .unwrap_or_default()
        },
        PathBuf::from,
    );
    let rules = Source::new(&config, value("--project").unwrap_or_default()).rules();
    let scan = match added_since(&rules, Path::new(repo), base, "HEAD") {
        Ok(scan) => scan,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    for a in &scan.allowed {
        println!("{}:{} ({}) allowed by {ALLOW}", a.file, a.line, a.what);
    }
    if !scan.hits.is_empty() {
        println!("{}", refusal(&scan.hits));
    }
    for p in &rules.problems {
        eprintln!("{p}");
    }
    match (rules.problems.is_empty(), scan.hits.is_empty()) {
        (false, _) => 2,
        (true, false) => 1,
        (true, true) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every fake is put together at run time, so this file holds none.
    fn fakes() -> Vec<(&'static str, String)> {
        let a = |n: usize| "a1".repeat(n / 2);
        vec![
            (
                "private key",
                format!("{0}BEGIN RSA PRIVATE KEY{0}", "-".repeat(5)),
            ),
            (
                "private key",
                format!("{0}BEGIN PRIVATE KEY{0}", "-".repeat(5)),
            ),
            ("GitHub token", format!("ghp_{}", a(36))),
            ("GitHub token", format!("gho_{}", a(36))),
            ("GitHub token", format!("github_pat_{}", a(60))),
            ("Anthropic API key", format!("sk-ant-{}", a(40))),
            ("AWS access key id", format!("AKIA{}", "A1".repeat(8))),
            ("Slack token", format!("xoxb-{}", "12345-".repeat(3))),
            (ASSIGNED, format!("password = \"{}\"", a(16))),
            (ASSIGNED, format!("client_secret: '{}'", a(20))),
            (ASSIGNED, format!("\"api_token\": \"{}\"", a(24))),
        ]
    }

    fn diff_of(lines: &[String]) -> String {
        format!(
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,1 @@\n{}\n",
            lines.join("\n")
        )
    }

    fn built_in() -> Rules {
        Rules::from_files([])
    }

    fn list(text: &str) -> (tempfile::TempDir, Rules) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("words.txt");
        std::fs::write(&file, text).unwrap();
        let rules = Rules::from_files([file]);
        (dir, rules)
    }

    #[test]
    fn every_built_in_pattern_compiles() {
        assert_eq!(built_in().secrets.len(), SECRETS.len());
    }

    #[test]
    fn each_secret_hits_an_added_line_and_not_a_removed_or_context_one() {
        for (what, fake) in fakes() {
            let added = in_diff(&built_in(), &diff_of(&[format!("+x {fake} y")]));
            assert_eq!(added.hits.len(), 1, "{what}: {fake}");
            assert_eq!(added.hits[0].kind, Kind::Secret(what), "{fake}");
            assert_eq!(
                (added.hits[0].file.as_str(), added.hits[0].line),
                ("f.txt", 1)
            );
            let other = in_diff(
                &built_in(),
                &diff_of(&[format!("-{fake}"), format!(" {fake}")]),
            );
            assert_eq!(other, Scan::default(), "{fake}");
        }
    }

    #[test]
    fn a_secret_is_never_quoted_back() {
        for (_, fake) in fakes() {
            let hits = in_diff(&built_in(), &diff_of(&[format!("+{fake}")])).hits;
            let told = refusal(&hits);
            // The assignments keep their key; the value is masked.
            let secret = fake.split(['"', '\'']).rev().nth(1).unwrap_or(&fake);
            assert!(!told.contains(secret), "{told}");
            assert!(told.contains(MASK) && told.contains("f.txt:1"), "{told}");
        }
    }

    #[test]
    fn what_only_looks_a_little_like_a_secret_passes() {
        let lines: Vec<String> = [
            "+token = \"env:MASON_TEST_TOKEN_2\"",
            "+password = \"short1\"",
            "+secret = \"${SECRET_FROM_THE_ENV_1}\"",
            "+token: \"the-name-of-a-token\"",
            "+secret = \"a sentence with 1 space\"",
            "+let token = read_token_from(\"/x/1\");",
            "+ghp_ is a prefix, and so is sk-ant-",
            "+AKIAshort",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(in_diff(&built_in(), &diff_of(&lines)), Scan::default());
    }

    #[test]
    fn banned_words_hit_without_regard_to_case_and_regexes_work() {
        let (_dir, rules) = list("# a comment\n\nZebra.Word\nre: QX-[0-9]+\n");
        assert!(rules.problems.is_empty(), "{:?}", rules.problems);
        let scan = in_diff(
            &rules,
            &diff_of(&[
                "+the zEBRA.wORD is here".into(),
                "+zebraXword is not it: a literal's dot is a dot".into(),
                "+see qx-1234".into(),
                "+# a comment".into(),
                "-zebra.word".into(),
                " qx-1".into(),
            ]),
        );
        let at: Vec<u32> = scan.hits.iter().map(|h| h.line).collect();
        assert_eq!(at, vec![1, 3], "{scan:?}");
        assert!(scan.hits.iter().all(|h| h.kind == Kind::Banned));
        assert_eq!(places(&scan.hits), "f.txt:1, f.txt:3");
    }

    #[test]
    fn the_refusal_never_holds_the_list() {
        let (_dir, rules) = list("zebraword\nre:qx-[0-9]+\nunused-entry\n");
        let hits = in_diff(
            &rules,
            &diff_of(&["+a ZebraWord and QX-77 together".into()]),
        )
        .hits;
        let told = refusal(&hits).to_lowercase();
        for secret in ["zebraword", "qx-", "[0-9]", "unused-entry"] {
            assert!(!told.contains(secret), "{secret}: {told}");
        }
        assert!(
            told.contains("f.txt:1") && told.contains("together"),
            "{told}"
        );
    }

    #[test]
    fn a_bad_entry_is_reported_without_being_quoted_and_the_rest_still_apply() {
        let (_dir, rules) = list("re:(unclosed-zebra\nre:\nfine-word\n");
        assert_eq!(rules.problems.len(), 2, "{:?}", rules.problems);
        assert!(rules.problems[0].contains("line 1"));
        assert!(!rules.problems.join(" ").contains("zebra"));
        assert_eq!(
            in_diff(&rules, &diff_of(&["+fine-word".into()])).hits.len(),
            1
        );
    }

    #[test]
    fn the_project_list_is_merged_with_the_global_one() {
        let dir = tempfile::tempdir().unwrap();
        let c = dir.path();
        std::fs::create_dir(c.join("banned-words")).unwrap();
        std::fs::write(c.join("banned-words.txt"), "everywhere\n").unwrap();
        std::fs::write(c.join("banned-words/mine.txt"), "only-here\n").unwrap();
        let diff = diff_of(&["+everywhere".into(), "+only-here".into()]);
        // `MASON_BANNED_WORDS` is the exit test's: no test sets it in this process.
        if std::env::var_os(LIST_ENV).is_none() {
            let mine = Source::new(c, "mine").rules();
            assert_eq!(in_diff(&mine, &diff).hits.len(), 2);
            let other = Source::new(c, "other").rules();
            assert_eq!(in_diff(&other, &diff).hits.len(), 1);
        }
    }

    #[test]
    fn the_allow_comment_lets_a_secret_through_and_records_it() {
        let fake = format!("ghp_{}", "b2".repeat(18));
        for line in [
            format!("+let t = \"{fake}\"; // {ALLOW}"),
            format!("+T={fake} # {ALLOW}"),
            format!("+/* {fake} */ /* {ALLOW} */"),
            format!("+{fake} <!-- {ALLOW} -->  "),
        ] {
            let scan = in_diff(&built_in(), &diff_of(std::slice::from_ref(&line)));
            assert!(scan.hits.is_empty(), "{line}: {scan:?}");
            assert_eq!(scan.allowed.len(), 1, "{line}");
        }
        // Not a comment, or not at the end of the line: no.
        for line in [format!("+{fake} {ALLOW}"), format!("+// {ALLOW} {fake}")] {
            let scan = in_diff(&built_in(), &diff_of(std::slice::from_ref(&line)));
            assert_eq!(scan.hits.len(), 1, "{line}");
        }
        // It never allows a banned word.
        let (_dir, rules) = list("zebraword\n");
        let scan = in_diff(&rules, &diff_of(&[format!("+zebraword // {ALLOW}")]));
        assert_eq!(scan.hits.len(), 1);
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let o = git::command()
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }

    #[test]
    fn a_branch_and_a_worktree_are_scanned_for_what_they_add() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let (_words, rules) = list("zebraword\n");
        git_in(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("f.txt"), "zebraword was here first\n").unwrap();
        git_in(d, &["add", "-A"]);
        git_in(d, &["commit", "-qm", "base"]);
        git_in(d, &["checkout", "-q", "-b", "work"]);
        // The base drops the line afterwards; the branch that still has it did not add it.
        git_in(d, &["checkout", "-q", "main"]);
        std::fs::write(d.join("f.txt"), "clean\n").unwrap();
        git_in(d, &["commit", "-qam", "the base is cleaned"]);
        git_in(d, &["checkout", "-q", "work"]);
        assert!(added_in_worktree(&rules, d, "main")
            .unwrap()
            .hits
            .is_empty());
        std::fs::write(
            d.join("f.txt"),
            "zebraword was here first\nzebraword again\n",
        )
        .unwrap();
        let found = added_in_worktree(&rules, d, "main").unwrap().hits;
        assert_eq!(places(&found), "f.txt:2");
        git_in(d, &["commit", "-qam", "add it"]);
        let found = added_since(&rules, d, "main", "work").unwrap().hits;
        assert_eq!(places(&found), "f.txt:2");
    }
}
