// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason node doctor` and the node's start-up probe (`docs/multi-machine-nodes-plan.md`,
//! §3.14). A node runs as its own macOS user from a LaunchDaemon, with nobody logged in, so it
//! can never answer a privacy prompt: macOS denies the request, and the process gets `EPERM` or,
//! at worst, blocks. So the node checks, in words, everything it depends on: that it is signed
//! by a team (an ad-hoc signature changes with every build and drops every grant); that every
//! path it will write is writable, within a deadline so that a stalled disk or a denial reports
//! instead of hanging; that a configured volume is mounted; that its listener is up; and that its
//! toolchains run. A failure that stops it doing a job turns into `accepting = false` with a
//! `why_not` the owner can act on.
//!
//! What is checked is data (`Inputs`), judged by a pure function (`evaluate`), so the table of
//! failures is a table test; `gather` is the part that touches the machine.

use crate::node_run::job_path;
use mason_step_host::host::{check_sock_path, MAX_SOCK_PATH};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// How long one path or one tool may take before it is reported as stalled.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// What the node's own executable is signed with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signature {
    /// Anchored to a team: the grants hold across updates. The team id, if the system said it.
    Team(Option<String>),
    /// Ad hoc: its requirement is the hash of this build, which changes with every build.
    AdHoc,
    /// Not signed at all.
    Unsigned,
    /// `codesign` said something that is not understood, or did not answer.
    Unknown(String),
    /// Not looked at: not macOS, or the owner said a development node may be unsigned.
    Unchecked,
}

/// What came of writing, reading and deleting a probe file in a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathState {
    Ok,
    /// `EPERM`/`EACCES`: macOS (or the file's owner) said no.
    Denied,
    /// Did not answer within the deadline.
    Stalled,
    /// Under `/Volumes/<name>`, and `<name>` is not mounted.
    NotMounted(String),
    Other(String),
}

/// What mDNS did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mdns {
    Advertising,
    /// Registration failed: why.
    Failed(String),
    /// Not asked to (tests, or no listener).
    Off,
}

/// Everything the checks look at.
#[derive(Clone, Debug)]
pub struct Inputs {
    pub signature: Signature,
    pub paths: Vec<(PathBuf, PathState)>,
    /// The socket of a job's step host under the data directory (all are as long).
    pub host_socket: PathBuf,
    pub listener_bound: bool,
    pub mdns: Mdns,
    /// `(tool, Ok(what it printed) | Err(why it did not run))`, for `cargo`, `xcode-select` and
    /// `java`.
    pub tools: Vec<(String, Result<String, String>)>,
}

/// One line of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    /// What was found, or what to do.
    pub detail: String,
    /// A failure of this check stops the node taking work.
    pub blocks: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub at_ms: u64,
    pub checks: Vec<Check>,
}

impl Report {
    /// Why the node cannot take work: the first failed check that blocks, in words.
    pub fn problem(&self) -> Option<String> {
        self.checks
            .iter()
            .find(|c| !c.ok && c.blocks)
            .map(|c| c.detail.clone())
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "at_ms": self.at_ms,
            "ok": self.checks.iter().all(|c| c.ok || !c.blocks),
            "checks": self.checks.iter().map(|c| serde_json::json!({
                "name": c.name, "ok": c.ok, "detail": c.detail, "blocks": c.blocks,
            })).collect::<Vec<_>>(),
        })
    }
}

/// The volume a path is on, when it is under `/Volumes`: `/Volumes/sp/x` is `sp`.
pub fn volume_of(path: &Path) -> Option<String> {
    let mut c = path.components();
    if !matches!(c.next(), Some(std::path::Component::RootDir)) {
        return None;
    }
    match (c.next(), c.next()) {
        (Some(std::path::Component::Normal(v)), Some(std::path::Component::Normal(n)))
            if v == "Volumes" =>
        {
            Some(n.to_string_lossy().into_owned())
        }
        _ => None,
    }
}

