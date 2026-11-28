// SPDX-License-Identifier: GPL-3.0-or-later

//! The coordinator's executor for a node (`docs/multi-machine-nodes-plan.md`, §3.8): the third
//! kind of `Executor`, next to `Local` and `GitHub`. It mirrors [`crate::remote::execute`]:
//! write a record, snapshot what a local run would see, get the code to the node (a git bundle
//! of what the node lacks), start the job, follow it, and bring the result back as the same
//! [`Finished`] a local step host or a GitHub run gives, so that everything after it judges the
//! run exactly as before.
//!
//! A node answering "no room" is not a failure: the next node is tried, and when none has room
//! the run is [`Fail::Environmental`], which the caller sends to the next place (GitHub, or here).

use crate::daemon::{now_ms, Daemon};
use crate::node_client::{block_on, ClientError, NodeClient, Response};
use crate::node_run::{base64_decode, valid_sha, BASE_FAILED, BASE_NOT_SENT};
use crate::remote::{Fail, Finished, Outcome, RunSpec, StopWhy};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

/// How long past a run's limit the coordinator waits for the node's own enforcement of it.
const MARGIN_MS: u64 = 120_000;

/// What the coordinator keeps of a run it sent, before any network call: a restart finds the run
/// again by it, and asks the node for the job it already started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub nid: String,
    pub name: String,
    pub job: String,
    pub sha: String,
    pub sent_ms: u64,
    /// The base the job was sent with: `<name>@<sha>`, or [`BASE_NOT_SENT`] when the
    /// coordinator had none. Empty in a record from before the field.
    pub base: String,
}

impl Record {
    fn to_json(&self) -> Value {
        json!({"nid": self.nid, "name": self.name, "job": self.job, "sha": self.sha, "sent_ms": self.sent_ms, "base": self.base})
    }
    fn from_json(v: &Value) -> Option<Record> {
        Some(Record {
            nid: v["nid"].as_str()?.to_string(),
            name: v["name"].as_str()?.to_string(),
            job: v["job"].as_str()?.to_string(),
            sha: v["sha"].as_str()?.to_string(),
            sent_ms: v["sent_ms"].as_u64().unwrap_or(0),
            base: v["base"].as_str().unwrap_or_default().to_string(),
        })
    }
}

pub fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.node"))
}

fn load(path: &Path) -> Option<Record> {
    Record::from_json(&serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?)
}

fn save(path: &Path, r: &Record) {
    let tmp = path.with_extension("node.tmp");
    if std::fs::write(&tmp, r.to_json().to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// The text a finished run carries: "ran on node studio (arm64), 3 min".
pub fn ran_on(name: &str, arch: &str, secs: u64) -> String {
    let took = if secs < 90 {
        format!("{secs} s")
    } else {
        format!("{} min", secs.div_ceil(60))
    };
    let arch = if arch.is_empty() {
        String::new()
    } else {
        format!(" ({arch})")
    };
    format!("ran on node {name}{arch}, {took}")
}

/// What a failure of a job's `origin/<base>` says, wherever it is reported. It is how such a
/// failure is told from other ones ([`is_base_failure`]).
const NOT_SET: &str = " not set on node ";

/// Is `why` a node without the job's base: node git state, which no change to the task's code
/// can fix?
pub fn is_base_failure(why: &str) -> bool {
    why.contains(NOT_SET)
}

/// What a job's record keeps of the base it is sent with.
fn base_record(base: Option<&(String, String)>) -> String {
    base.map_or_else(
        || BASE_NOT_SENT.to_string(),
        |(name, tip)| format!("{name}@{tip}"),
    )
}

/// The failure a job's base makes, from `sent` (the record's) and `state` (the job's status on
/// the node: `set ...`, `failed: ...`, `not sent`, or empty while unknown and from a node on an
/// older runtime). `None`: nothing is known to be wrong. Pure.
fn base_failure(
    node: &str,
    project: &str,
    base_ref: &str,
    sent: &str,
    state: &str,
) -> Option<String> {
    let why = if sent == BASE_NOT_SENT {
        format!("not sent: the coordinator cannot resolve {base_ref}")
    } else if state == BASE_NOT_SENT || state.starts_with(BASE_FAILED) {
        state.to_string()
    } else {
        return None;
    };
    let name = sent
        .split_once('@')
        .map(|(n, _)| format!("origin/{n}"))
        .unwrap_or_else(|| base_ref.to_string());
    Some(format!(
        "{name}{NOT_SET}{node} (base: {why}). This is a node failure, not the task's: node git \
         state is MASON's job, so do not fetch or ask for a fetch by hand; the owner runs \
         `mason node sync {node} --project {project}`"
    ))
}

/// What a finished run's text says of its base, after [`ran_on`].
fn base_note(state: &str) -> String {
    match state.split_once('@') {
        _ if state.is_empty() => "base: not reported (older node runtime)".into(),
        Some((head, sha)) if state.starts_with("set ") => {
            format!("base: {head}@{}", &sha[..sha.len().min(10)])
        }
        _ => format!("base: {state}"),
    }
}

/// A path from the node that is safe to write under a worktree: plain components only, nothing
/// absolute and no `..`.
pub fn plain_relative(p: &str) -> Option<PathBuf> {
    let path = PathBuf::from(p);
    (!p.is_empty()
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))))
    .then_some(path)
}

