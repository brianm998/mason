// SPDX-License-Identifier: GPL-3.0-or-later

//! A project's agent skills: read from `.mason/skills/` on its base commit, as prompt
//! packs are, and the record of the ones each step run called (s1153, decision 1142).

use crate::config::ProjectCfg;
use mason_adapters::skills::SkillFile;
use std::path::Path;

const DIR: &str = ".mason/skills/";
/// One file, and all of a project's skills together.
const MAX_FILE: u64 = 1 << 20;
const MAX_ALL: u64 = 8 << 20;
/// How many different skills a run's record keeps.
const MAX_RECORDED: usize = 100;

/// A skill's name, as the Agent Skills specification has it.
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let o = mason_landing::git::command()
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(o.stdout)
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

fn try_load(p: &ProjectCfg) -> Result<Vec<SkillFile>, String> {
    let sha = mason_landing::git::rev_parse(&p.path, &p.base_ref()).map_err(|e| e.to_string())?;
    // `-l` adds each blob's size, so an outsized one is never read.
    let listing = git_bytes(
        &p.path,
        &["ls-tree", "-r", "-l", "-z", "--full-tree", &sha, "--", DIR],
    )?;
    let mut files = vec![];
    let mut total = 0;
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        // `<mode> <type> <object> <size>\t<path>`
        let Ok(entry) = std::str::from_utf8(entry) else {
            tracing::warn!(project = %p.id, "a skill file's name is not UTF-8; left out");
            continue;
        };
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let meta: Vec<&str> = meta.split_whitespace().collect();
        let [mode, "blob", object, size] = meta[..] else {
            continue;
        };
        let Some(path) = path.strip_prefix(DIR) else {
            continue;
        };
        // Symbolic links (120000) are left out: they could point anywhere.
        if !matches!(mode, "100644" | "100755")
            || !path.split_once('/').is_some_and(|(n, _)| valid_name(n))
        {
            continue;
        }
        let size: u64 = size.parse().unwrap_or(u64::MAX);
        if size > MAX_FILE || total + size > MAX_ALL {
            tracing::warn!(project = %p.id, file = %path, "a skill file is too large; left out");
            continue;
        }
        total += size;
        files.push(SkillFile {
            path: path.to_string(),
            data: git_bytes(&p.path, &["cat-file", "blob", object])?,
            executable: mode == "100755",
        });
    }
    Ok(files)
}

/// The project's skill files on its base commit. Never read from a worktree: a task's
/// change cannot add to or rewrite the skills it runs with. A base that cannot be read
/// gives none, with a warning: a session runs without skills rather than not at all.
pub fn load(p: &ProjectCfg) -> Vec<SkillFile> {
    try_load(p).unwrap_or_else(|why| {
        tracing::warn!(project = %p.id, "the project's skills could not be read: {why}");
        vec![]
    })
}

fn record_file(task_dir: &Path, run: u64) -> std::path::PathBuf {
    task_dir.join(format!("skills-r{run}.txt"))
}

/// The skills step run `run` has called, in the order first called.
pub fn invoked(task_dir: &Path, run: u64) -> Vec<String> {
    std::fs::read_to_string(record_file(task_dir, run))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

/// Note that `run` called `name`. A stream read again after a restart changes nothing.
pub fn record(task_dir: &Path, run: u64, name: &str) {
    let mut all = invoked(task_dir, run);
    let name = mason_core::progress::one_line(name);
    if name.is_empty() || all.len() >= MAX_RECORDED || all.contains(&name) {
        return;
    }
    all.push(name);
    if let Err(e) = std::fs::write(record_file(task_dir, run), all.join("\n") + "\n") {
        tracing::warn!(run, "a skill call could not be recorded: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, path: &str, text: &str) {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn skills_are_read_from_the_base_commit_not_the_worktree() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| mason_landing::git::run(&repo, args).unwrap();
        git(&["init", "-q", "-b", "main"]);
        let text = format!("path = {:?}\nbase = \"main\"\ntest = \"t\"\n", repo);
        let p = crate::config::parse_project("p", &text).unwrap();
        put(&repo, "README", "x");
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        assert_eq!(load(&p), [], "no .mason/skills, no skills");

        put(&repo, ".mason/skills/review/SKILL.md", "Committed.");
        put(&repo, ".mason/skills/review/references/a b.md", "A list.");
        put(&repo, ".mason/skills/Bad Name/SKILL.md", "x");
        put(&repo, ".mason/skills/loose.md", "x");
        let big = "x".repeat(MAX_FILE as usize + 1);
        put(&repo, ".mason/skills/review/huge.md", &big);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "skills"]);
        // An edit and a new skill in the worktree do not count.
        put(&repo, ".mason/skills/review/SKILL.md", "Edited.");
        put(&repo, ".mason/skills/second/SKILL.md", "Never committed.");

        let files = load(&p);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["review/SKILL.md", "review/references/a b.md"]);
        assert_eq!(files[0].data, b"Committed.");

        let skills = mason_adapters::skills::Skills {
            project: files,
            user: true,
        };
        let session = t.path().join("session");
        let made = mason_adapters::skills::prepare(&skills, &session, &repo, None).unwrap();
        let dir = made.plugin_dir.unwrap().join("skills");
        assert!(dir.join("review/SKILL.md").is_file());
        assert!(!dir.join("second").exists());
    }

    #[test]
    fn a_runs_called_skills_are_kept_once_each() {
        let t = tempfile::tempdir().unwrap();
        for name in ["mason:review", "mine", "mason:review"] {
            record(t.path(), 4, name);
        }
        assert_eq!(invoked(t.path(), 4), ["mason:review", "mine"]);
        assert!(invoked(t.path(), 5).is_empty());
    }
}
