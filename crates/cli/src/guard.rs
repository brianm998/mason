// SPDX-License-Identifier: GPL-3.0-or-later

//! The agent command guard (s1107): refuses, with a reason the agent can act on,
//! the `git` forms that destroy uncommitted or shared work and any recursive `rm`
//! that reaches outside the places the agent owns. Nobody watches an agent, so it
//! denies; it never asks.
//!
//! It reads the command line, it does not run it, so it is a guard against
//! mistakes and not a boundary: a script file or another interpreter gets past it.
//! The OS sandbox is the boundary.
//!
//! The command is first flattened (comments and here-document text dropped,
//! subshells and substitutions marked) so that [`split_unquoted`] can cut it into
//! simple commands; each is then read word by word, following `cd`.

use crate::resources::split_unquoted;
use mason_adapters::skills::Policy;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub const DISCARD: &str = "This throws away uncommitted work. Commit it (or `git stash push -m \
<tag>` and apply by SHA) first; if you really mean to discard it, discard the specific files with \
`git checkout <commit> -- <file>`.";

pub const STASH: &str = "The stash stack is shared with other worktrees and the owner's checkout, \
so this can take or destroy another session's work. Commit a WIP commit instead, or save with \
`git stash push -m <tag>`, find its SHA with `git stash list --format='%H %gs'`, restore it with \
`git stash apply <sha>` and leave the entry where it is.";

pub const BRANCH: &str = "`git branch -D` can delete commits that exist nowhere else. Use `git \
branch -d`, which refuses an unmerged branch, or leave the branch where it is.";

pub const PUSH: &str = "Agents never push: the pipeline does all remote git work. Commit on your \
branch and submit it.";

pub const REMOVE: &str = "A recursive `rm` may only delete things inside your worktree, MASON's \
state directory or $TMPDIR, not those directories themselves. Name each path literally (no `$(...)`, \
unknown variables or brace lists) and leave everything outside them alone.";

/// The variables of the agent's shell the guard will expand in an `rm` target.
const VARS: &[&str] = &["HOME", "TMPDIR", "MASON_TASK_CACHE", "MASON_WORKTREE"];

/// Marks left in the flattened command, all from the private use area.
const BRACE_OPEN: char = '\u{E000}';
const BRACE_CLOSE: char = '\u{E001}';
const SUB_OPEN: &str = "\u{E002}";
const SUB_CLOSE: &str = "\u{E003}";

/// Nesting (`bash -c`, substitutions) followed before the command is refused.
const DEPTH: u32 = 8;

/// Where the agent is and what it owns.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    /// A recursive `rm` must stay strictly inside one of these.
    pub roots: Vec<PathBuf>,
    /// The shell's directory when the command starts.
    pub cwd: Option<PathBuf>,
    pub vars: BTreeMap<String, String>,
}

impl Scope {
    /// From the hook input's `cwd` and the hook's environment. The worktree is what the
    /// adapter named (`MASON_GUARD_WORKTREE`), else the CLI's project directory, else
    /// the checkout the shell is in.
    pub fn from_hook(input: &Value, env: &dyn Fn(&str) -> Option<String>) -> Scope {
        let env = |k: &str| env(k).filter(|v| !v.is_empty());
        let cwd = input["cwd"]
            .as_str()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        let worktree = env("MASON_GUARD_WORKTREE")
            .or_else(|| env("MASON_WORKTREE"))
            .or_else(|| env("CLAUDE_PROJECT_DIR"))
            .map(PathBuf::from)
            .or_else(|| {
                let c = cwd.as_deref()?;
                let top = c.ancestors().find(|a| a.join(".git").exists());
                Some(top.unwrap_or(c).to_path_buf())
            });
        let roots = worktree
            .into_iter()
            .chain(
                ["MASON_GUARD_STATE", "MASON_TASK_CACHE", "TMPDIR"]
                    .iter()
                    .filter_map(|k| env(k).map(PathBuf::from)),
            )
            // `/` as a root would allow everything.
            .filter(|p| p.is_absolute() && p.parent().is_some())
            .collect();
        let vars = VARS
            .iter()
            .filter_map(|k| env(k).map(|v| (k.to_string(), v)))
            .collect();
        Scope { roots, cwd, vars }
    }
}

/// The `PreToolUse` answer for a Bash command the guard refuses; `None` for
/// everything else (the guard never allows anything itself).
pub fn hook_guard(input: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<Value> {
    let v: Value = serde_json::from_str(input).ok()?;
    if v["tool_name"].as_str() != Some("Bash") {
        return None;
    }
    let command = v["tool_input"]["command"].as_str()?;
    let reason = check(command, &Scope::from_hook(&v, env))?;
    Some(json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }}))
}

/// The `PreToolUse` answer for a `Skill` call the session's policy refuses (s1153);
/// `None` for everything else. An unreadable policy or input is an error, which the
/// hook's command turns into a refusal.
pub fn hook_skill(
    input: &str,
    policy: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Value>, String> {
    let v: Value = serde_json::from_str(input).map_err(|e| format!("the hook's input: {e}"))?;
    if v["tool_name"].as_str() != Some("Skill") {
        return Ok(None);
    }
    let text = std::fs::read_to_string(policy).map_err(|e| format!("the skill policy: {e}"))?;
    let policy = serde_json::from_str(&text)
        .ok()
        .and_then(|v| Policy::from_json(&v))
        .ok_or("the skill policy is not readable")?;
    let skill = v["tool_input"]["skill"].as_str().unwrap_or_default();
    // Where the CLI was started, which is where it loads a directory's skills from.
    let worktree = env("MASON_GUARD_WORKTREE")
        .or_else(|| v["cwd"].as_str().map(String::from))
        .ok_or("no working directory")?;
    Ok(policy.refusal(skill, Path::new(&worktree)).map(|reason| {
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }})
    }))
}

