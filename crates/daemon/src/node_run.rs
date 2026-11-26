// SPDX-License-Identifier: GPL-3.0-or-later

//! What a node does with a job (`docs/multi-machine-nodes-plan.md`, §3.5-§3.7): keep a bare git
//! mirror per coordinator and project, check a snapshot out in a reused slot, run the pinned
//! build and command under step hosts, and keep the result until the coordinator has collected it.
//!
//! It is a Rust port of the `run` step of `.github/workflows/mason-run.yml`. It reads only what
//! the coordinator sent with the job (the run description, never anything of the snapshot's
//! tree), clears the environment and builds it again, and judges nothing: the coordinator reads
//! the output with the suite's own parser. A job survives a restart of this daemon, since its
//! commands run under step hosts and its state is a file.

use crate::config::NodeSettings;
use crate::daemon::{now_ms, Daemon};
use mason_landing::git;
use mason_step_host::client::{collect_to_file, ClientError, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{CURRENT, PREVIOUS};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Directories a clean between two jobs keeps, so builds stay incremental: the warm build
/// output is the main reason to use a node at all.
const KEEP: [&str; 8] = [
    "target",
    ".build",
    "build",
    "DerivedData",
    "node_modules",
    ".gradle",
    "Pods",
    ".swiftpm",
];

/// The most a log response carries.
pub const LOG_CHUNK: usize = 4 * 1024 * 1024;

/// The most result files a job's artifact carries, in bytes.
const ARTIFACT_MAX: u64 = 64 * 1024 * 1024;

/// `[A-Za-z0-9._-]`, 1 to 120 characters, not starting with a dot: a name that is safe as one
/// path component.
pub fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 120
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A full git object name: 40 or 64 hex digits.
pub fn valid_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The first 16 characters of a coordinator's fingerprint: the directory its work is kept in, so
/// two coordinators can never meet.
pub fn fp16(fingerprint: &str) -> String {
    fingerprint.chars().take(16).collect()
}

/// Where a job is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    /// Being prepared or running.
    Running,
    /// Ended on its own: `result.json` says how.
    Done,
    /// Cancelled (by the coordinator, the owner, or an expired lease).
    Cancelled,
    /// Could not be run at all (the checkout failed, a host was lost).
    Failed,
}

impl JobState {
    pub fn name(self) -> &'static str {
        match self {
            JobState::Running => "running",
            JobState::Done => "done",
            JobState::Cancelled => "cancelled",
            JobState::Failed => "failed",
        }
    }
    fn parse(s: &str) -> JobState {
        match s {
            "done" => JobState::Done,
            "cancelled" => JobState::Cancelled,
            "failed" => JobState::Failed,
            _ => JobState::Running,
        }
    }
}

/// One job, as kept in `job.json`.
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub project: String,
    pub sha: String,
    /// The coordinator's directory name (`fp16`).
    pub coord: String,
    pub build: Option<String>,
    pub command: String,
    pub results_file: Option<String>,
    /// Only `MASON_*` variables, as the coordinator sent them.
    pub env: Vec<(String, String)>,
    pub limit_s: u64,
    pub lease_s: u64,
    /// The base branch the coordinator measures from, as `(name, tip)`: the checkout gets
    /// `origin/<name>` at `tip`. Absent from an older coordinator.
    pub base: Option<(String, String)>,
    pub state: JobState,
    /// `build` or `run`: where the job is, or where it ended.
    pub phase: String,
    /// The slot's number, 1-based.
    pub slot: u32,
    pub prepared: bool,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub last_poll_ms: u64,
    pub code: i32,
    pub signal: i32,
    pub timed_out: bool,
    pub build_s: u64,
    pub run_s: u64,
    /// Why it was cancelled or failed.
    pub why: String,
}

impl Job {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id, "project": self.project, "sha": self.sha, "coord": self.coord,
            "build": self.build, "command": self.command, "results_file": self.results_file,
            "env": self.env.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "limit_s": self.limit_s, "lease_s": self.lease_s,
            "base": self.base.as_ref().map(|(n, s)| json!({"name": n, "sha": s})),
            "state": self.state.name(), "phase": self.phase, "slot": self.slot,
            "prepared": self.prepared, "started_ms": self.started_ms, "ended_ms": self.ended_ms,
            "last_poll_ms": self.last_poll_ms, "code": self.code, "signal": self.signal,
            "timed_out": self.timed_out, "build_s": self.build_s, "run_s": self.run_s,
            "why": self.why,
        })
    }

    fn from_json(v: &Value) -> Option<Job> {
        let s = |k: &str| v[k].as_str().map(String::from);
        let n = |k: &str| v[k].as_u64().unwrap_or(0);
        Some(Job {
            id: s("id")?,
            project: s("project")?,
            sha: s("sha")?,
            coord: s("coord")?,
            build: s("build"),
            command: s("command")?,
            results_file: s("results_file"),
            env: v["env"]
                .as_array()?
                .iter()
                .filter_map(|e| Some((e[0].as_str()?.to_string(), e[1].as_str()?.to_string())))
                .collect(),
            limit_s: n("limit_s"),
            lease_s: n("lease_s"),
            base: parse_base(&v["base"]),
            state: JobState::parse(v["state"].as_str().unwrap_or("")),
            phase: s("phase").unwrap_or_default(),
            slot: n("slot") as u32,
            prepared: v["prepared"].as_bool().unwrap_or(false),
            started_ms: n("started_ms"),
            ended_ms: n("ended_ms"),
            last_poll_ms: n("last_poll_ms"),
            code: v["code"].as_i64().unwrap_or(0) as i32,
            signal: v["signal"].as_i64().unwrap_or(0) as i32,
            timed_out: v["timed_out"].as_bool().unwrap_or(false),
            build_s: n("build_s"),
            run_s: n("run_s"),
            why: s("why").unwrap_or_default(),
        })
    }

    /// What `GET /node/jobs/{id}` says.
    pub fn view(&self) -> Value {
        json!({
            "id": self.id, "state": self.state.name(), "phase": self.phase,
            "started_ms": self.started_ms, "ended_ms": self.ended_ms,
            "code": self.code, "signal": self.signal, "timed_out": self.timed_out,
            "why": self.why, "slot": self.slot,
        })
    }
}

/// `{"name": "develop", "sha": <40 hex>}` as the base of a job; anything else is no base. A name
/// that could not be a branch is no base either (checked again with git where it is used).
fn parse_base(v: &Value) -> Option<(String, String)> {
    let name = v["name"].as_str()?;
    let sha = v["sha"].as_str()?;
    let plain = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('-')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'));
    (plain && valid_sha(sha)).then(|| (name.to_string(), sha.to_string()))
}

/// A job in memory: its record, and the flag that cancels it.
pub struct Handle {
    pub job: Mutex<Job>,
    pub cancel: AtomicBool,
}

/// Why a job could not be started.
#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    /// No room, or not accepting: the reason, in words.
    Busy(String),
    /// The commit is not in the mirror.
    Missing(String),
    /// The request is not a job.
    Bad(String),
    Io(String),
}

/// What a request to start a job carries.
pub struct StartReq {
    pub id: String,
    pub project: String,
    pub sha: String,
    pub build: Option<String>,
    pub command: String,
    pub results_file: Option<String>,
    pub env: Vec<(String, String)>,
    pub limit_s: u64,
    pub lease_s: u64,
    /// See [`Job::base`].
    pub base: Option<(String, String)>,
}

