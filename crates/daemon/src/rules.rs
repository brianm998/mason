// SPDX-License-Identifier: GPL-3.0-or-later

//! Write-time rules (s1152): `.mason/rules/*.toml` on the project's base commit, checked on
//! every `Edit`, `Write` and `MultiEdit` of a writing agent. `mason hook edit` forwards the
//! hook's input over the step's contract socket; the answer is the `PreToolUse` reply.
//!
//! The rules are never read from the worktree, so a task cannot change what it is checked
//! against. Everything here fails open: input or a rule that cannot be read lets the edit
//! through and leaves one line in the step's log. It reads edits, so it catches mistakes and
//! is not a boundary (a file written through `Bash` is not seen).

use mason_landing::git;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use toml_edit::{DocumentMut, TomlError};

pub const DIR: &str = ".mason/rules";
const MAX_RULE: usize = 32 * 1024;
const MAX_RULES: usize = 100;
/// A file larger than this is not checked.
const MAX_FILE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The edit goes ahead and the agent is given the message.
    Warn,
    /// The edit is refused with the message.
    Block,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Warn => "warn",
            Action::Block => "block",
        }
    }
}

/// The keys a rule file may have.
const KEYS: [&str; 7] = [
    "id",
    "paths",
    "cfg_test_paths",
    "pattern",
    "unless",
    "action",
    "message",
];

/// One rule file.
#[derive(Debug)]
pub struct Rule {
    pub id: String,
    /// Files checked whole.
    paths: Vec<String>,
    /// Rust files where only what is inside a `#[cfg(test)]` item is checked.
    cfg_test_paths: Vec<String>,
    /// Matched against the file as the edit leaves it; a match counts when it
    /// covers an added line.
    pattern: Regex,
    /// Found anywhere in the same file, it clears a hit.
    unless: Option<Regex>,
    pub action: Action,
    pub message: String,
}

pub fn parse(text: &str) -> Result<Rule, String> {
    // The first and last line of the error: what is between is a drawing.
    let short = |e: String| {
        let lines: Vec<&str> = e.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        match lines.as_slice() {
            [] => String::new(),
            [one] => one.to_string(),
            [first, .., last] => format!("{first}: {last}"),
        }
    };
    let doc: DocumentMut = text.parse().map_err(|e: TomlError| short(e.to_string()))?;
    if let Some((k, _)) = doc.iter().find(|(k, _)| !KEYS.contains(k)) {
        return Err(format!("unknown key `{k}`"));
    }
    let string = |k: &str| -> Result<Option<String>, String> {
        doc.get(k)
            .map(|v| {
                v.as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(String::from)
                    .ok_or_else(|| format!("`{k}` must be a non-empty string"))
            })
            .transpose()
    };
    let needed = |k: &str| string(k)?.ok_or_else(|| format!("`{k}` is missing"));
    let globs = |k: &str| -> Result<Vec<String>, String> {
        let bad = || format!("`{k}` must be a list of globs");
        doc.get(k).map_or(Ok(vec![]), |v| {
            v.as_array()
                .ok_or_else(bad)?
                .iter()
                .map(|g| g.as_str().map(String::from).ok_or_else(bad))
                .collect()
        })
    };
    let (paths, cfg_test_paths) = (globs("paths")?, globs("cfg_test_paths")?);
    if paths.is_empty() && cfg_test_paths.is_empty() {
        return Err("`paths` names no file".into());
    }
    let regex = |s: &str| Regex::new(s).map_err(|e| short(e.to_string()));
    Ok(Rule {
        id: needed("id")?,
        paths,
        cfg_test_paths,
        pattern: regex(&needed("pattern")?)?,
        unless: string("unless")?.as_deref().map(regex).transpose()?,
        action: match needed("action")?.as_str() {
            "warn" => Action::Warn,
            "block" => Action::Block,
            other => return Err(format!("`action` is `warn` or `block`, not {other:?}")),
        },
        message: needed("message")?,
    })
}

#[derive(Debug, Default)]
pub struct Rules {
    pub rules: Vec<Rule>,
    /// Why a rule is not in force, one line each.
    pub problems: Vec<String>,
}