/// Why `command` is refused, if it is.
pub fn check(command: &str, scope: &Scope) -> Option<String> {
    let mut scope = scope.clone();
    // A variable the command sets itself no longer has the value the hook sees.
    scope.vars.retain(|k, _| !assigns(command, k));
    check_script(command, &scope, 0)
}

fn assigns(command: &str, name: &str) -> bool {
    let set = format!("{name}=");
    command.match_indices(&set).any(|(i, _)| {
        !command[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
    })
}

fn refuse(words: &[Word], why: &str) -> String {
    let said: String = words
        .iter()
        .map(|w| w.text.replace(BRACE_OPEN, "{").replace(BRACE_CLOSE, "}"))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect();
    format!("MASON refused `{said}`. {why}")
}

fn check_script(script: &str, scope: &Scope, depth: u32) -> Option<String> {
    if depth > DEPTH {
        return Some("MASON refused a command nested too deeply to check.".into());
    }
    let chars: Vec<char> = script.chars().collect();
    let mut flat = Flat::default();
    flatten(&chars, 0, false, &mut flat, 0);
    // Substitutions inside quotes or here-documents: they run somewhere unknown.
    let mut texts = vec![(flat.out, scope.cwd.clone())];
    texts.extend(flat.extra.into_iter().map(|t| (t, None)));
    for (text, start) in texts {
        let mut cwd = start;
        let mut saved = vec![];
        for part in split_unquoted(&text) {
            match part.trim() {
                SUB_OPEN => saved.push(cwd.clone()),
                SUB_CLOSE => {
                    if let Some(c) = saved.pop() {
                        cwd = c;
                    }
                }
                _ => {
                    let words = words(&part, scope, cwd.as_deref());
                    if let Some(why) = check_words(&words, scope, &mut cwd, depth) {
                        return Some(why);
                    }
                }
            }
        }
    }
    None
}

#[derive(Default)]
struct Flat {
    out: String,
    extra: Vec<String>,
}

/// A here-document waiting for the end of its line.
struct Doc {
    end: String,
    quoted: bool,
    tabs: bool,
}

/// Copy `c[i..]` into `f.out` in the form [`split_unquoted`] can cut. With
/// `until_paren`, stop after the `)` that closes the substitution being read.
/// Returns where it stopped.
fn flatten(c: &[char], mut i: usize, until_paren: bool, f: &mut Flat, depth: u32) -> usize {
    let (mut single, mut double) = (false, false);
    let mut parens = 0u32;
    let mut docs: Vec<Doc> = vec![];
    // Where the current line starts in `f.out`.
    let mut line = f.out.len();
    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        if single {
            single = ch != '\'';
            f.out.push(ch);
            i += 1;
            continue;
        }
        match ch {
            '\\' => {
                // A backslash before a newline joins the lines.
                if next != Some('\n') {
                    f.out.push(ch);
                    f.out.extend(next);
                }
                i += 2;
                continue;
            }
            '$' if next == Some('(') && depth < 64 => {
                let mut inner = Flat::default();
                i = flatten(c, i + 2, true, &mut inner, depth + 1);
                f.extra.append(&mut inner.extra);
                // The word holding it has a value nobody knows yet.
                if double {
                    f.out.push_str("$?");
                    f.extra.push(inner.out);
                } else {
                    f.out
                        .push_str(&format!("$?\n{SUB_OPEN}\n{}\n{SUB_CLOSE}\n", inner.out));
                }
                continue;
            }
            '`' => {
                let mut j = i + 1;
                while j < c.len() && c[j] != '`' {
                    j += if c[j] == '\\' { 2 } else { 1 };
                }
                let body: Vec<char> = c[i + 1..j.min(c.len())].to_vec();
                if depth < 64 {
                    let mut inner = Flat::default();
                    flatten(&body, 0, false, &mut inner, depth + 1);
                    f.extra.append(&mut inner.extra);
                    f.extra.push(inner.out);
                }
                f.out.push_str("$?");
                i = j + 1;
                continue;
            }
            '"' => {
                double = !double;
                f.out.push(ch);
            }
            _ if double => f.out.push(ch),
            '\'' => {
                single = true;
                f.out.push(ch);
            }
            '#' if i == 0 || " \t\n;&|(".contains(c[i - 1]) => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '<' if next == Some('<') => {
                if c.get(i + 2) == Some(&'<') {
                    f.out.push_str("<<<");
                    i += 3;
                    continue;
                }
                match here_doc(c, i + 2) {
                    Some((doc, after)) => {
                        docs.push(doc);
                        f.out.push(' ');
                        i = after;
                    }
                    None => {
                        f.out.push_str("<<");
                        i += 2;
                    }
                }
                continue;
            }
            '>' | '<' => {
                f.out.push(ch);
                // `2>&1`, `>|file`: not a separator.
                if matches!(next, Some('&' | '|')) {
                    i += 1;
                }
            }
            '&' if next == Some('>') => {}
            '(' => {
                parens += 1;
                f.out.push_str(&format!("\n{SUB_OPEN}\n"));
            }
            ')' => {
                if parens == 0 && until_paren {
                    return i + 1;
                }
                parens = parens.saturating_sub(1);
                f.out.push_str(&format!("\n{SUB_CLOSE}\n"));
            }
            '{' | '}' => {
                let alone = |x: Option<&char>| x.is_none_or(|x| " \t\n;&|()".contains(*x));
                let before = alone(i.checked_sub(1).and_then(|p| c.get(p)));
                let group = before && alone(c.get(i + 1));
                f.out.push(match (group, ch) {
                    (true, _) => ch,
                    (false, '{') => BRACE_OPEN,
                    (false, _) => BRACE_CLOSE,
                });
            }
            '\n' => {
                let shell = feeds_a_shell(&f.out[line.min(f.out.len())..]);
                f.out.push('\n');
                i += 1;
                for doc in docs.drain(..) {
                    let (body, after) = doc_body(c, i, &doc);
                    i = after;
                    let body: Vec<char> = body.chars().collect();
                    if depth >= 64 {
                        continue;
                    }
                    if shell {
                        let mut inner = Flat::default();
                        flatten(&body, 0, false, &mut inner, depth + 1);
                        f.extra.append(&mut inner.extra);
                        f.out
                            .push_str(&format!("{SUB_OPEN}\n{}\n{SUB_CLOSE}\n", inner.out));
                    } else if !doc.quoted {
                        substitutions(&body, f, depth + 1);
                    }
                }
                line = f.out.len();
                continue;
            }
            _ => f.out.push(ch),
        }
        i += 1;
    }
    i
}

