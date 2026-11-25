// SPDX-License-Identifier: GPL-3.0-or-later

//! The signed runtime a node runs (`docs/multi-machine-nodes-plan.md`, §3.14).
//!
//! **A node never builds or signs MASON itself.** The Developer ID private key is in the owner's
//! keychain on the coordinator, which a node's own macOS user cannot reach, and an ad-hoc
//! signature would change with every build and so drop every privacy grant, with nobody logged
//! in to answer the prompts again. So the coordinator builds for the node's architecture, signs
//! with `sign_identity` (`update::sign_installed`, the code that signs its own runtimes) and sends
//! the result; the node checks it against the team id it pinned when it paired, installs it under
//! `runtimes/`, and takes over once its running jobs are done.
//!
//! The package is one file: the runtime's files with their modes, flat, no archive format of
//! anybody else's (a node reads no `tar` that a peer made).

use crate::node_doctor::{parse_signature, run_status, Signature};
use crate::node_exec::plain_relative;
use crate::node_run::valid_sha;
use crate::update::{shell, sign_installed, Runtimes, UpdateCfg};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAGIC: &[u8; 4] = b"MNR1";
/// Most files a package may carry.
const MAX_FILES: usize = 64;
/// Most bytes a package may carry.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One file of a runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// A plain relative path: `masond`, `step-host`, `SHA`.
    pub path: String,
    pub mode: u32,
    pub bytes: Vec<u8>,
}

/// The package: the runtime's commit and its files.
pub fn pack(sha: &str, files: &[File]) -> Vec<u8> {
    let mut out = Vec::with_capacity(files.iter().map(|f| f.bytes.len() + 64).sum::<usize>() + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(sha.len() as u16).to_be_bytes());
    out.extend_from_slice(sha.as_bytes());
    out.extend_from_slice(&(files.len() as u32).to_be_bytes());
    for f in files {
        out.extend_from_slice(&(f.path.len() as u16).to_be_bytes());
        out.extend_from_slice(f.path.as_bytes());
        out.extend_from_slice(&f.mode.to_be_bytes());
        out.extend_from_slice(&(f.bytes.len() as u64).to_be_bytes());
        out.extend_from_slice(&f.bytes);
    }
    out
}

/// A cursor over the package that never reads past its end.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("the package is cut short".into());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u16(&mut self) -> Result<usize, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap_or([0; 2])) as usize)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().unwrap_or([0; 4]),
        ))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().unwrap_or([0; 8]),
        ))
    }
}

/// Read a package. Every name is a plain relative path, the sha a full commit id, and the sizes
/// are bounded, whatever the bytes say.
pub fn unpack(b: &[u8]) -> Result<(String, Vec<File>), String> {
    let mut r = Reader(b);
    if r.take(4)? != MAGIC {
        return Err("that is not a MASON runtime package".into());
    }
    let n = r.u16()?;
    let sha = String::from_utf8(r.take(n)?.to_vec()).map_err(|_| "the commit is not text")?;
    if !valid_sha(&sha) {
        return Err("the package does not name a full commit".into());
    }
    let count = r.u32()? as usize;
    if count == 0 || count > MAX_FILES {
        return Err(format!("a package has 1 to {MAX_FILES} files, not {count}"));
    }
    let mut files = Vec::with_capacity(count);
    let mut total = 0u64;
    for _ in 0..count {
        let n = r.u16()?;
        let path = String::from_utf8(r.take(n)?.to_vec()).map_err(|_| "a name is not text")?;
        if plain_relative(&path).is_none() || path.contains('/') {
            return Err(format!("{path:?} is not a plain file name"));
        }
        let mode = r.u32()?;
        let size = r.u64()?;
        total = total.saturating_add(size);
        if total > MAX_BYTES {
            return Err("the package is too large".into());
        }
        let size = usize::try_from(size).map_err(|_| "a file is too large")?;
        files.push(File {
            path,
            mode,
            bytes: r.take(size)?.to_vec(),
        });
    }
    if files
        .iter()
        .map(|f| &f.path)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != files.len()
    {
        return Err("a name is in the package twice".into());
    }
    Ok((sha, files))
}

