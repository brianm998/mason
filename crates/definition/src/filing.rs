// SPDX-License-Identifier: GPL-3.0-or-later

//! Filing work: how a task says "exploration is already done" (Part 4), and the
//! text that tells filers (people and models) how.

use std::collections::BTreeMap;

pub const LABEL_EXPLORED: &str = "mason:explored";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filing {
    /// `explored: true` was claimed in the front matter. Whether it counts
    /// depends on who filed it; see `Task::explored_counts`.
    pub explored: bool,
    /// The body with any front matter removed.
    pub body: String,
    pub front_matter: BTreeMap<String, String>,
}

fn truthy(v: &str) -> bool {
    matches!(v.trim().to_lowercase().as_str(), "true" | "yes" | "1")
}

/// Split `---` front matter off the start of a task body.
pub fn parse(text: &str) -> Filing {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Filing {
            body: text.to_string(),
            ..Default::default()
        };
    }
    let mut front = BTreeMap::new();
    let mut consumed = 1;
    let mut closed = false;
    for l in lines.by_ref() {
        consumed += 1;
        if l.trim_end() == "---" {
            closed = true;
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            front.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    if !closed {
        // No closing line: it was not front matter.
        return Filing {
            body: text.to_string(),
            ..Default::default()
        };
    }
    let body: String = text.lines().skip(consumed).collect::<Vec<_>>().join("\n");
    Filing {
        explored: front.get("explored").is_some_and(|v| truthy(v)),
        body: body.trim_start_matches('\n').to_string(),
        front_matter: front,
    }
}

/// Remove fenced code blocks and inline code spans: text in them is quoted, not said.
fn without_code(text: &str) -> String {
    let mut out = String::new();
    let mut fence: Option<char> = None;
    for line in text.lines() {
        let t = line.trim_start();
        let mark = if t.starts_with("```") {
            Some('`')
        } else if t.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        match (fence, mark) {
            (None, Some(m)) => {
                fence = Some(m);
                continue;
            }
            (Some(f), Some(m)) if f == m => {
                fence = None;
                continue;
            }
            (Some(_), _) => continue,
            _ => {}
        }
        // Inline spans: drop everything between a pair of backticks.
        let mut in_span = false;
        for c in line.chars() {
            if c == '`' {
                in_span = !in_span;
            } else if !in_span {
                out.push(c);
            }
        }
        out.push('\n');
    }
    out
}

/// One reference at the start of `s`: its length, and the issue number if it
/// names an issue of `own_repo` (`#N`, or a URL of that repository).
fn reference(s: &str, own_repo: &str) -> Option<(usize, Option<u64>)> {
    fn digits(s: &str) -> Option<(usize, u64)> {
        let n = s.chars().take_while(char::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        // A number followed by more word characters is something else.
        if s[n..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            return None;
        }
        s[..n].parse().ok().map(|v| (n, v))
    }
    if let Some(rest) = s.strip_prefix('#') {
        return digits(rest).map(|(n, v)| (n + 1, Some(v)));
    }
    let lower = s.to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if lower.starts_with(scheme) {
            let end = s.find(char::is_whitespace).unwrap_or(s.len());
            let url = &s[..end];
            let url_trim = url.trim_end_matches(|c: char| ",.;)".contains(c));
            let path = &url_trim[scheme.len()..];
            let parts: Vec<&str> = path.split('/').collect();
            let own = parts.len() >= 5
                && parts[0].eq_ignore_ascii_case("github.com")
                && format!("{}/{}", parts[1], parts[2]).eq_ignore_ascii_case(own_repo)
                && parts[3] == "issues";
            let num = if own {
                let tail = parts[4].split(['#', '?']).next().unwrap_or("");
                tail.parse::<u64>().ok()
            } else {
                None
            };
            return Some((url_trim.len(), num));
        }
    }
    // `owner/repo#N`: another repository.
    let word_end = s
        .find(|c: char| c.is_whitespace() || c == ',')
        .unwrap_or(s.len());
    let w = &s[..word_end];
    if let Some((repo, num)) = w.split_once('#') {
        if repo.contains('/') && !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            let n = w.len();
            let same = repo.eq_ignore_ascii_case(own_repo);
            return Some((n, if same { num.parse().ok() } else { None }));
        }
    }
    None
}