/// The substitutions a shell runs while expanding an unquoted here-document.
fn substitutions(c: &[char], f: &mut Flat, depth: u32) {
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' {
            i += 2;
            continue;
        }
        if c[i] == '$' && c.get(i + 1) == Some(&'(') {
            let mut inner = Flat::default();
            i = flatten(c, i + 2, true, &mut inner, depth);
            f.extra.append(&mut inner.extra);
            f.extra.push(inner.out);
        } else if c[i] == '`' {
            let end = c[i + 1..].iter().position(|x| *x == '`');
            let j = end.map_or(c.len(), |n| i + 1 + n);
            let mut inner = Flat::default();
            flatten(&c[i + 1..j], 0, false, &mut inner, depth);
            f.extra.append(&mut inner.extra);
            f.extra.push(inner.out);
            i = j + 1;
        } else {
            i += 1;
        }
    }
}

/// The delimiter after `<<`, and where it ends. `None` for what is not a
/// here-document (a shift in arithmetic).
fn here_doc(c: &[char], mut i: usize) -> Option<(Doc, usize)> {
    let tabs = c.get(i) == Some(&'-');
    if tabs {
        i += 1;
    }
    while matches!(c.get(i), Some(' ' | '\t')) {
        i += 1;
    }
    if !c
        .get(i)
        .is_some_and(|x| x.is_alphabetic() || "_'\"\\".contains(*x))
    {
        return None;
    }
    let (mut end, mut quoted) = (String::new(), false);
    while let Some(&x) = c.get(i) {
        match x {
            '\'' | '"' | '\\' => quoted = true,
            ' ' | '\t' | '\n' | ';' | '&' | '|' | '<' | '>' | '(' | ')' => break,
            _ => end.push(x),
        }
        i += 1;
    }
    (!end.is_empty()).then_some((Doc { end, quoted, tabs }, i))
}

fn doc_body(c: &[char], mut i: usize, doc: &Doc) -> (String, usize) {
    let mut body = String::new();
    while i < c.len() {
        let end = c[i..]
            .iter()
            .position(|x| *x == '\n')
            .map_or(c.len(), |n| i + n);
        let line: String = c[i..end].iter().collect();
        i = (end + 1).min(c.len());
        let bare = if doc.tabs {
            line.trim_start_matches('\t')
        } else {
            &line
        };
        if bare == doc.end {
            break;
        }
        body.push_str(&line);
        body.push('\n');
    }
    (body, i)
}

/// Whether the line a here-document hangs off runs it as shell commands.
fn feeds_a_shell(line: &str) -> bool {
    line.split([';', '&', '|', '\n']).any(|part| {
        part.split_whitespace()
            .find(|w| !w.contains('='))
            .is_some_and(|w| is_shell(basename(w)))
    })
}

fn is_shell(name: &str) -> bool {
    matches!(name, "bash" | "sh" | "zsh" | "dash")
}

fn basename(w: &str) -> &str {
    w.rsplit('/').next().unwrap_or(w)
}

#[derive(Clone, Debug, Default)]
struct Word {
    /// With quotes removed and the known variables expanded.
    text: String,
    /// Where the first unquoted `*`, `?` or `[` is in `text`.
    glob: Option<usize>,
    /// Holds something only the shell can work out.
    dynamic: bool,
}

