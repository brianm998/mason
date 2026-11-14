// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the machine's CPU goes, by task. Every `CPU_EVERY` the daemon reads one
//! process table, works out which task (or fixed bucket) each process belongs to
//! and how much CPU time each used since the last table, and keeps the result for
//! the views. The attribution and the arithmetic are pure functions of the table;
//! only [`read_table`] and the sampling thread touch the machine.
//!
//! `ps -S` adds an exited child's time to its parent, so a short process that
//! starts and ends between two tables is still counted through the live tree.
//! The compile idle check (`hung.rs`) reads the latest table from here instead of
//! running its own `ps`.
//!
//! Windows has no `ps`: there `read_table` finds nothing and the reading stays
//! "unknown" (`busy_permille` -1, no shares) until the port provides process times.

use crate::daemon::Daemon;
use crate::hung::{parse_time, tree_of, Proc};
use mason_protocol::{CpuShare, CpuUsage};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the process table is read.
pub const CPU_EVERY: Duration = Duration::from_secs(5);

/// A share changing by this much (permille) is worth pushing a new home view.
pub const PUSH_PERMILLE: u32 = 10;

/// One process table: `procs` and `commands` are parallel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub procs: Vec<Proc>,
    pub commands: Vec<String>,
}

/// `ps -S -axo pid=,ppid=,pgid=,time=,stat=,command=`: five columns, then the
/// command, which runs to the end of the line and may hold spaces.
pub fn parse_table(text: &str) -> Table {
    let mut t = Table::default();
    for l in text.lines() {
        let mut rest = l.trim_start();
        let mut f = [""; 5];
        let mut n = 0;
        while n < 5 {
            match rest.split_once(char::is_whitespace) {
                Some((a, b)) => {
                    f[n] = a;
                    rest = b.trim_start();
                    n += 1;
                }
                None => {
                    // No command (a process that has none): the state ends the line.
                    if n == 4 && !rest.is_empty() {
                        f[4] = rest;
                        rest = "";
                        n = 5;
                    }
                    break;
                }
            }
        }
        if n < 5 {
            continue;
        }
        let (Ok(pid), Ok(ppid), Ok(pgid), Some(cpu_ms)) =
            (f[0].parse(), f[1].parse(), f[2].parse(), parse_time(f[3]))
        else {
            continue;
        };
        t.procs.push(Proc {
            pid,
            ppid,
            pgid,
            cpu_ms,
            stopped: f[4].starts_with('T'),
        });
        t.commands.push(rest.trim_end().to_string());
    }
    t
}

/// The latest table the sampling thread read, for `hung.rs`.
static LATEST: Mutex<Option<(Instant, Arc<Table>)>> = Mutex::new(None);

/// The latest table, if it is recent enough to stand in for a fresh `ps`.
pub fn recent_table() -> Option<Arc<Table>> {
    let g = LATEST.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref()
        .filter(|(at, _)| at.elapsed() <= CPU_EVERY * 3)
        .map(|(_, t)| t.clone())
}

fn publish_table(t: &Arc<Table>) {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), t.clone()));
}

/// Tables from `MASON_PS_FILE` (separated by `---` lines), handed out in turn,
/// and none after the last: how the exit script drives the sampler without
/// reading the machine.
fn next_fake_table(path: &std::ffi::OsStr) -> Option<Table> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let text = std::fs::read_to_string(path).ok()?;
    let tables: Vec<&str> = text.split("\n---\n").collect();
    let i = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    tables.get(i).map(|t| parse_table(t))
}

/// Read the process table. `None` when `ps` fails (or there is none).
pub fn read_table() -> Option<Table> {
    if let Some(f) = std::env::var_os("MASON_PS_FILE") {
        return next_fake_table(&f);
    }
    // macOS `ps -S`; procps spells it `S`.
    let flag = if cfg!(target_os = "macos") { "-S" } else { "S" };
    let out = std::process::Command::new("ps")
        .args([flag, "-ww", "-axo", "pid=,ppid=,pgid=,time=,stat=,command="])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let t = parse_table(&String::from_utf8_lossy(&out.stdout));
    (!t.procs.is_empty()).then_some(t)
}

