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
    /// What `favor-local` weighs (#832); `None` reads as "this machine has room".
    pub spill: Option<Spill>,
    /// What `favor-remote` weighs (#833); `None` reads as "GitHub had room".
    pub remote_spill: Option<RemoteSpill>,
}

/// A reading of this machine's load, built from the scheduler's state (#832). Only the slots a
/// run needs are in `free`, `waiting` and `oldest_wait_ms`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalLoad {
    /// An active hold ("held for heat (die 99 °C)", "held for CPU (87% busy, limit 50%)").
    pub held: Option<String>,
    /// Free units of each slot the run needs.
    pub free: BTreeMap<String, u32>,
    /// Requests already waiting for any of those slots.
    pub waiting: u32,
    /// A waiting exclusive request (label): everything behind it waits for the machine to
    /// drain, so there is no room here whatever the free units say (2026-11-23).
    pub exclusive_waiting: Option<String>,
    /// The longest any of them has waited, in ms.
    pub oldest_wait_ms: u64,
}

impl LocalLoad {
    /// Build it from a scheduler reading, for a run that needs `needs`.
    pub fn from_reading(r: &crate::sched::LoadReading, needs: &[String]) -> LocalLoad {
        let mut load = LocalLoad {
            held: r.held.clone(),
            exclusive_waiting: r.exclusive_waiting.clone(),
            ..LocalLoad::default()
        };
        for n in needs {
            let used = r.used.get(n).copied().unwrap_or(0);
            let free = match r.limits.get(n) {
                Some(l) => l.saturating_sub(used),
                None => 1,
            };
            load.free.insert(n.clone(), free);
        }
        for (names, waited) in &r.waiting {
            if names.iter().any(|x| needs.contains(x)) {
                load.waiting += 1;
                load.oldest_wait_ms = load.oldest_wait_ms.max(*waited);
            }
        }
        load
    }

    /// Why this machine is overloaded, or `None`: a hold is active; or none of the needed slots
    /// is free and requests are already waiting; or the oldest wait is over `spill_after_ms`.
    /// Pure.
    pub fn overloaded(&self, spill_after_ms: u64) -> Option<String> {
        if let Some(h) = &self.held {
            return Some(format!("this machine {h}"));
        }
        if let Some(x) = &self.exclusive_waiting {
            return Some(format!("an exclusive request ({x}) is waiting here"));
        }
        let full = !self.free.is_empty() && self.free.values().all(|n| *n == 0);
        if full && self.waiting > 0 {
            let names: Vec<&str> = self.free.keys().map(String::as_str).collect();
            return Some(format!(
                "no {} slot free and {} waiting",
                names.join("/"),
                self.waiting
            ));
        }
        if self.waiting > 0 && self.oldest_wait_ms > spill_after_ms {
            return Some(format!(
                "the oldest local wait is {} s, over {} s",
                self.oldest_wait_ms / 1000,
                spill_after_ms / 1000
            ));
        }
        None
    }

    /// Whether a run can start here now (#833): no hold is active and, when slots are named, one
    /// of them is free.
    pub fn has_room(&self) -> bool {
        self.held.is_none()
            && self.exclusive_waiting.is_none()
            && (self.free.is_empty() || self.free.values().any(|n| *n > 0))
    }

    /// Why a run that would hold the slots in `free` cannot start here now, or `None` when it
    /// can: a hold is active, or one of those slots has no free unit. `covered`: the caller
    /// already holds those slots for this very run (a landing batch, s911), so their use is not
    /// counted against it; a hold still is, since new work is not welcome while it lasts. Pure.
    pub fn no_room(&self, covered: bool) -> Option<String> {
        if let Some(h) = &self.held {
            return Some(format!("this machine {h}"));
        }
        if covered {
            return None;
        }
        // An exclusive request at the head of the queue blocks every other request until the
        // machine drains, so nothing placed here would start.
        if let Some(x) = &self.exclusive_waiting {
            return Some(format!(
                "an exclusive request ({x}) is waiting here and blocks the queue"
            ));
        }
        // Room means a grant now (#1025): a free unit for each slot beyond the requests already
        // waiting for any of them, which the same gate holds back.
        let waiting = self.waiting;
        let blocked: Vec<String> = self
            .free
            .iter()
            .filter(|(_, n)| **n <= waiting)
            .map(|(k, n)| {
                if *n == 0 {
                    format!("{k} slot free")
                } else {
                    format!("{k} unit beyond the {waiting} waiting ({n} free)")
                }
            })
            .collect();
        (!blocked.is_empty()).then(|| format!("no {} here", blocked.join(", no ")))
    }

