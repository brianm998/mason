// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a suite or a check runs (#698): on this Mac or on GitHub Actions.
//!
//! One decision, [`decide`], from plain facts, so a table test can cover every row. The facts
//! come from [`facts_for`]: the global switch (`remote.toml`, read live on every call, so a
//! change needs no restart and no `project reload`), the task's own choice (#699, its `run_on` override), the project's `[remote_runs]`, and the suite's `remote`, `runs_on`, environment and
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

/// The placement mode (#831): where runs go. Set for all of MASON (`remote.toml`), per project
/// (`[remote_runs] mode`) and per task (`run_on`); the most specific level that is set wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Everything on this machine.
    Local,
    /// Everything on GitHub.
    Remote,
    /// Here until this machine is overloaded, then GitHub.
    FavorLocal,
    /// GitHub until it is overloaded, then here.
    FavorRemote,
}

impl Mode {
    pub const ALL: [Mode; 4] = [
        Mode::Local,
        Mode::Remote,
        Mode::FavorLocal,
        Mode::FavorRemote,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Local => "local",
            Mode::Remote => "remote",
            Mode::FavorLocal => "favor-local",
            Mode::FavorRemote => "favor-remote",
        }
    }

    /// The four names, and the old `github` (= `remote`).
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "local" => Some(Mode::Local),
            "remote" | "github" => Some(Mode::Remote),
            "favor-local" => Some(Mode::FavorLocal),
            "favor-remote" => Some(Mode::FavorRemote),
            _ => None,
        }
    }
}

/// Where a decision came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Task,
    Project,
    Global,
    Suite,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Task => "task",
            Source::Project => "project",
            Source::Global => "global switch",
            Source::Suite => "suite rule",
        }
    }

    /// The level's name in JSON: `task`, `project`, `global`, `suite`.
    pub fn key(self) -> &'static str {
        match self {
            Source::Task => "task",
            Source::Project => "project",
            Source::Global => "global",
            Source::Suite => "suite",
        }
    }
}

/// Everything the decision reads.
#[derive(Clone, Debug)]
pub struct Facts {
    /// The task's own mode (its `run_on` override); `None` for a task that has none and for a run
    /// no task owns.
    pub task: Option<Mode>,
    /// The project's `[remote_runs]` mode (`mode`, or the old `fallback`); `None`: follows global.
    pub project: Option<Mode>,
    pub global: Mode,
    /// The project's `[remote_runs] runs_on`; `None` when the project has no `[remote_runs]`.
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
    /// A project with a `[remote_runs]` and a suite with no rules of its own.
    pub fn plain(global: Mode) -> Facts {
        Facts {
            task: None,
            project: None,
            global,
            project_runs_on: Some("ubuntu-latest".into()),
            suite_remote: true,
            suite_runs_on: None,
            device: false,
            exclusive: false,
            setup_failed: None,
        }
    }

