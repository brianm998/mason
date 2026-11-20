// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a suite or a check runs (#698): on this Mac or on GitHub Actions.
//!
//! One decision, [`decide`], from plain facts, so a table test can cover every row. The facts
//! come from [`facts_for`]: the global switch (`remote.toml`, read live on every call, so a
//! change needs no restart and no `project reload`), the task's own choice (#699, its `run_on` override), the project's `[remote]`, and the suite's `remote`, `runs_on`, environment and
//! `uses`. Moving a run changes no gate: the same command is judged by the same parser and
//! minimum.
//!
//! The switch lives in `<config>/remote.toml`:
//!
//! ```toml
//! default = "github"   # or "local"; a missing file means local
//! changed_by = "brian" # who made the last change
//! changed_at = 1763600000000
//! ```
//!
//! It is edited with `toml_edit` (comments and other keys survive) and each change is also
//! appended to `<config>/remote-changes.log`.

use crate::daemon::{now_ms, Daemon};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;
use toml_edit::{value, DocumentMut};

/// The global switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Global {
    Local,
    GitHub,
}

impl Global {
    pub fn name(self) -> &'static str {
        match self {
            Global::Local => "local",
            Global::GitHub => "github",
        }
    }
}

/// A task's own choice (#699).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Local,
    GitHub,
}

/// Where a decision came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Task,
    Global,
    Suite,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Task => "task",
            Source::Global => "global switch",
            Source::Suite => "suite rule",
        }
    }
}

/// Everything the decision reads.
#[derive(Clone, Debug)]
pub struct Facts {
    /// The task's own choice (#699); `None` for a task that has none and for a run no task owns.
    pub task: Option<Choice>,
    pub global: Global,
    /// The project's `[remote] runs_on`; `None` when the project has no `[remote]`.
    pub project_runs_on: Option<String>,
    /// `remote = false` on the suite keeps it here.
    pub suite_remote: bool,
    pub suite_runs_on: Option<String>,
    /// Its environment leases a device, simulator or emulator.
    pub device: bool,
    /// It uses `exclusive`.
    pub exclusive: bool,
    /// The project's remote setup check (#703) failed: why.
    pub setup_failed: Option<String>,
}

impl Facts {
    /// A project with a `[remote]` and a suite with no rules of its own.
    pub fn plain(global: Global) -> Facts {
        Facts {
            task: None,
            global,
            project_runs_on: Some("ubuntu-latest".into()),
            suite_remote: true,
            suite_runs_on: None,
            device: false,
            exclusive: false,
            setup_failed: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Local { why: String, from: Source },
    GitHub { runs_on: String, from: Source },
}

impl Place {
    pub fn is_github(&self) -> bool {
        matches!(self, Place::GitHub { .. })
    }

    /// "this Mac, because ..." / "GitHub (macos-15), from the global switch".
    pub fn text(&self) -> String {
        match self {
            Place::Local { why, .. } => format!("on this Mac: {why}"),
            Place::GitHub { runs_on, from } => {
                format!("on GitHub ({runs_on}), from the {}", from.label())
            }
        }
    }

    pub fn from(&self) -> Source {
        match self {
            Place::Local { from, .. } | Place::GitHub { from, .. } => *from,
        }
    }
}

/// The one decision. A task's choice comes first, then the global switch; a run that would go to
/// GitHub stays here when the project has no `[remote]`, the suite says `remote = false`, its
/// environment leases a device, it uses `exclusive`, or the remote setup check failed. The
/// reason is always given.
pub fn decide(f: &Facts) -> Place {
    let (wants, from) = match f.task {
        Some(Choice::Local) => {
            return Place::Local {
                why: "the task asked to run here".into(),
                from: Source::Task,
            }
        }
        Some(Choice::GitHub) => (true, Source::Task),
        None => (f.global == Global::GitHub, Source::Global),
    };
    if !wants {
        return Place::Local {
            why: "the global switch is off".into(),
            from: Source::Global,
        };
    }
    let stay = |why: String| Place::Local {
        why,
        from: Source::Suite,
    };
    let Some(project_runs_on) = &f.project_runs_on else {
        return stay("the project has no [remote] section".into());
    };
    if !f.suite_remote {
        return stay("the suite says remote = false".into());
    }
    if f.device {
        return stay("its environment leases a device, simulator or emulator".into());
    }
    if f.exclusive {
        return stay("it uses `exclusive`, which only this machine can give".into());
    }
    if let Some(why) = &f.setup_failed {
        return stay(format!("the project's remote setup check failed: {why}"));
    }
    match &f.suite_runs_on {
        Some(r) => Place::GitHub {
            runs_on: r.clone(),
            from,
        },
        None => Place::GitHub {
            runs_on: project_runs_on.clone(),
            from,
        },
    }
}

// ---- the global switch --------------------------------------------------------------------

pub fn remote_path(d: &Daemon) -> std::path::PathBuf {
    d.cfg.config_dir.join("remote.toml")
}

/// What `remote.toml` says now. Read on every call. A missing file is local; so is one that
/// cannot be read or says something else, with the problem in the second part.
pub fn read_global(d: &Daemon) -> (Global, Option<String>) {
    let text = match std::fs::read_to_string(remote_path(d)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Global::Local, None),
        Err(e) => return (Global::Local, Some(format!("remote.toml: {e}"))),
    };
    match parse_default(&text) {
        Ok(g) => (g, None),
        Err(e) => (Global::Local, Some(e)),
    }
}

pub fn parse_default(text: &str) -> Result<Global, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("remote.toml: {e}"))?;
    match doc.get("default") {
        None => Ok(Global::Local),
        Some(i) => match i.as_str() {
            Some("local") => Ok(Global::Local),
            Some("github") => Ok(Global::GitHub),
            _ => Err("remote.toml: `default` must be \"local\" or \"github\"".into()),
        },
    }
}

