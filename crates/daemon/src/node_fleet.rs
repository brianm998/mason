// SPDX-License-Identifier: GPL-3.0-or-later

//! Updating every node with the coordinator (s1122, decision 1111).
//!
//! `mason update` is a fleet update. While the coordinator builds its own new runtime, a
//! cross-build of the same commit is made for each node architecture ([`begin`]); once the
//! coordinator's build has passed its self-test and before it takes over, each is sent to the
//! nodes of that architecture ([`deliver`]). A node installs it and takes over when its jobs end,
//! as it does for any runtime ([`crate::node_serve`]). A node that is off or unreachable is
//! "will update when it returns": the poll ([`crate::nodes`]) sends it the runtime when it next
//! answers, and `mason node update` sends it now ([`start_push`] with `force`).
//!
//! What each node's update looks like to the owner ([`Fleet`]) is worked out from what the node
//! reports (its running and installed runtimes) and from what this coordinator last tried
//! ([`Record`]); nothing is kept on disk, because after a takeover the node's own report is the
//! truth.
//!
//! Building and sending go through [`FleetIo`], so that tests deliver without a Developer ID.

use crate::daemon::{now_ms, Daemon};
use crate::update::{Runtimes, UpdateCfg};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long after a try (failed, or sent and not yet seen) the poll tries again.
pub const PUSH_RETRY_MS: u64 = 10 * 60 * 1000;

// ---- what was tried -----------------------------------------------------------------------

/// Where the last try to bring a node up to date had got to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// A runtime is being built for this architecture.
    Building(String),
    Sending,
    /// The node took it; its next status shows it.
    Sent,
    Failed(String),
}

/// The last try for one node: the commit, how far it got, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub sha: String,
    pub phase: Phase,
    pub at_ms: u64,
}

impl Record {
    fn inflight(&self) -> bool {
        matches!(self.phase, Phase::Building(_) | Phase::Sending)
    }
}

fn records() -> &'static Mutex<BTreeMap<String, Record>> {
    static R: std::sync::OnceLock<Mutex<BTreeMap<String, Record>>> = std::sync::OnceLock::new();
    R.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn set(nid: &str, sha: &str, phase: Phase) {
    records().lock().unwrap_or_else(|e| e.into_inner()).insert(
        nid.to_string(),
        Record {
            sha: sha.to_string(),
            phase,
            at_ms: now_ms(),
        },
    );
}

/// Forget the last try for node `nid` (a test starting over).
pub fn forget(nid: &str) {
    records()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(nid);
}

/// The last try for node `nid`.
pub fn record_of(nid: &str) -> Option<Record> {
    records()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(nid)
        .cloned()
}

// ---- what the owner sees ------------------------------------------------------------------

/// One node's update, from the owner's side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fleet {
    UpToDate,
    /// Building the runtime for this architecture.
    Building(String),
    Sending,
    /// Sent; the node has not reported it yet.
    Sent,
    /// Installed on the node, which takes over when its `n` jobs have ended.
    WaitingForJobs(u32),
    Failed(String),
    /// Off, or not answering: it is sent the runtime when it next answers.
    WillUpdateWhenBack,
    /// Answering and lacking the runtime, and nothing has been started (the next poll does).
    Queued,
}

impl Fleet {
    /// A word for a program to key on.
    pub fn key(&self) -> &'static str {
        match self {
            Fleet::UpToDate => "up-to-date",
            Fleet::Building(_) => "building",
            Fleet::Sending => "sending",
            Fleet::Sent => "sent",
            Fleet::WaitingForJobs(_) => "installing",
            Fleet::Failed(_) => "failed",
            Fleet::WillUpdateWhenBack => "pending",
            Fleet::Queued => "queued",
        }
    }

    /// The words shown by `mason update status`, `mason node list` and the monitor.
    pub fn text(&self) -> String {
        match self {
            Fleet::UpToDate => "up to date".into(),
            Fleet::Building(arch) => format!("building for {arch}"),
            Fleet::Sending => "sending the new runtime".into(),
            Fleet::Sent => "sent; installing".into(),
            Fleet::WaitingForJobs(0) => "installed; taking over now".into(),
            Fleet::WaitingForJobs(n) => format!(
                "installed; waiting for {n} job{} to end",
                if *n == 1 { "" } else { "s" }
            ),
            Fleet::Failed(why) => format!("failed ({why})"),
            Fleet::WillUpdateWhenBack => "will update when it returns".into(),
            Fleet::Queued => "will be sent the new runtime at the next poll".into(),
        }
    }
}

