// SPDX-License-Identifier: GPL-3.0-or-later

//! A test command run under a step host, so that a daemon which restarts, or
//! takes over by `exec`, finds it again instead of leaving it orphaned.
//!
//! Used by the develop agent's `run_check` (host id `t<task>r<run>-check<n>`) and
//! by landing's re-test (`b<batch>-retest<n>`). Each run has a record next to its
//! log, `<id>.check`, holding a key (the tree it ran on, and the command) and
//! whether its result has been given to a caller. A call:
//!
//! - attaches to a run with the same key that is in flight (in this process, or
//!   found again after a restart);
//! - takes the result of a run with the same key that finished while the daemon
//!   was away and was never given to anyone;
//! - otherwise starts a new run on the next free number.
//!
//! The limit counts from the host's start, not from the call that collects it.

use crate::daemon::{now_ms, Daemon};
use mason_core::StepRunId;
use mason_landing::git;
use mason_step_host::client::{collect_to_file, collect_to_file_observed, ClientError, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{CURRENT, PREVIOUS};
use mason_step_host::registry::{read_kv, write_kv, HostInfo};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub struct CheckSpec {
    /// `t5r7-check`, `b3-retest`: the run's number is appended.
    pub id_prefix: String,
    /// Where the log, exit record and run record go.
    pub log_dir: PathBuf,
    pub cmd: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub limit: Duration,
    /// What the command runs on, from [`tree_key`].
    pub tree: String,
    /// Polled while the command runs: `Some(note)` stops it (the owner skipped or forced
    /// what it belongs to). The outcome then carries the note in `stopped`.
    pub stop: Option<StopFn>,
    /// The machine slots the caller holds while the command runs (empty: none). Written next
    /// to the host so that a daemon which restarts counts them as held until the host's
    /// command ends (s589).
    pub holds: std::collections::BTreeMap<String, u32>,
    /// Whose run it is, as the resources list shows it: `#567 check`.
    pub label: String,
    /// The task and suite a develop-phase `run_check` runs for, so that the one function that
    /// decides where runs go (#698) can say. `None` for everything else (a landing re-test is
    /// #702): it stays on this machine.
    pub place: Option<(mason_core::TaskId, Box<mason_testing::Suite>)>,
}

/// What a step host's command holds of the machine, as recorded at its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostHold {
    pub dir: PathBuf,
    pub label: String,
    pub uses: std::collections::BTreeMap<String, u32>,
}

const HOLDS_FILE: &str = "holds.kv";

fn write_holds(dir: &Path, spec: &CheckSpec) {
    if spec.holds.is_empty() {
        return;
    }
    let uses = spec
        .holds
        .iter()
        .map(|(k, n)| format!("{k}={n}"))
        .collect::<Vec<_>>()
        .join(",");
    let _ = write_kv(
        &dir.join(HOLDS_FILE),
        &[("label", spec.label.replace('\n', " ")), ("uses", uses)],
    );
}

/// Every step host under `hosts` whose command still runs and recorded slots it holds.
/// A host whose command ended, or that was lost, holds nothing.
pub fn running_holds(hosts: &Path) -> Vec<HostHold> {
    use mason_step_host::registry::{HostState, Registry};
    let Ok(all) = Registry::new(hosts).scan() else {
        return vec![];
    };
    all.into_iter()
        .filter(|h| h.state == HostState::Running)
        .filter_map(|h| {
            let kv = read_kv(&h.info.dir.join(HOLDS_FILE)).ok()?;
            let uses = kv
                .get("uses")?
                .split(',')
                .filter_map(|p| {
                    let (k, n) = p.split_once('=')?;
                    Some((k.to_string(), n.parse().ok()?))
                })
                .collect();
            Some(HostHold {
                dir: h.info.dir,
                label: kv
                    .get("label")
                    .cloned()
                    .unwrap_or_else(|| h.info.id.clone()),
                uses,
            })
        })
        .collect()
}

/// Where a check's live counts go: the develop step's run, as the sub-count `name`, from
/// the output of the suite's parser (none: nothing to count).
#[derive(Clone)]
pub struct CheckProgress {
    pub run: StepRunId,
    /// Which entry of the run's check history this run belongs to.
    pub check: usize,
    /// "check cargo-workspace".
    pub name: String,
    pub suite: Option<mason_testing::Suite>,
}