/// Where a process's CPU is counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bucket {
    Task(u64),
    Masond,
    Monitor,
    Sccache,
}

/// A running step host's command: its task, and the process it started.
#[derive(Clone, Copy, Debug)]
pub struct HostRoot {
    pub task: u64,
    pub child_pid: u32,
}

/// Paths that name a task in a command line.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    /// `<worktrees>/`: a project directory and a task number follow.
    pub worktrees: String,
    /// Each project's `task_cache` up to where the task number goes.
    pub caches: Vec<String>,
    /// The daemon's own pid.
    pub self_pid: u32,
}

/// The task number directly after `prefix` in `cmd`, if `prefix` occurs.
fn number_after(cmd: &str, prefix: &str) -> Option<u64> {
    if prefix.is_empty() {
        return None;
    }
    let mut from = 0;
    while let Some(i) = cmd[from..].find(prefix) {
        let rest = &cmd[from + i + prefix.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return digits.parse().ok();
        }
        from += i + prefix.len();
    }
    None
}

/// The task a command line names, by a worktree or task-cache path.
pub fn task_in_command(cmd: &str, roots: &Roots) -> Option<u64> {
    if !roots.worktrees.is_empty() {
        let mut from = 0;
        while let Some(i) = cmd[from..].find(&roots.worktrees) {
            let rest = &cmd[from + i + roots.worktrees.len()..];
            // `<project>/<task>/`
            if let Some((_, after)) = rest.split_once('/') {
                let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
                if !digits.is_empty() {
                    return digits.parse().ok();
                }
            }
            from += i + roots.worktrees.len();
        }
    }
    roots.caches.iter().find_map(|c| number_after(cmd, c))
}

/// Whether the executable (the start of the command, up to its first option) is `name`.
fn exe_is(cmd: &str, name: &str) -> bool {
    let head = cmd.split(" -").next().unwrap_or(cmd);
    let base = head.rsplit('/').next().unwrap_or(head);
    base == name || base.strip_prefix(name).is_some_and(|r| r.starts_with(' '))
}

fn is_monitor(cmd: &str) -> bool {
    let c = cmd.to_ascii_lowercase();
    c.contains("masonmonitor") || c.contains("mason-monitor") || c.contains("mason.monitor")
}

/// Which bucket each process belongs to, by pid; a process in none is absent.
/// A process is counted once, by the first rule that matches: it is in a running
/// host's tree; its command names a task's directories; it is the daemon, a step
/// host, a monitor or the sccache server.
pub fn attribute(table: &Table, hosts: &[HostRoot], roots: &Roots) -> HashMap<u32, Bucket> {
    let mut out: HashMap<u32, Bucket> = HashMap::new();
    for h in hosts.iter().filter(|h| h.child_pid > 0) {
        for p in tree_of(&table.procs, h.child_pid) {
            out.entry(p.pid).or_insert(Bucket::Task(h.task));
        }
    }
    for (p, cmd) in table.procs.iter().zip(&table.commands) {
        if out.contains_key(&p.pid) {
            continue;
        }
        let b = if let Some(t) = task_in_command(cmd, roots) {
            Bucket::Task(t)
        } else if p.pid == roots.self_pid || exe_is(cmd, "masond") || exe_is(cmd, "step-host") {
            Bucket::Masond
        } else if is_monitor(cmd) {
            Bucket::Monitor
        } else if exe_is(cmd, "sccache") {
            Bucket::Sccache
        } else {
            continue;
        };
        out.insert(p.pid, b);
    }
    out
}

/// Turns tables into CPU use: remembers each process's CPU time at the last table.
#[derive(Default)]
pub struct Sampler {
    prev: Option<(u64, HashMap<u32, u64>)>,
}