/// The checks, judged. Pure.
pub fn evaluate(i: &Inputs, at_ms: u64) -> Report {
    let mut checks = vec![];
    let mut add = |name: &str, ok: bool, blocks: bool, detail: String| {
        checks.push(Check {
            name: name.to_string(),
            ok,
            detail,
            blocks,
        })
    };

    match &i.signature {
        Signature::Team(t) => add(
            "signature",
            true,
            true,
            format!(
                "signed by team {}: privacy grants hold across updates",
                t.as_deref().unwrap_or("(not named)")
            ),
        ),
        Signature::AdHoc => add(
            "signature",
            false,
            true,
            "masond is signed ad hoc; privacy grants will not hold: install the runtime from the coordinator (`mason node export-runtime`)".into(),
        ),
        Signature::Unsigned => add(
            "signature",
            false,
            true,
            "masond is not signed; macOS will not keep any privacy grant for it: install the runtime from the coordinator (`mason node export-runtime`)".into(),
        ),
        Signature::Unknown(why) => add(
            "signature",
            false,
            true,
            format!("could not read masond's signature: {why}"),
        ),
        Signature::Unchecked => add("signature", true, false, "not checked".into()),
    }

    for (path, st) in &i.paths {
        let shown = path.display();
        let vol = volume_of(path);
        let (ok, detail) = match st {
            PathState::Ok => (true, format!("{shown} is writable")),
            PathState::Denied => (
                false,
                match &vol {
                    Some(v) => format!(
                        "macOS denied access to /Volumes/{v}: give MASON Daemon.app Full Disk Access (System Settings → Privacy & Security → Full Disk Access)"
                    ),
                    None => format!(
                        "access to {shown} was denied: it must be owned and writable by the node's user; if macOS is guarding it, give MASON Daemon.app Full Disk Access"
                    ),
                },
            ),
            PathState::Stalled => (
                false,
                format!(
                    "{shown} did not answer within {} s (a stalled or asleep disk?); keep the node's data on the internal disk",
                    DEADLINE.as_secs()
                ),
            ),
            PathState::NotMounted(v) => (
                false,
                format!(
                    "/Volumes/{v} is not mounted; see AutomountDisksWithoutUserLogin in docs/nodes.md"
                ),
            ),
            PathState::Other(e) => (false, format!("{shown}: {e}")),
        };
        add("path", ok, true, detail);
    }

    let len = i.host_socket.as_os_str().len();
    let fits = check_sock_path(&i.host_socket);
    add(
        "socket path",
        fits.is_ok(),
        true,
        match fits {
            Ok(()) => format!("step host sockets fit ({len} of {MAX_SOCK_PATH} characters)"),
            Err(e) => format!("no job can start: {e}"),
        },
    );

    add(
        "listener",
        i.listener_bound,
        true,
        if i.listener_bound {
            "the node listener is bound".into()
        } else {
            "the node listener is not bound".into()
        },
    );
    match &i.mdns {
        Mdns::Advertising => add("mdns", true, false, advertising_message()),
        Mdns::Off => add("mdns", true, false, "not advertising (switched off)".into()),
        // A node can still be reached by address (`[nodes] static`), so this does not stop it.
        Mdns::Failed(e) => add(
            "mdns",
            false,
            false,
            format!("mDNS is not advertising ({e}): a coordinator must be given this node's address (`mason node add --host`)"),
        ),
    }

    for (tool, r) in &i.tools {
        // A node with no Rust cannot do the job it is for; Xcode and Java matter only to the
        // projects that use them, and the coordinator already sends those only to a node that has
        // them, so they are reported and do not stop the node.
        let blocks = tool == "cargo";
        match r {
            Ok(out) => add(
                tool,
                true,
                blocks,
                format!("{tool}: {}", out.lines().next().unwrap_or("").trim()),
            ),
            Err(why) => add(
                tool,
                false,
                blocks,
                format!(
                    "`{tool}` does not run as this user: {why}{}",
                    if tool == "xcode-select" {
                        " (a new user may need `sudo xcodebuild -license accept` and `sudo xcodebuild -runFirstLaunch`)"
                    } else if tool == "cargo" && why.contains(NO_TOOLCHAIN) {
                        " (rustup has no default toolchain: run `rustup default <version>` as this user, for example `rustup default 1.96.0`)"
                    } else {
                        ""
                    }
                ),
            ),
        }
    }
    Report { at_ms, checks }
}