/// The issues of `own_repo` a body says it depends on: `Depends on #3, #4`,
/// `Blocked by #5` or a URL of this repository, any case. References to
/// other repositories are skipped, and so is anything in code.
pub fn dependencies(body: &str, own_repo: &str) -> Vec<u64> {
    let text = without_code(body);
    let mut found = vec![];
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let mut from = 0;
        while let Some((at, len)) = ["depends on", "blocked by"]
            .iter()
            .filter_map(|p| lower[from..].find(p).map(|i| (i + from, p.len())))
            .min()
        {
            let mut rest = &line[at + len..];
            from = at + len;
            loop {
                let t = rest.trim_start_matches(|c: char| {
                    c.is_whitespace() || c == ',' || c == ':' || c == '&'
                });
                let t = match t.get(..3) {
                    Some(a) if a.eq_ignore_ascii_case("and") && t[3..].starts_with(' ') => {
                        t[3..].trim_start()
                    }
                    _ => t,
                };
                match reference(t, own_repo) {
                    Some((n, num)) => {
                        if let Some(v) = num {
                            if !found.contains(&v) {
                                found.push(v);
                            }
                        }
                        rest = &t[n..];
                    }
                    None => break,
                }
            }
        }
    }
    found
}

pub fn explored_from_labels(labels: &[&str]) -> bool {
    labels
        .iter()
        .any(|l| l.eq_ignore_ascii_case(LABEL_EXPLORED))
}

/// `mason help filing`.
pub const HELP: &str = "\
FILING WORK

A task goes through exploring first: an agent reads the code and writes a plan.
If the task already is a plan, say so and that step is skipped.

Say \"already explored\" in any of these ways:

  mason task add --explored \"Title\" --body \"...\"      from the command line
  explored: true                                         front matter at the very
                                                         top of a task body:
                                                           ---
                                                           explored: true
                                                           ---
  the label  mason:explored                              on a tracker issue
  the `explored` argument of the file_proposal tool      for agents filing work

Use it when the body already names the files, says how to change them and what
done looks like. Do not use it for a hunch: a task that turns out not to hold up
is sent back to exploring by the developing agent (no failure is charged), but
that is wasted work.

Without the flag, a deterministic check looks for exactly that: files that exist
on the base branch (mark a file you will create with `(new)`), a section called
\"Acceptance criteria\" with at least one item, and an \"Approach\" with some
substance. If all three are there, exploring is skipped anyway. Anything
uncertain is explored.

Only you and your own agents can skip exploring. The flag on a task from anyone
else is ignored until you approve the task, and then your approval stands.

Task detail shows why exploring was skipped (flag or clarity) and offers
\"Explore anyway\":  mason task explore-anyway <id>

The pipeline's definition decides where skipped tasks start (`explored_entry` in
.mason/mason.toml). Without it, every task explores.

ORDER
Say a task waits for others:  mason task add --after 3,4 \"Title\"
On a tracker, GitHub's \"blocked by\" relationship counts, and so does a body line
\"Depends on #3, #4\" or \"Blocked by #3\" (not inside code). The task is held until
those land; `mason task show <id>` lists what it waits for and what waits for it.
";

/// The short section `mason init` adds to a project's AGENTS.md or CLAUDE.md.
pub const AGENT_NOTES: &str = "\
## Filing work for the pipeline

