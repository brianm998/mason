// SPDX-License-Identifier: GPL-3.0-or-later

//! The dropped-line audit. When a task's merge with the base resolved a conflict, every
//! line the base added since the merge base must still be in the result, unless the
//! task's own diff removed it or the task listed it as a deliberate removal.
//!
//! A line is kept when it is there exactly, or, for a line of at least [`MIN_TOKENS`]
//! identifier or number tokens, when all its tokens appear together within [`WINDOW`]
//! adjacent lines of the result: that accepts a line both sides edited or one that was
//! re-wrapped. Blank lines, lines with no word character and `source-hash:` lines
//! (regeneration rewrites them) are ignored.
//!
//! The rules are pure ([`unkept`]); [`audit`] gathers the diffs and file contents from git.

use crate::git::GitError;
use crate::proc;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::process::Command;

/// A line needs this many tokens to be matched by token window rather than exactly.
pub const MIN_TOKENS: usize = 3;
/// How many adjacent result lines may hold a line's tokens together.
pub const WINDOW: usize = 3;
/// The feedback names at most this many lines.
pub const MAX_LISTED: usize = 20;

/// A base line the result no longer has.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dropped {
    pub file: String,
    pub line: String,
}

/// A line the task says it removes on purpose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removal {
    pub file: String,
    pub line: String,
}

/// Lines of a `git diff -U0`, by file: the `+` lines and the `-` lines. Only lines
/// inside a hunk count, so the `---`/`+++` file headers are never taken for content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: BTreeMap<String, Vec<String>>,
    pub removed: BTreeMap<String, BTreeSet<String>>,
}

/// Parse the text of `git diff -U0 --no-renames`.
pub fn parse_diff(diff: &str) -> Changes {
    let mut out = Changes::default();
    let mut file: Option<String> = None;
    let mut in_hunk = false;
    let mut old_name: Option<String> = None;
    for l in diff.split('\n') {
        if !in_hunk || l.starts_with("diff --git ") {
            if l.starts_with("diff --git ") {
                in_hunk = false;
                file = None;
                old_name = None;
            } else if let Some(n) = l.strip_prefix("--- ") {
                old_name = n.strip_prefix("a/").map(String::from);
            } else if let Some(n) = l.strip_prefix("+++ ") {
                file = match n.strip_prefix("b/") {
                    Some(n) => Some(n.to_string()),
                    None => old_name.clone(), // +++ /dev/null: a deleted file
                };
            } else if l.starts_with("@@") {
                in_hunk = true;
            }
            continue;
        }
        if l.starts_with("@@") {
            continue;
        }
        let Some(f) = &file else { continue };
        if let Some(t) = l.strip_prefix('+') {
            out.added.entry(f.clone()).or_default().push(t.to_string());
        } else if let Some(t) = l.strip_prefix('-') {
            out.removed
                .entry(f.clone())
                .or_default()
                .insert(t.to_string());
        }
    }
    out
}

/// Identifier tokens (a letter or underscore, then letters, digits, underscores) and
/// number tokens (a run of digits).
pub fn tokens(line: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut BTreeSet<String>| {
        if !cur.is_empty() {
            out.insert(std::mem::take(cur));
        }
    };
    for ch in line.chars() {
        let word = ch.is_ascii_alphanumeric() || ch == '_';
        if !word {
            flush(&mut cur, &mut out);
            continue;
        }
        let starts_number = cur.chars().next().is_some_and(|c| c.is_ascii_digit());
        if starts_number && !ch.is_ascii_digit() {
            // `12abc`: the number ends, an identifier starts.
            flush(&mut cur, &mut out);
        }
        cur.push(ch);
    }
    flush(&mut cur, &mut out);
    out
}

fn ignored(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || !t.chars().any(|c| c.is_alphanumeric() || c == '_')
        || t.contains("source-hash:")
}

/// What the result of one file holds, ready to be asked about.
struct Haystack {
    exact: HashSet<String>,
    windows: Vec<BTreeSet<String>>,
}

impl Haystack {
    fn new(text: &str) -> Haystack {
        let lines: Vec<&str> = text.lines().collect();
        let toks: Vec<BTreeSet<String>> = lines.iter().map(|l| tokens(l)).collect();
        let windows = (0..lines.len())
            .map(|i| {
                toks[i..(i + WINDOW).min(lines.len())]
                    .iter()
                    .flatten()
                    .cloned()
                    .collect()
            })
            .collect();
        Haystack {
            exact: lines.iter().map(|l| l.to_string()).collect(),
            windows,
        }
    }