impl Sampler {
    /// Note a table read at `now_ms`. Returns each bucket's share of the machine
    /// (permille of `cores` logical cores) since the last table; `None` for the
    /// first table, or when no time has passed. A process whose CPU time went
    /// down (its pid was reused) starts from zero; one that is new has used all
    /// it shows.
    pub fn observe(
        &mut self,
        now_ms: u64,
        table: &Table,
        assign: &HashMap<u32, Bucket>,
        cores: u32,
    ) -> Option<BTreeMap<Bucket, u32>> {
        let cur: HashMap<u32, u64> = table.procs.iter().map(|p| (p.pid, p.cpu_ms)).collect();
        let prev = self.prev.replace((now_ms, cur));
        let (prev_at, prev_cpu) = prev?;
        let elapsed = now_ms.saturating_sub(prev_at);
        if elapsed == 0 || cores == 0 {
            return None;
        }
        let mut ms: BTreeMap<Bucket, u64> = BTreeMap::new();
        for p in &table.procs {
            let Some(b) = assign.get(&p.pid) else {
                continue;
            };
            let used = match prev_cpu.get(&p.pid) {
                Some(&before) if p.cpu_ms >= before => p.cpu_ms - before,
                _ => p.cpu_ms,
            };
            *ms.entry(*b).or_default() += used;
        }
        let denom = elapsed * u64::from(cores);
        Some(
            ms.into_iter()
                .map(|(b, used)| (b, (used * 1000 / denom) as u32))
                .collect(),
        )
    }
}

/// What the views read: the shares, and each running task's own figure.
#[derive(Clone, Debug)]
pub struct Reading {
    pub usage: CpuUsage,
    /// Tasks that have a running process, with their permille (0 included).
    pub by_task: BTreeMap<u64, u32>,
}

impl Reading {
    /// Before the first reading, and on a platform that cannot read.
    pub fn none() -> Reading {
        Reading {
            usage: CpuUsage {
                busy_permille: -1,
                cores: 0,
                sampled_ms: 0,
                shares: vec![],
            },
            by_task: BTreeMap::new(),
        }
    }
}

fn share(task: u64, kind: &str, label: String, permille: u32) -> CpuShare {
    CpuShare {
        task,
        kind: kind.into(),
        label,
        title: String::new(),
        permille,
    }
}

/// The reading for one table's buckets. `busy_pct` is the overall figure the
/// compile gate uses; `other` is what it holds beyond everything attributed.
pub fn build_reading(
    buckets: &BTreeMap<Bucket, u32>,
    busy_pct: Option<u32>,
    cores: u32,
    sampled_ms: u64,
) -> Reading {
    let mut shares: Vec<CpuShare> = buckets
        .iter()
        .map(|(b, p)| match b {
            Bucket::Task(t) => share(*t, "task", format!("#{t}"), *p),
            Bucket::Masond => share(0, "masond", "masond".into(), *p),
            Bucket::Monitor => share(0, "monitor", "monitor".into(), *p),
            Bucket::Sccache => share(0, "sccache", "sccache".into(), *p),
        })
        .collect();
    let busy_permille = busy_pct.map_or(-1, |b| (b * 10) as i32);
    if busy_permille >= 0 {
        let attributed: u32 = buckets.values().sum();
        let other = (busy_permille as u32).saturating_sub(attributed);
        shares.push(share(0, "other", "other".into(), other));
    }
    shares.retain(|s| s.permille >= 1);
    sort_shares(&mut shares);
    Reading {
        usage: CpuUsage {
            busy_permille,
            cores,
            sampled_ms,
            shares,
        },
        by_task: buckets
            .iter()
            .filter_map(|(b, p)| match b {
                Bucket::Task(t) => Some((*t, *p)),
                _ => None,
            })
            .collect(),
    }
}

/// Highest first; ties by task id, then kind.
pub fn sort_shares(shares: &mut [CpuShare]) {
    shares.sort_by(|a, b| {
        b.permille
            .cmp(&a.permille)
            .then(a.task.cmp(&b.task))
            .then(a.kind.cmp(&b.kind))
    });
}