    /// "2 compile slots free" for the local reason.
    pub fn room_text(&self) -> String {
        let free: Vec<String> = self
            .free
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(k, n)| format!("{n} {k} slot{} free", if *n == 1 { "" } else { "s" }))
            .collect();
        if free.is_empty() {
            "this machine had room".into()
        } else {
            free.join(", ")
        }
    }
}

/// A reading of GitHub's load (#833). Pure data, so a table test covers the decision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteLoad {
    /// Free units of the `gha` pool (and `gha-macos` for a macOS runner).
    pub pool_free: u32,
    /// Requests waiting for those units.
    pub pool_waiting: u32,
    /// Jobs queued on the whole account; `None` until something counts them (#819).
    pub account_queued: Option<u32>,
    /// The median pickup time of the last few runs, in seconds.
    pub recent_pickup_s: Option<u64>,
}

impl RemoteLoad {
    /// Why GitHub is overloaded, or `None`: no free pool unit with others already waiting; more
    /// than `queue_max` jobs queued on the account; or recent pickups slower than `spill_after_s`.
    pub fn overloaded(&self, queue_max: u32, spill_after_s: u64) -> Option<String> {
        if self.pool_free == 0 && self.pool_waiting > 0 {
            return Some(format!(
                "the runner pool is full with {} waiting",
                self.pool_waiting
            ));
        }
        if let Some(q) = self.account_queued.filter(|q| *q > queue_max) {
            return Some(format!("GitHub had {q} queued"));
        }
        if let Some(p) = self.recent_pickup_s.filter(|p| *p > spill_after_s) {
            return Some(format!("recent pickups took {p} s, over {spill_after_s} s"));
        }
        None
    }
}

/// The inputs of the `favor-remote` rules (#833).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteSpill {
    pub local: LocalLoad,
    pub remote: RemoteLoad,
    pub queue_max: u32,
    pub spill_after_s: u64,
}

/// Why a run bound for GitHub cannot be taken back by this machine now, or `None` when it can
/// (a room check for `remote::fallback_lease`): no CPU or thermal hold is active, and each slot
/// the fallback would hold for `uses` has a free unit. The same slots that lease asks for, so
/// "room" here means the lease would be granted at once. `covered`: see
/// [`LocalLoad::no_room`]. `exclusive` is not looked at. Pure.
pub fn no_room_for(
    r: &crate::sched::LoadReading,
    uses: &BTreeMap<String, u32>,
    covered: bool,
) -> Option<String> {
    let needs: Vec<String> = uses
        .iter()
        .filter(|(k, n)| **n > 0 && k.as_str() != "exclusive")
        .map(|(k, _)| k.clone())
        .collect();
    LocalLoad::from_reading(r, &needs).no_room(covered)
}

/// The slots a local run of a suite needs: compile and what the suite `uses`.
pub fn local_needs(uses: &BTreeMap<String, u32>) -> Vec<String> {
    let mut needs = vec![mason_scheduler::COMPILE.to_string()];
    needs.extend(
        uses.iter()
            .filter(|(k, n)| {
                **n > 0 && k.as_str() != mason_scheduler::COMPILE && k.as_str() != "exclusive"
            })
            .map(|(k, _)| k.clone()),
    );
    needs
}

/// The inputs of the `favor-local` rules besides the mode itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spill {
    pub load: LocalLoad,
    /// GitHub has a free `gha` unit (and `gha-macos` for a macOS runner).
    pub github_room: bool,
    pub spill_after_ms: u64,
}

