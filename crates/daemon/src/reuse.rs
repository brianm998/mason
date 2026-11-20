// SPDX-License-Identifier: GPL-3.0-or-later
//! Not running a cargo test target again when what it was built from is unchanged since it last
//! passed (s707, decision 0700).
//!
//! The input key of a target is a digest of everything it can read, found without running it:
//!
//! * the files cargo's dep-info lists for its executable (`target/*/deps/<name>-<hash>.d`: every
//!   source, `include_str!` file and generated file), and the same for the library of every
//!   workspace package in its dependency closure, because the dep-info of an executable names
//!   only its own crate's files;
//! * the manifest and `build.rs` of each package in the closure, and every tracked or untracked
//!   file outside `crates/`, `clients/`, `docs/` and markdown (`Cargo.lock`, the toolchain file,
//!   scripts, `.mason/`, schemas), minus what [`crate::affected::ignorable`] says no cargo test
//!   reads;
//! * every file under the package's `tests/` (fixtures);
//! * the inputs of each workspace binary the package's tests or sources name (what they spawn);
//! * the suite's command template, and the build environment that changes what is compiled.
//!
//! A path is workspace-relative, so two worktrees with the same content have the same key. The
//! key is over-inclusive on purpose: a wrong key may cause a rerun, never a skip. A target with
//! no dep-info, or a group this cannot read (a test filter), has no key and always runs.

use crate::affected::Pkg;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// One thing `cargo test` runs for a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Lib,
    Bin(String),
    Test(String),
    Doc,
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The files named on the first rule of a dep-info file, and its `# env-dep:` lines.
pub fn parse_dep_info(text: &str) -> (Vec<String>, Vec<String>) {
    let mut files = vec![];
    let mut envs = vec![];
    let mut first = true;
    for line in text.lines() {
        if let Some(e) = line.strip_prefix("# env-dep:") {
            envs.push(e.to_string());
            continue;
        }
        if !first || line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        first = false;
        let Some((_, deps)) = line.split_once(": ") else {
            continue;
        };
        let mut cur = String::new();
        let mut chars = deps.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&' ') => {
                    cur.push(' ');
                    chars.next();
                }
                ' ' => {
                    if !cur.is_empty() {
                        files.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(c),
            }
        }
        if !cur.is_empty() {
            files.push(cur);
        }
    }
    (files, envs)
}

/// A workspace as the key sees it.
pub struct Workspace {
    root: PathBuf,
    target_dir: PathBuf,
    pkgs: Vec<Pkg>,
    /// Digest of the files outside `crates/`, `clients/`, `docs/` and markdown.
    common: String,
    env: Vec<(String, String)>,
    hashes: RefCell<HashMap<PathBuf, String>>,
}

/// The build environment that changes what is compiled.
pub const ENV_KEYS: [&str; 6] = [
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_BUILD_TARGET",
    "CARGO_PROFILE_DEV_DEBUG",
];

impl Workspace {
    /// `files` are the workspace-relative paths of every tracked (and untracked, unignored)
    /// file. `env` is the build environment, of which only [`ENV_KEYS`] count.
    pub fn new(
        root: &Path,
        target_dir: &Path,
        pkgs: Vec<Pkg>,
        files: &[String],
        env: &[(String, String)],
    ) -> Workspace {
        let ws = Workspace {
            root: root.to_path_buf(),
            target_dir: target_dir.to_path_buf(),
            pkgs,
            common: String::new(),
            env: env
                .iter()
                .filter(|(k, _)| ENV_KEYS.contains(&k.as_str()))
                .cloned()
                .collect(),
            hashes: RefCell::default(),
        };
        let mut lines = BTreeSet::new();
        for f in files {
            let outside = !(f.starts_with("crates/")
                || f.starts_with("clients/")
                || f.starts_with("docs/")
                || f.ends_with(".md"));
            if outside && !crate::affected::ignorable(f) {
                lines.insert(format!("{f}:{}", ws.file_hash(&ws.root.join(f))));
            }
        }
        let common = hex(lines.into_iter().collect::<Vec<_>>().join("\n").as_bytes());
        Workspace { common, ..ws }
    }