/// The words of one simple command, redirections left out.
fn words(part: &str, scope: &Scope, cwd: Option<&Path>) -> Vec<Word> {
    let c: Vec<char> = part.chars().collect();
    let mut all = vec![];
    let mut w = Word::default();
    let (mut started, mut redirected) = (false, false);
    let (mut single, mut double) = (false, false);
    let mut i = 0;
    let mut end = |w: &mut Word, started: &mut bool, redirected: &mut bool| {
        if std::mem::take(started) {
            let word = std::mem::take(w);
            if !std::mem::take(redirected) {
                all.push(word);
            }
        }
    };
    while i < c.len() {
        let ch = c[i];
        i += 1;
        match ch {
            '\'' if !double => {
                single = !single;
                started = true;
            }
            _ if single => w.text.push(ch),
            '"' => {
                double = !double;
                started = true;
            }
            '\\' => {
                if let Some(&n) = c.get(i) {
                    // Inside double quotes the backslash stays unless it escapes.
                    if double && !"$`\"\\".contains(n) {
                        w.text.push('\\');
                    }
                    w.text.push(n);
                    i += 1;
                    started = true;
                }
            }
            '$' => {
                started = true;
                i = variable(&c, i, scope, cwd, &mut w);
            }
            _ if double => w.text.push(ch),
            ' ' | '\t' | '\r' => end(&mut w, &mut started, &mut redirected),
            '<' | '>' => {
                // A number right before it is the file descriptor.
                if started && w.text.chars().all(|d| d.is_ascii_digit()) {
                    w = Word::default();
                    started = false;
                }
                end(&mut w, &mut started, &mut redirected);
                while matches!(c.get(i), Some('<' | '>')) {
                    i += 1;
                }
                redirected = true;
            }
            '~' if !started => {
                started = true;
                match (c.get(i), scope.vars.get("HOME")) {
                    (None | Some('/' | ' ' | '\t'), Some(h)) => w.text.push_str(h),
                    _ => {
                        w.dynamic = true;
                        w.text.push(ch);
                    }
                }
            }
            _ => {
                started = true;
                if matches!(ch, '*' | '?' | '[') {
                    w.glob.get_or_insert(w.text.len());
                }
                if matches!(ch, BRACE_OPEN | BRACE_CLOSE) {
                    w.dynamic = true;
                }
                w.text.push(ch);
            }
        }
    }
    end(&mut w, &mut started, &mut redirected);
    all
}

/// Read the variable after a `$` at `c[i..]` into `w`; returns where it ends.
fn variable(c: &[char], i: usize, scope: &Scope, cwd: Option<&Path>, w: &mut Word) -> usize {
    let braced = matches!(c.get(i), Some(&'{') | Some(&BRACE_OPEN));
    let start = i + usize::from(braced);
    let len = c[start.min(c.len())..]
        .iter()
        .take_while(|x| x.is_ascii_alphanumeric() || **x == '_')
        .count();
    let name: String = c[start.min(c.len())..start.min(c.len()) + len]
        .iter()
        .collect();
    let mut after = start + len;
    let closed = !braced || matches!(c.get(after), Some(&'}') | Some(&BRACE_CLOSE));
    let value = if name == "PWD" {
        cwd.map(|p| p.display().to_string())
    } else {
        scope.vars.get(&name).cloned()
    };
    match value {
        Some(v) if closed => {
            after += usize::from(braced);
            w.text.push_str(&v);
            after
        }
        _ => {
            w.dynamic = true;
            w.text.push('$');
            i
        }
    }
}

