// SPDX-License-Identifier: GPL-3.0-or-later

//! Question 1 of `mason init`: look at the repository and the machine, and say what was found.
//! Reads only; the one process it runs is `git`.

use super::questions::{parse_remote, project_id, Agent, Detected};
use mason_landing::git;
use std::path::{Path, PathBuf};

pub fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(name))
            .find(|c| c.is_file())
    })
}

fn has(root: &Path, name: &str) -> bool {
    root.join(name).exists()
}

fn any_with_ext(dir: &Path, ext: &str) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == ext))
        })
        .unwrap_or(false)
}

/// The Xcode project, relative to `root`: at the top, or one folder down (an app kept in
/// `App/App.xcodeproj` beside its README is common). Hidden folders are skipped; the first by
/// name wins, so the answer does not depend on the order the disk lists them in.
pub fn xcode_project(root: &Path) -> Option<PathBuf> {
    let projects_in = |dir: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "xcodeproj") && p.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    };
    if let Some(p) = projects_in(root).into_iter().next() {
        return p.strip_prefix(root).ok().map(Path::to_path_buf);
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && !p
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                        && p.extension().is_none_or(|x| x != "xcodeproj")
                })
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs.iter()
        .find_map(|d| projects_in(d).into_iter().next())
        .and_then(|p| p.strip_prefix(root).ok().map(Path::to_path_buf))
}

/// Languages by their build files: `rust`, `python`, `swift`, `go`, `node`, `gradle`, `make`.
pub fn languages(root: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut add = |cond: bool, name: &str| {
        if cond {
            out.push(name.to_string());
        }
    };
    add(has(root, "Cargo.toml"), "rust");
    add(
        has(root, "pyproject.toml")
            || has(root, "setup.py")
            || has(root, "requirements.txt")
            || has(root, "tox.ini"),
        "python",
    );
    add(
        has(root, "Package.swift") || xcode_project(root).is_some(),
        "swift",
    );
    add(has(root, "go.mod"), "go");
    add(has(root, "package.json"), "node");
    add(
        has(root, "build.gradle") || has(root, "build.gradle.kts") || has(root, "gradlew"),
        "gradle",
    );
    add(has(root, "Makefile"), "make");
    out
}

pub fn ci_systems(root: &Path) -> Vec<String> {
    let mut out = vec![];
    if any_with_ext(&root.join(".github/workflows"), "yml")
        || any_with_ext(&root.join(".github/workflows"), "yaml")
    {
        out.push("GitHub Actions".to_string());
    }
    if has(root, ".gitlab-ci.yml") {
        out.push("GitLab CI".to_string());
    }
    if has(root, "Jenkinsfile") {
        out.push("Jenkins".to_string());
    }
    out
}

/// The repository at or above `dir`, as question 1 sees it.
pub fn detect(dir: &Path) -> Result<Detected, String> {
    let root = git::run(dir, &["rev-parse", "--show-toplevel"])
        .map(|s| PathBuf::from(s.trim()))
        .map_err(|_| {
            format!(
                "{} is not inside a git repository; run `git init` first",
                dir.display()
            )
        })?;
    let root = root.canonicalize().unwrap_or(root);
    let remote_name = git::run(&root, &["remote"]).ok().and_then(|r| {
        let names: Vec<String> = r.lines().map(str::to_string).collect();
        names
            .iter()
            .find(|n| *n == "origin")
            .cloned()
            .or_else(|| names.first().cloned())
    });
    let remote = remote_name
        .as_ref()
        .and_then(|n| git::run(&root, &["remote", "get-url", n]).ok())
        .and_then(|u| parse_remote(&u));
    let branch = remote_name
        .as_ref()
        .and_then(|n| {
            git::run(
                &root,
                &["symbolic-ref", "--short", &format!("refs/remotes/{n}/HEAD")],
            )
            .ok()
        })
        .map(|s| {
            s.trim()
                .split_once('/')
                .map_or(s.trim().to_string(), |(_, b)| b.to_string())
        })
        .filter(|b| !b.is_empty())
        .or_else(|| {
            git::run(&root, &["branch", "--show-current"])
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|b| !b.is_empty())
        })
        .unwrap_or_else(|| "main".into());
    let agents = [(Agent::Claude, "claude"), (Agent::Codex, "codex")]
        .into_iter()
        .filter_map(|(a, bin)| on_path(bin).map(|p| (a, p)))
        .collect();
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(Detected {
        id: project_id(&name),
        languages: languages(&root),
        ci: ci_systems(&root),
        existing_mason: root.join(".mason").exists(),
        root,
        remote_name,
        remote,
        branch,
        agents,
    })
}