    /// The mode in force and the level that set it: task, then project, then global.
    pub fn effective(&self) -> (Mode, Source) {
        if let Some(m) = self.task {
            (m, Source::Task)
        } else if let Some(m) = self.project {
            (m, Source::Project)
        } else {
            (self.global, Source::Global)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Local {
        why: String,
        from: Source,
    },
    GitHub {
        runs_on: String,
        from: Source,
        /// For a balancing mode: why GitHub ("favor-remote, GitHub had room"). Empty otherwise.
        why: String,
    },
}

impl Place {
    pub fn is_github(&self) -> bool {
        matches!(self, Place::GitHub { .. })
    }

    /// "this Mac, because ..." / "GitHub (macos-15), from the global switch".
    pub fn text(&self) -> String {
        match self {
            Place::Local { why, .. } => format!("on this Mac: {why}"),
            Place::GitHub { runs_on, from, why } if why.is_empty() => {
                format!("on GitHub ({runs_on}), from the {}", from.label())
            }
            Place::GitHub { runs_on, from, why } => {
                format!("on GitHub ({runs_on}), from the {}: {why}", from.label())
            }
        }
    }

    pub fn from(&self) -> Source {
        match self {
            Place::Local { from, .. } | Place::GitHub { from, .. } => *from,
        }
    }
}

/// The two balancing modes (#831). Called only for `favor-local` and `favor-remote`, after the
/// suite's own rules have let the run go to GitHub. For now `favor-local` is `local` and
/// `favor-remote` is `remote`; the bounded fallback to this machine after `pickup_timeout`
/// (s812, `remote::fallback_lease`) is what spills `favor-remote` over. The load rules of the
/// follow-up tickets replace this body.
pub fn balance(mode: Mode, f: &Facts, runs_on: &str, from: Source) -> Place {
    match mode {
        Mode::FavorLocal => Place::Local {
            why: "favor-local, this machine had room".into(),
            from,
        },
        _ => Place::GitHub {
            runs_on: runs_on.to_string(),
            from,
            why: format!(
                "favor-remote, GitHub had room{}",
                if f.suite_runs_on.is_some() {
                    " (suite runner)"
                } else {
                    ""
                }
            ),
        },
    }
}

/// The one decision. The mode comes from the task, else the project, else the global switch. A
/// run that would go to GitHub stays here when the project has no `[remote_runs]`, the suite says
/// `remote = false`, its environment leases a device, it uses `exclusive`, or the remote setup
/// check failed. The reason is always given.
pub fn decide(f: &Facts) -> Place {
    let (mode, from) = f.effective();
    if mode == Mode::Local {
        return Place::Local {
            why: match from {
                Source::Task => "the task asked to run here".into(),
                Source::Project => "the project's mode is local".into(),
                _ => "the global switch is off".into(),
            },
            from,
        };
    }
    let stay = |why: String| Place::Local {
        why,
        from: Source::Suite,
    };
    let Some(project_runs_on) = &f.project_runs_on else {
        return stay("the project has no [remote_runs] section".into());
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
    let runs_on = f.suite_runs_on.as_ref().unwrap_or(project_runs_on);
    match mode {
        Mode::Remote => Place::GitHub {
            runs_on: runs_on.clone(),
            from,
            why: String::new(),
        },
        _ => balance(mode, f, runs_on, from),
    }
}

// ---- the global switch --------------------------------------------------------------------

pub fn remote_path(d: &Daemon) -> std::path::PathBuf {
    d.cfg.config_dir.join("remote.toml")
}

/// What `remote.toml` says now. Read on every call. A missing file is local; so is one that
/// cannot be read or says something else, with the problem in the second part.
pub fn read_global(d: &Daemon) -> (Mode, Option<String>) {
    let text = match std::fs::read_to_string(remote_path(d)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Mode::Local, None),
        Err(e) => return (Mode::Local, Some(format!("remote.toml: {e}"))),
    };
    match parse_default(&text) {
        Ok(g) => (g, None),
        Err(e) => (Mode::Local, Some(e)),
    }
}

/// The mode in `remote.toml`: `mode = "<mode>"`, else the old `default = "local" | "github"`.
pub fn parse_default(text: &str) -> Result<Mode, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("remote.toml: {e}"))?;
    if let Some(i) = doc.get("mode") {
        return i.as_str().and_then(Mode::parse).ok_or_else(|| {
            "remote.toml: `mode` must be \"local\", \"remote\", \"favor-local\" or \"favor-remote\""
                .to_string()
        });
    }
    match doc.get("default") {
        None => Ok(Mode::Local),
        Some(i) => match i.as_str() {
            Some("local") => Ok(Mode::Local),
            Some("github") => Ok(Mode::Remote),
            _ => Err("remote.toml: `default` must be \"local\" or \"github\"".into()),
        },
    }
}

static EDITING: Mutex<()> = Mutex::new(());

/// Change the switch. The file's comments and other keys stay; who and when are recorded in it
/// and in `remote-changes.log`.
pub fn set_global(d: &Daemon, to: Mode, who: &str) -> Result<Value, (u16, String)> {
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
    let was = parse_default(&old).unwrap_or(Mode::Local);
    let now = now_ms();
    doc["mode"] = value(to.name());
    // The old key would be shadowed by `mode`; drop it so the file says one thing.
    doc.remove("default");
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
    Ok(
        json!({"mode": to.name(), "default": to.name(), "was": was.name(), "changed_by": who, "changed_at": now}),
    )
}

/// The project's own mode, from its `[remote_runs]`: `mode`, else the old `fallback` (`local` is
/// `favor-remote`, `none` is `remote`). `None`: it follows the global mode, or has no
/// `[remote_runs]` at all.
pub fn project_mode(d: &Daemon, project: &str) -> Option<Mode> {
    let r = d
        .project_id_by_name(project)
        .and_then(|p| d.project(p))?
        .services
        .remote
        .as_ref()?;
    r.mode.as_deref().and_then(Mode::parse)
}

