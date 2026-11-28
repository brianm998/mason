// SPDX-License-Identifier: GPL-3.0-or-later

//! The end of `mason init`: land the setup branch when the owner said to. Nothing here runs
//! unless `--land` (or the terminal question) asked for it; `--yes` and a missing terminal both
//! mean `no`. Every refusal prints the exact commands and changes nothing.

use crate::questions::{Answers, Detected, Land, Landing, SETUP_BRANCH};
use mason_adapter_api::http::Curl;
use mason_adapter_api::service::GitHostAdapter;
use mason_adapters::github::{GitHub, TokenRef};
use mason_landing::git;
use std::sync::Arc;

/// What happened to the base branch, for what comes after.
#[derive(Debug, Default, PartialEq, Eq, Clone, serde::Serialize)]
pub struct Landed {
    /// The base branch here now has `.mason/`.
    pub merged: bool,
    /// The base is on the remote too (or there is no remote to push to).
    pub published: bool,
    /// The owner asked for something and it was refused or failed.
    pub refused: bool,
}

pub const PR_TITLE: &str = "Set up MASON (mason init)";
pub const PR_BODY: &str = "Written by `mason init`: `.mason/` and the \"Filing work\" notes. Merge it and tasks will use this workflow.";

fn api_base() -> String {
    std::env::var("MASON_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into())
}

/// The commands that do by hand what `--land` would have done.
pub fn commands(d: &Detected, a: &Answers) -> String {
    let remote = d.remote_name.as_deref().unwrap_or("origin");
    let mut s = format!(
        "  git diff {}...{SETUP_BRANCH}\n  git checkout {} && git merge --ff-only {SETUP_BRANCH}",
        a.base, a.base
    );
    if d.remote_name.is_some() {
        if a.landing == Landing::Pr {
            s = format!(
                "  git diff {}...{SETUP_BRANCH}\n  git push {remote} {SETUP_BRANCH}\n  {}",
                a.base,
                pr_command(a)
            );
        } else {
            s.push_str(&format!("\n  git push {remote} {}", a.base));
        }
    }
    s
}

/// Does the remote's copy of the base branch have `.mason/`? MASON branches every task from
/// `<remote>/<base>`, so only this counts. `true` when there is no remote (nothing to compare).
pub fn remote_base_has_setup(d: &Detected, base: &str) -> bool {
    match d.remote_name.as_deref() {
        None => true,
        Some(r) => git::ok(
            &d.root,
            &["cat-file", "-e", &format!("{r}/{base}:.mason/mason.toml")],
        ),
    }
}

/// Said before Add when there is a remote and the chosen landing leaves `.mason/` off the remote
/// base: tasks branch from there, so they would run the built-in pipeline. Names the commands.
/// `push` never warns here (it pushes; [`apply`] says so if that is refused).
pub fn unpublished_warning(
    d: &Detected,
    base: &str,
    landing: Landing,
    land: Land,
) -> Option<String> {
    let r = d.remote_name.as_deref()?;
    if land == Land::Push || remote_base_has_setup(d, base) {
        return None;
    }
    let how = match (landing, land) {
        (Landing::Pr, Land::Pr) => format!(
            "the pull request has to be merged first (`git push {r} {SETUP_BRANCH}` and `gh pr create --base {base} --head {SETUP_BRANCH}`)"
        ),
        (_, Land::Merge) => format!("it is merged into {base} here only; run `git push {r} {base}`"),
        (Landing::Pr, _) => format!(
            "merge {SETUP_BRANCH} through a pull request: `git push {r} {SETUP_BRANCH}`, open the pull request, merge it"
        ),
        _ => format!(
            "merge {SETUP_BRANCH} into {base} (`git checkout {base} && git merge --ff-only {SETUP_BRANCH}`) and run `git push {r} {base}`"
        ),
    };
    Some(format!(
        "{r}/{base} has no .mason/ and MASON branches every task from it, so tasks would run the built-in pipeline, not your workflow: {how}. The proof task is not filed until {r}/{base} has it"
    ))
}

fn pr_command(a: &Answers) -> String {
    format!(
        "gh pr create --base {} --head {SETUP_BRANCH} --title {:?} --body {:?}",
        a.base, PR_TITLE, PR_BODY
    )
}

fn merge(d: &Detected, a: &Answers) -> Result<(), String> {
    let on = git::run(&d.root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .map_err(|e| e.to_string())?;
    if on != a.base {
        return Err(format!(
            "{} is checked out here, not the base {}, so the merge was not done",
            on, a.base
        ));
    }
    let dirty = git::run(&d.root, &["status", "--porcelain", "--untracked-files=no"])
        .map_err(|e| e.to_string())?;
    if !dirty.trim().is_empty() {
        return Err(format!(
            "the working tree has uncommitted changes, so the merge was not done:\n{}",
            dirty.trim_end()
        ));
    }
    git::run(&d.root, &["merge", "--ff-only", "-q", SETUP_BRANCH]).map_err(|e| {
        format!(
            "{SETUP_BRANCH} does not fast-forward {} ({e}); nothing was merged",
            a.base
        )
    })?;
    Ok(())
}

fn push(d: &Detected, what: &str) -> Result<(), String> {
    let remote = d.remote_name.as_deref().ok_or("there is no remote")?;
    git::run(&d.root, &["push", "-q", remote, what])
        .map(|_| ())
        .map_err(|e| format!("git push {remote} {what} failed: {e}"))
}

fn open_pr(d: &Detected, a: &Answers, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    let Some(r) = d.github() else {
        return Err("the remote is not on GitHub".into());
    };
    let token = TokenRef::Env("GITHUB_TOKEN".into());
    if token.read().is_err() {
        return Err("GITHUB_TOKEN is not set, so no pull request was opened".into());
    }
    let gh = GitHub::new(Arc::new(Curl::default()), &api_base(), &r.repo, token);
    let pr = gh
        .open_pr(&a.base, SETUP_BRANCH, PR_TITLE, PR_BODY)
        .map_err(|e| format!("opening the pull request failed: {e}"))?;
    say(&format!("  Opened pull request #{}: {}", pr.number, pr.url));
    Ok(())
}

/// Do what the owner asked, saying each step. On a refusal, print what to run instead.
pub fn apply(d: &Detected, a: &Answers, say: &mut dyn FnMut(&str)) -> Landed {
    let mut out = Landed::default();
    if a.land == Land::No {
        return out;
    }
    say(&format!("\nLanding ({}):", a.land.name()));
    let pr_mode = a.landing == Landing::Pr;
    let result = (|| -> Result<(), String> {
        // In pr mode the branch itself goes up, and the base moves only when the PR merges.
        let merge_here = a.land == Land::Merge || (a.land == Land::Push && !pr_mode);
        if merge_here {
            merge(d, a)?;
            out.merged = true;
            say(&format!(
                "  Merged {SETUP_BRANCH} into {} (fast-forward).",
                a.base
            ));
        }
        if d.remote_name.is_none() {
            out.published = out.merged;
            if out.merged {
                say("  No remote: masond cannot land onto a base branch that is checked out here, so switch to another branch (`git checkout -b my-work`) before the first task lands.");
            }
            return Ok(());
        }
        match a.land {
            Land::Push if !pr_mode => {
                push(d, &a.base)?;
                out.published = true;
                say(&format!("  Pushed {}.", a.base));
            }
            Land::Push | Land::Pr => {
                push(d, SETUP_BRANCH)?;
                say(&format!("  Pushed {SETUP_BRANCH}."));
                if a.land == Land::Pr {
                    if let Err(e) = open_pr(d, a, say) {
                        say(&format!("  {e}. Open it yourself:\n  {}", pr_command(a)));
                    }
                } else {
                    say(&format!(
                        "  Open the pull request when you are ready:\n  {}",
                        pr_command(a)
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    })();
    if let Err(e) = result {
        out.refused = true;
        say(&format!(
            "  Not done: {e}\n  To do it yourself:\n{}",
            commands(d, a)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::questions::*;

    fn d(remote: bool) -> Detected {
        Detected {
            root: ".".into(),
            id: "r".into(),
            branch: "main".into(),
            remote_name: remote.then(|| "origin".to_string()),
            remote: None,
            languages: vec![],
            ci: vec![],
            agents: vec![],
            existing_mason: false,
        }
    }

    #[test]
    fn the_commands_follow_the_remote_and_the_landing_mode() {
        let a = |landing| Answers {
            id: "r".into(),
            tracker: Tracker::Local,
            agent: Agent::Claude,
            agent_bin: None,
            sessions: 1,
            workflow: Workflow::SoloFast,
            accept_suites: false,
            base: "main".into(),
            landing,
            alerts: None,
            ci_workflow: false,
            land: Land::No,
            prove: false,
            token: None,
            placement: None,
            test: None,
            notes: vec![],
            warnings: vec![],
        };
        let direct = commands(&d(true), &a(Landing::Direct));
        assert!(direct.contains("git push origin main") && direct.contains("--ff-only"));
        assert!(!commands(&d(false), &a(Landing::Direct)).contains("push"));
        let pr = commands(&d(true), &a(Landing::Pr));
        assert!(pr.contains("git push origin mason/setup") && pr.contains("gh pr create"));
    }
}

#[cfg(test)]
mod warn_tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let o = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}");
    }

    #[test]
    fn a_remote_without_mason_warns_with_the_push_command_until_it_has_it() {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("bare.git");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", "bare.git"],
        );
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        std::fs::write(work.join("a"), "a").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "a"]);
        git(&work, &["push", "-q", "origin", "main"]);
        let d = crate::detect::detect(&work).unwrap();
        let w = unpublished_warning(&d, "main", Landing::Direct, Land::Merge).unwrap();
        assert!(w.contains("git push origin main"), "{w}");
        assert!(unpublished_warning(&d, "main", Landing::Direct, Land::No).is_some());
        assert!(unpublished_warning(&d, "main", Landing::Direct, Land::Push).is_none());
        std::fs::create_dir_all(work.join(".mason")).unwrap();
        std::fs::write(work.join(".mason/mason.toml"), "").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "mason"]);
        assert!(!remote_base_has_setup(&d, "main"));
        git(&work, &["push", "-q", "origin", "main"]);
        assert!(remote_base_has_setup(&d, "main"));
        assert!(unpublished_warning(&d, "main", Landing::Direct, Land::Merge).is_none());
    }
}