fn git(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let o = Command::new(mason_landing::git::program())
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if o.status.success() {
        Ok(o.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

fn git_text(cwd: &Path, args: &[&str]) -> Option<String> {
    git(cwd, args)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A ref the bundle is made from, deleted again when this goes out of scope.
struct TempRef<'a> {
    cwd: &'a Path,
    name: String,
}

impl<'a> TempRef<'a> {
    fn new(cwd: &'a Path, id: &str, sha: &str) -> Result<TempRef<'a>, String> {
        let name = format!("refs/mason/node/{id}");
        git(cwd, &["update-ref", &name, sha])?;
        Ok(TempRef { cwd, name })
    }
}

impl Drop for TempRef<'_> {
    fn drop(&mut self) {
        let _ = git(self.cwd, &["update-ref", "-d", &self.name]);
    }
}

/// The commits the snapshot builds on that a node might already have, nearest first: the
/// snapshot's parent (the worktree's `HEAD`), where it left the base branch, and the base tip.
fn base_candidates(cwd: &Path, sha: &str, base_ref: &str) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    let mut add = |s: Option<String>| {
        if let Some(s) = s.filter(|s| valid_sha(s)) {
            if !v.contains(&s) {
                v.push(s);
            }
        }
    };
    add(git_text(cwd, &["rev-parse", &format!("{sha}^")]));
    add(git_text(cwd, &["merge-base", base_ref, sha]));
    add(git_text(cwd, &["rev-parse", base_ref]));
    v
}

/// The base branch as a job's checkout should see it: `(name, tip)`, `("develop", sha)` for
/// `origin/develop`. A base that is a local branch (the project has no remote) is that name. `None`
/// when the base does not resolve, and the job then runs as it did before this was sent.
fn base_info(cwd: &Path, base_ref: &str) -> Option<(String, String)> {
    let tip = git_text(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("{base_ref}^{{commit}}"),
        ],
    )
    .filter(|s| valid_sha(s))?;
    let remote_tracking = git(
        cwd,
        &[
            "show-ref",
            "--verify",
            "-q",
            &format!("refs/remotes/{base_ref}"),
        ],
    )
    .is_ok();
    let name = if remote_tracking {
        base_ref.split_once('/')?.1
    } else {
        base_ref.strip_prefix("refs/heads/").unwrap_or(base_ref)
    };
    (!name.is_empty() && name.len() <= 200).then(|| (name.to_string(), tip))
}

/// How long one request of a sync may take: a bundle of a project's whole history.
pub(crate) const SYNC_TIMEOUT: Duration = Duration::from_secs(600);

/// What [`sync_base`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Synced {
    /// The base's name and the tip that was sent.
    pub name: String,
    pub tip: String,
    /// Where `origin/<name>` is in the node's mirror afterwards.
    pub sha: String,
    /// The mirror's path on the node.
    pub path: String,
}