    fn has(&self, line: &str) -> bool {
        if self.exact.contains(line) {
            return true;
        }
        let t = tokens(line);
        t.len() >= MIN_TOKENS && self.windows.iter().any(|w| t.is_subset(w))
    }
}

/// The base's added lines that the result lost. `base_added` and `task_removed` come
/// from [`parse_diff`]; `read(file)` is the file's text in the result (`None`: it does
/// not exist there). A line the task's own diff removed, or listed in `deliberate`, is
/// not reported. Sorted by file, then in the order the base added them.
pub fn unkept(
    base_added: &BTreeMap<String, Vec<String>>,
    task_removed: &BTreeMap<String, BTreeSet<String>>,
    deliberate: &[Removal],
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<Dropped> {
    let mut out = vec![];
    for (file, lines) in base_added {
        let mut hay: Option<Haystack> = None;
        let mut seen = HashSet::new();
        for line in lines {
            if ignored(line) || !seen.insert(line.as_str()) {
                continue;
            }
            if task_removed.get(file).is_some_and(|s| s.contains(line)) {
                continue;
            }
            if deliberate
                .iter()
                .any(|r| r.file == *file && r.line.trim() == line.trim())
            {
                continue;
            }
            let h = hay.get_or_insert_with(|| Haystack::new(&read(file).unwrap_or_default()));
            if !h.has(line) {
                out.push(Dropped {
                    file: file.clone(),
                    line: line.clone(),
                });
            }
        }
    }
    out
}

fn diff_text(repo: &Path, range: &str) -> Result<String, GitError> {
    let args = [
        "-c",
        "core.quotepath=off",
        "diff",
        "-U0",
        "--no-color",
        "--no-renames",
        "--no-ext-diff",
        range,
    ];
    // Not `git::run`, which trims: a first or last line must match exactly.
    let out = proc::output(
        Command::new("git").args(args).current_dir(repo),
        crate::git::default_deadline(),
    );
    match out {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(failed(&args, String::from_utf8_lossy(&o.stderr).into_owned())),
        Err(e) => Err(failed(&args, format!("{e:?}"))),
    }
}

fn failed(args: &[&str], stderr: String) -> GitError {
    GitError {
        args: args.iter().map(|s| s.to_string()).collect(),
        code: None,
        stderr,
        timed_out: false,
    }
}

fn file_at(repo: &Path, rev: &str, file: &str) -> Option<String> {
    let spec = format!("{rev}:{file}");
    let o = proc::output(
        Command::new("git")
            .args(["show", &spec])
            .current_dir(repo),
        crate::git::default_deadline(),
    )
    .ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Audit a resolved merge: what the base (`base`) added since `merge_base` against
/// what `result` holds. `task` is the task's side of the merge: lines its diff from
/// `merge_base` removed are its own business. `later_base` is the base's current tip:
/// lines the base itself removed after `base` are not the task's to keep.
pub fn audit(
    repo: &Path,
    merge_base: &str,
    base: &str,
    task: &str,
    later_base: Option<&str>,
    result: &str,
    deliberate: &[Removal],
) -> Result<Vec<Dropped>, GitError> {
    let added = parse_diff(&diff_text(repo, &format!("{merge_base}..{base}"))?).added;
    if added.is_empty() {
        return Ok(vec![]);
    }
    let mut removed = parse_diff(&diff_text(repo, &format!("{merge_base}..{task}"))?).removed;
    if let Some(tip) = later_base {
        for (f, s) in parse_diff(&diff_text(repo, &format!("{base}..{tip}"))?).removed {
            removed.entry(f).or_default().extend(s);
        }
    }
    Ok(unkept(&added, &removed, deliberate, &|f| {
        file_at(repo, result, f)
    }))
}

/// The one-line reason for a send-back.
pub fn reason(d: &[Dropped]) -> String {
    let files: BTreeSet<&str> = d.iter().map(|x| x.file.as_str()).collect();
    format!(
        "conflict resolution dropped {} landed line(s) in: {}",
        d.len(),
        files.into_iter().collect::<Vec<_>>().join(", ")
    )
}

/// What the agent is told: each file and line, at most [`MAX_LISTED`].
pub fn feedback(d: &[Dropped]) -> String {
    let mut s = String::from(
        "Your resolution of a merge with the base branch dropped lines the base had already \
         landed. Restore each one, or, if your task's change removes it on purpose, list it \
         under `removes` ({file, line, reason}) when you submit.\n",
    );
    for x in d.iter().take(MAX_LISTED) {
        s.push_str(&format!("\n{}: {}", x.file, x.line.trim_end()));
    }
    if d.len() > MAX_LISTED {
        s.push_str(&format!("\n... and {} more", d.len() - MAX_LISTED));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(file: &str, lines: &[&str]) -> BTreeMap<String, Vec<String>> {
        BTreeMap::from([(
            file.to_string(),
            lines.iter().map(|s| s.to_string()).collect(),
        )])
    }

    fn run(lines: &[&str], result: &str) -> Vec<String> {
        unkept(
            &one("f.rs", lines),
            &BTreeMap::new(),
            &[],
            &|_| Some(result.to_string()),
        )
        .into_iter()
        .map(|d| d.line)
        .collect()
    }

    #[test]
    fn exact_line_is_kept() {
        assert!(run(&["let a = compute(x, y);"], "x\nlet a = compute(x, y);\ny\n").is_empty());
    }

    #[test]
    fn a_dropped_line_is_flagged() {
        assert_eq!(
            run(&["let a = compute(x, y);"], "x\ny\n"),
            ["let a = compute(x, y);"]
        );
    }

    #[test]
    fn a_missing_file_drops_everything() {
        let r = unkept(
            &one("f.rs", &["let a = compute(x, y);"]),
            &BTreeMap::new(),
            &[],
            &|_| None,
        );
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn a_rewrapped_line_is_kept() {
        let r = "let a = compute(\n    x,\n    y,\n);\n";
        assert!(run(&["let a = compute(x, y);"], r).is_empty());
    }

    #[test]
    fn an_argument_added_on_both_sides_is_kept() {
        assert!(run(&["let a = compute(x, y);"], "let a = compute(x, y, z);\n").is_empty());
    }

    #[test]
    fn tokens_spread_beyond_the_window_are_not_kept() {
        let r = "compute\n1\n2\n3\nx\ny\n";
        assert_eq!(run(&["compute(x, y)"], r).len(), 1);
    }

    #[test]
    fn short_lines_must_match_exactly() {
        // Two tokens: `a` and `b` are there, but not the line.
        assert_eq!(run(&["a = b;"], "a\nb\n"), ["a = b;"]);
        assert!(run(&["a = b;"], "a = b;\n").is_empty());
    }

    #[test]
    fn ignored_lines() {
        let r = run(
            &["", "   ", "});", "}", "// source-hash: abc123", "- - -"],
            "nothing\n",
        );
        assert!(r.is_empty(), "{r:?}");
    }

    #[test]
    fn a_line_the_task_removed_is_not_flagged() {
        let removed = BTreeMap::from([(
            "f.rs".to_string(),
            BTreeSet::from(["let a = compute(x, y);".to_string()]),
        )]);
        let r = unkept(
            &one("f.rs", &["let a = compute(x, y);"]),
            &removed,
            &[],
            &|_| Some(String::new()),
        );
        assert!(r.is_empty());
    }

    #[test]
    fn a_deliberate_removal_is_only_for_its_file() {
        let del = [Removal {
            file: "f.rs".into(),
            line: "  let a = compute(x, y);".into(),
        }];
        let mut base = one("f.rs", &["let a = compute(x, y);"]);
        base.insert("g.rs".into(), vec!["let a = compute(x, y);".into()]);
        let r = unkept(&base, &BTreeMap::new(), &del, &|_| Some(String::new()));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].file, "g.rs");
    }

    #[test]
    fn tokens_are_identifiers_and_numbers() {
        let t: Vec<String> = tokens("foo_1(bar, 42) + 7x").into_iter().collect();
        assert_eq!(t, ["42", "7", "bar", "foo_1", "x"]);
    }

    #[test]
    fn diff_parsing_ignores_headers() {
        let d = "diff --git a/f.rs b/f.rs\nindex 1..2 100644\n--- a/f.rs\n+++ b/f.rs\n@@ -1 +1,2 @@\n-old line\n+new line\n++ not a header\n\\ No newline at end of file\ndiff --git a/g b/g\n--- a/g\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        let c = parse_diff(d);
        assert_eq!(c.added["f.rs"], ["new line", "+ not a header"]);
        assert!(c.removed["f.rs"].contains("old line"));
        assert!(c.removed["g"].contains("gone"));
        assert!(!c.added.contains_key("g"));
    }

    #[test]
    fn feedback_is_capped() {
        let d: Vec<Dropped> = (0..25)
            .map(|i| Dropped {
                file: "a".into(),
                line: format!("line {i}"),
            })
            .collect();
        let f = feedback(&d);
        assert!(f.contains("a: line 19") && !f.contains("a: line 20"));
        assert!(f.contains("and 5 more"));
        assert!(reason(&d).contains("25 landed line(s) in: a"));
    }
}
