// SPDX-License-Identifier: GPL-3.0-or-later

//! Which cargo test targets a change can affect (#538, decision 0539).
//!
//! `scope` picks whole packages. This picks test targets inside them: a changed test file
//! runs itself; a changed `src` file runs its crate's unit tests, plus the integration
//! targets of that crate and of its dependents whose source mentions the changed module or a
//! public item of it. The scan is static (words in source files), never timing. A reference
//! it cannot decide includes the target. Only a path that can reach every target (the root
//! `Cargo.toml` and `Cargo.lock`, the toolchain file, `.cargo/`, a path the suite's own command
//! names) selects everything, and says which (#652). A package's `Cargo.toml` or `build.rs`
//! runs that package whole plus its dependents' targets; a file under `tests/` that is not a
//! target selects the package's targets that name it; any other path selects the targets, in
//! any package, whose text names the path or its file name, and nothing if none does.
//!
//! The landing batch runs the whole workspace, so what this leaves out is still run before
//! anything reaches `develop`.

use crate::covmap::{hash_text, CoverageMap};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One workspace package, as far as test selection needs it.
#[derive(Clone, Debug, Default)]
pub struct Pkg {
    pub name: String,
    /// Relative to the workspace root, no trailing slash.
    pub dir: String,
    /// Names of the workspace packages it depends on by path.
    pub deps: Vec<String>,
    /// The library target's name as code writes it (`mason_daemon`).
    pub lib: Option<String>,
    pub bins: Vec<String>,
    pub tests: Vec<TestTarget>,
}

/// An integration test target.
#[derive(Clone, Debug)]
pub struct TestTarget {
    pub name: String,
    /// Its root file, relative to the workspace root.
    pub src: String,
}

/// What to run in one package. `whole` runs the package's every target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    pub lib: bool,
    pub bins: bool,
    pub doc: bool,
    pub tests: BTreeSet<String>,
    pub whole: bool,
    /// Directory test targets narrowed to some of their modules (#655): target name to its
    /// libtest filters (`resume_loss::`). A target also in `tests` (or a `whole` group) runs
    /// whole and its filters are dropped.
    pub modules: BTreeMap<String, BTreeSet<String>>,
    /// One line per narrowed directory target, how its modules were picked (#656).
    pub notes: Vec<String>,
    /// Unit tests (`--lib`, `--bins`) narrowed to some modules (#958): the libtest filters
    /// (`covmap::`), and which of the two targets the group runs. Ignored where `lib`/`bins`
    /// (or `whole`) already run the target whole.
    pub unit_modules: BTreeSet<String>,
    pub unit_lib: bool,
    pub unit_bins: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Something that can affect every target changed: run everything. `because` names the path.
    All { because: String },
    /// Per package name.
    Some(BTreeMap<String, Group>),
}

