// SPDX-License-Identifier: GPL-3.0-or-later
#![allow(dead_code)]

//! A scratch world for end-to-end tests: a bare "origin", a clone, a project
//! file pointing at the scripted stand-in for `claude`, and a daemon config.

pub mod lw;
#[cfg(unix)]
pub mod slots;

use mason_core::*;
use mason_daemon::config::DaemonConfig;
use mason_daemon::Daemon;
use mason_landing::git;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A binary from another package. Cargo only builds (and rebuilds) every
/// package's binaries for `cargo test --workspace`, so build it, once per run:
/// a stale binary would silently test old behaviour.
pub fn bin(package: &str, name: &str) -> PathBuf {
    static BUILT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let exe = std::env::current_exe().unwrap();
    let target = exe.parent().unwrap().parent().unwrap();
    let mut built = BUILT.lock().unwrap();
    if !built.iter().any(|b| b == name) {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        // Build into the target directory this test binary came from. The suite passes
        // `--target-dir` on the command line, which a nested cargo does not inherit, so without
        // this the binary lands in the worktree's `target/` while we return a path under the
        // suite's: a narrowed run (`-p mason-daemon --test it`) then finds no `mason` at all,
        // or a stale one left in a restored cache.
        let target_root = target.parent().unwrap();
        let ok = std::process::Command::new(cargo)
            .args(["build", "-q", "-p", package, "--bin", name])
            .arg("--target-dir")
            .arg(target_root)
            // The suite's runner gives this process RUSTC_BOOTSTRAP=1 for `--report-time`
            // (s634); build scripts rerun when it changes, so a build that kept it would
            // recompile the workspace the suite's build just compiled without it.
            .env_remove("RUSTC_BOOTSTRAP")
            .status()
            .unwrap()
            .success();
        assert!(ok, "could not build {name}");
        built.push(name.to_string());
    }
    target.join(name)
}

pub struct Scratch {
    pub root: tempfile::TempDir,
    pub origin: PathBuf,
    pub repo: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub script: PathBuf,
    /// Held for as long as this scratch world lives (declared last, so released last).
    _slot: Option<Slot>,
    /// The root's `owner.lock`, held while this world lives.
    _owner: Owner,
}

/// How many scratch worlds (each a real daemon with step hosts, git and a scripted
/// agent) run at once on the whole machine, across every test binary and every task's
/// suites. A per-binary limit let N binaries run 8*N daemons and hang the Mac. A test
/// waits for a slot before its world exists, so the wait never counts against its
/// deadlines. A test that already holds a world does not wait for its next one
/// ([`slots::claim`]). `MASON_TEST_WORLDS` overrides the number.
pub fn world_limit() -> usize {
    std::env::var("MASON_TEST_WORLDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| {
            let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
            (cpus / 4).clamp(2, 8)
        })
}

/// How long a test waits for a world slot before it goes ahead without one.
///
/// Going ahead is what the wait always ends in when it ends badly: a test that makes two worlds
/// cannot deadlock on its own slot, and a slot held by a leaked or hung process cannot stop the
/// suite for good. So a long patience only delays. It was 600 s, and in a week of recorded
/// times tests that needed one second were charged 284, 384, 601 and 602 s, 160 of the 196
/// minutes of the workspace's slowest tests. A world is held for seconds by most tests and for
/// a few minutes by the slowest, so two minutes outlasts an ordinary queue (the wait is fair,
/// see [`slots`]) and still gives up quickly on a slot that is not coming free.
pub const WORLD_PATIENCE: Duration = Duration::from_secs(120);

#[cfg(unix)]
struct Slot(#[allow(dead_code)] slots::Claim);

#[cfg(unix)]
impl Slot {
    fn take() -> Slot {
        let t = std::time::Instant::now();
        let claim = slots::claim(&slots::shared_dir(), world_limit(), WORLD_PATIENCE);
        // The wait counts in libtest's `--report-time`; say how long it was so the parser can
        // take it off (#778). The thread's name is the test's name.
        let ms = t.elapsed().as_millis();
        if ms >= 50 {
            let me = std::thread::current();
            report_world_wait(ms, me.name().unwrap_or("?"));
        }
        Slot(claim)
    }
}

/// Say, in the run's log, that test `name` waited `ms` for a world slot.
///
/// This writes to the process's stderr itself. `eprintln!` is captured by libtest for the
/// running test and shown only if it fails, so for a passing test (every one that matters here)
/// the line never reached the log and the parser (`mason-testing`'s `subtract_world_waits`)
/// never saw it: the recorded time of a test that waited 600 s stayed 600 s.
#[cfg(unix)]
pub fn report_world_wait(ms: u128, name: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "mason-world-wait {ms} {name}");
}

/// Windows has no flock-based slots; the daemon tests only build there, never run.
#[cfg(not(unix))]
struct Slot;

#[cfg(not(unix))]
impl Slot {
    fn take() -> Slot {
        Slot
    }
}

static PROCESS_WORLD: std::sync::OnceLock<Slot> = std::sync::OnceLock::new();