    /// Read the packages from `cargo metadata` and the file list from git, in `wt`.
    pub fn open(wt: &Path, target_dir: Option<&Path>, env: &[(String, String)]) -> Option<Self> {
        let out = std::process::Command::new("cargo")
            .args([
                "metadata",
                "--no-deps",
                "--format-version",
                "1",
                "--offline",
            ])
            .current_dir(wt)
            .output()
            .ok()
            .filter(|o| o.status.success())?;
        let meta: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        let root = std::fs::canonicalize(wt).ok()?;
        let pkgs = crate::affected::packages(&meta, &root)?;
        let td = match target_dir {
            Some(t) => t.to_path_buf(),
            None => PathBuf::from(meta["target_directory"].as_str()?),
        };
        let listed = mason_landing::git::run(
            wt,
            &["ls-files", "--cached", "--others", "--exclude-standard"],
        )
        .ok()?;
        let files: Vec<String> = listed.lines().map(String::from).collect();
        Some(Workspace::new(&root, &td, pkgs, &files, env))
    }

    /// The names of the workspace packages.
    pub fn pkg_names(&self) -> Vec<String> {
        self.pkgs.iter().map(|p| p.name.clone()).collect()
    }

    pub fn pkg(&self, name: &str) -> Option<&Pkg> {
        self.pkgs.iter().find(|p| p.name == name)
    }

    fn file_hash(&self, p: &Path) -> String {
        if let Some(h) = self.hashes.borrow().get(p) {
            return h.clone();
        }
        let h = match std::fs::read(p) {
            Ok(b) => hex(&b),
            Err(_) => "absent".to_string(),
        };
        self.hashes.borrow_mut().insert(p.to_path_buf(), h.clone());
        h
    }

