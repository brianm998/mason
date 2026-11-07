// SPDX-License-Identifier: GPL-3.0-or-later

//! Project extensions in the daemon (Part 12, level 3: process plugins).
//!
//! * **Where they come from.** Only the project's base commit: `.mason/extensions/<name>/`
//!   is read with `git`, never from a worktree or a task branch, so a change cannot
//!   rewrite the tool that checks it.
//! * **Trust.** An extension runs only if the owner approved its content hash
//!   (`mason extension approve <name>`). The approvals live in
//!   `<config dir>/trust/<project>.toml`, outside every repository. Any change to any
//!   file in the folder is a different hash and needs a new approval.
//! * **No gap between check and run.** The bytes that were hashed are written to
//!   `<config dir>/extensions/<hash>/` and that copy is what runs.

use crate::config::ProjectCfg;
use crate::daemon::{now_ms, Daemon};
use mason_definition::model::StepType;
use mason_definition::Capabilities;
use mason_landing::git;
use mason_plugins::hash::{folder_hash, Entry};
use mason_plugins::host::{self, Launch, StepRequest};
use mason_plugins::manifest::{parse_manifest, Manifest};
use mason_plugins::schema;
use mason_plugins::PluginType;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

pub const DIR: &str = ".mason/extensions";
const MAX_FILES: usize = 500;
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// One folder under `.mason/extensions/` on the base commit.
#[derive(Clone, Debug)]
pub struct Extension {
    /// The folder's name, which is also the name approvals use.
    pub dir: String,
    pub hash: String,
    pub entries: Vec<Entry>,
    /// The manifest, or why this extension is refused.
    pub manifest: Result<Manifest, Vec<String>>,
    pub schema: Value,
}

impl Extension {
    pub fn refused(&self) -> Option<String> {
        self.manifest.as_ref().err().map(|e| e.join("; "))
    }

    /// The step types it provides, as the definition and editor see them.
    pub fn types(&self) -> Vec<PluginType> {
        let Ok(m) = &self.manifest else {
            return vec![];
        };
        m.step_types
            .iter()
            .map(|t| PluginType {
                name: t.clone(),
                plugin: self.dir.clone(),
                categories: m.categories.clone(),
                config_schema: self.schema.clone(),
                about: if m.description.is_empty() {
                    format!("From the `{}` extension.", self.dir)
                } else {
                    m.description.clone()
                },
            })
            .collect()
    }
}

/// What the base commit has.
#[derive(Debug, Default)]
pub struct Scan {
    pub commit: String,
    pub extensions: Vec<Extension>,
}

impl Scan {
    /// The usable extension that provides this step type.
    pub fn providing(&self, step_type: &str) -> Option<&Extension> {
        self.extensions.iter().find(|e| {
            e.manifest
                .as_ref()
                .is_ok_and(|m| m.step_types.iter().any(|t| t == step_type))
        })
    }

    pub fn get(&self, dir: &str) -> Option<&Extension> {
        self.extensions.iter().find(|e| e.dir == dir)
    }

    pub fn types(&self) -> Vec<PluginType> {
        self.extensions.iter().flat_map(Extension::types).collect()
    }
}

fn blob(repo: &Path, object: &str) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .args(["cat-file", "blob", object])
        .current_dir(repo)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// Read every extension folder from a commit. Never touches a worktree.