impl StartReq {
    /// Read and check a `POST /node/jobs` body. The environment keeps only `MASON_*` names made
    /// of letters, digits and `_`, whatever was sent.
    pub fn parse(b: &Value) -> Result<StartReq, String> {
        let id = b["id"].as_str().unwrap_or_default().to_string();
        let project = b["project"].as_str().unwrap_or_default().to_string();
        let sha = b["sha"].as_str().unwrap_or_default().to_string();
        if !safe_name(&id) {
            return Err("`id` must be letters, digits, `.`, `_` and `-` (at most 120)".into());
        }
        if !safe_name(&project) {
            return Err("`project` must be letters, digits, `.`, `_` and `-`".into());
        }
        if !valid_sha(&sha) {
            return Err("`sha` must be a full commit id".into());
        }
        let run = &b["run"];
        let command = run["command"].as_str().unwrap_or_default().to_string();
        if command.trim().is_empty() {
            return Err("the run has no `command`".into());
        }
        let env = run["env"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter(|(k, _)| {
                        k.starts_with("MASON_")
                            && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    })
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        Ok(StartReq {
            id,
            project,
            sha,
            build: run["build"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from),
            command,
            results_file: run["results_file"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from),
            env,
            limit_s: b["limit_s"].as_u64().unwrap_or(1800).clamp(10, 24 * 3600),
            lease_s: b["lease_s"].as_u64().unwrap_or(300),
            base: parse_base(&b["base"]),
        })
    }
}

/// The node's jobs.
pub struct Jobs {
    root: PathBuf,
    live: Mutex<BTreeMap<String, Arc<Handle>>>,
    busy: Mutex<BTreeSet<(String, String, u32)>>,
}

fn key(coord: &str, id: &str) -> String {
    format!("{coord}/{id}")
}

/// The length of a [`host_key`].
pub const HOST_KEY_LEN: usize = 16;

/// The file in a node host's directory that says which job and phase it is for.
const HOST_NAME_FILE: &str = "name";

/// What a node host is called for people: which coordinator's job, and which phase of it.
fn host_name(coord: &str, id: &str, phase: &str) -> String {
    format!("{coord}-{id}-{phase}")
}

/// The name of the directory of a node's step host: the first [`HOST_KEY_LEN`] hex digits of
/// SHA-256 over the coordinator, the job id and the phase, each ended by a zero byte but the
/// last. Bounded, whatever the ids are.
pub fn host_key(coord: &str, id: &str, phase: &str) -> String {
    let mut k = crate::lan::sha256_hex(format!("{coord}\0{id}\0{phase}").as_bytes());
    k.truncate(HOST_KEY_LEN);
    k
}

/// The socket of a step host of the node whose data directory is `data_dir`. Every one has the
/// same length, so this also stands for the longest.
pub fn host_socket(data_dir: &Path, key: &str) -> PathBuf {
    Jobs::new(data_dir).hosts_dir().join(key).join("sock")
}

impl Jobs {
    pub fn new(data_dir: &Path) -> Jobs {
        Jobs {
            root: data_dir.join("node"),
            live: Mutex::new(BTreeMap::new()),
            busy: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn jobs_dir(&self, coord: &str) -> PathBuf {
        self.root.join("jobs").join(coord)
    }

    fn job_dir(&self, coord: &str, id: &str) -> PathBuf {
        self.jobs_dir(coord).join(id)
    }

    pub fn mirror_path(&self, coord: &str, project: &str) -> PathBuf {
        self.root
            .join("mirrors")
            .join(coord)
            .join(format!("{project}.git"))
    }

    fn slot_dir(&self, coord: &str, project: &str, k: u32) -> PathBuf {
        self.root
            .join("work")
            .join(coord)
            .join(project)
            .join(format!("{project}-slot-{k}"))
    }

    fn hosts_dir(&self) -> PathBuf {
        self.root.join("hosts")
    }

    /// The directory of the step host of one phase of a job. Its name is [`host_key`], so that
    /// the socket path under it does not grow with the job's id. A directory with the readable
    /// name an older node gave it is still used, so that a restart onto this build finds it.
    fn host_dir(&self, coord: &str, id: &str, phase: &str) -> PathBuf {
        let hosts = self.hosts_dir();
        let old = hosts.join(host_name(coord, id, phase));
        if old.join("host.kv").exists() {
            return old;
        }
        hosts.join(host_key(coord, id, phase))
    }

    fn lock_live(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<Handle>>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Jobs that are running now, from every coordinator.
    pub fn running(&self) -> u32 {
        self.lock_live()
            .values()
            .filter(|h| h.job.lock().is_ok_and(|j| j.state == JobState::Running))
            .count() as u32
    }

    /// The jobs the node knows, for `mason node jobs`.
    pub fn list(&self) -> Vec<Value> {
        self.lock_live()
            .values()
            .filter_map(|h| h.job.lock().ok().map(|j| j.view()))
            .collect()
    }

    pub fn handle(&self, coord: &str, id: &str) -> Option<Arc<Handle>> {
        if let Some(h) = self.lock_live().get(&key(coord, id)) {
            return Some(h.clone());
        }
        // A job kept on disk after a restart, finished and not yet collected.
        let text = std::fs::read_to_string(self.job_dir(coord, id).join("job.json")).ok()?;
        let job = Job::from_json(&serde_json::from_str(&text).ok()?)?;
        let h = Arc::new(Handle {
            job: Mutex::new(job),
            cancel: AtomicBool::new(false),
        });
        self.lock_live().insert(key(coord, id), h.clone());
        Some(h)
    }

    fn persist(&self, job: &Job) {
        let dir = self.job_dir(&job.coord, &job.id);
        let _ = std::fs::create_dir_all(&dir);
        let tmp = dir.join("job.json.tmp");
        if std::fs::write(&tmp, job.to_json().to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("job.json"));
        }
    }

    /// Which commits of `shas` the mirror of `project` holds.
    pub fn has(&self, coord: &str, project: &str, shas: &[String]) -> Vec<String> {
        let m = self.mirror_path(coord, project);
        if !m.join("HEAD").exists() {
            return vec![];
        }
        shas.iter()
            .filter(|s| {
                valid_sha(s) && git::ok(&m, &["cat-file", "-e", &format!("{s}^{{commit}}")])
            })
            .cloned()
            .collect()
    }

    /// Add what a bundle carries to the mirror of `project`. The bundle is checked first: one
    /// whose prerequisites the mirror lacks is refused with the commits it wants.
    pub fn add_bundle(&self, coord: &str, project: &str, file: &Path) -> Result<(), String> {
        let m = self.mirror_path(coord, project);
        if !m.join("HEAD").exists() {
            std::fs::create_dir_all(&m).map_err(|e| e.to_string())?;
            git::run(&m, &["init", "-q", "--bare"]).map_err(|e| e.to_string())?;
        }
        let f = file.to_string_lossy().into_owned();
        git::run(&m, &["bundle", "verify", &f]).map_err(|e| e.to_string())?;
        git::run(
            &m,
            &[
                "fetch",
                "-q",
                "--no-tags",
                &f,
                "+refs/mason/node/*:refs/mason/node/*",
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The share of the whole machine's CPU, in percent, that the running jobs' process trees
    /// use, from `ps`.
    pub fn cpu_pct(&self, cores: u32) -> u32 {
        let roots: Vec<u32> = self
            .lock_live()
            .values()
            .filter_map(|h| {
                let j = h.job.lock().ok()?;
                (j.state == JobState::Running).then(|| self.host_dir(&j.coord, &j.id, &j.phase))
            })
            .filter_map(|dir| {
                mason_step_host::registry::HostInfo::read(&dir)
                    .ok()
                    .map(|i| i.child_pid)
            })
            .filter(|p| *p > 0)
            .collect();
        if roots.is_empty() {
            return 0;
        }
        let Ok(o) = std::process::Command::new("/bin/ps")
            .args(["-axo", "pid=,ppid=,pcpu="])
            .output()
        else {
            return 0;
        };
        let used = tree_cpu(&String::from_utf8_lossy(&o.stdout), &roots);
        ((used / cores.max(1) as f32).round() as u32).min(100)
    }

    /// The projects of `coord` whose mirror this node holds, with how many slots it has made:
    /// what makes a node "warm" for a project.
    pub fn warm(&self, coord: &str) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        let Ok(rd) = std::fs::read_dir(self.root.join("mirrors").join(coord)) else {
            return out;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(project) = name.strip_suffix(".git") else {
                continue;
            };
            let slots = std::fs::read_dir(self.root.join("work").join(coord).join(project))
                .map(|r| r.flatten().count())
                .unwrap_or(0);
            out.insert(project.to_string(), json!({"slots": slots}));
        }
        out
    }

    /// Start a job, or return the one with this id already (a coordinator asking again after a
    /// restart). `readings` says whether the node can take it.
    pub fn start(
        self: &Arc<Self>,
        d: &Arc<Daemon>,
        coord: &str,
        req: StartReq,
        settings: &NodeSettings,
        no_room: &dyn Fn(u32) -> Option<String>,
    ) -> Result<Arc<Handle>, StartError> {
        if let Some(h) = self.handle(coord, &req.id) {
            return Ok(h);
        }
        if let Some(why) = no_room(self.running()) {
            return Err(StartError::Busy(why));
        }
        let mirror = self.mirror_path(coord, &req.project);
        if !git::ok(
            &mirror,
            &["cat-file", "-e", &format!("{}^{{commit}}", req.sha)],
        ) {
            return Err(StartError::Missing(req.sha));
        }
        let slot = self
            .take_slot(coord, &req.project, settings.max_jobs)
            .ok_or_else(|| {
                StartError::Busy(format!("all {} job slots are in use", settings.max_jobs))
            })?;
        let now = now_ms();
        let job = Job {
            id: req.id,
            project: req.project,
            sha: req.sha,
            coord: coord.to_string(),
            build: req.build,
            command: req.command,
            results_file: req.results_file,
            env: req.env,
            limit_s: req.limit_s,
            lease_s: req.lease_s.min(settings.lease_s).max(10),
            base: req.base,
            state: JobState::Running,
            phase: "setup".into(),
            slot,
            prepared: false,
            started_ms: now,
            ended_ms: 0,
            last_poll_ms: now,
            code: 0,
            signal: 0,
            timed_out: false,
            build_s: 0,
            run_s: 0,
            why: String::new(),
        };
        self.persist(&job);
        let h = Arc::new(Handle {
            job: Mutex::new(job),
            cancel: AtomicBool::new(false),
        });
        self.lock_live().insert(
            key(
                coord,
                &h.job.lock().map_err(|_| StartError::Io("lock".into()))?.id,
            ),
            h.clone(),
        );
        self.spawn(d, &h);
        Ok(h)
    }

    /// The least recently used slot of `project` that is free, up to `max` slots.
    fn take_slot(&self, coord: &str, project: &str, max: u32) -> Option<u32> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        let taken: BTreeSet<u32> = busy
            .iter()
            .filter(|(c, p, _)| c == coord && p == project)
            .map(|(_, _, k)| *k)
            .collect();
        let used_at = |k: u32| {
            std::fs::metadata(self.slot_dir(coord, project, k))
                .and_then(|m| m.modified())
                .ok()
        };
        let k = pick_slot(&taken, max, used_at)?;
        busy.insert((coord.to_string(), project.to_string(), k));
        Some(k)
    }

    /// Is `job` the only job of its project (of its coordinator) holding a slot just now?
    fn alone(&self, job: &Job) -> bool {
        self.busy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(c, p, _)| *c == job.coord && *p == job.project)
            .count()
            <= 1
    }

    fn free_slot(&self, job: &Job) {
        self.busy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(job.coord.clone(), job.project.clone(), job.slot));
    }

    /// Run the job on a thread of its own.
    fn spawn(self: &Arc<Self>, d: &Arc<Daemon>, h: &Arc<Handle>) {
        let (jobs, d, h) = (self.clone(), d.clone(), h.clone());
        let _ = std::thread::Builder::new()
            .name("node-job".into())
            .spawn(move || {
                drive(&jobs, &d, &h);
                let j = h.job.lock().map(|j| j.clone());
                if let Ok(j) = j {
                    jobs.free_slot(&j);
                }
            });
    }

    /// A job's output so far, from `from`, at most [`LOG_CHUNK`] bytes: the build's, then the
    /// command's.
    pub fn log(&self, coord: &str, id: &str, from: u64) -> Vec<u8> {
        let dir = self.job_dir(coord, id);
        let mut out = Vec::new();
        let mut at = 0u64;
        for part in ["build.log", "run.log"] {
            let Ok(mut f) = std::fs::File::open(dir.join(part)) else {
                continue;
            };
            let len = f.metadata().map_or(0, |m| m.len());
            let (start, end) = (at, at + len);
            at = end;
            if from >= end || out.len() >= LOG_CHUNK {
                continue;
            }
            use std::io::{Read, Seek, SeekFrom};
            let skip = from.saturating_sub(start);
            if f.seek(SeekFrom::Start(skip)).is_err() {
                continue;
            }
            let want = (len - skip).min((LOG_CHUNK - out.len()) as u64);
            let mut buf = vec![0u8; want as usize];
            if f.read_exact(&mut buf).is_ok() {
                out.extend(buf);
            }
        }
        out
    }

    /// What a finished job leaves: `result.json` and the results files, for the coordinator.
    pub fn artifact(&self, coord: &str, id: &str) -> Result<Value, String> {
        let h = self.handle(coord, id).ok_or("no such job")?;
        let j = h.job.lock().map_err(|_| "lock")?.clone();
        if j.state == JobState::Running {
            return Err("the job is still running".into());
        }
        let dir = self.job_dir(coord, id);
        let mut files = vec![];
        let mut total = 0u64;
        let root = dir.join("results");
        let mut stack = vec![PathBuf::new()];
        while let Some(rel) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(root.join(&rel)) else {
                continue;
            };
            for e in rd.flatten() {
                let r = rel.join(e.file_name());
                let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                    continue;
                };
                if m.is_dir() {
                    stack.push(r);
                } else if m.is_file() {
                    total += m.len();
                    if total > ARTIFACT_MAX {
                        return Err("the results files are too large".into());
                    }
                    if let Ok(b) = std::fs::read(e.path()) {
                        files.push(json!({
                            "path": r.to_string_lossy(),
                            "b64": base64_encode(&b),
                        }));
                    }
                }
            }
        }
        Ok(json!({
            "result": {
                "code": j.code, "signal": j.signal, "build_s": j.build_s, "run_s": j.run_s,
                "phase": j.phase, "timed_out": j.timed_out, "state": j.state.name(), "why": j.why,
                "started_ms": j.started_ms, "ended_ms": j.ended_ms,
            },
            "results": files,
        }))
    }