/// Where one node's update stands, given what it reported, the commit the fleet is moving to,
/// and the last try. Pure.
///
/// `status` is the node's last answer (`None`: it never answered); `reachable` is whether the
/// last poll succeeded. A try for another commit than `goal` is history.
pub fn fleet_state(
    status: Option<&Value>,
    reachable: bool,
    goal: &str,
    rec: Option<&Record>,
) -> Fleet {
    if let Some(s) = status {
        if s["runtime_sha"].as_str() == Some(goal) {
            return Fleet::UpToDate;
        }
        let held = s["pending_runtime"].as_str() == Some(goal)
            || s["runtimes"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(goal)));
        if held {
            return Fleet::WaitingForJobs(s["jobs"]["running"].as_u64().unwrap_or(0) as u32);
        }
    }
    match rec.filter(|r| r.sha == goal).map(|r| &r.phase) {
        Some(Phase::Building(a)) => Fleet::Building(a.clone()),
        Some(Phase::Sending) => Fleet::Sending,
        Some(Phase::Failed(e)) => Fleet::Failed(e.clone()),
        Some(Phase::Sent) => Fleet::Sent,
        None if reachable && status.is_some() => Fleet::Queued,
        None => Fleet::WillUpdateWhenBack,
    }
}

/// The commit the fleet is moving to: the update's target while one is under way, else the one
/// this coordinator runs. `None` for a daemon that is not running from a runtime.
pub fn goal(d: &Daemon) -> Option<String> {
    let s = d.update_status.lock().unwrap_or_else(|e| e.into_inner());
    match (s.phase.as_str(), &s.target) {
        ("building" | "testing" | "waiting for landing" | "taking over", Some(t)) => {
            Some(t.clone())
        }
        _ => d.cfg.runtime_sha.clone(),
    }
}

/// Is an update (or a takeover) under way?
pub fn update_under_way(d: &Daemon) -> bool {
    matches!(
        d.update_status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .phase
            .as_str(),
        "fetching" | "building" | "testing" | "waiting for landing" | "taking over"
    )
}

/// Every paired node's update against `goal`, as `(nid, name, state)`.
pub fn states_for(d: &Daemon, goal: &str) -> Vec<(String, String, Fleet)> {
    let (rows, views) = d.nodes.snapshot();
    rows.iter()
        .map(|r| {
            let v = views.get(&r.nid);
            let status = v.and_then(|v| v.status.as_ref());
            let reachable = v.is_some_and(|v| v.failures == 0);
            let f = fleet_state(status, reachable, goal, record_of(&r.nid).as_ref());
            (r.nid.clone(), r.name.clone(), f)
        })
        .collect()
}

/// [`states_for`] the commit the fleet is moving to.
pub fn states(d: &Daemon) -> Vec<(String, String, Fleet)> {
    goal(d).map_or_else(Vec::new, |g| states_for(d, &g))
}

/// [`states`] as JSON, for `GET /update`: the lines of `mason update status` and the monitor.
pub fn states_json(d: &Daemon) -> Value {
    Value::Array(
        states(d)
            .into_iter()
            .map(|(nid, name, f)| json!({"nid": nid, "name": name, "state": f.key(), "text": f.text()}))
            .collect(),
    )
}

/// What needs the owner's eyes: a node whose update failed, until a try works (the alert table's
/// `(key, kind, text)`).
pub fn alerts(d: &Daemon) -> Vec<(String, String, String)> {
    states(d)
        .into_iter()
        .filter_map(|(nid, name, f)| match f {
            Fleet::Failed(why) => Some((
                format!("node:{nid}:update"),
                "node".to_string(),
                format!(
                    "node {name} could not be updated to the coordinator's runtime: {why} \
                     (`mason node update {name}` tries again)"
                ),
            )),
            _ => None,
        })
        .collect()
}

// ---- building and sending -----------------------------------------------------------------

/// What an update does with the outside world, so that a test can stand in for it.
pub trait FleetIo: Send + Sync {
    /// The package of runtime `sha` for a node of `arch`, cross-built when that is not this
    /// machine's. `owner`: the owner asked for the update (the compile lease ranks above task work).
    fn package(
        &self,
        d: &Arc<Daemon>,
        ucfg: &UpdateCfg,
        sha: &str,
        arch: &str,
        owner: bool,
    ) -> Result<Vec<u8>, String>;

    /// `PUT /node/runtime` to node `nid`.
    fn send(&self, d: &Arc<Daemon>, nid: &str, package: &[u8]) -> Result<(), String>;
}

/// The real thing: `cross_build` under a compile lease, one build at a time, and the node's
/// own TLS client.
pub struct RealIo;

