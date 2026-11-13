// SPDX-License-Identifier: GPL-3.0-or-later

//! Which cargo packages a change touches (s210).
//!
//! While a task is developed, `run_check` can run only the packages the worktree's diff
//! touches, plus the workspace packages that depend on them (their tests can break too).
//! `cargo metadata` gives the packages and their path dependencies. A path that belongs to no
//! package (the root `Cargo.toml`, the toolchain file) means anything may be affected, and so
//! does a change nobody can place: then there is no list and the full command runs. Docs and
//! markdown are placed nowhere on purpose; a change made only of them has no packages either.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// The packages for the changed `paths` (relative to the worktree), as sorted names. `None`
/// when no list can be given: no cargo, no workspace, no package touched, or a path outside
/// every package that may matter.
pub fn changed_packages(wt: &Path, paths: &[String]) -> Option<Vec<String>> {
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
    let meta: Value = serde_json::from_slice(&out.stdout).ok()?;
    let root = std::fs::canonicalize(wt).ok()?;
    select(&meta, &root, paths)
}

fn ignorable(path: &str) -> bool {
    path.starts_with("docs/") || path.ends_with(".md")
}

/// The pure part: `meta` is `cargo metadata --no-deps` output and `root` the worktree.
pub fn select(meta: &Value, root: &Path, paths: &[String]) -> Option<Vec<String>> {
    struct Pkg {
        name: String,
        dir: String,
        deps: Vec<String>,
    }
    let members: BTreeSet<&str> = meta["workspace_members"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut pkgs: Vec<Pkg> = vec![];
    for p in meta["packages"].as_array()? {
        if !members.is_empty() && !members.contains(p["id"].as_str().unwrap_or_default()) {
            continue;
        }
        let manifest = Path::new(p["manifest_path"].as_str()?);
        let dir = manifest.parent()?.strip_prefix(root).ok()?;
        pkgs.push(Pkg {
            name: p["name"].as_str()?.to_string(),
            dir: dir.to_string_lossy().trim_matches('/').to_string(),
            deps: p["dependencies"]
                .as_array()
                .map(|d| {
                    d.iter()
                        .filter(|d| d["path"].is_string())
                        .filter_map(|d| d["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    let mut touched: BTreeSet<String> = BTreeSet::new();
    for path in paths {
        let owner = pkgs
            .iter()
            .filter(|p| {
                p.dir.is_empty()
                    || path == &p.dir
                    || path
                        .strip_prefix(&p.dir)
                        .is_some_and(|r| r.starts_with('/'))
            })
            .max_by_key(|p| p.dir.len());
        match owner {
            // A root package owns every path; the files a real package does not own are the
            // workspace's own, which no package can answer for.
            Some(p) if !p.dir.is_empty() => {
                touched.insert(p.name.clone());
            }
            _ if ignorable(path) => {}
            _ => return None,
        }
    }
    if touched.is_empty() {
        return None;
    }
    // Close over the dependents until nothing new joins.
    loop {
        let more: Vec<String> = pkgs
            .iter()
            .filter(|p| !touched.contains(&p.name) && p.deps.iter().any(|d| touched.contains(d)))
            .map(|p| p.name.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        touched.extend(more);
    }
    Some(touched.into_iter().collect())
}

/// `-p a -p b`, for `{packages}`.
pub fn flags(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("-p {n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    /// core <- engine <- daemon; `lonely` depends on nothing and nothing on it.
    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(
            &r.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n",
        );
        let krate = |name: &str, deps: &[&str]| {
            let deps: String = deps
                .iter()
                .map(|d| format!("{d} = {{ path = \"../{d}\" }}\n"))
                .collect();
            write(
                &r.join(format!("crates/{name}/Cargo.toml")),
                &format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\n{deps}"
                ),
            );
            write(&r.join(format!("crates/{name}/src/lib.rs")), "");
        };
        krate("mason-core", &[]);
        krate("mason-engine", &["mason-core"]);
        krate("mason-daemon", &["mason-engine"]);
        krate("lonely", &[]);
        d
    }

    fn pick(d: &tempfile::TempDir, paths: &[&str]) -> Option<Vec<String>> {
        let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
        changed_packages(d.path(), &paths)
    }

    #[test]
    fn a_change_under_a_crate_selects_it_and_its_dependents_only() {
        let d = fixture();
        assert_eq!(
            pick(&d, &["crates/mason-daemon/src/x.rs"]),
            Some(vec!["mason-daemon".to_string()])
        );
        assert_eq!(
            pick(&d, &["crates/mason-engine/src/x.rs", "docs/stages/s1.md"]),
            Some(vec!["mason-daemon".to_string(), "mason-engine".to_string()])
        );
        assert_eq!(
            pick(&d, &["crates/mason-core/src/x.rs"]),
            Some(vec![
                "mason-core".to_string(),
                "mason-daemon".to_string(),
                "mason-engine".to_string()
            ])
        );
    }

    #[test]
    fn what_no_package_can_answer_for_gives_no_list() {
        let d = fixture();
        assert_eq!(pick(&d, &["Cargo.toml"]), None);
        assert_eq!(
            pick(&d, &["crates/lonely/src/x.rs", "rust-toolchain.toml"]),
            None
        );
        assert_eq!(pick(&d, &["docs/only.md"]), None);
        assert_eq!(pick(&d, &[]), None);
    }

    #[test]
    fn flags_are_one_p_each() {
        assert_eq!(flags(&["a".into(), "b".into()]), "-p a -p b");
    }
}
