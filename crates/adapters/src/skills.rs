// SPDX-License-Identifier: GPL-3.0-or-later

//! Agent Skills for a Claude session (s1153, decision 1142).
//!
//! A project's skills come from `.mason/skills/` on the base commit. The daemon reads
//! them; this writes them into the session's own directory as a plugin named
//! [`PLUGIN`], so a session lists them as `mason:<name>` and loads a body only when it
//! applies.
//!
//! Claude Code also loads `.claude/skills/` and `.claude/commands/` of the working
//! directory, which a task's branch can write, and a directory there named
//! `mason:<name>` takes the place of the plugin's skill. So every session turns those
//! off by name (`skillOverrides`) and puts a guard in front of the `Skill` tool that
//! refuses anything that is not a project skill or, unless the project turns them off,
//! one of the owner's.

use serde_json::{json, Map, Value};
use std::path::{Component, Path, PathBuf};

/// The generated plugin's name: a project skill `x` is `mason:x`.
pub const PLUGIN: &str = "mason";

/// Skills the CLI ships that `disableBundledSkills` leaves on.
const BUILTIN: [&str; 2] = ["doctor", "plugin-authoring"];

/// One file of the project's skills.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillFile {
    /// `<skill>/...`, relative to `.mason/skills/`.
    pub path: String,
    pub data: Vec<u8>,
    pub executable: bool,
}

/// What a session may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skills {
    /// The project's skills, as read from its base commit.
    pub project: Vec<SkillFile>,
    /// Whether the owner's user-level skills load too (`[agent] user_skills`).
    pub user: bool,
}

impl Default for Skills {
    fn default() -> Self {
        Skills {
            project: vec![],
            user: true,
        }
    }
}

/// What the `Skill` guard lets through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// The project's skills, `mason:<name>`.
    pub project: Vec<String>,
    /// The owner's own skills and commands, by name.
    pub user: Vec<String>,
    /// The owner's enabled plugins: `<plugin>:<anything>` is theirs.
    pub user_plugins: Vec<String>,
    /// Names the working directory defines that were turned off at launch.
    pub off: Vec<String>,
}

impl Policy {
    pub fn to_json(&self) -> Value {
        json!({"project": self.project, "user": self.user,
               "user_plugins": self.user_plugins, "off": self.off})
    }

    pub fn from_json(v: &Value) -> Option<Policy> {
        let list = |k: &str| -> Option<Vec<String>> {
            v[k].as_array()?
                .iter()
                .map(|s| s.as_str().map(String::from))
                .collect()
        };
        Some(Policy {
            project: list("project")?,
            user: list("user")?,
            user_plugins: list("user_plugins")?,
            off: list("off")?,
        })
    }

    /// Why a `Skill` call for `skill` in `cwd` is refused, if it is.
    pub fn refusal(&self, skill: &str, cwd: &Path) -> Option<String> {
        let name = skill.trim().trim_start_matches('/');
        let has = |l: &[String]| l.iter().any(|x| x == name);
        if has(&self.user) {
            return None;
        }
        if name
            .split_once(':')
            .is_some_and(|(p, _)| self.user_plugins.iter().any(|x| x == p))
        {
            return None;
        }
        if has(&self.project) {
            // Written into the working directory after the launch: it would load
            // in place of the project's skill.
            if !has(&self.off) && defines(cwd, name) {
                return Some(format!(
                    "MASON refused the skill `{name}`: the working directory defines one of that \
                     name, which would load in place of the project's"
                ));
            }
            return None;
        }
        Some(format!(
            "MASON refused the skill `{name}`: a session runs only the project's skills \
             (`.mason/skills/` on the base commit) and the owner's"
        ))
    }
}

/// Does `dir/.claude/` define a skill or command called `name`?
fn defines(dir: &Path, name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return false;
    }
    let claude = dir.join(".claude");
    let command: PathBuf = name.split(':').collect();
    claude.join("skills").join(name).exists()
        || claude
            .join("commands")
            .join(format!("{}.md", command.display()))
            .exists()
}

fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .collect();
    v.sort();
    v
}

/// The `name:` of a skill's frontmatter: the CLI answers to it as well as to the
/// directory's name.
fn frontmatter_name(skill_md: &Path) -> Option<String> {
    let text = std::fs::read_to_string(skill_md).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    lines
        .take_while(|l| l.trim() != "---")
        .find_map(|l| l.strip_prefix("name:"))
        .map(|n| n.trim().trim_matches(['"', '\'']).to_string())
        .filter(|n| !n.is_empty())
}

/// `commands/a/b.md` is the command `a:b`.
fn commands(dir: &Path, prefix: &str, depth: u8, out: &mut Vec<String>) {
    for (name, path) in entries(dir) {
        if path.is_dir() {
            if depth < 4 {
                commands(&path, &format!("{prefix}{name}:"), depth + 1, out);
            }
        } else if let Some(stem) = name.strip_suffix(".md") {
            out.push(format!("{prefix}{stem}"));
        }
    }
}