fn is_assignment(w: &str) -> bool {
    w.split_once('=').is_some_and(|(n, _)| {
        let n = n.strip_suffix('+').unwrap_or(n);
        !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Words that come before the command they run.
const BEFORE: &[&str] = &[
    "if",
    "then",
    "else",
    "elif",
    "do",
    "while",
    "until",
    "!",
    "time",
    "{",
    "sudo",
    "env",
    "command",
    "nohup",
    "nice",
    "exec",
    "builtin",
    "timeout",
    "gtimeout",
    "caffeinate",
    "stdbuf",
];

/// The command and its arguments, past assignments and the likes of `env` and `timeout`.
fn command_of(words: &[Word]) -> &[Word] {
    let mut i = 0;
    let mut wrapped = false;
    while let Some(w) = words.get(i) {
        let t = w.text.as_str();
        if BEFORE.contains(&basename(t)) {
            wrapped = true;
        } else if !(is_assignment(t)
            || wrapped && (t.starts_with('-') || t.starts_with(|c: char| c.is_ascii_digit())))
        {
            break;
        }
        i += 1;
    }
    &words[i..]
}

/// The single-letter options among `args` (up to `--`) include `f`.
fn short(args: &[Word], f: char) -> bool {
    options(args).any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(f))
}

fn long(args: &[Word], name: &str) -> bool {
    options(args).any(|a| a == name)
}

fn options(args: &[Word]) -> impl Iterator<Item = &str> {
    args.iter()
        .map(|w| w.text.as_str())
        .take_while(|a| *a != "--")
}

fn recursive(args: &[Word]) -> bool {
    short(args, 'r') || short(args, 'R') || long(args, "--recursive")
}

fn check_words(
    words: &[Word],
    scope: &Scope,
    cwd: &mut Option<PathBuf>,
    depth: u32,
) -> Option<String> {
    let cmd = command_of(words);
    let (first, args) = cmd.split_first()?;
    match basename(&first.text) {
        "git" => git(args).map(|why| refuse(cmd, why)),
        "rm" => rm(args, scope, cwd.as_deref()).map(|why| refuse(cmd, &why)),
        "cd" | "pushd" => {
            *cwd = change_dir(args, scope, cwd.as_deref());
            None
        }
        "popd" => {
            *cwd = None;
            None
        }
        name if is_shell(name) => {
            let c = args.iter().position(|a| {
                a.text.starts_with('-') && !a.text.starts_with("--") && a.text.contains('c')
            })?;
            let script = args[c + 1..].iter().find(|a| !a.text.starts_with('-'))?;
            let inner = Scope {
                cwd: cwd.clone(),
                ..scope.clone()
            };
            check_script(&script.text, &inner, depth + 1)
        }
        "eval" => {
            let script: Vec<&str> = args.iter().map(|a| a.text.as_str()).collect();
            let inner = Scope {
                cwd: cwd.clone(),
                ..scope.clone()
            };
            check_script(&script.join(" "), &inner, depth + 1)
        }
        "mason" => {
            let at = args.iter().position(|a| a.text == "--")?;
            if !args[..at].iter().any(|a| a.text == "run") {
                return None;
            }
            check_words(&args[at + 1..], scope, &mut cwd.clone(), depth + 1)
        }
        "xargs" => {
            let at = args.iter().position(|a| {
                matches!(basename(&a.text), "rm" | "git") || is_shell(basename(&a.text))
            })?;
            let inner = &args[at..];
            if basename(&inner[0].text) == "rm" && recursive(&inner[1..]) {
                return Some(refuse(
                    cmd,
                    &format!("Its targets come from standard input. {REMOVE}"),
                ));
            }
            check_words(inner, scope, &mut cwd.clone(), depth + 1)
        }
        "find" => find(args, scope, cwd.as_deref()).map(|why| refuse(cmd, &why)),
        _ => None,
    }
}

fn change_dir(args: &[Word], scope: &Scope, cwd: Option<&Path>) -> Option<PathBuf> {
    let to = args
        .iter()
        .find(|a| !(a.text.len() > 1 && a.text.starts_with('-') || a.text == "--"));
    let Some(to) = to else {
        return scope.vars.get("HOME").map(PathBuf::from);
    };
    if to.dynamic || to.glob.is_some() || to.text == "-" || to.text.starts_with('+') {
        return None;
    }
    resolve(&to.text, cwd, true)
}

/// `path` as the kernel would walk it: made absolute against `cwd`, `..` applied
/// after any link before it. The last name is followed only with `follow` (`rm`
/// removes a link, not what it points to).
fn resolve(path: &str, cwd: Option<&Path>, follow: bool) -> Option<PathBuf> {
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd?.join(p)
    };
    let follow = follow || path.ends_with('/') || path.ends_with("/.");
    let parts: Vec<Component> = full.components().collect();
    let mut out = PathBuf::new();
    for (n, part) in parts.iter().enumerate() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => {
                out.push(other);
                if follow || n + 1 < parts.len() {
                    if let Ok(real) = std::fs::canonicalize(&out) {
                        out = real;
                    }
                }
            }
        }
    }
    Some(out)
}

/// Whether `target` is under one of the roots; `or_root` accepts a root itself.
fn inside(target: &Path, scope: &Scope, or_root: bool) -> bool {
    let roots: Vec<PathBuf> = scope
        .roots
        .iter()
        .filter_map(|r| resolve(&r.display().to_string(), None, true))
        .collect();
    // A root that holds another (a temporary directory holding the worktree) does
    // not make the inner one removable.
    if !or_root && roots.iter().any(|r| r == target) {
        return false;
    }
    roots.iter().any(|r| target.starts_with(r))
}

/// Why this `rm` target may not be removed recursively.
fn outside(w: &Word, scope: &Scope, cwd: Option<&Path>) -> Option<String> {
    let shown = w.text.replace(BRACE_OPEN, "{").replace(BRACE_CLOSE, "}");
    if w.dynamic {
        return Some(format!("`{shown}` cannot be worked out before it runs."));
    }
    // A pattern is judged by the directory it matches in.
    let (path, or_root) = match w.glob {
        None => (w.text.clone(), false),
        Some(at) => {
            let dir = w.text[..at].rfind('/').map_or(0, |s| s + 1);
            let rest: Vec<&str> = w.text[dir..].split('/').collect();
            // `.*` matches `..` in some shells.
            let climbs = rest
                .iter()
                .any(|c| *c == ".." || c.starts_with('.') && c.contains(['*', '?', '[']));
            if climbs {
                return Some(format!("`{shown}` can match a parent directory."));
            }
            let dir = &w.text[..dir];
            (if dir.is_empty() { "." } else { dir }.to_string(), true)
        }
    };
    let follow = w.glob.is_some();
    match resolve(&path, cwd, follow) {
        None => Some(format!(
            "`{shown}` is relative to a directory that cannot be worked out."
        )),
        Some(t) if inside(&t, scope, or_root) => None,
        Some(t) => Some(format!("`{shown}` is `{}`.", t.display())),
    }
}

