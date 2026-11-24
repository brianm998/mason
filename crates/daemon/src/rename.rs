// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason project rename OLD NEW`: give a registered project a new name and keep its id and
//! history. The name is the stem of `projects/<name>.toml`, is recorded by `ProjectRegistered`,
//! and builds keys and paths, so a rename moves the file, appends `ProjectRenamed`, rewrites
//! the text `project` columns and name-built keys in one store transaction, and re-keys the
//! in-memory registration. No restart is needed.

use crate::daemon::Daemon;
use mason_core::{EventKind, State};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// The landing worktrees of `name`: `l/<name>-b<N>`, and no other project's.
fn landing_worktrees(d: &Daemon, name: &str) -> Vec<PathBuf> {
    let prefix = format!("{name}-b");
    let mut out: Vec<PathBuf> = std::fs::read_dir(d.cfg.landing_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.strip_prefix(&prefix)
                .is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// Branches in the repository that carry the old name; they are left alone.
fn named_branches(repo: &Path, name: &str) -> Vec<String> {
    let pats = [
        format!("refs/heads/mason/{name}/*"),
        format!("refs/heads/mason/int/{name}/*"),
        format!("refs/heads/mason/ci/{name}-*"),
    ];
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .args(["for-each-ref", "--format=%(refname:short)"]);
    cmd.args(&pats);
    match cmd.output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        _ => vec![],
    }
}

impl Daemon {
    /// Rename `old` to `new`. Without `yes`, only say what would be done. `Err` carries the
    /// reasons it was refused; nothing has changed then.
    pub fn rename_project(
        self: &Arc<Self>,
        old: &str,
        new: &str,
        yes: bool,
    ) -> Result<Value, Vec<String>> {
        let pid = self
            .project_id_by_name(old)
            .ok_or_else(|| vec![format!("no project {old:?}")])?;
        let mut errs = vec![];
        if !crate::config::valid_id(new) {
            errs.push(format!(
                "{new:?} is not a valid project id: lowercase letters, digits, - and _"
            ));
        }
        if new == old {
            errs.push(format!("{old} already has that name"));
        }
        if self.project_id_by_name(new).is_some() {
            errs.push(format!("{new} is already a registered project"));
        }
        let projects_dir = self.cfg.config_dir.join("projects");
        let from = projects_dir.join(format!("{old}.toml"));
        let to = projects_dir.join(format!("{new}.toml"));
        if to.exists() {
            errs.push(format!("{} already exists", to.display()));
        }
        if !from.exists() {
            errs.push(format!("{} does not exist", from.display()));
        }
        let world = self.world().map_err(|e| vec![e.to_string()])?;
        if world
            .projects
            .values()
            .any(|p| p.id != pid && p.name == new)
        {
            errs.push(format!("{new} is already the name of another project"));
        }
        let open: Vec<String> = world
            .tasks
            .values()
            .filter(|t| t.project == pid && !matches!(t.state, State::Landed | State::Canceled))
            .map(|t| format!("#{} ({:?})", t.id.0, t.state))
            .collect();
        if !open.is_empty() {
            errs.push(format!(
                "{old} has tasks that are not landed or canceled: {}. Land or cancel them first",
                open.join(", ")
            ));
        }
        if self.landing().busy_projects().contains(&pid) {
            errs.push(format!(
                "{old} has a landing batch running; wait for it to finish"
            ));
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        let cfg = self
            .project(pid)
            .ok_or_else(|| vec![format!("no project {old:?}")])?;
        let trees = landing_worktrees(self, old);
        let branches = named_branches(&cfg.path, old);
        let mut actions = vec![
            format!("move {} to {}", from.display(), to.display()),
            format!("record the rename of project #{} ({old} -> {new})", pid.0),
            "rewrite the project name in the store's tables and keys".to_string(),
        ];
        for t in &trees {
            actions.push(format!("remove the landing worktree {}", t.display()));
        }
        let plan = |result: &str, rows: usize| {
            json!({
                "result": result, "project": pid.0, "old": old, "new": new,
                "actions": actions, "rows": rows,
                "branches_left": branches,
            })
        };
        if !yes {
            return Ok(plan("plan", 0));
        }

        std::fs::rename(&from, &to)
            .map_err(|e| vec![format!("moving the registration file: {e}")])?;
        let back = |why: String| {
            let _ = std::fs::rename(&to, &from);
            vec![why]
        };
        let newcfg = crate::config::load_project(&self.cfg.config_dir, new).map_err(&back)?;
        self.emit(EventKind::ProjectRenamed {
            project: pid,
            name: new.to_string(),
        })
        .map_err(|e| back(format!("recording the rename: {e}")))?;
        // The event is in the log, so the rename stands; a failure from here on is reported
        // and a restart replays the event.
        let rows = self.store.rename_project(old, new).map_err(|e| {
            vec![format!(
                "the rename is recorded but the store rewrite failed: {e}"
            )]
        })?;
        if let Some(t) = &newcfg.services.tracker {
            self.tracker.set_interval(pid, t.interval_s);
        }
        let leaked: &'static crate::config::ProjectCfg = Box::leak(Box::new(newcfg));
        self.registry
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pid, leaked);
        for t in &trees {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&cfg.path)
                .args(["worktree", "remove", "--force"])
                .arg(t)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                tracing::warn!(tree = %t.display(), "could not remove the old landing worktree");
            }
        }
        let _ = Command::new("git")
            .arg("-C")
            .arg(&cfg.path)
            .args(["worktree", "prune"])
            .status();
        let _ = std::fs::remove_dir(self.cfg.worktrees_dir().join(old));
        tracing::info!(old, new, rows, "project renamed");
        Ok(plan("renamed", rows))
    }
}