    /// Cancel a job: its commands are stopped, and it ends as cancelled.
    pub fn cancel(&self, coord: &str, id: &str, why: &str) -> bool {
        let Some(h) = self.handle(coord, id) else {
            return false;
        };
        if let Ok(mut j) = h.job.lock() {
            if j.state == JobState::Running && j.why.is_empty() {
                j.why = why.to_string();
            }
        }
        h.cancel.store(true, Ordering::SeqCst);
        true
    }

    /// Cancel every running job (the owner needs the machine back now).
    pub fn cancel_all(&self, why: &str) -> u32 {
        let live: Vec<Arc<Handle>> = self.lock_live().values().cloned().collect();
        let mut n = 0;
        for h in live {
            let running = h.job.lock().is_ok_and(|j| j.state == JobState::Running);
            if running {
                if let Ok(mut j) = h.job.lock() {
                    if j.why.is_empty() {
                        j.why = why.to_string();
                    }
                }
                h.cancel.store(true, Ordering::SeqCst);
                n += 1;
            }
        }
        n
    }

    /// The coordinator has the result: the job's directory goes.
    pub fn ack(&self, coord: &str, id: &str) -> bool {
        let Some(h) = self.handle(coord, id) else {
            return false;
        };
        if h.job.lock().is_ok_and(|j| j.state == JobState::Running) {
            return false;
        }
        self.lock_live().remove(&key(coord, id));
        let _ = std::fs::remove_dir_all(self.job_dir(coord, id));
        true
    }

