// SPDX-License-Identifier: GPL-3.0-or-later

//! A run-time coverage map of a directory integration target (#656, decision 0721).
//!
//! `affected.rs` can only guess, from the words in a module's source, which modules of the
//! daemon's `it` executable a change reaches: every module can spawn `masond`. This records
//! what each module really ran. A job builds the package with `-C instrument-coverage`, runs
//! the target one module at a time with every child process writing profiles to that module's
//! own directory, and keeps module -> the `src` files with a covered line, with the commit it
//! was made on. Selection uses the map where it can and falls back to the name scan.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// A map older than this is not used (seconds).
pub const MAX_AGE_SECS: u64 = 7 * 24 * 3600;

/// Where the daemon keeps the map file, set once at start. `None`: no map is ever used.
static DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

pub fn file_in(dir: &Path) -> PathBuf {
    dir.join("covmap.kv")
}

/// The map file's usable contents now: present, readable and not older than [`MAX_AGE_SECS`].
pub fn load_fresh(now_secs: u64) -> Option<CoverageMap> {
    let text = std::fs::read_to_string(file_in(DIR.get()?)).ok()?;
    let m = CoverageMap::decode(&text)?;
    (now_secs.saturating_sub(m.made_at) <= MAX_AGE_SECS).then_some(m)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoverageMap {
    /// The commit the map was made on.
    pub commit: String,
    /// Unix seconds.
    pub made_at: u64,
    /// The target's root file (`crates/daemon/tests/it/main.rs`).
    pub target: String,
    /// A hash of that root file's text: the module list changed when it differs.
    pub main_hash: u64,
    /// Module name -> workspace-relative `src` files with any covered line.
    pub modules: BTreeMap<String, BTreeSet<String>>,
}

/// FNV-1a, enough to see a changed `main.rs`. Trailing whitespace is left out: the daemon reads
/// the file through `git show`, whose output `git::run` trims, while the job that makes the map
/// reads it from disk. Hashing both as they came made every map from GitHub "not the one the
/// daemon saw".
pub fn hash_text(t: &str) -> u64 {
    t.trim_end().bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

impl CoverageMap {
    pub fn encode(&self) -> String {
        let mut s = format!(
            "commit={}\nmade_at={}\ntarget={}\nmain_hash={}\n",
            self.commit, self.made_at, self.target, self.main_hash
        );
        for (m, files) in &self.modules {
            let f: Vec<&str> = files.iter().map(String::as_str).collect();
            s.push_str(&format!("mod {m} {}\n", f.join(" ")));
        }
        s
    }

    pub fn decode(text: &str) -> Option<CoverageMap> {
        let mut m = CoverageMap::default();
        let mut seen = 0;
        for line in text.lines() {
            if let Some(r) = line.strip_prefix("mod ") {
                let mut w = r.split_whitespace();
                let name = w.next()?.to_string();
                m.modules.insert(name, w.map(String::from).collect());
            } else if let Some((k, v)) = line.split_once('=') {
                match k {
                    "commit" => m.commit = v.into(),
                    "made_at" => m.made_at = v.parse().ok()?,
                    "target" => m.target = v.into(),
                    "main_hash" => m.main_hash = v.parse().ok()?,
                    _ => continue,
                }
                seen += 1;
            }
        }
        (seen >= 4 && !m.commit.is_empty()).then_some(m)
    }

    /// The modules whose covered set holds `file`.
    pub fn modules_of<'a>(&'a self, file: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.modules
            .iter()
            .filter(move |(_, f)| f.contains(file))
            .map(|(m, _)| m.as_str())
    }

    /// Does any module's set name `file`?
    pub fn knows(&self, file: &str) -> bool {
        self.modules.values().any(|f| f.contains(file))
    }
}

/// The files of an lcov report with at least one line run, as paths under `root`, keeping only
/// `crates/<name>/src/...` ones.
pub fn lcov_files(lcov: &str, root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut cur: Option<String> = None;
    let mut hit = false;
    let mut flush = |cur: &mut Option<String>, hit: &mut bool| {
        if let (Some(f), true) = (cur.take(), *hit) {
            out.insert(f);
        }
        *hit = false;
    };
    for line in lcov.lines() {
        if let Some(f) = line.strip_prefix("SF:") {
            flush(&mut cur, &mut hit);
            cur = Path::new(f)
                .strip_prefix(root)
                .ok()
                .map(|r| r.to_string_lossy().into_owned())
                .filter(|r| {
                    r.strip_prefix("crates/")
                        .is_some_and(|x| x.contains("/src/"))
                });
        } else if let Some(r) = line.strip_prefix("DA:") {
            if r.split(',').nth(1).and_then(|c| c.parse::<u64>().ok()) > Some(0) {
                hit = true;
            }
        } else if line == "end_of_record" {
            flush(&mut cur, &mut hit);
        }
    }
    flush(&mut cur, &mut hit);
    out
}

/// Module names from `<test exe> --list` output (`mod::test_name: test`).
pub fn modules_listed(list: &str) -> BTreeSet<String> {
    list.lines()
        .filter_map(|l| l.strip_suffix(": test"))
        .filter_map(|l| l.split_once("::").map(|(m, _)| m.to_string()))
        .collect()
}

/// Paths of the executables in `cargo ... --message-format=json` output.
pub fn executables(json_lines: &str) -> Vec<(String, String, bool)> {
    json_lines
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["reason"] == "compiler-artifact")
        .filter_map(|v| {
            let exe = v["executable"].as_str()?.to_string();
            let name = v["target"]["name"].as_str()?.to_string();
            Some((name, exe, v["profile"]["test"].as_bool().unwrap_or(false)))
        })
        .collect()
}