static EDITING: Mutex<()> = Mutex::new(());

/// Change the switch. The file's comments and other keys stay; who and when are recorded in it
/// and in `remote-changes.log`.
pub fn set_global(d: &Daemon, to: Global, who: &str) -> Result<Value, (u16, String)> {
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = remote_path(d);
    let old = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err((500, format!("remote.toml: {e}"))),
    };
    let mut doc: DocumentMut = old
        .parse()
        .map_err(|e| (400, format!("remote.toml: {e}")))?;
    let was = parse_default(&old).unwrap_or(Global::Local);
    let now = now_ms();
    doc["default"] = value(to.name());
    doc["changed_by"] = value(who);
    doc["changed_at"] = value(i64::try_from(now).unwrap_or(i64::MAX));
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).map_err(|e| (500, format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        (500, format!("{}: {e}", path.display()))
    })?;
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(d.cfg.config_dir.join("remote-changes.log"))
    {
        let _ = writeln!(f, "{now} {who} {} -> {}", was.name(), to.name());
    }
    Ok(json!({"default": to.name(), "was": was.name(), "changed_by": who, "changed_at": now}))
}

/// For `mason remote status` and the Settings page.
pub fn status(d: &Daemon) -> Value {
    let (g, problem) = read_global(d);
    let doc: Option<DocumentMut> = std::fs::read_to_string(remote_path(d))
        .ok()
        .and_then(|t| t.parse().ok());
    let by = doc
        .as_ref()
        .and_then(|d| d.get("changed_by"))
        .and_then(|i| i.as_str())
        .map(str::to_string);
    let at = doc
        .as_ref()
        .and_then(|d| d.get("changed_at"))
        .and_then(|i| i.as_integer());
    let projects: Vec<Value> = d
        .projects()
        .values()
        .map(|p| {
            json!({"project": p.id, "remote": p.services.remote.is_some(),
                   "runs_on": p.services.remote.as_ref().map(|r| r.runs_on.clone())})
        })
        .collect();
    json!({"default": g.name(), "problem": problem, "changed_by": by, "changed_at": at,
           "projects": projects})
}

// ---- the facts of a real run ---------------------------------------------------------------