/// A local run already queued for `waited_ms` while a hold is active moves to GitHub when GitHub
/// has room and the wait is over `spill_after_ms`. Returns why. Pure (#832).
pub fn replace_waiting(waited_ms: u64, sp: &Spill) -> Option<String> {
    let held = sp.load.held.as_ref()?;
    if sp.github_room && waited_ms > sp.spill_after_ms {
        Some(format!(
            "waited {} s here while this machine {held}",
            waited_ms / 1000
        ))
    } else {
        None
    }
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
            spill: None,
            remote_spill: None,
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
/// suite's own rules have let the run go to GitHub. `favor-local` runs here unless this machine
/// is overloaded and GitHub has room (#832); `favor-remote` is `remote`; the bounded fallback to this machine after `pickup_timeout`
/// (s812, `remote::fallback_lease`) is what spills `favor-remote` over. The load rules of the
/// follow-up tickets replace this body.
pub fn balance(mode: Mode, f: &Facts, runs_on: &str, from: Source) -> Place {
    match mode {
        Mode::FavorLocal => {
            let local = |why: String| Place::Local {
                why: format!("favor-local, {why}"),
                from,
            };
            let Some(sp) = &f.spill else {
                return local("this machine had room".into());
            };
            match sp.load.overloaded(sp.spill_after_ms) {
                None => local(sp.load.room_text()),
                Some(why) if sp.github_room => Place::GitHub {
                    runs_on: runs_on.to_string(),
                    from,
                    why: format!("favor-local, {why}"),
                },
                Some(why) => local(format!("{why}, and GitHub is full too, so it queues here")),
            }
        }
        _ => {
            let github = |why: String| Place::GitHub {
                runs_on: runs_on.to_string(),
                from,
                why,
            };
            let suffix = if f.suite_runs_on.is_some() {
                " (suite runner)"
            } else {
                ""
            };
            let Some(sp) = &f.remote_spill else {
                return github(format!("favor-remote, GitHub had room{suffix}"));
            };
            match sp.remote.overloaded(sp.queue_max, sp.spill_after_s) {
                None => github(format!("favor-remote, GitHub had room{suffix}")),
                Some(why) if sp.local.has_room() => Place::Local {
                    why: format!("favor-remote, {why}; this machine had room"),
                    from,
                },
                Some(why) => {
                    let busy = sp
                        .local
                        .held
                        .clone()
                        .or_else(|| {
                            sp.local
                                .exclusive_waiting
                                .as_ref()
                                .map(|x| format!("has an exclusive request ({x}) waiting"))
                        })
                        .unwrap_or_else(|| "has no slot free".into());
                    github(format!(
                        "favor-remote, {why}, but this machine {busy}, so it waits on GitHub{suffix}"
                    ))
                }
            }
        }
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
    write_project_mode(d, project, Some(to))
}

/// Remove `[remote_runs] mode` (and the old `fallback`), so the project follows the global mode
/// again ("Follow MASON" in the monitors).
pub fn clear_project_mode(d: &Daemon, project: &str) -> Result<Value, (u16, String)> {
    write_project_mode(d, project, None)
}

fn write_project_mode(d: &Daemon, project: &str, to: Option<Mode>) -> Result<Value, (u16, String)> {
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
    match to {
        Some(m) => {
            t.insert("mode", value(m.name()));
        }
        None => {
            t.remove("mode");
            t.remove("fallback");
        }
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).map_err(|e| (500, format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        (500, format!("{}: {e}", path.display()))
    })?;
    let changed = d
        .reload_registration(project)
        .map_err(|e| (400, e.join("; ")))?;
    let (mode, source) = project_effective(d, project);
    Ok(json!({"project": project, "mode": mode.name(), "source": source, "changed": changed}))
}

/// The placement of a task as the views carry it: the mode in force and the level that set it.
/// `project` is the task's project name; `None` for no task.
pub fn task_placement(d: &Daemon, project: &str, task: u64) -> mason_protocol::Placement {
    let mut f = Facts::plain(read_global(d).0);
    f.task = task_choice(d, Some(task));
    f.project = project_mode(d, project);
    let has = has_remote(d, project);
    let (m, s) = if has {
        f.effective()
    } else {
        (Mode::Local, Source::Project)
    };
    mason_protocol::Placement {
        mode: m.name().into(),
        source: s.key().into(),
        own: f.task.is_some(),
        settable: true,
    }
}

/// A project's placement: its own mode, else the global one. A project with no `[remote_runs]`
/// always runs here and cannot be set.
pub fn project_placement(d: &Daemon, project: &str) -> mason_protocol::Placement {
    let own = project_mode(d, project);
    let (m, s) = project_effective(d, project);
    let settable = has_remote(d, project);
    mason_protocol::Placement {
        mode: m.name().into(),
        source: if settable {
            s.to_string()
        } else {
            "project".into()
        },
        own: own.is_some(),
        settable,
    }
}

/// The global placement, for the home view.
pub fn global_placement(d: &Daemon) -> mason_protocol::Placement {
    mason_protocol::Placement {
        mode: read_global(d).0.name().into(),
        source: "global".into(),
        own: true,
        settable: true,
    }
}

fn has_remote(d: &Daemon, project: &str) -> bool {
    d.project_id_by_name(project)
        .and_then(|p| d.project(p))
        .is_some_and(|p| p.services.remote.is_some())
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
    let mut f = Facts {
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
        spill: None,
        remote_spill: None,
    };
    match f.effective().0 {
        Mode::FavorLocal => f.spill = spill_for(d, project, suite),
        Mode::FavorRemote => f.remote_spill = remote_spill_for(d, project, suite),
        _ => {}
    }
    f
}

/// The load and GitHub's room for a suite of `project`, read from the scheduler (#832).
fn spill_for(d: &Daemon, project: &str, suite: &mason_testing::Suite) -> Option<Spill> {
    let cfg = d
        .project_id_by_name(project)
        .and_then(|p| d.project(p))
        .and_then(|p| p.services.remote.clone())?;
    let needs = local_needs(&suite.uses);
    let runs_on = suite.runs_on.clone().unwrap_or(cfg.runs_on);
    let room = |name: &str| d.broker.limit(name).is_none_or(|l| d.broker.used(name) < l);
    Some(Spill {
        load: LocalLoad::from_reading(&d.broker.load_reading(now_ms()), &needs),
        github_room: room(mason_scheduler::GHA)
            && (!runs_on.contains("macos") || room(mason_scheduler::GHA_MACOS)),
        spill_after_ms: cfg.spill_after_s.saturating_mul(1000),
    })
}

/// GitHub's load and this machine's, for a `favor-remote` suite of `project` (#833).
fn remote_spill_for(
    d: &Daemon,
    project: &str,
    suite: &mason_testing::Suite,
) -> Option<RemoteSpill> {
    let cfg = d
        .project_id_by_name(project)
        .and_then(|p| d.project(p))
        .and_then(|p| p.services.remote.clone())?;
    let runs_on = suite.runs_on.clone().unwrap_or(cfg.runs_on);
    let mut pools = vec![mason_scheduler::GHA];
    if runs_on.contains("macos") {
        pools.push(mason_scheduler::GHA_MACOS);
    }
    let reading = d.broker.load_reading(now_ms());
    let pool_free = pools
        .iter()
        .map(|n| match d.broker.limit(n) {
            Some(l) => l.saturating_sub(d.broker.used(n)),
            None => u32::MAX,
        })
        .min()
        .unwrap_or(u32::MAX);
    let pool_waiting = reading
        .waiting
        .iter()
        .filter(|(names, _)| names.iter().any(|n| pools.contains(&n.as_str())))
        .count() as u32;
    Some(RemoteSpill {
        local: LocalLoad::from_reading(&reading, &local_needs(&suite.uses)),
        remote: RemoteLoad {
            pool_free,
            pool_waiting,
            account_queued: None,
            recent_pickup_s: crate::remote::recent_pickup_s(),
        },
        queue_max: cfg.remote_queue_max,
        spill_after_s: cfg.spill_after_s,
    })
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

/// How long a `favor-local` placement stands, so the several calls one run makes (is it remote?
/// then the run itself) agree, and a run queued here is only re-placed by [`keep_or_move`].
const STICKY_MS: u64 = 600_000;

type StickyKey = (String, u64, String, &'static str);
static STICKY: Mutex<BTreeMap<StickyKey, (Place, u64)>> = Mutex::new(BTreeMap::new());

/// Forget the standing placement of a run that is over, so the next one is decided afresh.
pub fn forget(project: &str, task: Option<u64>, suite: &str, kind: &'static str) {
    STICKY.lock().unwrap_or_else(|e| e.into_inner()).remove(&(
        project.to_string(),
        task.unwrap_or(0),
        suite.to_string(),
        kind,
    ));
}

/// A `favor-local` run placed at `placed_at` stays where it was, except that one placed here
/// and waiting longer than `spill_after` while a hold is active moves to GitHub when GitHub has
/// room (#832). `now` is given, so a test fixes the clock. `None`: decide afresh.
pub fn keep_or_move(prev: &(Place, u64), now: u64, f: &Facts) -> Option<Place> {
    let (place, at) = prev;
    if now.saturating_sub(*at) > STICKY_MS {
        return None;
    }
    let Place::Local { from, .. } = place else {
        return Some(place.clone());
    };
    let (Some(sp), Some(runs_on)) = (
        &f.spill,
        f.suite_runs_on.as_ref().or(f.project_runs_on.as_ref()),
    ) else {
        return Some(place.clone());
    };
    match replace_waiting(now.saturating_sub(*at), sp) {
        Some(why) => Some(Place::GitHub {
            runs_on: runs_on.clone(),
            from: *from,
            why: format!("favor-local, {why}"),
        }),
        None => Some(place.clone()),
    }
}

/// Decide for a suite of a project and remember it.
pub fn place_of(
    d: &Daemon,
    project: &str,
    task: Option<u64>,
    suite: &mason_testing::Suite,
    kind: &'static str,
) -> Place {
    let f = facts_for(d, project, task, suite);
    let key = (
        project.to_string(),
        task.unwrap_or(0),
        suite.name.clone(),
        kind,
    );
    let balanced = f.spill.is_some() || f.remote_spill.is_some();
    let now = now_ms();
    if balanced {
        let prev = STICKY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned();
        if let Some(p) = prev.as_ref().and_then(|pv| keep_or_move(pv, now, &f)) {
            if prev.as_ref().is_some_and(|pv| pv.0 != p) {
                STICKY
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, (p.clone(), now));
            }
            note(project, task, &suite.name, kind, &p);
            return p;
        }
    }
    let p = decide(&f);
    if balanced {
        STICKY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (p.clone(), now));
    }
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
    fn load(held: Option<&str>, free: u32, waiting: u32, oldest_s: u64) -> LocalLoad {
        LocalLoad {
            held: held.map(String::from),
            free: BTreeMap::from([("compile".to_string(), free)]),
            waiting,
            oldest_wait_ms: oldest_s * 1000,
            exclusive_waiting: None,
        }
    }

    fn favor_local(load: LocalLoad, github_room: bool) -> Facts {
        Facts {
            task: Some(Mode::FavorLocal),
            spill: Some(Spill {
                load,
                github_room,
                spill_after_ms: 120_000,
            }),
            ..Facts::plain(Mode::Local)
        }
    }

    fn favor_remote(local: LocalLoad, remote: RemoteLoad) -> Facts {
        Facts {
            task: Some(Mode::FavorRemote),
            remote_spill: Some(RemoteSpill {
                local,
                remote,
                queue_max: 4,
                spill_after_s: 120,
            }),
            ..Facts::plain(Mode::Local)
        }
    }

    fn remote_load(
        free: u32,
        waiting: u32,
        queued: Option<u32>,
        pickup: Option<u64>,
    ) -> RemoteLoad {
        RemoteLoad {
            pool_free: free,
            pool_waiting: waiting,
            account_queued: queued,
            recent_pickup_s: pickup,
        }
    }

    /// #833: the pure favor-remote table.
    #[test]
    fn favor_remote_spills_only_when_github_is_overloaded_and_this_machine_has_room() {
        let heat = Some("held for heat (die 99 °C)");
        let rows: Vec<(&str, LocalLoad, RemoteLoad, bool)> = vec![
            (
                "GitHub room",
                load(None, 2, 0, 0),
                remote_load(3, 0, Some(1), Some(10)),
                false,
            ),
            (
                "pool full, waiters, local room",
                load(None, 2, 0, 0),
                remote_load(0, 2, None, None),
                true,
            ),
            (
                "pool full, no waiters",
                load(None, 2, 0, 0),
                remote_load(0, 0, None, None),
                false,
            ),
            (
                "pool full, local hold",
                load(heat, 2, 0, 0),
                remote_load(0, 2, None, None),
                false,
            ),
            (
                "pool full, local slot taken",
                load(None, 0, 0, 0),
                remote_load(0, 2, None, None),
                false,
            ),
            (
                "account queue over max, local room",
                load(None, 1, 0, 0),
                remote_load(2, 0, Some(5), None),
                true,
            ),
            (
                "account queue at max",
                load(None, 1, 0, 0),
                remote_load(2, 0, Some(4), None),
                false,
            ),
            (
                "slow pickups, local room",
                load(None, 1, 0, 0),
                remote_load(2, 0, None, Some(300)),
                true,
            ),
            (
                "slow pickups, local hold",
                load(heat, 1, 0, 0),
                remote_load(2, 0, None, Some(300)),
                false,
            ),
        ];
        for (name, l, r, local) in rows {
            let got = decide(&favor_remote(l, r));
            assert_eq!(!got.is_github(), local, "{name}: {}", got.text());
        }
    }

    #[test]
    fn a_favor_remote_placement_says_why() {
        let t = decide(&favor_remote(
            load(None, 1, 0, 0),
            remote_load(0, 19, None, None),
        ))
        .text();
        assert!(
            t.contains(
                "favor-remote, the runner pool is full with 19 waiting; this machine had room"
            ),
            "{t}"
        );
        let t = decide(&favor_remote(
            load(None, 1, 0, 0),
            remote_load(2, 0, Some(19), None),
        ))
        .text();
        assert!(t.contains("GitHub had 19 queued"), "{t}");
        let t = decide(&favor_remote(
            load(None, 1, 0, 0),
            remote_load(2, 0, None, None),
        ))
        .text();
        assert!(t.contains("favor-remote, GitHub had room"), "{t}");
    }

    #[test]
    fn pickup_median_of_the_last_runs() {
        assert_eq!(crate::remote::median_pickup(&[]), None);
        assert_eq!(crate::remote::median_pickup(&[500, 10, 30]), Some(30));
    }

    /// #832: the pure favor-local table.
    #[test]
    fn favor_local_spills_only_when_overloaded_and_github_has_room() {
        let heat = Some("held for heat (die 99 °C)");
        let rows: Vec<(&str, LocalLoad, bool, bool)> = vec![
            ("hold, GitHub room", load(heat, 2, 0, 0), true, true),
            ("hold, GitHub full", load(heat, 2, 0, 0), false, false),
            ("slots free", load(None, 2, 0, 0), true, false),
            ("slots full, waiters, room", load(None, 0, 3, 5), true, true),
            (
                "slots full, waiters, GitHub full",
                load(None, 0, 3, 5),
                false,
                false,
            ),
            (
                "slots full, nobody waiting",
                load(None, 0, 0, 0),
                true,
                false,
            ),
            (
                "oldest wait over spill_after",
                load(None, 1, 1, 121),
                true,
                true,
            ),
            (
                "oldest wait under spill_after",
                load(None, 1, 1, 119),
                true,
                false,
            ),
        ];
        for (name, l, room, remote) in rows {
            let got = decide(&favor_local(l, room));
            assert_eq!(got.is_github(), remote, "{name}: {got:?}");
        }
        // A project without [remote_runs] stays here, however loaded.
        let f = Facts {
            project_runs_on: None,
            ..favor_local(load(heat, 0, 3, 500), true)
        };
        assert!(!decide(&f).is_github());
    }

    #[test]
    fn a_favor_local_placement_says_why() {
        let t = decide(&favor_local(
            load(Some("held for heat (die 99 °C)"), 2, 0, 0),
            true,
        ))
        .text();
        assert!(
            t.contains("favor-local, this machine held for heat (die 99 °C)"),
            "{t}"
        );
        let t = decide(&favor_local(load(None, 2, 0, 0), true)).text();
        assert!(t.contains("favor-local, 2 compile slots free"), "{t}");
        let t = decide(&favor_local(load(None, 0, 2, 0), false)).text();
        assert!(t.contains("GitHub is full too"), "{t}");
    }

    /// #832: a run queued here moves once its wait passes `spill_after` while a hold is active
    /// and GitHub has room; the clock is a fixed number.
    #[test]
    fn a_queued_local_run_moves_when_its_wait_passes_spill_after_under_a_hold() {
        let here = Place::Local {
            why: "favor-local, 1 compile slot free".into(),
            from: Source::Task,
        };
        let placed_at = 1_000_000;
        let prev = (here.clone(), placed_at);
        let heat = Some("held for heat (die 99 °C)");
        let f = |l, room| favor_local(l, room);
        // Under spill_after: stays.
        assert_eq!(
            keep_or_move(&prev, placed_at + 100_000, &f(load(heat, 0, 1, 0), true)),
            Some(here.clone())
        );
        // Over it, held, GitHub room: moves.
        let moved =
            keep_or_move(&prev, placed_at + 130_000, &f(load(heat, 0, 1, 0), true)).unwrap();
        assert!(moved.is_github(), "{moved:?}");
        // Over it but no hold, or GitHub full: stays.
        assert_eq!(
            keep_or_move(&prev, placed_at + 130_000, &f(load(None, 0, 1, 0), true)),
            Some(here.clone())
        );
        assert_eq!(
            keep_or_move(&prev, placed_at + 130_000, &f(load(heat, 0, 1, 0), false)),
            Some(here)
        );
        // A placement on GitHub stands; an old one is decided afresh.
        let gh = (moved.clone(), placed_at);
        assert_eq!(
            keep_or_move(&gh, placed_at + 1, &f(load(None, 2, 0, 0), true)),
            Some(moved)
        );
        assert_eq!(
            keep_or_move(
                &gh,
                placed_at + STICKY_MS + 1,
                &f(load(None, 2, 0, 0), true)
            ),
            None
        );
    }

    #[test]
    fn a_load_is_built_from_a_scheduler_reading() {
        let r = crate::sched::LoadReading {
            used: BTreeMap::from([("compile".to_string(), 2)]),
            limits: BTreeMap::from([("compile".to_string(), 2), ("test-suite".to_string(), 4)]),
            waiting: vec![
                (vec!["compile".to_string()], 7_000),
                (vec!["test-suite".to_string()], 99_000),
            ],
            held: None,
            ..Default::default()
        };
        let l = LocalLoad::from_reading(&r, &["compile".to_string()]);
        assert_eq!(
            (l.free["compile"], l.waiting, l.oldest_wait_ms),
            (0, 1, 7_000)
        );
    }

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

    /// A run that would hold slots here has room only with no hold active and a free unit of each
    /// slot; a covered run (its caller holds the slots) is not counted against its own use.
    #[test]
    fn room_needs_no_hold_and_a_free_unit_of_each_slot() {
        use crate::sched::LoadReading;
        let uses = BTreeMap::from([("compile".to_string(), 1u32), ("test-suite".to_string(), 1)]);
        let reading = |held: Option<&str>, compile: u32, suite: u32| LoadReading {
            used: BTreeMap::from([
                ("compile".to_string(), compile),
                ("test-suite".to_string(), suite),
            ]),
            limits: BTreeMap::from([("compile".to_string(), 2), ("test-suite".to_string(), 1)]),
            waiting: vec![],
            held: held.map(String::from),
            ..Default::default()
        };
        // Free and no hold: room.
        assert_eq!(no_room_for(&reading(None, 1, 0), &uses, false), None);
        // A heat hold: no room, and the reason names it.
        let why = no_room_for(
            &reading(Some("held for heat (die 94 C)"), 0, 0),
            &uses,
            false,
        )
        .expect("no room");
        assert!(why.contains("held for heat"), "{why}");
        // One of the slots full (the other free): no room, naming it.
        let why = no_room_for(&reading(None, 0, 1), &uses, false).expect("no room");
        assert!(why.contains("test-suite"), "{why}");
        // A slot the run does not need being full does not matter.
        let only = BTreeMap::from([("compile".to_string(), 1u32)]);
        assert_eq!(no_room_for(&reading(None, 1, 1), &only, false), None);
        // Nothing to hold: only a hold can say no.
        let none = BTreeMap::new();
        assert_eq!(no_room_for(&reading(None, 2, 1), &none, false), None);
        assert!(no_room_for(&reading(Some("held for CPU"), 0, 0), &none, false).is_some());
        // Covered: the batch's own full slot is not counted, a hold still is.
        assert_eq!(no_room_for(&reading(None, 1, 1), &uses, true), None);
        assert!(no_room_for(&reading(Some("held for heat"), 1, 1), &uses, true).is_some());
    }

    /// #1025: room is a grant now, so requests already waiting for the slots count against the
    /// free units: compile 1/6 used (5 free) with 5 waiting is no room; with 4 waiting it is.
    #[test]
    fn room_counts_the_queue_already_waiting() {
        use crate::sched::LoadReading;
        let uses = BTreeMap::from([("compile".to_string(), 1u32)]);
        let reading = |waiting: usize| LoadReading {
            used: BTreeMap::from([("compile".to_string(), 1)]),
            limits: BTreeMap::from([("compile".to_string(), 6)]),
            waiting: vec![(vec!["compile".to_string()], 1000); waiting],
            held: None,
            ..Default::default()
        };
        let why = no_room_for(&reading(5), &uses, false).expect("no room behind 5 waiting");
        assert!(why.contains("compile"), "{why}");
        assert_eq!(no_room_for(&reading(4), &uses, false), None);
        // A batch's covered run is not counted against the queue.
        assert_eq!(no_room_for(&reading(5), &uses, true), None);
    }

    /// 2026-11-23: an exclusive request waiting at the head of the queue blocks every other
    /// local request until the machine drains, so there is no room here (and favor-local sees
    /// the machine as overloaded) whatever the free units say; a covered run is not affected.
    #[test]
    fn a_waiting_exclusive_request_is_no_room() {
        use crate::sched::LoadReading;
        let uses = BTreeMap::from([("compile".to_string(), 1u32)]);
        let reading = |x: Option<&str>| LoadReading {
            used: BTreeMap::from([("compile".to_string(), 0)]),
            limits: BTreeMap::from([("compile".to_string(), 6)]),
            exclusive_waiting: x.map(String::from),
            ..Default::default()
        };
        assert_eq!(no_room_for(&reading(None), &uses, false), None);
        let why = no_room_for(&reading(Some("trd test")), &uses, false).expect("no room");
        assert!(
            why.contains("exclusive") && why.contains("trd test"),
            "{why}"
        );
        assert_eq!(no_room_for(&reading(Some("trd test")), &uses, true), None);
        let load = LocalLoad::from_reading(&reading(Some("trd test")), &["compile".to_string()]);
        assert!(!load.has_room());
        assert!(load.overloaded(1_000).is_some());
        assert!(
            LocalLoad::from_reading(&reading(None), &["compile".to_string()])
                .overloaded(1_000)
                .is_none()
        );
    }
}