/// Take one world slot for the whole test process, once, and let every [`Scratch`] made after
/// it share it instead of taking its own. For a binary whose tests run one at a time under a
/// process-wide lock (so one world exists at a time): each test taking its own slot waited up
/// to its whole patience ([`WORLD_PATIENCE`]) for it with the lock held while other binaries held every slot, and the
/// waits of the tests queued behind added up. Call it with the lock held.
pub fn hold_world_for_process() {
    PROCESS_WORLD.get_or_init(Slot::take);
}

/// What the scripted agent commits. Every task's worktree also gets a file of its
/// own, so a task that starts after another has landed still has something to
/// commit, whatever the timing (identical commits would leave it with nothing).
pub const TASK_SCRIPT_COMMIT: &str = "echo hello > hello.txt && echo task > unique-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'";

/// The same as `TASK_SCRIPT_COMMIT` (kept for the tests that named it).
pub const UNIQUE_COMMIT: &str = TASK_SCRIPT_COMMIT;

pub fn sh(cwd: &Path, args: &[&str]) -> String {
    git::run(cwd, args).unwrap_or_else(|e| panic!("{e}"))
}

impl Drop for Scratch {
    /// Step hosts outlive their daemon by design, so a test's temp dir going away
    /// would leave them (and their agents) running for ever. Runs on a panic too.
    fn drop(&mut self) {
        kill_hosts(&self.data_dir);
    }
}

#[cfg(unix)]
pub mod orphans {
    use nix::fcntl::{Flock, FlockArg};
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// One process of `ps`.
    struct Proc {
        pid: i32,
        ppid: i32,
        pgid: i32,
        command: String,
    }

