// SPDX-License-Identifier: GPL-3.0-or-later

//! The logic of `mason init` and `mason adopt`, without a terminal: detect a repository, resolve
//! the questions (each also a flag), render and write `.mason/` on a setup branch, write the
//! project overlay, and land the branch when the owner said to. The CLI is a terminal front end;
//! the daemon serves the same steps over HTTP through [`plan`] and [`apply`].
//!
//! Registering with a running masond and filing the proof task are the caller's: the CLI calls
//! HTTP, the daemon calls itself. Nothing here depends on the daemon, so the overlay check is a
//! [`Validator`] the caller passes in (the daemon's own parser), with a TOML-level default.

pub mod adopt;
pub mod detect;
pub mod land;
pub mod questions;
pub mod suites;

use mason_landing::git;
use questions::{
    Agent, Alert, Answers, Detected, Flags, Landing, Mode, Question, SuiteSpec, SETUP_BRANCH,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The text of the harmless first task, filed by `mason init` and by the daemon's Add Project.
pub const PROOF_TITLE: &str = "Add a file that proves MASON works";
pub const PROOF_BODY: &str = "Create a file named MASON-WORKS.md at the top of the repository containing one line, `MASON landed this task.`, and nothing else. Change no other file.\n\n## Acceptance criteria\n- MASON-WORKS.md exists with exactly that one line.\n\n## Approach\nCreate MASON-WORKS.md (new) with the line above and commit it. This task exists only to show the pipeline works end to end; delete the file afterwards if you like.\n";

/// Checks a rendered project overlay `(id, text)` before anything is written.
pub type Validator<'a> = &'a dyn Fn(&str, &str) -> Result<(), String>;

/// What can be checked without the daemon's parser: valid TOML, a valid id, the required keys.
pub fn basic_validate(id: &str, text: &str) -> Result<(), String> {
    if !questions::valid_project_id(id) {
        return Err(format!(
            "project id {id:?} must be lowercase letters, digits, - and _"
        ));
    }
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("{id}: not valid TOML: {e}"))?;
    for k in ["path", "base"] {
        if doc
            .get(k)
            .and_then(|v| v.as_str())
            .is_none_or(|v| v.trim().is_empty())
        {
            return Err(format!("{id}: `{k}` is required"));
        }
    }
    Ok(())
}

pub const FILING_FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

/// Where the Filing section goes: AGENTS.md if there is one, else CLAUDE.md, else a new AGENTS.md.
pub fn filing_file(exists: impl Fn(&str) -> bool) -> &'static str {
    FILING_FILES
        .into_iter()
        .find(|f| exists(f))
        .unwrap_or(FILING_FILES[0])
}

pub fn overlay_path(config_dir: &Path, id: &str) -> PathBuf {
    config_dir.join("projects").join(format!("{id}.toml"))
}

fn git_cmd(dir: &Path, args: &[&str]) -> Result<String, String> {
    git::run(dir, args).map_err(|e| format!("git {}: {e}", args.join(" ")))
}

/// Commit identity: the user's own if they have one, else a stand-in, so a fresh machine can
/// still make the setup commit.
fn identity(dir: &Path) -> Vec<String> {
    if git::run(dir, &["config", "user.email"]).is_ok_and(|e| !e.trim().is_empty()) {
        vec![]
    } else {
        ["-c", "user.name=MASON", "-c", "user.email=mason@localhost"]
            .map(String::from)
            .to_vec()
    }
}

/// Where each choice lives, so it can be changed later.
pub fn summary(a: &Answers, config_dir: &Path) -> String {
    format!(
        "\nWhere to change what you chose:\n  definition (the steps)  .mason/mason.toml, or `mason definition` and the monitor's editor\n  landing ({})  .mason/landing.toml\n  alerts ({})  {}\n  providers and sessions ({})  {} and {}\n  this project  {} (`mason project reload` applies an edit)\n",
        a.landing.name(),
        a.alerts.as_ref().map_or("built-in routing".to_string(), |c| {
            c.iter().map(|x| x.name()).collect::<Vec<_>>().join(", ")
        }),
        config_dir.join("notify.toml").display(),
        a.agent.name(),
        config_dir.join("providers.toml").display(),
        config_dir.join("resources.toml").display(),
        overlay_path(config_dir, &a.id).display(),
    )
}