    /// Note that the coordinator asked after a job: its lease starts again.
    pub fn renew(&self, h: &Handle) {
        if let Ok(mut j) = h.job.lock() {
            j.last_poll_ms = now_ms();
        }
    }

    /// One pass of upkeep: a job whose coordinator has not asked for `lease_s` is cancelled, and
    /// a job that ended a day ago and was never collected is deleted.
    pub fn tick(&self, now: u64) {
        let live: Vec<Arc<Handle>> = self.lock_live().values().cloned().collect();
        for h in live {
            let Ok(j) = h.job.lock().map(|j| j.clone()) else {
                continue;
            };
            match j.state {
                JobState::Running => {
                    if lease_expired(j.last_poll_ms, j.lease_s, now) {
                        self.cancel(
                            &j.coord,
                            &j.id,
                            "its coordinator stopped asking after it (lease expired)",
                        );
                    }
                }
                _ => {
                    if now.saturating_sub(j.ended_ms) > 24 * 3600 * 1000 {
                        self.ack(&j.coord, &j.id);
                    }
                }
            }
        }
    }

    /// Take up the jobs a restart found running: each one's commands are still under their step
    /// hosts, so the thread of each only collects what they say.
    pub fn resume(self: &Arc<Self>, d: &Arc<Daemon>) {
        let Ok(coords) = std::fs::read_dir(self.root.join("jobs")) else {
            return;
        };
        for c in coords.flatten() {
            let Ok(jobs) = std::fs::read_dir(c.path()) else {
                continue;
            };
            for e in jobs.flatten() {
                let Ok(text) = std::fs::read_to_string(e.path().join("job.json")) else {
                    continue;
                };
                let Some(mut job) = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| Job::from_json(&v))
                else {
                    continue;
                };
                if job.state != JobState::Running
                    || self.lock_live().contains_key(&key(&job.coord, &job.id))
                {
                    continue;
                }
                // Its coordinator has the full lease from now.
                job.last_poll_ms = now_ms();
                self.busy.lock().unwrap_or_else(|e| e.into_inner()).insert((
                    job.coord.clone(),
                    job.project.clone(),
                    job.slot,
                ));
                let h = Arc::new(Handle {
                    job: Mutex::new(job.clone()),
                    cancel: AtomicBool::new(false),
                });
                self.lock_live().insert(key(&job.coord, &job.id), h.clone());
                self.spawn(d, &h);
            }
        }
    }
}

/// The CPU, in percent of one core, of the processes in `roots` and everything under them, from
/// `ps -axo pid=,ppid=,pcpu=`. Pure.
pub fn tree_cpu(ps: &str, roots: &[u32]) -> f32 {
    let rows: Vec<(u32, u32, f32)> = ps
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some((
                w.next()?.parse().ok()?,
                w.next()?.parse().ok()?,
                w.next()?.parse().ok()?,
            ))
        })
        .collect();
    let mut in_tree: BTreeSet<u32> = roots.iter().copied().collect();
    loop {
        let before = in_tree.len();
        for (pid, ppid, _) in &rows {
            if in_tree.contains(ppid) {
                in_tree.insert(*pid);
            }
        }
        if in_tree.len() == before {
            break;
        }
    }
    rows.iter()
        .filter(|(pid, _, _)| in_tree.contains(pid))
        .map(|(_, _, c)| *c)
        .sum()
}

/// Has a job's lease run out: nobody has asked after it for `lease_s` seconds?
pub fn lease_expired(last_poll_ms: u64, lease_s: u64, now: u64) -> bool {
    now.saturating_sub(last_poll_ms) > lease_s.saturating_mul(1000)
}

/// The slot to use: of slots `1..=max` the ones not in `taken`, the one used longest ago (a slot
/// never made counts as oldest), then the lowest number. Pure.
pub fn pick_slot(
    taken: &BTreeSet<u32>,
    max: u32,
    used_at: impl Fn(u32) -> Option<std::time::SystemTime>,
) -> Option<u32> {
    (1..=max)
        .filter(|k| !taken.contains(k))
        .min_by_key(|k| (used_at(*k), *k))
}

// ---- the checkout ---------------------------------------------------------------------------

/// Make the slot hold exactly `sha`: a worktree of the mirror, reused between jobs, cleaned of
/// everything but the build output directories.
pub fn prepare_slot(mirror: &Path, slot: &Path, sha: &str) -> Result<(), String> {
    if let Some(parent) = slot.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let s = slot.to_string_lossy().into_owned();
    if slot.join(".git").exists() {
        git::run(slot, &["checkout", "-q", "--detach", "-f", sha]).map_err(|e| e.to_string())?;
        let mut args = vec!["clean", "-ffdxq"];
        let keep: Vec<String> = KEEP.iter().map(|k| format!("/{k}")).collect();
        for k in &keep {
            args.push("-e");
            args.push(k);
        }
        git::run(slot, &args).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(slot);
    let _ = git::run(mirror, &["worktree", "prune"]);
    git::run(
        mirror,
        &["worktree", "add", "-q", "--detach", "--force", &s, sha],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Make `origin/<name>` resolve in the slots of this mirror: `refs/remotes/origin/<name>` is set to
/// `tip`, when the mirror has that commit (a coordinator that sends a base always sends it too).
///
/// The slots are worktrees of one mirror, and `refs/remotes` is shared by all of them, so a ref
/// cannot be set per slot. What keeps concurrent jobs of a project safe: with `exclusive` (no
/// other job of the project holds a slot) the ref is set to `tip` exactly, so the job sees its own
/// coordinator's base, whatever the ref was (a force-pushed base included); otherwise it only
/// moves forward, to a `tip` that descends from the ref's commit, so a running job never has its
/// base taken back to an older or unrelated commit, and may see a newer one a concurrent job sent.
/// The ref also keeps the base's commits from being collected.
pub fn set_origin_base(
    mirror: &Path,
    name: &str,
    tip: &str,
    exclusive: bool,
) -> Result<(), String> {
    let r = format!("refs/remotes/origin/{name}");
    git::run(mirror, &["check-ref-format", &r]).map_err(|e| e.to_string())?;
    if !valid_sha(tip) || !git::ok(mirror, &["cat-file", "-e", &format!("{tip}^{{commit}}")]) {
        return Err(format!("the mirror lacks the base commit {tip}"));
    }
    let current = git::run(
        mirror,
        &["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")],
    )
    .ok()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty());
    let advance = match &current {
        None => true,
        Some(c) if c == tip => false,
        Some(_) if exclusive => true,
        Some(c) => git::ok(mirror, &["merge-base", "--is-ancestor", c, tip]),
    };
    if advance {
        git::run(mirror, &["update-ref", &r, tip]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---- the environment --------------------------------------------------------------------------

/// The shims a node puts first on `PATH`: nothing a job does may appear on the screen. `caffeinate`
/// loses its `-u` (which wakes the display) and `xcodebuild` refuses a test run that selects a UI
/// test bundle (the "Automation Running" overlay).
pub fn write_shims(dir: &Path, path: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir)?;
        let find = |name: &str| {
            std::env::split_paths(path)
                .filter(|d| d.is_absolute() && d != dir)
                .map(|d| d.join(name))
                .find(|p| {
                    p.metadata()
                        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                })
        };
        for (tool, body) in [("caffeinate", CAFFEINATE), ("xcodebuild", XCODEBUILD)] {
            let Some(real) = find(tool) else {
                let _ = std::fs::remove_file(dir.join(tool));
                continue;
            };
            let text = format!(
                "#!/bin/sh\n# SPDX-License-Identifier: GPL-3.0-or-later\n# Written by masond for a node's jobs.\nreal='{}'\n{body}",
                real.to_string_lossy().replace('\'', r"'\''")
            );
            let file = dir.join(tool);
            if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
                let tmp = dir.join(format!(".{tool}.{}.tmp", std::process::id()));
                std::fs::write(&tmp, text)?;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
                std::fs::rename(&tmp, &file)?;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, path);
    }
    Ok(())
}

/// The `mason` command line a node's jobs get: the one in the directory of the running daemon (the
/// runtime that has `masond` and `step-host`), or the file `MASON_NODE_CLI` names (for tests).
pub fn node_cli() -> Option<PathBuf> {
    let p = std::env::var_os("MASON_NODE_CLI")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|e| e.with_file_name("mason"))
        })?;
    p.is_file().then_some(p)
}