/// `llvm-profdata` and `llvm-cov` from the `llvm-tools` rustup component, if installed.
pub fn llvm_tools() -> Option<(PathBuf, PathBuf)> {
    let out = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .ok()?;
    let sysroot = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    for e in std::fs::read_dir(sysroot.join("lib/rustlib"))
        .ok()?
        .flatten()
    {
        let bin = e.path().join("bin");
        let (p, c) = (bin.join("llvm-profdata"), bin.join("llvm-cov"));
        if p.is_file() && c.is_file() {
            return Some((p, c));
        }
    }
    None
}

/// What the job needs to know about the checkout it measures.
pub struct Job<'a> {
    /// The checkout (a worktree at `commit`).
    pub wt: &'a Path,
    /// The package directory the tests run in, relative to `wt` (`crates/daemon`).
    pub pkg_dir: &'a str,
    pub package: &'a str,
    pub test: &'a str,
    /// The target's root file, relative to `wt`.
    pub main_rs: &'a str,
    pub commit: &'a str,
    /// Its own `CARGO_TARGET_DIR`; never the pipeline's.
    pub target_dir: &'a Path,
    /// Where each module's profiles go.
    pub work_dir: &'a Path,
    pub now_secs: u64,
    /// `false` ends the job between modules.
    pub go_on: &'a dyn Fn() -> bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JobError {
    /// The `llvm-tools` component is missing: selection stays on the name scan.
    NoLlvmTools,
    Failed(String),
}

fn sh(c: &mut Command, what: &str) -> Result<std::process::Output, JobError> {
    c.output()
        .map_err(|e| JobError::Failed(format!("{what}: {e}")))
}