/// Questions 5 and part of 8: everything that is written. Returns the closing instructions.
pub fn setup(
    flags: &Flags,
    d: &Detected,
    a: &Answers,
    config_dir: &Path,
    say: &mut dyn FnMut(&str),
    validate: Validator,
) -> Result<String, String> {
    // Refusals first, before anything is written.
    if a.agent_bin.is_none() {
        let n = a.agent.name();
        return Err(format!("no `{n}` executable was found on PATH, in another project's setting or in ~/.local/bin, so the project could not start an agent; nothing was written. Pass --agent-bin PATH (the absolute path of `{n}`)"));
    }
    if git::rev_parse(&d.root, "HEAD").is_err() {
        return Err(
            "the repository has no commits yet; make one first (the setup is a branch from it)"
                .into(),
        );
    }
    let on_head = git::run(&d.root, &["ls-tree", "--name-only", "HEAD", ".mason"])
        .is_ok_and(|s| !s.trim().is_empty());
    if (on_head || d.existing_mason) && !flags.force {
        return Err("this repository already has a .mason/ folder. `mason adopt` registers it as it is; `mason init --force` replaces it".into());
    }
    let overlay = overlay_path(config_dir, &a.id);
    if overlay.exists() && !flags.force {
        return Err(format!("{} already exists (project {:?} is registered); `mason init --force` replaces it, or pass --id for another name", overlay.display(), a.id));
    }
    if git::ok(
        &d.root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{SETUP_BRANCH}"),
        ],
    ) && !flags.force
    {
        return Err(format!("the branch {SETUP_BRANCH} already exists from an earlier init; merge or delete it, or use --force"));
    }
    if !git::ok(
        &d.root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{}", a.base),
        ],
    ) && d.remote_name.as_ref().is_none_or(|r| {
        !git::ok(
            &d.root,
            &[
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/remotes/{r}/{}", a.base),
            ],
        )
    }) {
        return Err(format!(
            "the base branch {:?} does not exist here; pass --base with an existing branch",
            a.base
        ));
    }

    // 5. Tests: propose, run each once, keep what parses.
    let mut kept: Vec<SuiteSpec> = vec![];
    let mut baselines = vec![];
    if a.accept_suites {
        let proposals = suites::propose(&d.root, &d.languages);
        if proposals.is_empty() {
            say(&format!(
                "\nTests: no suite could be proposed for {}.",
                if d.languages.is_empty() {
                    "this repository (no build files recognised)".to_string()
                } else {
                    d.languages.join(", ")
                }
            ));
        }
        for s in proposals {
            say(&format!(
                "\nTests: running `{}` once to check it parses...",
                s.command
            ));
            let b = suites::run_once(&s, &d.root);
            if b.executed > 0 {
                say(&format!(
                    "  {}: {} test(s) found, {} passed, {} failed, {:.1}s{}",
                    s.name,
                    b.executed,
                    b.passed,
                    b.failed,
                    b.seconds,
                    if b.green { "" } else { " (NOT green)" }
                ));
                kept.push(s);
            } else {
                say(&format!("  {}: dropped.", s.name));
            }
            if let Some(p) = &b.problem {
                say(&format!("  warning: {}: {p}", b.name));
            }
            baselines.push(b);
        }
    }
    let test_cmd = if kept.is_empty() {
        say("\nwarning: no test suite is set up, so the test step runs `true` and proves nothing. Add one in .mason/mason.toml (see `mason test parse`) before trusting a landing.");
        "true".to_string()
    } else {
        kept.iter()
            .map(|s| s.command.as_str())
            .collect::<Vec<_>>()
            .join(" && ")
    };

    let overlay_text = questions::render_overlay(a, d, &test_cmd);
    validate(&a.id, &overlay_text)
        .map_err(|e| format!("the overlay would not load (a bug in init): {e}"))?;

    // The branch, in a temporary worktree, so the working tree is never touched.
    let tmp = std::env::temp_dir().join(format!("mason-setup-{}-{}", std::process::id(), a.id));
    let _ = std::fs::remove_dir_all(&tmp);
    let tmp_s = tmp.display().to_string();
    git_cmd(&d.root, &["worktree", "prune"])?;
    let branch_flag = if flags.force { "-B" } else { "-b" };
    git_cmd(
        &d.root,
        &[
            "worktree",
            "add",
            "-q",
            branch_flag,
            SETUP_BRANCH,
            &tmp_s,
            "HEAD",
        ],
    )?;
    let built = (|| -> Result<Vec<String>, String> {
        let mut said = vec![];
        let write = |rel: &str, text: &str| -> Result<(), String> {
            let p = tmp.join(rel);
            std::fs::create_dir_all(p.parent().unwrap_or(&tmp))
                .map_err(|e| format!("{rel}: {e}"))?;
            std::fs::write(&p, text).map_err(|e| format!("{rel}: {e}"))
        };
        if flags.force {
            let _ = std::fs::remove_dir_all(tmp.join(".mason"));
        }
        let mason_toml = questions::render_mason_toml(a.workflow, &kept);
        if let Err(p) = mason_definition::parse(&mason_toml) {
            return Err(format!(
                "the {} template with these suites does not parse (a bug in init): {p:?}",
                a.workflow.name()
            ));
        }
        write(".mason/mason.toml", &mason_toml)?;
        write(
            ".mason/landing.toml",
            &questions::render_landing_toml(a.workflow, a.landing, &a.base),
        )?;
        if !baselines.is_empty() {
            write(
                ".mason/baselines.toml",
                &suites::render_baselines(&baselines),
            )?;
        }
        // The Filing section.
        let target = filing_file(|f| tmp.join(f).is_file());
        let existing = std::fs::read_to_string(tmp.join(target)).unwrap_or_default();
        match questions::with_filing(&existing) {
            Some(text) => {
                write(target, &text)?;
                said.push(format!("added the \"Filing work\" notes to {target} (they explain the `explored` flag)"));
            }
            None => said.push(format!(
                "{target} already has the \"Filing work\" notes; left alone"
            )),
        }
        // CI trigger check, for the one mode that needs it.
        if a.landing == Landing::DirectCi {
            let problems = mason_adapters::github::workflow_problems_at(&d.root, "HEAD");
            if problems.is_empty() {
                said.push("CI: a workflow already runs on `mason/ci/**` or by dispatch".into());
            } else if a.ci_workflow {
                write(
                    ".github/workflows/mason-ci.yml",
                    &questions::render_ci_workflow(&test_cmd),
                )?;
                said.push("CI: added .github/workflows/mason-ci.yml, which runs the tests on `mason/ci/**` (on the setup branch only; nothing is pushed)".into());
            } else {
                said.push(format!("CI: direct-ci will not work yet: {}. Re-run with --ci-workflow yes to add one, or add the trigger yourself", problems.iter().map(|p| p.0.clone()).collect::<Vec<_>>().join("; ")));
            }
        }
        git_cmd(&tmp, &["add", "-A"])?;
        let mut commit = identity(&tmp);
        commit.extend(
            [
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "Set up MASON (mason init)",
                "-m",
                "Written by `mason init`. Review it, merge it, and tasks will use this workflow.",
            ]
            .map(String::from),
        );
        let refs: Vec<&str> = commit.iter().map(String::as_str).collect();
        git_cmd(&tmp, &refs)?;
        Ok(said)
    })();
    let _ = git::run(&d.root, &["worktree", "remove", "--force", &tmp_s]);
    let _ = std::fs::remove_dir_all(&tmp);
    let said = match built {
        Ok(s) => s,
        Err(e) => {
            let _ = git::run(&d.root, &["branch", "-D", SETUP_BRANCH]);
            return Err(e);
        }
    };

    // The overlay and the machine-wide files.
    std::fs::create_dir_all(overlay.parent().unwrap_or(config_dir))
        .map_err(|e| format!("{}: {e}", config_dir.display()))?;
    std::fs::write(&overlay, &overlay_text).map_err(|e| format!("{}: {e}", overlay.display()))?;
    let mut said = said;
    if let Some(channels) = &a.alerts {
        let p = config_dir.join("notify.toml");
        if p.exists() {
            said.push(format!(
                "{} exists; alert routing left as it is",
                p.display()
            ));
        } else {
            std::fs::write(&p, questions::render_notify(channels))
                .map_err(|e| format!("{}: {e}", p.display()))?;
            said.push(format!(
                "wrote {} (alerts: monitor{})",
                p.display(),
                channels
                    .iter()
                    .filter(|c| matches!(c, Alert::Sound | Alert::Speech))
                    .map(|c| format!(", {}", c.name()))
                    .collect::<String>()
            ));
        }
        for c in channels
            .iter()
            .filter(|c| matches!(c, Alert::Email | Alert::Push))
        {
            said.push(format!("{} needs an account and secrets: add an [{}] section to notify.toml (`mason notify` shows what is set up)", c.name(), if *c == Alert::Email { "email" } else { "ntfy" }));
        }
    }
    let res = config_dir.join("resources.toml");
    if a.agent == Agent::Claude {
        if res.exists() {
            said.push(format!("{} exists; sessions left as it is", res.display()));
        } else {
            std::fs::write(&res, format!("[llm]\nclaude = {}\n", a.sessions))
                .map_err(|e| format!("{}: {e}", res.display()))?;
            said.push(format!(
                "wrote {} ({} session(s) at once)",
                res.display(),
                a.sessions
            ));
        }
    } else {
        said.push("sessions for codex are set per account in providers.toml".into());
    }
    if a.agent_bin.is_none() {
        said.push(format!(
            "{} is logged in? MASON cannot check; if not, run `{}` once and log in",
            a.agent.name(),
            a.agent.name()
        ));
    }

    let mut out = String::from("\nWritten:\n");
    out.push_str(&format!("  branch    {SETUP_BRANCH} (one commit: .mason/ and the notes; your working tree is untouched)\n  project   {} ({})\n", a.id, overlay.display()));
    for s in said {
        out.push_str(&format!("  {s}\n"));
    }
    out.push_str(&format!(
        "\nNext: review and merge the branch, so tasks use it:\n{}\nthen start `masond` if it is not running, and run `mason init --prove-only` to land a harmless first task.",
        land::commands(d, a)
    ));
    Ok(out)
}

