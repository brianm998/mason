// SPDX-License-Identifier: GPL-3.0-or-later

//! The clarity check (Part 4): is a task already clear enough to skip exploring?
//! Deterministic, no model. It looks for three things in the task text:
//!
//! * **named files** that exist on the base branch (a file that will be created
//!   must say so with `(new)`),
//! * **acceptance criteria**: a section of that name with at least one item,
//! * a described **approach**: a section of that name with some substance.
//!
//! Anything uncertain gets explored: when in doubt, the answer is "not clear".

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clarity {
    pub clear: bool,
    /// Why not, one line each (empty when clear).
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedFile {
    pub path: String,
    /// The text says it will be created (`(new)` or "new file").
    pub new: bool,
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')
}

/// Does this token look like a file path?
fn path_like(t: &str) -> bool {
    if t.is_empty() || t.len() > 200 || t.contains("//") || t.starts_with('/') && t.len() == 1 {
        return false;
    }
    if !t.chars().all(is_path_char) || t.starts_with(['-', '.']) && !t.starts_with("./") {
        return false;
    }
    let ext = t.rsplit_once('.').map(|(_, e)| e);
    let has_ext = ext.is_some_and(|e| {
        (2..=6).contains(&e.len())
            && e.chars().all(|c| c.is_ascii_alphanumeric())
            && e.chars().any(|c| c.is_ascii_alphabetic())
    });
    let has_dir = t.contains('/') && t.trim_end_matches('/').contains('/') || t.starts_with("./");
    has_ext || (has_dir && !t.ends_with('/'))
}

/// Every file-looking token in the text, in order, once each.
pub fn named_files(body: &str) -> Vec<NamedFile> {
    let mut out: Vec<NamedFile> = vec![];
    for line in body.lines() {
        let lower = line.to_lowercase();
        let new_line = lower.contains("(new)") || lower.contains("new file");
        for raw in line.split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '`' | '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '<' | '>' | '*'
                )
        }) {
            let trimmed = raw.trim_end_matches(['.', ':', '!', '?']);
            let dotslash = trimmed.starts_with("./") && trimmed.len() > 2;
            let t = trimmed.trim_start_matches("./");
            let looks_like_a_file =
                path_like(t) || (dotslash && t.chars().all(is_path_char) && !t.contains("//"));
            if t.starts_with("http") || t.contains('@') || !looks_like_a_file {
                continue;
            }
            if !out.iter().any(|f| f.path == t) {
                out.push(NamedFile {
                    path: t.to_string(),
                    new: new_line,
                });
            }
        }
    }
    out
}

fn is_heading(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(h) = t.strip_prefix('#') {
        return Some(
            h.trim_start_matches('#')
                .trim()
                .trim_end_matches(':')
                .to_lowercase(),
        );
    }
    // "Acceptance criteria:" on a line of its own.
    if t.ends_with(':')
        && !t.starts_with(['-', '*'])
        && t.split_whitespace().count() <= 4
        && t.len() > 1
    {
        return Some(t.trim_end_matches(':').to_lowercase());
    }
    None
}

fn is_item(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("+ ")
        || t.split_once(". ")
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The lines under the first heading whose text contains any of `words`.
fn section<'a>(body: &'a str, words: &[&str]) -> Option<Vec<&'a str>> {
    let mut lines = body.lines().peekable();
    while let Some(l) = lines.next() {
        if let Some(h) = is_heading(l) {
            if words.iter().any(|w| h.contains(w)) {
                let mut v = vec![];
                while let Some(n) = lines.peek() {
                    if is_heading(n).is_some() {
                        break;
                    }
                    v.push(lines.next().expect("peeked"));
                }
                return Some(v);
            }
        }
    }
    None
}