pub fn scan_commit(repo: &Path, commit: &str) -> Scan {
    let mut scan = Scan {
        commit: commit.to_string(),
        extensions: vec![],
    };
    let Ok(out) = Command::new("git")
        .args([
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            commit,
            "--",
            &format!("{DIR}/"),
        ])
        .current_dir(repo)
        .output()
    else {
        return scan;
    };
    if !out.status.success() {
        return scan;
    }
    let mut folders: BTreeMap<String, Vec<(String, bool, String)>> = BTreeMap::new();
    let mut odd: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for rec in String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|r| !r.is_empty())
    {
        // "<mode> <type> <object>\t<path>"
        let Some((meta, path)) = rec.split_once('\t') else {
            continue;
        };
        let mut f = meta.split_whitespace();
        let (mode, kind, object) = (
            f.next().unwrap_or(""),
            f.next().unwrap_or(""),
            f.next().unwrap_or(""),
        );
        let Some(rest) = path.strip_prefix(&format!("{DIR}/")) else {
            continue;
        };
        let Some((dir, file)) = rest.split_once('/') else {
            continue; // a file directly in extensions/ is not an extension
        };
        if kind != "blob" || mode == "120000" {
            odd.entry(dir.to_string()).or_default().push(format!(
                "`{file}` is a symbolic link or submodule; extensions hold plain files only"
            ));
            continue;
        }
        folders.entry(dir.to_string()).or_default().push((
            file.to_string(),
            mode == "100755",
            object.to_string(),
        ));
    }
    for dir in odd.keys() {
        folders.entry(dir.clone()).or_default();
    }
    let mut provided: BTreeMap<String, String> = BTreeMap::new();
    for (dir, files) in folders {
        let mut problems = odd.remove(&dir).unwrap_or_default();
        let mut entries = vec![];
        let mut total = 0;
        if files.len() > MAX_FILES {
            problems.push(format!(
                "{} files is more than the {MAX_FILES} allowed",
                files.len()
            ));
        } else {
            for (path, exec, object) in files {
                match blob(repo, &object) {
                    Some(bytes) => {
                        total += bytes.len();
                        entries.push(Entry { path, exec, bytes });
                    }
                    None => problems.push(format!("cannot read `{path}` from {commit}")),
                }
            }
        }
        if total > MAX_BYTES {
            problems.push(format!(
                "{total} bytes is more than the {MAX_BYTES} allowed"
            ));
        }
        let hash = folder_hash(&entries);
        let text = entries
            .iter()
            .find(|e| e.path == "manifest.toml")
            .map(|e| String::from_utf8_lossy(&e.bytes).into_owned());
        let mut schema_value = json!({"type": "object"});
        let manifest = if !problems.is_empty() {
            Err(problems)
        } else {
            match text {
                None => Err(vec!["there is no manifest.toml".to_string()]),
                Some(t) => parse_manifest(&t).and_then(|m| {
                    let mut bad = vec![];
                    if m.name != dir {
                        bad.push(format!(
                            "the manifest's name `{}` must be the folder's name `{dir}`",
                            m.name
                        ));
                    }
                    if let Some(f) = &m.config_schema {
                        match entries.iter().find(|e| &e.path == f) {
                            None => bad.push(format!("config_schema `{f}` is not in the folder")),
                            Some(e) => match serde_json::from_slice::<Value>(&e.bytes) {
                                Err(err) => bad.push(format!("{f} is not JSON: {err}")),
                                Ok(v) => {
                                    let p = schema::check_schema(&v);
                                    bad.extend(p.into_iter().map(|x| format!("{f}: {x}")));
                                    schema_value = v;
                                }
                            },
                        }
                    }
                    for t in &m.step_types {
                        if StepType::from_name(t).is_some() || t == "plugin" {
                            bad.push(format!("step type `{t}` is a built-in name"));
                        }
                        if let Some(other) = provided.get(t) {
                            bad.push(format!("step type `{t}` is already provided by `{other}`"));
                        }
                    }
                    if !m.run.is_empty()
                        && !entries
                            .iter()
                            .any(|e| m.run[0].trim_start_matches("./") == e.path)
                    {
                        // `run` may name an interpreter on the PATH (python3 check.py), so only a path is checked.
                        if m.run[0].starts_with("./") {
                            bad.push(format!(
                                "`run` names {} which is not in the folder",
                                m.run[0]
                            ));
                        }
                    }
                    if bad.is_empty() {
                        Ok(m)
                    } else {
                        Err(bad)
                    }
                }),
            }
        };
        if let Ok(m) = &manifest {
            for t in &m.step_types {
                provided.insert(t.clone(), dir.clone());
            }
        }
        scan.extensions.push(Extension {
            dir,
            hash,
            entries,
            manifest,
            schema: schema_value,
        });
    }
    scan
}

// ---- trust ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub name: String,
    pub hash: String,
    pub by: String,
    pub at_ms: u64,
}

fn trust_path(config_dir: &Path, project: &str) -> PathBuf {
    config_dir.join("trust").join(format!("{project}.toml"))
}

