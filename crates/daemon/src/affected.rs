// SPDX-License-Identifier: GPL-3.0-or-later

//! Which cargo test targets a change can affect (#538, decision 0539).
//!
//! `scope` picks whole packages. This picks test targets inside them: a changed test file
//! runs itself; a changed `src` file runs its crate's unit tests, plus the integration
//! targets of that crate and of its dependents whose source mentions the changed module or a
//! public item of it. The scan is static (words in source files), never timing. A reference
//! it cannot decide includes the target. A path nobody can place (`Cargo.*`, `build.rs`, a
//! test's `support/` directory, the toolchain file) selects everything, as before.
//!
//! The landing batch runs the whole workspace, so what this leaves out is still run before
//! anything reaches `develop`.

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Something nobody can place changed: run everything.
    All,
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
fn ignorable(path: &str) -> bool {
    path.starts_with("docs/") || path.ends_with(".md") || path.starts_with("scripts/exit/")
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

/// The affected test targets for `changed` paths. `files` is every tracked path, `read` gives a
/// file's text by its workspace-relative path.
pub fn select(
    pkgs: &[Pkg],
    changed: &[String],
    files: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Pick {
    let mut out: BTreeMap<String, Group> = BTreeMap::new();
    let mut src_changes: Vec<(&Pkg, String)> = vec![];
    for path in changed {
        if ignorable(path) {
            continue;
        }
        let Some(p) = owner(pkgs, path) else {
            return Pick::All;
        };
        let rel = path[p.dir.len() + 1..].to_string();
        if let Some(t) = p.tests.iter().find(|t| target_owns(t, path)) {
            out.entry(p.name.clone())
                .or_default()
                .tests
                .insert(t.name.clone());
        } else if rel.starts_with("src/") {
            src_changes.push((p, rel));
        } else {
            // `Cargo.toml`, `build.rs`, a test's `support/` directory, fixtures: no one can
            // say which targets read them.
            return Pick::All;
        }
    }
    let mut texts: BTreeMap<String, String> = BTreeMap::new();
    for (p, rel) in &src_changes {
        let content = read(&format!("{}/{rel}", p.dir)).unwrap_or_default();
        let g = out.entry(p.name.clone()).or_default();
        g.lib |= p.lib.is_some();
        g.bins |= !p.bins.is_empty();
        g.doc |= p.lib.is_some() && content.contains("```");
        // The crate and everything that depends on it, through any chain of path dependencies.
        let mut family: BTreeSet<&str> = BTreeSet::from([p.name.as_str()]);
        loop {
            let more: Vec<&str> = pkgs
                .iter()
                .filter(|q| !family.contains(q.name.as_str()))
                .filter(|q| q.deps.iter().any(|d| family.contains(d.as_str())))
                .map(|q| q.name.as_str())
                .collect();
            if more.is_empty() {
                break;
            }
            family.extend(more);
        }
        for q in pkgs.iter().filter(|q| family.contains(q.name.as_str())) {
            for t in &q.tests {
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
    Pick::Some(out)
}

/// `-p pkg --lib --bins --doc --test a --test b` for each package, in name order.
pub fn render(groups: &BTreeMap<String, Group>) -> Vec<String> {
    groups
        .iter()
        .filter_map(|(name, g)| {
            let mut s = format!("-p {name}");
            if !g.whole {
                if !(g.lib || g.bins || g.doc || !g.tests.is_empty()) {
                    return None;
                }
                for (on, flag) in [(g.lib, "--lib"), (g.bins, "--bins"), (g.doc, "--doc")] {
                    if on {
                        s.push(' ');
                        s.push_str(flag);
                    }
                }
                for t in &g.tests {
                    s.push_str(&format!(" --test {t}"));
                }
            }
            Some(s)
        })
        .collect()
}

/// Add the lines of a smoke file (`-p pkg --test name`, `#` comments) to `groups`. A line this
/// cannot read is returned as written, to be run as a group of its own.
pub fn add_smoke(groups: &mut BTreeMap<String, Group>, text: &str) -> Vec<String> {
    let mut raw = vec![];
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut pkg: Option<String> = None;
        let mut g = Group::default();
        let mut ok = true;
        let mut it = line.split_whitespace();
        while let Some(w) = it.next() {
            match w {
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
            let changed: Vec<String> = changed.iter().map(|s| s.to_string()).collect();
            let files: Vec<String> = self.files.keys().cloned().collect();
            select(&self.pkgs, &changed, &files, &|p| {
                self.files.get(p).cloned()
            })
        }

        fn rendered(&self, changed: &[&str]) -> Vec<String> {
            match self.pick(changed) {
                Pick::Some(g) => render(&g),
                Pick::All => vec!["ALL".into()],
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
            vec!["-p leaf --lib --test own", "-p top --test uses"]
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

    #[test]
    fn support_cargo_and_build_files_select_everything() {
        let mut fx = Fx::new();
        fx.files
            .insert("crates/top/tests/support/mod.rs".into(), String::new());
        for p in [
            "crates/top/tests/support/mod.rs",
            "crates/top/Cargo.toml",
            "crates/top/build.rs",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "scripts/check-spdx.sh",
        ] {
            assert_eq!(fx.pick(&[p]), Pick::All, "{p}");
        }
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
        assert!(g["leaf"].bins);
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
}