/// `mason node sync`: send the node a full bundle of the base's tip in `repo`, then have it set
/// `origin/<name>` in the project's mirror, and give back what it says. Blocks.
pub fn sync_base(
    client: &NodeClient,
    node: &str,
    repo: &Path,
    project: &str,
    base_ref: &str,
) -> Result<Synced, String> {
    let (name, tip) = base_info(repo, base_ref)
        .ok_or_else(|| format!("{base_ref} does not resolve in {}", repo.display()))?;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!(
        "sync-{}-{}",
        now_ms(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let tag = TempRef::new(repo, &id, &tip)?;
    // No exclusions: whatever the mirror holds or lost, it has the whole base afterwards.
    let bundle = git(repo, &["bundle", "create", "-", &tag.name])?;
    let said = |r: Response, what: &str| {
        if r.status == 200 {
            Ok(r)
        } else if r.status == 404 {
            Err(format!(
                "node {node} cannot {what}: its runtime is older than this coordinator's (`mason node update {node}`)"
            ))
        } else {
            Err(format!("node {node} did not {what}: {}", r.error_text()))
        }
    };
    let r = answered(block_on(client.send(
        "PUT",
        &format!("/node/mirror/{project}"),
        Some("application/octet-stream"),
        bundle,
    )))?;
    said(r, "take the bundle")?;
    let r = answered(block_on(client.request(
        "POST",
        &format!("/node/mirror/{project}/base"),
        Some(&json!({"name": name, "sha": tip})),
    )))?;
    let v = said(r, &format!("set origin/{name}"))?.json();
    let sha = v["sha"]
        .as_str()
        .filter(|s| valid_sha(s))
        .ok_or_else(|| format!("node {node} did not say where origin/{name} is"))?;
    Ok(Synced {
        sha: sha.to_string(),
        path: v["path"].as_str().unwrap_or_default().to_string(),
        name,
        tip,
    })
}

/// What the node answered to a start: it has the job, no room, or lacks the commit.
enum Started {
    Yes,
    NoRoom(String),
    Lacks,
}

struct Candidate {
    nid: String,
    name: String,
    client: NodeClient,
    lease_s: u64,
}

fn candidates(d: &Arc<Daemon>, want: Option<&str>, spec: &RunSpec) -> Vec<Candidate> {
    let cfg = d.nodes.cfg(&d.cfg.config_dir);
    let Ok(identity) = crate::lan::load_or_create_identity(&d.cfg.data_dir) else {
        return vec![];
    };
    let mut rows = d.nodes.usable(&cfg, &spec.suite, &spec.command);
    // The node the placement chose first, then the rest as they were ranked.
    if let Some(w) = want {
        if let Some(i) = rows.iter().position(|r| r.0.nid == w) {
            let first = rows.remove(i);
            rows.insert(0, first);
        }
    }
    rows.into_iter()
        .filter_map(|(row, host, port, _status)| {
            let client = NodeClient::new(identity.clone(), &host, port, &row.fingerprint).ok()?;
            Some(Candidate {
                lease_s: cfg.settings_for(&row.name).lease_s,
                nid: row.nid,
                name: row.name,
                client: client.with_timeout(Duration::from_secs(60)),
            })
        })
        .collect()
}

fn answered(r: Result<Response, ClientError>) -> Result<Response, String> {
    r.map_err(|e| e.to_string())
}

/// Send the node what it lacks of the snapshot, as a bundle of the commits it has none of.
#[allow(clippy::too_many_arguments)]
fn send_snapshot(
    c: &Candidate,
    cwd: &Path,
    project: &str,
    id: &str,
    sha: &str,
    base_ref: &str,
    base: Option<&(String, String)>,
    full: bool,
) -> Result<(), String> {
    let mut have: Vec<String> = vec![];
    if !full {
        let cands = base_candidates(cwd, sha, base_ref);
        if !cands.is_empty() {
            let q: String = cands.iter().map(|s| format!("c={s}&")).collect();
            let r = answered(block_on(c.client.request(
                "GET",
                &format!("/node/mirror/{project}/has?{}", q.trim_end_matches('&')),
                None,
            )))?;
            if r.status == 200 {
                have = r.json()["has"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
            }
        }
    }
    let tag = TempRef::new(cwd, id, sha)?;
    // The base tip goes as a ref of its own when the node lacks it: it need not be an ancestor of
    // the snapshot (a branch that is behind, or a base that moved on), so the snapshot's ref does
    // not carry it. One the node has is excluded, and git would drop its ref with a warning.
    let base_tag = match base {
        Some((_, tip)) if !have.contains(tip) && tip != sha => {
            Some(TempRef::new(cwd, &format!("{id}-base"), tip)?)
        }
        _ => None,
    };
    let excludes: Vec<String> = have.iter().map(|h| format!("^{h}")).collect();
    let mut args: Vec<&str> = vec!["bundle", "create", "-", &tag.name];
    if let Some(b) = &base_tag {
        args.push(&b.name);
    }
    args.extend(excludes.iter().map(String::as_str));
    let bundle = git(cwd, &args)?;
    let r = answered(block_on(c.client.send(
        "PUT",
        &format!("/node/mirror/{project}"),
        Some("application/octet-stream"),
        bundle,
    )))?;
    if r.status == 200 {
        return Ok(());
    }
    Err(format!(
        "node {} refused the bundle: {}",
        c.name,
        r.error_text()
    ))
}

fn start_job(c: &Candidate, body: &Value) -> Result<Started, String> {
    let r = answered(block_on(c.client.request("POST", "/node/jobs", Some(body))))?;
    match r.status {
        200 | 201 => Ok(Started::Yes),
        409 => Ok(Started::NoRoom(r.error_text())),
        412 => Ok(Started::Lacks),
        _ => Err(format!(
            "node {} would not start the job: {}",
            c.name,
            r.error_text()
        )),
    }
}

/// Send `spec` to a node and have the job started there, leaving the record that a restart finds
/// it by. `want`: the node the placement chose (tried first). `cwd`: the worktree the command
/// would run in, which a snapshot is taken of. A node with no room is not a failure: the next is
/// tried, and when none has room the run is [`Fail::Environmental`].
pub fn start(
    d: &Arc<Daemon>,
    want: Option<&str>,
    spec: &RunSpec,
    cwd: &Path,
    base_ref: &str,
    dir: &Path,
) -> Result<Record, Fail> {
    let _ = std::fs::create_dir_all(dir);
    let rec_path = record_path(dir, &spec.id);
    let cands = candidates(d, want, spec);
    if cands.is_empty() {
        return Err(Fail::Environmental("no node is taking work".into()));
    }
    let sha = crate::remote::snapshot(
        cwd,
        base_ref,
        None,
        &crate::remote::run_json(spec),
        &format!("mason run {} ({})", spec.id, spec.suite),
    )
    .map_err(|e| Fail::Error(format!("cannot snapshot the worktree: {e}")))?;
    let run: Value = serde_json::from_str(&crate::remote::run_json(spec)).unwrap_or(Value::Null);
    let base = base_info(cwd, base_ref);
    if base.is_none() {
        tracing::warn!(run = %spec.id, project = %spec.project, "base not sent: {base_ref} does not resolve here");
    }
    let mut why: Vec<String> = vec![];
    'nodes: for c in &cands {
        // The record first: a daemon that dies from here on finds the run.
        let rec = Record {
            nid: c.nid.clone(),
            name: c.name.clone(),
            job: spec.id.clone(),
            sha: sha.clone(),
            sent_ms: now_ms(),
            base: base_record(base.as_ref()),
        };
        save(&rec_path, &rec);
        let mut body = json!({
            "id": spec.id, "project": spec.project, "sha": sha, "run": run,
            "limit_s": spec.limit.as_secs(), "lease_s": c.lease_s,
        });
        // Additive: a node that does not know `base` ignores it.
        if let Some((name, tip)) = &base {
            body["base"] = json!({"name": name, "sha": tip});
        }
        let mut full = false;
        for attempt in 0..2 {
            if let Err(e) = send_snapshot(
                c,
                cwd,
                &spec.project,
                &spec.id,
                &sha,
                base_ref,
                base.as_ref(),
                full,
            ) {
                why.push(e);
                continue 'nodes;
            }
            match start_job(c, &body) {
                Ok(Started::Yes) => return Ok(rec),
                Ok(Started::NoRoom(w)) => {
                    why.push(format!("node {}: {w}", c.name));
                    continue 'nodes;
                }
                // The mirror lacks the commit after all: once more with the whole history.
                Ok(Started::Lacks) if attempt == 0 => full = true,
                Ok(Started::Lacks) => {
                    why.push(format!(
                        "node {} still lacks the commit after a full bundle",
                        c.name
                    ));
                    continue 'nodes;
                }
                Err(e) => {
                    why.push(e);
                    continue 'nodes;
                }
            }
        }
    }
    let _ = std::fs::remove_file(&rec_path);
    Err(Fail::Environmental(format!(
        "no node had room: {}",
        why.join("; ")
    )))
}

/// Run `spec` on a node and give back its result: [`start`] it (or, after a restart, attach to
/// the job its record names), then [`follow`] it. `live` is told each piece of output as it
/// arrives.
///
/// `want` marked by [`crate::place::wait_nid`] is a run that must not come back to this machine
/// (a nodes-only project in `remote` mode, #1150): no node having room, or the node giving it up,
/// is waited out and tried again, for as long as `stop` does not end it. Such a call never
/// returns [`Fail::Environmental`]. A node that cannot give the job its base is not waited for:
/// that is told at once, as a [`Fail::Error`] made by [`crate::remote::env_err`], so the caller
/// shows it as an outside failure and tries again later.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    d: &Arc<Daemon>,
    want: Option<&str>,
    spec: &RunSpec,
    cwd: &Path,
    base_ref: &str,
    dir: &Path,
    stop: &dyn Fn() -> Option<StopWhy>,
    live: &mut dyn FnMut(u64, &[u8]),
) -> Result<Outcome, Fail> {
    let _run = crate::nodewait::register(&spec.id, &spec.project, &spec.suite, now_ms());
    let Some(chosen) = want.and_then(crate::place::wait_for) else {
        return attempt(d, want, spec, cwd, base_ref, dir, stop, live);
    };
    // The run holds the broker's unit of the node `nodewait::pick` names (when any could take
    // it), so that is the node it goes to first.
    let chosen = if chosen.is_empty() {
        crate::nodewait::pick_for(d, &spec.project, &spec.suite, &spec.command).unwrap_or_default()
    } else {
        chosen.to_string()
    };
    let step = poll_interval();
    let mut chosen = Some(chosen.as_str());
    let mut failed = 0u32;
    let mut said = false;
    let mut waiting = Waiting::new(d, &spec.id);
    // How long to leave the nodes alone after an attempt that gave nothing.
    let mut rest = Duration::ZERO;
    let mut since = std::time::Instant::now();
    loop {
        if let Some(why) = stop() {
            return Ok(Outcome::Stopped(why));
        }
        if d.is_shutting_down() {
            return Ok(Outcome::Stopped(StopWhy::Gone));
        }
        // A job already sent is attached to whatever room the nodes say they have: it is the
        // one taking it up.
        let sent = load(&record_path(dir, &spec.id)).is_some();
        let room = if sent { None } else { no_room(d, spec) };
        if sent || (since.elapsed() >= rest && room.is_none()) {
            waiting.end();
            match attempt(d, chosen.take(), spec, cwd, base_ref, dir, stop, live) {
                Err(Fail::Environmental(why)) if is_base_failure(&why) => {
                    tracing::warn!(run = %spec.id, "{why}");
                    return Err(Fail::Error(crate::remote::env_err(&why)));
                }
                Err(Fail::Environmental(why)) => {
                    tracing::warn!(run = %spec.id, "{why}; waiting for a node (this project has no GitHub setup)");
                    // Doubling, up to 64 polls.
                    rest = step * (1u32 << failed.min(6));
                    failed += 1;
                    since = std::time::Instant::now();
                }
                other => return other,
            }
        } else {
            // Nothing is running: say so on the task, with why, as every other wait does.
            // Resting after a node gave the job back reads as a full node: it had no room.
            let why = room.unwrap_or(crate::nodewait::Why::Full);
            if !said {
                said = true;
                tracing::info!(run = %spec.id, project = %spec.project, "waiting for a node: {}", why.text());
            }
            waiting.start(&why);
        }
        std::thread::sleep(step);
    }
}

/// The wait this run has put on its task while it waits in [`execute`] with no unit in the
/// broker's line: the same `WaitStarted` and `WaitEnded` every other wait emits, so the task
/// reads as waiting and not running. Ends with the run, whichever way it ends.
struct Waiting<'a> {
    d: &'a Arc<Daemon>,
    id: String,
    task: Option<mason_core::TaskId>,
    on: bool,
}