/// Build with coverage and run each module of the target alone; merge each module's profiles
/// and read the `src` files it ran.
pub fn run_job(j: &Job) -> Result<CoverageMap, JobError> {
    let (profdata, cov) = llvm_tools().ok_or(JobError::NoLlvmTools)?;
    // The profiles carry the paths the compiler saw: use the canonical root throughout.
    let root = std::fs::canonicalize(j.wt).map_err(|e| JobError::Failed(e.to_string()))?;
    let flags = "-C instrument-coverage";
    let built = sh(
        Command::new("cargo")
            .args(["test", "-p", j.package, "--test", j.test, "--no-run"])
            .arg("--message-format=json")
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", j.target_dir)
            .env("RUSTFLAGS", flags),
        "cargo build",
    )?;
    if !built.status.success() {
        return Err(JobError::Failed("the coverage build failed".into()));
    }
    let exes = executables(&String::from_utf8_lossy(&built.stdout));
    let test_exe = exes
        .iter()
        .find(|(n, _, t)| n == j.test && *t)
        .map(|(_, e, _)| e.clone())
        .ok_or_else(|| JobError::Failed("no test executable in the build output".into()))?;
    let listed = sh(Command::new(&test_exe).arg("--list"), "list")?;
    let mods = modules_listed(&String::from_utf8_lossy(&listed.stdout));
    let mut map = CoverageMap {
        commit: j.commit.to_string(),
        made_at: j.now_secs,
        target: j.main_rs.to_string(),
        main_hash: hash_text(&std::fs::read_to_string(j.wt.join(j.main_rs)).unwrap_or_default()),
        modules: BTreeMap::new(),
    };
    for m in mods {
        if !(j.go_on)() {
            return Err(JobError::Failed("stopped".into()));
        }
        let dir = j.work_dir.join(&m);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| JobError::Failed(e.to_string()))?;
        // The run's own status does not matter: a red test still leaves its profile.
        sh(
            Command::new(&test_exe)
                .arg(format!("{m}::"))
                .current_dir(root.join(j.pkg_dir))
                .env("LLVM_PROFILE_FILE", dir.join("p-%p-%m.profraw")),
            "run",
        )?;
        let raws: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| JobError::Failed(e.to_string()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "profraw"))
            .collect();
        if raws.is_empty() {
            map.modules.insert(m, BTreeSet::new());
            continue;
        }
        let merged = dir.join("merged.profdata");
        let st = sh(
            Command::new(&profdata)
                .args(["merge", "-sparse", "-o"])
                .arg(&merged)
                .args(&raws),
            "llvm-profdata",
        )?;
        if !st.status.success() {
            return Err(JobError::Failed(format!(
                "llvm-profdata merge failed for {m}"
            )));
        }
        let mut c = Command::new(&cov);
        c.args(["export", "--format=lcov", "-instr-profile"])
            .arg(&merged)
            .arg(&test_exe);
        for (_, e, _) in exes.iter().filter(|(_, e, _)| *e != test_exe) {
            c.arg(format!("-object={e}"));
        }
        let out = sh(&mut c, "llvm-cov")?;
        if !out.status.success() {
            return Err(JobError::Failed(format!("llvm-cov export failed for {m}")));
        }
        map.modules
            .insert(m, lcov_files(&String::from_utf8_lossy(&out.stdout), &root));
    }
    Ok(map)
}

/// Is a new map due? None yet, or a day old and `changed_src` (the daemon's `src` changed since
/// its commit), or `main.rs` is not the one it saw.
pub fn due(
    old: Option<&CoverageMap>,
    now_secs: u64,
    changed_src: bool,
    main_hash_now: u64,
) -> bool {
    match old {
        None => true,
        Some(m) => {
            m.main_hash != main_hash_now
                || (now_secs.saturating_sub(m.made_at) >= 24 * 3600 && changed_src)
        }
    }
}

/// What the runner runs for a `covmap` run (#780): the daemon's own coverage job, in this
/// checkout, writing [`RESULTS`]. `rustup component add llvm-tools` is here, not in the workflow,
/// so the workflow file stays as it is.
pub const REMOTE_COMMAND: &str = "rustup component add llvm-tools && \
cargo run -q -p mason-daemon --bin masond -- covmap-job";

/// The results file the remote run leaves in the checkout's root.
pub const RESULTS: &str = "covmap.kv";

/// `masond covmap-job`: make the map of the checkout in the current directory and write it to
/// [`RESULTS`]. The commit comes from `MASON_COVMAP_COMMIT` (the runner's checkout is a snapshot
/// commit, not the base tip). Returns the exit code.
pub fn job_main() -> i32 {
    let Ok(cwd) = std::env::current_dir() else {
        return 2;
    };
    let Ok(commit) = std::env::var("MASON_COVMAP_COMMIT") else {
        eprintln!("covmap-job: MASON_COVMAP_COMMIT is not set");
        return 2;
    };
    let r = run_job(&Job {
        wt: &cwd,
        pkg_dir: PKG_DIR,
        package: PACKAGE,
        test: TEST,
        main_rs: MAIN_RS,
        commit: &commit,
        target_dir: &cwd.join("target/covmap"),
        work_dir: &cwd.join("target/covmap-prof"),
        now_secs: now_secs(),
        go_on: &|| true,
    });
    match r {
        Ok(m) => match std::fs::write(cwd.join(RESULTS), m.encode()) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("covmap-job: cannot write {RESULTS}: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("covmap-job: {e:?}");
            1
        }
    }
}

/// A map made on GitHub is used only if it is of the commit asked for and of the `main.rs` the
/// daemon saw.
pub fn accept(text: &str, tip: &str, main_hash: u64) -> Result<CoverageMap, String> {
    let m = CoverageMap::decode(text).ok_or("the run returned no coverage map")?;
    if m.commit != tip {
        return Err(format!("the map is of {}, not of {tip}", m.commit));
    }
    if m.main_hash != main_hash {
        return Err("the map's main.rs is not the one the daemon saw".into());
    }
    Ok(m)
}