impl FleetIo for RealIo {
    fn package(
        &self,
        d: &Arc<Daemon>,
        ucfg: &UpdateCfg,
        sha: &str,
        arch: &str,
        owner: bool,
    ) -> Result<Vec<u8>, String> {
        // Two nodes of one architecture, or a push and an update, build it once: the second
        // finds the finished build.
        static ONE_BUILD: Mutex<()> = Mutex::new(());
        let _one = ONE_BUILD.lock().unwrap_or_else(|e| e.into_inner());
        let rt = Runtimes::new(&d.cfg.data_dir);
        if crate::node_runtime::arch_matches_host(arch) {
            return crate::node_runtime::package_of(ucfg, &rt, sha, arch);
        }
        let started = Instant::now();
        crate::update::with_compile_lease(
            &d.broker,
            "node runtime build",
            owner,
            Duration::from_secs(ucfg.update_wait_max_s),
            &|| started.elapsed(),
            &|| {},
            || crate::node_runtime::package_of(ucfg, &rt, sha, arch),
        )
    }

    fn send(&self, d: &Arc<Daemon>, nid: &str, package: &[u8]) -> Result<(), String> {
        let (_, client, _) = d
            .nodes
            .client_for(d, nid)
            .ok_or("the node is not paired any more")?;
        let client = client.with_timeout(Duration::from_secs(900));
        let r = crate::node_client::block_on(client.send(
            "PUT",
            crate::node_serve::RUNTIME_ROUTE,
            Some("application/octet-stream"),
            package.to_vec(),
        ))
        .map_err(|e| e.to_string())?;
        if r.status == 200 {
            Ok(())
        } else {
            Err(r.error_text())
        }
    }
}

/// A paired node that has answered and lacks the runtime `sha`.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub nid: String,
    pub name: String,
    pub arch: String,
}