    /// The label of a file in the key: workspace-relative, or relative to the target directory
    /// for what a build script generated, or absolute for anything else.
    fn label(&self, p: &Path) -> String {
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };
        if let Ok(r) = abs.strip_prefix(&self.root) {
            return r.to_string_lossy().to_string();
        }
        if let Ok(r) = abs.strip_prefix(&self.target_dir) {
            return format!("target:{}", r.to_string_lossy());
        }
        abs.to_string_lossy().to_string()
    }

    /// The dep-info files of the executables named `name` (any hash, any profile): all of them,
    /// since a stale one only adds inputs.
    fn dep_files(&self, name: &str) -> Vec<PathBuf> {
        let mut dirs = vec![];
        let mut bases = vec![self.target_dir.clone()];
        if let Ok(rd) = std::fs::read_dir(&self.target_dir) {
            bases.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        for b in bases {
            for prof in ["debug", "release"] {
                let d = b.join(prof).join("deps");
                if d.is_dir() {
                    dirs.push(d);
                }
            }
        }
        let prefix = format!("{name}-");
        let mut out = vec![];
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let f = e.file_name().to_string_lossy().to_string();
                let Some(rest) = f.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".d")) else {
                    continue;
                };
                if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()) {
                    out.push(e.path());
                }
            }
        }
        out.sort();
        out
    }

    /// `label:hash` of every input the dep-info of `name` lists, among the dep-info files that
    /// list a source under `dir` (the package: two packages may each have a test named `it`);
    /// `None` when there is none.
    fn dep_inputs(&self, name: &str, dir: &str, into: &mut BTreeSet<String>) -> Option<()> {
        let mut found = false;
        let prefix = format!("{dir}/");
        for f in self.dep_files(name) {
            let text = std::fs::read_to_string(&f).ok()?;
            let (deps, envs) = parse_dep_info(&text);
            let mut mine = BTreeSet::new();
            for d in deps {
                let p = Path::new(&d);
                let abs = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    self.root.join(p)
                };
                mine.insert(format!("{}:{}", self.label(&abs), self.file_hash(&abs)));
            }
            if !mine.iter().any(|m| m.starts_with(&prefix)) {
                continue;
            }
            found = true;
            into.extend(mine);
            for e in envs {
                into.insert(format!("env-dep:{e}"));
            }
        }
        found.then_some(())
    }

    /// The workspace packages `pkg` depends on, itself included.
    fn closure<'a>(&'a self, pkg: &'a Pkg) -> Vec<&'a Pkg> {
        let mut seen: BTreeMap<&'a str, &'a Pkg> = BTreeMap::new();
        let mut todo = vec![pkg];
        while let Some(p) = todo.pop() {
            if seen.insert(p.name.as_str(), p).is_some() {
                continue;
            }
            todo.extend(p.deps.iter().filter_map(|d| self.pkg(d)));
        }
        seen.into_values().collect()
    }

    fn walk(&self, dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                self.walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }

    /// The workspace binaries the tests or sources of `pkg` can spawn: `(package, bin)`. A
    /// bin is named by `CARGO_BIN_EXE_<bin>` or by a word equal to its name in any file.
    fn spawned(&self, pkg: &Pkg) -> Vec<(&Pkg, String)> {
        let mut text = String::new();
        let mut files = vec![];
        self.walk(&self.root.join(&pkg.dir).join("tests"), &mut files);
        self.walk(&self.root.join(&pkg.dir).join("src"), &mut files);
        for f in files {
            if f.extension().is_some_and(|e| e == "rs") {
                if let Ok(t) = std::fs::read_to_string(&f) {
                    text.push_str(&t);
                    text.push('\n');
                }
            }
        }
        let words: BTreeSet<&str> = text
            .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
            .filter(|w| !w.is_empty())
            .collect();
        let mut out = vec![];
        for p in &self.pkgs {
            for b in &p.bins {
                let under = b.replace('-', "_");
                let exe = format!("CARGO_BIN_EXE_{b}");
                let exe2 = format!("CARGO_BIN_EXE_{under}");
                if words.contains(b.as_str())
                    || words.contains(under.as_str())
                    || words.contains(exe.as_str())
                    || words.contains(exe2.as_str())
                {
                    out.push((p, b.clone()));
                }
            }
        }
        out
    }

    fn closure_inputs(&self, pkg: &Pkg, into: &mut BTreeSet<String>) -> Option<()> {
        for p in self.closure(pkg) {
            let manifest = self.root.join(&p.dir).join("Cargo.toml");
            into.insert(format!(
                "manifest:{}:{}",
                self.label(&manifest),
                self.file_hash(&manifest)
            ));
            let build = self.root.join(&p.dir).join("build.rs");
            if build.is_file() {
                into.insert(format!(
                    "build:{}:{}",
                    self.label(&build),
                    self.file_hash(&build)
                ));
            }
            if let Some(lib) = &p.lib {
                self.dep_inputs(lib, &p.dir, into)?;
            }
        }
        Some(())
    }

    /// The input key of `item` of `pkg`, or `None` when it cannot be known (no dep-info).
    /// `tag` is the suite's command template and anything else that changes how it runs.
    pub fn key(&self, pkg: &Pkg, item: &Item, tag: &str) -> Option<String> {
        let mut lines: BTreeSet<String> = BTreeSet::new();
        lines.insert(format!("tag:{tag}"));
        lines.insert(format!("common:{}", self.common));
        for (k, v) in &self.env {
            lines.insert(format!("env:{k}={v}"));
        }
        let exe = match item {
            Item::Lib => pkg.lib.clone()?,
            Item::Bin(b) | Item::Test(b) => b.replace('-', "_"),
            Item::Doc => return None,
        };
        self.dep_inputs(&exe, &pkg.dir, &mut lines)?;
        self.closure_inputs(pkg, &mut lines)?;
        let mut fixtures = vec![];
        self.walk(&self.root.join(&pkg.dir).join("tests"), &mut fixtures);
        for f in fixtures {
            lines.insert(format!("fixture:{}:{}", self.label(&f), self.file_hash(&f)));
        }
        for (p, bin) in self.spawned(pkg) {
            self.dep_inputs(&bin.replace('-', "_"), &p.dir, &mut lines)?;
            self.closure_inputs(p, &mut lines)?;
        }
        Some(hex(lines
            .into_iter()
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes()))
    }
}

