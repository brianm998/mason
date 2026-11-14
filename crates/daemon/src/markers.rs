// SPDX-License-Identifier: GPL-3.0-or-later

//! Unresolved merge conflict markers in what a change adds.
//!
//! On 2026-11-13 five develop agents committed a merge of the base with its conflict markers
//! still in the files, and one of them landed: `landing.rs` did not parse for 22 minutes while
//! eight more batches stacked on it. The agent could not build (the owner had skipped testing,
//! so `run_check` ran nothing), the test step was skipped, and nothing between `submit_change`
//! and the base read the text. A marker is cheap to see and never right, so every path that
//! takes a change looks for it, and they share this scan: the submit validator, `run_check`
//! (even where testing is skipped) and the landing job.
//!
//! A marker is a line that starts, in column 1, with exactly seven `<` or seven `>` and then a
//! space or the end of the line. A line of `=` alone is not one: a Markdown heading underline
//! is the same text. Only the lines a change *adds* count, so a base that already has markers
//! is not the change's to fix.

use mason_landing::git;
use std::path::Path;

/// One marker line a change adds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub file: String,
    /// Its line number in the changed file.
    pub line: u32,
    /// The line itself, cut short.
    pub text: String,
}

/// How many markers a message names before it says "and N more".
const NAMED: usize = 6;

fn is_marker(line: &str) -> bool {
    let line = line.strip_suffix('\r').unwrap_or(line);
    ['<', '>'].iter().any(|c| {
        let n = line.chars().take_while(|x| x == c).count();
        n == 7 && line[n..].chars().next().is_none_or(|x| x == ' ')
    })
}

/// The path of a `+++ ` header line: `b/dir/file`, `"b/odd\tname"` or `/dev/null`.
fn path_of(header: &str) -> String {
    let p = header.trim_end_matches('\t');
    if p == "/dev/null" {
        return String::new();
    }
    let p = match p.strip_prefix('"').and_then(|q| q.strip_suffix('"')) {
        Some(q) => q
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
            .replace("\\t", "\t"),
        None => p.to_string(),
    };
    p.strip_prefix("b/").unwrap_or(&p).to_string()
}

/// The first line the new side of a hunk covers: the `c` of `@@ -a,b +c,d @@`.
fn new_start(hunk: &str) -> u32 {
    hunk.split_whitespace()
        .find_map(|t| t.strip_prefix('+'))
        .and_then(|t| t.split(',').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
}

fn cut(text: &str) -> String {
    let t = text.strip_suffix('\r').unwrap_or(text);
    match t.char_indices().nth(60) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.to_string(),
    }
}

/// The markers among the lines a unified diff adds. The diff may have any amount of context.
pub fn in_diff(diff: &str) -> Vec<Marker> {
    let mut found = vec![];
    let mut file = String::new();
    let mut in_hunk = false;
    let mut next: u32 = 1;
    for l in diff.lines() {
        if l.starts_with("diff --git ") {
            in_hunk = false;
            continue;
        }
        if l.starts_with("@@") {
            next = new_start(l);
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            // Between `diff --git` and the first `@@`: the only line that matters names the file.
            if let Some(p) = l.strip_prefix("+++ ") {
                file = path_of(p);
            }
            continue;
        }
        // In a hunk every line has its prefix, so a `+++` here is an added line, not a header.
        match l.as_bytes().first() {
            Some(b'+') => {
                if is_marker(&l[1..]) {
                    found.push(Marker {
                        file: file.clone(),
                        line: next,
                        text: cut(&l[1..]),
                    });
                }
                next += 1;
            }
            Some(b' ') => next += 1,
            _ => {}
        }
    }
    found
}

fn diff_args(range: &str) -> [&str; 6] {
    ["diff", "-U0", "--no-color", "--no-ext-diff", range, "--"]
}

/// The markers `head` adds since it left the base: the diff of `base_ref...head`, so a marker
/// the base itself has (or had, and a fix removed) is not counted.
pub fn added_since(dir: &Path, base_ref: &str, head: &str) -> Result<Vec<Marker>, git::GitError> {
    let range = format!("{base_ref}...{head}");
    Ok(in_diff(&git::run(dir, &diff_args(&range))?))
}