/// The paired nodes that need `sha` ([`crate::nodes::needs_runtime`]); a node on another protocol
/// is one of them. A node that did not answer its last poll is not (it is "will update when it
/// returns", and the first poll that finds it sends the runtime), nor is one that never answered:
/// its architecture is not known.
pub fn candidates(d: &Daemon, sha: &str) -> Vec<Candidate> {
    let (rows, views) = d.nodes.snapshot();
    rows.into_iter()
        .filter_map(|r| {
            let v = views.get(&r.nid).filter(|v| v.failures == 0)?;
            let status = v.status.as_ref()?;
            crate::nodes::needs_runtime(status, sha).then(|| Candidate {
                nid: r.nid,
                name: r.name,
                arch: status["arch"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

fn log(d: &Daemon, line: String) {
    tracing::info!("{line}");
    d.update_status
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .log
        .push(line);
    d.poke();
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// Bring node `c` up to date with `sha` on a thread of its own: build for its architecture, send,
/// record how it went. `force` ignores the retry wait of an earlier try (`mason node update`).
/// `Err` says why nothing was started.
pub fn start_push(
    d: &Arc<Daemon>,
    ucfg: &UpdateCfg,
    c: &Candidate,
    sha: &str,
    force: bool,
) -> Result<(), String> {
    {
        let mut p = records().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = p.get(&c.nid).filter(|r| r.sha == sha) {
            if r.inflight() {
                return Err("a runtime is already being sent to it".into());
            }
            if !force && now_ms().saturating_sub(r.at_ms) < PUSH_RETRY_MS {
                return Err("tried a moment ago; the poll tries again in a few minutes".into());
            }
        }
        p.insert(
            c.nid.clone(),
            Record {
                sha: sha.to_string(),
                phase: Phase::Building(c.arch.clone()),
                at_ms: now_ms(),
            },
        );
    }
    let (d, ucfg, c, sha) = (d.clone(), ucfg.clone(), c.clone(), sha.to_string());
    let spawned = std::thread::Builder::new()
        .name("node-runtime-push".into())
        .spawn(move || {
            let io = d.nodes.io();
            let r = io
                .package(&d, &ucfg, &sha, &c.arch, false)
                .and_then(|bytes| {
                    set(&c.nid, &sha, Phase::Sending);
                    io.send(&d, &c.nid, &bytes)
                });
            match r {
                Ok(()) => {
                    tracing::info!(node = %c.name, runtime = %sha, "sent a runtime to a node");
                    set(&c.nid, &sha, Phase::Sent);
                }
                Err(e) => {
                    tracing::warn!(node = %c.name, "could not send a runtime to a node: {e}");
                    set(&c.nid, &sha, Phase::Failed(e));
                }
            }
            d.poke();
        });
    spawned.map(|_| ()).map_err(|e| e.to_string())
}

// ---- the update ---------------------------------------------------------------------------

enum Build {
    Running,
    Done(Arc<Vec<u8>>),
    Failed(String),
}

type Builds = Arc<(Mutex<BTreeMap<String, Build>>, Condvar)>;

/// The cross-builds an update has started, one per node architecture other than this machine's.
pub struct Handle {
    builds: Builds,
}

/// An update to `target` has begun building: start the cross-builds for the nodes, so that they
/// run beside the coordinator's own build (under their own compile leases). Nothing is sent
/// until [`deliver`], after the coordinator's build has passed its self-test.
pub fn begin(d: &Arc<Daemon>, ucfg: &UpdateCfg, target: &str, owner: bool) -> Handle {
    let builds: Builds = Arc::new((Mutex::new(BTreeMap::new()), Condvar::new()));
    let cfg = d.nodes.cfg(&d.cfg.config_dir);
    if !cfg.push_runtime || ucfg.sign_identity.is_none() {
        return Handle { builds };
    }
    let cands = candidates(d, target);
    let archs: BTreeSet<String> = cands
        .iter()
        .filter(|c| {
            !crate::node_runtime::arch_matches_host(&c.arch)
                && crate::node_runtime::triple_for(&c.arch).is_some()
        })
        .map(|c| c.arch.clone())
        .collect();
    for arch in archs {
        for c in cands.iter().filter(|c| c.arch == arch) {
            set(&c.nid, target, Phase::Building(arch.clone()));
        }
        builds
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(arch.clone(), Build::Running);
        let (d, ucfg, target, builds) =
            (d.clone(), ucfg.clone(), target.to_string(), builds.clone());
        let _ = std::thread::Builder::new()
            .name("fleet-build".into())
            .spawn(move || {
                let r = d.nodes.io().package(&d, &ucfg, &target, &arch, owner);
                let mut g = builds.0.lock().unwrap_or_else(|e| e.into_inner());
                g.insert(
                    arch,
                    match r {
                        Ok(b) => Build::Done(Arc::new(b)),
                        Err(e) => Build::Failed(e),
                    },
                );
                drop(g);
                builds.1.notify_all();
            });
    }
    Handle { builds }
}

impl Handle {
    /// The package for `arch` once its build has finished, or why there is none, within
    /// `deadline`. `None`: no build was started for `arch`.
    fn package(&self, arch: &str, deadline: Instant) -> Option<Result<Arc<Vec<u8>>, String>> {
        let mut g = self.builds.0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match g.get(arch) {
                None => return None,
                Some(Build::Done(b)) => return Some(Ok(b.clone())),
                Some(Build::Failed(e)) => return Some(Err(e.clone())),
                Some(Build::Running) => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Some(Err("its build was not finished in time".into()));
            }
            g = self
                .builds
                .1
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// The coordinator's build of `target` has passed its self-test and it is about to take over:
/// send each node that needs it the runtime, and wait (at most `wait`) for the sends. A failure
/// is recorded per node and never fails the update: the node is tried again by the poll, and
/// `mason node update` tries it now. Returns how many nodes were sent a runtime.
pub fn deliver(
    d: &Arc<Daemon>,
    ucfg: &UpdateCfg,
    target: &str,
    handle: &Handle,
    wait: Duration,
) -> usize {
    let cfg = d.nodes.cfg(&d.cfg.config_dir);
    if d.nodes.snapshot().0.is_empty() {
        return 0;
    }
    if !cfg.push_runtime {
        log(
            d,
            "nodes are not updated: `[nodes] push_runtime` is off".into(),
        );
        return 0;
    }
    if ucfg.sign_identity.is_none() {
        log(
            d,
            "nodes are not updated: update.toml has no `sign_identity`".into(),
        );
        return 0;
    }
    let deadline = Instant::now() + wait;
    let cands = candidates(d, target);
    let io = d.nodes.io();
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let mut sending = 0;
    let mut own: BTreeMap<String, Result<Arc<Vec<u8>>, String>> = BTreeMap::new();
    for c in &cands {
        let pkg = match handle.package(&c.arch, deadline) {
            Some(r) => r,
            // This machine's own architecture (or one that was not built ahead): the build is
            // complete, so this is only packing it.
            None => own
                .entry(c.arch.clone())
                .or_insert_with(|| {
                    set(&c.nid, target, Phase::Building(c.arch.clone()));
                    io.package(d, ucfg, target, &c.arch, true).map(Arc::new)
                })
                .clone(),
        };
        match pkg {
            Err(e) => {
                set(&c.nid, target, Phase::Failed(e.clone()));
                log(d, format!("node {}: no runtime to send: {e}", c.name));
            }
            Ok(bytes) => {
                set(&c.nid, target, Phase::Sending);
                sending += 1;
                let (d2, tx, c2, sha, io) = (
                    d.clone(),
                    tx.clone(),
                    c.clone(),
                    target.to_string(),
                    io.clone(),
                );
                let spawned = std::thread::Builder::new()
                    .name("fleet-send".into())
                    .spawn(move || {
                        match io.send(&d2, &c2.nid, &bytes) {
                            Ok(()) => set(&c2.nid, &sha, Phase::Sent),
                            Err(e) => {
                                tracing::warn!(node = %c2.name, "could not send a runtime to a node: {e}");
                                set(&c2.nid, &sha, Phase::Failed(e));
                            }
                        }
                        let _ = tx.send(());
                    });
                if spawned.is_err() {
                    sending -= 1;
                    set(
                        &c.nid,
                        target,
                        Phase::Failed("no thread to send with".into()),
                    );
                }
            }
        }
    }
    drop(tx);
    for _ in 0..sending {
        let left = deadline.saturating_duration_since(Instant::now());
        if rx.recv_timeout(left).is_err() {
            break;
        }
    }
    let mut sent = 0;
    for (_, name, f) in states_for(d, target) {
        if matches!(f, Fleet::Sent | Fleet::WaitingForJobs(_)) {
            sent += 1;
        }
        log(d, format!("node {name}: {} ({})", f.text(), short(target)));
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn status(running: Option<&str>, held: &[&str], jobs: u64) -> Value {
        json!({"node_proto": 1, "arch": "arm64", "runtime_sha": running, "runtimes": held, "jobs": {"running": jobs, "max": 4}})
    }

    fn rec(phase: Phase) -> Record {
        Record {
            sha: SHA.into(),
            phase,
            at_ms: 1,
        }
    }

    #[test]
    fn a_node_goes_through_building_sent_installed_and_up_to_date() {
        let old = status(Some("b"), &["b"], 2);
        let go = |s: Option<&Value>, r: Option<Record>| fleet_state(s, true, SHA, r.as_ref());
        assert_eq!(go(Some(&old), None), Fleet::Queued);
        assert_eq!(
            go(Some(&old), Some(rec(Phase::Building("arm64".into())))),
            Fleet::Building("arm64".into())
        );
        assert_eq!(go(Some(&old), Some(rec(Phase::Sending))), Fleet::Sending);
        assert_eq!(go(Some(&old), Some(rec(Phase::Sent))), Fleet::Sent);
        // Installed: the node lists it and waits for its jobs.
        let held = status(Some("b"), &["b", SHA], 2);
        assert_eq!(
            go(Some(&held), Some(rec(Phase::Sent))),
            Fleet::WaitingForJobs(2)
        );
        let pending = json!({"runtime_sha": "b", "runtimes": [], "pending_runtime": SHA, "jobs": {"running": 1}});
        assert_eq!(go(Some(&pending), None), Fleet::WaitingForJobs(1));
        let now = status(Some(SHA), &[SHA], 0);
        assert_eq!(go(Some(&now), Some(rec(Phase::Sent))), Fleet::UpToDate);
    }

    #[test]
    fn a_failure_is_named_and_an_absent_node_will_update_when_it_returns() {
        let old = status(Some("b"), &[], 0);
        let failed = rec(Phase::Failed("it said no".into()));
        assert_eq!(
            fleet_state(Some(&old), true, SHA, Some(&failed)),
            Fleet::Failed("it said no".into())
        );
        // A try for another commit is history.
        let other = Record {
            sha: "c".repeat(40),
            ..failed.clone()
        };
        assert_eq!(
            fleet_state(Some(&old), true, SHA, Some(&other)),
            Fleet::Queued
        );
        // Off, or never heard from: pending, in the words the owner reads.
        let off = fleet_state(Some(&old), false, SHA, None);
        assert_eq!(off, Fleet::WillUpdateWhenBack);
        assert_eq!(off.text(), "will update when it returns");
        assert_eq!(
            fleet_state(None, false, SHA, None),
            Fleet::WillUpdateWhenBack
        );
        for (f, text) in [
            (Fleet::Building("arm64".into()), "building for arm64"),
            (Fleet::UpToDate, "up to date"),
            (
                Fleet::WaitingForJobs(3),
                "installed; waiting for 3 jobs to end",
            ),
            (
                Fleet::WaitingForJobs(1),
                "installed; waiting for 1 job to end",
            ),
            (Fleet::Failed("x".into()), "failed (x)"),
        ] {
            assert_eq!(f.text(), text);
        }
    }
}
