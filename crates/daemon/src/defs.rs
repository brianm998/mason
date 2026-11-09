// SPDX-License-Identifier: GPL-3.0-or-later

//! Definitions in the daemon: loading them from the base branch, the local
//! overlay layered on top, pinning tasks to the version they started under, and
//! applying edits live. An invalid edit is refused (and reported) while the
//! current definition keeps running.

use crate::config::ProjectCfg;
use crate::daemon::{now_ms, Daemon};
use mason_core::ProjectId;
use mason_definition::builtin::BUILTIN;
use mason_definition::layers::{parse_overlay, Overlay};
use mason_definition::{parse_with, validate, Definition, Plugins};
use mason_landing::git;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    /// Read from `.mason/mason.toml` at this commit of the base branch.
    Project {
        commit: String,
    },
}

impl Origin {
    pub fn label(&self) -> String {
        match self {
            Origin::Builtin => "built-in".into(),
            Origin::Project { commit } => {
                format!("base branch at {}", &commit[..commit.len().min(10)])
            }
        }
    }
}

#[derive(Clone)]
pub struct Current {
    pub def: Arc<Definition>,
    pub origin: Origin,
}

pub struct ProjectDefs {
    pub current: Current,
    pub overlay: Overlay,
    /// What the last look saw (base commit, overlay hash), so the watcher only
    /// reloads when something changed.
    watched: (Option<String>, String),
    /// Why the latest edit was refused, until a good one replaces it.
    pub error: Option<Vec<String>>,
}

#[derive(Default)]
pub struct Defs {
    by_project: Mutex<HashMap<ProjectId, ProjectDefs>>,
    cache: Mutex<HashMap<String, Arc<Definition>>>,
    files: Mutex<HashMap<ProjectId, (String, Arc<FileIndex>)>>,
    /// Extensions on each project's base commit.
    pub(crate) exts: Mutex<HashMap<String, (String, Arc<crate::plugins::Scan>)>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reload {
    Unchanged,
    Applied {
        hash: String,
        origin: String,
    },
    /// Something was invalid. What was valid was still applied.
    Refused(Vec<String>),
}

/// The files on the base branch, for the clarity check.
pub struct FileIndex {
    paths: HashSet<String>,
    basenames: HashSet<String>,
}

impl FileIndex {
    pub fn exists(&self, p: &str) -> bool {
        self.paths.contains(p) || (!p.contains('/') && self.basenames.contains(p))
    }
}

fn builtin() -> Arc<Definition> {
    Arc::new(mason_definition::builtin::builtin())
}

fn describe(what: &str, problems: &[mason_definition::Problem]) -> Vec<String> {
    problems.iter().map(|p| format!("{what}: {p}")).collect()
}

impl Daemon {
    /// Read the definition from the project's base branch: `.mason/mason.toml`
    /// there, or the built-in one if the project has none. Errors say what is wrong.
    fn read_definition(&self, p: &ProjectCfg) -> Result<(Current, Option<String>), Vec<String>> {
        let base_ref = p.base_ref();
        let sha = git::rev_parse(&p.path, &base_ref).ok();
        let path = format!("{base_ref}:.mason/mason.toml");
        let (text, origin) = match &sha {
            Some(sha) if git::ok(&p.path, &["cat-file", "-e", &path]) => {
                match git::run(&p.path, &["show", &path]) {
                    Ok(t) => (
                        t,
                        Origin::Project {
                            commit: sha.clone(),
                        },
                    ),
                    Err(e) => return Err(vec![format!("cannot read {path}: {e}")]),
                }
            }
            _ => (BUILTIN.to_string(), Origin::Builtin),
        };
        let what = format!(".mason/mason.toml on {base_ref}");
        let caps = self.caps_for(p);
        let def = parse_with(&text, Plugins::Named(&caps.plugin_names()))
            .map_err(|e| describe(&what, &e))?;
        let problems = validate(&def, &caps);
        if !problems.is_empty() {
            return Err(describe(&what, &problems));
        }
        for w in mason_definition::validate::warnings(&def, &caps) {
            tracing::warn!(project = %p.id, "{what}: {w}");
        }
        self.store
            .put_definition(&def.hash, &text, now_ms())
            .map_err(|e| vec![e.to_string()])?;
        Ok((
            Current {
                def: Arc::new(def),
                origin,
            },
            sha,
        ))
    }