impl CheckProgress {
    /// A function that takes the output as it arrives, keeps the counts and the test
    /// executable cargo is on, and reports both. For display: no verdict comes of it.
    fn feeder(&self, d: &Arc<Daemon>) -> impl FnMut(&[u8]) {
        let mut live = self
            .suite
            .as_ref()
            .and_then(|s| mason_testing::LiveCount::new(s.parser, s))
            .map(|l| (l, Default::default()));
        let (d, run, check, name) = (d.clone(), self.run, self.check, self.name.clone());
        let mut partial: Vec<u8> = vec![];
        let mut exe = String::new();
        move |b: &[u8]| {
            if let Some((l, last)) = live.as_mut() {
                l.feed(b);
                let c: mason_testing::Counts = l.counts();
                if *last != Some(c.passed) {
                    *last = Some(c.passed);
                    d.progress_sub(run, &name, None, |s| {
                        if s.done.source != mason_core::progress::Source::Reported {
                            s.unit = "tests".into();
                            s.done = mason_core::progress::Figure::counted(c.passed);
                        }
                    });
                }
            }
            partial.extend_from_slice(b);
            while let Some(i) = partial.iter().position(|x| *x == b'\n') {
                let line: Vec<u8> = partial.drain(..=i).collect();
                if let Some(e) = executable_of(&String::from_utf8_lossy(&line)) {
                    if e != exe {
                        exe = e.clone();
                        d.check_edit(run, check, |c, _| c.current = e);
                    }
                }
            }
        }
    }
}

/// The test executable a cargo `Running unittests src/lib.rs (target/debug/deps/foo-0123abcd…)`
/// line names (the file's name without cargo's hash), `None` for any other line.
pub fn executable_of(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("Running ")?;
    let path = rest.rsplit_once('(').and_then(|(_, p)| p.strip_suffix(')'));
    let base = path.map(|p| p.rsplit('/').next().unwrap_or(p));
    let name = match base {
        Some(b) => match b.rsplit_once('-') {
            Some((n, h)) if h.len() >= 8 && h.bytes().all(|c| c.is_ascii_hexdigit()) => n,
            _ => b,
        },
        None => rest,
    };
    (!name.is_empty()).then(|| name.to_string())
}

/// See [`CheckSpec::stop`].
pub type StopFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct CheckOutcome {
    pub passed: bool,
    pub timed_out: bool,
    /// The last lines of the output.
    pub output: String,
    /// The id of the host that ran it.
    pub id: String,
    /// Why it was stopped before it ended, when [`CheckSpec::stop`] stopped it. Such a run
    /// did not pass or fail; it was not finished.
    pub stopped: Option<String>,
}

type Shared = Result<CheckOutcome, String>;

#[derive(Default)]
struct Slot {
    result: Mutex<Option<Shared>>,
    cv: Condvar,
}

fn inflight() -> &'static Mutex<HashMap<PathBuf, Arc<Slot>>> {
    static M: std::sync::OnceLock<Mutex<HashMap<PathBuf, Arc<Slot>>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// What the tree a command will run on is: its committed tree, and, when the
/// worktree differs from it, a hash of the difference. A key no two states share
/// if git cannot say.
pub fn tree_key(wt: &Path) -> String {
    let unique = || format!("unknown-{}", now_ms());
    let Ok(tree) = git::run(wt, &["rev-parse", "HEAD^{tree}"]) else {
        return unique();
    };
    let tree = tree.trim().to_string();
    let Ok(status) = git::run(
        wt,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
    ) else {
        return unique();
    };
    if status.trim().is_empty() {
        return tree;
    }
    let mut h = Sha256::new();
    h.update(status.as_bytes());
    let Ok(diff) = git::run(wt, &["--no-optional-locks", "diff", "HEAD"]) else {
        return unique();
    };
    h.update(diff.as_bytes());
    let Ok(untracked) = git::run(
        wt,
        &["ls-files", "--others", "--exclude-standard", "--full-name"],
    ) else {
        return unique();
    };
    let files: Vec<&str> = untracked.lines().filter(|l| !l.is_empty()).collect();
    for chunk in files.chunks(200) {
        let mut args = vec!["hash-object", "--no-filters", "--"];
        args.extend(chunk.iter().copied());
        match git::run(wt, &args) {
            Ok(o) => h.update(o.as_bytes()),
            Err(_) => return unique(),
        }
    }
    let sum: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!("{tree}+{}", &sum[..16])
}