    fn process_table() -> Vec<Proc> {
        let Ok(out) = std::process::Command::new("/bin/ps")
            .args(["-axo", "pid=,ppid=,pgid=,command="])
            .output()
        else {
            return vec![];
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let pid = it.next()?.parse().ok()?;
                let ppid = it.next()?.parse().ok()?;
                let pgid = it.next()?.parse().ok()?;
                let command = it.collect::<Vec<_>>().join(" ");
                Some(Proc {
                    pid,
                    ppid,
                    pgid,
                    command,
                })
            })
            .collect()
    }

    /// The `/tmp/ms*` scratch root a command line's `--dir` or `--data-dir` names, if any.
    /// Only that shape matches: a real pipeline's hosts live elsewhere.
    pub fn scratch_root_of(command: &str) -> Option<PathBuf> {
        let mut words = command.split(' ');
        while let Some(w) = words.next() {
            if w != "--dir" && w != "--data-dir" {
                continue;
            }
            let path = Path::new(words.next()?);
            let rest = path
                .strip_prefix("/tmp")
                .or_else(|_| path.strip_prefix("/private/tmp"))
                .ok()?;
            let first = rest.components().next()?.as_os_str().to_str()?;
            if first.starts_with("ms") && first.len() > 2 {
                return Some(Path::new("/tmp").join(first));
            }
        }
        None
    }

    /// How old a root without `owner.lock` must be before it counts as left over: an older
    /// test binary (made before locks) may still be running in a younger one.
    const LOCKLESS_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

    /// The scratch root's ownership: an exclusive `flock` on `<root>/owner.lock`, held for
    /// the scratch's life. The kernel drops it when the process dies, however it dies.
    pub struct Owner(#[allow(dead_code)] Option<Flock<File>>);

    /// Take ownership of `root`, writing this pid in the lock file for diagnostics. The pid
    /// is written after the lock is held, so a sweep that sees an unlocked, empty file knows
    /// the owner is still starting up.
    pub fn claim(root: &Path) -> Owner {
        let open = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("owner.lock"));
        let Ok(file) = open else { return Owner(None) };
        let Ok(mut lock) = Flock::lock(file, FlockArg::LockExclusiveNonblock) else {
            return Owner(None);
        };
        let _ = lock.set_len(0);
        let _ = write!(&mut *lock, "{}", std::process::id());
        Owner(Some(lock))
    }

    /// What a root's lock says.
    #[derive(PartialEq, Debug)]
    enum Lock {
        Missing,
        Held,
        /// Nobody holds it and a pid was written: the owner is gone.
        Free,
        /// Nobody holds it and nothing was written: an owner between create and lock.
        Starting,
    }

    fn lock_state(root: &Path) -> Lock {
        let path = root.join("owner.lock");
        if !path.exists() {
            return Lock::Missing;
        }
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
            return Lock::Held;
        };
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Err(_) => Lock::Held,
            Ok(lock) => {
                if lock.metadata().map_or(0, |m| m.len()) > 0 {
                    Lock::Free
                } else {
                    Lock::Starting
                }
            }
        }
    }

    fn age(path: &Path) -> std::time::Duration {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .unwrap_or_default()
    }

    /// What the sweep makes of a scratch root that exists.
    #[derive(PartialEq, Debug)]
    enum Fate {
        /// Its owner holds it (or may be starting up): never touched.
        Alive,
        /// Its owner is gone: stop what runs under it, then remove it.
        Dead,
        /// No lock and over a day old (an older test binary's): removed only if no process
        /// references it, and nothing is stopped.
        Stale,
    }

    fn fate(root: &Path) -> Fate {
        match lock_state(root) {
            Lock::Held => Fate::Alive,
            Lock::Free => Fate::Dead,
            Lock::Starting | Lock::Missing if age(root) > LOCKLESS_AGE => Fate::Stale,
            Lock::Starting | Lock::Missing => Fate::Alive,
        }
    }

    /// Remove a dead root, its `owner.lock` last: a removal cut short (the test binary ended)
    /// leaves a free lock with a pid in it, so the next sweep still knows the root is dead.
    fn remove_root(root: &Path) {
        let lock = root.join("owner.lock");
        for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let path = e.path();
            if path == lock {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
        let _ = std::fs::remove_file(&lock);
        let _ = std::fs::remove_dir(root);
    }

    /// Stop step hosts and test daemons whose scratch root is gone or whose owner is gone
    /// (with their children and the groups those lead), then remove the dead roots, and
    /// lock-less roots over a day old that no process references. A root whose lock is held
    /// is never touched.
    pub fn sweep_now() {
        for root in sweep_processes() {
            remove_root(&root);
        }
    }

    /// Held for a whole sweep. A sweep probes each `owner.lock` with a non-blocking flock, so
    /// a second sweep probing at the same moment reads a dead root as held and skips it.
    /// Sweeps of this process queue on a mutex, those of other test binaries on a lock file.
    struct SweepGuard(
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
        #[allow(dead_code)] Option<Flock<File>>,
    );

    impl SweepGuard {
        fn take() -> SweepGuard {
            static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let m = M.lock().unwrap_or_else(|e| e.into_inner());
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open("/tmp/.mason-test-sweep.lock")
                .ok()
                .and_then(|f| Flock::lock(f, FlockArg::LockExclusive).ok());
            SweepGuard(m, file)
        }
    }

    /// Stop what runs under dead roots, as `sweep_now`, and return the roots to remove.
    fn sweep_processes() -> Vec<PathBuf> {
        use nix::sys::signal::{kill, killpg, Signal};
        use nix::unistd::{getpgrp, getpid, Pid};
        let _one_sweep = SweepGuard::take();
        let table = process_table();
        let (me, my_group) = (getpid().as_raw(), getpgrp().as_raw());
        let mut dead: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut stale: Vec<PathBuf> = Vec::new();
        let tmp = std::fs::read_dir("/tmp").into_iter().flatten().flatten();
        for e in tmp {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with("ms") || name.len() <= 2 {
                continue;
            }
            let root = e.path();
            if !root.is_dir() {
                continue;
            }
            match fate(&root) {
                Fate::Alive => {}
                Fate::Dead => {
                    dead.insert(root.clone());
                    seen.insert(root);
                }
                Fate::Stale => {
                    // Referenced by any process, under either spelling of /tmp: left alone.
                    let a = format!("{}/", root.display());
                    let b = format!("/private{a}");
                    let used = table
                        .iter()
                        .any(|p| p.command.contains(&a) || p.command.contains(&b));
                    if !used {
                        stale.push(root);
                    }
                }
            }
        }
        for p in &table {
            if let Some(root) = scratch_root_of(&p.command) {
                if !root.exists() {
                    dead.insert(root);
                }
            }
        }
        for p in &table {
            let Some(root) = scratch_root_of(&p.command) else {
                continue;
            };
            if !dead.contains(&root) || p.pid <= 1 || p.pid == me {
                continue;
            }
            for c in table.iter().filter(|c| c.ppid == p.pid) {
                if c.pgid > 1 && c.pgid != my_group {
                    let _ = killpg(Pid::from_raw(c.pgid), Signal::SIGKILL);
                }
                let _ = kill(Pid::from_raw(c.pid), Signal::SIGKILL);
            }
            if p.pgid > 1 && p.pgid != my_group {
                let _ = killpg(Pid::from_raw(p.pgid), Signal::SIGKILL);
            }
            let _ = kill(Pid::from_raw(p.pid), Signal::SIGKILL);
        }
        stale.extend(dead.into_iter().filter(|r| seen.contains(r)));
        stale
    }

    /// The sweep, once per test binary, before its first world. Processes are stopped before
    /// the first world starts; the roots, which can be many and large, are removed on a
    /// thread of their own, so no test waits for the disk.
    pub fn sweep_orphans() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let roots = sweep_processes();
            if !roots.is_empty() {
                std::thread::spawn(move || roots.iter().for_each(|r| remove_root(r)));
            }
        });
    }

    /// A shell `while` loop that blocks until `release` exists, and ends early when `dir` is
    /// gone or the process `owner` (the test) is. A killed test never runs its cleanup, so the
    /// loop must notice by itself, or the step host running it polls for ever.
    pub fn blocking_loop(dir: &Path, release: &Path, owner: u32, tick: &str) -> String {
        format!(
            "while [ -d '{d}' ] && [ ! -e '{f}' ] && kill -0 {owner} 2>/dev/null; do sleep {tick}; done",
            d = dir.display(),
            f = release.display(),
        )
    }
}

#[cfg(unix)]
use orphans::{claim, sweep_orphans, Owner};

/// Orphan sweeping needs `ps` and signals; other platforms skip it.
#[cfg(not(unix))]
fn sweep_orphans() {}