    /// The overlay (or what is wrong with it) and the hash of the text read, so
    /// the watcher can tell a change from no change even when the text is bad.
    fn read_overlay(&self, p: &ProjectCfg) -> (Result<Overlay, Vec<String>>, String) {
        let text = std::fs::read_to_string(
            self.cfg
                .config_dir
                .join("projects")
                .join(format!("{}.toml", p.id)),
        )
        .unwrap_or_else(|_| p.overlay_text.clone());
        let hash = mason_definition::model::hash(&text);
        let mut o = match parse_overlay(&text) {
            Ok(o) => o,
            Err(e) => {
                return (
                    Err(describe(&format!("local overlay {}.toml", p.id), &e)),
                    hash,
                )
            }
        };
        // The older `max_test_rounds` key is a layer-3 setting on the test step.
        if let Some(n) = crate::config::parse_project(&p.id, &text)
            .ok()
            .and_then(|c| c.max_test_rounds)
        {
            o.steps
                .entry("test".into())
                .or_default()
                .entry("max_rounds".into())
                .or_insert_with(|| n.to_string());
        }
        (Ok(o), hash)
    }

    /// Load every project's definition at startup. A project whose definition
    /// is invalid starts on the last good one (or the built-in) and says why.
    pub(crate) fn load_definitions(&self) {
        for (pid, p) in &self.projects {
            if let Some(r) = &p.remote {
                let _ = git::run(&p.path, &["fetch", "--prune", r]);
            }
            let fallback = self
                .store
                .read(|c| {
                    c.query_row(
                        "SELECT value FROM meta WHERE key = ?1",
                        [format!("def:{}", p.id)],
                        |r| r.get::<_, String>(0),
                    )
                })
                .ok()
                .and_then(|h| self.pinned_def(&h))
                .unwrap_or_else(builtin);
            self.defs
                .by_project
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    *pid,
                    ProjectDefs {
                        current: Current {
                            def: fallback,
                            origin: Origin::Builtin,
                        },
                        overlay: Overlay::default(),
                        watched: (None, String::new()),
                        error: None,
                    },
                );
            let r = self.reload_definition(*pid);
            if let Reload::Refused(e) = r {
                tracing::warn!(project = %p.id, problems = ?e, "the definition was refused at startup; using the last good one");
            }
        }
    }

    /// Look again at a project's definition and overlay. A valid new one is
    /// applied for tasks admitted from now on (tasks already started stay
    /// pinned); an invalid one is refused and reported while the current keeps
    /// running.
    pub fn reload_definition(&self, pid: ProjectId) -> Reload {
        if let Some(p) = self.projects.get(&pid) {
            // Someone may have landed a change: look at the remote's base, not our last view of it.
            self.fetch_project(p);
        }
        self.reload_without_fetch(pid)
    }

    fn fetch_project(&self, p: &ProjectCfg) {
        if let Some(r) = &p.remote {
            let lock = self.repo_lock(self.project_id_by_name(&p.id).unwrap_or(ProjectId(0)));
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            let _ = git::run(&p.path, &["fetch", "--prune", r]);
        }
    }

    fn reload_without_fetch(&self, pid: ProjectId) -> Reload {
        let Some(p) = self.projects.get(&pid) else {
            return Reload::Unchanged;
        };
        let cand = self.read_definition(p);
        let ov = self.read_overlay(p);
        let mut errors: Vec<String> = vec![];
        let mut g = self
            .defs
            .by_project
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(e) = g.get_mut(&pid) else {
            return Reload::Unchanged;
        };
        let mut applied = None;
        let mut sha = e.watched.0.clone();
        match cand {
            Ok((cur, base_sha)) => {
                sha = base_sha;
                if cur.def.hash != e.current.def.hash {
                    applied = Some((cur.def.hash.clone(), cur.origin.label()));
                    let h = cur.def.hash.clone();
                    let name = p.id.clone();
                    let _ = self.store.write(move |c| {
                        c.execute(
                            "INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)",
                            [format!("def:{name}"), h],
                        )
                        .map(|_| ())
                    });
                }
                e.current = cur;
            }
            Err(es) => {
                sha = git::rev_parse(&p.path, &p.base_ref()).ok().or(sha);
                errors.extend(es);
            }
        }
        let (ov, overlay_hash) = ov;
        match ov {
            Ok(o) => {
                if o != e.overlay || overlay_hash != e.watched.1 {
                    applied =
                        applied.or_else(|| Some((e.current.def.hash.clone(), "overlay".into())));
                }
                e.overlay = o;
            }
            Err(es) => errors.extend(es),
        }
        e.watched = (sha, overlay_hash);
        if errors.is_empty() {
            e.error = None;
            match applied {
                Some((hash, origin)) => Reload::Applied { hash, origin },
                None => Reload::Unchanged,
            }
        } else {
            e.error = Some(errors.clone());
            Reload::Refused(errors)
        }
    }