/// The facts a due-check needs, as the daemon finds them.
pub struct Want<'a> {
    pub old: Option<&'a CoverageMap>,
    pub tip: &'a str,
    pub main_text: &'a str,
    pub changed_src: bool,
    pub now_secs: u64,
}

/// Ask GitHub for a map when one is due: one run of kind `covmap` on `tip` (`site.cwd` is a
/// checkout of it), whose command is [`REMOTE_COMMAND`] and whose results file is [`RESULTS`].
/// `Ok(None)`: none is due, nothing was pushed.
pub fn make_remote(
    site: &crate::remote::Site,
    want: &Want,
    project: &str,
    log_dir: &Path,
    limit: std::time::Duration,
    stop: &dyn Fn() -> Option<crate::remote::StopWhy>,
) -> Result<Option<CoverageMap>, String> {
    use crate::remote as rm;
    let hash = hash_text(want.main_text);
    if !due(want.old, want.now_secs, want.changed_src, hash) {
        return Ok(None);
    }
    let _ = std::fs::remove_file(site.cwd.join(RESULTS));
    let id = format!("covmap-{}", &want.tip[..want.tip.len().min(12)]);
    let env = vec![("MASON_COVMAP_COMMIT".to_string(), want.tip.to_string())];
    let mut spec = rm::spec_for(
        project,
        &id,
        None,
        REMOTE_COMMAND,
        limit,
        &env,
        "ubuntu-latest",
        "",
    );
    spec.suite = "covmap".into();
    spec.results_file = Some(RESULTS.into());
    let spec = rm::with_site_env(spec, site);
    let out = match rm::execute(
        site,
        &spec,
        log_dir,
        stop,
        &rm::Grant::unplumbed("covmap: a background run outside the pipeline's slots"),
    ) {
        Ok(o) => o,
        Err(rm::Fail::Environmental(e)) | Err(rm::Fail::Error(e)) => return Err(e),
    };
    let f = match out {
        rm::Outcome::Done(f) => f,
        rm::Outcome::Stopped(_) => return Err("stopped".into()),
    };
    if f.code != 0 {
        return Err(format!("the covmap run exited {}", f.code));
    }
    let text = std::fs::read_to_string(site.cwd.join(RESULTS))
        .map_err(|_| "the covmap run left no covmap.kv".to_string())?;
    accept(&text, want.tip, hash).map(Some)
}

/// The package the map is made for. The job is about the daemon's `it` executable.
const PACKAGE: &str = "mason-daemon";
const PKG_DIR: &str = "crates/daemon";
const TEST: &str = "it";
const MAIN_RS: &str = "crates/daemon/tests/it/main.rs";