#[cfg(not(unix))]
pub struct Owner;

#[cfg(not(unix))]
fn claim(_root: &Path) -> Owner {
    Owner
}

impl Scratch {
    pub fn new(test_cmd: &str, script: Value) -> Scratch {
        sweep_orphans();
        let slot = if PROCESS_WORLD.get().is_some() {
            None
        } else {
            Some(Slot::take())
        };
        let root = tempfile::Builder::new()
            .prefix("ms")
            .tempdir_in("/tmp")
            .unwrap();
        let owner = claim(root.path());
        let r = std::fs::canonicalize(root.path()).unwrap();
        let origin = r.join("o.git");
        let repo = r.join("repo");
        std::fs::create_dir_all(&origin).unwrap();
        sh(&origin, &["init", "-q", "--bare", "-b", "main"]);
        sh(&r, &["clone", "-q", origin.to_str().unwrap(), "repo"]);
        sh(&repo, &["checkout", "-q", "-b", "main"]);
        std::fs::write(repo.join("README"), "scratch\n").unwrap();
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "init"]);
        sh(&repo, &["push", "-q", "origin", "main"]);
        // The owner's checkout sits on another branch, as it would while MASON works.
        sh(&repo, &["checkout", "-q", "-b", "owner-work"]);

        let config_dir = r.join("c");
        std::fs::create_dir_all(config_dir.join("projects")).unwrap();
        let script_path = r.join("script.json");
        std::fs::write(&script_path, script.to_string()).unwrap();
        let s = Scratch {
            root,
            origin,
            repo,
            config_dir,
            data_dir: r.join("d"),
            script: script_path,
            _slot: slot,
            _owner: owner,
        };
        s.write_project("scratch", test_cmd, "");
        s
    }

    pub fn write_project(&self, id: &str, test_cmd: &str, extra: &str) {
        let toml = format!(
            "path = \"{repo}\"\nbase = \"main\"\nremote = \"origin\"\ntest = \"{test}\"\nclaude_bin = \"{fake}\"\n\
             step_timeout_s = 120\ntest_timeout_s = 60\n{extra}\n[agent_env]\nFAKE_CLAUDE_SCRIPT = \"{script}\"\n",
            repo = self.repo.display(),
            test = test_cmd,
            fake = bin("mason-adapters", "fake-claude").display(),
            script = self.script.display(),
        );
        std::fs::write(
            self.config_dir.join("projects").join(format!("{id}.toml")),
            toml,
        )
        .unwrap();
    }

    pub fn config(&self) -> DaemonConfig {
        let mut c = DaemonConfig::new(
            &self.data_dir,
            &self.config_dir,
            bin("mason-step-host", "step-host"),
            bin("mason-adapters", "mason-mcp"),
        );
        c.sweep_interval = Duration::from_secs(3600);
        c.heartbeat = Duration::from_millis(100);
        c.hold_retry = Duration::from_millis(300);
        // Tests never read the machine's real load.
        c.cpu = Some(mason_daemon::cpu::FakeCpu::new(0));
        // Nor the real disk: a full volume must not hold up the suite.
        c.disk = Some(mason_daemon::disk::FakeDisk::new_gb(1000));
        // The launches the frozen-launch alert counts are the whole test process's: one test's
        // host that is slow to come up on a busy Mac must not raise an alert in every other
        // test's daemon (the notify tests count alerts and announcements exactly).
        c.launch_frozen_alert = false;
        // Tests file their tasks one after another and expect them in one batch: the window
        // stays whole unless a test is about the early formation (#731).
        c.landing_window_always = true;
        c
    }

    /// What a daemon that ran before would already have fetched: startup reads the
    /// remote-tracking refs as they are and does no network (the watcher fetches).
    pub fn prefetch(&self) {
        // Only a remote on this disk: a test that points origin at a network
        // address (a silent listener) means to leave it unanswered.
        let url = git::run(&self.repo, &["config", "remote.origin.url"])
            .map(|o| o.trim().to_string())
            .unwrap_or_default();
        if !url.starts_with('/') {
            return;
        }
        let _ = git::run_for(
            &self.repo,
            &["fetch", "-q", "--prune", "origin"],
            Duration::from_secs(120),
        );
    }

    pub fn start(&self) -> Arc<Daemon> {
        self.start_with(|_| {})
    }

    /// Start a daemon after letting `f` adjust its configuration.
    pub fn start_with(&self, f: impl FnOnce(&mut DaemonConfig)) -> Arc<Daemon> {
        self.prefetch();
        let mut c = self.config();
        f(&mut c);
        let d = Daemon::start(c).expect("daemon starts");
        // Admission waits for the first definitions pass; a test that files work and looks at
        // admission right away would otherwise be looking at that wait.
        let end = Instant::now() + Duration::from_secs(60);
        while !d.ready_to_admit() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        d
    }

    /// Commit files to the remote's `main`, as somebody else would (a landed
    /// change to `.mason/`, a new source file, ...), without touching our clone.
    pub fn push_files(&self, files: &[(&str, &str)], msg: &str) {
        let dir = self.repo.parent().unwrap().join("pusher");
        if !dir.exists() {
            sh(
                self.repo.parent().unwrap(),
                &[
                    "clone",
                    "-q",
                    self.origin.to_str().unwrap(),
                    dir.to_str().unwrap(),
                ],
            );
        }
        sh(&dir, &["pull", "-q", "origin", "main"]);
        for (path, body) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", msg]);
        sh(&dir, &["push", "-q", "origin", "HEAD:main"]);
    }

    pub fn origin_log(&self) -> Vec<String> {
        sh(&self.origin, &["log", "--format=%s", "main"])
            .lines()
            .map(String::from)
            .collect()
    }
}