/// The project's effective mode and where it came from, before any task's choice: for
/// `mason remote status`. A project with no `[remote_runs]` is always local.
pub fn project_effective(d: &Daemon, project: &str) -> (Mode, &'static str) {
    let has = d
        .project_id_by_name(project)
        .and_then(|p| d.project(p))
        .is_some_and(|p| p.services.remote.is_some());
    if !has {
        return (Mode::Local, "no [remote_runs]");
    }
    match project_mode(d, project) {
        Some(m) => (m, "project"),
        None => (read_global(d).0, "global"),
    }
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
            let (mode, source) = project_effective(d, &p.id);
            json!({"project": p.id, "remote": p.services.remote.is_some(),
                   "runs_on": p.services.remote.as_ref().map(|r| r.runs_on.clone()),
                   "mode": mode.name(), "source": source})
        })
        .collect();
    json!({"mode": g.name(), "default": if g == Mode::Remote { "github" } else { g.name() },
           "problem": problem, "changed_by": by, "changed_at": at, "projects": projects})
}

/// Set `[remote_runs] mode` in a project's registration file and re-read it (like `project
/// reload`). The file's comments and other keys stay. A project with no `[remote_runs]` is
/// always local, so there is nothing to set and the answer says so.
pub fn set_project_mode(d: &Daemon, project: &str, to: Mode) -> Result<Value, (u16, String)> {
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    if d.project_id_by_name(project).is_none() {
        return Err((404, format!("no project {project:?}")));
    }
    let path = d
        .cfg
        .config_dir
        .join("projects")
        .join(format!("{project}.toml"));
    let old =
        std::fs::read_to_string(&path).map_err(|e| (500, format!("{}: {e}", path.display())))?;
    let mut doc: DocumentMut = old
        .parse()
        .map_err(|e| (400, format!("{}: {e}", path.display())))?;
    let Some(t) = doc
        .get_mut("remote_runs")
        .and_then(|i| i.as_table_like_mut())
    else {
        return Err((
            400,
            format!("{project} has no [remote_runs], so it always runs here; add one first"),
        ));
    };
    t.insert("mode", value(to.name()));
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).map_err(|e| (500, format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        (500, format!("{}: {e}", path.display()))
    })?;
    let changed = d
        .reload_registration(project)
        .map_err(|e| (400, e.join("; ")))?;
    Ok(json!({"project": project, "mode": to.name(), "source": "project", "changed": changed}))
}

// ---- the facts of a real run ---------------------------------------------------------------

/// The task's own mode: its `run_on` override, read live so a change applies from the next suite
/// or check. Nothing, or a value that is no mode, is no choice. The old `github` is `remote`.
pub fn task_choice(d: &Daemon, task: Option<u64>) -> Option<Mode> {
    let v = d.store.task_overrides(task?).ok()?;
    Mode::parse(
        v.iter()
            .find(|(k, _)| k == mason_definition::layers::RUN_ON)?
            .1
            .as_str(),
    )
}

/// The place of a run no suite describes (a landing check, the checks of a task's develop step,
/// the whole step in `pool_needs`): the task's mode when `task` is given, else the project's,
/// else the global one; the project's `[remote_runs]` and the setup check; nothing of a suite's
/// own.
pub fn plain_place(d: &Daemon, project: &str, task: Option<u64>) -> Place {
    let pid = d.project_id_by_name(project);
    let mut f = Facts::plain(read_global(d).0);
    f.task = task_choice(d, task);
    f.project = project_mode(d, project);
    f.project_runs_on = pid
        .and_then(|p| d.project(p))
        .and_then(|p| p.services.remote.as_ref().map(|r| r.runs_on.clone()));
    decide(&f)
}

/// Do the checks of `task` in `project` go to GitHub (#704)? Whether a suite then stays here
/// for its own reasons does not matter: the agent is told to use `run_check` either way.
pub fn task_checks_on_github(d: &Daemon, project: &str, task: u64) -> bool {
    plain_place(d, project, Some(task)).is_github()
}