fn run_key(spec: &CheckSpec) -> String {
    let mut h = Sha256::new();
    h.update(spec.tree.as_bytes());
    h.update([0]);
    h.update(spec.cmd.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn record_path(spec: &CheckSpec, id: &str) -> PathBuf {
    spec.log_dir.join(format!("{id}.check"))
}

fn mark_delivered(spec: &CheckSpec, id: &str, key: &str) {
    let _ = write_kv(
        &record_path(spec, id),
        &[("key", key.to_string()), ("delivered", "1".into())],
    );
}

/// Is there already a run of `spec` whose result nobody has been given: one in
/// flight (in this process, or found again after a restart) or one that
/// finished while the daemon was away? Such a call attaches or collects, and
/// starts nothing, so it has nothing to queue for.
pub fn pending(d: &Daemon, spec: &CheckSpec) -> bool {
    let key = run_key(spec);
    (0u32..)
        .map(|n| format!("{}{n}", spec.id_prefix))
        .map_while(|id| read_kv(&record_path(spec, &id)).ok().map(|kv| (id, kv)))
        .any(|(id, kv)| {
            kv.get("key") == Some(&key)
                && kv.get("delivered").map(String::as_str) != Some("1")
                && (inflight()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(&d.cfg.hosts_dir().join(&id))
                    || matches!(
                        mason_step_host::registry::status_of(&d.cfg.hosts_dir().join(&id)),
                        Ok(s) if s.state != mason_step_host::registry::HostState::Lost
                    ))
        })
}

/// Run `spec` (or attach to, or take the result of, the run that already is it).
/// Blocks until there is a result.
pub fn run(d: &Arc<Daemon>, spec: &CheckSpec) -> Shared {
    run_with(d, spec, None)
}

/// [`run`], feeding the develop step's progress with the run's live counts (for display only;
/// the verdict is what the command's exit says, as ever).
pub fn run_with(d: &Arc<Daemon>, spec: &CheckSpec, progress: Option<&CheckProgress>) -> Shared {
    let _ = std::fs::create_dir_all(&spec.log_dir);
    let key = run_key(spec);
    // Find or allocate, one caller at a time, so two calls never take one number.
    // Earlier runs of this prefix on another tree that nobody here is collecting: a call
    // on a new tree means their results will never be asked for.
    let mut superseded: Vec<String> = vec![];
    let (id, slot, collector) = {
        let mut map = inflight().lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0u32;
        loop {
            let id = format!("{}{n}", spec.id_prefix);
            let rec = record_path(spec, &id);
            let mapkey = d.cfg.hosts_dir().join(&id);
            match read_kv(&rec) {
                Err(_) => {
                    // A free number: this run is new.
                    for ext in ["log", "exit", "remote"] {
                        let _ = std::fs::remove_file(spec.log_dir.join(format!("{id}.{ext}")));
                    }
                    let _ = write_kv(&rec, &[("key", key.clone()), ("delivered", "0".into())]);
                    let slot = Arc::new(Slot::default());
                    map.insert(mapkey, slot.clone());
                    break (id, slot, true);
                }
                Ok(kv) => {
                    let same = kv.get("key") == Some(&key);
                    let delivered = kv.get("delivered").map(String::as_str) == Some("1");
                    if same && !delivered {
                        if let Some(slot) = map.get(&mapkey) {
                            break (id, slot.clone(), false);
                        }
                        // In flight or finished in a daemon that is gone: collect it.
                        let slot = Arc::new(Slot::default());
                        map.insert(mapkey, slot.clone());
                        break (id, slot, true);
                    }
                    if !same && !delivered && !map.contains_key(&mapkey) {
                        superseded.push(id);
                    }
                }
            }
            n += 1;
        }
    };
    for old in superseded {
        if running(d, &old) && stop_host(d, &old) {
            tracing::info!(host = %old, now = %id, "stopped a check on an older tree that nobody will collect");
        }
        let _ = write_kv(
            &record_path(spec, &old),
            &[
                (
                    "key",
                    read_kv(&record_path(spec, &old))
                        .ok()
                        .and_then(|kv| kv.get("key").cloned())
                        .unwrap_or_default(),
                ),
                ("delivered", "1".into()),
                ("superseded", id.clone()),
            ],
        );
    }
    if !collector {
        let mut g = slot.result.lock().unwrap_or_else(|e| e.into_inner());
        while g.is_none() {
            g = slot.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        return g.clone().unwrap_or_else(|| Err("no result".into()));
    }
    let res = collect(d, spec, &id, progress);
    // Given to its callers now: a later call on the same tree runs again.
    mark_delivered(spec, &id, &key);
    {
        let mut g = slot.result.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(res.clone());
        slot.cv.notify_all();
    }
    inflight()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&d.cfg.hosts_dir().join(&id));
    res
}

/// Is the step host `id` alive with its command still running?
fn running(d: &Daemon, id: &str) -> bool {
    matches!(
        mason_step_host::registry::status_of(&d.cfg.hosts_dir().join(id)),
        Ok(s) if s.state == mason_step_host::registry::HostState::Running
    )
}

/// Ask the step host `id` to stop its command (and its children). `false`: it could not be
/// reached.
fn stop_host(d: &Daemon, id: &str) -> bool {
    let sock = d.cfg.hosts_dir().join(id).join("sock");
    HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon").is_ok_and(|mut h| h.stop(1000).is_ok())
}

/// The task and run a develop check's host id names (`t575r3670-check-cargo-workspace2`).
pub fn check_owner(id: &str) -> Option<(u64, u64)> {
    let (head, _) = id.split_once("-check")?;
    let (t, r) = head.strip_prefix('t')?.split_once('r')?;
    Some((t.parse().ok()?, r.parse().ok()?))
}

/// Stop develop checks that survived a daemon restart and that no run can collect any more:
/// their task's open run is another one, or none. Their step hosts outlive the daemon by
/// design, and since s589 they keep their slots, so left alone they run a whole suite for
/// nobody while holding a slot (2026-11-18: three such runs beside two real ones, five full
/// suites under a limit of two). A check this process is collecting is never touched, nor
/// one whose run is still open: its agent may call again and attach to it.
pub fn stop_unwanted(d: &Arc<Daemon>) -> usize {
    use mason_step_host::registry::{HostState, Registry};
    let Ok(all) = Registry::new(d.cfg.hosts_dir()).scan() else {
        return 0;
    };
    let Ok(w) = d.world() else { return 0 };
    let mut n = 0;
    for h in all.into_iter().filter(|h| h.state == HostState::Running) {
        let id = h.info.id.clone();
        let Some((task, run)) = check_owner(&id) else {
            continue;
        };
        if inflight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&h.info.dir)
        {
            continue;
        }
        if w.open_run(mason_core::TaskId(task)) == Some(StepRunId(run)) {
            continue;
        }
        if stop_host(d, &id) {
            tracing::info!(host = %id, "stopped a check whose run is over: nobody can collect it");
            n += 1;
        }
    }
    n
}

/// The check on GitHub Actions, when the one function that decides where runs go says so
/// (#697); `None` for the local path, which is every run until #698. The result is the same
/// `CheckOutcome`: `passed` is what the command's exit says. A remote run that is lost is run
/// again, not resumed. No live counts come from it.
fn remote_check(d: &Arc<Daemon>, spec: &CheckSpec, id: &str) -> Option<Shared> {
    use crate::remote as rm;
    if !rm::any_remote(d) {
        return None;
    }
    let project = rm::project_of(d, &spec.cwd)?;
    let place = spec.place.as_ref().map(|(t, s)| (*t, &**s));
    let rm::Executor::GitHub { runs_on } = rm::executor_for(d, &project, rm::Kind::Check, place)
    else {
        return None;
    };
    // The suite's build is the first part of what runs there (it was not run here).
    let cmd = match place.and_then(|(_, s)| s.build.as_deref()) {
        Some(b) if !spec.cmd.starts_with(b) => format!("{b} && {}", spec.cmd),
        _ => spec.cmd.clone(),
    };
    let site = rm::site_for(d, &project, &spec.cwd)?;
    let rs = rm::spec_for(
        &project, id, None, &cmd, spec.limit, &spec.env, &runs_on, &spec.tree,
    );
    let note: Mutex<Option<String>> = Mutex::new(None);
    let stop = || {
        let n = spec.stop.as_ref().and_then(|f| f())?;
        *note.lock().unwrap_or_else(|e| e.into_inner()) = Some(n);
        Some(rm::StopWhy::Skipped)
    };
    Some(
        match rm::run_or_fallback(&site, &rs, &spec.log_dir, &stop) {
            Ok(None) => return None,
            Ok(Some(rm::Outcome::Stopped(_))) => Ok(CheckOutcome {
                passed: false,
                timed_out: false,
                output: String::new(),
                id: id.to_string(),
                stopped: note.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            }),
            Ok(Some(rm::Outcome::Done(f))) => Ok(CheckOutcome {
                passed: f.code == 0 && f.signal == 0 && !f.timed_out,
                timed_out: f.timed_out,
                output: format!("({})\n{}", f.ran_on, crate::jobs::tail(&f.output, 40)),
                id: id.to_string(),
                stopped: None,
            }),
            Err(e) => Err(match rm::environmental(&e) {
                Some(why) => format!("the check could not run on GitHub: {why}; run it again"),
                None => e,
            }),
        },
    )
}

fn collect(
    d: &Arc<Daemon>,
    spec: &CheckSpec,
    id: &str,
    progress: Option<&CheckProgress>,
) -> Shared {
    if let Some(r) = remote_check(d, spec, id) {
        return r;
    }
    let hosts = d.cfg.hosts_dir();
    let dir = hosts.join(id);
    if !dir.join("host.kv").exists() {
        let o = SpawnOptions {
            host_bin: d.cfg.host_bin.clone(),
            root: hosts.clone(),
            id: id.to_string(),
            cmd: vec!["sh".into(), "-c".into(), spec.cmd.clone()],
            cwd: Some(spec.cwd.clone()),
            env: spec.env.clone(),
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: d.cfg.heartbeat,
        };
        spawn_host(&o).map_err(|e| format!("cannot start the command: {e}"))?;
        write_holds(&dir, spec);
    }
    let started = HostInfo::read(&dir).map_or_else(|_| now_ms(), |i| i.started_ms);
    let deadline = started.saturating_add(spec.limit.as_millis() as u64);
    // Stop it when the limit, counted from the host's start, has passed.
    let done = Arc::new(AtomicBool::new(false));
    let fired = Arc::new(AtomicBool::new(false));
    let stopped: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let (done, fired, sock) = (done.clone(), fired.clone(), dir.join("sock"));
        let (stop, stopped) = (spec.stop.clone(), stopped.clone());
        std::thread::spawn(move || loop {
            if done.load(Ordering::SeqCst) {
                return;
            }
            if let Some(note) = stop.as_ref().and_then(|f| f()) {
                *stopped.lock().unwrap_or_else(|e| e.into_inner()) = Some(note);
                if let Ok(mut h) = HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon") {
                    let _ = h.stop(1000);
                }
                return;
            }
            if now_ms() >= deadline {
                fired.store(true, Ordering::SeqCst);
                if let Ok(mut h) = HostClient::connect(&sock, PREVIOUS, CURRENT, "daemon") {
                    let _ = h.stop(1000);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        });
    }
    let log = spec.log_dir.join(format!("{id}.log"));
    let exit = spec.log_dir.join(format!("{id}.exit"));
    let rec = match progress {
        None => collect_to_file(&dir, &log, &exit),
        Some(pr) => {
            let mut feed = pr.feeder(d);
            // After a restart the log is read again from its start, so the counts are exact.
            if let Ok(old) = std::fs::read(&log) {
                feed(&old);
            }
            collect_to_file_observed(&dir, &log, &exit, &mut |_, b| feed(b))
        }
    };
    done.store(true, Ordering::SeqCst);
    let rec = match rec {
        Ok(r) => r,
        Err(ClientError::HostLost { .. }) => {
            return Err("the check was lost (its host died); run it again".into());
        }
        Err(e) => return Err(format!("cannot collect the check's result: {e}")),
    };
    let stopped = stopped.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let timed_out = stopped.is_none()
        && (fired.load(Ordering::SeqCst)
            || (rec.signal != 0 && rec.ended_ms > 0 && rec.ended_ms + 1500 >= deadline));
    Ok(CheckOutcome {
        passed: stopped.is_none() && rec.code == 0 && rec.signal == 0 && !timed_out,
        timed_out,
        output: crate::jobs::tail(&std::fs::read_to_string(&log).unwrap_or_default(), 40),
        id: id.to_string(),
        stopped,
    })
}