pub fn explore_turn() -> Value {
    json!([{"call": {"tool": "submit_plan", "args": {
        "summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file",
        "acceptance_criteria": ["hello.txt exists"]}}}])
}

pub fn develop_turn() -> Value {
    json!([
        {"sh": TASK_SCRIPT_COMMIT},
        {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}
    ])
}

/// The script for the happy path: one explore turn, one develop turn.
pub fn happy_script() -> Value {
    json!({"steps": {"explore": [explore_turn()], "develop": [develop_turn()]}})
}

/// Like `happy_script`, but every task commits a file of its own (see `UNIQUE_COMMIT`),
/// for tests where tasks run one after another.
pub fn unique_script() -> Value {
    json!({"steps": {"explore": [explore_turn()], "develop": [[
        {"sh": UNIQUE_COMMIT},
        {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}
    ]]}})
}

pub fn file_task(d: &Arc<Daemon>, title: &str) -> TaskId {
    let pid = d.project_id_by_name("scratch").unwrap();
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.into(),
            body: format!("Please do: {title}"),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .unwrap();
    a.effects
        .iter()
        .find_map(|e| {
            if let Effect::TaskCreated { task } = e {
                Some(*task)
            } else {
                None
            }
        })
        .unwrap()
}

/// File a task with a body and an explicit explored claim and trust.
pub fn file_task_as(
    d: &Arc<Daemon>,
    title: &str,
    body: &str,
    explored: bool,
    trust: TrustSource,
    proposed: bool,
) -> TaskId {
    let pid = d.project_id_by_name("scratch").unwrap();
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.into(),
            body: body.into(),
            priority: Priority::Normal,
            trust,
            explored,
            proposed,
            blockers: vec![],
            not_before: None,
        })
        .unwrap();
    a.effects
        .iter()
        .find_map(|e| {
            if let Effect::TaskCreated { task } = e {
                Some(*task)
            } else {
                None
            }
        })
        .unwrap()
}

pub fn task_of(d: &Arc<Daemon>, t: TaskId) -> Task {
    d.world().unwrap().tasks[&t].clone()
}

/// The step names a task ran, in order.
pub fn steps_run(d: &Arc<Daemon>, t: TaskId) -> Vec<String> {
    d.world()
        .unwrap()
        .step_runs
        .values()
        .filter(|r| r.task == t)
        .map(|r| r.name.clone())
        .collect()
}

/// Every wait gets at least this long: a step host can take over a minute to
/// launch on a busy or syspolicyd-stalled Mac, and that is not a test failure.
const MIN_WAIT_SECS: u64 = 900;

/// The floor: `MASON_TEST_MIN_WAIT_S` when it holds a whole number of seconds, else
/// [`MIN_WAIT_SECS`]. A runner with nothing competing sets it lower, so a wait that can never
/// succeed gives up sooner; a wait that succeeds is unchanged.
fn min_wait_secs() -> u64 {
    floor_from(std::env::var("MASON_TEST_MIN_WAIT_S").ok().as_deref())
}

fn floor_from(v: Option<&str>) -> u64 {
    v.and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(MIN_WAIT_SECS)
}

/// What one look at a wait's condition found.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    /// What the test waits for has happened.
    Done,
    /// Not yet, but it still can.
    Pending,
    /// It can no longer happen, for this reason: fail now rather than sit out the floor.
    Never(String),
}

/// The loop under every wait. `expired` and `pause` are injected so the helper's own tests
/// run on a fake clock and count polls instead of measuring time.
fn wait_loop(
    what: &str,
    secs: u64,
    expired: &mut dyn FnMut() -> bool,
    pause: &mut dyn FnMut(),
    poll: &mut dyn FnMut() -> Poll,
) -> Result<(), String> {
    loop {
        match poll() {
            Poll::Done => return Ok(()),
            Poll::Never(why) => return Err(format!("gave up waiting for {what}: {why}")),
            Poll::Pending => {}
        }
        if expired() {
            return Err(format!("timed out after {secs}s waiting for {what}"));
        }
        pause();
    }
}

fn try_wait(
    what: &str,
    secs: u64,
    pause_ms: u64,
    mut poll: impl FnMut() -> Poll,
) -> Result<(), String> {
    let end = Instant::now() + Duration::from_secs(secs);
    let mut polls = 0u32;
    let mut watched = |p: &mut dyn FnMut() -> Poll| -> Poll {
        match p() {
            Poll::Pending => {
                polls += 1;
                // One `ps` per second at most, not one per poll.
                if polls.is_multiple_of(20) {
                    if let Some(why) = daemon_gone() {
                        return Poll::Never(why);
                    }
                }
                Poll::Pending
            }
            other => other,
        }
    };
    wait_loop(
        what,
        secs,
        &mut || Instant::now() >= end,
        &mut || std::thread::sleep(Duration::from_millis(pause_ms)),
        &mut || watched(&mut poll),
    )
}