If you file a task that already contains the plan (the files to change, the
approach and the acceptance criteria), mark it explored so the pipeline skips
its exploring step: pass `explored: true` to `file_proposal`, or `--explored` to
`mason task add`, or put `explored: true` in the task's front matter. Only do
this when the task body really is the plan. Run `mason help filing` for details.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_is_split_off_and_read() {
        let f = parse("---\nexplored: true\nowner: me\n---\nDo the thing.\n\nMore.");
        assert!(f.explored);
        assert_eq!(f.body, "Do the thing.\n\nMore.");
        assert_eq!(f.front_matter["owner"], "me");
    }

    #[test]
    fn only_true_ish_values_count_and_only_at_the_very_top() {
        assert!(parse("---\nexplored: yes\n---\nx").explored);
        assert!(parse("---\nEXPLORED: True\n---\nx").explored);
        assert!(!parse("---\nexplored: false\n---\nx").explored);
        assert!(!parse("---\nexplored: maybe\n---\nx").explored);
        // Not at the start: it is just text.
        let f = parse("Intro\n---\nexplored: true\n---\nx");
        assert!(!f.explored);
        assert!(f.body.starts_with("Intro"));
        // An unclosed block is not front matter either.
        let f = parse("---\nexplored: true\nno closing line");
        assert!(!f.explored);
        assert_eq!(f.body, "---\nexplored: true\nno closing line");
        assert_eq!(parse("").body, "");
    }

    #[test]
    fn body_lines_name_dependencies() {
        let d = |b: &str| dependencies(b, "me/repo");
        assert_eq!(d("Depends on #3"), vec![3]);
        assert_eq!(d("depends on #3, #4"), vec![3, 4]);
        assert_eq!(d("DEPENDS ON: #3 and #4."), vec![3, 4]);
        assert_eq!(d("Blocked by #5\r\nand more text #9"), vec![5]);
        assert_eq!(d("intro\nDepends on #1\nBlocked by #2\n"), vec![1, 2]);
        assert_eq!(d("Depends on #3, #3"), vec![3]);
        assert_eq!(d("see #3 for context"), Vec::<u64>::new());
        assert_eq!(d("Depends on #3x"), Vec::<u64>::new());
    }

    #[test]
    fn other_repositories_do_not_count_but_own_urls_do() {
        let d = |b: &str| dependencies(b, "me/repo");
        assert_eq!(d("Depends on other/thing#7"), Vec::<u64>::new());
        assert_eq!(
            d("Depends on https://github.com/other/thing/issues/7"),
            Vec::<u64>::new()
        );
        assert_eq!(d("Depends on other/thing#7, #8"), vec![8]);
        assert_eq!(
            d("Blocked by https://github.com/me/repo/issues/12."),
            vec![12]
        );
        assert_eq!(
            d("Blocked by https://github.com/Me/Repo/issues/12#issuecomment-1"),
            vec![12]
        );
        assert_eq!(d("Depends on me/repo#4"), vec![4]);
    }

    #[test]
    fn code_is_quoted_not_said() {
        let d = |b: &str| dependencies(b, "me/repo");
        assert_eq!(d("```\nDepends on #3\n```\nDepends on #4"), vec![4]);
        assert_eq!(d("~~~\nBlocked by #3\n~~~"), Vec::<u64>::new());
        assert_eq!(d("write `Depends on #3` in the body"), Vec::<u64>::new());
        assert_eq!(d("`x` Depends on #6 `y`"), vec![6]);
        assert_eq!(d("```rust\nlet a = 1;\n```\nafter: Blocked by #2"), vec![2]);
    }

    #[test]
    fn the_label_is_recognised() {
        assert!(explored_from_labels(&["bug", "mason:explored"]));
        assert!(explored_from_labels(&["MASON:EXPLORED"]));
        assert!(!explored_from_labels(&["explored", "mason"]));
    }

    #[test]
    fn the_help_text_names_every_route() {
        for must in [
            "--explored",
            "explored: true",
            "mason:explored",
            "file_proposal",
            "explore-anyway",
            "explored_entry",
            "ignored until you approve",
            "--after",
        ] {
            assert!(HELP.contains(must), "{must}");
        }
        assert!(AGENT_NOTES.contains("file_proposal") && AGENT_NOTES.contains("mason help filing"));
    }
}