/// The task's own choice (#699): its `run_on` override, read live so a change applies from the
/// next suite or check. Nothing, or a value that is not `github` or `local`, is no choice.
pub fn task_choice(d: &Daemon, task: Option<u64>) -> Option<Choice> {
    let v = d.store.task_overrides(task?).ok()?;
    match v
        .iter()
        .find(|(k, _)| k == mason_definition::layers::RUN_ON)?
        .1
        .as_str()
    {
        "github" => Some(Choice::GitHub),
        "local" => Some(Choice::Local),
        _ => None,
    }
}

/// "runs on: GitHub (task choice)" / "runs on: this Mac (global switch)": the place a task's
/// runs go before the suite's own rules, and where that came from. For `mason task show` and
/// the task detail the monitors read.
pub fn effective_line(d: &Daemon, task: u64) -> Value {
    let (choice, global) = (task_choice(d, Some(task)), read_global(d).0);
    let (github, source) = match choice {
        Some(c) => (c == Choice::GitHub, Source::Task),
        None => (global == Global::GitHub, Source::Global),
    };
    let place = if github { "GitHub" } else { "this Mac" };
    let from = match source {
        Source::Task => "task choice",
        _ => "global switch",
    };
    json!({
        "where": if github { "github" } else { "local" },
        "from": if choice.is_some() { "task" } else { "global" },
        "line": format!("runs on: {place} ({from})"),
    })
}

/// The facts for a suite of a project. `task` is the task whose run it is (its own choice,
/// #699, comes from it).
pub fn facts_for(
    d: &Daemon,
    project: &str,
    task: Option<u64>,
    suite: &mason_testing::Suite,
) -> Facts {
    let pid = d.project_id_by_name(project);
    let device = pid.is_some_and(|p| {
        d.current_def(p)
            .testing
            .environment(&suite.env)
            .is_some_and(|e| mason_testing::env::device_of(e).is_some())
    });
    Facts {
        task: task_choice(d, task),
        global: read_global(d).0,
        project_runs_on: pid
            .and_then(|p| d.project(p))
            .and_then(|p| p.services.remote.as_ref().map(|r| r.runs_on.clone())),
        suite_remote: suite.remote,
        suite_runs_on: suite.runs_on.clone(),
        device,
        exclusive: suite.uses.get("exclusive").is_some_and(|n| *n > 0),
        setup_failed: None,
    }
}

// ---- what is running where, for status and task show -----------------------------------------

#[derive(Clone, Debug)]
pub struct Seen {
    pub project: String,
    pub task: Option<u64>,
    pub name: String,
    pub kind: &'static str,
    pub place: Place,
    pub at: u64,
}

static SEEN: Mutex<BTreeMap<(String, u64, String), Seen>> = Mutex::new(BTreeMap::new());

/// Remember where a run went, for `mason status` and `mason task show`. Newest 300 kept.
pub fn note(project: &str, task: Option<u64>, name: &str, kind: &'static str, place: &Place) {
    let mut m = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let at = now_ms();
    m.insert(
        (
            project.to_string(),
            task.unwrap_or(0),
            format!("{kind}:{name}"),
        ),
        Seen {
            project: project.into(),
            task,
            name: name.into(),
            kind,
            place: place.clone(),
            at,
        },
    );
    while m.len() > 300 {
        let oldest = m.iter().min_by_key(|(_, s)| s.at).map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                m.remove(&k);
            }
            None => break,
        }
    }
}

fn seen_json(s: &Seen) -> Value {
    json!({
        "project": s.project, "task": s.task, "name": s.name, "kind": s.kind,
        "where": if s.place.is_github() { "github" } else { "local" },
        "runs_on": match &s.place { Place::GitHub { runs_on, .. } => json!(runs_on), _ => Value::Null },
        "why": s.place.text(), "from": s.place.from().label(), "at": s.at,
    })
}

/// The recent decisions, of one task or (None) all of them.
pub fn seen(task: Option<u64>) -> Vec<Value> {
    let m = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<&Seen> = m
        .values()
        .filter(|s| task.is_none() || s.task == task)
        .collect();
    v.sort_by_key(|s| s.at);
    v.into_iter().map(seen_json).collect()
}