fn real_wait(what: &str, secs: u64, pause_ms: u64, poll: impl FnMut() -> Poll) {
    let r = try_wait(what, secs, pause_ms, poll);
    assert!(r.is_ok(), "{}", r.unwrap_err());
}

thread_local! {
    /// The scratch daemons this test thread started and has not dropped.
    static DAEMONS: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A test daemon of this thread that has exited and is still waiting to be reaped: it died
/// on its own (a panic, a crash), because a test that stops one reaps it at once. A daemon
/// already reaped is not reported.
fn daemon_gone() -> Option<String> {
    let pids = DAEMONS.with(|d| d.borrow().clone());
    for pid in pids {
        let o = Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if String::from_utf8_lossy(&o.stdout)
            .trim_start()
            .starts_with('Z')
        {
            return Some(format!("the test daemon (pid {pid}) exited while waiting"));
        }
    }
    None
}

/// Wait for `f`, at least [`MIN_WAIT_SECS`] if it stays possible. Fails at once when a scratch
/// daemon of this test has exited.
pub fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    wait_poll(what, secs, || if f() { Poll::Done } else { Poll::Pending });
}

/// What task `t` is doing, for a wait that gave up: its state, step, activity, failure,
/// each step run with its outcome, and the logs of its hosts.
pub fn describe_task(d: &Arc<Daemon>, t: TaskId) -> String {
    let w = d.world().unwrap();
    let k = &w.tasks[&t];
    let mut runs: Vec<_> = w.step_runs.values().filter(|r| r.task == t).collect();
    runs.sort_by_key(|r| r.id.0);
    let runs: Vec<String> = runs
        .iter()
        .map(|r| format!("  {} (attempt {}): {:?}", r.name, r.attempt, r.outcome))
        .collect();
    let out = format!(
        "task {t}: state {:?}, step {:?}, activity {:?}, failure {:?} ({:?}), failures {}\nstep runs:\n{}",
        k.state,
        k.step,
        k.activity,
        k.failure,
        k.failure_class,
        k.failures,
        runs.join("\n")
    );
    drop(w);
    format!("{out}\nlogs:\n{}", mason_daemon::http::render_logs(d, t.0))
}

/// As [`wait_for`], and when it gives up the failure also carries `report()`.
pub fn wait_for_reporting(
    what: &str,
    secs: u64,
    mut f: impl FnMut() -> bool,
    report: impl Fn() -> String,
) {
    let r = try_wait(what, secs.max(min_wait_secs()), 50, || {
        if f() {
            Poll::Done
        } else {
            Poll::Pending
        }
    });
    if let Err(why) = r {
        panic!("{why}\n{}", report());
    }
}

/// As [`wait_for`], and the condition can also say the wait can no longer succeed.
pub fn wait_poll(what: &str, secs: u64, poll: impl FnMut() -> Poll) {
    real_wait(what, secs.max(min_wait_secs()), 50, poll);
}

/// As [`wait_for_exact`], but the failure comes back as a message.
pub fn try_wait_for_exact(
    what: &str,
    secs: u64,
    pause_ms: u64,
    mut f: impl FnMut() -> bool,
) -> Result<(), String> {
    try_wait(what, secs, pause_ms, || {
        if f() {
            Poll::Done
        } else {
            Poll::Pending
        }
    })
}

/// As [`wait_for`] with exactly `secs` (no floor), for waits on things that need no step host.
pub fn wait_for_exact(what: &str, secs: u64, pause_ms: u64, mut f: impl FnMut() -> bool) {
    real_wait(what, secs, pause_ms, || {
        if f() {
            Poll::Done
        } else {
            Poll::Pending
        }
    });
}

pub fn state_of(d: &Arc<Daemon>, t: TaskId) -> State {
    d.world().unwrap().tasks[&t].state
}

fn terminal(s: State) -> bool {
    matches!(s, State::Failed | State::Canceled | State::Landed)
}

pub fn wait_state(d: &Arc<Daemon>, t: TaskId, want: State, secs: u64) {
    wait_state_settled(d, t, want, secs, |_| false);
}

/// Wait for task `t` to reach `want`. Fails at once if it is in another terminal state, or in
/// a state `settled` says it will not leave without the test acting.
pub fn wait_state_settled(
    d: &Arc<Daemon>,
    t: TaskId,
    want: State,
    secs: u64,
    settled: impl Fn(State) -> bool,
) {
    wait_for(&format!("task {t} to reach {want:?}"), secs, || {
        let s = state_of(d, t);
        if s != want && (terminal(s) || settled(s)) {
            let evs: Vec<String> = d
                .store
                .read_since(0, 1000)
                .unwrap()
                .iter()
                .map(|e| format!("{}: {:?}", e.seq, e.event.kind))
                .collect();
            panic!(
                "task {t} reached {s:?} while waiting for {want:?}\nevents:\n{}\nlogs:\n{}",
                evs.join("\n"),
                mason_daemon::http::render_logs(d, t.0)
            );
        }
        s == want
    });
}