fn rm(args: &[Word], scope: &Scope, cwd: Option<&Path>) -> Option<String> {
    if !recursive(args) {
        return None;
    }
    let mut literal = false;
    for a in args {
        if !literal && a.text == "--" {
            literal = true;
        } else if literal || !a.text.starts_with('-') {
            if let Some(why) = outside(a, scope, cwd) {
                return Some(format!("{why} {REMOVE}"));
            }
        }
    }
    None
}

/// `find ... -exec rm -r {}`: what is removed is under the starting points.
fn find(args: &[Word], scope: &Scope, cwd: Option<&Path>) -> Option<String> {
    let removes = args.iter().enumerate().any(|(i, a)| {
        matches!(a.text.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir")
            && args.get(i + 1).is_some_and(|c| basename(&c.text) == "rm")
            && recursive(&args[i + 2..])
    });
    if !removes {
        return None;
    }
    let starts: Vec<&Word> = args
        .iter()
        .skip_while(|a| matches!(a.text.as_str(), "-H" | "-L" | "-P" | "-E" | "-X" | "-x"))
        .take_while(|a| !a.text.starts_with('-') && a.text != "!" && a.text != "(")
        .collect();
    let here = Word {
        text: ".".into(),
        ..Word::default()
    };
    let starts = if starts.is_empty() {
        vec![&here]
    } else {
        starts
    };
    for s in starts {
        let ok = !s.dynamic
            && s.glob.is_none()
            && resolve(&s.text, cwd, true).is_some_and(|t| inside(&t, scope, true));
        if !ok {
            return Some(format!("`find` starts at `{}`. {REMOVE}", s.text));
        }
    }
    None
}

/// Why this `git` command is refused.
fn git(args: &[Word]) -> Option<&'static str> {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match a.text.as_str() {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env" => i += 2,
            o if o.starts_with('-') => i += 1,
            _ => break,
        }
    }
    let (sub, args) = args.get(i..)?.split_first()?;
    match sub.text.as_str() {
        "push" => Some(PUSH),
        // git accepts any unambiguous start of a long option.
        "reset" => options(args)
            .any(|a| a.len() >= 4 && "--hard".starts_with(a))
            .then_some(DISCARD),
        "clean" => {
            let force = short(args, 'f') || long(args, "--force");
            let dry = short(args, 'n') || long(args, "--dry-run");
            (force && !dry).then_some(DISCARD)
        }
        "checkout" => checkout(args).then_some(DISCARD),
        "restore" => {
            let staged = short(args, 'S') || long(args, "--staged");
            let worktree = short(args, 'W') || long(args, "--worktree");
            let help = long(args, "--help") || long(args, "-h");
            (!help && (!staged || worktree)).then_some(DISCARD)
        }
        "stash" => stash(args).then_some(STASH),
        "branch" => {
            let delete = short(args, 'd') || long(args, "--delete");
            let force = short(args, 'f') || long(args, "--force");
            (short(args, 'D') || delete && force).then_some(BRANCH)
        }
        _ => None,
    }
}

/// `git checkout -- <paths>` and `git checkout .`; `<commit> -- <file>` is the way
/// to discard one file on purpose, so it passes unless it names everything.
fn checkout(args: &[Word]) -> bool {
    let everything = |a: &Word| matches!(a.text.as_str(), "." | "./" | ".." | ":/" | "*" | ":/*");
    match args.iter().position(|a| a.text == "--") {
        Some(at) => {
            let (before, paths) = (&args[..at], &args[at + 1..]);
            let from_commit = before.iter().any(|a| !a.text.starts_with('-'));
            !paths.is_empty() && (!from_commit || paths.iter().any(everything))
        }
        None => args.iter().any(everything),
    }
}