/// Put `mason` in the shims directory, as a script that runs `target` (or remove it when there is
/// none): so a suite can run `mason definition check` on a node. It is a script rather than a
/// link so that it can never be mistaken for a file of the job's checkout, and is rewritten each
/// job so that it follows a runtime update.
pub fn write_cli_shim(dir: &Path, target: Option<&Path>) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = dir.join("mason");
        let Some(target) = target else {
            let _ = std::fs::remove_file(&file);
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let text = format!(
            "#!/bin/sh\n# SPDX-License-Identifier: GPL-3.0-or-later\n# Written by masond for a node's jobs.\nexec '{}' \"$@\"\n",
            target.to_string_lossy().replace('\'', r"'\''")
        );
        if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
            let tmp = dir.join(format!(".mason.{}.tmp", std::process::id()));
            std::fs::write(&tmp, text)?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            std::fs::rename(&tmp, &file)?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, target);
    }
    Ok(())
}

#[cfg(unix)]
const CAFFEINATE: &str = r#"# `-u` declares the user active and turns a sleeping display on: never on a node. Options are read
# up to the first word that is not one (the command to run), skipping the value of -t and -w.
n=$#; rest=0; skip=0
while [ "$n" -gt 0 ]; do
  a=$1; shift; n=$((n-1))
  if [ "$rest" = 1 ] || [ "$skip" = 1 ]; then skip=0; set -- "$@" "$a"; continue; fi
  case "$a" in
    --) rest=1; set -- "$@" "$a" ;;
    -*) c=$(printf '%s' "${a#-}" | tr -d u)
        if [ -n "$c" ]; then set -- "$@" "-$c"; fi
        case "$c" in *t|*w) skip=1 ;; esac ;;
    *) rest=1; set -- "$@" "$a" ;;
  esac
done
exec "$real" "$@"
"#;

#[cfg(unix)]
const XCODEBUILD: &str = r#"# A UI test bundle drives the screen ("Automation Running"): refuse a run that selects one.
for a in "$@"; do
  case "$a" in
    -skip-testing:*) ;;
    -only-testing:*UITests*|-only-testing:*UITest*) echo "a node does not run UI tests: they would appear on the screen" >&2; exit 69 ;;
  esac
done
exec "$real" "$@"
"#;

/// The environment a job's commands run in: built here, from nothing. The node's own base (its
/// `PATH` with the shims first, `HOME`, `TMPDIR` and the toolchain homes), then the coordinator's
/// `MASON_*` facts, then what a node always sets. Path-valued variables of the coordinator's
/// (`MASON_TASK_CACHE`, `MASON_WORKTREE`, `MASON_RESULT_FILE`, `MASON_PROGRESS_FILE`) name disks
/// this machine does not have: they are dropped, and `MASON_WORKTREE` is this slot.
pub fn job_env(
    job: &Job,
    slot: &Path,
    shims: &Path,
    base: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    const DROP: [&str; 4] = [
        "MASON_TASK_CACHE",
        "MASON_WORKTREE",
        "MASON_RESULT_FILE",
        "MASON_PROGRESS_FILE",
    ];
    let mut env: Vec<(String, String)> = vec![];
    let mut put = |k: &str, v: String| {
        env.retain(|(n, _)| n != k);
        env.push((k.to_string(), v));
    };
    let home = base("HOME").unwrap_or_default();
    let mut path: Vec<String> = vec![shims.to_string_lossy().into_owned()];
    if !home.is_empty() {
        path.push(format!("{home}/.cargo/bin"));
    }
    for p in base("PATH")
        .unwrap_or_default()
        .split(':')
        .map(String::from)
        .chain(
            [
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/sbin",
            ]
            .map(String::from),
        )
    {
        if !p.is_empty() && !path.contains(&p) {
            path.push(p);
        }
    }
    put("PATH", path.join(":"));
    for k in [
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TMPDIR",
        "LANG",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "DEVELOPER_DIR",
        "JAVA_HOME",
    ] {
        if let Some(v) = base(k) {
            put(k, v);
        }
    }
    for (k, v) in &job.env {
        if k.starts_with("MASON_") && !DROP.contains(&k.as_str()) {
            put(k, v.clone());
        }
    }
    put("MASON_WORKTREE", slot.to_string_lossy().into_owned());
    put("MASON_NODE", "1".into());
    // The `mason` on a job's PATH finds no daemon and no configuration of this node: the commands
    // that need one say masond is not running, while `mason definition check` (which reads files)
    // works. The directories are named and never made.
    put(
        "MASON_DATA_DIR",
        shims
            .with_file_name("no-daemon")
            .to_string_lossy()
            .into_owned(),
    );
    put(
        "MASON_CONFIG_DIR",
        shims
            .with_file_name("no-config")
            .to_string_lossy()
            .into_owned(),
    );
    put("MASON_PROJECT", job.project.clone());
    // Set so that everything that treats a pipeline run differently (the shims above, the test
    // mode of a project's scripts) does here; the coordinator's own number is kept when sent.
    if !job.env.iter().any(|(k, _)| k == "MASON_TASK") {
        put("MASON_TASK", "node".into());
    }
    put("CARGO_TERM_COLOR", "never".into());
    put("NO_COLOR", "1".into());
    put("GIT_TERMINAL_PROMPT", "0".into());
    env
}

// ---- running ----------------------------------------------------------------------------------

fn finish(jobs: &Jobs, h: &Handle, f: impl FnOnce(&mut Job)) {
    if let Ok(mut j) = h.job.lock() {
        f(&mut j);
        j.ended_ms = now_ms();
        jobs.persist(&j);
    }
}