// ---- running the real binaries ----

use std::process::{Child, Command, Output, Stdio};

pub struct Masond {
    pub child: Child,
    data_dir: PathBuf,
}

impl Drop for Masond {
    fn drop(&mut self) {
        let pid = self.child.id();
        DAEMONS.with(|d| d.borrow_mut().retain(|p| *p != pid));
        let _ = self.child.kill();
        let _ = self.child.wait();
        kill_hosts(&self.data_dir);
    }
}

/// Step hosts outlive the daemon by design; tests must not leak them.
///
/// Only a host whose pids still are the processes the registry recorded (start
/// strings match, the command line names this host directory, the child leads its
/// own group) is signalled; pids 0 and 1 and our own pid and group never are. A
/// registry keeps every host that ever started, and on a busy machine (ten tasks
/// running tests at once) killing a stale pid could take down another task's host.
pub fn kill_hosts(data_dir: &Path) {
    mason_step_host::registry::reap_verified(&data_dir.join("h"));
}

impl Scratch {
    pub fn masond(&self, extra_args: &[&str], env: &[(&str, &str)]) -> Masond {
        self.masond_with(true, extra_args, env)
    }

    /// As `masond`, but with no `--port`: what the port tests need.
    pub fn masond_no_port(&self, extra_args: &[&str]) -> Masond {
        self.masond_with(false, extra_args, &[])
    }