/// "runs on: GitHub (task choice)" / "runs on: this Mac (global switch)": the place a task's
/// runs go before the suite's own rules, and where that came from. For `mason task show` and
/// the task detail the monitors read.
pub fn effective_line(d: &Daemon, task: u64) -> Value {
    let project = d
        .world()
        .ok()
        .and_then(|w| w.tasks.get(&mason_core::TaskId(task)).map(|t| t.project))
        .and_then(|p| d.project(p))
        .map(|p| p.id.clone());
    let choice = task_choice(d, Some(task));
    let (mode, from, has_remote) = match &project {
        Some(p) => {
            let has = d
                .project_id_by_name(p)
                .and_then(|i| d.project(i))
                .is_some_and(|p| p.services.remote.is_some());
            let mut f = Facts::plain(read_global(d).0);
            f.task = choice;
            f.project = project_mode(d, p);
            let (m, s) = f.effective();
            (m, s, has)
        }
        None => {
            let mut f = Facts::plain(read_global(d).0);
            f.task = choice;
            let (m, s) = f.effective();
            (m, s, true)
        }
    };
    let (mode, from, why) = if has_remote {
        (mode, from, None)
    } else {
        (Mode::Local, Source::Project, Some("no [remote_runs]"))
    };
    let label = match (from, why) {
        (_, Some(w)) => w.to_string(),
        (Source::Task, _) => "task choice".to_string(),
        (s, _) => s.label().to_string(),
    };
    let line = match mode {
        Mode::Local => format!("runs on: this Mac ({label})"),
        Mode::Remote => format!("runs on: GitHub ({label})"),
        Mode::FavorLocal => format!("runs on: favor-local, this Mac first ({label})"),
        Mode::FavorRemote => format!("runs on: favor-remote, GitHub first ({label})"),
    };
    json!({
        "mode": mode.name(),
        "where": match mode {
            Mode::Remote | Mode::FavorRemote => "github",
            _ => "local",
        },
        "from": from.key(),
        "line": line,
    })
}

/// The facts for a suite of a project. `task` is the task whose run it is (its own choice
/// comes from it).
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
        project: project_mode(d, project),
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

/// One run of a landing batch (a tier, a rerun, a bisect half): its id, which starts `b<batch>-`,
/// its suite and where it ran, with the run's link when it ran on GitHub (#702).
type LandingRun = (String, String, String, String, u64);
static LANDING_RUNS: Mutex<Vec<LandingRun>> = Mutex::new(Vec::new());

/// Remember where a run no task owns ran. Newest 200 kept.
pub fn note_landing_run(project: &str, id: &str, suite: &str, text: &str) {
    let mut v = LANDING_RUNS.lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|r| !(r.0 == project && r.1 == id));
    v.push((
        project.into(),
        id.into(),
        suite.into(),
        text.into(),
        now_ms(),
    ));
    let n = v.len();
    if n > 200 {
        v.drain(..n - 200);
    }
}

/// The recent landing runs, oldest first, for `GET /landing`.
pub fn landing_runs() -> Vec<Value> {
    LANDING_RUNS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(p, id, suite, text, at)| {
            json!({"project": p, "id": id, "suite": suite, "where": text, "at": at})
        })
        .collect()
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