static SAID_NO_TOOLS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Look once an hour (`debt_interval`, the idle pass's interval; zero turns it off) whether a
/// map is due on each project's base branch.
pub fn spawn(d: &std::sync::Arc<crate::daemon::Daemon>) {
    let every = d.cfg.debt_interval;
    if every.is_zero() {
        return;
    }
    set_dir(d.cfg.data_dir.join("covmap"));
    let mut first = true;
    crate::bg::spawn_loop(d, "covmap", move |weak| {
        std::thread::sleep(if first {
            every.min(std::time::Duration::from_secs(300))
        } else {
            every
        });
        first = false;
        let Some(d) = weak.upgrade() else {
            return false;
        };
        pass(&d);
        true
    });
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// One look at every project. Idle time only: no compile waiting, and a `test-suite` slot taken
/// as work behind every task.
pub fn pass(d: &std::sync::Arc<crate::daemon::Daemon>) {
    use mason_landing::git;
    let Some(dir) = DIR.get().cloned() else {
        return;
    };
    for (pid, p) in d.projects() {
        if d.is_shutting_down() {
            return;
        }
        let Ok(tip) = git::run(&p.path, &["rev-parse", &p.base_ref()]) else {
            continue;
        };
        let tip = tip.trim().to_string();
        let Ok(main_text) = git::run(&p.path, &["show", &format!("{tip}:{MAIN_RS}")]) else {
            continue;
        };
        let old = std::fs::read_to_string(file_in(&dir))
            .ok()
            .and_then(|t| CoverageMap::decode(&t));
        let changed_src = old.as_ref().is_none_or(|m| {
            git::run(
                &p.path,
                &[
                    "diff",
                    "--name-only",
                    &m.commit,
                    &tip,
                    "--",
                    "crates/daemon/src",
                ],
            )
            .map_or(true, |o| !o.trim().is_empty())
        });
        if !due(old.as_ref(), now_secs(), changed_src, hash_text(&main_text)) {
            continue;
        }
        // With remote runs on, GitHub makes the map (a node does not: it is GitHub's own `gha`
        // pool that is leased here) and the work leases `gha`, not this Mac's
        // `test-suite` (#780). `gha` full: wait for a later pass.
        let remote = matches!(
            crate::remote::executor_global(d, &p.id),
            crate::remote::Executor::GitHub { .. }
        ) && crate::remote::site_for(d, &p.id, &p.path).is_some();
        let c = d.broker.compilation();
        if remote {
            if d.broker
                .limit("gha")
                .is_some_and(|l| d.broker.used("gha") >= l)
            {
                return;
            }
        } else if !c.pending.is_empty() || (c.limit > 0 && c.used >= c.limit) {
            return;
        }
        let mut uses = BTreeMap::new();
        uses.insert(if remote { "gha" } else { "test-suite" }.to_string(), 1u32);
        let id = d.broker.lease_open_as(
            "coverage map",
            &uses,
            Some(crate::sched::Standing {
                steps_remaining: 100,
                span: 1,
                priority: 3,
            }),
        );
        let granted = loop {
            if d.is_shutting_down() {
                break false;
            }
            match d.broker.lease_poll(id) {
                crate::sched::LeaseState::Granted { .. } => break true,
                crate::sched::LeaseState::Gone => break false,
                crate::sched::LeaseState::Waiting { .. } => {
                    std::thread::sleep(std::time::Duration::from_secs(2))
                }
            }
        };
        if granted && remote {
            let want = Want {
                old: old.as_ref(),
                tip: &tip,
                main_text: &main_text,
                changed_src,
                now_secs: now_secs(),
            };
            make_on_github(d, pid, &p.id, &p.path, &dir, &want);
        } else if granted {
            make(d, pid, &p.path, &dir, &tip);
        }
        d.broker.lease_close(id);
    }
}

/// Make the map on GitHub, from a checkout of the tip, and keep it.
fn make_on_github(
    d: &std::sync::Arc<crate::daemon::Daemon>,
    pid: mason_core::ProjectId,
    project: &str,
    repo: &Path,
    dir: &Path,
    want: &Want,
) {
    use mason_landing::git;
    let _ = std::fs::create_dir_all(dir);
    let wt = dir.join("wt");
    let lock = d.repo_lock(pid);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(repo, &wt);
        if let Err(e) = git::run(
            repo,
            &[
                "worktree",
                "add",
                "--detach",
                &wt.to_string_lossy(),
                want.tip,
            ],
        ) {
            tracing::warn!("coverage map: cannot make a checkout: {e}");
            return;
        }
    }
    let r = match crate::remote::site_for(d, project, &wt) {
        Some(site) => {
            let spec_like = crate::remote::spec_for(
                project,
                &format!("covmap-{}", &want.tip[..want.tip.len().min(12)]),
                None,
                REMOTE_COMMAND,
                REMOTE_LIMIT,
                &[],
                "ubuntu-latest",
                "",
            );
            let _hold = crate::remote::HoldGuard::new(
                &d.cfg.hosts_dir(),
                &spec_like,
                "coverage map",
                "coverage map",
                1,
                &dir.join("remote"),
                "debt",
                0,
            );
            make_remote(
                &site,
                want,
                project,
                &dir.join("remote"),
                REMOTE_LIMIT,
                &|| d.is_shutting_down().then_some(crate::remote::StopWhy::Gone),
            )
        }
        None => Err("the project has no [remote_runs] section".into()),
    };
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(repo, &wt);
    }
    match r {
        Ok(Some(m)) => store(dir, &m, want.tip),
        Ok(None) => {}
        Err(e) => tracing::warn!("coverage map (GitHub): {e}"),
    }
}

/// How long the remote job may run: an instrumented build and one run per module.
const REMOTE_LIMIT: std::time::Duration = std::time::Duration::from_secs(2 * 3600);

/// Keep a made map (a temporary file, then a rename).
pub fn store(dir: &Path, m: &CoverageMap, tip: &str) {
    let tmp = dir.join("covmap.kv.tmp");
    if std::fs::write(&tmp, m.encode())
        .and_then(|()| std::fs::rename(&tmp, file_in(dir)))
        .is_err()
    {
        tracing::warn!("coverage map: cannot write it");
    } else {
        tracing::info!(commit = tip, modules = m.modules.len(), "coverage map made");
    }
}