impl<'a> Waiting<'a> {
    fn new(d: &'a Arc<Daemon>, id: &str) -> Self {
        Waiting {
            d,
            id: id.to_string(),
            task: crate::nodewait::task_of_run(id).map(mason_core::TaskId),
            on: false,
        }
    }

    fn start(&mut self, why: &crate::nodewait::Why) {
        crate::nodewait::set_waiting(&self.id, Some((why.clone(), now_ms())));
        if self.on {
            return;
        }
        if let Some(task) = self.task {
            let now = now_ms();
            let text = crate::nodewait::describe(
                why,
                &crate::nodewait::holders(self.d, now),
                None,
                None,
                now,
            );
            self.on = self
                .d
                .emit(mason_core::EventKind::WaitStarted {
                    task,
                    on: text,
                    class: Some("environment".into()),
                })
                .is_ok();
        }
    }

    fn end(&mut self) {
        crate::nodewait::set_waiting(&self.id, None);
        if self.on {
            if let Some(task) = self.task {
                let _ = self.d.emit(mason_core::EventKind::WaitEnded { task });
            }
            self.on = false;
        }
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.end();
    }
}

/// Why no node can take `spec` now, or `None` when one says it has a job free: nodes on and open
/// to the project, a node accepting with the toolchains the run needs and a job free. The
/// broker's own count is not asked: the run holds its unit of the node already, or none could
/// ever take it.
fn no_room(d: &Arc<Daemon>, spec: &RunSpec) -> Option<crate::nodewait::Why> {
    let (enabled, allowed, cands) =
        crate::nodes::candidates_for(d, &spec.project, &spec.suite, &spec.command);
    crate::nodewait::diagnose(enabled, allowed, &cands)
}

