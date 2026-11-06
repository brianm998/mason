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
        ] {
            assert!(HELP.contains(must), "{must}");
        }
        assert!(AGENT_NOTES.contains("file_proposal") && AGENT_NOTES.contains("mason help filing"));
    }
}