/// Whether this takes or removes an entry of the shared stack, or adds one that
/// nobody can tell apart.
fn stash(args: &[Word]) -> bool {
    let named = |a: &[Word]| {
        options(a).any(|o| o.starts_with("--message") || o.starts_with("-m")) || short(a, 'm')
    };
    match args.first().map(|a| a.text.as_str()) {
        None => true,
        Some("-h" | "--help") => false,
        Some("push") => !named(&args[1..]),
        Some("drop" | "clear" | "pop" | "branch" | "save") => true,
        Some(o) if o.starts_with('-') => !named(args),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skill_hook_denies_what_the_policy_does_and_errs_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("skills.json");
        let policy = Policy {
            project: vec!["mason:review".into()],
            ..Policy::default()
        };
        std::fs::write(&file, policy.to_json().to_string()).unwrap();
        let ask = |skill: &str, file: &Path| {
            let input = json!({"tool_name": "Skill", "cwd": dir.path().display().to_string(),
                "tool_input": {"skill": skill}});
            hook_skill(&input.to_string(), file, &|_| None)
        };
        assert_eq!(ask("mason:review", &file), Ok(None));
        let denied = ask("sneaky", &file).unwrap().unwrap();
        assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(ask("mason:review", &dir.path().join("absent")).is_err());
        assert!(hook_skill("not json", &file, &|_| None).is_err());
    }

    /// A worktree with a subdirectory, a state directory, a temporary directory and a
    /// home, all separate.
    struct Fixture {
        _dir: tempfile::TempDir,
        base: PathBuf,
        scope: Scope,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        for d in ["wt/target", "wt/crates", "state/a", "tmp", "home", "other"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let path = |d: &str| base.join(d).display().to_string();
        let env: BTreeMap<&str, String> = [
            ("MASON_GUARD_WORKTREE", path("wt")),
            ("MASON_GUARD_STATE", path("state")),
            ("TMPDIR", path("tmp")),
            ("HOME", path("home")),
        ]
        .into();
        let scope = Scope::from_hook(&json!({"cwd": path("wt")}), &|k| env.get(k).cloned());
        Fixture {
            _dir: dir,
            base,
            scope,
        }
    }

    fn refused(b: &Fixture, command: &str) -> String {
        check(command, &b.scope).unwrap_or_else(|| panic!("allowed: {command}"))
    }

    fn allowed(b: &Fixture, command: &str) {
        if let Some(why) = check(command, &b.scope) {
            panic!("refused: {command}\n{why}");
        }
    }

    #[test]
    fn work_destroying_git_forms_are_refused_with_the_way_out() {
        let b = fixture();
        for c in [
            "git reset --hard",
            "git reset --hard HEAD~1",
            "git reset -q --har",
            "git -C crates reset --hard",
            "git -c core.x=y --no-pager reset --hard",
            "git clean -f",
            "git clean -fdx",
            "git clean -d --force",
            "git checkout -- .",
            "git checkout -- src/a.rs",
            "git checkout -q -- a b",
            "git checkout .",
            "git checkout HEAD -- .",
            "git checkout HEAD .",
            "git restore a.rs",
            "git restore .",
            "git restore --source=HEAD~1 a.rs",
            "git restore --staged --worktree a.rs",
            "git restore -SW a.rs",
        ] {
            let why = refused(&b, c);
            assert!(why.contains(DISCARD), "{c}: {why}");
        }
    }

    #[test]
    fn near_misses_of_the_git_forms_pass() {
        let b = fixture();
        for c in [
            "git reset --soft HEAD~1",
            "git reset HEAD a.rs",
            "git reset --mixed",
            "git clean -n",
            "git clean -nfd",
            "git checkout -b new",
            "git checkout develop",
            "git checkout HEAD~1 -- src/a.rs",
            "git checkout develop --",
            "git restore --staged a.rs",
            "git restore -S .",
            "git stash push -m x",
            "git stash push -u -m 'my tag' -- a.rs",
            "git stash push --message=x",
            "git stash -um x",
            "git stash list --format='%H %gs'",
            "git stash show -p abc123",
            "git stash apply abc123",
            "git branch -d old",
            "git branch -a",
            "git branch new",
            "git log --oneline -5 && git status",
            "git commit -m 'git push; git reset --hard'",
            "grep -rn \"git stash clear\" docs",
            "echo git reset --hard",
            "ls # git reset --hard",
        ] {
            allowed(&b, c);
        }
    }

    #[test]
    fn the_shared_stash_branch_deletes_and_pushes_are_refused() {
        let b = fixture();
        for c in [
            "git stash",
            "git stash -u",
            "git stash push",
            "git stash push -u",
            "git stash save wip",
            "git stash pop",
            "git stash pop stash@{0}",
            "git stash drop",
            "git stash drop stash@{1}",
            "git stash clear",
            "git stash branch x",
        ] {
            assert!(refused(&b, c).contains(STASH), "{c}");
        }
        for c in [
            "git branch -D old",
            "git branch -d -f old",
            "git branch --delete --force old",
        ] {
            assert!(refused(&b, c).contains(BRANCH), "{c}");
        }
        for c in ["git push", "git push -f origin HEAD", "git -C . push"] {
            assert!(refused(&b, c).contains(PUSH), "{c}");
        }
    }

    #[test]
    fn every_part_of_a_compound_command_is_checked() {
        let b = fixture();
        for c in [
            "ls && git stash clear",
            "ls; git reset --hard",
            "true || git clean -fd",
            "git status | cat\ngit stash drop",
            "(cd crates && git stash pop)",
            "{ git stash clear; }",
            "if true; then git stash clear; fi",
            "FOO=1 git reset --hard",
            "env X=1 timeout 5 git reset --hard",
            "/usr/bin/git reset --hard",
            "bash -c 'git stash clear'",
            "bash -lc \"cd crates && git reset --hard\"",
            "eval 'git stash clear'",
            "mason run --needs compile -- bash -c 'git stash clear'",
            "echo $(git stash clear)",
            "echo \"$(git stash clear)\"",
            "echo `git stash clear`",
            "bash <<'EOF'\ngit stash clear\nEOF",
            "cat <<EOF\n$(git stash clear)\nEOF",
            "cat <<EOF\ntext\nEOF\ngit stash clear",
            "git status 2>&1; git stash clear",
            "git reset \\\n  --hard",
        ] {
            refused(&b, c);
        }
    }

    #[test]
    fn quoted_and_here_document_text_is_not_a_command() {
        let b = fixture();
        for c in [
            r#"git commit -m "; rm -rf /""#,
            r#"git commit -m "fix (rm -rf /) { git stash clear }""#,
            "git commit -F - <<'EOF'\ngit stash clear\nrm -rf /\nit's refused now\nEOF",
            "cat > notes.md <<EOF\n- git reset --hard\nEOF\nls",
            "git commit -m \"$(cat <<'EOF'\nrm -rf / and the owner's `git push`\nEOF\n)\"",
        ] {
            allowed(&b, c);
        }
    }

    #[test]
    fn a_recursive_rm_stays_inside_what_the_agent_owns() {
        let b = fixture();
        let base = b.base.display();
        for c in [
            "rm -rf target/".to_string(),
            "rm -rf target crates/x ./y".into(),
            "rm -r -f target/debug 2>/dev/null".into(),
            "rm -rf target/* *.orig".into(),
            "rm -rf *".into(),
            "rm -rf \"$TMPDIR/scratch\" ${TMPDIR}/other".into(),
            "rm -rf $TMPDIR/*".into(),
            "rm -rf $PWD/target".into(),
            format!("rm -rf {base}/wt/target {base}/state/a/1 {base}/tmp/x"),
            "rm -f /etc/hosts".into(),
            "rm notes.txt".into(),
            "find . -name '*.tmp' -exec rm -rf {} +".into(),
            "find target -type d -exec rm -r {} \\;".into(),
        ] {
            allowed(&b, &c);
        }
        for c in [
            "rm -rf /".to_string(),
            "rm -rf ~".into(),
            "rm -rf ~/x".into(),
            "rm -rf ~/*".into(),
            "rm -rf /*".into(),
            "rm -fr $HOME/x".into(),
            "rm -R ../other".into(),
            "rm --recursive target ../other".into(),
            "rm -rf -- ../other".into(),
            "rm -rf .".into(),
            "rm -rf ..".into(),
            "rm -rf target/../..".into(),
            "rm -rf .*".into(),
            "rm -rf target/.*/x".into(),
            "rm -rf ../*".into(),
            "rm -rf $TMPDIR".into(),
            "rm -rf $UNKNOWN/target".into(),
            "rm -rf \"${UNKNOWN:-/}\"".into(),
            "rm -rf $(pwd)/target".into(),
            "rm -rf `pwd`".into(),
            "rm -rf {..,target}".into(),
            "rm -rf ~someone/x".into(),
            "TMPDIR=/; rm -rf $TMPDIR/etc".into(),
            "ls | xargs rm -rf".into(),
            "find / -name x -exec rm -rf {} +".into(),
            "sudo rm -rf /var/x".into(),
            "/bin/rm -rf /var/x".into(),
            "\\rm -rf /var/x".into(),
            format!("rm -rf {base}/wt"),
            format!("rm -rf {base}/state"),
            format!("rm -rf {base}/other"),
        ] {
            let why = refused(&b, &c);
            assert!(why.contains(REMOVE), "{c}: {why}");
        }
    }

    #[test]
    fn relative_targets_follow_cd() {
        let b = fixture();
        let base = b.base.display();
        for c in [
            "cd crates && rm -rf x".to_string(),
            "cd crates; rm -rf ../target".into(),
            "cd $TMPDIR && rm -rf x".into(),
            "(cd /); rm -rf target".into(),
            "echo $(cd /) && rm -rf target".into(),
        ] {
            allowed(&b, &c);
        }
        for c in [
            "cd /tmp && rm -rf ~/x".to_string(),
            "cd .. && rm -rf other".into(),
            format!("cd {base}/other && rm -rf x"),
            "cd && rm -rf x".into(),
            "cd - && rm -rf x".into(),
            "cd $SOMEWHERE && rm -rf x".into(),
            "(cd / && rm -rf x)".into(),
            "cd crates && rm -rf ../../other".into(),
            "bash -c 'cd / && rm -rf x'".into(),
        ] {
            refused(&b, &c);
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_are_followed_only_where_rm_follows_them() {
        let b = fixture();
        std::os::unix::fs::symlink(b.base.join("other"), b.base.join("wt/link")).unwrap();
        allowed(&b, "rm -rf link");
        refused(&b, "rm -rf link/");
        refused(&b, "rm -rf link/x");
        refused(&b, "rm -rf link/*");
        refused(&b, "cd link && rm -rf x");
    }

    #[test]
    fn the_hook_denies_with_a_reason_and_is_silent_otherwise() {
        let none = |_: &str| None;
        let input =
            |c: &str| json!({"tool_name": "Bash", "tool_input": {"command": c}}).to_string();
        let out = hook_guard(&input("git stash clear"), &none).unwrap();
        let out = &out["hookSpecificOutput"];
        assert_eq!(out["permissionDecision"], "deny");
        assert!(out["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains(STASH));
        assert!(hook_guard(&input("git status"), &none).is_none());
        assert!(hook_guard("not json", &none).is_none());
        let read = json!({"tool_name": "Read", "tool_input": {}}).to_string();
        assert!(hook_guard(&read, &none).is_none());
        // With nothing to say where the worktree is, a relative target is refused.
        assert!(hook_guard(&input("rm -rf target"), &none).is_some());
    }

    #[test]
    fn without_the_adapters_variables_the_checkout_is_the_worktree() {
        let b = fixture();
        std::fs::create_dir(b.base.join("wt/.git")).unwrap();
        let cwd = b.base.join("wt/crates");
        let scope = Scope::from_hook(&json!({"cwd": cwd}), &|_| None);
        assert_eq!(scope.roots, [b.base.join("wt")]);
        assert_eq!(check("rm -rf ../target", &scope), None);
        assert!(check("rm -rf ../../other", &scope).is_some());
    }
}