/// Whether a new reading differs enough from the last one pushed to be worth
/// a new home view.
pub fn changed_enough(old: &CpuUsage, new: &CpuUsage) -> bool {
    if (old.busy_permille < 0) != (new.busy_permille < 0) {
        return true;
    }
    let key = |s: &CpuShare| (s.kind.clone(), s.task);
    let a: HashMap<_, _> = old.shares.iter().map(|s| (key(s), s.permille)).collect();
    let b: HashMap<_, _> = new.shares.iter().map(|s| (key(s), s.permille)).collect();
    a.keys().chain(b.keys()).any(|k| {
        let (x, y) = (
            a.get(k).copied().unwrap_or(0),
            b.get(k).copied().unwrap_or(0),
        );
        x.abs_diff(y) >= PUSH_PERMILLE
    })
}

fn cores() -> u32 {
    std::env::var("MASON_CPU_CORES")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(|n| n.get() as u32)
        })
        .unwrap_or(1)
}

/// The paths that name a task in a command line.
fn roots(d: &Daemon) -> Roots {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    // A task number no real task has: where `{task}` goes in a template.
    const MARK: u64 = u64::MAX;
    let caches = d
        .projects()
        .values()
        .filter_map(|p| p.task_cache.as_ref())
        .filter_map(|t| {
            let e = crate::taskcache::expand(t, MARK, &home);
            e.split_once(&MARK.to_string()).map(|(a, _)| a.to_string())
        })
        .collect();
    Roots {
        worktrees: format!("{}/", d.cfg.worktrees_dir().display()),
        caches,
        self_pid: std::process::id(),
    }
}

/// Running step hosts and the task each belongs to.
fn running_hosts(d: &Daemon) -> Vec<HostRoot> {
    use mason_step_host::registry::{HostState, Registry};
    Registry::new(d.cfg.hosts_dir())
        .scan()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| h.state == HostState::Running)
        .filter_map(|h| {
            let (task, _) = crate::sweep::parse_host_id(&h.info.id)?;
            Some(HostRoot {
                task,
                child_pid: h.info.child_pid,
            })
        })
        .collect()
}