fn make(
    d: &std::sync::Arc<crate::daemon::Daemon>,
    pid: mason_core::ProjectId,
    repo: &Path,
    dir: &Path,
    tip: &str,
) {
    use mason_landing::git;
    let _ = std::fs::create_dir_all(dir);
    let wt = dir.join("wt");
    let lock = d.repo_lock(pid);
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(repo, &wt);
        if let Err(e) = git::run(
            repo,
            &["worktree", "add", "--detach", &wt.to_string_lossy(), tip],
        ) {
            tracing::warn!("coverage map: cannot make a checkout: {e}");
            return;
        }
    }
    let def = d.current_def(pid);
    let r = crate::jobs::setup_checkout(&def, &wt)
        .map_err(JobError::Failed)
        .and_then(|()| {
            run_job(&Job {
                wt: &wt,
                pkg_dir: PKG_DIR,
                package: PACKAGE,
                test: TEST,
                main_rs: MAIN_RS,
                commit: tip,
                target_dir: &dir.join("target"),
                work_dir: &dir.join("prof"),
                now_secs: now_secs(),
                go_on: &|| !d.is_shutting_down(),
            })
        });
    {
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        git::remove_worktree(repo, &wt);
    }
    match r {
        Ok(m) => store(dir, &m, tip),
        Err(JobError::NoLlvmTools) => {
            if !SAID_NO_TOOLS.swap(true, std::sync::atomic::Ordering::SeqCst) {
                tracing::warn!(
                    "coverage map: the llvm-tools component is not installed \
                     (`rustup component add llvm-tools`); test selection stays on the name scan"
                );
            }
        }
        Err(JobError::Failed(e)) => tracing::warn!("coverage map: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> CoverageMap {
        CoverageMap {
            commit: "abc1234def".into(),
            made_at: 100,
            target: "t/it/main.rs".into(),
            main_hash: 5,
            modules: BTreeMap::from([
                (
                    "holds".to_string(),
                    BTreeSet::from(["c/src/holds.rs".to_string(), "c/src/sched.rs".to_string()]),
                ),
                ("empty".to_string(), BTreeSet::new()),
            ]),
        }
    }

    #[test]
    fn a_map_round_trips() {
        let m = map();
        assert_eq!(CoverageMap::decode(&m.encode()), Some(m));
        assert_eq!(CoverageMap::decode("junk"), None);
    }

    #[test]
    fn a_map_made_from_the_file_is_accepted_for_the_text_git_show_gave() {
        // The job hashes main.rs as read from disk (with its final newline); the daemon hashes
        // the trimmed `git show` output it got through `git::run`.
        let on_disk = "mod holds;\nmod sched;\n";
        let from_git = on_disk.trim();
        let mut m = map();
        m.main_hash = hash_text(on_disk);
        assert!(accept(&m.encode(), &m.commit, hash_text(from_git)).is_ok());
        assert!(!due(Some(&m), m.made_at, false, hash_text(from_git)));
        // A real change to main.rs is still seen.
        assert!(accept(&m.encode(), &m.commit, hash_text("mod holds;\n")).is_err());
    }

    #[test]
    fn lcov_keeps_covered_src_files_only() {
        let root = Path::new("/w");
        let lcov = "SF:/w/crates/a/src/x.rs\nDA:1,3\nend_of_record\n\
                    SF:/w/crates/a/src/never.rs\nDA:1,0\nend_of_record\n\
                    SF:/w/crates/a/tests/t.rs\nDA:1,9\nend_of_record\n\
                    SF:/elsewhere/crates/a/src/y.rs\nDA:1,9\nend_of_record\n";
        assert_eq!(
            lcov_files(lcov, root),
            BTreeSet::from(["crates/a/src/x.rs".to_string()])
        );
    }

    #[test]
    fn the_list_gives_module_names() {
        let l = "a::t1: test\na::b::t2: test\nc::t3: test\n3 tests, 0 benchmarks\n";
        assert_eq!(
            modules_listed(l),
            BTreeSet::from(["a".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn due_when_missing_or_main_changed_or_old_and_changed() {
        let m = map();
        assert!(due(None, 0, false, 5));
        assert!(due(Some(&m), 101, false, 6));
        assert!(!due(Some(&m), 101, true, 5));
        assert!(due(Some(&m), 100 + 24 * 3600, true, 5));
        assert!(!due(Some(&m), 100 + 24 * 3600, false, 5));
    }
}