/// The approvals on record for a project.
pub fn approvals(config_dir: &Path, project: &str) -> Vec<Approval> {
    let Ok(text) = std::fs::read_to_string(trust_path(config_dir, project)) else {
        return vec![];
    };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return vec![];
    };
    doc.get("approved")
        .and_then(|a| a.as_array_of_tables())
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some(Approval {
                        name: t.get("name")?.as_str()?.to_string(),
                        hash: t.get("hash")?.as_str()?.to_string(),
                        by: t.get("by")?.as_str()?.to_string(),
                        at_ms: t.get("at_ms").and_then(|x| x.as_integer()).unwrap_or(0) as u64,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Is this exact content approved? (An approval of another hash does not count.)
pub fn approved(config_dir: &Path, project: &str, name: &str, hash: &str) -> Option<Approval> {
    approvals(config_dir, project)
        .into_iter()
        .find(|a| a.name == name && a.hash == hash)
}

pub fn record_approval(config_dir: &Path, project: &str, a: &Approval) -> std::io::Result<()> {
    let path = trust_path(config_dir, project);
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .parse()
        .unwrap_or_default();
    if doc.get("approved").is_none() {
        doc["approved"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    let mut t = toml_edit::Table::new();
    t["name"] = toml_edit::value(a.name.clone());
    t["hash"] = toml_edit::value(a.hash.clone());
    t["by"] = toml_edit::value(a.by.clone());
    t["at_ms"] = toml_edit::value(a.at_ms as i64);
    if let Some(arr) = doc["approved"].as_array_of_tables_mut() {
        arr.push(t);
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(tmp, path)
}

// ---- the copy that runs ----------------------------------------------------------------------------

fn disk_hash(dir: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    fn walk(base: &Path, dir: &Path, out: &mut Vec<Entry>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            let meta = std::fs::symlink_metadata(&p)?;
            if meta.is_dir() {
                walk(base, &p, out)?;
            } else {
                out.push(Entry {
                    path: p
                        .strip_prefix(base)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    exec: meta.permissions().mode() & 0o111 != 0,
                    bytes: std::fs::read(&p)?,
                });
            }
        }
        Ok(())
    }
    let mut v = vec![];
    walk(dir, dir, &mut v).ok()?;
    Some(folder_hash(&v))
}

/// Write the exact bytes that were hashed to `<config dir>/extensions/<hash>/`, unless an
/// identical copy is already there, and return that folder.
pub fn materialize(config_dir: &Path, ext: &Extension) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let root = config_dir.join("extensions");
    let dest = root.join(&ext.hash);
    if dest.exists() {
        if disk_hash(&dest).as_deref() == Some(ext.hash.as_str()) {
            return Ok(dest);
        }
        std::fs::remove_dir_all(&dest)?;
    }
    let tmp = root.join(format!("{}.tmp{}", ext.hash, std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    for e in &ext.entries {
        let p = tmp.join(&e.path);
        if e.path.split('/').any(|c| c == ".." || c.is_empty()) {
            return Err(std::io::Error::other(format!(
                "`{}` is not a path inside the folder",
                e.path
            )));
        }
        std::fs::create_dir_all(p.parent().expect("has a parent"))?;
        std::fs::write(&p, &e.bytes)?;
        std::fs::set_permissions(
            &p,
            std::fs::Permissions::from_mode(if e.exec { 0o755 } else { 0o644 }),
        )?;
    }
    std::fs::create_dir_all(&tmp)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

// ---- on the daemon ---------------------------------------------------------------------------------

impl Daemon {
    /// The extensions on the project's base commit (cached per commit).
    pub fn extensions(&self, p: &ProjectCfg) -> Arc<Scan> {
        let Ok(sha) = git::rev_parse(&p.path, &p.base_ref()) else {
            return Arc::new(Scan::default());
        };
        let mut cache = self.defs.exts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((s, scan)) = cache.get(&p.id) {
            if *s == sha {
                return scan.clone();
            }
        }
        let scan = Arc::new(scan_commit(&p.path, &sha));
        cache.insert(p.id.clone(), (sha, scan.clone()));
        scan
    }

    /// What this project's definition may use: the built-in steps plus the step types
    /// its (valid) extensions provide. Approval does not matter here, only at run time.
    pub fn caps_for(&self, p: &ProjectCfg) -> Capabilities {
        Capabilities::now().with_plugins(self.extensions(p).types())
    }
}

/// `GET .../extensions`: every extension, what it provides, and whether it may run.
pub fn list(d: &Daemon, p: &ProjectCfg) -> Value {
    let scan = d.extensions(p);
    let trusted = approvals(&d.cfg.config_dir, &p.id);
    json!({
        "project": p.id,
        "commit": scan.commit,
        "extensions": scan.extensions.iter().map(|e| {
            let ok = trusted.iter().find(|a| a.name == e.dir && a.hash == e.hash);
            let ever = trusted.iter().any(|a| a.name == e.dir);
            let status = if e.refused().is_some() { "refused" }
                else if ok.is_some() { "approved" }
                else if ever { "changed" } else { "unapproved" };
            json!({
                "name": e.dir,
                "hash": e.hash,
                "status": status,
                "refused": e.refused(),
                "provides": e.manifest.as_ref().map(|m| m.provides()).unwrap_or_default(),
                "version": e.manifest.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
                "files": e.entries.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
                "approved_by": ok.map(|a| a.by.clone()),
                "approved_at_ms": ok.map(|a| a.at_ms),
            })
        }).collect::<Vec<_>>(),
    })
}

/// `POST .../extensions/<name>/approve`. Only the owner; records who and when.
pub fn approve(
    d: &Daemon,
    p: &ProjectCfg,
    name: &str,
    who: &str,
) -> Result<Value, crate::attention::Refusal> {
    if !crate::attention::is_owner(d, who) {
        crate::attention::intervene(
            d,
            "refused_answer",
            None,
            who,
            "not the owner's identity (extension approve)",
            "",
        );
        return Err((
            403,
            format!("{who:?} is not the owner's identity; only the owner can approve an extension"),
        ));
    }
    let scan = d.extensions(p);
    let Some(e) = scan.get(name) else {
        let have: Vec<&str> = scan.extensions.iter().map(|e| e.dir.as_str()).collect();
        return Err((
            404,
            format!(
                "there is no extension `{name}` on the base branch; it has: {}",
                if have.is_empty() {
                    "none".into()
                } else {
                    have.join(", ")
                }
            ),
        ));
    };
    if let Some(why) = e.refused() {
        return Err((
            422,
            format!("extension `{name}` is refused and cannot be approved: {why}"),
        ));
    }
    let a = Approval {
        name: name.to_string(),
        hash: e.hash.clone(),
        by: who.trim().to_string(),
        at_ms: now_ms(),
    };
    record_approval(&d.cfg.config_dir, &p.id, &a)
        .map_err(|err| (500, format!("cannot record the approval: {err}")))?;
    crate::attention::intervene(
        d,
        "extension_approved",
        None,
        who,
        &format!("{name} {}", e.hash),
        "",
    );
    Ok(json!({"name": name, "hash": a.hash, "approved_by": a.by}))
}

/// The settings and task facts a plugin's step gets.
fn request(c: &crate::jobs::Ctx) -> StepRequest {
    StepRequest {
        step: c.step.name.clone(),
        step_type: c.step.plugin.clone().unwrap_or_default(),
        config: Value::Object(c.step.config.clone().into_iter().collect()),
        task: c.task.0,
        title: c.title.clone(),
        worktree: c.wt.to_string_lossy().into_owned(),
        env: crate::jobs::step_env(c),
    }
}

/// Run a plugin's step. The step fails, naming the command, unless the owner approved
/// exactly the bytes about to run.
pub fn plugin_job(c: &crate::jobs::Ctx) {
    use mason_definition::graph::Outcome;
    let ty = c.step.plugin.clone().unwrap_or_default();
    let scan = c.d.extensions(&c.project);
    let Some(ext) = scan.providing(&ty) else {
        return c.fail(format!(
            "no extension on {} provides the step type `{ty}`",
            c.project.base_ref()
        ));
    };
    let name = ext.dir.clone();
    if approved(&c.d.cfg.config_dir, &c.project.id, &name, &ext.hash).is_none() {
        let seen = approvals(&c.d.cfg.config_dir, &c.project.id)
            .iter()
            .any(|a| a.name == name);
        return c.fail(format!(
            "needs approval: mason extension approve {name} --project {} (extension `{name}` {} has not been approved{})",
            c.project.id,
            &ext.hash[..12],
            if seen { "; it changed since it was last approved" } else { "" }
        ));
    }
    let dir = match materialize(&c.d.cfg.config_dir, ext) {
        Ok(d) => d,
        Err(e) => return c.fail(format!("cannot prepare extension `{name}`: {e}")),
    };
    let Ok(m) = &ext.manifest else {
        unreachable!("providing() returns usable extensions")
    };
    if let Err(why) = crate::jobs::prepare_worktree_only(c) {
        return c.fail(why);
    }
    let settings = Value::Object(c.step.config.clone().into_iter().collect());
    let problems = schema::validate(&ext.schema, &settings);
    if !problems.is_empty() {
        return c.fail(format!(
            "the settings of `{}` do not fit the `{name}` plugin: {}",
            c.step.name,
            problems.join("; ")
        ));
    }
    let launch = Launch {
        dir,
        run: m.run.clone(),
        log: Some(
            c.task_dir()
                .join(format!("plugin-{}-r{}.log", c.step.name, c.run.0)),
        ),
    };
    let limit = c
        .res
        .budget
        .as_ref()
        .map_or(Duration::from_secs(600), |a| a.value);
    match host::run_step(&launch, &name, &request(c), Duration::from_secs(30), limit) {
        Ok(r) if r.passed => {
            let _ = std::fs::remove_file(c.task_dir().join("feedback.txt"));
            let _ = std::fs::write(
                c.task_dir()
                    .join(format!("plugin-{}-r{}.result", c.step.name, c.run.0)),
                &r.summary,
            );
            c.green();
        }
        Ok(r) => c.conclude(
            Outcome::Red,
            format!("`{}` failed: {}", c.step.name, r.summary),
            Some(format!(
                "The `{}` check ({ty}) failed: {}",
                c.step.name, r.summary
            )),
        ),
        Err(e) => c.fail(format!("plugin `{name}` failed: {e}")),
    }
}