pub(crate) fn poll_interval() -> Duration {
    Duration::from_millis(
        std::env::var("MASON_NODE_POLL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000),
    )
}

/// One try of [`execute`].
#[allow(clippy::too_many_arguments)]
fn attempt(
    d: &Arc<Daemon>,
    want: Option<&str>,
    spec: &RunSpec,
    cwd: &Path,
    base_ref: &str,
    dir: &Path,
    stop: &dyn Fn() -> Option<StopWhy>,
    live: &mut dyn FnMut(u64, &[u8]),
) -> Result<Outcome, Fail> {
    attempt_noting(d, want, spec, cwd, base_ref, dir, stop, live, &|| {})
}

/// [`attempt`], telling `on_start` once a node has the job (or already had it): a run split into
/// jobs (#1252) waits for room only while one of its own jobs is out.
#[allow(clippy::too_many_arguments)]
pub(crate) fn attempt_noting(
    d: &Arc<Daemon>,
    want: Option<&str>,
    spec: &RunSpec,
    cwd: &Path,
    base_ref: &str,
    dir: &Path,
    stop: &dyn Fn() -> Option<StopWhy>,
    live: &mut dyn FnMut(u64, &[u8]),
    on_start: &dyn Fn(),
) -> Result<Outcome, Fail> {
    let rec = match load(&record_path(dir, &spec.id)) {
        // The node it names is paired still: ask it for the job it has, taking no new snapshot.
        Some(r) if d.nodes.client_for(d, &r.nid).is_some() => r,
        Some(_) => {
            let _ = std::fs::remove_file(record_path(dir, &spec.id));
            start(d, want, spec, cwd, base_ref, dir)?
        }
        None => start(d, want, spec, cwd, base_ref, dir)?,
    };
    on_start();
    follow(d, &rec, spec, cwd, base_ref, dir, stop, live)
}

/// Tells the "Remote runs" list that a run ended, when it goes out of scope.
struct Ended {
    scope: PathBuf,
    id: String,
    outcome: std::cell::Cell<&'static str>,
}

impl Drop for Ended {
    fn drop(&mut self) {
        crate::remote_runs::finished(&self.scope, &self.id, Some(self.outcome.get()));
    }
}

/// Follow a started job to its end: ask after it (which renews its lease), tail its output, and
/// bring the result back. A node that stops answering for `lost_after_polls` polls in a row is
/// given up on ([`Fail::Environmental`]), and so is one that cancelled the job or could not run it,
/// or whose job did not get its base ([`is_base_failure`]).
#[allow(clippy::too_many_arguments)]
pub fn follow(
    d: &Arc<Daemon>,
    rec: &Record,
    spec: &RunSpec,
    cwd: &Path,
    base_ref: &str,
    dir: &Path,
    stop: &dyn Fn() -> Option<StopWhy>,
    live: &mut dyn FnMut(u64, &[u8]),
) -> Result<Outcome, Fail> {
    let rec_path = record_path(dir, &spec.id);
    let log = dir.join(format!("{}.log", spec.id));
    let poll = poll_interval();
    let cfg = d.nodes.cfg(&d.cfg.config_dir);
    let Some((row, client, arch)) = d.nodes.client_for(d, &rec.nid) else {
        let _ = std::fs::remove_file(&rec_path);
        return Err(Fail::Environmental(format!(
            "node {} is not paired any more",
            rec.name
        )));
    };
    let name = row.name.clone();
    let id = spec.id.clone();
    // Shown in the monitor's "Remote runs" until it ends, however it ends.
    let hosts = d.cfg.hosts_dir();
    crate::remote_runs::node_started(&hosts, &id, &spec.suite, &name, &rec_path);
    // On the node's list of runs until this returns, so a job it gives back leaves it (#1155).
    let (kind, task) = crate::running::classify(&id);
    let _running = crate::running::start(
        &hosts,
        crate::running::Run {
            id: id.clone(),
            project: spec.project.clone(),
            task,
            kind,
            name: if spec.suite.is_empty() {
                id.clone()
            } else {
                spec.suite.clone()
            },
            host: crate::running::Host::Node(name.clone()),
            since_ms: rec.sent_ms,
            url: String::new(),
        },
    );
    let _ended = Ended {
        scope: hosts,
        id: id.clone(),
        outcome: std::cell::Cell::new("cancelled"),
    };
    let mut off = std::fs::metadata(&log).map_or(0, |m| m.len());
    let mut misses = 0u32;
    let give_up = rec.sent_ms + spec.limit.as_millis() as u64 + MARGIN_MS;
    let lost_after = cfg.lost_after_polls.max(1);
    let end = loop {
        if let Some(why) = stop() {
            let _ = block_on(client.request("DELETE", &format!("/node/jobs/{id}"), None));
            let _ = std::fs::remove_file(&rec_path);
            return Ok(Outcome::Stopped(why));
        }
        let st = block_on(client.request("GET", &format!("/node/jobs/{id}"), None));
        match st {
            Ok(r) if r.status == 200 => {
                misses = 0;
                let job = r.json()["job"].clone();
                let base = job["base"].as_str().unwrap_or_default();
                if let Some(why) = base_failure(&name, &spec.project, base_ref, &rec.base, base) {
                    // Cancelled while it runs; a job that has ended is collected away.
                    let _ = block_on(client.request("DELETE", &format!("/node/jobs/{id}"), None));
                    let _ = block_on(client.request("POST", &format!("/node/jobs/{id}/ack"), None));
                    let _ = std::fs::remove_file(&rec_path);
                    let _ = std::fs::write(dir.join(format!("{id}.where")), format!("{why}\n"));
                    return Err(Fail::Environmental(why));
                }
                // Tail what arrived, until nothing more does.
                loop {
                    let l = block_on(client.request(
                        "GET",
                        &format!("/node/jobs/{id}/log?from={off}"),
                        None,
                    ));
                    match l {
                        Ok(l) if l.status == 200 && !l.body.is_empty() => {
                            use std::io::Write;
                            if let Ok(mut f) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&log)
                            {
                                let _ = f.write_all(&l.body);
                            }
                            live(off, &l.body);
                            off += l.body.len() as u64;
                        }
                        _ => break,
                    }
                }
                match job["state"].as_str().unwrap_or("running") {
                    "running" => {}
                    other => break (other.to_string(), job),
                }
            }
            Ok(r) if r.status == 404 => {
                let _ = std::fs::remove_file(&rec_path);
                return Err(Fail::Environmental(format!(
                    "node {name} no longer has the job"
                )));
            }
            Ok(r) if r.status == 403 => {
                let _ = std::fs::remove_file(&rec_path);
                return Err(Fail::Environmental(format!(
                    "node {name} no longer trusts this coordinator"
                )));
            }
            Ok(_) | Err(_) => {
                misses += 1;
                if misses >= lost_after {
                    let _ = block_on(client.request("DELETE", &format!("/node/jobs/{id}"), None));
                    let _ = std::fs::remove_file(&rec_path);
                    return Err(Fail::Environmental(format!(
                        "node {name} went away ({misses} polls without an answer)"
                    )));
                }
            }
        }
        if now_ms() > give_up {
            let _ = block_on(client.request("DELETE", &format!("/node/jobs/{id}"), None));
            let _ = std::fs::remove_file(&rec_path);
            return Err(Fail::Environmental(format!(
                "node {name} did not finish within the limit of {} s",
                spec.limit.as_secs()
            )));
        }
        std::thread::sleep(poll);
    };
    let (state, job) = end;
    if matches!(state.as_str(), "cancelled" | "failed") {
        let why = job["why"]
            .as_str()
            .unwrap_or("it did not say why")
            .to_string();
        let _ = block_on(client.request("POST", &format!("/node/jobs/{id}/ack"), None));
        let _ = std::fs::remove_file(&rec_path);
        return Err(Fail::Environmental(format!(
            "node {name} {state} the job: {why}"
        )));
    }
    // The result, and the results files where a local run would have left them.
    let art = answered(block_on(client.request(
        "GET",
        &format!("/node/jobs/{id}/artifact"),
        None,
    )))
    .map_err(Fail::Environmental)?;
    if art.status != 200 {
        return Err(Fail::Environmental(format!(
            "node {name} gave no result: {}",
            art.error_text()
        )));
    }
    let a = art.json();
    // Whatever of the output has not arrived yet.
    loop {
        match block_on(client.request("GET", &format!("/node/jobs/{id}/log?from={off}"), None)) {
            Ok(l) if l.status == 200 && !l.body.is_empty() => {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log)
                {
                    let _ = f.write_all(&l.body);
                }
                live(off, &l.body);
                off += l.body.len() as u64;
            }
            _ => break,
        }
    }
    if spec.results_file.is_some() {
        for f in a["results"].as_array().into_iter().flatten() {
            let (Some(rel), Some(bytes)) = (
                f["path"].as_str().and_then(plain_relative),
                f["b64"].as_str().and_then(base64_decode),
            ) else {
                continue;
            };
            let p = cwd.join(rel);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(p, bytes);
        }
    }
    let r = &a["result"];
    let (build_s, run_s) = (
        r["build_s"].as_u64().unwrap_or(0),
        r["run_s"].as_u64().unwrap_or(0),
    );
    _ended.outcome.set(
        if r["code"].as_i64() == Some(0) && r["signal"].as_i64().unwrap_or(0) == 0 {
            "green"
        } else {
            "red"
        },
    );
    let finished = Finished {
        code: r["code"].as_i64().unwrap_or(-1) as i32,
        signal: r["signal"].as_i64().unwrap_or(0) as i32,
        output: crate::jobs::read_lossy(&log).unwrap_or_default(),
        timed_out: r["timed_out"].as_bool().unwrap_or(false),
        stop: None,
        ran_ms: Some((build_s + run_s) * 1000),
        phase: r["phase"]
            .as_str()
            .filter(|p| matches!(*p, "build" | "run"))
            .map(str::to_string),
        ran_on: format!(
            "{}, {}",
            ran_on(&name, &arch, build_s + run_s),
            base_note(job["base"].as_str().unwrap_or_default())
        ),
        parts: vec![],
        missing: vec![],
    };
    let _ = std::fs::write(
        dir.join(format!("{}.where", spec.id)),
        format!("{}\n", finished.ran_on),
    );
    let _ = block_on(client.request("POST", &format!("/node/jobs/{id}/ack"), None));
    let _ = std::fs::remove_file(&rec_path);
    Ok(Outcome::Done(finished))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_run_says_where_and_how_long() {
        assert_eq!(
            ran_on("studio", "arm64", 12),
            "ran on node studio (arm64), 12 s"
        );
        assert_eq!(
            ran_on("studio", "arm64", 180),
            "ran on node studio (arm64), 3 min"
        );
        assert_eq!(ran_on("studio", "", 100), "ran on node studio, 2 min");
    }

    #[test]
    fn only_plain_relative_paths_come_back_from_a_node() {
        assert!(plain_relative("a/b.xml").is_some());
        assert!(plain_relative("build/res/x.xml").is_some());
        for bad in ["", "/etc/passwd", "../x", "a/../../x", "./a/../b"] {
            assert!(plain_relative(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_base_that_does_not_resolve_is_recorded_as_not_sent_and_fails_the_node() {
        let t = tempfile::tempdir().unwrap();
        git(t.path(), &["init", "-q"]).unwrap();
        let base = base_info(t.path(), "origin/develop");
        assert_eq!(base, None);
        assert_eq!(base_record(base.as_ref()), "not sent");
        // The record keeps it, and it is a node failure whatever the node says.
        let why = base_failure("studio", "p", "origin/develop", "not sent", "").unwrap();
        assert!(is_base_failure(&why), "{why}");
        assert!(
            why.starts_with("origin/develop not set on node studio (base: not sent"),
            "{why}"
        );
    }

    #[test]
    fn only_a_failed_or_missing_base_is_a_node_failure() {
        let sent = format!("develop@{}", "a".repeat(40));
        let set = format!("set develop@{}", "a".repeat(40));
        let of = |state: &str| base_failure("studio", "p", "origin/develop", &sent, state);
        // Unknown yet, an older node, and a base that was set are no failure.
        assert_eq!(of(""), None);
        assert_eq!(of(&set), None);
        let why = of("failed: origin/develop: locked").unwrap();
        assert!(
            why.starts_with("origin/develop not set on node studio (base: failed: "),
            "{why}"
        );
        assert!(why.contains("mason node sync studio --project p"), "{why}");
        assert!(of("not sent").is_some());
        assert!(!is_base_failure("node studio went away"));
        assert_eq!(base_note(&set), "base: set develop@aaaaaaaaaa");
        assert_eq!(base_note(""), "base: not reported (older node runtime)");
    }

    #[test]
    fn the_record_survives_a_round_trip() {
        let r = Record {
            nid: "n".into(),
            name: "studio".into(),
            job: "t1r1".into(),
            sha: "a".repeat(40),
            sent_ms: 9,
            base: base_record(None),
        };
        let t = tempfile::tempdir().unwrap();
        let p = record_path(t.path(), "t1r1");
        save(&p, &r);
        assert_eq!(load(&p), Some(r));
        assert!(load(&t.path().join("nope.node")).is_none());
    }
}
