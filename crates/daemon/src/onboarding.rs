// SPDX-License-Identifier: GPL-3.0-or-later

//! Add Project over HTTP: the questions of `mason init` asked by a monitor, answered on the
//! daemon's machine (the repository is there, not on the monitor's). Three owner-only calls over
//! the `mason-onboard` crate:
//!
//! - `GET /onboarding/candidates`: git work trees one level under each `[onboarding] roots`.
//! - `POST /onboarding/plan {path, answers}`: the next question and the files that would be
//!   written. Writes nothing.
//! - `POST /onboarding/apply {path, answers}`: write, register, file the proof task.
//!
//! `answers` is the flag set of `mason init` by its field names (`tracker`, `agent`, `land`,
//! `no_prove`, ...). Landing (`land` = no, merge, push or pr) happens only as the owner answers;
//! the default is no (decision 0290).

use crate::daemon::Daemon;
use mason_onboard::questions::{project_id, Flags};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Why a call is refused: an HTTP status and the reason.
pub type Refusal = (u16, String);

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The id of a registered project whose directory is `path` or contains it.
pub fn registered_around(d: &Daemon, path: &Path) -> Option<String> {
    let path = canon(path);
    d.projects()
        .values()
        .find(|p| path.starts_with(canon(&p.path)))
        .map(|p| p.id.clone())
}

/// Git work trees one level under each root, not registered and not inside a registered project.
pub fn candidates(d: &Daemon) -> Value {
    let registered: Vec<PathBuf> = d.projects().values().map(|p| canon(&p.path)).collect();
    let mut found: Vec<(String, Value)> = vec![];
    let mut errors = vec![];
    for root in &d.cfg.onboarding.roots {
        let rd = match std::fs::read_dir(root) {
            Ok(rd) => rd,
            Err(e) => {
                errors.push(json!({"path": root.display().to_string(), "error": e.to_string()}));
                continue;
            }
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() || !p.join(".git").exists() {
                continue;
            }
            let c = canon(&p);
            if registered.iter().any(|r| c.starts_with(r)) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let remote = mason_landing::git::run(&p, &["remote", "get-url", "origin"])
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            found.push((
                name.clone(),
                json!({
                    "path": p.display().to_string(),
                    "name": name,
                    "has_mason": p.join(".mason").is_dir(),
                    "remote": remote,
                }),
            ));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    json!({
        "candidates": found.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "errors": errors,
    })
}

fn parse(body: &Value) -> Result<(PathBuf, Flags), Refusal> {
    let path = body["path"]
        .as_str()
        .filter(|p| !p.trim().is_empty())
        .ok_or((400, "path is required".to_string()))?;
    let flags: Flags = match body.get("answers") {
        None | Some(Value::Null) => Flags::default(),
        Some(a) => serde_json::from_value(a.clone()).map_err(|e| (400, format!("answers: {e}")))?,
    };
    Ok((PathBuf::from(path), flags))
}

fn refuse_registered(d: &Daemon, path: &Path) -> Result<(), Refusal> {
    match registered_around(d, path) {
        Some(id) => Err((
            409,
            format!(
                "{} is already part of the registered project {id:?}",
                path.display()
            ),
        )),
        None => Ok(()),
    }
}

/// `POST /onboarding/plan`. Writes nothing.
pub fn plan(d: &Daemon, body: &Value) -> Result<Value, Refusal> {
    let (path, flags) = parse(body)?;
    refuse_registered(d, &path)?;
    let p = mason_onboard::plan(&path, &flags);
    Ok(json!({
        "detected": p.detected,
        "kind": p.kind,
        "next": p.next,
        "files": p.files.iter().map(|(n, t)| json!({"path": n, "contents": t})).collect::<Vec<_>>(),
        "errors": p.errors,
    }))
}

/// What `apply` did up to registration; the proof task is filed by the async caller.
pub struct Applied {
    pub id: String,
    pub registered: bool,
    pub body: Value,
    pub prove: bool,
}

/// `POST /onboarding/apply`, up to and including registration. Every refusal comes before
/// anything is written.
pub fn apply(d: &std::sync::Arc<Daemon>, body: &Value) -> Result<Applied, Refusal> {
    let (path, flags) = parse(body)?;
    refuse_registered(d, &path)?;
    let det = mason_onboard::detect::detect(&path).map_err(|e| (400, e))?;
    let id = flags.id.clone().unwrap_or_else(|| project_id(&det.id));
    if d.project_id_by_name(&id).is_some() {
        return Err((
            409,
            format!("a project named {id:?} is already registered; choose another id"),
        ));
    }
    if flags.land.as_deref() == Some("merge") {
        let dirty = mason_landing::git::run(
            &det.root,
            &["status", "--porcelain", "--untracked-files=no"],
        )
        .map_err(|e| (400, e.to_string()))?;
        if !dirty.trim().is_empty() {
            return Err((
                409,
                format!(
                    "the working tree has uncommitted changes, so it cannot be merged into:\n{}",
                    dirty.trim_end()
                ),
            ));
        }
    }
    let mut progress: Vec<String> = vec![];
    let applied = {
        let lock = d.repo_lock_at(&det.root);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        mason_onboard::apply_validated(
            &path,
            &flags,
            &d.cfg.config_dir,
            &mut |l| progress.push(l.to_string()),
            &|id, text| crate::config::parse_project(id, text).map(|_| ()),
        )
        .map_err(|e| (400, e))?
    };
    // Registration reads the definition under the repository lock, so it comes after.
    let (registered, error) = match d.register_project(&applied.id) {
        Ok((_, new)) => (new, None),
        Err(e) => (false, Some(e)),
    };
    let mut out = json!({
        "id": applied.id,
        "registered": registered,
        "landed": serde_json::to_value(&applied.landed).unwrap_or(Value::Null),
        "proof_task": null,
        "base": applied.base,
        "kind": applied.kind,
        "report": applied.report,
        "notes": applied.notes,
        "progress": progress,
    });
    if let Some(e) = error {
        out["error"] = json!(e);
    }
    Ok(Applied {
        id: applied.id,
        registered,
        body: out,
        prove: !flags.no_prove,
    })
}