/// The packages of `cargo metadata --no-deps` output (`root` is the canonical workspace root).
pub fn packages(meta: &Value, root: &Path) -> Option<Vec<Pkg>> {
    let members: BTreeSet<&str> = meta["workspace_members"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let rel = |p: &str| -> Option<String> {
        Some(
            Path::new(p)
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .to_string(),
        )
    };
    let mut out = vec![];
    for p in meta["packages"].as_array()? {
        if !members.is_empty() && !members.contains(p["id"].as_str().unwrap_or_default()) {
            continue;
        }
        let manifest = rel(p["manifest_path"].as_str()?)?;
        let dir = manifest
            .rsplit_once('/')
            .map_or("", |(d, _)| d)
            .trim_matches('/')
            .to_string();
        let mut pkg = Pkg {
            name: p["name"].as_str()?.to_string(),
            dir,
            deps: p["dependencies"]
                .as_array()
                .map(|d| {
                    d.iter()
                        .filter(|d| d["path"].is_string())
                        .filter_map(|d| d["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            ..Default::default()
        };
        for t in p["targets"].as_array().into_iter().flatten() {
            let kinds: Vec<&str> = t["kind"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let name = t["name"].as_str().unwrap_or_default().to_string();
            if kinds
                .iter()
                .any(|k| k.ends_with("lib") || *k == "proc-macro")
            {
                pkg.lib = Some(name.replace('-', "_"));
            } else if kinds.contains(&"bin") {
                pkg.bins.push(name);
            } else if kinds.contains(&"test") {
                if let Some(src) = t["src_path"].as_str().and_then(rel) {
                    pkg.tests.push(TestTarget { name, src });
                }
            }
        }
        out.push(pkg);
    }
    Some(out)
}

/// Paths that no cargo test of ours reads as input: docs, markdown, and the exit scripts (the
/// `exit-scripts` suite runs those). Decision 0173 kept the cargo suite for docs-only changes;
/// decision 0539 replaces that with the smoke set here and the whole run at the batch.
pub(crate) fn ignorable(path: &str) -> bool {
    path.starts_with("docs/")
        || path.ends_with(".md")
        || path.starts_with("scripts/exit/")
        || path == crate::scope::SMOKE_FILE
}

/// Does a change touch only paths no cargo test of ours reads ([`ignorable`])? Empty: no.
pub fn reads_none(paths: &[String]) -> bool {
    !paths.is_empty() && paths.iter().all(|p| ignorable(p))
}

fn owner<'a>(pkgs: &'a [Pkg], path: &str) -> Option<&'a Pkg> {
    pkgs.iter()
        .filter(|p| !p.dir.is_empty())
        .filter(|p| {
            path.strip_prefix(&p.dir)
                .is_some_and(|r| r.starts_with('/'))
        })
        .max_by_key(|p| p.dir.len())
}

fn target_owns(t: &TestTarget, path: &str) -> bool {
    if t.src == path {
        return true;
    }
    match t.src.strip_suffix("/main.rs") {
        Some(dir) => path.strip_prefix(dir).is_some_and(|r| r.starts_with('/')),
        None => false,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Does `text` read the repository's own copy of a `.mason/` file? A fixture
/// that writes its own `.mason/mason.toml` does not; one that names
/// `../../<path>` or `root().join("<path>")` does.
fn reads_repo_file(text: &str, path: &str) -> bool {
    text.contains(&format!("../../{path}")) || text.contains(&format!("root().join(\"{path}\")"))
}

/// Does `text` hold `word` as a whole identifier?
pub fn contains_word(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(i) = text[from..].find(word) {
        let at = from + i;
        let end = at + word.len();
        let before = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after = text[end..].chars().next().is_none_or(|c| !is_word_char(c));
        if before && after {
            return true;
        }
        from = end;
    }
    false
}

/// The module names a `src` file stands for: `src/a/b.rs` is `a`, `b`. A crate root or a
/// `mod.rs` adds nothing, and a file with no module name leaves the question undecided.
fn modules(rel_in_src: &str) -> Vec<String> {
    rel_in_src
        .trim_end_matches(".rs")
        .split('/')
        .filter(|s| !matches!(*s, "lib" | "main" | "mod" | ""))
        .map(String::from)
        .collect()
}

/// The names a file makes public: after `pub fn`, `pub struct` and the like (everything after
/// `use` for a `pub use`), plus `macro_rules!` names. Over-inclusive on purpose.
fn public_items(text: &str) -> Vec<String> {
    const KEYWORDS: [&str; 10] = [
        "fn", "struct", "enum", "trait", "const", "static", "type", "mod", "union", "use",
    ];
    const QUALIFIERS: [&str; 3] = ["async", "unsafe", "extern"];
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !is_word_char(c))
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    };
    let mut out = vec![];
    for line in text.lines() {
        let l = line.trim_start();
        if let Some(rest) = l.strip_prefix("macro_rules!") {
            out.extend(words(rest).into_iter().take(1));
        }
        let Some(rest) = l.strip_prefix("pub ") else {
            continue;
        };
        let ws = words(rest);
        let mut it = ws.iter().skip_while(|w| QUALIFIERS.contains(&w.as_str()));
        let Some(first) = it.next() else { continue };
        if first == "use" {
            out.extend(it.cloned());
        } else if first == "const" {
            // `pub const fn name` or `pub const NAME`.
            match it.next() {
                Some(n) if n == "fn" => out.extend(it.next().cloned()),
                Some(n) => out.push(n.clone()),
                None => {}
            }
        } else if KEYWORDS.contains(&first.as_str()) {
            out.extend(it.next().cloned());
        }
    }
    out
}

/// The text of a test target: its root file and, for a `dir/main.rs` target, the `.rs` files
/// under its directory.
fn target_text(t: &TestTarget, files: &[String], read: &dyn Fn(&str) -> Option<String>) -> String {
    let mut text = read(&t.src).unwrap_or_default();
    if let Some(dir) = t.src.strip_suffix("/main.rs") {
        for f in files {
            if f != &t.src
                && f.ends_with(".rs")
                && f.strip_prefix(dir).is_some_and(|r| r.starts_with('/'))
            {
                text.push('\n');
                text.push_str(&read(f).unwrap_or_default());
            }
        }
    }
    text
}

/// A `tests/<dir>/main.rs` target seen as its `mod <name>;` modules (#655).
struct DirTarget {
    dir: String,
    /// Module name and the text of its file(s), the module's own subtree only.
    mods: Vec<(String, String)>,
}

impl DirTarget {
    /// The module a changed path belongs to, if any: `dir/name.rs` or anything under `dir/name/`.
    fn module_of(&self, path: &str) -> Option<&str> {
        let rest = path.strip_prefix(&self.dir)?.strip_prefix('/')?;
        self.mods
            .iter()
            .map(|(n, _)| n.as_str())
            .find(|n| rest == format!("{n}.rs") || rest.starts_with(&format!("{n}/")))
    }
}

/// The modules of a directory target: the `mod name;` lines of its `main.rs` that have a file
/// beside it and carry no `#[path]` (an included shared harness is not a module). `None` for a
/// single-file target or one with no such module.
fn dir_target(
    t: &TestTarget,
    files: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Option<DirTarget> {
    let dir = t.src.strip_suffix("/main.rs")?;
    let main = read(&t.src)?;
    let mut mods = vec![];
    let mut prev_path = false;
    for line in main.lines() {
        let l = line.trim();
        let has_path = l.contains("#[path");
        let decl = l
            .strip_prefix("pub(crate) ")
            .or_else(|| l.strip_prefix("pub "))
            .unwrap_or(l);
        let decl = decl.rsplit("] ").next().unwrap_or(decl);
        if let Some(name) = decl
            .strip_prefix("mod ")
            .and_then(|r| r.strip_suffix(';'))
            .map(str::trim)
        {
            let own = format!("{dir}/{name}.rs");
            let sub = format!("{dir}/{name}/");
            let mine: Vec<&String> = files
                .iter()
                .filter(|f| **f == own || (f.starts_with(&sub) && f.ends_with(".rs")))
                .collect();
            if !has_path && !prev_path && !mine.is_empty() {
                let text = mine
                    .iter()
                    .map(|f| read(f).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("\n");
                mods.push((name.to_string(), text));
            }
        }
        prev_path = l.starts_with("#[path") && !l.contains(';');
    }
    (!mods.is_empty()).then(|| DirTarget {
        dir: dir.to_string(),
        mods,
    })
}

/// One changed `src` file: does `text` (a test target's source) depend on it?
fn references(p: &Pkg, same_package: bool, changed_rel: &str, content: &str, text: &str) -> bool {
    // Tests that run the crate's binaries, or include its source by path, depend on all of it.
    let spawns = p.bins.iter().any(|b| text.contains(b.as_str()))
        || (same_package
            && (text.contains("CARGO_BIN_EXE_")
                || text.contains("CARGO_BIN_NAME")
                || text.contains("../src/")
                || text.contains("#[path")));
    if spawns {
        return true;
    }
    let Some(lib) = &p.lib else { return false };
    if !contains_word(text, lib) {
        return false;
    }
    let mods = modules(changed_rel.strip_prefix("src/").unwrap_or(changed_rel));
    if !changed_rel.ends_with(".rs") || mods.is_empty() || content.contains("macro_export") {
        return true;
    }
    mods.iter().any(|m| contains_word(text, m))
        || public_items(content).iter().any(|i| contains_word(text, i))
}

/// The lib's modules of a package, as `a::b` paths, from its `src/` files: path to module.
/// `lib.rs`, `main.rs`, and `src/bin/` are not modules of the lib.
fn lib_modules(p: &Pkg, files: &[String]) -> BTreeMap<String, String> {
    let prefix = format!("{}/src/", p.dir);
    let mut out = BTreeMap::new();
    for f in files {
        let Some(rel) = f.strip_prefix(&prefix) else {
            continue;
        };
        if let Some(m) = module_path(rel) {
            out.insert(f.clone(), m);
        }
    }
    out
}

/// The module path of a file under `src/` (`foo/bar.rs` is `foo::bar`, `foo/mod.rs` is `foo`);
/// `None` for the crate roots, anything under `bin/`, and non-Rust files.
fn module_path(rel_in_src: &str) -> Option<String> {
    let stem = rel_in_src.strip_suffix(".rs")?;
    if matches!(stem, "lib" | "main") || stem.starts_with("bin/") {
        return None;
    }
    let mut segs: Vec<&str> = stem.split('/').collect();
    if segs.last() == Some(&"mod") {
        segs.pop();
    }
    (!segs.is_empty()).then(|| segs.join("::"))
}

/// Does a `mod.rs` hold nothing but module declarations, attributes, comments and `use` lines?
fn only_declares(text: &str) -> bool {
    text.lines().map(str::trim).all(|l| {
        l.is_empty()
            || l.starts_with("//")
            || l.starts_with("#[")
            || l.starts_with("#![")
            || l.ends_with(';')
                && (l.starts_with("mod ")
                    || l.starts_with("pub mod ")
                    || l.starts_with("pub(crate) mod ")
                    || l.starts_with("use ")
                    || l.starts_with("pub use ")
                    || l.starts_with("pub(crate) use "))
    })
}

/// Text without its declaration and `use` lines, which name every module without using it.
fn without_decls(text: &str) -> String {
    text.lines()
        .filter(|l| {
            let l = l.trim_start();
            !(l.starts_with("mod ")
                || l.starts_with("pub mod ")
                || l.starts_with("pub(crate) mod ")
                || l.starts_with("use ")
                || l.starts_with("pub use ")
                || l.starts_with("pub(crate) use "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The libtest filters that cover the unit tests of one changed `src` file and of the modules
/// that refer to it (#958); `None` when the whole lib and bins must run. A module is referred
/// to by a word scan of the other modules' text (`crate::m`, `super::m`, `use m::...`), plus
/// the names a `use` line in `lib.rs` or a `mod.rs` brings from it. `mods` maps every module
/// file to its path, `read` gives text.
fn unit_filters(
    p: &Pkg,
    rel: &str,
    content: &str,
    mods: &BTreeMap<String, String>,
    files: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Option<BTreeSet<String>> {
    let in_src = rel.strip_prefix("src/")?;
    let mut me = module_path(in_src)?;
    if content.contains("macro_export") || (in_src.ends_with("/mod.rs") && only_declares(content)) {
        return None;
    }
    // A unit test file of a module (`foo/tests.rs`) is the module's own tests.
    if let Some(parent) = me.strip_suffix("::tests") {
        me = parent.to_string();
    } else if me == "tests" {
        return None;
    }
    let name = me.rsplit("::").next().unwrap_or(&me).to_string();
    let mut names = vec![name];
    let prefix = format!("{}/src/", p.dir);
    let mut picked: BTreeSet<String> = BTreeSet::from([me.clone()]);
    // Names a `use` line of lib.rs or a mod.rs brings out of the changed module.
    for f in files.iter().filter(|f| f.starts_with(&prefix)) {
        let file = f.rsplit('/').next().unwrap_or(f);
        if file != "lib.rs" && file != "mod.rs" {
            continue;
        }
        let text = read(f).unwrap_or_default();
        for l in text.lines().map(str::trim) {
            if l.contains("use ") && contains_word(l, &names[0]) {
                if l.contains('*') {
                    return None;
                }
                names.extend(
                    l.split(|c: char| !is_word_char(c))
                        .filter(|w| {
                            !w.is_empty()
                                && !matches!(*w, "pub" | "use" | "crate" | "self" | "super" | "as")
                        })
                        .map(String::from),
                );
            }
        }
    }
    for f in files.iter().filter(|f| f.starts_with(&prefix)) {
        let in_f = &f[prefix.len()..];
        if !in_f.ends_with(".rs") || in_f == rel {
            continue;
        }
        let text = without_decls(&read(f).unwrap_or_default());
        let hit = names.iter().any(|n| contains_word(&text, n));
        match mods.get(f) {
            Some(m) => {
                if hit {
                    picked.insert(m.clone());
                }
            }
            // Crate roots and binaries have their own unit tests: filters cannot reach them.
            None if hit && (text.contains("#[test]") || text.contains("#[cfg(test)]")) => {
                return None
            }
            None => {}
        }
    }
    // A filter that another covers (`foo::` holds `foo::bar::`) is redundant.
    let all = picked.clone();
    picked.retain(|m| {
        !all.iter()
            .any(|o| o != m && m.starts_with(&format!("{o}::")))
    });
    Some(picked.into_iter().map(|m| format!("{m}::")).collect())
}

/// The packages that depend on `name` through any chain of path dependencies, and `name`.
fn family<'a>(pkgs: &'a [Pkg], name: &'a str) -> BTreeSet<&'a str> {
    let mut family: BTreeSet<&str> = BTreeSet::from([name]);
    loop {
        let more: Vec<&str> = pkgs
            .iter()
            .filter(|q| !family.contains(q.name.as_str()))
            .filter(|q| q.deps.iter().any(|d| family.contains(d.as_str())))
            .map(|q| q.name.as_str())
            .collect();
        if more.is_empty() {
            return family;
        }
        family.extend(more);
    }
}

/// Can this path reach every target? `named` is the suite's command text.
fn reaches_all(path: &str, named: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    path == "Cargo.toml"
        || path == "Cargo.lock"
        || file.starts_with("rust-toolchain") && !path.contains('/')
        || path.starts_with(".cargo/")
        || (!path.is_empty() && named.contains(path))
}

/// The affected test targets for `changed` paths. `files` is every tracked path, `read` gives a
/// file's text by its workspace-relative path. `named` is the text of the suite's commands: a
/// changed path it names runs everything.
pub fn select(
    pkgs: &[Pkg],
    changed: &[String],
    files: &[String],
    read: &dyn Fn(&str) -> Option<String>,
    named: &str,
) -> Pick {
    select_with(pkgs, changed, files, read, named, None)
}

/// [`select`] with a run-time coverage map (#656, decision 0721). For a directory target the
/// map covers, a changed `src` file picks the modules whose covered set holds it; a file no
/// module's set names, and a module the map does not list, are picked by the name scan as
/// before. A map made before the target's `main.rs` last changed is not used for it.
pub fn select_with(
    pkgs: &[Pkg],
    changed: &[String],
    files: &[String],
    read: &dyn Fn(&str) -> Option<String>,
    named: &str,
    cov: Option<&CoverageMap>,
) -> Pick {
    let mut out: BTreeMap<String, Group> = BTreeMap::new();
    // Directory targets by root file, and the modules selected in them: (package, target) to
    // module names. Modules are a heuristic (decision 0696); the landing batch runs `it` whole.
    let dirs: BTreeMap<String, DirTarget> = if changed.iter().all(|c| ignorable(c)) {
        BTreeMap::new()
    } else {
        pkgs.iter()
            .flat_map(|p| &p.tests)
            .filter_map(|t| Some((t.src.clone(), dir_target(t, files, read)?)))
            .collect()
    };
    let mut picked: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut src_changes: Vec<(&Pkg, String)> = vec![];
    // The map, for each directory target it fits; targets where a changed file fell back to the
    // name scan.
    let fitting = |t: &TestTarget| {
        cov.filter(|m| {
            m.target == t.src && m.main_hash == hash_text(&read(&t.src).unwrap_or_default())
        })
    };
    let mut fell_back: BTreeSet<(String, String)> = BTreeSet::new();
    // Packages run whole, with the targets of their dependents.
    let mut whole: BTreeSet<&str> = BTreeSet::new();
    // Paths read by whichever targets name them: (path, packages to look in, None for all).
    let mut readers: Vec<(String, Option<&Pkg>)> = vec![];
    for path in changed {
        if ignorable(path) {
            continue;
        }
        if reaches_all(path, named) {
            return Pick::All {
                because: format!("`{path}` changed"),
            };
        }
        let Some(p) = owner(pkgs, path) else {
            readers.push((path.clone(), None));
            continue;
        };
        let rel = path[p.dir.len() + 1..].to_string();
        if let Some(t) = p.tests.iter().find(|t| target_owns(t, path)) {
            // A module file selects that module; `main.rs` and shared files select the target.
            let module = dirs.get(&t.src).and_then(|d| d.module_of(path));
            match module {
                Some(m) => {
                    picked
                        .entry((p.name.clone(), t.name.clone()))
                        .or_default()
                        .insert(m.to_string());
                }
                None => {
                    out.entry(p.name.clone())
                        .or_default()
                        .tests
                        .insert(t.name.clone());
                }
            }
        } else if rel.starts_with("src/") {
            src_changes.push((p, rel));
        } else if rel == "Cargo.toml" || rel == "build.rs" {
            whole.insert(p.name.as_str());
        } else if rel.starts_with("tests/") {
            readers.push((path.clone(), Some(p)));
        } else {
            readers.push((path.clone(), None));
        }
    }
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    // A path nothing names selects nothing; a package's build script that names it runs whole.
    for (path, _) in readers.iter().filter(|(_, only)| only.is_none()) {
        let file = path.rsplit('/').next().unwrap_or(path);
        for q in pkgs {
            let build = read(&format!("{}/build.rs", q.dir)).unwrap_or_default();
            if build.contains(path.as_str()) || build.contains(file) {
                whole.insert(q.name.as_str());
            }
        }
    }
    for (path, only) in &readers {
        let file = path.rsplit('/').next().unwrap_or(path);
        let tokens: Vec<String> = match only {
            Some(p) => {
                let rel = path[p.dir.len() + 1..].trim_start_matches("tests/");
                let (dir, _) = rel.split_once('/').unwrap_or(("", rel));
                let stem = file.trim_end_matches(".rs");
                let mut t = vec![];
                if file.ends_with(".rs") {
                    if !dir.is_empty() {
                        t.push(dir.to_string());
                    }
                    if stem != "mod" {
                        t.push(stem.to_string());
                    }
                }
                t
            }
            None => vec![],
        };
        for q in pkgs
            .iter()
            .filter(|q| only.is_none_or(|o| o.name == q.name))
        {
            for t in &q.tests {
                let text = texts
                    .entry(t.src.clone())
                    .or_insert_with(|| target_text(t, files, read));
                let hit = if only.is_some() && !tokens.is_empty() {
                    tokens.iter().any(|w| contains_word(text, w))
                } else if path.starts_with(".mason/") {
                    reads_repo_file(text, path)
                } else {
                    text.contains(path.as_str()) || text.contains(file)
                };
                if hit {
                    out.entry(q.name.clone())
                        .or_default()
                        .tests
                        .insert(t.name.clone());
                }
            }
        }
    }
    for name in &whole {
        out.entry(name.to_string()).or_default().whole = true;
        for q in pkgs
            .iter()
            .filter(|q| family(pkgs, name).contains(q.name.as_str()))
        {
            if q.name != *name {
                let g = out.entry(q.name.clone()).or_default();
                g.tests.extend(q.tests.iter().map(|t| t.name.clone()));
            }
        }
    }
    // Per package: the unit-test module filters, `None` for the lib and bins whole.
    let mut unit: BTreeMap<String, Option<BTreeSet<String>>> = BTreeMap::new();
    for (p, rel) in &src_changes {
        let content = read(&format!("{}/{rel}", p.dir)).unwrap_or_default();
        let g = out.entry(p.name.clone()).or_default();
        g.doc |= p.lib.is_some() && content.contains("```");
        let mods = lib_modules(p, files);
        match p
            .lib
            .is_some()
            .then(|| unit_filters(p, rel, &content, &mods, files, read))
        {
            Some(Some(f)) => unit
                .entry(p.name.clone())
                .or_insert_with(|| Some(BTreeSet::new()))
                .as_mut()
                .map(|set| set.extend(f)),
            _ => {
                unit.insert(p.name.clone(), None);
                None
            }
        };
        let fam = family(pkgs, &p.name);
        for q in pkgs.iter().filter(|q| fam.contains(q.name.as_str())) {
            for t in &q.tests {
                if let Some(d) = dirs.get(&t.src) {
                    // Each module on its own: neither the shared harness `main.rs` includes
                    // nor `main.rs` itself makes every module reference the change.
                    let map = fitting(t);
                    let full = format!("{}/{rel}", p.dir);
                    for (m, text) in &d.mods {
                        let (covered, scan) = match map {
                            Some(c) => {
                                let listed = c.modules.get(m);
                                let unknown = listed.is_none() || !c.knows(&full);
                                (listed.is_some_and(|f| f.contains(&full)), unknown)
                            }
                            None => (false, true),
                        };
                        if map.is_some() && scan {
                            fell_back.insert((q.name.clone(), t.name.clone()));
                        }
                        if covered || (scan && references(p, q.name == p.name, rel, &content, text))
                        {
                            picked
                                .entry((q.name.clone(), t.name.clone()))
                                .or_default()
                                .insert(m.clone());
                        }
                    }
                    continue;
                }
                let text = texts
                    .entry(t.src.clone())
                    .or_insert_with(|| target_text(t, files, read));
                if references(p, q.name == p.name, rel, &content, text) {
                    out.entry(q.name.clone())
                        .or_default()
                        .tests
                        .insert(t.name.clone());
                }
            }
        }
    }
    for (name, filters) in unit {
        let Some(p) = pkgs.iter().find(|q| q.name == name) else {
            continue;
        };
        let g = out.entry(name).or_default();
        let total: BTreeSet<String> = lib_modules(p, files).into_values().collect();
        match filters {
            Some(f) if !f.is_empty() && f.len() * 2 <= total.len() => {
                g.unit_modules.extend(f.iter().cloned());
                g.unit_lib = p.lib.is_some();
                g.unit_bins = !p.bins.is_empty();
                g.notes.push(format!(
                    "{} lib: {} of {} modules, by name scan",
                    p.name,
                    f.len(),
                    total.len()
                ));
            }
            _ => {
                g.lib |= p.lib.is_some();
                g.bins |= !p.bins.is_empty();
            }
        }
    }
    // More than half of a target's modules: the filter list saves little, run it whole.
    for ((pkg, target), mods) in picked {
        let total = pkgs
            .iter()
            .filter(|q| q.name == pkg)
            .flat_map(|q| &q.tests)
            .find(|t| t.name == target)
            .and_then(|t| dirs.get(&t.src))
            .map_or(0, |d| d.mods.len());
        let how = pkgs
            .iter()
            .filter(|q| q.name == pkg)
            .flat_map(|q| &q.tests)
            .find(|t| t.name == target)
            .and_then(&fitting)
            .map(|m| {
                let back = fell_back.contains(&(pkg.clone(), target.clone()));
                format!(
                    "by the coverage map of {}{}",
                    m.commit.chars().take(7).collect::<String>(),
                    if back {
                        " (name scan for files it lacks)"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_else(|| "by name scan (no coverage map)".to_string());
        let g = out.entry(pkg).or_default();
        if mods.len() * 2 > total {
            g.tests.insert(target);
        } else {
            g.notes.push(format!(
                "{target}: {} of {total} modules, {how}",
                mods.len()
            ));
            g.modules
                .entry(target)
                .or_default()
                .extend(mods.iter().map(|m| format!("{m}::")));
        }
    }
    Pick::Some(out)
}

/// `-p pkg --lib --bins --test a --test b` for each package, in name order. Doc tests go in a
/// group of their own (`-p pkg --doc`): cargo refuses "--doc" next to any other target selector
/// ("can't mix --doc with other target selecting options").
pub fn render(groups: &BTreeMap<String, Group>) -> Vec<String> {
    let mut out = vec![];
    for (name, g) in groups {
        let mut s = format!("-p {name}");
        if g.whole {
            out.push(s);
            continue;
        }
        if g.lib || g.bins || !g.tests.is_empty() {
            for (on, flag) in [(g.lib, "--lib"), (g.bins, "--bins")] {
                if on {
                    s.push(' ');
                    s.push_str(flag);
                }
            }
            for t in &g.tests {
                s.push_str(&format!(" --test {t}"));
            }
            out.push(s);
        }
        if g.doc {
            out.push(format!("-p {name} --doc"));
        }
    }
    // Narrowed unit tests: a group of their own, since the filter would reach any other target.
    for (name, g) in groups {
        let lib_whole = g.lib || !g.unit_lib;
        let bins_whole = g.bins || !g.unit_bins;
        if !(g.whole || g.unit_modules.is_empty() || lib_whole && bins_whole) {
            let mut s = format!("-p {name}");
            if g.unit_lib {
                s.push_str(" --lib");
            }
            if g.unit_bins {
                s.push_str(" --bins");
            }
            let f: Vec<&str> = g.unit_modules.iter().map(String::as_str).collect();
            out.push(format!("{s} -- {}", f.join(" ")));
        }
    }
    // A filtered target is a group of its own, as cargo applies a filter to every target of a
    // command; one run whole (here or by the package) drops its filter.
    for (name, g) in groups {
        for (t, filters) in &g.modules {
            if !g.whole && !g.tests.contains(t) && !filters.is_empty() {
                let f: Vec<&str> = filters.iter().map(String::as_str).collect();
                out.push(format!("-p {name} --test {t} -- {}", f.join(" ")));
            }
        }
    }
    out
}

/// Add the lines of a smoke file (`-p pkg --test name`, `#` comments) to `groups`. A line this
/// cannot read is returned as written, to be run as a group of its own. A line with a test
/// filter (`-p pkg --test it -- module::`) cannot merge into a group, since cargo applies a
/// filter to every target of its command: it is returned as a command group of its own, unless
/// the groups already run its targets whole.
pub fn add_smoke(groups: &mut BTreeMap<String, Group>, text: &str) -> Vec<String> {
    let mut raw = vec![];
    let mut filtered: Vec<(String, Group, Vec<String>)> = vec![];
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut pkg: Option<String> = None;
        let mut g = Group::default();
        let mut ok = true;
        let mut filters: Vec<String> = vec![];
        let mut it = line.split_whitespace();
        while let Some(w) = it.next() {
            match w {
                "--" => {
                    filters = it.by_ref().map(String::from).collect();
                    ok &= !filters.is_empty();
                }
                "-p" => pkg = it.next().map(String::from),
                "--lib" => g.lib = true,
                "--bins" => g.bins = true,
                "--doc" => g.doc = true,
                "--test" => match it.next() {
                    Some(t) => {
                        g.tests.insert(t.to_string());
                    }
                    None => ok = false,
                },
                _ => ok = false,
            }
        }
        match (pkg, ok) {
            (Some(p), true) if !filters.is_empty() => {
                // A filter on the doc tests, or on no target at all, is not a smoke group.
                if g.doc || !(g.lib || g.bins || !g.tests.is_empty()) {
                    raw.push(line.to_string());
                } else {
                    filtered.push((p, g, filters));
                }
            }
            (Some(p), true) => {
                let e = groups.entry(p).or_default();
                e.lib |= g.lib;
                e.bins |= g.bins;
                e.doc |= g.doc;
                e.tests.extend(g.tests);
            }
            _ => raw.push(line.to_string()),
        }
    }
    for (p, g, filters) in filtered {
        let covered = groups.get(&p).is_some_and(|e| {
            e.whole || ((!g.lib || e.lib) && (!g.bins || e.bins) && g.tests.is_subset(&e.tests))
        });
        if covered {
            continue;
        }
        // Two filtered selections of one target merge their filters.
        if !g.lib && !g.bins && g.tests.len() == 1 {
            let t = g.tests.iter().next().cloned().unwrap_or_default();
            if let Some(m) = groups.get_mut(&p).and_then(|e| e.modules.get_mut(&t)) {
                m.extend(filters);
                continue;
            }
        }
        let mut s = format!("-p {p}");
        if g.lib {
            s.push_str(" --lib");
        }
        if g.bins {
            s.push_str(" --bins");
        }
        for t in &g.tests {
            s.push_str(&format!(" --test {t}"));
        }
        s.push_str(" -- ");
        s.push_str(&filters.join(" "));
        if !raw.contains(&s) {
            raw.push(s);
        }
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pkg(name: &str, dir: &str, deps: &[&str], tests: &[&str]) -> Pkg {
        Pkg {
            name: name.into(),
            dir: dir.into(),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            lib: Some(name.replace('-', "_")),
            bins: vec![],
            tests: tests
                .iter()
                .map(|t| TestTarget {
                    name: t.to_string(),
                    src: format!("{dir}/tests/{t}.rs"),
                })
                .collect(),
        }
    }

    struct Fx {
        pkgs: Vec<Pkg>,
        files: BTreeMap<String, String>,
    }

    impl Fx {
        /// `leaf` has tests `own` and `other`; `top` depends on it and has `uses` (calls
        /// `leaf::shapes::area`), `names` (mentions `leaf` and nothing in `shapes`),
        /// and `plain` (never mentions it).
        fn new() -> Fx {
            let pkgs = vec![
                pkg("leaf", "crates/leaf", &[], &["own", "other"]),
                pkg("top", "crates/top", &["leaf"], &["uses", "names", "plain"]),
            ];
            let mut files = BTreeMap::new();
            let mut put = |p: &str, t: &str| {
                files.insert(p.to_string(), t.to_string());
            };
            put(
                "crates/leaf/src/lib.rs",
                "pub mod shapes;\npub mod other_mod;\n",
            );
            put(
                "crates/leaf/src/shapes.rs",
                "pub fn area() -> u32 { 1 }\nfn private() {}\n",
            );
            put("crates/leaf/src/other_mod.rs", "pub fn zzz() {}\n");
            put("crates/leaf/tests/own.rs", "use leaf::shapes::area;\n");
            put("crates/leaf/tests/other.rs", "fn t() {}\n");
            put("crates/top/src/lib.rs", "");
            put("crates/top/tests/uses.rs", "fn t() { leaf::area(); }\n");
            put("crates/top/tests/names.rs", "use leaf::other_mod::zzz;\n");
            put("crates/top/tests/plain.rs", "fn t() {}\n");
            Fx { pkgs, files }
        }

        fn pick(&self, changed: &[&str]) -> Pick {
            self.pick_named(changed, "cargo test")
        }

        fn pick_named(&self, changed: &[&str], named: &str) -> Pick {
            let changed: Vec<String> = changed.iter().map(|s| s.to_string()).collect();
            let files: Vec<String> = self.files.keys().cloned().collect();
            select(
                &self.pkgs,
                &changed,
                &files,
                &|p| self.files.get(p).cloned(),
                named,
            )
        }

        fn rendered(&self, changed: &[&str]) -> Vec<String> {
            match self.pick(changed) {
                Pick::Some(g) => render(&g),
                Pick::All { because } => vec![format!("ALL {because}")],
            }
        }
    }

    #[test]
    fn a_changed_test_file_selects_only_that_target() {
        let fx = Fx::new();
        assert_eq!(
            fx.rendered(&["crates/top/tests/plain.rs"]),
            vec!["-p top --test plain"]
        );
    }

    #[test]
    fn a_src_change_selects_the_unit_tests_and_the_targets_that_reference_it() {
        let fx = Fx::new();
        // `shapes.rs` makes `area` public. `own` names the module, `uses` the item; `other`,
        // `names` and `plain` mention neither.
        assert_eq!(
            fx.rendered(&["crates/leaf/src/shapes.rs"]),
            vec![
                "-p leaf --test own",
                "-p top --test uses",
                "-p leaf --lib -- shapes::"
            ]
        );
    }

    #[test]
    fn a_target_that_does_not_name_the_crate_at_all_is_left_out() {
        let fx = Fx::new();
        let Pick::Some(g) = fx.pick(&["crates/leaf/src/other_mod.rs"]) else {
            panic!("a src change is not undecidable")
        };
        // `names` uses `leaf::other_mod::zzz`; `own` mentions the crate but neither the
        // module nor its item.
        assert!(g["top"].tests.contains("names"));
        assert!(!g["leaf"].tests.contains("own"));
        assert!(!g["top"].tests.contains("plain"));
    }

    fn all(p: &Pick) -> &str {
        match p {
            Pick::All { because } => because,
            Pick::Some(g) => panic!("expected All, got {g:?}"),
        }
    }

    // #652: this test used to say support, Cargo and build files select everything. That is
    // the intended behaviour change, not a weakened test: only the root files still do.
    #[test]
    fn only_root_cargo_files_the_toolchain_and_named_scripts_select_everything() {
        let fx = Fx::new();
        for p in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            ".cargo/config.toml",
        ] {
            assert!(all(&fx.pick(&[p])).contains(p), "{p}");
        }
        let named = fx.pick_named(&["scripts/boot.sh"], "sh scripts/boot.sh && cargo test");
        assert!(all(&named).contains("scripts/boot.sh"));
        assert_eq!(fx.pick(&["scripts/boot.sh"]), Pick::Some(BTreeMap::new()));
    }

    #[test]
    fn a_data_file_selects_the_targets_that_name_it() {
        let mut fx = Fx::new();
        fx.files.insert(
            "crates/top/tests/names.rs".into(),
            "include_str!(\"../../../clients/features.toml\");".into(),
        );
        assert_eq!(
            fx.rendered(&["clients/features.toml"]),
            vec!["-p top --test names"]
        );
        // Nothing names it: nothing of its own, and not everything.
        assert_eq!(
            fx.pick(&[".github/workflows/ci.yml"]),
            Pick::Some(BTreeMap::new())
        );
    }

    #[test]
    fn a_mason_file_selects_only_the_targets_that_read_the_repository_copy() {
        let mut fx = Fx::new();
        for f in ["crates/top/tests/plain.rs", "crates/leaf/tests/own.rs"] {
            fx.files.insert(
                f.into(),
                "std::fs::write(d.join(\".mason/mason.toml\"), \"x\");".into(),
            );
        }
        fx.files.insert(
            "crates/top/tests/names.rs".into(),
            "include_str!(\"../../../.mason/mason.toml\");".into(),
        );
        assert_eq!(
            fx.rendered(&[".mason/mason.toml"]),
            vec!["-p top --test names"]
        );
        fx.files.insert(
            "crates/top/tests/uses.rs".into(),
            "std::fs::read_to_string(root().join(\".mason/mason.toml\"));".into(),
        );
        assert_eq!(
            fx.rendered(&[".mason/mason.toml"]),
            vec!["-p top --test names --test uses"]
        );
        // Outside `.mason/` the bare file name still matches.
        fx.files.insert(
            "crates/top/tests/tpl.rs".into(),
            "write(\"mason.toml\");".into(),
        );
        // A file alone is not a target: `top` must list `tpl` among its tests too.
        fx.pkgs[1].tests.push(TestTarget {
            name: "tpl".into(),
            src: "crates/top/tests/tpl.rs".into(),
        });
        assert!(fx
            .rendered(&["templates/x/mason.toml"])
            .iter()
            .any(|r| r.contains("--test tpl")));
    }

    #[test]
    fn a_file_a_build_script_names_runs_that_package_and_its_dependents() {
        let mut fx = Fx::new();
        fx.files.insert(
            "crates/leaf/build.rs".into(),
            "compile(\"../../schemas/x.proto\")".into(),
        );
        assert_eq!(
            fx.rendered(&["schemas/x.proto"]),
            vec!["-p leaf", "-p top --test names --test plain --test uses"]
        );
    }

    #[test]
    fn a_support_file_selects_the_targets_of_its_package_that_declare_it() {
        let mut fx = Fx::new();
        fx.files
            .insert("crates/leaf/tests/support/mod.rs".into(), String::new());
        fx.files
            .insert("crates/leaf/tests/own.rs".into(), "mod support;\n".into());
        fx.files
            .insert("crates/top/tests/uses.rs".into(), "mod support;\n".into());
        assert_eq!(
            fx.rendered(&["crates/leaf/tests/support/mod.rs"]),
            vec!["-p leaf --test own"]
        );
    }

    #[test]
    fn a_package_manifest_or_build_script_runs_it_whole_with_its_dependents() {
        let fx = Fx::new();
        for p in ["crates/leaf/Cargo.toml", "crates/leaf/build.rs"] {
            assert_eq!(
                fx.rendered(&[p]),
                vec!["-p leaf", "-p top --test names --test plain --test uses"],
                "{p}"
            );
        }
    }

    #[test]
    fn the_smoke_file_alone_selects_nothing_of_its_own_and_an_unnamed_makefile_selects_nothing() {
        let fx = Fx::new();
        assert_eq!(
            fx.pick(&[".mason/smoke-tests.txt"]),
            Pick::Some(BTreeMap::new())
        );
        assert!(reads_none(&[".mason/smoke-tests.txt".to_string()]));
        assert_eq!(fx.pick(&["Makefile"]), Pick::Some(BTreeMap::new()));
        assert!(all(&fx.pick(&["Cargo.toml"])).contains("Cargo.toml"));
    }

    #[test]
    fn docs_and_exit_scripts_select_nothing_of_their_own() {
        let fx = Fx::new();
        assert_eq!(
            fx.pick(&["docs/stages/s1.md", "scripts/exit/s1.sh", "README.md"]),
            Pick::Some(BTreeMap::new())
        );
    }

    #[test]
    fn a_reference_that_cannot_be_decided_includes_the_target() {
        let fx = Fx::new();
        // The crate root names no module: every target that mentions the crate may use it.
        let Pick::Some(g) = fx.pick(&["crates/leaf/src/lib.rs"]) else {
            panic!()
        };
        assert!(g["top"].tests.contains("uses") && g["top"].tests.contains("names"));
        assert!(!g["top"].tests.contains("plain"));
        // A target that runs the crate's binary depends on all of it.
        let mut fx = Fx::new();
        fx.pkgs[0].bins = vec!["leaf-cli".into()];
        fx.files.insert(
            "crates/top/tests/plain.rs".into(),
            "run(\"leaf-cli\");".into(),
        );
        let Pick::Some(g) = fx.pick(&["crates/leaf/src/shapes.rs"]) else {
            panic!()
        };
        assert!(g["top"].tests.contains("plain"));
        assert!(g["leaf"].unit_bins);
    }

    #[test]
    fn a_dir_target_owns_the_files_under_it() {
        let mut fx = Fx::new();
        fx.pkgs[1].tests.push(TestTarget {
            name: "big".into(),
            src: "crates/top/tests/big/main.rs".into(),
        });
        assert_eq!(
            fx.rendered(&["crates/top/tests/big/helpers.rs"]),
            vec!["-p top --test big"]
        );
        assert_eq!(
            fx.rendered(&["crates/top/tests/big/main.rs"]),
            vec!["-p top --test big"]
        );
    }

    #[test]
    fn smoke_lines_merge_into_the_groups() {
        let mut g = BTreeMap::new();
        let raw = add_smoke(
            &mut g,
            "# the fast end-to-end targets\n-p leaf --test own  # one\n-p top --lib\n--workspace\n",
        );
        assert_eq!(render(&g), vec!["-p leaf --test own", "-p top --lib"]);
        assert_eq!(raw, vec!["--workspace"]);
    }

    #[test]
    fn a_filtered_smoke_line_is_its_own_group_unless_the_target_runs_whole() {
        let mut g = BTreeMap::new();
        let raw = add_smoke(&mut g, "-p top --test it -- landing_floor:: e2e::smoke\n");
        assert!(render(&g).is_empty());
        assert_eq!(raw, vec!["-p top --test it -- landing_floor:: e2e::smoke"]);
        let mut sel = BTreeMap::new();
        sel.entry("top".to_string())
            .or_insert_with(Group::default)
            .tests
            .insert("it".into());
        assert!(add_smoke(&mut sel, "-p top --test it -- a::\n").is_empty());
        let mut whole = BTreeMap::new();
        whole
            .entry("top".to_string())
            .or_insert_with(Group::default)
            .whole = true;
        assert!(add_smoke(&mut whole, "-p top --test it -- a::\n").is_empty());
        // An unfiltered smoke line for the same target also covers it.
        let mut both = BTreeMap::new();
        let raw = add_smoke(&mut both, "-p top --test it -- a::\n-p top --test it\n");
        assert!(raw.is_empty());
        // Unreadable: a bare `--` is run raw.
        let mut bad = BTreeMap::new();
        assert_eq!(
            add_smoke(&mut bad, "-p top --test it --\n"),
            vec!["-p top --test it --"]
        );
    }

    #[test]
    fn doc_tests_render_in_their_own_group() {
        let mut g = BTreeMap::new();
        let raw = add_smoke(&mut g, "-p top --test x --doc\n-p leaf --lib --doc\n");
        assert!(raw.is_empty());
        assert_eq!(
            render(&g),
            vec![
                "-p leaf --lib",
                "-p leaf --doc",
                "-p top --test x",
                "-p top --doc"
            ]
        );
        let mut d = BTreeMap::new();
        add_smoke(&mut d, "-p a --doc\n");
        assert_eq!(render(&d), vec!["-p a --doc"]);
        for line in render(&g).iter().chain(render(&d).iter()) {
            assert!(
                !(line.contains("--doc")
                    && (line.contains("--lib")
                        || line.contains("--bins")
                        || line.contains("--test"))),
                "{line}"
            );
        }
    }

    #[test]
    fn words_match_whole_identifiers() {
        assert!(contains_word("use leaf::x;", "leaf"));
        assert!(!contains_word("use leafy::x;", "leaf"));
        assert!(!contains_word("my_leaf", "leaf"));
    }

    #[test]
    fn packages_read_cargo_metadata() {
        let meta = serde_json::json!({
            "workspace_members": ["a 0.1.0 (path+file:///w/a)"],
            "packages": [{
                "id": "a 0.1.0 (path+file:///w/a)", "name": "a-b",
                "manifest_path": "/w/crates/a/Cargo.toml",
                "dependencies": [{"name": "c", "path": "/w/crates/c"}, {"name": "serde"}],
                "targets": [
                    {"kind": ["lib"], "name": "a-b", "src_path": "/w/crates/a/src/lib.rs"},
                    {"kind": ["bin"], "name": "abd", "src_path": "/w/crates/a/src/main.rs"},
                    {"kind": ["test"], "name": "t1", "src_path": "/w/crates/a/tests/t1.rs"}
                ]
            }]
        });
        let pk = packages(&meta, &PathBuf::from("/w")).unwrap();
        assert_eq!(pk.len(), 1);
        assert_eq!(pk[0].dir, "crates/a");
        assert_eq!(pk[0].deps, vec!["c"]);
        assert_eq!(pk[0].lib.as_deref(), Some("a_b"));
        assert_eq!(pk[0].bins, vec!["abd"]);
        assert_eq!(pk[0].tests[0].src, "crates/a/tests/t1.rs");
    }

    /// `leaf` gains a directory target `it` (`main.rs`, a `#[path]` harness, modules `mod_a`,
    /// `mod_b`, `mod_c`); only `mod_a` names `shapes`.
    fn with_dir_target() -> Fx {
        let mut fx = Fx::new();
        fx.pkgs[0].tests.push(TestTarget {
            name: "it".into(),
            src: "crates/leaf/tests/it/main.rs".into(),
        });
        let mut put = |p: &str, t: &str| {
            fx.files.insert(p.to_string(), t.to_string());
        };
        put(
            "crates/leaf/tests/it/main.rs",
            "#[path = \"../support/mod.rs\"]\nmod support;\n\nmod mod_a;\nmod mod_b;\nmod mod_c;\n",
        );
        put(
            "crates/leaf/tests/support/mod.rs",
            "fn start() { env!(\"CARGO_BIN_EXE_masond\"); }\n",
        );
        put("crates/leaf/tests/it/mod_a.rs", "use leaf::shapes::area;\n");
        put("crates/leaf/tests/it/mod_b.rs", "fn t() {}\n");
        put("crates/leaf/tests/it/mod_c.rs", "fn t() {}\n");
        fx
    }

    #[test]
    fn a_src_change_selects_only_the_modules_that_reference_it() {
        let fx = with_dir_target();
        let r = fx.rendered(&["crates/leaf/src/shapes.rs"]);
        assert!(
            r.contains(&"-p leaf --test it -- mod_a::".to_string()),
            "{r:?}"
        );
        assert!(!r
            .iter()
            .any(|l| l.contains("mod_b") || l == "-p leaf --test it"));
    }

    #[test]
    fn a_changed_module_file_selects_that_module_only() {
        let fx = with_dir_target();
        assert_eq!(
            fx.rendered(&["crates/leaf/tests/it/mod_b.rs"]),
            vec!["-p leaf --test it -- mod_b::"]
        );
    }

    #[test]
    fn a_module_naming_the_package_binary_is_selected_for_any_src_change() {
        let mut fx = with_dir_target();
        fx.pkgs[0].bins = vec!["leafd".into()];
        fx.files.insert(
            "crates/leaf/tests/it/mod_c.rs".into(),
            "fn t() { run(\"leafd\"); }\n".into(),
        );
        let r = fx.rendered(&["crates/leaf/src/other_mod.rs"]);
        assert!(
            r.iter().any(|l| l == "-p leaf --test it -- mod_c::"),
            "{r:?}"
        );
    }

    #[test]
    fn main_rs_or_a_path_included_file_selects_the_target_whole() {
        let fx = with_dir_target();
        for p in [
            "crates/leaf/tests/it/main.rs",
            "crates/leaf/tests/support/mod.rs",
        ] {
            assert_eq!(fx.rendered(&[p]), vec!["-p leaf --test it"], "{p}");
        }
    }

    #[test]
    fn more_than_half_the_modules_select_the_target_whole() {
        let fx = with_dir_target();
        let r = fx.rendered(&[
            "crates/leaf/tests/it/mod_a.rs",
            "crates/leaf/tests/it/mod_b.rs",
        ]);
        assert_eq!(r, vec!["-p leaf --test it"]);
    }

    #[test]
    fn whole_beats_filtered_and_filtered_merge() {
        let fx = with_dir_target();
        assert_eq!(
            fx.rendered(&[
                "crates/leaf/tests/it/mod_b.rs",
                "crates/leaf/tests/it/main.rs"
            ]),
            vec!["-p leaf --test it"]
        );
        let Pick::Some(mut g) = fx.pick(&["crates/leaf/tests/it/mod_b.rs"]) else {
            panic!()
        };
        let raw = add_smoke(&mut g, "-p leaf --test it -- mod_c::\n");
        assert!(raw.is_empty(), "{raw:?}");
        assert_eq!(render(&g), vec!["-p leaf --test it -- mod_b:: mod_c::"]);
        let raw = add_smoke(&mut g, "-p leaf --test it\n");
        assert!(raw.is_empty());
        assert_eq!(render(&g), vec!["-p leaf --test it"]);
    }

    fn map_for(fx: &Fx, rows: &[(&str, &[&str])]) -> CoverageMap {
        CoverageMap {
            commit: "1234567890".into(),
            made_at: 1,
            target: "crates/leaf/tests/it/main.rs".into(),
            main_hash: hash_text(&fx.files["crates/leaf/tests/it/main.rs"]),
            modules: rows
                .iter()
                .map(|(m, f)| (m.to_string(), f.iter().map(|s| s.to_string()).collect()))
                .collect(),
        }
    }

    fn rendered_with(fx: &Fx, changed: &[&str], cov: Option<&CoverageMap>) -> Vec<String> {
        let changed: Vec<String> = changed.iter().map(|s| s.to_string()).collect();
        let files: Vec<String> = fx.files.keys().cloned().collect();
        match select_with(
            &fx.pkgs,
            &changed,
            &files,
            &|p| fx.files.get(p).cloned(),
            "cargo test",
            cov,
        ) {
            Pick::Some(g) => render(&g),
            Pick::All { because } => vec![format!("ALL {because}")],
        }
    }

    /// The lines of a rendered pick that run the directory target `it`.
    fn it_lines(v: Vec<String>) -> Vec<String> {
        v.into_iter().filter(|l| l.contains("--test it")).collect()
    }

    #[test]
    fn a_coverage_map_picks_the_modules_that_ran_the_file() {
        let fx = with_dir_target();
        let m = map_for(
            &fx,
            &[
                ("mod_a", &["crates/leaf/src/other_mod.rs"]),
                ("mod_b", &["crates/leaf/src/shapes.rs"]),
                ("mod_c", &[]),
            ],
        );
        // The name scan would pick mod_a (it names `shapes`); the map says mod_b ran it.
        assert_eq!(
            it_lines(rendered_with(&fx, &["crates/leaf/src/shapes.rs"], Some(&m))),
            vec!["-p leaf --test it -- mod_b::"]
        );
        assert_eq!(
            it_lines(rendered_with(&fx, &["crates/leaf/src/shapes.rs"], None)),
            vec!["-p leaf --test it -- mod_a::"]
        );
    }

    #[test]
    fn support_changes_still_select_the_target_whole_with_a_map() {
        let fx = with_dir_target();
        let m = map_for(&fx, &[("mod_a", &[]), ("mod_b", &[]), ("mod_c", &[])]);
        let c = ["crates/leaf/tests/support/mod.rs"];
        assert_eq!(
            rendered_with(&fx, &c, Some(&m)),
            rendered_with(&fx, &c, None)
        );
        assert!(
            it_lines(rendered_with(&fx, &c, Some(&m))).contains(&"-p leaf --test it".to_string())
        );
    }

    #[test]
    fn an_unmapped_file_a_stale_map_or_no_map_gives_the_name_scan_pick() {
        let mut fx = with_dir_target();
        fx.files
            .insert("crates/leaf/src/new.rs".into(), "pub fn nn() {}\n".into());
        let m = map_for(&fx, &[("mod_a", &[]), ("mod_b", &[]), ("mod_c", &[])]);
        let shapes = ["crates/leaf/src/shapes.rs"];
        let scan = rendered_with(&fx, &shapes, None);
        assert_eq!(it_lines(scan.clone()), vec!["-p leaf --test it -- mod_a::"]);
        // shapes.rs is in no module's set: the name scan decides.
        assert_eq!(rendered_with(&fx, &shapes, Some(&m)), scan);
        let new = ["crates/leaf/src/new.rs"];
        assert_eq!(
            rendered_with(&fx, &new, Some(&m)),
            rendered_with(&fx, &new, None)
        );
        // A map made before main.rs changed is not used.
        let mut stale = map_for(&fx, &[("mod_b", &["crates/leaf/src/shapes.rs"])]);
        stale.main_hash ^= 1;
        assert_eq!(rendered_with(&fx, &shapes, Some(&stale)), scan);
    }

    #[test]
    fn the_note_says_how_the_modules_were_picked() {
        let fx = with_dir_target();
        let m = map_for(
            &fx,
            &[
                ("mod_a", &[]),
                ("mod_b", &["crates/leaf/src/shapes.rs"]),
                ("mod_c", &[]),
            ],
        );
        let notes = |cov: Option<&CoverageMap>| {
            let changed = vec!["crates/leaf/src/shapes.rs".to_string()];
            let files: Vec<String> = fx.files.keys().cloned().collect();
            match select_with(
                &fx.pkgs,
                &changed,
                &files,
                &|p| fx.files.get(p).cloned(),
                "x",
                cov,
            ) {
                Pick::Some(g) => g.values().flat_map(|g| g.notes.clone()).collect::<Vec<_>>(),
                Pick::All { .. } => vec![],
            }
        };
        assert_eq!(
            notes(Some(&m)),
            vec![
                "leaf lib: 1 of 2 modules, by name scan",
                "it: 1 of 3 modules, by the coverage map of 1234567"
            ]
        );
        assert_eq!(
            notes(None),
            vec![
                "leaf lib: 1 of 2 modules, by name scan",
                "it: 1 of 3 modules, by name scan (no coverage map)"
            ]
        );
    }

    #[test]
    fn the_rendered_pick_is_one_test_command_with_filters_after_the_dashes() {
        let fx = with_dir_target();
        let m = map_for(
            &fx,
            &[
                ("mod_a", &["crates/leaf/src/shapes.rs"]),
                ("mod_b", &[]),
                ("mod_c", &[]),
            ],
        );
        let r = rendered_with(&fx, &["crates/leaf/src/shapes.rs"], Some(&m));
        let it = it_lines(r.clone());
        assert_eq!(it, vec!["-p leaf --test it -- mod_a::"]);
        // The build form has no filters.
        let b = crate::scope::expand(
            "cargo test --no-run {tests_for_change}",
            &crate::scope::Narrow::Targets(r),
            true,
        );
        assert!(b.contains("--test it") && !b.contains("mod_a"), "{b}");
    }
}