/// What a phase did.
struct Phase {
    code: i32,
    signal: i32,
    secs: u64,
    timed_out: bool,
    cancelled: bool,
}

/// One command under a step host, stopped at the deadline, on cancel, or when its lease runs out.
#[allow(clippy::too_many_arguments)]
fn run_phase(
    jobs: &Jobs,
    d: &Arc<Daemon>,
    h: &Arc<Handle>,
    name: &str,
    cmd: &str,
    slot: &Path,
    env: Vec<(String, String)>,
    deadline: Instant,
) -> Result<Phase, String> {
    let (coord, id) = {
        let j = h.job.lock().map_err(|_| "lock")?;
        (j.coord.clone(), j.id.clone())
    };
    let hosts = jobs.hosts_dir();
    let dir = jobs.host_dir(&coord, &id, name);
    if !dir.join("host.kv").exists() {
        let host_id = host_key(&coord, &id, name);
        std::fs::create_dir_all(&dir)
            .and_then(|()| {
                std::fs::write(
                    dir.join(HOST_NAME_FILE),
                    format!("{}\n", host_name(&coord, &id, name)),
                )
            })
            .map_err(|e| format!("cannot start the {name}: {}: {e}", dir.display()))?;
        let o = SpawnOptions {
            host_bin: d.cfg.host_bin.clone(),
            root: hosts.clone(),
            id: host_id,
            cmd: vec!["sh".into(), "-c".into(), cmd.to_string()],
            cwd: Some(slot.to_path_buf()),
            env,
            clear_env: true,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: d.cfg.heartbeat,
        };
        spawn_host(&o).map_err(|e| format!("cannot start the {name}: {e}"))?;
    }
    let done = Arc::new(AtomicBool::new(false));
    let why = Arc::new(std::sync::atomic::AtomicU8::new(0)); // 1 timed out, 2 cancelled
    {
        let (done, why, h, sock) = (done.clone(), why.clone(), h.clone(), dir.join("sock"));
        std::thread::spawn(move || loop {
            if done.load(Ordering::SeqCst) {
                return;
            }
            let code = if Instant::now() >= deadline {
                1
            } else if h.cancel.load(Ordering::SeqCst) {
                2
            } else {
                0
            };
            if code != 0 {
                why.store(code, Ordering::SeqCst);
                if let Ok(mut c) = HostClient::connect(&sock, PREVIOUS, CURRENT, "node") {
                    let _ = c.stop(1000);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        });
    }
    let jdir = jobs.job_dir(&coord, &id);
    let log = jdir.join(format!("{name}.log"));
    let exit = jdir.join(format!("{name}.exit"));
    let rec = collect_to_file(&dir, &log, &exit);
    done.store(true, Ordering::SeqCst);
    let rec = match rec {
        Ok(r) => r,
        Err(ClientError::HostLost { .. }) => {
            return Err(format!("the {name} was lost (its host died)"));
        }
        Err(e) => return Err(format!("cannot collect the {name}: {e}")),
    };
    let secs = match (
        mason_step_host::registry::HostInfo::read(&dir),
        mason_step_host::registry::ExitRecord::read(&dir),
    ) {
        (Ok(i), Ok(e)) => e.ended_ms.saturating_sub(i.started_ms) / 1000,
        _ => 0,
    };
    Ok(Phase {
        code: rec.code,
        signal: rec.signal,
        secs,
        timed_out: why.load(Ordering::SeqCst) == 1,
        cancelled: why.load(Ordering::SeqCst) == 2,
    })
}

/// Run a job to its end. Safe to call again for a job a restart found: a step host that exists is
/// only collected, and a phase that ended is not run again.
fn drive(jobs: &Arc<Jobs>, d: &Arc<Daemon>, h: &Arc<Handle>) {
    let Ok(mut job) = h.job.lock().map(|j| j.clone()) else {
        return;
    };
    let slot = jobs.slot_dir(&job.coord, &job.project, job.slot);
    let mirror = jobs.mirror_path(&job.coord, &job.project);
    if !job.prepared {
        match prepare_slot(&mirror, &slot, &job.sha) {
            Ok(()) => {
                if let Some((name, tip)) = &job.base {
                    if let Err(e) = set_origin_base(&mirror, name, tip, jobs.alone(&job)) {
                        // The job still runs: only a command that reads `origin/<base>` fails.
                        eprintln!(
                            "node job {} of coordinator {}: no origin/{name}: {e}",
                            job.id, job.coord
                        );
                    }
                }
                job.prepared = true;
                job.phase = if job.build.is_some() { "build" } else { "run" }.into();
                if let Ok(mut j) = h.job.lock() {
                    *j = job.clone();
                    jobs.persist(&j);
                }
            }
            Err(e) => {
                finish(jobs, h, |j| {
                    j.state = JobState::Failed;
                    j.phase = "setup".into();
                    j.why = format!("could not check the commit out: {e}");
                });
                return;
            }
        }
    }
    let shims = jobs.root.join("shims");
    let base = |k: &str| std::env::var(k).ok();
    let path = base("PATH").unwrap_or_default();
    let _ = write_shims(&shims, &path);
    let _ = write_cli_shim(&shims, node_cli().as_deref());
    let env = job_env(&job, &slot, &shims, &base);
    let deadline = Instant::now()
        + Duration::from_millis((job.started_ms + job.limit_s * 1000).saturating_sub(now_ms()));
    let mut result: Option<(i32, i32, bool, bool)> = None; // code, signal, timed_out, cancelled
    let mut ended_in = job.phase.clone();
    let phases: Vec<(&str, String)> = job
        .build
        .iter()
        .map(|b| ("build", b.clone()))
        .chain(std::iter::once(("run", job.command.clone())))
        .collect();
    for (name, cmd) in phases {
        // A phase a restart finds already over is not run again.
        let exit = jobs
            .job_dir(&job.coord, &job.id)
            .join(format!("{name}.exit"));
        let finished = exit.exists() && std::fs::read_to_string(&exit).is_ok_and(|t| !t.is_empty());
        if finished && name == "build" && job.phase == "run" {
            continue;
        }
        ended_in = name.to_string();
        if let Ok(mut j) = h.job.lock() {
            j.phase = name.to_string();
            jobs.persist(&j);
        }
        match run_phase(jobs, d, h, name, &cmd, &slot, env.clone(), deadline) {
            Ok(p) => {
                finish_phase(jobs, h, name, p.secs);
                let bad = p.code != 0 || p.signal != 0 || p.timed_out || p.cancelled;
                result = Some((p.code, p.signal, p.timed_out, p.cancelled));
                if bad {
                    break;
                }
            }
            Err(e) => {
                eprintln!(
                    "node job {} of coordinator {} failed: {e}",
                    job.id, job.coord
                );
                finish(jobs, h, |j| {
                    j.state = JobState::Failed;
                    j.why = e;
                });
                return;
            }
        }
    }
    let (code, signal, timed_out, cancelled) = result.unwrap_or((0, 0, false, false));
    // The results file(s) the suite reads, with their paths, as the workflow's `tar` does.
    if let Some(r) = &job.results_file {
        if !cancelled {
            let out = jobs.job_dir(&job.coord, &job.id).join("results");
            let _ = std::fs::create_dir_all(&out);
            let script = format!(
                "tar cf - {r} 2>/dev/null | tar xf - -C '{}'",
                out.to_string_lossy().replace('\'', r"'\''")
            );
            let _ = std::process::Command::new("sh")
                .args(["-c", &script])
                .current_dir(&slot)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .status();
        }
    }
    // The whole output in one file, the build's then the command's.
    let jdir = jobs.job_dir(&job.coord, &job.id);
    let mut all = std::fs::read(jdir.join("build.log")).unwrap_or_default();
    all.extend(std::fs::read(jdir.join("run.log")).unwrap_or_default());
    let _ = std::fs::write(jdir.join("output.log"), all);
    finish(jobs, h, |j| {
        j.code = code;
        j.signal = signal;
        j.timed_out = timed_out;
        j.phase = ended_in;
        if cancelled {
            j.state = JobState::Cancelled;
            if j.why.is_empty() {
                j.why = "cancelled".into();
            }
        } else {
            j.state = JobState::Done;
        }
    });
}

fn finish_phase(jobs: &Jobs, h: &Handle, name: &str, secs: u64) {
    if let Ok(mut j) = h.job.lock() {
        if name == "build" {
            j.build_s = secs;
        } else {
            j.run_s = secs;
        }
        jobs.persist(&j);
    }
}

// ---- base64 -----------------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = B64.iter().position(|x| *x == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The data directory `docs/nodes.md` sets a node up with.
    const DOCUMENTED: &str = "/Users/masonnode/Library/Application Support/mason";

    #[test]
    fn the_base_of_a_job_is_optional_and_a_bad_one_is_no_base() {
        let sha = "a".repeat(40);
        let body = |base: Value| json!({"id": "j", "project": "p", "sha": sha, "run": {"command": "true"}, "base": base});
        let r = StartReq::parse(&body(json!({"name": "release/1.0", "sha": sha}))).unwrap();
        assert_eq!(r.base, Some(("release/1.0".into(), sha.clone())));
        // No field (an older coordinator), or one that cannot be used: the job starts without.
        let old = json!({"id": "j", "project": "p", "sha": sha, "run": {"command": "true"}});
        assert_eq!(StartReq::parse(&old).unwrap().base, None);
        for bad in [
            json!({"name": "a..b", "sha": sha}),
            json!({"name": "-x", "sha": sha}),
            json!({"name": "", "sha": sha}),
            json!({"name": "ok", "sha": "zz"}),
            json!("develop"),
        ] {
            assert_eq!(
                StartReq::parse(&body(bad.clone())).unwrap().base,
                None,
                "{bad}"
            );
        }
        // A job record from before the field reads back with none.
        assert_eq!(parse_base(&Value::Null), None);
    }

    #[test]
    fn the_cli_shim_runs_its_target_and_goes_when_there_is_none() {
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("it's mason");
        std::fs::write(&target, "#!/bin/sh\necho \"cli $*\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
            let dir = t.path().join("shims");
            write_cli_shim(&dir, Some(&target)).unwrap();
            let o = std::process::Command::new(dir.join("mason"))
                .args(["a", "b"])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&o.stdout), "cli a b\n");
            write_cli_shim(&dir, None).unwrap();
            assert!(!dir.join("mason").exists());
        }
    }

    #[test]
    fn a_host_key_is_stable_and_differs_when_any_part_does() {
        let k = host_key("ad7507fdc188cdd0", "t1113r6691-check", "run");
        assert_eq!(k.len(), HOST_KEY_LEN);
        assert!(k.bytes().all(|c| c.is_ascii_hexdigit()), "{k}");
        assert_eq!(k, host_key("ad7507fdc188cdd0", "t1113r6691-check", "run"));
        for other in [
            host_key("ad7507fdc188cdd1", "t1113r6691-check", "run"),
            host_key("ad7507fdc188cdd0", "t1113r6692-check", "run"),
            host_key("ad7507fdc188cdd0", "t1113r6691-check", "build"),
            // The parts are kept apart: moving a character between two is another key.
            host_key("ad7507fdc188cdd0t", "1113r6691-check", "run"),
        ] {
            assert_ne!(k, other);
        }
    }

    #[test]
    fn a_long_job_id_fits_a_socket_path_under_the_documented_data_dir() {
        let id = "x".repeat(60);
        let key = host_key("ad7507fdc188cdd0", &id, "build");
        let sock = host_socket(Path::new(DOCUMENTED), &key);
        assert!(
            sock.as_os_str().len() <= mason_step_host::host::MAX_SOCK_PATH,
            "{}",
            sock.display()
        );
        assert!(mason_step_host::host::check_sock_path(&sock).is_ok());
    }

    #[test]
    fn a_host_an_older_node_named_is_found_again() {
        let t = tempfile::tempdir().unwrap();
        let jobs = Jobs::new(t.path());
        let hosts = t.path().join("node/hosts");
        let new = hosts.join(host_key("c", "j", "run"));
        assert_eq!(jobs.host_dir("c", "j", "run"), new);
        let old = hosts.join("c-j-run");
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(jobs.host_dir("c", "j", "run"), new, "no host came up there");
        std::fs::write(old.join("host.kv"), "id=c-j-run\n").unwrap();
        assert_eq!(jobs.host_dir("c", "j", "run"), old);
    }

    #[test]
    fn names_and_commits_are_checked() {
        for ok in ["a", "t12r3-check0", "b480-r1-lint", "x.y_z"] {
            assert!(safe_name(ok), "{ok}");
        }
        for bad in ["", ".hidden", "a/b", "../x", "a b", "a\n", &"x".repeat(121)] {
            assert!(!safe_name(bad), "{bad:?}");
        }
        assert!(valid_sha(&"a".repeat(40)) && valid_sha(&"0".repeat(64)));
        assert!(!valid_sha("abc") && !valid_sha(&"g".repeat(40)));
        assert_eq!(fp16(&"ab".repeat(32)), "abababababababab");
    }

    #[test]
    fn a_start_request_keeps_only_masons_own_variables() {
        let sha = "a".repeat(40);
        let b = json!({
            "id": "t1r1", "project": "mason", "sha": sha, "limit_s": 5, "lease_s": 99999,
            "run": {
                "command": "sh checks.sh a", "build": "sh build.sh a", "results_file": "r/*.xml",
                "env": {"MASON_X": "1", "PATH": "/evil", "HOME": "/x", "MASON_BAD-NAME": "1",
                         "CARGO_TARGET_DIR": "/coordinators/disk", "MASON_N": 3}
            }
        });
        let r = StartReq::parse(&b).unwrap();
        assert_eq!(r.env, vec![("MASON_X".to_string(), "1".to_string())]);
        assert_eq!(r.limit_s, 10, "a limit is at least 10 s");
        assert_eq!(r.build.as_deref(), Some("sh build.sh a"));
        assert_eq!(r.results_file.as_deref(), Some("r/*.xml"));
        for (bad, why) in [
            (
                json!({"id": "../x", "project": "p", "sha": sha, "run": {"command": "x"}}),
                "`id`",
            ),
            (
                json!({"id": "a", "project": "a/b", "sha": sha, "run": {"command": "x"}}),
                "`project`",
            ),
            (
                json!({"id": "a", "project": "p", "sha": "xyz", "run": {"command": "x"}}),
                "`sha`",
            ),
            (
                json!({"id": "a", "project": "p", "sha": sha, "run": {"command": " "}}),
                "`command`",
            ),
        ] {
            let e = StartReq::parse(&bad).err().expect("refused");
            assert!(e.contains(why), "{e}");
        }
    }

    #[test]
    fn a_job_round_trips_through_its_file() {
        let j = Job {
            id: "a".into(),
            project: "p".into(),
            sha: "b".repeat(40),
            coord: "c".into(),
            build: Some("make".into()),
            command: "sh t".into(),
            results_file: None,
            env: vec![("MASON_X".into(), "1".into())],
            limit_s: 60,
            lease_s: 30,
            base: None,
            state: JobState::Done,
            phase: "run".into(),
            slot: 2,
            prepared: true,
            started_ms: 5,
            ended_ms: 9,
            last_poll_ms: 7,
            code: 1,
            signal: 0,
            timed_out: false,
            build_s: 3,
            run_s: 4,
            why: String::new(),
        };
        let back = Job::from_json(&j.to_json()).unwrap();
        assert_eq!(back.to_json(), j.to_json());
        assert_eq!(back.state, JobState::Done);
    }

    #[test]
    fn the_least_recently_used_free_slot_is_taken() {
        use std::time::{Duration, UNIX_EPOCH};
        let at = |secs: u64| Some(UNIX_EPOCH + Duration::from_secs(secs));
        let times = |k: u32| match k {
            1 => at(300),
            2 => at(100),
            3 => None, // never made
            _ => at(200),
        };
        let none = BTreeSet::new();
        assert_eq!(
            pick_slot(&none, 4, times),
            Some(3),
            "a slot not made yet is oldest"
        );
        assert_eq!(pick_slot(&BTreeSet::from([3]), 4, times), Some(2));
        assert_eq!(pick_slot(&BTreeSet::from([2, 3]), 4, times), Some(4));
        assert_eq!(pick_slot(&BTreeSet::from([1, 2, 3, 4]), 4, times), None);
        assert_eq!(
            pick_slot(&none, 2, |_| None),
            Some(1),
            "ties go to the lowest number"
        );
        assert_eq!(pick_slot(&none, 0, times), None);
    }

    #[test]
    fn a_process_trees_cpu_is_the_roots_and_all_below_them() {
        let ps = "  10     1  5.0\n  11    10 20.0\n  12    11 30.5\n  20     1 99.0\n  21    20  1.0\n bad line\n";
        assert_eq!(tree_cpu(ps, &[10]), 55.5);
        assert_eq!(tree_cpu(ps, &[20, 10]), 155.5);
        assert_eq!(tree_cpu(ps, &[999]), 0.0);
        assert_eq!(tree_cpu("", &[10]), 0.0);
    }

    #[test]
    fn a_lease_runs_out_after_its_seconds_without_a_look() {
        assert!(!lease_expired(1_000, 300, 1_000 + 300_000));
        assert!(lease_expired(1_000, 300, 1_000 + 300_001));
        assert!(
            !lease_expired(5_000, 10, 1_000),
            "a clock that went back is not expiry"
        );
    }

    #[test]
    fn the_environment_is_built_again_and_drops_the_coordinators_paths() {
        let job = Job {
            id: "a".into(),
            project: "mason".into(),
            sha: "b".repeat(40),
            coord: "c".into(),
            build: None,
            command: "x".into(),
            results_file: None,
            env: vec![
                ("MASON_TASK".into(), "42".into()),
                ("MASON_TASK_CACHE".into(), "/coordinator/cache".into()),
                ("MASON_WORKTREE".into(), "/coordinator/wt".into()),
                ("MASON_RESULT_FILE".into(), "/coordinator/r.json".into()),
                ("MASON_PROGRESS_FILE".into(), "/coordinator/p".into()),
                ("MASON_SKIP_SWIFT".into(), "1".into()),
                ("PATH".into(), "/evil".into()),
            ],
            limit_s: 60,
            lease_s: 30,
            base: None,
            state: JobState::Running,
            phase: String::new(),
            slot: 1,
            prepared: false,
            started_ms: 0,
            ended_ms: 0,
            last_poll_ms: 0,
            code: 0,
            signal: 0,
            timed_out: false,
            build_s: 0,
            run_s: 0,
            why: String::new(),
        };
        let base = |k: &str| match k {
            "HOME" => Some("/Users/masonnode".to_string()),
            "PATH" => Some("/usr/bin:/bin".to_string()),
            "SECRET_TOKEN" => Some("nope".to_string()),
            _ => None,
        };
        let env = job_env(&job, Path::new("/n/slot-1"), Path::new("/n/shims"), &base);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert!(get("PATH")
            .unwrap()
            .starts_with("/n/shims:/Users/masonnode/.cargo/bin:"));
        assert!(!get("PATH").unwrap().contains("evil"));
        assert_eq!(get("MASON_WORKTREE"), Some("/n/slot-1"));
        assert_eq!(
            get("MASON_TASK"),
            Some("42"),
            "the coordinator's task number is kept"
        );
        assert_eq!(get("MASON_TASK_CACHE"), None);
        assert_eq!(get("MASON_RESULT_FILE"), None);
        assert_eq!(get("MASON_PROGRESS_FILE"), None);
        assert_eq!(get("MASON_SKIP_SWIFT"), Some("1"));
        assert_eq!(get("MASON_NODE"), Some("1"));
        assert_eq!(get("NO_COLOR"), Some("1"));
        assert_eq!(get("HOME"), Some("/Users/masonnode"));
        assert_eq!(
            get("SECRET_TOKEN"),
            None,
            "nothing else of the node's own is passed on"
        );
        // Without a task number a job is still marked as a run of the pipeline.
        let mut j2 = job.clone();
        j2.env.retain(|(k, _)| k != "MASON_TASK");
        let env = job_env(&j2, Path::new("/n/s"), Path::new("/n/shims"), &base);
        assert_eq!(
            env.iter()
                .find(|(n, _)| n == "MASON_TASK")
                .map(|(_, v)| v.as_str()),
            Some("node")
        );
    }

    #[test]
    fn base64_round_trips() {
        for s in ["", "a", "ab", "abc", "abcd", "hello, world\n\u{0}\u{ff}"] {
            let e = base64_encode(s.as_bytes());
            assert_eq!(base64_decode(&e).unwrap(), s.as_bytes(), "{s:?}");
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(base64_decode("!!!!").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_shims_drop_caffeinate_u_and_refuse_ui_tests_and_pass_the_rest_through() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Fake real tools that print what they were called with.
        for tool in ["caffeinate", "xcodebuild"] {
            let p = bin.join(tool);
            std::fs::write(&p, "#!/bin/sh\necho \"$tool $*\"\n".replace("$tool", tool)).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let shims = t.path().join("shims");
        write_shims(&shims, &bin.to_string_lossy()).unwrap();
        let run = |tool: &str, args: &[&str]| {
            let o = std::process::Command::new(shims.join(tool))
                .args(args)
                .output()
                .unwrap();
            (
                o.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&o.stdout).trim().to_string(),
                String::from_utf8_lossy(&o.stderr).trim().to_string(),
            )
        };
        assert_eq!(run("caffeinate", &["-u", "-t", "5"]).1, "caffeinate -t 5");
        assert_eq!(
            run("caffeinate", &["-dui", "-w", "9"]).1,
            "caffeinate -di -w 9"
        );
        assert_eq!(run("caffeinate", &["-u"]).1, "caffeinate");
        assert_eq!(run("caffeinate", &["--", "ls"]).1, "caffeinate -- ls");
        assert_eq!(
            run("caffeinate", &["-i", "sleep", "1"]).1,
            "caffeinate -i sleep 1"
        );
        let (code, _, err) = run(
            "xcodebuild",
            &["test", "-only-testing:AppUITests/LoginTests"],
        );
        assert_eq!(code, 69);
        assert!(err.contains("UI tests"), "{err}");
        assert_eq!(
            run(
                "xcodebuild",
                &["test", "-skip-testing:AppUITests", "-scheme", "App"]
            )
            .1,
            "xcodebuild test -skip-testing:AppUITests -scheme App"
        );
        assert_eq!(run("xcodebuild", &["-version"]).1, "xcodebuild -version");
    }
}