/// The rules on the base commit of the repository at `repo`. None when there is no rules
/// directory; a file that cannot be used is left out and named in `problems`.
pub fn load(repo: &Path, base_ref: &str) -> Rules {
    let mut out = Rules::default();
    let listed = git::rev_parse(repo, base_ref).and_then(|sha| {
        let dir = format!("{DIR}/");
        git::run(repo, &["ls-tree", "--name-only", &sha, "--", &dir]).map(|names| (sha, names))
    });
    let (sha, names) = match listed {
        Ok(v) => v,
        Err(e) => {
            let e = e.to_string().replace('\n', " ");
            out.problems
                .push(format!("the rules could not be read from {base_ref}: {e}"));
            return out;
        }
    };
    let names: Vec<&str> = names.lines().filter(|n| n.ends_with(".toml")).collect();
    if names.len() > MAX_RULES {
        out.problems.push(format!(
            "{DIR} holds over {MAX_RULES} files; the rest are not read"
        ));
    }
    for name in names.into_iter().take(MAX_RULES) {
        let rule = git::run(repo, &["show", &format!("{sha}:{name}")])
            .map_err(|e| e.to_string().replace('\n', " "))
            .and_then(|text| {
                if text.len() > MAX_RULE {
                    Err(format!("it is over {MAX_RULE} bytes"))
                } else {
                    parse(&text)
                }
            });
        match rule {
            Ok(r) => out.rules.push(r),
            Err(e) => out.problems.push(format!("{name} is not used: {e}")),
        }
    }
    out
}

/// What one tool call would do to one file of the worktree.
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    /// Relative to the worktree, with `/`.
    pub path: String,
    pub before: String,
    pub after: String,
}

/// `path` without `.` and `..`, its existing part with links resolved.
fn real(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => {
                out.push(other);
                if let Ok(r) = std::fs::canonicalize(&out) {
                    out = r;
                }
            }
        }
    }
    out
}

/// The edit a `PreToolUse` input describes. `Ok(None)` for another tool or a file
/// outside the worktree; `Err` for input that cannot be read.
pub fn edit_of(input: &str, root: &Path) -> Result<Option<Edit>, String> {
    let v: Value = serde_json::from_str(input).map_err(|e| format!("it is not JSON ({e})"))?;
    let tool = v["tool_name"].as_str().ok_or("it names no tool")?;
    if !matches!(tool, "Edit" | "Write" | "MultiEdit") {
        return Ok(None);
    }
    let args = &v["tool_input"];
    let file = args["file_path"]
        .as_str()
        .ok_or_else(|| format!("{tool} has no file_path"))?;
    let full = real(&root.join(file));
    let Ok(rel) = full.strip_prefix(real(root)) else {
        return Ok(None);
    };
    let path = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let before = match std::fs::metadata(&full) {
        Ok(m) if !m.is_file() || m.len() > MAX_FILE => {
            return Err(format!("{path} is not a file that can be checked"))
        }
        Ok(_) => String::from_utf8_lossy(
            &std::fs::read(&full).map_err(|e| format!("{path} could not be read: {e}"))?,
        )
        .into_owned(),
        Err(_) => String::new(),
    };
    let after = match tool {
        "Write" => args["content"]
            .as_str()
            .ok_or("Write has no content")?
            .to_string(),
        "Edit" => replaced(&before, args)?,
        _ => args["edits"]
            .as_array()
            .ok_or("MultiEdit has no edits")?
            .iter()
            .try_fold(before.clone(), |text, e| replaced(&text, e))?,
    };
    Ok(Some(Edit {
        path,
        before,
        after,
    }))
}

/// `text` after one `{old_string, new_string, replace_all}` replacement.
fn replaced(text: &str, e: &Value) -> Result<String, String> {
    let (Some(old), Some(new)) = (e["old_string"].as_str(), e["new_string"].as_str()) else {
        return Err("an edit has no old_string and new_string".into());
    };
    Ok(if !old.is_empty() && text.contains(old) {
        if e["replace_all"].as_bool().unwrap_or(false) {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        }
    } else if text.is_empty() {
        new.to_string()
    } else {
        // The tool will refuse this edit; the new text is still looked at.
        format!("{text}\n{new}")
    })
}