/// Judge a task body. `exists` says whether a path is a file on the base
/// branch. A bare file name (no `/`) should match a file with that name
/// anywhere, since prose says "token.rs" for `src/auth/token.rs`.
pub fn check(body: &str, exists: &dyn Fn(&str) -> bool) -> Clarity {
    let mut reasons = vec![];

    let files = named_files(body);
    if files.is_empty() {
        reasons.push("no files are named".to_string());
    } else {
        let mut found = 0;
        for f in &files {
            if exists(&f.path) {
                found += 1;
            } else if !f.new {
                reasons.push(format!("`{}` is named but is not a file on the base branch (mark it `(new)` if it will be created)", f.path));
            }
        }
        if found == 0 && reasons.is_empty() {
            reasons.push("no existing file is named: every named file is new".to_string());
        }
    }

    match section(body, &["acceptance", "criteria"]) {
        Some(lines) if lines.iter().any(|l| is_item(l)) => {}
        Some(_) => reasons.push("the acceptance criteria section has no items".to_string()),
        None => reasons.push("no acceptance criteria".to_string()),
    }

    match section(body, &["approach", "plan", "implementation", "how"]) {
        Some(lines) => {
            let words: usize = lines.iter().map(|l| l.split_whitespace().count()).sum();
            if words < 8 {
                reasons.push("the approach is described in too few words to act on".to_string());
            }
        }
        None => reasons.push("no approach is described".to_string()),
    }

    Clarity {
        clear: reasons.is_empty(),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &'static [&'static str]) -> impl Fn(&str) -> bool {
        // Like the daemon's: exact, or by bare file name.
        move |p| {
            list.contains(&p)
                || (!p.contains('/') && list.iter().any(|f| f.ends_with(&format!("/{p}"))))
        }
    }

    const CLEAR: &str = "Make token expiry configurable.\n\n\
        Files: `src/auth/token.rs` and src/config.rs.\n\n\
        ## Approach\nAdd a `ttl_seconds` field to Config, read it in token.rs when signing, and keep 3600 as the default.\n\n\
        ## Acceptance criteria\n- a ttl of 60 expires tokens after a minute\n- the default is unchanged\n";

    #[test]
    fn a_task_with_files_criteria_and_an_approach_is_clear() {
        let c = check(CLEAR, &files(&["src/auth/token.rs", "src/config.rs"]));
        assert_eq!(
            c,
            Clarity {
                clear: true,
                reasons: vec![]
            }
        );
    }

    #[test]
    fn each_missing_ingredient_makes_it_unclear_and_says_why() {
        let exists = files(&["src/auth/token.rs", "src/config.rs"]);
        let cases: &[(&str, &str)] = &[
            ("Fix the login bug.", "no files are named"),
            ("Fix it in src/nope.rs\n## Approach\nchange the thing in the file to do the other thing instead\n## Acceptance criteria\n- ok\n", "`src/nope.rs` is named but is not a file"),
            ("Edit src/config.rs\n## Approach\nchange the thing in the file to do the other thing instead\n", "no acceptance criteria"),
            ("Edit src/config.rs\n## Acceptance criteria\nit works\n## Approach\nchange the thing in the file to do the other thing instead\n", "section has no items"),
            ("Edit src/config.rs\n## Acceptance criteria\n- ok\n", "no approach is described"),
            ("Edit src/config.rs\n## Acceptance criteria\n- ok\n## Approach\nJust do it.\n", "too few words"),
            ("Create src/brand_new.rs (new)\n## Approach\nwrite the new module and wire it in from main as the plan says\n## Acceptance criteria\n- ok\n", "every named file is new"),
        ];
        for (body, want) in cases {
            let c = check(body, &exists);
            assert!(!c.clear, "{body:?}");
            assert!(
                c.reasons.iter().any(|r| r.contains(want)),
                "{body:?}: wanted {want:?} in {:?}",
                c.reasons
            );
        }
    }

    #[test]
    fn a_new_file_is_fine_next_to_an_existing_one() {
        let body = "Edit src/config.rs and add src/extra.rs (new).\n## Approach\nextend the config and add a helper module used by it\n## Acceptance criteria\n1. works\n";
        assert!(check(body, &files(&["src/config.rs"])).clear);
    }

    #[test]
    fn headings_come_in_several_spellings() {
        let body = "Touch README.md\nApproach:\nrewrite the intro paragraph so that it names the project and says what it does\nAcceptance criteria:\n* reads well\n";
        assert!(check(body, &files(&["README.md"])).clear);
    }

    #[test]
    fn file_detection_ignores_prose_urls_and_versions() {
        let f: Vec<String> = named_files("See https://example.com/a/b.html, e.g. foo. Version 1.2.3 of v2.0; mail me@x.org. Done. Edit `src/lib.rs`, ./Makefile and docs/guide.md.").into_iter().map(|f| f.path).collect();
        assert_eq!(f, vec!["src/lib.rs", "Makefile", "docs/guide.md"]);
    }

    #[test]
    fn empty_and_odd_bodies_are_unclear_not_errors() {
        for b in ["", "   \n\n", "###", "- - -", "`", "a/b/", "...."] {
            assert!(!check(b, &|_| true).clear, "{b:?}");
        }
    }
}