/// The markers a worktree adds to `base_ref`, committed or not (tracked files): what
/// `run_check` looks at while the agent is still working.
pub fn added_in_worktree(dir: &Path, base_ref: &str) -> Result<Vec<Marker>, git::GitError> {
    Ok(in_diff(&git::run(dir, &diff_args(base_ref))?))
}

/// `file:line` for each marker, the first few, for a reason or a log line.
pub fn places(found: &[Marker]) -> String {
    let list: Vec<String> = found
        .iter()
        .take(NAMED)
        .map(|m| format!("{}:{}", m.file, m.line))
        .collect();
    match found.len().checked_sub(NAMED) {
        Some(n) if n > 0 => format!("{} and {n} more", list.join(", ")),
        _ => list.join(", "),
    }
}

/// What to tell the agent: where the markers are, and what to do about them.
pub fn refusal(found: &[Marker]) -> String {
    let mut files: Vec<&str> = vec![];
    for m in found {
        if !files.contains(&m.file.as_str()) {
            files.push(&m.file);
        }
    }
    let list: Vec<String> = found
        .iter()
        .take(NAMED)
        .map(|m| format!("{}:{} `{}`", m.file, m.line, m.text))
        .collect();
    let more = match found.len().checked_sub(NAMED) {
        Some(n) if n > 0 => format!(" (and {n} more)"),
        _ => String::new(),
    };
    format!(
        "the change adds unresolved merge conflict markers, so it cannot be right: {}{more}. \
         Open {}, keep what is right from both sides, delete every marker line (the lines that \
         start with seven `<`, seven `=` or seven `>`), build, and commit again. A line that is \
         meant to look like a marker (a fixture, a document quoting a conflict) must not start \
         in column 1.",
        list.join("; "),
        files.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn open() -> String {
        "<".repeat(7)
    }
    fn close() -> String {
        ">".repeat(7)
    }
    fn mid() -> String {
        "=".repeat(7)
    }

    /// A diff of one file with `lines` (already prefixed) as one hunk that starts at `start`.
    fn diff_of(file: &str, start: u32, lines: &[String]) -> String {
        let added = lines.iter().filter(|l| !l.starts_with('-')).count();
        format!(
            "diff --git a/{file} b/{file}\nindex 1111111..2222222 100644\n--- a/{file}\n+++ b/{file}\n@@ -1,0 +{start},{added} @@ fn x\n{}\n",
            lines.join("\n")
        )
    }

    #[test]
    fn a_marker_in_an_added_line_is_found_with_its_file_and_line() {
        let d = diff_of(
            "crates/x/src/a.rs",
            40,
            &[
                "+fn kept() {}".into(),
                format!("+{} HEAD", open()),
                "+let x = 1;".into(),
                format!("+{}", mid()),
                "+let x = 2;".into(),
                format!("+{} origin/develop", close()),
            ],
        );
        let found = in_diff(&d);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(
            (found[0].file.as_str(), found[0].line),
            ("crates/x/src/a.rs", 41)
        );
        assert_eq!(found[0].text, format!("{} HEAD", open()));
        assert_eq!(found[1].line, 45);
        assert!(found[1].text.starts_with(&close()));
        assert_eq!(places(&found), "crates/x/src/a.rs:41, crates/x/src/a.rs:45");
    }

    #[test]
    fn a_marker_with_nothing_after_it_counts_and_a_look_alike_does_not() {
        let d = diff_of(
            "a.txt",
            1,
            &[
                format!("+{}", open()),
                // The `=` line alone is a heading underline in Markdown.
                format!("+{}", mid()),
                // Eight characters, text glued on, indented, or in a removed or context line.
                format!("+{}<", open()),
                format!("+{}x", close()),
                format!("+    {} HEAD", open()),
                format!("-{} HEAD", open()),
                format!(" {} HEAD", open()),
                "+a >>>>>>> b".into(),
            ],
        );
        let found = in_diff(&d);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].line, 1);
        assert!(in_diff("").is_empty());
    }

    #[test]
    fn a_line_added_as_plus_plus_plus_is_content_not_a_file_header() {
        // The added line `++ b/other.rs` shows up as `+++ b/other.rs` inside the hunk.
        let mut d = diff_of(
            "one.rs",
            10,
            &["+++ b/other.rs".into(), format!("+{} HEAD", open())],
        );
        d.push_str(&diff_of(
            "two.rs",
            3,
            &["+ok".into(), format!("+{} x", close())],
        ));
        let found = in_diff(&d);
        let at: Vec<(&str, u32)> = found.iter().map(|m| (m.file.as_str(), m.line)).collect();
        assert_eq!(at, vec![("one.rs", 11), ("two.rs", 4)]);
    }

    #[test]
    fn a_quoted_path_and_a_deleted_file_are_read() {
        assert_eq!(path_of("b/dir/f.rs"), "dir/f.rs");
        assert_eq!(path_of("\"b/odd\\tname.rs\""), "odd\tname.rs");
        assert_eq!(path_of("b/with space.txt\t"), "with space.txt");
        assert_eq!(path_of("/dev/null"), "");
    }

    #[test]
    fn the_refusal_names_the_places_and_says_what_to_do() {
        let many: Vec<Marker> = (1..=8)
            .map(|i| Marker {
                file: "a.rs".into(),
                line: i,
                text: format!("{} HEAD", open()),
            })
            .collect();
        let t = refusal(&many);
        assert!(
            t.contains("a.rs:1 ") && t.contains("a.rs:6 ") && !t.contains("a.rs:7 "),
            "{t}"
        );
        assert!(t.contains("and 2 more") && t.contains("Open a.rs,"), "{t}");
        assert_eq!(places(&many[..2]), "a.rs:1, a.rs:2");
        assert!(places(&many).ends_with("and 2 more"));
    }

    // ---- against a real repository -------------------------------------------------------

    fn git_in(dir: &Path, args: &[&str]) {
        let o = Command::new("git")
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

    fn conflicted(body: &str) -> String {
        format!(
            "{body}\n{} HEAD\nmine\n{}\ntheirs\n{} origin/develop\n",
            open(),
            mid(),
            close()
        )
    }

    #[test]
    fn what_a_branch_adds_is_scanned_and_what_the_base_already_has_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_in(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("f.txt"), "one\n").unwrap();
        git_in(d, &["add", "-A"]);
        git_in(d, &["commit", "-qm", "base"]);
        git_in(d, &["checkout", "-q", "-b", "work"]);
        std::fs::write(d.join("f.txt"), conflicted("one")).unwrap();
        git_in(d, &["commit", "-qam", "merge, badly"]);

        let found = added_since(d, "main", "work").unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!((found[0].file.as_str(), found[0].line), ("f.txt", 2));

        // A broken base that a branch grows from is not the branch's doing...
        git_in(d, &["checkout", "-q", "main"]);
        std::fs::write(d.join("f.txt"), conflicted("one")).unwrap();
        git_in(d, &["commit", "-qam", "the base is broken"]);
        git_in(d, &["checkout", "-q", "-b", "inherited"]);
        std::fs::write(d.join("g.txt"), "two\n").unwrap();
        git_in(d, &["add", "-A"]);
        git_in(d, &["commit", "-qm", "an unrelated change"]);
        assert!(added_since(d, "main", "inherited").unwrap().is_empty());
        // ...and when the base is fixed, the stale branch still adds none (the fix wins the merge).
        git_in(d, &["checkout", "-q", "main"]);
        std::fs::write(d.join("f.txt"), "one\n").unwrap();
        git_in(d, &["commit", "-qam", "the base is fixed"]);
        assert!(added_since(d, "main", "inherited").unwrap().is_empty());
    }

    #[test]
    fn a_worktree_is_scanned_before_anything_is_committed() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        git_in(d, &["init", "-q", "-b", "main"]);
        std::fs::write(d.join("f.txt"), "one\n").unwrap();
        git_in(d, &["add", "-A"]);
        git_in(d, &["commit", "-qm", "base"]);
        assert!(added_in_worktree(d, "main").unwrap().is_empty());
        std::fs::write(d.join("f.txt"), conflicted("one")).unwrap();
        let found = added_in_worktree(d, "main").unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        // Resolving it clears it.
        std::fs::write(d.join("f.txt"), "one\nmine\n").unwrap();
        assert!(added_in_worktree(d, "main").unwrap().is_empty());
    }
}