/// What a group of `cargo test` arguments selects: `(package, items)`; `None` for a group
/// this cannot read (a test filter after `--`, an unknown flag) or an unknown package.
fn parse_group(ws: &Workspace, group: &str) -> Option<(String, Vec<Item>)> {
    let mut toks = group.split_whitespace();
    let (mut pkg, mut lib, mut bins, mut doc) = (None, false, false, false);
    let (mut tests, mut bin_names) = (vec![], vec![]);
    while let Some(t) = toks.next() {
        match t {
            "-p" => pkg = Some(toks.next()?.to_string()),
            "--lib" => lib = true,
            "--bins" => bins = true,
            "--doc" => doc = true,
            "--test" => tests.push(toks.next()?.to_string()),
            "--bin" => bin_names.push(toks.next()?.to_string()),
            _ => return None,
        }
    }
    let name = pkg?;
    let p = ws.pkg(&name)?;
    let whole = !lib && !bins && !doc && tests.is_empty() && bin_names.is_empty();
    let mut items = vec![];
    if (lib || whole) && p.lib.is_some() {
        items.push(Item::Lib);
    }
    if bins || whole {
        items.extend(p.bins.iter().cloned().map(Item::Bin));
    }
    items.extend(bin_names.into_iter().map(Item::Bin));
    if whole {
        items.extend(p.tests.iter().map(|t| Item::Test(t.name.clone())));
    }
    items.extend(tests.into_iter().map(Item::Test));
    if doc || (whole && p.lib.is_some()) {
        items.push(Item::Doc);
    }
    Some((name, items))
}

fn render(pkg: &str, items: &[Item]) -> Vec<String> {
    let mut g = format!("-p {pkg}");
    let mut any = false;
    for i in items {
        match i {
            Item::Lib => g.push_str(" --lib"),
            Item::Bin(b) => g.push_str(&format!(" --bin {b}")),
            Item::Test(t) => g.push_str(&format!(" --test {t}")),
            Item::Doc => continue,
        }
        any = true;
    }
    let mut out = vec![];
    if any {
        out.push(g);
    }
    if items.contains(&Item::Doc) {
        out.push(format!("-p {pkg} --doc"));
    }
    out
}

fn label(pkg: &str, item: &Item) -> String {
    match item {
        Item::Lib => format!("{pkg}:lib"),
        Item::Bin(b) => format!("{pkg}:bin:{b}"),
        Item::Test(t) => format!("{pkg}:test:{t}"),
        Item::Doc => format!("{pkg}:doc"),
    }
}

/// A target that runs, with the key it was built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    /// The executable's name as cargo prints it (`mason_daemon`, `it`).
    pub exe: String,
    /// `None`: no key could be made, so it runs and nothing is recorded.
    pub key: Option<String>,
}

/// A target left out because it was green on the same inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reused {
    pub label: String,
    pub tests: usize,
    pub tree: String,
    pub task: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub project: String,
    pub suite: String,
    /// What is left to run, in the same form as the groups given.
    pub groups: Vec<String>,
    pub run: Vec<Entry>,
    pub reused: Vec<Reused>,
}

/// A recorded green: `(tests, tree, task)`.
pub type Green = (usize, String, u64);

/// Leave out of `groups` each target whose key `lookup(label, key)` finds green.
pub fn plan(
    ws: &Workspace,
    project: &str,
    suite: &str,
    groups: &[String],
    tag: &str,
    lookup: &dyn Fn(&str, &str) -> Option<Green>,
) -> Plan {
    let mut out = Plan {
        project: project.into(),
        suite: suite.into(),
        ..Default::default()
    };
    for g in groups {
        let Some((name, items)) = parse_group(ws, g) else {
            out.groups.push(g.clone());
            continue;
        };
        let pkg = ws.pkg(&name).expect("parse_group found it");
        let mut keep = vec![];
        for it in items {
            let lab = label(&name, &it);
            let key = ws.key(pkg, &it, tag);
            let hit = key.as_deref().and_then(|k| lookup(&lab, k));
            match hit {
                Some((tests, tree, task)) => out.reused.push(Reused {
                    label: lab,
                    tests,
                    tree,
                    task,
                }),
                None => {
                    let exe = match &it {
                        Item::Lib => pkg.lib.clone().unwrap_or_default(),
                        Item::Bin(b) | Item::Test(b) => b.replace('-', "_"),
                        Item::Doc => format!("doc:{}", name.replace('-', "_")),
                    };
                    out.run.push(Entry {
                        label: lab,
                        exe,
                        key,
                    });
                    keep.push(it);
                }
            }
        }
        out.groups.extend(render(&name, &keep));
    }
    // A selection nothing was left out of runs exactly as it was given.
    if out.reused.is_empty() {
        out.groups = groups.to_vec();
    }
    out
}