    fn masond_with(&self, free_port: bool, extra_args: &[&str], env: &[(&str, &str)]) -> Masond {
        self.prefetch();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_masond"));
        let cfg = self.config();
        cmd.args(["--data-dir", &self.data_dir.display().to_string()])
            .args(["--config-dir", &self.config_dir.display().to_string()])
            .args(["--host-bin", &cfg.host_bin.display().to_string()])
            .args(["--mcp-bin", &cfg.mcp_bin.display().to_string()])
            .args([
                "--heartbeat-ms",
                "100",
                "--hold-retry-s",
                "1",
                "--sweep-ms",
                "3600000",
            ])
            // A free port, so parallel test daemons never collide on the default.
            .args(if free_port {
                vec!["--port", "0"]
            } else {
                vec![]
            })
            .args(extra_args)
            .stdout(Stdio::null())
            .stderr(match std::env::var("MASON_TEST_LOG") {
                // The test asks for this daemon's log in a file of its own.
                _ if env.iter().any(|(k, _)| *k == "MASON_TEST_STDERR") => env
                    .iter()
                    .find(|(k, _)| *k == "MASON_TEST_STDERR")
                    .and_then(|(_, p)| std::fs::File::create(p).ok())
                    .map_or(Stdio::null(), Stdio::from),
                // A path: append the daemon's log there (failures then quote its tail).
                Ok(p) if p.starts_with('/') => std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .map_or(Stdio::null(), Stdio::from),
                Ok(_) => Stdio::inherit(),
                Err(_) => Stdio::null(),
            });
        // Tests never read the machine's real load, and never run a sampler per daemon.
        let cpu = self.root.path().join("cpu-for-masond");
        let _ = std::fs::write(&cpu, "0\n");
        cmd.env("MASON_CPU_FILE", &cpu);
        let disk = self.root.path().join("disk-for-masond");
        let _ = std::fs::write(&disk, "1000\n");
        cmd.env("MASON_DISK_FILE", &disk);
        // When MASON runs this suite as a task's test step, the step's own variables are in
        // this process's environment. A test daemon is not part of that step: its hosts
        // inherit what it has, so a check would read MASON_STEP and act as a test step.
        for k in [
            "MASON_TASK",
            "MASON_TITLE",
            "MASON_PROJECT",
            "MASON_WORKTREE",
            "MASON_BRANCH",
            "MASON_BASE",
            "MASON_STEP",
            "MASON_ATTEMPT",
            "MASON_HELD",
            "MASON_RESULT_FILE",
            "MASON_PROGRESS_FILE",
        ] {
            cmd.env_remove(k);
        }
        for (k, v) in env.iter().filter(|(k, _)| *k != "MASON_TEST_STDERR") {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("masond starts");
        DAEMONS.with(|d| d.borrow_mut().push(child.id()));
        Masond {
            child,
            data_dir: self.data_dir.clone(),
        }
    }

    /// Wait until the daemon has announced itself and answers.
    pub fn wait_daemon(&self, m: &mut Masond) {
        wait_for("masond to answer", 60, || {
            if let Ok(Some(st)) = m.child.try_wait() {
                panic!("masond exited early: {st}");
            }
            self.mason(&["status", "--json"]).status.success()
        });
    }

    pub fn mason(&self, args: &[&str]) -> Output {
        Command::new(bin("mason-pipeline", "mason"))
            .env("MASON_UPDATE_WAIT_S", "8")
            .args(["--data-dir", &self.data_dir.display().to_string()])
            .args(["--config-dir", &self.config_dir.display().to_string()])
            .args(args)
            .output()
            .expect("mason runs")
    }

    pub fn mason_ok(&self, args: &[&str]) -> String {
        let o = self.mason(args);
        assert!(
            o.status.success(),
            "mason {args:?} failed: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    pub fn task_json(&self, id: u64) -> Value {
        serde_json::from_str(&self.mason_ok(&["task", "show", &id.to_string(), "--json"])).unwrap()
    }

    pub fn wait_task(&self, id: u64, state: &str, secs: u64) {
        wait_for(&format!("task {id} to be {state}"), secs, || {
            let o = self.mason(&["task", "show", &id.to_string(), "--json"]);
            if !o.status.success() {
                return false; // the daemon may be down or restarting
            }
            let v: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
            let now = v["state"].as_str().unwrap_or("");
            if now != state && matches!(now, "failed" | "canceled" | "landed") {
                panic!("task {id} ended {now}, wanted {state}: {v:#}");
            }
            now == state
        });
    }
}

/// With `UPDATE_FIXTURES=1`, save a run's event log as a replay fixture:
/// one `at<TAB>kind<TAB>payload` line per event, in the on-disk text format.
pub fn maybe_record(d: &Arc<Daemon>, name: &str) {
    if std::env::var("UPDATE_FIXTURES").is_err() {
        return;
    }
    let mut out = String::new();
    for e in d.store.read_since(0, 100_000).unwrap() {
        let (kind, payload) = mason_store::encode_event(&e.event.kind);
        out.push_str(&format!("{}\t{kind}\t{payload}\n", e.event.at.0));
    }
    let path = format!(
        "{}/tests/fixtures/{name}.events",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
    std::fs::write(path, out).unwrap();
}

#[cfg(test)]
mod wait_tests {
    use super::*;

    #[test]
    fn the_floor_comes_from_the_variable_and_is_900_without_it() {
        assert_eq!(floor_from(None), 900);
        assert_eq!(floor_from(Some("180")), 180);
        assert_eq!(floor_from(Some(" 60 ")), 60);
        assert_eq!(floor_from(Some("")), 900);
        assert_eq!(floor_from(Some("soon")), 900);
        assert_eq!(floor_from(Some("0")), 900);
        // What the real helper uses is the same function of the real environment.
        let set = std::env::var("MASON_TEST_MIN_WAIT_S").ok();
        assert_eq!(min_wait_secs(), floor_from(set.as_deref()));
    }

    #[test]
    fn a_wait_that_gives_up_names_the_floor_it_was_given() {
        let r = wait_loop(
            "x",
            floor_from(Some("180")),
            &mut || true,
            &mut || {},
            &mut || Poll::Pending,
        );
        assert_eq!(r.unwrap_err(), "timed out after 180s waiting for x");
    }

    #[test]
    fn a_wait_that_can_no_longer_succeed_stops_at_once() {
        let mut polls = 0;
        let mut ticks = 0;
        let r = wait_loop(
            "task 3 to reach Failed",
            900,
            &mut || false, // the floor never runs out in this test
            &mut || ticks += 1,
            &mut || {
                polls += 1;
                if polls == 3 {
                    Poll::Never("task 3 reached Landed".into())
                } else {
                    Poll::Pending
                }
            },
        );
        let e = r.unwrap_err();
        assert!(e.contains("task 3 reached Landed"), "{e}");
        assert!(!e.contains("timed out"), "{e}");
        assert_eq!(polls, 3);
        assert_eq!(ticks, 2);
    }

    #[test]
    fn a_wait_still_able_to_succeed_runs_to_its_deadline() {
        let mut polls = 0;
        let mut left = 5;
        let r = wait_loop(
            "x",
            900,
            &mut || {
                left -= 1;
                left == 0
            },
            &mut || {},
            &mut || {
                polls += 1;
                Poll::Pending
            },
        );
        assert_eq!(r.unwrap_err(), "timed out after 900s waiting for x");
        assert_eq!(polls, 5);
    }

    #[test]
    fn a_wait_that_succeeds_late_still_succeeds() {
        let mut polls = 0;
        let r = wait_loop("x", 900, &mut || false, &mut || {}, &mut || {
            polls += 1;
            if polls == 40 {
                Poll::Done
            } else {
                Poll::Pending
            }
        });
        assert!(r.is_ok());
        assert_eq!(polls, 40);
    }

    #[test]
    fn a_daemon_that_exited_is_named() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        let pid = child.id();
        DAEMONS.with(|d| d.borrow_mut().push(pid));
        // Let it exit without reaping it, as a crashed daemon would be.
        let mut why = None;
        let r = wait_loop(
            "a thing",
            900,
            &mut || false,
            &mut || std::thread::sleep(Duration::from_millis(20)),
            &mut || match daemon_gone() {
                Some(w) => {
                    why = Some(w.clone());
                    Poll::Never(w)
                }
                None => Poll::Pending,
            },
        );
        let e = r.unwrap_err();
        assert!(e.contains("exited") && e.contains(&pid.to_string()), "{e}");
        assert!(!e.contains("timed out"), "{e}");
        DAEMONS.with(|d| d.borrow_mut().clear());
        let _ = child.wait();
        assert!(why.is_some());
    }
}