/// Decide for a suite of a project and remember it.
pub fn place_of(
    d: &Daemon,
    project: &str,
    task: Option<u64>,
    suite: &mason_testing::Suite,
    kind: &'static str,
) -> Place {
    let p = decide(&facts_for(d, project, task, suite));
    note(project, task, &suite.name, kind, &p);
    p
}

/// Each suite's effective place and where it came from, for `mason config show`.
pub fn config_view(d: &Daemon, project: &str) -> Vec<Value> {
    let Some(pid) = d.project_id_by_name(project) else {
        return vec![];
    };
    d.current_def(pid)
        .testing
        .suites
        .iter()
        .map(|s| {
            let p = decide(&facts_for(d, project, None, s));
            json!({"suite": s.name, "where": if p.is_github() {"github"} else {"local"},
                   "text": p.text(), "from": p.from().label()})
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, f: Facts, want: Place) {
        assert_eq!(decide(&f), want, "{name}");
    }

    fn local(why: &str, from: Source) -> Place {
        Place::Local {
            why: why.into(),
            from,
        }
    }

    fn gh(r: &str, from: Source) -> Place {
        Place::GitHub {
            runs_on: r.into(),
            from,
        }
    }

    #[test]
    fn the_decision_table() {
        let on = || Facts::plain(Global::GitHub);
        let off = || Facts::plain(Global::Local);
        row("global on", on(), gh("ubuntu-latest", Source::Global));
        row(
            "global off",
            off(),
            local("the global switch is off", Source::Global),
        );
        row(
            "task github beats global off",
            Facts {
                task: Some(Choice::GitHub),
                ..off()
            },
            gh("ubuntu-latest", Source::Task),
        );
        row(
            "task local beats global on",
            Facts {
                task: Some(Choice::Local),
                ..on()
            },
            local("the task asked to run here", Source::Task),
        );
        row(
            "task none follows global",
            Facts { task: None, ..on() },
            gh("ubuntu-latest", Source::Global),
        );
        row(
            "suite runs_on",
            Facts {
                suite_runs_on: Some("macos-15".into()),
                ..on()
            },
            gh("macos-15", Source::Global),
        );
        row(
            "suite remote = false",
            Facts {
                suite_remote: false,
                ..on()
            },
            local("the suite says remote = false", Source::Suite),
        );
        row(
            "task github, suite remote = false",
            Facts {
                task: Some(Choice::GitHub),
                suite_remote: false,
                ..off()
            },
            local("the suite says remote = false", Source::Suite),
        );
        row(
            "device environment",
            Facts {
                device: true,
                ..on()
            },
            local(
                "its environment leases a device, simulator or emulator",
                Source::Suite,
            ),
        );
        row(
            "exclusive",
            Facts {
                exclusive: true,
                ..on()
            },
            local(
                "it uses `exclusive`, which only this machine can give",
                Source::Suite,
            ),
        );
        row(
            "no [remote]",
            Facts {
                project_runs_on: None,
                ..on()
            },
            local("the project has no [remote] section", Source::Suite),
        );
        row(
            "no [remote], global off says the switch",
            Facts {
                project_runs_on: None,
                ..off()
            },
            local("the global switch is off", Source::Global),
        );
        row(
            "setup check failed",
            Facts {
                setup_failed: Some("no runner".into()),
                ..on()
            },
            local(
                "the project's remote setup check failed: no runner",
                Source::Suite,
            ),
        );
    }

    #[test]
    fn the_switch_file_parses() {
        assert_eq!(parse_default(""), Ok(Global::Local));
        assert_eq!(parse_default("default = \"github\""), Ok(Global::GitHub));
        assert!(parse_default("default = \"yes\"").is_err());
    }

    #[test]
    fn toggling_keeps_comments_and_other_keys() {
        let old = "# my note\nextra = 1 # keep\ndefault = \"local\"\n";
        let mut doc: DocumentMut = old.parse().unwrap();
        doc["default"] = value("github");
        doc["changed_by"] = value("me");
        let t = doc.to_string();
        assert!(t.contains("# my note") && t.contains("extra = 1 # keep"));
        assert_eq!(parse_default(&t), Ok(Global::GitHub));
    }
}