/// What was found, in words, for the person to confirm.
pub fn describe(d: &Detected) -> String {
    let mut s = format!("Found a git repository at {}\n", d.root.display());
    match (&d.remote_name, &d.remote) {
        (Some(n), Some(r)) => s.push_str(&format!("  remote    {n} -> {} {}\n", r.host, r.repo)),
        (Some(n), None) => s.push_str(&format!("  remote    {n} (not a host MASON knows)\n")),
        _ => s.push_str("  remote    none (tasks will land on the local base branch)\n"),
    }
    s.push_str(&format!("  branch    {}\n", d.branch));
    let list = |v: &[String]| {
        if v.is_empty() {
            "none found".to_string()
        } else {
            v.join(", ")
        }
    };
    s.push_str(&format!(
        "  languages {}\n  CI        {}\n",
        list(&d.languages),
        list(&d.ci)
    ));
    let agents: Vec<String> = d
        .agents
        .iter()
        .map(|(a, p)| format!("{} ({})", a.name(), p.display()))
        .collect();
    s.push_str(&format!("  agents    {}\n", list(&agents)));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_xcode_project_is_found_at_the_top_or_one_folder_down() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(xcode_project(d.path()), None);
        std::fs::create_dir_all(d.path().join(".build/Hidden.xcodeproj")).unwrap();
        std::fs::create_dir_all(d.path().join("b/B.xcodeproj")).unwrap();
        std::fs::create_dir_all(d.path().join("a/A.xcodeproj")).unwrap();
        assert_eq!(
            xcode_project(d.path()),
            Some(PathBuf::from("a/A.xcodeproj"))
        );
        assert_eq!(languages(d.path()), ["swift"]);
        std::fs::create_dir_all(d.path().join("Top.xcodeproj")).unwrap();
        assert_eq!(
            xcode_project(d.path()),
            Some(PathBuf::from("Top.xcodeproj"))
        );
        // Two folders down is too deep to guess.
        let deep = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(deep.path().join("x/y/Z.xcodeproj")).unwrap();
        assert_eq!(xcode_project(deep.path()), None);
    }

    #[test]
    fn languages_and_ci_come_from_build_files() {
        let d = tempfile::tempdir().unwrap();
        assert!(languages(d.path()).is_empty() && ci_systems(d.path()).is_empty());
        for f in [
            "Cargo.toml",
            "go.mod",
            "Package.swift",
            "requirements.txt",
            "package.json",
            "Makefile",
        ] {
            std::fs::write(d.path().join(f), "").unwrap();
        }
        std::fs::create_dir_all(d.path().join(".github/workflows")).unwrap();
        std::fs::write(d.path().join(".github/workflows/ci.yml"), "").unwrap();
        assert_eq!(
            languages(d.path()),
            ["rust", "python", "swift", "go", "node", "make"]
        );
        assert_eq!(ci_systems(d.path()), ["GitHub Actions"]);
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_refused_with_the_fix() {
        let d = tempfile::tempdir().unwrap();
        let e = detect(d.path()).unwrap_err();
        assert!(e.contains("git init"), "{e}");
    }
}