/// The target triple of an architecture the owner names (`arm64` as `uname` says it, or
/// `aarch64`, or `x86_64`).
pub fn triple_for(arch: &str) -> Option<&'static str> {
    match arch {
        "arm64" | "aarch64" => Some("aarch64-apple-darwin"),
        "x86_64" | "x86-64" | "amd64" => Some("x86_64-apple-darwin"),
        _ => None,
    }
}

/// This machine's architecture, as a node reports it.
pub fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        a => a,
    }
}

/// `target/release` as a cross build leaves it: `target/<triple>/release`.
pub fn cross_out_dir(out_dir: &str, triple: &str) -> Result<String, String> {
    match out_dir.strip_prefix("target/") {
        Some(rest) if !rest.is_empty() => Ok(format!("target/{triple}/{rest}")),
        _ => Err(format!(
            "update.toml out_dir {out_dir:?} does not start with `target/`: a cross build cannot say where it leaves its files"
        )),
    }
}

// ---- checking a package -------------------------------------------------------------------------

/// Is this signature the team's? `Err` says why not, in words.
pub fn check_signature(requirement: &str, details: &str, team: &str) -> Result<(), String> {
    match parse_signature(requirement, details) {
        Signature::Team(Some(t)) if t == team => Ok(()),
        Signature::Team(Some(t)) => Err(format!(
            "it is signed by team {t}, not by team {team}, which this node paired with"
        )),
        Signature::Team(None) => Err("it is signed by a team that is not named".into()),
        Signature::AdHoc => Err("it is signed ad hoc".into()),
        Signature::Unsigned => Err("it is not signed".into()),
        Signature::Unknown(w) => Err(format!("its signature is not understood ({w})")),
        Signature::Unchecked => Err("its signature could not be checked".into()),
    }
}

/// What `codesign` gives back for a list of arguments: `(success, text)`. A fake one in a test.
pub type Codesign<'a> = &'a dyn Fn(&[&str]) -> Result<(bool, String), String>;

/// The real `codesign`.
pub fn real_codesign(args: &[&str]) -> Result<(bool, String), String> {
    run_status("/usr/bin/codesign", args, Duration::from_secs(120))
}

/// Check every executable of a package under `dir`: it verifies (`--verify --strict`) and it is
/// signed by `team`. A package that is not, from anyone, is refused.
pub fn verify_dir(dir: &Path, files: &[File], team: &str, cs: Codesign<'_>) -> Result<(), String> {
    for f in files.iter().filter(|f| f.mode & 0o111 != 0) {
        let p = dir.join(&f.path).to_string_lossy().into_owned();
        let (ok, text) = cs(&["--verify", "--strict", &p])?;
        if !ok {
            return Err(format!(
                "{} does not verify: {}",
                f.path,
                text.trim().lines().next().unwrap_or("")
            ));
        }
        let (_, req) = cs(&["-dr", "-", &p])?;
        let (_, det) = cs(&["-dvv", &p])?;
        check_signature(&req, &det, team).map_err(|e| format!("{}: {e}", f.path))?;
    }
    Ok(())
}