impl Plan {
    pub fn all_reused(&self) -> bool {
        !self.reused.is_empty() && self.groups.is_empty()
    }

    /// The step note: what was reused, from which run.
    pub fn note(&self) -> Option<String> {
        if self.reused.is_empty() {
            return None;
        }
        let tests: usize = self.reused.iter().map(|r| r.tests).sum();
        let mut from: BTreeMap<(u64, &str), Vec<&str>> = BTreeMap::new();
        for r in &self.reused {
            from.entry((r.task, r.tree.as_str()))
                .or_default()
                .push(r.label.as_str());
        }
        let parts: Vec<String> = from
            .iter()
            .map(|((task, tree), labels)| {
                let t = tree.get(..12).unwrap_or(tree);
                let who = if *task == 0 {
                    "a landing batch".to_string()
                } else {
                    format!("#{task}")
                };
                format!(
                    "{} green on {who} (tree {t}) with the same inputs",
                    labels.join(", ")
                )
            })
            .collect();
        Some(format!(
            "{}: reused {} of {} targets ({tests} tests): {}; {} run",
            self.suite,
            self.reused.len(),
            self.reused.len() + self.run.len(),
            parts.join("; "),
            self.run.len()
        ))
    }

    /// Record a green run of every target that ran and had a key.
    pub fn record_green(
        &self,
        store: &mason_store::Store,
        log: &str,
        tree: &str,
        task: u64,
        now_ms: u64,
    ) {
        let counts = counts_by_exe(log);
        for e in &self.run {
            let Some(k) = &e.key else { continue };
            let _ = store.put_target_green(
                &self.project,
                &self.suite,
                &e.label,
                k,
                counts.get(&e.exe).copied().unwrap_or(0),
                tree,
                task,
                now_ms,
            );
        }
    }

    /// Record the targets of the log that failed (every target that ran when none can be told
    /// apart) as red on their keys: never reused.
    pub fn record_red(
        &self,
        store: &mason_store::Store,
        log: &str,
        tree: &str,
        task: u64,
        now_ms: u64,
    ) {
        let failed = failed_exes(log);
        for e in &self.run {
            let Some(k) = &e.key else { continue };
            if failed.is_empty() || failed.contains(&e.exe) {
                let _ = store.put_target_red(
                    &self.project,
                    &self.suite,
                    &e.label,
                    k,
                    tree,
                    task,
                    now_ms,
                );
            }
        }
    }
}

/// The executable named on a `Running ... (target/debug/deps/name-hash)` line.
fn exe_name(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("Running ")?;
    let path = rest.rsplit_once('(')?.1.strip_suffix(')')?;
    let file = path.rsplit('/').next()?;
    let (name, hash) = file.rsplit_once('-')?;
    (!name.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| name.to_string())
}

/// Tests passed per executable in a cargo log (`test result: ok. 12 passed`).
pub fn counts_by_exe(log: &str) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    let mut cur: Option<String> = None;
    for l in log.lines() {
        if let Some(n) = exe_name(l) {
            cur = Some(n);
        } else if let Some(r) = l.trim().strip_prefix("test result:") {
            let n = r
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|w| w[1].starts_with("passed"))
                .and_then(|w| w[0].parse::<usize>().ok())
                .unwrap_or(0);
            if let Some(c) = cur.take() {
                *out.entry(c).or_default() += n;
            }
        }
    }
    out
}