/// Which lines of `after` the edit added: between the unchanged start and end, those
/// that outnumber their copies in the replaced part of `before`.
fn added_lines(before: &str, after: &str) -> Vec<bool> {
    let (b, a): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
    let start = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
    let end = b[start..]
        .iter()
        .rev()
        .zip(a[start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let mut old: HashMap<&str, usize> = HashMap::new();
    for l in &b[start..b.len() - end] {
        *old.entry(l).or_default() += 1;
    }
    let mut added = vec![false; a.len()];
    for i in start..a.len() - end {
        match old.get_mut(a[i]) {
            Some(n) if *n > 0 => *n -= 1,
            _ => added[i] = true,
        }
    }
    added
}

/// `text` with comments, strings and character literals blanked (newlines kept), so
/// that braces can be counted.
fn code_only(text: &str) -> Vec<char> {
    let c: Vec<char> = text.chars().collect();
    let mut out = c.clone();
    let ident = |x: char| x.is_alphanumeric() || x == '_';
    let at = |i: usize| c.get(i).copied();
    let mut i = 0;
    while i < c.len() {
        let start = i;
        match c[i] {
            '/' if at(i + 1) == Some('/') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if at(i + 1) == Some('*') => {
                let mut depth = 0;
                while i < c.len() {
                    if c[i] == '/' && at(i + 1) == Some('*') {
                        depth += 1;
                        i += 2;
                    } else if c[i] == '*' && at(i + 1) == Some('/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            // A raw string: `r"…"`, `r#"…"#`, `br"…"`.
            'r' if (i == 0 || !ident(c[i - 1]) || c[i - 1] == 'b')
                && matches!(at(i + 1), Some('"' | '#')) =>
            {
                let hashes = c[i + 1..].iter().take_while(|x| **x == '#').count();
                if at(i + 1 + hashes) != Some('"') {
                    i += 1;
                    continue;
                }
                i += hashes + 2;
                while i < c.len() && !(c[i] == '"' && (1..=hashes).all(|k| at(i + k) == Some('#')))
                {
                    i += 1;
                }
                i += hashes + 1;
            }
            '"' => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    i += if c[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            }
            // A character literal; a lifetime has no closing quote.
            '\'' if at(i + 1) == Some('\\') => {
                i += 3;
                while i < c.len() && c[i] != '\'' && c[i] != '\n' {
                    i += 1;
                }
                i += 1;
            }
            '\'' if at(i + 2) == Some('\'') => i += 3,
            _ => {
                i += 1;
                continue;
            }
        }
        for x in out.iter_mut().take(i.min(c.len())).skip(start) {
            if *x != '\n' {
                *x = ' ';
            }
        }
    }
    out
}

/// Which lines of a Rust file are inside an item marked `#[cfg(test)]` (or
/// `cfg(all(test, …))`), or all of them under `#![cfg(test)]`.
fn test_lines(text: &str) -> Vec<bool> {
    let code: String = code_only(text).into_iter().collect();
    let mut inside = vec![false; text.lines().count()];
    let mark = Regex::new(r"#(!?)\[\s*cfg\s*\(\s*(?:all\s*\(\s*)?test\b").expect("a valid regex");
    for m in mark.captures_iter(&code) {
        let whole = m.get(0).expect("the match");
        if &m[1] == "!" {
            inside.fill(true);
            break;
        }
        let first = code[..whole.start()].matches('\n').count();
        let (mut braces, mut brackets) = (0i32, 0i32);
        let mut end = code.len();
        // The item ends at its closing brace, or at a `;` before any brace.
        for (i, ch) in code[whole.start()..].char_indices() {
            match ch {
                '(' | '[' => brackets += 1,
                ')' | ']' => brackets -= 1,
                '{' => braces += 1,
                '}' => braces -= 1,
                _ => {}
            }
            if (ch == '}' && braces <= 0) || (ch == ';' && braces == 0 && brackets == 0) {
                end = whole.start() + i;
                break;
            }
        }
        let last = code[..end].matches('\n').count();
        for l in inside.iter_mut().take(last + 1).skip(first) {
            *l = true;
        }
    }
    inside
}

/// The rules an edit breaks.
pub fn check<'a>(rules: &'a [Rule], edit: &Edit) -> Vec<&'a Rule> {
    let added = added_lines(&edit.before, &edit.after);
    if !added.contains(&true) {
        return vec![];
    }
    let in_test = if edit.path.ends_with(".rs") {
        test_lines(&edit.after)
    } else {
        Vec::new()
    };
    let named = |globs: &[String]| {
        globs
            .iter()
            .any(|g| mason_testing::glob::matches(g, &edit.path))
    };
    // Where each line of `after` starts.
    let starts: Vec<usize> = std::iter::once(0)
        .chain(edit.after.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line = |at: usize| starts.partition_point(|s| *s <= at) - 1;
    rules
        .iter()
        .filter(|r| {
            let whole = named(&r.paths);
            if !whole && !named(&r.cfg_test_paths) {
                return false;
            }
            if r.unless.as_ref().is_some_and(|u| u.is_match(&edit.after)) {
                return false;
            }
            r.pattern.find_iter(&edit.after).any(|m| {
                let last = m.end().saturating_sub(1).max(m.start());
                (line(m.start())..=line(last)).any(|l| {
                    added.get(l) == Some(&true) && (whole || in_test.get(l) == Some(&true))
                })
            })
        })
        .collect()
}

/// The `PreToolUse` reply for the rules an edit breaks; `None` when it breaks none.
pub fn reply(hits: &[&Rule]) -> Option<Value> {
    if hits.is_empty() {
        return None;
    }
    let text = hits
        .iter()
        .map(|r| format!("MASON rule `{}`: {}", r.id, r.message.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let out = if hits.iter().any(|r| r.action == Action::Block) {
        json!({"hookEventName": "PreToolUse", "permissionDecision": "deny",
               "permissionDecisionReason": text})
    } else {
        json!({"hookEventName": "PreToolUse", "permissionDecision": "allow",
               "permissionDecisionReason": text, "additionalContext": text})
    };
    Some(json!({"hookSpecificOutput": out}))
}

fn hits_file(task_dir: &Path) -> PathBuf {
    task_dir.join("rule-hits.jsonl")
}

/// Where a step run's lines about rules that could not be applied are kept.
pub fn log_file(task_dir: &Path, run: u64) -> PathBuf {
    task_dir.join(format!("rules-r{run}.log"))
}

/// Every recorded hit of a task, oldest first: `{at_ms, run, step, rule, action, path}`.
pub fn hits(task_dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(hits_file(task_dir))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn append(file: &Path, line: &str) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?;
    writeln!(f, "{line}")
}

/// The rules of one step run.
pub struct Checker {
    rules: Rules,
    worktree: PathBuf,
    task_dir: PathBuf,
    run: u64,
    step: String,
    /// Held while the step's files are written: hooks of one session can run together.
    writing: Mutex<BTreeMap<String, ()>>,
}

impl Checker {
    /// Reads the rules from `base_ref` of the project's repository, and logs what of
    /// them cannot be used.
    pub fn new(
        repo: &Path,
        base_ref: &str,
        worktree: &Path,
        task_dir: &Path,
        run: u64,
        step: &str,
    ) -> Checker {
        let c = Checker {
            rules: load(repo, base_ref),
            worktree: worktree.to_path_buf(),
            task_dir: task_dir.to_path_buf(),
            run,
            step: step.to_string(),
            writing: Mutex::default(),
        };
        for p in &c.rules.problems {
            c.log(p);
        }
        c
    }

    /// One line in the step's log, once however often it happens.
    fn log(&self, what: &str) {
        let _held = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let line = format!("MASON rules: {what} (edits are not refused for this)");
        let file = log_file(&self.task_dir, self.run);
        let old = std::fs::read_to_string(&file).unwrap_or_default();
        if old.lines().any(|l| l == line) {
            return;
        }
        tracing::warn!(run = self.run, "{line}");
        let _ = std::fs::create_dir_all(&self.task_dir);
        if let Err(e) = append(&file, &line) {
            tracing::warn!(run = self.run, "cannot write the rules log: {e}");
        }
    }

    /// The hook's reply for one `PreToolUse` input, empty to say nothing; records
    /// every hit.
    pub fn answer(&self, input: &str) -> String {
        if self.rules.rules.is_empty() {
            return String::new();
        }
        let edit = match edit_of(input, &self.worktree) {
            Ok(Some(e)) => e,
            Ok(None) => return String::new(),
            Err(e) => {
                self.log(&format!("the hook's input could not be checked: {e}"));
                return String::new();
            }
        };
        let found = check(&self.rules.rules, &edit);
        if found.is_empty() {
            return String::new();
        }
        {
            let _held = self.writing.lock().unwrap_or_else(|e| e.into_inner());
            let _ = std::fs::create_dir_all(&self.task_dir);
            for r in &found {
                let hit = json!({"at_ms": crate::daemon::now_ms(), "run": self.run,
                    "step": self.step, "rule": r.id, "action": r.action.name(),
                    "path": edit.path});
                if let Err(e) = append(&hits_file(&self.task_dir), &hit.to_string()) {
                    tracing::warn!(run = self.run, "cannot record a rule hit: {e}");
                }
            }
        }
        reply(&found).map(|v| v.to_string()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MASON's own rules, as seeded.
    const SEED: [&str; 4] = [
        include_str!("../../../.mason/rules/timing-assertion.toml"),
        include_str!("../../../.mason/rules/unsigned-binary-copy.toml"),
        include_str!("../../../.mason/rules/caffeinate-user-active.toml"),
        include_str!("../../../.mason/rules/xcuitest-run.toml"),
    ];

    fn seed() -> Vec<Rule> {
        SEED.iter().map(|t| parse(t).unwrap()).collect()
    }

    fn ids(path: &str, before: &str, after: &str) -> Vec<String> {
        let edit = Edit {
            path: path.into(),
            before: before.into(),
            after: after.into(),
        };
        check(&seed(), &edit).iter().map(|r| r.id.clone()).collect()
    }

    fn new_file(path: &str, text: &str) -> Vec<String> {
        ids(path, "", text)
    }

    const TIMED: &str = "assert!(start.elapsed() < Duration::from_secs(2));\n";

    #[test]
    fn a_timing_assertion_is_caught_in_test_code_only() {
        let hit = ["timing-assertion"];
        for path in [
            "crates/daemon/tests/it/a.rs",
            "crates/daemon/tests/a.rs",
            "crates/cli/src/x/tests.rs",
        ] {
            assert_eq!(new_file(path, TIMED), hit, "{path}");
        }
        // Under src/ outside a test item it is not a test's assertion.
        assert!(new_file(
            "crates/daemon/src/a.rs",
            &format!("fn f() {{\n    {TIMED}}}\n")
        )
        .is_empty());
        let module = format!(
            "fn f() {{}}\n\n#[cfg(test)]\nmod tests {{\n    // }}\n    const S: &str = \"}}\";\n    \
             #[test]\n    fn t() {{\n        {TIMED}    }}\n}}\n"
        );
        assert_eq!(new_file("crates/daemon/src/a.rs", &module), hit);
        // After the test module closes, it is ordinary code again.
        let after = format!("#[cfg(test)]\nmod tests {{}}\n\nfn f() {{\n    {TIMED}}}\n");
        assert!(new_file("crates/daemon/src/a.rs", &after).is_empty());
        let single = format!("#[cfg(test)]\nuse x::y;\nfn f() {{\n    {TIMED}}}\n");
        assert!(new_file("crates/daemon/src/a.rs", &single).is_empty());
        for text in [
            "assert!(took < Duration::from_millis(200), \"{took:?}\");\n",
            "assert!(\n    started.elapsed() <= limit,\n    \"slow\"\n);\n",
            "assert!(Instant::now() < deadline);\n",
            "assert!(limit > t0.elapsed());\n",
        ] {
            assert_eq!(new_file("crates/a/tests/a.rs", text), hit, "{text}");
        }
        // Hang guards and plain values are not timing assertions.
        for text in [
            "assert!(rx.recv_timeout(Duration::from_secs(60)).is_ok());\n",
            "wait_for(\"the file\", 60, || path.exists());\n",
            "assert_eq!(cfg.timeout, Duration::from_secs(1));\n",
            "assert!(a < b);\nlet d = Duration::from_secs(1);\n",
            "assert!(matches!(x, Some(_)));\nif t.elapsed() > limit { panic!(\"hung\") }\n",
        ] {
            assert!(new_file("crates/a/tests/a.rs", text).is_empty(), "{text}");
        }
    }

    #[test]
    fn only_what_the_edit_adds_counts() {
        let before = format!("fn t() {{\n    {TIMED}}}\n");
        let after = format!("fn t() {{\n    {TIMED}    other();\n}}\n");
        assert!(ids("crates/a/tests/a.rs", &before, &after).is_empty());
        assert!(ids("crates/a/tests/a.rs", &before, "fn t() {}\n").is_empty());
        // A changed line inside a statement whose first line stayed is still seen.
        let before = "assert!(\n    done,\n);\n";
        let after = "assert!(\n    start.elapsed() < limit,\n);\n";
        assert_eq!(
            ids("crates/a/tests/a.rs", before, after),
            ["timing-assertion"]
        );
        // A second copy of an old line is an added line.
        let before = format!("{TIMED}x();\n");
        let after = format!("{TIMED}x();\n{TIMED}");
        assert_eq!(
            ids("crates/a/tests/a.rs", &before, &after),
            ["timing-assertion"]
        );
    }

    #[test]
    fn a_copied_binary_needs_codesign_in_the_same_file() {
        let copy = "std::fs::copy(std::env::current_exe().unwrap(), &to).unwrap();\n";
        let hit = ["unsigned-binary-copy"];
        assert_eq!(new_file("crates/a/tests/a.rs", copy), hit);
        assert_eq!(
            new_file("crates/a/tests/a.rs", "fs::copy(bin(\"p\", \"x\"), to)?;\n"),
            hit
        );
        assert_eq!(
            new_file(
                "scripts/exit/s1.sh",
                "cp \"$root/target/debug/mason\" \"$tmp/m\"\n"
            ),
            hit
        );
        let signed = format!("{copy}Command::new(\"codesign\").args([\"-s\", \"-\"]).arg(&to);\n");
        assert!(new_file("crates/a/tests/a.rs", &signed).is_empty());
        assert!(new_file("crates/a/tests/a.rs", "fs::copy(&fixture, &to)?;\n").is_empty());
        assert!(new_file("crates/a/src/a.rs", copy).is_empty());
    }

    #[test]
    fn caffeinate_u_and_ui_test_runs_are_caught_but_not_comments_or_docs() {
        let hit = ["caffeinate-user-active"];
        for text in [
            "caffeinate -u -t 5\n",
            "/usr/bin/caffeinate -d -u make\n",
            "caffeinate -t 60 -du &\n",
            "Command::new(\"caffeinate\").args([\"-d\", \"-u\"]);\n",
        ] {
            assert_eq!(new_file("scripts/a.sh", text), hit, "{text}");
        }
        for text in [
            "caffeinate -dims make\n",
            "# never `caffeinate -u`\n",
            "// no caffeinate -u here\n",
        ] {
            assert!(new_file("scripts/a.sh", text).is_empty(), "{text}");
        }
        assert!(new_file("docs/a.md", "caffeinate -u\n").is_empty());

        let hit = ["xcuitest-run"];
        for text in [
            "xcodebuild test -scheme MonitorUITests\n",
            "xcodebuild -scheme Monitor \\\n  -only-testing:MonitorUITests \\\n  test\n",
            "xcodebuild test-without-building -only-testing:MonitorUITests/A\n",
        ] {
            assert_eq!(new_file("scripts/a.sh", text), hit, "{text}");
        }
        for text in [
            "xcodebuild test -scheme Monitor -skip-testing:MonitorUITests\n",
            "xcodebuild build-for-testing -scheme MonitorUITests\n",
            "xcodebuild test -scheme Monitor\n",
            "# xcodebuild test -scheme MonitorUITests\n",
        ] {
            assert!(new_file("scripts/a.sh", text).is_empty(), "{text}");
        }
    }

    fn repo() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        git::run(t.path(), &["init", "-q", "-b", "main"]).unwrap();
        t
    }

    fn put(root: &Path, path: &str, text: &str) {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn commit(root: &Path) {
        git::run(root, &["add", "-A"]).unwrap();
        git::run(
            root,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "c",
                "--allow-empty",
            ],
        )
        .unwrap();
    }

    fn write(path: &Path, content: &str) -> String {
        json!({"tool_name": "Write", "tool_input": {"file_path": path, "content": content}})
            .to_string()
    }

    fn checker(repo: &Path, dir: &Path) -> Checker {
        Checker::new(repo, "main", repo, dir, 7, "develop")
    }

    #[test]
    fn rules_come_from_the_base_commit_not_the_worktree() {
        let t = repo();
        let r = t.path();
        let out = tempfile::tempdir().unwrap();
        commit(r);
        put(r, ".mason/rules/timing.toml", SEED[0]);
        let file = r.join("crates/a/tests/a.rs");
        assert_eq!(checker(r, out.path()).answer(&write(&file, TIMED)), "");
        commit(r);
        // An edit of the rule in the worktree changes nothing.
        put(r, ".mason/rules/timing.toml", "broken");
        let said = checker(r, out.path()).answer(&write(&file, TIMED));
        let said: Value = serde_json::from_str(&said).unwrap();
        let said = &said["hookSpecificOutput"];
        assert_eq!(said["permissionDecision"], "deny");
        let why = said["permissionDecisionReason"].as_str().unwrap();
        assert!(why.contains(seed()[0].message.trim()), "{why}");
        let hits = hits(out.path());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["rule"], "timing-assertion");
        assert_eq!(hits[0]["path"], "crates/a/tests/a.rs");
        assert_eq!(hits[0]["run"], 7);
        assert!(!log_file(out.path(), 7).exists());
    }

    #[test]
    fn a_malformed_rules_file_allows_the_edit_and_logs_one_line() {
        for bad in [
            "not toml [",
            "id = \"x\"\npaths = [\"**\"]\npattern = \"(\"\naction = \"block\"\nmessage = \"m\"\n",
        ] {
            let t = repo();
            let out = tempfile::tempdir().unwrap();
            put(t.path(), ".mason/rules/bad.toml", bad);
            commit(t.path());
            let file = t.path().join("crates/a/tests/a.rs");
            for _ in 0..2 {
                let c = checker(t.path(), out.path());
                assert_eq!(c.answer(&write(&file, TIMED)), "");
            }
            let log = std::fs::read_to_string(log_file(out.path(), 7)).unwrap();
            assert_eq!(log.lines().count(), 1, "{log}");
            assert!(log.contains(".mason/rules/bad.toml"), "{log}");
        }
    }

    #[test]
    fn unreadable_input_is_allowed_and_logged_and_a_warning_allows_with_context() {
        let t = repo();
        let out = tempfile::tempdir().unwrap();
        put(
            t.path(),
            ".mason/rules/todo.toml",
            "id = \"todo\"\npaths = [\"**\"]\npattern = \"TODO\"\naction = \"warn\"\nmessage = \"File it.\"\n",
        );
        commit(t.path());
        let c = checker(t.path(), out.path());
        assert_eq!(c.answer("not json"), "");
        assert_eq!(c.answer("{\"tool_name\":\"Write\",\"tool_input\":{}}"), "");
        let log = std::fs::read_to_string(log_file(out.path(), 7)).unwrap();
        assert_eq!(log.lines().count(), 2, "{log}");
        // Edit and MultiEdit are applied to the file on disk.
        put(t.path(), "a.txt", "one\ntwo\n");
        for input in [
            json!({"tool_name": "Edit", "tool_input": {"file_path": t.path().join("a.txt"),
                "old_string": "two", "new_string": "two TODO"}}),
            json!({"tool_name": "MultiEdit", "tool_input": {"file_path": "a.txt", "edits": [
                {"old_string": "one", "new_string": "uno"},
                {"old_string": "uno", "new_string": "TODO"}]}}),
        ] {
            let said: Value = serde_json::from_str(&c.answer(&input.to_string())).unwrap();
            let said = &said["hookSpecificOutput"];
            assert_eq!(said["permissionDecision"], "allow");
            assert!(said["additionalContext"]
                .as_str()
                .unwrap()
                .contains("File it."));
        }
        // Outside the worktree nothing is checked.
        let outside = out.path().join("b.txt");
        assert_eq!(c.answer(&write(&outside, "TODO")), "");
        assert_eq!(hits(out.path()).len(), 2);
    }
}