// ---- reading the signature --------------------------------------------------------------------

/// What `codesign -dr - <file>` and `-dvv` said, as a [`Signature`]. Pure.
pub fn parse_signature(requirement: &str, details: &str) -> Signature {
    if requirement.contains("not signed at all") || details.contains("not signed at all") {
        return Signature::Unsigned;
    }
    let team = details
        .lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && t != "not set");
    if requirement.contains("certificate leaf[subject.OU]")
        || requirement.contains("anchor apple generic")
    {
        return Signature::Team(team);
    }
    if requirement.contains("cdhash") || details.contains("Signature=adhoc") {
        return Signature::AdHoc;
    }
    Signature::Unknown(requirement.lines().next().unwrap_or("").trim().to_string())
}

/// Run a program within `limit` and give back whether it succeeded and what it printed (both
/// streams), or why it did not finish.
pub fn run_status(cmd: &str, args: &[&str], limit: Duration) -> Result<(bool, String), String> {
    let mut command = std::process::Command::new(cmd);
    command.args(args);
    finish_within(command, limit)
}

/// [`run_status`] for a command already set up (its environment and directory).
fn finish_within(
    mut command: std::process::Command,
    limit: Duration,
) -> Result<(bool, String), String> {
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("did not finish in {} s", limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let o = child.wait_with_output().map_err(|e| e.to_string())?;
    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&o.stderr));
    Ok((o.status.success(), text))
}

/// [`run_status`] where a failing exit is an error, with the first line it printed.
pub fn run_deadline(cmd: &str, args: &[&str], limit: Duration) -> Result<String, String> {
    judge(run_status(cmd, args, limit)?)
}

/// What rustup says when `cargo` is there and no toolchain is chosen for it.
const NO_TOOLCHAIN: &str = "could not choose a version";

/// What a tool printed, or the line of it that says why it failed.
fn judge((ok, text): (bool, String)) -> Result<String, String> {
    if ok {
        return Ok(text);
    }
    let lines = || text.trim().lines();
    Err(lines()
        .find(|l| l.contains(NO_TOOLCHAIN))
        .or_else(|| lines().next())
        .unwrap_or("it failed")
        .to_string())
}

/// [`run_deadline`] for a tool as a job would find it: on the `PATH` a job gets
/// ([`job_path`]), from the node user's home, so that no project's `rust-toolchain.toml` is in
/// force.
fn run_as_a_job(
    cmd: &str,
    args: &[&str],
    base: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let home = base("HOME").unwrap_or_default();
    let mut command = std::process::Command::new(cmd);
    command.args(args).env(
        "PATH",
        job_path(None, &home, &base("PATH").unwrap_or_default()),
    );
    for k in [
        "HOME",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "DEVELOPER_DIR",
        "JAVA_HOME",
    ] {
        if let Some(v) = base(k) {
            command.env(k, v);
        }
    }
    if Path::new(&home).is_dir() {
        command.current_dir(&home);
    }
    judge(finish_within(command, DEADLINE)?)
}

/// How `exe` is signed, read with `codesign` (macOS), within the deadline.
pub fn signature_of(exe: &Path) -> Signature {
    signature_of_within(exe, DEADLINE)
}

/// [`signature_of`] with each `codesign` call given `limit`.
pub fn signature_of_within(exe: &Path, limit: Duration) -> Signature {
    if !cfg!(target_os = "macos") {
        return Signature::Unchecked;
    }
    let f = exe.to_string_lossy().into_owned();
    // `codesign` exits non-zero for a file that is not signed, and says so in words: read both.
    let req = run_status("/usr/bin/codesign", &["-dr", "-", &f], limit);
    let det = run_status("/usr/bin/codesign", &["-dvv", &f], limit);
    match (req, det) {
        (Ok((_, r)), Ok((_, d))) => parse_signature(&r, &d),
        (Err(e), _) | (_, Err(e)) => Signature::Unknown(e),
    }
}