/// The executables with a failing test line in a cargo log.
pub fn failed_exes(log: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut cur: Option<String> = None;
    for l in log.lines() {
        if let Some(n) = exe_name(l) {
            cur = Some(n);
        } else if (l.starts_with("test ") && l.ends_with("FAILED"))
            || l.trim().starts_with("test result: FAILED")
        {
            out.extend(cur.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn pkg(name: &str, deps: &[&str], lib: Option<&str>, tests: &[&str], bins: &[&str]) -> Pkg {
        Pkg {
            name: name.into(),
            dir: format!("crates/{name}"),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            lib: lib.map(String::from),
            bins: bins.iter().map(|s| s.to_string()).collect(),
            tests: tests
                .iter()
                .map(|t| crate::affected::TestTarget {
                    name: t.to_string(),
                    src: format!("crates/{name}/tests/{t}.rs"),
                })
                .collect(),
        }
    }

    /// A scratch workspace: crate `a` (lib `a`, test `it`), `b` (lib `b` depends on `a`, test
    /// `it_b`, bin `bd`), with dep-info as cargo writes it.
    fn scratch() -> (tempfile::TempDir, Vec<Pkg>) {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        put(r, "Cargo.toml", "[workspace]\n");
        put(r, "Cargo.lock", "lock 1\n");
        put(r, "scripts/run.sh", "echo 1\n");
        put(r, "docs/a.txt", "doc\n");
        put(r, "clients/x.swift", "swift\n");
        put(r, "crates/a/Cargo.toml", "[package]\nname='a'\n");
        put(r, "crates/a/src/lib.rs", "pub fn a() {}\n");
        put(r, "crates/a/tests/it.rs", "#[test] fn t() {}\n");
        put(r, "crates/a/tests/data/fix.txt", "fixture\n");
        put(r, "crates/b/Cargo.toml", "[package]\nname='b'\n");
        put(r, "crates/b/src/lib.rs", "pub fn b() {}\n");
        put(r, "crates/b/src/main.rs", "fn main() {}\n");
        put(r, "crates/b/tests/it_b.rs", "// spawns CARGO_BIN_EXE_bd\n");
        for (name, src) in [
            ("a", "crates/a/src/lib.rs"),
            ("it", "crates/a/tests/it.rs"),
            ("b", "crates/b/src/lib.rs"),
            ("bd", "crates/b/src/main.rs"),
            ("it_b", "crates/b/tests/it_b.rs"),
        ] {
            let abs = format!("{}/{}", r.display(), src);
            put(
                r,
                &format!("target/debug/deps/{name}-0123456789abcdef.d"),
                &format!("target/debug/deps/{name}-0123456789abcdef: {abs}\n\n{abs}:\n"),
            );
        }
        let pkgs = vec![
            pkg("a", &[], Some("a"), &["it"], &[]),
            pkg("b", &["a"], Some("b"), &["it_b"], &["bd"]),
        ];
        (d, pkgs)
    }

    fn ws(root: &Path, pkgs: &[Pkg]) -> Workspace {
        let files: Vec<String> = [
            "Cargo.toml",
            "Cargo.lock",
            "scripts/run.sh",
            "docs/a.txt",
            "clients/x.swift",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        Workspace::new(root, &root.join("target"), pkgs.to_vec(), &files, &[])
    }

    fn key_of(root: &Path, pkgs: &[Pkg], pkg: &str, item: Item) -> Option<String> {
        let w = ws(root, pkgs);
        w.key(w.pkg(pkg).unwrap(), &item, "tag")
    }

    #[test]
    fn dep_info_lists_files_and_env() {
        let (f, e) = parse_dep_info("x: a.rs dir/b\\ c.rs\n\na.rs:\n# env-dep:K=V\n");
        assert_eq!(f, vec!["a.rs", "dir/b c.rs"]);
        assert_eq!(e, vec!["K=V"]);
    }

    #[test]
    fn the_key_changes_with_every_input_that_can_matter() {
        let (d, pkgs) = scratch();
        let r = d.path();
        let base = key_of(r, &pkgs, "a", Item::Test("it".into())).unwrap();
        let again = key_of(r, &pkgs, "a", Item::Test("it".into())).unwrap();
        assert_eq!(base, again);
        for (rel, what) in [
            ("crates/a/tests/it.rs", "a dep-info input"),
            ("Cargo.lock", "Cargo.lock"),
            ("crates/a/tests/data/fix.txt", "a fixture"),
            ("scripts/run.sh", "a root script"),
            ("crates/a/Cargo.toml", "a manifest"),
        ] {
            let before = std::fs::read_to_string(r.join(rel)).unwrap();
            put(r, rel, &format!("{before}// changed\n"));
            let k = key_of(r, &pkgs, "a", Item::Test("it".into())).unwrap();
            assert_ne!(k, base, "{what} must change the key");
            put(r, rel, &before);
        }
        for rel in ["docs/a.txt", "clients/x.swift", "crates/b/src/lib.rs"] {
            put(r, rel, "changed\n");
            let k = key_of(r, &pkgs, "a", Item::Test("it".into())).unwrap();
            assert_eq!(k, base, "{rel} is no input of a's test");
        }
    }

    #[test]
    fn a_dependency_and_a_spawned_binary_are_inputs() {
        let (d, pkgs) = scratch();
        let r = d.path();
        let b_test = key_of(r, &pkgs, "b", Item::Test("it_b".into())).unwrap();
        // b depends on a: a's library source is an input of b's tests.
        put(r, "crates/a/src/lib.rs", "pub fn a() { 1; }\n");
        let after_dep = key_of(r, &pkgs, "b", Item::Test("it_b".into())).unwrap();
        assert_ne!(b_test, after_dep);
        // it_b names CARGO_BIN_EXE_bd: the binary's own source is an input.
        put(r, "crates/b/src/main.rs", "fn main() { 1; }\n");
        let after_bin = key_of(r, &pkgs, "b", Item::Test("it_b".into())).unwrap();
        assert_ne!(after_dep, after_bin);
        // a's tests do not spawn it.
        let a1 = key_of(r, &pkgs, "a", Item::Test("it".into())).unwrap();
        put(r, "crates/b/src/main.rs", "fn main() { 2; }\n");
        assert_eq!(a1, key_of(r, &pkgs, "a", Item::Test("it".into())).unwrap());
    }

    #[test]
    fn two_worktrees_with_the_same_content_have_the_same_key() {
        let (d1, pkgs) = scratch();
        let (d2, _) = scratch();
        let k1 = key_of(d1.path(), &pkgs, "b", Item::Test("it_b".into())).unwrap();
        let k2 = key_of(d2.path(), &pkgs, "b", Item::Test("it_b".into())).unwrap();
        assert_ne!(d1.path(), d2.path());
        assert_eq!(k1, k2);
    }

    #[test]
    fn a_target_with_no_dep_info_has_no_key() {
        let (d, pkgs) = scratch();
        std::fs::remove_file(d.path().join("target/debug/deps/it-0123456789abcdef.d")).unwrap();
        assert!(key_of(d.path(), &pkgs, "a", Item::Test("it".into())).is_none());
    }

    #[test]
    fn the_plan_leaves_out_green_targets_and_keeps_doc_tests() {
        let (d, pkgs) = scratch();
        let w = ws(d.path(), &pkgs);
        let groups = vec!["-p a".to_string(), "-p b --test it_b".to_string()];
        let ka = w
            .key(w.pkg("a").unwrap(), &Item::Test("it".into()), "t")
            .unwrap();
        let green = |lab: &str, key: &str| {
            (lab == "a:test:it" && key == ka).then(|| (3, "tree0123456789abcdef".to_string(), 7))
        };
        let p = plan(&w, "p", "s", &groups, "t", &green);
        assert_eq!(p.reused.len(), 1);
        assert_eq!(
            p.groups,
            vec!["-p a --lib", "-p a --doc", "-p b --test it_b"]
        );
        assert!(p.note().unwrap().contains("a:test:it green on #7"));
        // Nothing green: the groups run as given.
        let p = plan(&w, "p", "s", &groups, "t", &|_, _| None);
        assert_eq!(p.groups, groups);
        // Every target green, doc tests aside: only the doc group is left.
        let p = plan(
            &w,
            "p",
            "s",
            &["-p a --test it".to_string()],
            "t",
            &|_, _| Some((1, "t".into(), 1)),
        );
        assert!(p.all_reused());
    }

    #[test]
    fn a_red_or_flaky_key_is_never_reused() {
        let dir = tempfile::tempdir().unwrap();
        let store = mason_store::Store::open(dir.path().join("s.db")).unwrap();
        store
            .put_target_green("p", "s", "a:test:it", "k1", 3, "tree", 1, 1)
            .unwrap();
        assert!(store
            .target_green("p", "s", "a:test:it", "k1")
            .unwrap()
            .is_some());
        // Red on the same key, then green again on it (flaky): not reusable.
        store
            .put_target_red("p", "s", "a:test:it", "k1", "tree", 2, 2)
            .unwrap();
        assert!(store
            .target_green("p", "s", "a:test:it", "k1")
            .unwrap()
            .is_none());
        store
            .put_target_green("p", "s", "a:test:it", "k1", 3, "tree", 3, 3)
            .unwrap();
        assert!(store
            .target_green("p", "s", "a:test:it", "k1")
            .unwrap()
            .is_none());
        assert!(store
            .target_green("p", "s", "a:test:it", "k2")
            .unwrap()
            .is_none());
    }

    #[test]
    fn the_log_gives_counts_and_failures_per_executable() {
        let log = "     Running unittests src/lib.rs (target/debug/deps/a-0123456789abcdef)\n\
                   test result: ok. 4 passed; 0 failed\n\
                   \x20    Running tests/it.rs (target/debug/deps/it-0123456789abcdef)\n\
                   test x ... FAILED\n\
                   test result: FAILED. 1 passed; 1 failed\n";
        let c = counts_by_exe(log);
        assert_eq!(c["a"], 4);
        assert_eq!(c["it"], 1);
        assert_eq!(failed_exes(log).into_iter().collect::<Vec<_>>(), vec!["it"]);
    }

    fn build_scratch(root: &Path) {
        let st = std::process::Command::new("cargo")
            .args(["test", "--no-run", "--offline"])
            .current_dir(root)
            .env_remove("RUSTC_BOOTSTRAP")
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("CARGO_BUILD_TARGET")
            .env("CARGO_TARGET_DIR", root.join("target"))
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    /// Two real crates built by cargo, and the plans of two tasks over them (s707).
    #[test]
    fn a_second_task_changing_only_crate_b_runs_b_and_reuses_a() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        put(
            r,
            "Cargo.toml",
            "[workspace]\nresolver = \"2\"\nmembers = [\"crates/a\", \"crates/b\"]\n",
        );
        for c in ["a", "b"] {
            put(
                r,
                &format!("crates/{c}/Cargo.toml"),
                &format!("[package]\nname = \"{c}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
            );
            put(
                r,
                &format!("crates/{c}/src/lib.rs"),
                "pub fn f() -> u32 { 1 }\n",
            );
            put(
                r,
                &format!("crates/{c}/tests/it.rs"),
                &format!("#[test]\nfn works() {{ assert_eq!({c}::f(), 1); }}\n"),
            );
        }
        put(r, ".gitignore", "target\n");
        let o = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(r)
            .output()
            .unwrap();
        assert!(o.status.success());
        let dir = tempfile::tempdir().unwrap();
        let store = mason_store::Store::open(dir.path().join("s.db")).unwrap();
        let groups = vec!["-p a".to_string(), "-p b".to_string()];
        let lookup = |l: &str, k: &str| store.target_green("p", "s", l, k).ok().flatten();
        let plan_now = || {
            build_scratch(r);
            let w = Workspace::open(r, Some(&r.join("target")), &[]).unwrap();
            plan(&w, "p", "s", &groups, "tag", &lookup)
        };
        // Task 1: nothing is recorded, so everything runs; its green is recorded.
        let p1 = plan_now();
        assert!(p1.reused.is_empty());
        assert_eq!(p1.groups, groups);
        let log = "     Running unittests src/lib.rs (target/debug/deps/a-0123456789abcdef)\n\
                   test result: ok. 0 passed; 0 failed\n\
                   \x20    Running tests/it.rs (target/debug/deps/it-0123456789abcdef)\n\
                   test result: ok. 1 passed; 0 failed\n";
        p1.record_green(&store, log, "tree-one-0000", 1, 1);
        // Task 2 changes crate b only: a's targets are reused, naming task 1's tree.
        put(r, "crates/b/src/lib.rs", "pub fn f() -> u32 { 1 + 0 }\n");
        let p2 = plan_now();
        let reused: Vec<&str> = p2.reused.iter().map(|x| x.label.as_str()).collect();
        assert!(
            reused.contains(&"a:lib") && reused.contains(&"a:test:it"),
            "{p2:?}"
        );
        assert!(!reused.iter().any(|l| l.starts_with("b:")), "{p2:?}");
        assert!(
            p2.note().unwrap().contains("tree-one-0000"),
            "{:?}",
            p2.note()
        );
        assert!(p2.run.iter().any(|e| e.label == "b:test:it"));
        // Changing a's test file runs it again.
        put(
            r,
            "crates/a/tests/it.rs",
            "#[test]\nfn works() { assert_eq!(a::f(), 1); }\n#[test]\nfn more() {}\n",
        );
        let p3 = plan_now();
        assert!(p3.run.iter().any(|e| e.label == "a:test:it"), "{p3:?}");
    }
}