/// The names `claude_dir` (a `.claude` directory) defines as skills and commands.
/// `aliases` adds each skill's frontmatter name.
fn names_in(claude_dir: &Path, aliases: bool) -> Vec<String> {
    let skills = claude_dir.join("skills");
    let mut dirs = entries(&skills);
    // Skills synced from the owner's claude.ai account: `synced/<bucket>/<name>`.
    for (_, bucket) in entries(&skills.join("synced")) {
        dirs.extend(entries(&bucket));
    }
    let mut out = vec![];
    for (name, path) in dirs {
        let md = path.join("SKILL.md");
        if md.is_file() {
            if let Some(n) = frontmatter_name(&md).filter(|_| aliases) {
                out.push(n);
            }
            out.push(name);
        }
    }
    commands(&claude_dir.join("commands"), "", 0, &mut out);
    out.sort();
    out.dedup();
    out
}

/// The plugins the owner's user settings turn on, as `(settings key, plugin name)`.
fn enabled_plugins(config_dir: &Path) -> Vec<(String, String)> {
    let Some(v) = std::fs::read_to_string(config_dir.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return vec![];
    };
    v["enabledPlugins"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, on)| on.as_bool() != Some(false))
        .map(|(k, _)| (k.clone(), k.split('@').next().unwrap_or(k).to_string()))
        .collect()
}