/// Which file a path was when it was read: its inode, size and modification time. A signature
/// cannot change while all three are the same.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileId(u64, u64, std::time::SystemTime);

impl FileId {
    pub fn of(path: &Path) -> Option<FileId> {
        let m = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        let ino = {
            use std::os::unix::fs::MetadataExt;
            m.ino()
        };
        // The inode is not read off Unix (std's windows file_index is unstable); size and
        // mtime still identify a replaced binary.
        #[cfg(not(unix))]
        let ino = 0u64;
        Some(FileId(ino, m.len(), m.modified().ok()?))
    }
}

/// The signatures read so far, by path, with the file each was read from.
pub type SignatureCache = std::sync::Mutex<std::collections::HashMap<PathBuf, (FileId, Signature)>>;

/// [`signature_of`] for the node's own executable, asked every ten minutes while `syspolicyd` can
/// keep `codesign` waiting past the deadline. A signature that was read is kept for that file (its
/// inode, size and mtime) and `codesign` is not asked again until the file is a different one;
/// one that could not be read (`Unknown`) is not kept, so the next check asks again. A file that
/// changed is read afresh, so a bad new signature is still refused.
///
/// A first read can find `syspolicyd` so busy that `codesign` never finishes within the
/// deadline, and then nothing would ever be kept. So the read runs on its own thread with a
/// longer limit ([`SLOW_SIGNATURE`]); the check waits the usual deadline for it, and if it is not
/// back by then the check says "could not read" while the read carries on and fills the cache
/// for the next check. One read per binary is in flight at a time.
pub fn signature_cached(exe: &Path, cache: &'static SignatureCache) -> Signature {
    static IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::LazyLock::new(Default::default);
    if let Some(sig) = cached(exe, cache) {
        return sig;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let started = IN_FLIGHT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(exe.to_path_buf());
    if started {
        let path = exe.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("node-signature".into())
            .spawn(move || {
                let sig = signature_cached_with(&path, cache, &|p| {
                    signature_of_within(p, SLOW_SIGNATURE)
                });
                IN_FLIGHT
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&path);
                let _ = tx.send(sig);
            });
        if spawned.is_err() {
            IN_FLIGHT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(exe);
            return signature_cached_with(exe, cache, &signature_of);
        }
        match rx.recv_timeout(2 * DEADLINE) {
            Ok(sig) => sig,
            Err(_) => Signature::Unknown(format!(
                "did not finish in {} s (still reading in the background)",
                2 * DEADLINE.as_secs()
            )),
        }
    } else {
        Signature::Unknown("an earlier read is still waiting on codesign".into())
    }
}

/// How long one background `codesign` call may take in [`signature_cached`].
pub const SLOW_SIGNATURE: Duration = Duration::from_secs(300);

/// The kept signature for `exe`, when the file is still the one it was read from.
fn cached(exe: &Path, cache: &SignatureCache) -> Option<Signature> {
    let id = FileId::of(exe)?;
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(exe)
        .filter(|(was, _)| *was == id)
        .map(|(_, sig)| sig.clone())
}

/// [`signature_cached`] with the reader given: a fake in a test.
pub fn signature_cached_with(
    exe: &Path,
    cache: &SignatureCache,
    read: &dyn Fn(&Path) -> Signature,
) -> Signature {
    let Some(id) = FileId::of(exe) else {
        return read(exe);
    };
    if let Some((_, sig)) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(exe)
        .filter(|(was, _)| *was == id)
    {
        return sig.clone();
    }
    let sig = read(exe);
    if !matches!(sig, Signature::Unknown(_)) {
        cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(exe.to_path_buf(), (id, sig.clone()));
    }
    sig
}