    /// One look for changes: has the base branch moved, or the overlay file changed?
    pub(crate) fn watch_definitions(&self, fetch: bool) {
        for (pid, p) in &self.projects {
            if fetch {
                self.fetch_project(p);
            }
            let sha = git::rev_parse(&p.path, &p.base_ref()).ok();
            let text = std::fs::read_to_string(
                self.cfg
                    .config_dir
                    .join("projects")
                    .join(format!("{}.toml", p.id)),
            )
            .unwrap_or_default();
            let seen = (sha, mason_definition::model::hash(&text));
            let changed = self
                .defs
                .by_project
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(pid)
                .is_some_and(|e| e.watched != seen);
            if changed {
                if let Reload::Applied { hash, origin } = self.reload_without_fetch(*pid) {
                    tracing::info!(project = %p.id, %hash, %origin, "a new definition is in force for tasks admitted from now on");
                }
            }
        }
    }

    pub(crate) fn spawn_watcher(self: &Arc<Self>) {
        let every = self.cfg.reload_interval;
        let fetch_every = self.cfg.fetch_interval;
        let mut last_fetch = std::time::Instant::now();
        crate::bg::spawn_loop(self, "definition-watcher", move |weak| {
            std::thread::sleep(every);
            let Some(d) = weak.upgrade() else {
                return false;
            };
            let fetch = last_fetch.elapsed() >= fetch_every;
            if fetch {
                last_fetch = std::time::Instant::now();
            }
            d.watch_definitions(fetch);
            // A pause that has run out lets claims go; alerts follow conditions.
            d.admit();
            crate::attention::sync_alerts(&d);
            crate::notify::tick(&d);
            true
        });
    }

    pub fn current_def(&self, pid: ProjectId) -> Arc<Definition> {
        self.defs
            .by_project
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
            .map_or_else(builtin, |e| e.current.def.clone())
    }

    pub fn current_origin(&self, pid: ProjectId) -> Origin {
        self.defs
            .by_project
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
            .map_or(Origin::Builtin, |e| e.current.origin.clone())
    }

    pub fn overlay(&self, pid: ProjectId) -> Overlay {
        self.defs
            .by_project
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
            .map(|e| e.overlay.clone())
            .unwrap_or_default()
    }

    /// Why the latest edit to this project's configuration was refused, if it was.
    pub fn definition_error(&self, pid: ProjectId) -> Option<Vec<String>> {
        self.defs
            .by_project
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
            .and_then(|e| e.error.clone())
    }

    /// The definition a task is pinned to. An empty hash is a task from before
    /// definitions existed: it ran the built-in graph.
    pub fn pinned_def(&self, hash: &str) -> Option<Arc<Definition>> {
        if hash.is_empty() {
            return Some(builtin());
        }
        if let Some(d) = self
            .defs
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(hash)
        {
            return Some(d.clone());
        }
        let text = self.store.definition(hash).ok().flatten()?;
        let def = Arc::new(parse_with(&text, Plugins::Any).ok()?);
        self.defs
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(hash.to_string(), def.clone());
        Some(def)
    }

    /// Files on the project's base branch (cached per commit).
    pub fn file_index(&self, pid: ProjectId) -> Option<Arc<FileIndex>> {
        let p = self.projects.get(&pid)?;
        let base_ref = p.base_ref();
        let sha = git::rev_parse(&p.path, &base_ref).ok()?;
        if let Some((s, idx)) = self
            .defs
            .files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&pid)
        {
            if *s == sha {
                return Some(idx.clone());
            }
        }
        let out = git::run(&p.path, &["ls-tree", "-r", "--name-only", &sha]).ok()?;
        let paths: HashSet<String> = out.lines().map(String::from).collect();
        let basenames = paths
            .iter()
            .filter_map(|f| f.rsplit('/').next())
            .map(String::from)
            .collect();
        let idx = Arc::new(FileIndex { paths, basenames });
        self.defs
            .files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pid, (sha, idx.clone()));
        Some(idx)
    }
}