/// Start the sampling thread. It reads the table, never holding a lock another
/// reader waits on, and publishes an `Arc` the views clone. A failed read leaves
/// the last reading in place; its `sampled_ms` shows how old it is.
pub fn spawn(d: &Arc<Daemon>) {
    let source = d.broker.cpu_source();
    let fake = std::env::var_os("MASON_PS_FILE").is_some();
    let mut sampler = Sampler::default();
    let mut fake_now = 0u64;
    let mut first = true;
    let mut pushed: Option<CpuUsage> = None;
    crate::bg::spawn_loop(d, "cpu-by-task", move |weak| {
        if !std::mem::take(&mut first) {
            std::thread::sleep(CPU_EVERY);
        }
        let Some(d) = weak.upgrade() else {
            return false;
        };
        let Some(table) = read_table() else {
            return true;
        };
        let table = Arc::new(table);
        publish_table(&table);
        let now = if fake {
            fake_now += CPU_EVERY.as_millis() as u64;
            fake_now
        } else {
            crate::daemon::now_ms()
        };
        let assign = attribute(&table, &running_hosts(&d), &roots(&d));
        let n = cores();
        let Some(buckets) = sampler.observe(now, &table, &assign, n) else {
            return true;
        };
        let reading = build_reading(&buckets, source.busy_pct(), n, crate::daemon::now_ms());
        let push = pushed
            .as_ref()
            .is_none_or(|old| changed_enough(old, &reading.usage));
        if push {
            pushed = Some(reading.usage.clone());
        }
        d.set_cpu_reading(reading);
        if push {
            d.poke();
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Roots {
        Roots {
            worktrees: "/data/mason/w/".into(),
            caches: vec!["/Volumes/sp/mason-target/".into()],
            self_pid: 1000,
        }
    }

    fn table(text: &str) -> Table {
        parse_table(text)
    }

    #[test]
    fn the_command_column_may_hold_spaces() {
        let t = table(
            "  10     1    10   0:01.50 Ss   /Applications/Mason Monitor.app/Contents/MacOS/MasonMonitor --flag x\n\
             \x20 11    10    10 01:02:03 T+   cargo build\n\
             \x20 12    10    10 2-00:00:01 S    sh\n\
             \x20 bad line\n",
        );
        assert_eq!(t.procs.len(), 3);
        assert_eq!(
            t.commands[0],
            "/Applications/Mason Monitor.app/Contents/MacOS/MasonMonitor --flag x"
        );
        assert_eq!(t.procs[0].cpu_ms, 1500);
        assert_eq!(t.procs[1].cpu_ms, 3_723_000);
        assert!(t.procs[1].stopped && !t.procs[0].stopped);
        assert_eq!(t.procs[2].cpu_ms, 2 * 86_400_000 + 1000);
        assert_eq!(t.commands[2], "sh");
    }

    #[test]
    fn a_hosts_tree_and_its_process_group_belong_to_its_task() {
        let t = table(
            "  20     1    20   0:00.00 S    step-host t5r1-test\n\
             \x20 21    20    21   0:00.00 S    sh -c cargo test\n\
             \x20 22    21    21   0:00.00 S    cargo test\n\
             \x20 23     1    21   0:00.00 S    rustc stray\n\
             \x20 30     1    30   0:00.00 S    unrelated\n",
        );
        let hosts = [HostRoot {
            task: 5,
            child_pid: 21,
        }];
        let a = attribute(&t, &hosts, &roots());
        assert_eq!(a.get(&21), Some(&Bucket::Task(5)));
        assert_eq!(a.get(&22), Some(&Bucket::Task(5)));
        assert_eq!(a.get(&23), Some(&Bucket::Task(5)), "same process group");
        assert_eq!(a.get(&20), Some(&Bucket::Masond), "the step host itself");
        assert_eq!(a.get(&30), None);
    }

    #[test]
    fn a_compile_by_the_sccache_server_goes_to_the_task_it_names() {
        let t = table(
            "  40     1    40   0:00.00 S    sccache --start-server\n\
             \x20 41    40    40   0:00.00 S    rustc --crate-name x --out-dir /Volumes/sp/mason-target/287/debug/deps /data/mason/w/mason/287/src/lib.rs\n\
             \x20 42    40    40   0:00.00 S    rustc /data/mason/w/mason/301/src/lib.rs\n",
        );
        let a = attribute(&t, &[], &roots());
        assert_eq!(a.get(&41), Some(&Bucket::Task(287)));
        assert_eq!(a.get(&42), Some(&Bucket::Task(301)));
        assert_eq!(a.get(&40), Some(&Bucket::Sccache));
    }

    #[test]
    fn a_process_in_a_tree_and_naming_a_path_is_counted_once() {
        let t = table(
            "  50     1    50   0:00.00 S    sh\n\
             \x20 51    50    50   0:00.00 S    rustc /data/mason/w/mason/9/src/lib.rs\n",
        );
        let hosts = [HostRoot {
            task: 5,
            child_pid: 50,
        }];
        let a = attribute(&t, &hosts, &roots());
        assert_eq!(a.get(&51), Some(&Bucket::Task(5)), "the tree comes first");
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn the_daemon_hosts_and_monitors_have_their_own_buckets() {
        let t = table(
            "1000     1  1000   0:00.00 S    /usr/local/bin/masond --daemon\n\
             \x201001  1000  1000   0:00.00 S    /usr/local/bin/step-host --id t1r1-a\n\
             \x201002     1  1002   0:00.00 S    /Applications/MasonMonitor.app/Contents/MacOS/MasonMonitor\n\
             \x201003     1  1003   0:00.00 S    java -jar mason-monitor.jar\n\
             \x201004     1  1004   0:00.00 S    /usr/bin/safari\n",
        );
        let a = attribute(&t, &[], &roots());
        assert_eq!(a.get(&1000), Some(&Bucket::Masond));
        assert_eq!(a.get(&1001), Some(&Bucket::Masond));
        assert_eq!(a.get(&1002), Some(&Bucket::Monitor));
        assert_eq!(a.get(&1003), Some(&Bucket::Monitor));
        assert_eq!(a.get(&1004), None);
    }

    #[test]
    fn nine_core_seconds_in_five_on_eighteen_cores_is_a_tenth() {
        let hosts = [HostRoot {
            task: 5,
            child_pid: 21,
        }];
        let before =
            table("  21     1    21  0:10.00 S    cargo\n  22    21    21  0:00.00 S    rustc\n");
        // 22 ended; -S folded its 4 s into 21.
        let after = table("  21     1    21  0:19.00 S    cargo\n");
        let mut s = Sampler::default();
        let a = attribute(&before, &hosts, &roots());
        assert!(s.observe(0, &before, &a, 18).is_none());
        let a = attribute(&after, &hosts, &roots());
        let m = s.observe(5_000, &after, &a, 18).unwrap();
        assert_eq!(m.get(&Bucket::Task(5)), Some(&100));
    }

    #[test]
    fn a_pid_whose_time_went_down_starts_from_zero() {
        let hosts = [HostRoot {
            task: 5,
            child_pid: 21,
        }];
        let before = table("  21     1    21  1:40.00 S    cargo\n");
        let after = table("  21     1    21  0:09.00 S    cargo\n");
        let mut s = Sampler::default();
        let a = attribute(&before, &hosts, &roots());
        s.observe(0, &before, &a, 18);
        let m = s.observe(5_000, &after, &a, 18).unwrap();
        // 9 s over 90 core-seconds.
        assert_eq!(m.get(&Bucket::Task(5)), Some(&100));
    }

    #[test]
    fn shares_sort_highest_first_and_the_rest_is_other() {
        let mut b = BTreeMap::new();
        b.insert(Bucket::Task(9), 200);
        b.insert(Bucket::Task(3), 200);
        b.insert(Bucket::Masond, 50);
        b.insert(Bucket::Monitor, 0);
        b.insert(Bucket::Task(4), 700);
        let r = build_reading(&b, Some(100), 18, 1);
        // Busy 1000: attributed 1150 leaves nothing for other, clamped at 0.
        let order: Vec<(String, u32)> = r
            .usage
            .shares
            .iter()
            .map(|s| (s.label.clone(), s.permille))
            .collect();
        assert_eq!(
            order,
            vec![
                ("#4".to_string(), 700),
                ("#3".to_string(), 200),
                ("#9".to_string(), 200),
                ("masond".to_string(), 50),
            ]
        );
        assert_eq!(r.by_task.get(&4), Some(&700));
        let r = build_reading(&b, Some(200), 18, 1);
        assert_eq!(r.usage.busy_permille, 2000);
        assert_eq!(r.usage.shares.last().unwrap().label, "masond");
        let mut small = BTreeMap::new();
        small.insert(Bucket::Task(1), 300);
        let r = build_reading(&small, Some(50), 18, 1);
        let o = r.usage.shares.iter().find(|s| s.kind == "other").unwrap();
        assert_eq!(o.permille, 200);
        let r = build_reading(&small, None, 18, 1);
        assert_eq!(r.usage.busy_permille, -1);
        assert!(r.usage.shares.iter().all(|s| s.kind != "other"));
    }

    #[test]
    fn a_new_home_view_is_pushed_for_a_ten_permille_move_only() {
        let mut b = BTreeMap::new();
        b.insert(Bucket::Task(1), 300);
        let a = build_reading(&b, Some(40), 18, 1).usage;
        b.insert(Bucket::Task(1), 305);
        assert!(!changed_enough(
            &a,
            &build_reading(&b, Some(40), 18, 2).usage
        ));
        b.insert(Bucket::Task(1), 310);
        assert!(changed_enough(
            &a,
            &build_reading(&b, Some(40), 18, 2).usage
        ));
    }
}