/// Where a run goes that no task owns and no suite describes (a landing check, #702): the
/// project's mode, else the global one, the project's `[remote_runs]`, and nothing of a suite's
/// own.
pub fn global_place(d: &Daemon, project: &str) -> Place {
    let p = plain_place(d, project, None);
    note(project, None, "landing", "check", &p);
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
    #[test]
    fn a_landing_run_is_remembered_once_with_where_it_ran() {
        note_landing_run("p733", "b9-u-unit", "unit", "ran on this Mac");
        note_landing_run(
            "p733",
            "b9-u-unit",
            "unit",
            "ran on GitHub (ubuntu-latest), run http://x/1",
        );
        let mine: Vec<Value> = landing_runs()
            .into_iter()
            .filter(|r| r["project"] == "p733")
            .collect();
        assert_eq!(mine.len(), 1, "{mine:?}");
        assert!(mine[0]["where"]
            .as_str()
            .unwrap()
            .contains("run http://x/1"));
    }

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
            why: String::new(),
        }
    }

    #[test]
    fn the_decision_table() {
        let on = || Facts::plain(Mode::Remote);
        let off = || Facts::plain(Mode::Local);
        row("global on", on(), gh("ubuntu-latest", Source::Global));
        row(
            "global off",
            off(),
            local("the global switch is off", Source::Global),
        );
        row(
            "task remote beats global off",
            Facts {
                task: Some(Mode::Remote),
                ..off()
            },
            gh("ubuntu-latest", Source::Task),
        );
        row(
            "task local beats global on",
            Facts {
                task: Some(Mode::Local),
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
            "project beats global",
            Facts {
                project: Some(Mode::Local),
                ..on()
            },
            local("the project's mode is local", Source::Project),
        );
        row(
            "project remote beats global local",
            Facts {
                project: Some(Mode::Remote),
                ..off()
            },
            gh("ubuntu-latest", Source::Project),
        );
        row(
            "task beats project",
            Facts {
                task: Some(Mode::Remote),
                project: Some(Mode::Local),
                ..off()
            },
            gh("ubuntu-latest", Source::Task),
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
            "task remote, suite remote = false",
            Facts {
                task: Some(Mode::Remote),
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

    /// Every (global, project, task) combination: the most specific set level wins, a project
    /// without `[remote_runs]` is local, and the favor modes balance (for now: favor-local here,
    /// favor-remote on GitHub).
    #[test]
    fn every_combination_of_the_three_levels() {
        let opts: Vec<Option<Mode>> = std::iter::once(None)
            .chain(Mode::ALL.iter().map(|m| Some(*m)))
            .collect();
        for g in Mode::ALL {
            for p in &opts {
                for t in &opts {
                    let f = Facts {
                        task: *t,
                        project: *p,
                        ..Facts::plain(g)
                    };
                    let want = t.or(*p).unwrap_or(g);
                    let src = if t.is_some() {
                        Source::Task
                    } else if p.is_some() {
                        Source::Project
                    } else {
                        Source::Global
                    };
                    let got = decide(&f);
                    let name = format!("global {g:?} project {p:?} task {t:?}");
                    match want {
                        Mode::Local | Mode::FavorLocal => {
                            assert!(!got.is_github(), "{name}: {got:?}");
                        }
                        Mode::Remote | Mode::FavorRemote => {
                            assert!(got.is_github(), "{name}: {got:?}");
                        }
                    }
                    assert_eq!(got.from(), src, "{name}");
                    // A project with no [remote_runs] is local, whatever the mode says.
                    let none = decide(&Facts {
                        project_runs_on: None,
                        ..f
                    });
                    assert!(!none.is_github(), "{name} without [remote_runs]: {none:?}");
                }
            }
        }
    }

    #[test]
    fn a_balanced_run_says_why_it_went_where_it_did() {
        let fr = Facts {
            task: Some(Mode::FavorRemote),
            ..Facts::plain(Mode::Local)
        };
        assert!(decide(&fr).text().contains("favor-remote, GitHub had room"));
        let fl = Facts {
            task: Some(Mode::FavorLocal),
            ..Facts::plain(Mode::Remote)
        };
        assert!(decide(&fl)
            .text()
            .contains("favor-local, this machine had room"));
    }

    #[test]
    fn the_old_values_still_read() {
        assert_eq!(Mode::parse("github"), Some(Mode::Remote));
        assert_eq!(Mode::parse("local"), Some(Mode::Local));
        assert_eq!(Mode::parse("favor-remote"), Some(Mode::FavorRemote));
        assert_eq!(Mode::parse("default"), None);
        assert_eq!(
            parse_default("mode = \"favor-local\""),
            Ok(Mode::FavorLocal)
        );
        // `mode` wins over a leftover `default`.
        assert_eq!(
            parse_default("default = \"github\"\nmode = \"local\"\n"),
            Ok(Mode::Local)
        );
        assert!(parse_default("mode = \"sometimes\"").is_err());
    }

    #[test]
    fn the_switch_file_parses() {
        assert_eq!(parse_default(""), Ok(Mode::Local));
        assert_eq!(parse_default("default = \"github\""), Ok(Mode::Remote));
        assert!(parse_default("default = \"yes\"").is_err());
    }

    #[test]
    fn toggling_keeps_comments_and_other_keys() {
        let old = "# my note\nextra = 1 # keep\ndefault = \"local\"\n";
        let mut doc: DocumentMut = old.parse().unwrap();
        doc["mode"] = value("remote");
        doc["changed_by"] = value("me");
        let t = doc.to_string();
        assert!(t.contains("# my note") && t.contains("extra = 1 # keep"));
        assert_eq!(parse_default(&t), Ok(Mode::Remote));
    }
}