/// Whether the repository is set up fresh (`mason init`) or already has a `.mason/` (`mason adopt`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Init,
    Adopt,
}

/// Where a repository stands, for a front end that asks one question at a time.
#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub detected: Detected,
    pub kind: Kind,
    /// The first unanswered question, with its recommended default; `None` when all are answered.
    pub next: Option<Question>,
    /// What would be written, as `(path, contents)`: repository paths, plus the project overlay
    /// as `projects/<id>.toml` (relative to the config directory). Test suites are the proposed
    /// ones, not yet run. Empty while `errors` is not.
    pub files: Vec<(String, String)>,
    pub errors: Vec<String>,
    /// Things that would leave the setup unable to run as written (no agent binary, a token
    /// variable the daemon does not have).
    pub warnings: Vec<String>,
}

/// Resolve every answered question, name the first unanswered one and render the files. Writes
/// nothing and runs nothing (no suite is executed).
pub fn plan(path: &Path, answers: &Flags) -> Plan {
    let d = match detect::detect(path) {
        Ok(d) => d,
        Err(e) => {
            return Plan {
                detected: Detected::default(),
                kind: Kind::Init,
                next: None,
                files: vec![],
                errors: vec![e],
                warnings: vec![],
            }
        }
    };
    let on_head = git::ok(&d.root, &["cat-file", "-e", "HEAD:.mason/mason.toml"]);
    let kind = if d.existing_mason || on_head {
        Kind::Adopt
    } else {
        Kind::Init
    };
    let mut p = Plan {
        detected: d.clone(),
        kind,
        next: None,
        files: vec![],
        errors: vec![],
        warnings: vec![],
    };
    let mut flags = answers.clone();
    flags.yes = false;
    let mut first: Option<Question> = None;
    let resolved = {
        let mut ask = |q: &Question| {
            if first.is_none() {
                first = Some(q.clone());
            }
            None
        };
        questions::resolve(
            &flags,
            &d,
            &mut Mode {
                interactive: true,
                ask: &mut ask,
            },
            &mut |_| {},
        )
    };
    p.next = first;
    let a = match resolved {
        Ok(a) => a,
        Err(e) => {
            p.errors.push(e);
            return p;
        }
    };
    p.warnings.extend(a.warnings.iter().cloned());
    match kind {
        Kind::Adopt => {
            if let Err(e) = adopt::check(&d, &adopt::Args::from_flags(answers)) {
                p.errors.extend(e.lines);
                p.errors.push(e.message);
            }
        }
        Kind::Init => {
            let suites = if a.accept_suites {
                suites::propose(&d.root, &d.languages)
            } else {
                vec![]
            };
            let test_cmd = if suites.is_empty() {
                "true".to_string()
            } else {
                suites
                    .iter()
                    .map(|s| s.command.as_str())
                    .collect::<Vec<_>>()
                    .join(" && ")
            };
            let mut push = |path: &str, text: String| p.files.push((path.to_string(), text));
            push(
                ".mason/mason.toml",
                questions::render_mason_toml(a.workflow, &suites),
            );
            push(
                ".mason/landing.toml",
                questions::render_landing_toml(a.workflow, a.landing, &a.base),
            );
            let target = filing_file(|f| d.root.join(f).is_file());
            let existing = std::fs::read_to_string(d.root.join(target)).unwrap_or_default();
            if let Some(text) = questions::with_filing(&existing) {
                push(target, text);
            }
            push(
                &format!("projects/{}.toml", a.id),
                questions::render_overlay(&a, &d, &test_cmd),
            );
        }
    }
    p
}