/// The team id `exe` is signed with, when it is signed by a team.
pub fn team_of(exe: &Path) -> Option<String> {
    match signature_of(exe) {
        Signature::Team(t) => t,
        _ => None,
    }
}

// ---- touching the machine --------------------------------------------------------------------

/// Create `dir`, write a probe file in it, read it back and delete it, on a thread that is given
/// up on after `limit`.
pub fn probe_path(dir: &Path, limit: Duration) -> PathState {
    if let Some(v) = volume_of(dir) {
        if !Path::new("/Volumes").join(&v).exists() {
            return PathState::NotMounted(v);
        }
    }
    let (tx, rx) = mpsc::channel();
    let d = dir.to_path_buf();
    std::thread::spawn(move || {
        let r = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(&d)?;
            let f = d.join(format!(".mason-probe-{}", std::process::id()));
            std::fs::write(&f, b"probe")?;
            let back = std::fs::read(&f)?;
            std::fs::remove_file(&f)?;
            if back != b"probe" {
                return Err(std::io::Error::other("what was written did not read back"));
            }
            Ok(())
        })();
        let _ = tx.send(r);
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(())) => PathState::Ok,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::PermissionDenied => PathState::Denied,
        Ok(Err(e)) => PathState::Other(e.to_string()),
        Err(_) => PathState::Stalled,
    }
}

/// Every path the node writes, for the checks: its data directory and what is under it, the
/// build cache and the toolchain homes.
pub fn paths_of(data_dir: &Path, home: &Path, cargo_home: Option<&str>) -> Vec<PathBuf> {
    let mut v = vec![
        data_dir.to_path_buf(),
        data_dir.join("node").join("mirrors"),
        data_dir.join("node").join("work"),
        data_dir.join("runtimes"),
        home.join(".cache").join("mason-target"),
    ];
    if let Some(c) = cargo_home {
        v.push(PathBuf::from(c));
    } else {
        v.push(home.join(".cargo"));
    }
    v.dedup();
    v
}

/// The longest socket path a job's step host can need under `data_dir`.
pub fn longest_host_socket(data_dir: &Path) -> PathBuf {
    crate::node_run::host_socket(data_dir, &"0".repeat(crate::node_run::HOST_KEY_LEN))
}

/// Look at the machine.
pub fn gather(
    exe: &Path,
    data_dir: &Path,
    listener_bound: bool,
    mdns: Mdns,
    allow_unsigned: bool,
) -> Inputs {
    let base = |k: &str| std::env::var(k).ok();
    gather_in(&base, exe, data_dir, listener_bound, mdns, allow_unsigned)
}

/// The signatures this process has read, so the periodic check asks `codesign` once per binary.
static SIGNATURES: std::sync::LazyLock<SignatureCache> = std::sync::LazyLock::new(Default::default);

/// [`gather`] with the environment given: `base` is this process's variables.
pub fn gather_in(
    base: &dyn Fn(&str) -> Option<String>,
    exe: &Path,
    data_dir: &Path,
    listener_bound: bool,
    mdns: Mdns,
    allow_unsigned: bool,
) -> Inputs {
    let home = PathBuf::from(base("HOME").unwrap_or_default());
    let paths = paths_of(data_dir, &home, base("CARGO_HOME").as_deref())
        .into_iter()
        .map(|p| {
            let st = probe_path(&p, DEADLINE);
            (p, st)
        })
        .collect();
    let tool = |cmd: &str, args: &[&str]| run_as_a_job(cmd, args, base);
    let tools = vec![
        ("cargo".to_string(), tool("cargo", &["--version"])),
        (
            "xcode-select".to_string(),
            tool("/usr/bin/xcode-select", &["-p"]),
        ),
        ("java".to_string(), tool("java", &["-version"])),
    ];
    Inputs {
        signature: if allow_unsigned {
            Signature::Unchecked
        } else {
            signature_cached(exe, &SIGNATURES)
        },
        paths,
        host_socket: longest_host_socket(data_dir),
        listener_bound,
        mdns,
        tools,
    }
}