/// A skill file's path is plain names only: nothing that leaves the directory.
fn safe(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// The names of the project's skills: those with a `<name>/SKILL.md`.
fn project_names(files: &[SkillFile]) -> Vec<String> {
    files
        .iter()
        .filter(|f| safe(&f.path))
        .filter_map(|f| f.path.strip_suffix("/SKILL.md"))
        .filter(|n| !n.contains('/'))
        .map(String::from)
        .collect()
}

/// A session's skills, ready for its command line.
#[derive(Debug)]
pub struct Prepared {
    /// The plugin directory for `--plugin-dir`; none for a project with no skills.
    pub plugin_dir: Option<PathBuf>,
    /// Keys for the session's `--settings` file.
    pub settings: Map<String, Value>,
    pub policy: Policy,
}

/// Write the project's skills under `session_dir` and work out what to turn off.
/// `worktree` is the session's working directory, `config_dir` the owner's `~/.claude`.
pub fn prepare(
    skills: &Skills,
    session_dir: &Path,
    worktree: &Path,
    config_dir: Option<&Path>,
) -> std::io::Result<Prepared> {
    let names = project_names(&skills.project);
    let plugin_dir = if names.is_empty() {
        None
    } else {
        let root = session_dir.join("plugin");
        std::fs::create_dir_all(root.join(".claude-plugin"))?;
        let manifest = json!({"name": PLUGIN, "version": "1.0.0",
            "description": "The project's skills, from .mason/skills/ on its base commit"});
        std::fs::write(
            root.join(".claude-plugin/plugin.json"),
            manifest.to_string(),
        )?;
        for f in &skills.project {
            let in_a_skill = f
                .path
                .split_once('/')
                .is_some_and(|(n, _)| names.iter().any(|x| x == n));
            if !safe(&f.path) || !in_a_skill {
                continue;
            }
            let to = root.join("skills").join(&f.path);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&to, &f.data)?;
            if f.executable {
                mason_platform::fs::set_executable(&to, true)?;
            }
        }
        Some(root)
    };

    let project: Vec<String> = names.iter().map(|n| format!("{PLUGIN}:{n}")).collect();
    let plugins = config_dir.map(enabled_plugins).unwrap_or_default();
    let own = config_dir.map(|d| names_in(d, true)).unwrap_or_default();
    let mut off = names_in(&worktree.join(".claude"), false);
    let mut settings = Map::new();
    // A skill's `!`command`` lines run outside the command guard, so they do not run.
    settings.insert("disableSkillShellExecution".into(), json!(true));
    settings.insert("disableBundledSkills".into(), json!(true));
    let (user, user_plugins) = if skills.user {
        // The owner's skill wins over a working directory's of the same name.
        off.retain(|n| !own.contains(n));
        (own, plugins.into_iter().map(|(_, n)| n).collect())
    } else {
        off.extend(config_dir.map(|d| names_in(d, false)).unwrap_or_default());
        if !plugins.is_empty() {
            let m: Map<String, Value> = plugins
                .into_iter()
                .map(|(k, _)| (k, json!(false)))
                .collect();
            settings.insert("enabledPlugins".into(), Value::Object(m));
        }
        (vec![], vec![])
    };
    off.sort();
    off.dedup();
    let overrides: Map<String, Value> = off
        .iter()
        .map(String::as_str)
        .chain(BUILTIN)
        .map(|n| (n.to_string(), json!("off")))
        .collect();
    settings.insert("skillOverrides".into(), Value::Object(overrides));
    Ok(Prepared {
        plugin_dir,
        settings,
        policy: Policy {
            project,
            user,
            user_plugins,
            off,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> SkillFile {
        SkillFile {
            path: path.into(),
            data: text.as_bytes().to_vec(),
            executable: false,
        }
    }

    fn put(root: &Path, path: &str, text: &str) {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// A worktree with a skill and a command of its own, and an owner with one of each,
    /// a synced skill and a plugin.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let (wt, home) = (t.path().join("wt"), t.path().join("home"));
        put(&wt, ".claude/skills/sneaky/SKILL.md", "x");
        put(&wt, ".claude/skills/mason:review/SKILL.md", "x");
        put(&wt, ".claude/skills/mine/SKILL.md", "x");
        put(&wt, ".claude/commands/grp/deep.md", "x");
        put(
            &home,
            "skills/mine/SKILL.md",
            "---\nname: alias\n---\nbody\n",
        );
        put(&home, "skills/synced/b/cloud/SKILL.md", "x");
        put(&home, "commands/mycmd.md", "x");
        put(
            &home,
            "settings.json",
            r#"{"enabledPlugins":{"tools@market":true,"gone@market":false}}"#,
        );
        (t, wt, home)
    }

    fn off(p: &Prepared) -> Vec<&str> {
        p.settings["skillOverrides"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn project_skills_become_a_plugin_and_the_worktrees_are_turned_off() {
        let (t, wt, home) = fixture();
        let skills = Skills {
            project: vec![
                file("review/SKILL.md", "---\nname: review\n---\nbody\n"),
                file("review/references/list.md", "a list"),
                file("README.md", "not a skill"),
                file("nobody/notes.md", "no SKILL.md"),
                file("../escape/SKILL.md", "no"),
            ],
            user: true,
        };
        let session = t.path().join("s");
        let p = prepare(&skills, &session, &wt, Some(&home)).unwrap();
        let root = p.plugin_dir.clone().unwrap();
        assert!(root.starts_with(&session));
        let manifest = std::fs::read_to_string(root.join(".claude-plugin/plugin.json")).unwrap();
        assert!(manifest.contains("\"name\":\"mason\""), "{manifest}");
        assert!(root.join("skills/review/SKILL.md").is_file());
        assert!(root.join("skills/review/references/list.md").is_file());
        for absent in ["skills/README.md", "skills/nobody", "escape"] {
            assert!(!root.join(absent).exists(), "{absent}");
        }
        assert!(!session.join("escape").exists());
        assert_eq!(p.policy.project, ["mason:review"]);
        // `mine` is the owner's too, whose copy wins.
        assert_eq!(p.policy.off, ["grp:deep", "mason:review", "sneaky"]);
        assert!(off(&p).contains(&"doctor") && off(&p).contains(&"sneaky"));
        assert_eq!(p.policy.user, ["alias", "cloud", "mine", "mycmd"]);
        assert_eq!(p.policy.user_plugins, ["tools"]);
        assert_eq!(p.settings["disableSkillShellExecution"], true);
        assert_eq!(p.settings["disableBundledSkills"], true);
        assert!(!p.settings.contains_key("enabledPlugins"));
    }

    #[test]
    fn user_skills_off_turns_off_every_skill_and_plugin_of_the_owners() {
        let (t, wt, home) = fixture();
        let skills = Skills {
            project: vec![],
            user: false,
        };
        let p = prepare(&skills, &t.path().join("s"), &wt, Some(&home)).unwrap();
        assert_eq!(p.plugin_dir, None);
        assert!(!t.path().join("s/plugin").exists());
        for n in ["mine", "cloud", "mycmd", "sneaky", "grp:deep"] {
            assert!(off(&p).contains(&n), "{n}: {:?}", off(&p));
        }
        assert_eq!(p.settings["enabledPlugins"], json!({"tools@market": false}));
        assert!(p.policy.user.is_empty() && p.policy.user_plugins.is_empty());
        assert!(p.policy.refusal("mine", &wt).is_some());
        assert!(p.policy.refusal("tools:x", &wt).is_some());
    }

    #[test]
    fn the_guard_lets_through_only_the_projects_and_the_owners_skills() {
        let (t, wt, home) = fixture();
        let skills = Skills {
            project: vec![file("review/SKILL.md", "x"), file("late/SKILL.md", "x")],
            user: true,
        };
        let p = prepare(&skills, &t.path().join("s"), &wt, Some(&home)).unwrap();
        let policy = Policy::from_json(&p.policy.to_json()).unwrap();
        assert_eq!(policy, p.policy);
        for ok in [
            "mason:review",
            "/mason:late",
            "mine",
            "alias",
            "mycmd",
            "tools:any",
        ] {
            assert_eq!(policy.refusal(ok, &wt), None, "{ok}");
        }
        for no in [
            "sneaky",
            "grp:deep",
            "gone:x",
            "mason:other",
            "doctor",
            "new",
            "",
        ] {
            assert!(policy.refusal(no, &wt).is_some(), "{no}");
        }
        // Written after the launch, so it is not among the ones turned off.
        put(&wt, ".claude/skills/mason:late/SKILL.md", "x");
        assert!(policy.refusal("mason:late", &wt).is_some());
        assert_eq!(policy.refusal("mason:review", &wt), None);
    }
}