/// What [`apply`] did.
#[derive(Clone, Debug, Serialize)]
pub struct Applied {
    pub id: String,
    pub base: String,
    pub kind: Kind,
    pub landed: land::Landed,
    /// The project overlay that was written.
    pub overlay: PathBuf,
    /// The closing report of `setup` (empty for an adopt).
    pub report: String,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
    pub detected: Detected,
}

/// Do what the answers say: set up (or adopt) and land, exactly as answered. A question with no
/// answer is an error naming its flag, unless `answers.yes` takes the recommended one. Progress
/// lines go to `progress`. Registration and the proof task are the caller's.
pub fn apply(
    path: &Path,
    answers: &Flags,
    config_dir: &Path,
    progress: &mut dyn FnMut(&str),
) -> Result<Applied, String> {
    apply_validated(path, answers, config_dir, progress, &basic_validate)
}

/// [`apply`] with the caller's overlay check (the daemon's own parser).
pub fn apply_validated(
    path: &Path,
    answers: &Flags,
    config_dir: &Path,
    progress: &mut dyn FnMut(&str),
    validate: Validator,
) -> Result<Applied, String> {
    let d = detect::detect(path)?;
    let on_head = git::ok(&d.root, &["cat-file", "-e", "HEAD:.mason/mason.toml"]);
    if (d.existing_mason || on_head) && !answers.force {
        let mut args = adopt::Args::from_flags(answers);
        if args.agent_bin.is_none() {
            let found = answers.machine.find_agent(questions::Agent::Claude).ok_or(
                "no `claude` executable was found on PATH, in another project's setting or in ~/.local/bin, so the project could not start an agent; nothing was written. Pass --agent-bin PATH (the absolute path of `claude`)",
            )?;
            args.agent_bin = Some(found.display().to_string());
        }
        let id = args.id.clone().unwrap_or_else(|| d.id.clone());
        let ad = adopt::adopt(&d, &args, config_dir, validate).map_err(|e| e.message)?;
        return Ok(Applied {
            id,
            base: ad.base,
            kind: Kind::Adopt,
            landed: land::Landed::default(),
            overlay: ad.overlay,
            report: String::new(),
            notes: vec![],
            warnings: vec![],
            detected: d,
        });
    }
    let a = questions::resolve(
        answers,
        &d,
        &mut Mode {
            interactive: false,
            ask: &mut |_| None,
        },
        &mut |_| {},
    )?;
    let report = setup(answers, &d, &a, config_dir, progress, validate)?;
    let landed = land::apply(&d, &a, progress);
    Ok(Applied {
        overlay: overlay_path(config_dir, &a.id),
        id: a.id,
        base: a.base,
        kind: Kind::Init,
        landed,
        report,
        notes: a.notes,
        warnings: a.warnings,
        detected: d,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let o = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}");
    }

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
        for (p, t) in files {
            let f = dir.path().join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-qm", "init"]);
        dir
    }

    /// A HOME whose `.local/bin` holds a fake `claude` (when asked), and a PATH with nothing.
    fn machine(with_claude: bool) -> (tempfile::TempDir, questions::Machine) {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        if with_claude {
            let bin = home.path().join(".local/bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("claude"), "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let m = questions::Machine {
            path: Some(home.path().join("empty-path").into()),
            home: Some(home.path().to_path_buf()),
            known_bins: vec![],
            env: Some(vec![]),
        };
        (home, m)
    }

    #[test]
    fn no_findable_agent_is_asked_for_and_apply_writes_nothing() {
        let d = repo(&[]);
        let cfg = tempfile::tempdir().unwrap();
        let (_h, m) = machine(false);
        let mut f = all_answered();
        f.machine = m;
        let p = plan(d.path(), &f);
        assert_eq!(p.next.as_ref().map(|q| q.id), Some("agent-bin"), "{p:?}");
        assert!(p.warnings.iter().any(|w| w.contains("claude")), "{p:?}");
        let e = apply(d.path(), &f, cfg.path(), &mut |_| {}).unwrap_err();
        assert!(e.contains("--agent-bin"), "{e}");
        assert!(!cfg.path().join("projects").exists());
        assert!(!git::ok(
            d.path(),
            &["rev-parse", "--verify", "-q", "refs/heads/mason/setup"]
        ));
    }

    #[test]
    fn an_agent_only_in_local_bin_is_written_as_an_absolute_path() {
        let d = repo(&[]);
        let cfg = tempfile::tempdir().unwrap();
        let (h, m) = machine(true);
        let mut f = all_answered();
        f.machine = m;
        let a = apply(d.path(), &f, cfg.path(), &mut |_| {}).unwrap();
        let overlay = std::fs::read_to_string(&a.overlay).unwrap();
        let want = h.path().join(".local/bin/claude");
        assert!(
            overlay.contains(&format!("claude_bin = {:?}", want.display().to_string())),
            "{overlay}"
        );
    }

    #[test]
    fn an_agent_on_path_is_written_as_an_absolute_path() {
        use std::os::unix::fs::PermissionsExt;
        let d = repo(&[]);
        let (h, mut m) = machine(false);
        let bin = h.path().join("pathbin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("claude"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        m.path = Some(bin.clone().into());
        let mut f = all_answered();
        f.machine = m;
        let p = plan(d.path(), &f);
        let overlay = &p
            .files
            .iter()
            .find(|(n, _)| n.starts_with("projects/"))
            .unwrap()
            .1;
        assert!(
            overlay.contains(&format!(
                "claude_bin = {:?}",
                bin.join("claude").display().to_string()
            )),
            "{overlay}"
        );
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        // A file that is not executable is not an agent.
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(f.machine.find_agent(questions::Agent::Claude).is_none());
    }

    #[test]
    fn an_unset_token_variable_is_a_warning_and_none_leaves_the_sections_out() {
        let d = repo(&[]);
        git(
            d.path(),
            &["remote", "add", "origin", "git@github.com:o/r.git"],
        );
        let (_h, m) = machine(true);
        let mut f = all_answered();
        f.machine = m;
        f.tracker = Some("github".into());
        let p = plan(d.path(), &f);
        assert!(
            p.warnings.iter().any(|w| w.contains("GITHUB_TOKEN")),
            "{p:?}"
        );
        f.token = Some("none".into());
        let p = plan(d.path(), &f);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let overlay = &p
            .files
            .iter()
            .find(|(n, _)| n.starts_with("projects/"))
            .unwrap()
            .1;
        assert!(!overlay.contains("[tracker]"), "{overlay}");
        f.token = Some("env:GITHUB_TOKEN".into());
        f.machine.env = Some(vec![("GITHUB_TOKEN".into(), "x".into())]);
        let p = plan(d.path(), &f);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    }

    fn all_answered() -> Flags {
        let s = |v: &str| Some(v.to_string());
        Flags {
            tracker: s("local"),
            agent: s("claude"),
            sessions: s("1"),
            workflow: s("solo-fast"),
            suites: s("skip"),
            base: s("main"),
            landing: s("direct"),
            alerts: s(""),
            land: s("no"),
            ..Default::default()
        }
    }

    #[test]
    fn an_empty_answer_set_returns_the_first_question_with_its_default() {
        let d = repo(&[]);
        let p = plan(d.path(), &Flags::default());
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.kind, Kind::Init);
        let q = p.next.expect("a question");
        assert_eq!(
            (q.id, q.default.as_str(), q.flag),
            ("tracker", "local", "--tracker")
        );
        assert!(q.options.iter().any(|o| o == "github"));
    }

    #[test]
    fn answering_everything_leaves_no_question_and_lists_the_files() {
        let d = repo(&[]);
        let p = plan(d.path(), &all_answered());
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert!(p.next.is_none());
        let names: Vec<&str> = p.files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&".mason/mason.toml"), "{names:?}");
        assert!(names.contains(&".mason/landing.toml"), "{names:?}");
        assert!(
            names.iter().any(|n| n.starts_with("projects/")),
            "{names:?}"
        );
        // Nothing was written.
        assert!(!d.path().join(".mason").exists());
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"kind\":\"init\""));
    }

    #[test]
    fn a_repository_with_a_mason_folder_is_an_adopt() {
        let toml = questions::render_mason_toml(questions::Workflow::SoloFast, &[]);
        let d = repo(&[(".mason/mason.toml", &toml)]);
        assert_eq!(plan(d.path(), &Flags::default()).kind, Kind::Adopt);
    }

    #[test]
    fn a_bad_landing_value_says_what_the_cli_says() {
        let d = repo(&[]);
        let mut f = all_answered();
        f.landing = Some("bogus".into());
        let p = plan(d.path(), &f);
        assert_eq!(
            p.errors,
            ["--landing is direct, direct-ci, pr or batched, not \"bogus\""]
        );
        assert!(p.files.is_empty());
    }

    #[test]
    fn apply_writes_the_setup_branch_and_the_overlay_and_nothing_in_the_working_tree() {
        let d = repo(&[]);
        let cfg = tempfile::tempdir().unwrap();
        let mut said = vec![];
        let (_h, m) = machine(true);
        let mut f = all_answered();
        f.machine = m;
        let a = apply(d.path(), &f, cfg.path(), &mut |t| said.push(t.to_string())).unwrap();
        assert_eq!(a.kind, Kind::Init);
        assert_eq!(a.base, "main");
        assert!(a.overlay.is_file());
        assert!(!d.path().join(".mason").exists());
        assert!(git::ok(
            d.path(),
            &["rev-parse", "--verify", "-q", "refs/heads/mason/setup"]
        ));
        assert!(!a.landed.merged);
    }
}