/// Install a package as `<data>/runtimes/<sha>/`: written aside, verified, marked ready, then
/// renamed into place, so a runtime is complete or absent. An installed one is left alone.
pub fn install(
    data_dir: &Path,
    sha: &str,
    files: &[File],
    team: &str,
    cs: Codesign<'_>,
) -> Result<PathBuf, String> {
    let rt = Runtimes::new(data_dir);
    if rt.is_ready(sha) {
        return Ok(rt.dir(sha));
    }
    let partial = rt.root.join(format!("{sha}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
    let r = (|| -> Result<(), String> {
        for f in files {
            let p = partial.join(&f.path);
            std::fs::write(&p, &f.bytes).map_err(|e| format!("{}: {e}", f.path))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = if f.mode & 0o111 != 0 { 0o755 } else { 0o644 };
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))
                    .map_err(|e| e.to_string())?;
            }
        }
        for need in ["masond", "step-host"] {
            if !files.iter().any(|f| f.path == need) {
                return Err(format!("the package has no {need}"));
            }
        }
        verify_dir(&partial, files, team, cs)?;
        std::fs::write(partial.join("SHA"), sha).map_err(|e| e.to_string())?;
        std::fs::write(partial.join("READY"), "").map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(rt.dir(sha));
    std::fs::rename(&partial, rt.dir(sha)).map_err(|e| e.to_string())?;
    Ok(rt.dir(sha))
}

// ---- making a package ---------------------------------------------------------------------------

/// The files of a runtime directory, flat (the binaries and the marks; a subdirectory such as
/// the app is not part of an update).
pub fn read_dir_files(dir: &Path) -> Result<Vec<File>, String> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
    {
        let m = e.metadata().map_err(|e| e.to_string())?;
        if !m.is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "READY" {
            continue;
        }
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            m.permissions().mode()
        };
        #[cfg(not(unix))]
        let mode = 0o644;
        out.push(File {
            path: name,
            mode,
            bytes: std::fs::read(e.path()).map_err(|e| e.to_string())?,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The runtime this daemon is on, for a node of architecture `arch`: its own when the architecture
/// is this machine's, else a cross build of the same commit, signed with `sign_identity`. Returns
/// the commit and the directory.
pub fn runtime_for(
    cfg: &UpdateCfg,
    rt: &Runtimes,
    arch: &str,
) -> Result<(String, PathBuf), String> {
    let sha = rt
        .current()
        .ok_or("this daemon is not running from a runtime (`mason update` first): there is nothing to give a node")?;
    let triple = triple_for(arch)
        .ok_or_else(|| format!("{arch:?} is not an architecture: arm64 or x86_64"))?;
    if cfg.sign_identity.is_none() {
        return Err("update.toml has no `sign_identity`: a node takes only a runtime signed by a team, since an ad-hoc signature drops its privacy grants at every update".into());
    }
    if arch_matches_host(arch) {
        return Ok((sha.clone(), rt.dir(&sha)));
    }
    Ok((sha.clone(), cross_build(cfg, rt, &sha, triple)?))
}

fn arch_matches_host(arch: &str) -> bool {
    matches!(
        (arch, host_arch()),
        ("arm64" | "aarch64", "arm64") | ("x86_64" | "x86-64" | "amd64", "x86_64")
    )
}

/// Build `sha` for `triple` into `<runtimes>/node/<triple>/<sha>/`, sign every binary with
/// `sign_identity`, and make "MASON Daemon.app" of its `masond` in `app/` beside them. Safe to
/// repeat: a finished one is left alone.
pub fn cross_build(
    cfg: &UpdateCfg,
    rt: &Runtimes,
    sha: &str,
    triple: &str,
) -> Result<PathBuf, String> {
    let done = rt.root.join("node").join(triple).join(sha);
    if done.join("READY").exists() {
        return Ok(done);
    }
    let out_rel = cross_out_dir(&cfg.out_dir, triple)?;
    let src = rt.root.join("src").join(format!("{sha}-{triple}"));
    let _ = std::fs::remove_dir_all(&src);
    std::fs::create_dir_all(&src).map_err(|e| e.to_string())?;
    let log = rt.root.join(format!("{sha}-{triple}.log"));
    shell(
        &rt.root,
        &format!(
            "git --git-dir='{}' archive {sha} | tar -x -C '{}'",
            rt.mirror().display(),
            src.display()
        ),
        &log,
        &[],
    )?;
    shell(&src, &format!("rustup target add {triple}"), &log, &[])?;
    let jobs = crate::update::update_build_jobs(
        std::thread::available_parallelism().map_or(8, |n| n.get()),
        std::env::var("MASON_UPDATE_BUILD_JOBS").ok().as_deref(),
    );
    shell(
        &src,
        &format!("{} --target {triple}", cfg.build),
        &log,
        &[("CARGO_BUILD_JOBS", &jobs.to_string())],
    )?;
    let partial = done.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
    for b in &cfg.bins {
        let from = src.join(&out_rel).join(b);
        if !from.is_file() {
            if b == "masond" || b == "step-host" || b == "mason-mcp" {
                return Err(format!("the build did not produce {}", from.display()));
            }
            continue;
        }
        std::fs::copy(&from, partial.join(b)).map_err(|e| format!("copying {b}: {e}"))?;
        sign_installed(&partial.join(b), cfg.sign_identity.as_deref())?;
    }
    std::fs::write(partial.join("SHA"), sha).map_err(|e| e.to_string())?;
    // The app a first install needs, made from the signed launcher.
    let app_dir = partial.join("app");
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    let script = src.join("scripts").join("make-daemon-app.sh");
    if script.is_file() {
        let masond = partial.join("masond");
        shell(
            &src,
            &format!(
                "bash '{}' '{}' '{}' '{}'",
                script.display(),
                masond.display(),
                app_dir.display(),
                cfg.sign_identity.as_deref().unwrap_or("")
            ),
            &log,
            &[],
        )?;
    }
    std::fs::write(partial.join("READY"), "").map_err(|e| e.to_string())?;
    if let Some(parent) = done.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&done);
    std::fs::rename(&partial, &done).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&src);
    Ok(done)
}

/// The node user's `mason`: sets the `PATH` of docs/nodes.md and runs the current runtime's CLI,
/// which each update moves.
pub const WRAPPER: &str = r#"#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Installed as ~/.local/bin/mason on a MASON node (docs/nodes.md).
export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
runtimes="$HOME/Library/Application Support/mason/runtimes"
exec "$runtimes/$(cat "$runtimes/current")/mason" "$@"
"#;

/// Write [`WRAPPER`] as `out/mason`, executable.
fn write_wrapper(out: &Path) -> Result<(), String> {
    let p = out.join("mason");
    std::fs::write(&p, WRAPPER).map_err(|e| format!("{}: {e}", p.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

/// `mason node export-runtime`: write the package for a node of `arch` into `out`: `runtimes/<sha>/`
/// (what an update sends), and what a first install needs, once: `MASON Daemon.app` and the node
/// user's `mason` wrapper. Returns the commit.
pub fn export(cfg: &UpdateCfg, rt: &Runtimes, arch: &str, out: &Path) -> Result<String, String> {
    let (sha, dir) = runtime_for(cfg, rt, arch)?;
    let target = out.join("runtimes").join(&sha);
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    for f in read_dir_files(&dir)? {
        let p = target.join(&f.path);
        std::fs::write(&p, &f.bytes).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(f.mode & 0o777))
                .map_err(|e| e.to_string())?;
        }
    }
    std::fs::write(target.join("READY"), "").map_err(|e| e.to_string())?;
    write_wrapper(out)?;
    let app = dir.join("app").join("MASON Daemon.app");
    if app.is_dir() {
        shell(
            out,
            &format!(
                "rm -rf 'MASON Daemon.app' && cp -R '{}' 'MASON Daemon.app'",
                app.display()
            ),
            &out.join("export.log"),
            &[],
        )?;
    } else {
        // This machine's own runtime has no app made yet: wrap its launcher, with the script of
        // the same commit.
        let script = out.join(".make-daemon-app.sh");
        shell(
            out,
            &format!(
                "git --git-dir='{}' show {sha}:scripts/make-daemon-app.sh > '{}' && bash '{}' '{}' '{}' '{}'",
                rt.mirror().display(),
                script.display(),
                script.display(),
                dir.join("masond").display(),
                out.display(),
                cfg.sign_identity.as_deref().unwrap_or("")
            ),
            &out.join("export.log"),
            &[],
        )?;
        let _ = std::fs::remove_file(script);
    }
    Ok(sha)
}

/// The whole package for a node of `arch`, to send.
pub fn package_for(
    cfg: &UpdateCfg,
    rt: &Runtimes,
    arch: &str,
) -> Result<(String, Vec<u8>), String> {
    let (sha, dir) = runtime_for(cfg, rt, arch)?;
    let files = read_dir_files(&dir)?;
    Ok((sha.clone(), pack(&sha, &files)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn files() -> Vec<File> {
        vec![
            File {
                path: "masond".into(),
                mode: 0o755,
                bytes: b"MACH-O masond".to_vec(),
            },
            File {
                path: "step-host".into(),
                mode: 0o755,
                bytes: b"MACH-O host".to_vec(),
            },
            File {
                path: "SHA".into(),
                mode: 0o644,
                bytes: SHA.as_bytes().to_vec(),
            },
        ]
    }

    #[cfg(unix)]
    #[test]
    fn the_wrapper_runs_the_current_runtimes_mason_with_the_nodes_path() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        write_wrapper(t.path()).unwrap();
        let rt = t.path().join("Library/Application Support/mason/runtimes");
        std::fs::create_dir_all(rt.join(SHA)).unwrap();
        std::fs::write(rt.join("current"), SHA).unwrap();
        let mason = rt.join(SHA).join("mason");
        std::fs::write(&mason, "#!/bin/sh\necho \"$PATH|$1|$2\"\n").unwrap();
        std::fs::set_permissions(&mason, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Through `sh`, so that the wrapper just written is never executed directly.
        let o = std::process::Command::new("/bin/sh")
            .arg(t.path().join("mason"))
            .args(["node", "two words"])
            .env_clear()
            .env("HOME", t.path())
            .output()
            .unwrap();
        assert!(o.status.success(), "{o:?}");
        assert_eq!(
            String::from_utf8_lossy(&o.stdout).trim(),
            format!(
                "{}/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin|node|two words",
                t.path().display()
            )
        );
    }

    #[test]
    fn a_package_round_trips() {
        let b = pack(SHA, &files());
        let (sha, got) = unpack(&b).unwrap();
        assert_eq!(sha, SHA);
        assert_eq!(got, files());
    }

    #[test]
    fn a_package_that_is_wrong_is_refused_not_trusted() {
        let good = pack(SHA, &files());
        assert!(unpack(b"").is_err());
        assert!(unpack(b"TAR!....")
            .unwrap_err()
            .contains("not a MASON runtime package"));
        for cut in [5, 20, good.len() - 1] {
            assert!(unpack(&good[..cut]).is_err(), "cut at {cut}");
        }
        let bad_sha = pack("not-a-commit", &files());
        assert!(unpack(&bad_sha).unwrap_err().contains("full commit"));
        for name in ["../escape", "/abs", "a/b", "", ".."] {
            let f = vec![File {
                path: name.into(),
                mode: 0o644,
                bytes: vec![1],
            }];
            let e = unpack(&pack(SHA, &f)).unwrap_err();
            assert!(e.contains("plain file name"), "{name:?}: {e}");
        }
        let twice = vec![files()[0].clone(), files()[0].clone()];
        assert!(unpack(&pack(SHA, &twice)).unwrap_err().contains("twice"));
        assert!(unpack(&pack(SHA, &[])).is_err());
        // A size that claims more than there is cannot read past the end.
        let mut lie = pack(SHA, &files()[..1]);
        let at = lie.len() - b"MACH-O masond".len() - 8;
        lie[at..at + 8].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(unpack(&lie).is_err());
    }

    #[test]
    fn architectures_and_cross_build_directories() {
        assert_eq!(triple_for("arm64"), Some("aarch64-apple-darwin"));
        assert_eq!(triple_for("aarch64"), Some("aarch64-apple-darwin"));
        assert_eq!(triple_for("x86_64"), Some("x86_64-apple-darwin"));
        assert_eq!(triple_for("riscv"), None);
        assert_eq!(
            cross_out_dir("target/release", "aarch64-apple-darwin").unwrap(),
            "target/aarch64-apple-darwin/release"
        );
        assert!(cross_out_dir("out/release", "x").is_err());
        assert!(cross_out_dir("target/", "x").is_err());
    }

    /// A `codesign` that says what a fixed table says, for each file name.
    struct Fake(BTreeMap<String, (bool, &'static str, &'static str)>);

    impl Fake {
        fn run(&self, args: &[&str]) -> Result<(bool, String), String> {
            let file = args.last().copied().unwrap_or("");
            let name = file.rsplit('/').next().unwrap_or("");
            let Some((ok, req, det)) = self.0.get(name) else {
                return Err(format!("no such file {name}"));
            };
            Ok(match args[0] {
                "--verify" => (
                    *ok,
                    if *ok {
                        String::new()
                    } else {
                        format!("{file}: a sealed resource is missing")
                    },
                ),
                "-dr" => (true, req.to_string()),
                _ => (true, det.to_string()),
            })
        }
    }

    const TEAM_REQ: &str = "designated => anchor apple generic and identifier \"com.brianm998.mason.masond\" and certificate leaf[subject.OU] = TEAM111111\n";
    const TEAM_DET: &str = "TeamIdentifier=TEAM111111\n";
    const OTHER_DET: &str = "TeamIdentifier=OTHER99999\n";
    const OTHER_REQ: &str =
        "designated => anchor apple generic and certificate leaf[subject.OU] = OTHER99999\n";
    const ADHOC_REQ: &str = "designated => cdhash H\"abc\"\n";

    fn fake(
        masond: (bool, &'static str, &'static str),
        host: (bool, &'static str, &'static str),
    ) -> Fake {
        Fake(BTreeMap::from([
            ("masond".to_string(), masond),
            ("step-host".to_string(), host),
        ]))
    }

    #[test]
    fn a_package_signed_by_the_pinned_team_verifies() {
        let t = tempfile::tempdir().unwrap();
        let f = fake((true, TEAM_REQ, TEAM_DET), (true, TEAM_REQ, TEAM_DET));
        verify_dir(t.path(), &files(), "TEAM111111", &|a| f.run(a)).unwrap();
    }

    #[test]
    fn an_ad_hoc_package_a_package_of_another_team_and_a_broken_seal_are_each_refused() {
        let t = tempfile::tempdir().unwrap();
        let cases: Vec<(&str, Fake, &str)> = vec![
            (
                "ad hoc",
                fake(
                    (true, ADHOC_REQ, "Signature=adhoc\n"),
                    (true, TEAM_REQ, TEAM_DET),
                ),
                "masond: it is signed ad hoc",
            ),
            (
                "another team",
                fake((true, TEAM_REQ, TEAM_DET), (true, OTHER_REQ, OTHER_DET)),
                "step-host: it is signed by team OTHER99999, not by team TEAM111111",
            ),
            (
                "unsigned",
                fake(
                    (true, "x: code object is not signed at all\n", ""),
                    (true, TEAM_REQ, TEAM_DET),
                ),
                "masond: it is not signed",
            ),
            (
                "broken seal",
                fake((false, TEAM_REQ, TEAM_DET), (true, TEAM_REQ, TEAM_DET)),
                "masond does not verify",
            ),
        ];
        for (name, f, want) in cases {
            let e = verify_dir(t.path(), &files(), "TEAM111111", &|a| f.run(a)).unwrap_err();
            assert!(e.contains(want), "{name}: {e}");
        }
    }

    #[test]
    fn installing_is_all_or_nothing_and_leaves_an_installed_runtime_alone() {
        let data = tempfile::tempdir().unwrap();
        let good = fake((true, TEAM_REQ, TEAM_DET), (true, TEAM_REQ, TEAM_DET));
        let bad = fake(
            (true, ADHOC_REQ, "Signature=adhoc\n"),
            (true, TEAM_REQ, TEAM_DET),
        );
        let rt = Runtimes::new(data.path());
        // Refused: nothing is left, partial or whole.
        let e = install(data.path(), SHA, &files(), "TEAM111111", &|a| bad.run(a)).unwrap_err();
        assert!(e.contains("signed ad hoc"), "{e}");
        assert!(!rt.dir(SHA).exists() && !rt.root.join(format!("{SHA}.partial")).exists());
        // Accepted: complete, with its marks, and the files executable.
        let dir = install(data.path(), SHA, &files(), "TEAM111111", &|a| good.run(a)).unwrap();
        assert!(rt.is_ready(SHA));
        assert_eq!(std::fs::read_to_string(dir.join("SHA")).unwrap(), SHA);
        assert_eq!(std::fs::read(dir.join("masond")).unwrap(), b"MACH-O masond");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.join("masond"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        // Again: left alone, and not even asked to verify.
        let never = |_: &[&str]| -> Result<(bool, String), String> { panic!("verified again") };
        install(data.path(), SHA, &files(), "TEAM111111", &never).unwrap();
        // A package without the daemon is no runtime.
        let sha2 = "1111111111111111111111111111111111111111";
        let e = install(data.path(), sha2, &files()[1..], "TEAM111111", &|a| {
            good.run(a)
        })
        .unwrap_err();
        assert!(e.contains("no masond"), "{e}");
    }

    #[test]
    fn the_cross_built_binaries_are_signed_by_the_same_arguments_as_the_runtimes_own() {
        let a = crate::update::codesign_args(
            Path::new("/r/node/aarch64-apple-darwin/abc/masond"),
            Some("Developer ID Application: T (TEAM111111)"),
        );
        let a: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert_eq!(
            a,
            [
                "--sign",
                "Developer ID Application: T (TEAM111111)",
                "--options",
                "runtime",
                "--force",
                "--identifier",
                "com.brianm998.mason.masond",
                "/r/node/aarch64-apple-darwin/abc/masond"
            ]
        );
    }

    #[test]
    fn a_runtime_directory_is_read_flat_without_its_ready_mark_or_its_app() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("masond"), "m").unwrap();
        std::fs::write(d.path().join("SHA"), SHA).unwrap();
        std::fs::write(d.path().join("READY"), "").unwrap();
        std::fs::create_dir_all(d.path().join("app/MASON Daemon.app")).unwrap();
        let got: Vec<String> = read_dir_files(d.path())
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(got, ["SHA", "masond"]);
    }

    #[test]
    fn a_node_is_given_only_a_signed_runtime_of_a_known_architecture() {
        let d = tempfile::tempdir().unwrap();
        let rt = Runtimes::new(d.path());
        let cfg = |sign: bool| {
            UpdateCfg::parse(&format!(
                "repo = \"/x\"\n{}",
                if sign {
                    "sign_identity = \"Developer ID Application: T (TEAM111111)\"\n"
                } else {
                    ""
                }
            ))
            .unwrap()
        };
        // Not running from a runtime: nothing to give.
        assert!(runtime_for(&cfg(true), &rt, "arm64")
            .unwrap_err()
            .contains("not running from a runtime"));
        std::fs::create_dir_all(rt.dir(SHA)).unwrap();
        std::fs::write(rt.dir(SHA).join("READY"), "").unwrap();
        rt.set_current(SHA).unwrap();
        assert!(runtime_for(&cfg(true), &rt, "riscv")
            .unwrap_err()
            .contains("not an architecture"));
        assert!(runtime_for(&cfg(false), &rt, "arm64")
            .unwrap_err()
            .contains("no `sign_identity`"));
        // This machine's own architecture is its own runtime, already signed by `build`.
        let (sha, dir) = runtime_for(&cfg(true), &rt, host_arch()).unwrap();
        assert_eq!((sha.as_str(), dir), (SHA, rt.dir(SHA)));
    }
}