/// What the doctor says about the advertised name. On macOS the system responder answers for
/// the current `LocalHostName`; if `gethostname` (what a long-running process may still hold)
/// differs, say so.
fn advertising_message() -> String {
    let advertised = crate::lan::hostname();
    #[allow(unused_mut)]
    let mut m = format!("advertising over mDNS as {advertised}.local");
    #[cfg(target_os = "macos")]
    if let Some(h) = mason_platform::host::name() {
        let h = h.trim_end_matches(".local");
        if !h.is_empty() && h != advertised {
            m.push_str(&format!(
                " (system LocalHostName is {advertised}, gethostname says {h}: they differ)"
            ));
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Inputs {
        Inputs {
            signature: Signature::Team(Some("ABCDE12345".into())),
            paths: vec![(PathBuf::from("/Users/masonnode/data"), PathState::Ok)],
            host_socket: PathBuf::from("/Users/masonnode/data/node/hosts/0123456789abcdef/sock"),
            listener_bound: true,
            mdns: Mdns::Advertising,
            tools: vec![
                ("cargo".into(), Ok("cargo 1.96.0 (abc)\n".into())),
                (
                    "xcode-select".into(),
                    Ok("/Applications/Xcode.app\n".into()),
                ),
                ("java".into(), Ok("openjdk 21\n".into())),
            ],
        }
    }

    fn problem(i: &Inputs) -> Option<String> {
        evaluate(i, 1).problem()
    }

    #[test]
    fn a_clean_node_has_no_problem() {
        let r = evaluate(&good(), 7);
        assert_eq!(r.problem(), None);
        assert!(r.checks.iter().all(|c| c.ok), "{:?}", r.checks);
        assert_eq!(r.at_ms, 7);
        assert_eq!(r.to_json()["ok"], true);
    }

    #[test]
    fn each_failure_is_a_why_not_the_owner_can_act_on() {
        let mut rows: Vec<(&str, Inputs, &str)> = vec![];
        let mut i = good();
        i.signature = Signature::AdHoc;
        rows.push((
            "ad hoc",
            i,
            "masond is signed ad hoc; privacy grants will not hold",
        ));
        let mut i = good();
        i.signature = Signature::Unsigned;
        rows.push(("unsigned", i, "masond is not signed"));
        let mut i = good();
        i.signature = Signature::Unknown("codesign did not finish in 10 s".into());
        rows.push(("unknown", i, "could not read masond's signature"));
        let mut i = good();
        i.paths
            .push((PathBuf::from("/Volumes/X/work"), PathState::Denied));
        rows.push((
            "volume denied",
            i,
            "macOS denied access to /Volumes/X: give MASON Daemon.app Full Disk Access",
        ));
        let mut i = good();
        i.paths.push((
            PathBuf::from("/Volumes/X/work"),
            PathState::NotMounted("X".into()),
        ));
        rows.push((
            "not mounted",
            i,
            "/Volumes/X is not mounted; see AutomountDisksWithoutUserLogin",
        ));
        let mut i = good();
        i.paths
            .push((PathBuf::from("/Volumes/X/work"), PathState::Stalled));
        rows.push(("stalled", i, "did not answer within 10 s"));
        let mut i = good();
        i.paths
            .push((PathBuf::from("/Users/masonnode/x"), PathState::Denied));
        rows.push(("home denied", i, "access to /Users/masonnode/x was denied"));
        let mut i = good();
        i.paths.push((
            PathBuf::from("/Users/masonnode/x"),
            PathState::Other("No space left".into()),
        ));
        rows.push(("other", i, "/Users/masonnode/x: No space left"));
        let mut i = good();
        i.listener_bound = false;
        rows.push(("listener", i, "the node listener is not bound"));
        let mut i = good();
        i.tools[0].1 = Err("No such file or directory".into());
        rows.push(("no cargo", i, "`cargo` does not run as this user"));
        let mut i = good();
        i.tools[0].1 = Err("error: rustup could not choose a version of cargo to run".into());
        rows.push(("no default toolchain", i, "run `rustup default <version>`"));
        for (name, i, want) in rows {
            let p = problem(&i).unwrap_or_else(|| panic!("{name}: no problem"));
            assert!(p.contains(want), "{name}: {p}");
        }
    }

    #[test]
    fn what_only_matters_to_some_projects_is_reported_and_does_not_stop_the_node() {
        let mut i = good();
        i.mdns = Mdns::Failed("no multicast".into());
        i.tools[1].1 = Err("no developer tools".into());
        i.tools[2].1 = Err("not installed".into());
        let r = evaluate(&i, 1);
        assert_eq!(r.problem(), None, "{:?}", r.checks);
        let failed: Vec<&str> = r
            .checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(failed, ["mdns", "xcode-select", "java"]);
        let xc = r.checks.iter().find(|c| c.name == "xcode-select").unwrap();
        assert!(
            xc.detail.contains("sudo xcodebuild -license accept"),
            "{}",
            xc.detail
        );
        assert_eq!(r.to_json()["ok"], true);
        // An unsigned development node, said to be allowed, is not judged by its signature.
        let mut i = good();
        i.signature = Signature::Unchecked;
        assert_eq!(problem(&i), None);
    }

    #[test]
    fn a_socket_path_of_101_characters_is_refused_with_its_length() {
        let mut i = good();
        i.host_socket = PathBuf::from(format!("/{}", "a".repeat(99)));
        assert_eq!(problem(&i), None, "100 characters fit");
        i.host_socket = PathBuf::from(format!("/{}", "a".repeat(100)));
        let p = problem(&i).expect("101 characters do not fit");
        assert!(p.contains("101 characters"), "{p}");
        assert!(p.contains(&"a".repeat(100)), "{p}");
    }

    #[test]
    fn the_first_blocking_failure_is_the_one_told() {
        let mut i = good();
        i.signature = Signature::AdHoc;
        i.listener_bound = false;
        assert!(problem(&i).unwrap().contains("signed ad hoc"));
    }

    #[test]
    fn a_volume_is_named_from_its_path() {
        assert_eq!(
            volume_of(Path::new("/Volumes/sp/git/x")).as_deref(),
            Some("sp")
        );
        assert_eq!(volume_of(Path::new("/Volumes/sp")).as_deref(), Some("sp"));
        assert_eq!(volume_of(Path::new("/Volumes")), None);
        assert_eq!(volume_of(Path::new("/Users/me")), None);
        assert_eq!(volume_of(Path::new("relative/Volumes/x")), None);
    }

    fn temp_exe(tag: &str, body: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("sigcache-{}-{tag}", std::process::id()));
        std::fs::write(&p, body).unwrap();
        p
    }

    fn timed_out() -> Signature {
        Signature::Unknown("did not finish in 10 s".into())
    }

    #[test]
    fn a_read_signature_is_kept_for_the_same_file_and_survives_a_timeout() {
        let exe = temp_exe("keep", b"one");
        let c = SignatureCache::default();
        let team = Signature::Team(Some("ABCDE12345".into()));
        let t = team.clone();
        assert_eq!(signature_cached_with(&exe, &c, &move |_| t.clone()), team);
        // codesign is not asked again, so a busy syspolicyd cannot matter.
        let again = signature_cached_with(&exe, &c, &|_| panic!("asked again"));
        assert_eq!(again, team);
        let _ = std::fs::remove_file(&exe);
    }

    #[test]
    fn a_first_time_timeout_is_not_kept_and_is_asked_again() {
        let exe = temp_exe("first", b"one");
        let c = SignatureCache::default();
        assert_eq!(
            signature_cached_with(&exe, &c, &|_| timed_out()),
            timed_out()
        );
        assert!(problem(&Inputs {
            signature: timed_out(),
            ..good()
        })
        .unwrap()
        .contains("could not read masond's signature"));
        let team = Signature::Team(None);
        let t = team.clone();
        assert_eq!(signature_cached_with(&exe, &c, &move |_| t.clone()), team);
        let _ = std::fs::remove_file(&exe);
    }

    #[test]
    fn a_changed_binary_is_read_again_and_a_bad_signature_is_refused() {
        let exe = temp_exe("changed", b"one");
        let c = SignatureCache::default();
        let team = Signature::Team(Some("ABCDE12345".into()));
        let t = team.clone();
        signature_cached_with(&exe, &c, &move |_| t.clone());
        // A different size is a different file, whatever the clock says.
        std::fs::write(&exe, b"another, longer").unwrap();
        let got = signature_cached_with(&exe, &c, &|_| Signature::AdHoc);
        assert_eq!(got, Signature::AdHoc);
        assert!(problem(&Inputs {
            signature: got,
            ..good()
        })
        .unwrap()
        .contains("signed ad hoc"));
        // And one that then cannot be read is not taken for the old good one.
        std::fs::write(&exe, b"a third, longer still").unwrap();
        assert_eq!(
            signature_cached_with(&exe, &c, &|_| timed_out()),
            timed_out()
        );
        let _ = std::fs::remove_file(&exe);
    }

    #[test]
    fn codesign_output_is_read_as_a_team_ad_hoc_or_unsigned() {
        let team_req = "designated => anchor apple generic and identifier \"com.brianm998.mason.masond\" and certificate leaf[subject.OU] = ABCDE12345\n";
        let team_det = "Identifier=com.brianm998.mason.masond\nTeamIdentifier=ABCDE12345\n";
        assert_eq!(
            parse_signature(team_req, team_det),
            Signature::Team(Some("ABCDE12345".into()))
        );
        let adhoc_req = "designated => cdhash H\"abcdef\"\n";
        assert_eq!(
            parse_signature(adhoc_req, "Signature=adhoc\nTeamIdentifier=not set\n"),
            Signature::AdHoc
        );
        assert_eq!(
            parse_signature("/x: code object is not signed at all\n", ""),
            Signature::Unsigned
        );
        assert!(matches!(
            parse_signature("something else\n", ""),
            Signature::Unknown(_)
        ));
        // A team requirement whose id the details do not give is still a team signature.
        assert_eq!(
            parse_signature(team_req, "TeamIdentifier=not set\n"),
            Signature::Team(None)
        );
    }

    #[test]
    fn a_path_is_probed_by_writing_reading_and_deleting() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("a/b");
        assert_eq!(probe_path(&d, Duration::from_secs(10)), PathState::Ok);
        assert!(d.is_dir());
        assert_eq!(
            std::fs::read_dir(&d).unwrap().count(),
            0,
            "the probe file is gone"
        );
        // A path under a volume that is not there is not mounted, whatever its owner.
        assert_eq!(
            probe_path(
                Path::new("/Volumes/mason-no-such-volume-xyz/x"),
                Duration::from_secs(10)
            ),
            PathState::NotMounted("mason-no-such-volume-xyz".into())
        );
        // A file where a directory should be is another failure, said as it is.
        let f = t.path().join("file");
        std::fs::write(&f, "x").unwrap();
        assert!(matches!(
            probe_path(&f.join("sub"), Duration::from_secs(10)),
            PathState::Other(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_may_not_be_written_is_denied() {
        use std::os::unix::fs::PermissionsExt;
        // Root writes anywhere; the check means nothing there.
        if std::env::var("USER").is_ok_and(|u| u == "root") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("locked");
        std::fs::create_dir(&d).unwrap();
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o500)).unwrap();
        let r = probe_path(&d, Duration::from_secs(10));
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(r, PathState::Denied);
    }

    #[test]
    fn a_tool_that_hangs_is_reported_not_waited_for() {
        let r = run_deadline("/bin/sleep", &["5"], Duration::from_millis(200));
        assert!(r.unwrap_err().contains("did not finish"));
        assert!(run_deadline("/nonexistent/tool", &[], Duration::from_secs(1)).is_err());
        assert_eq!(
            run_deadline("/bin/echo", &["hi"], Duration::from_secs(5))
                .unwrap()
                .trim(),
            "hi"
        );
    }
}
