// SPDX-License-Identifier: GPL-3.0-or-later

//! Asking MASON to improve itself (Part 13). The owner types a request; it becomes a task in
//! MASON's own project and follows the normal flow. With write access the branch lands by the
//! project's configured mode. Without it, the branch goes to the owner's fork, the local
//! daemon switches to the channel `branch:<name>`, and nothing goes upstream until the owner
//! has read the redacted version and confirmed. When the upstream pull request is merged the
//! daemon rejoins the main channel; when it is rejected, or no longer merges, the owner is
//! asked whether to keep carrying the patch.
//!
//! Privacy: what leaves the machine is the redacted title and text. Client projects' names and
//! paths are removed, fenced code is dropped, and a branch whose diff or messages mention a
//! client project is not published at all.

use crate::daemon::{now_ms, Daemon};
use crate::jobs::Ctx;
use crate::proposals::{self, Access};
use crate::update::{Channel, Request, UpdateCfg};
use mason_core::proposal::{redact, Source};
use mason_core::*;
use mason_landing::git;
use mason_store::ProposalRow;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

type Fail = (u16, String);

fn bad(code: u16, m: impl Into<String>) -> Fail {
    (code, m.into())
}

/// File the owner's request as a task in MASON's own project.
pub fn improve(d: &Arc<Daemon>, project: &str, text: &str) -> Result<(TaskId, String), Fail> {
    let pid = d
        .project_id_by_name(project)
        .ok_or_else(|| bad(404, format!("no project {project:?}")))?;
    let p = d.project(pid).expect("exists");
    if p.self_cfg.is_none() {
        return Err(bad(
            400,
            format!("{project} is not MASON's own repository (it has no [self] section)"),
        ));
    }
    let text = text.trim();
    if text.is_empty() {
        return Err(bad(400, "say what should be improved"));
    }
    crate::selfguard::check_filing_as(d, pid, TrustSource::Owner).map_err(|e| bad(429, e))?;
    let title: String = text
        .lines()
        .next()
        .unwrap_or(text)
        .chars()
        .take(120)
        .collect();
    let cfg = p.proposals();
    let appended = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.clone(),
            body: text.to_string(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: false,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .map_err(|e| bad(500, e.to_string()))?;
    let task = appended
        .effects
        .iter()
        .find_map(|e| {
            if let Effect::TaskCreated { task } = e {
                Some(*task)
            } else {
                None
            }
        })
        .ok_or_else(|| bad(500, "the task was not created"))?;
    let (access, branch) = match cfg.access {
        Access::Fork => ("fork", format!("mason/improve/{}", task.0)),
        Access::Write => ("write", p.branch(task.0)),
    };
    d.store
        .put_proposal(&ProposalRow {
            task: task.0,
            project: p.id.clone(),
            source: Source::Owner.name().into(),
            severity: "normal".into(),
            route: "pipeline".into(),
            filed_by: None,
            title,
            body: text.to_string(),
            evidence: String::new(),
            key: format!("improve:{}", task.0),
            status: "approved".into(),
            reject_reason: String::new(),
            publish_state: "local".into(),
            access: access.into(),
            branch: branch.clone(),
            pr: None,
            pr_state: String::new(),
            at: now_ms(),
        })
        .map_err(|e| bad(500, e.to_string()))?;
    Ok((task, branch))
}

// ---- the fork branch and the channel -----------------------------------------------------

fn switch_file(d: &Daemon) -> PathBuf {
    d.cfg.data_dir.join("improve").join("switch.json")
}

/// The URL of the fork: `fork` names a remote of the checkout, or is a URL or path itself.
fn fork_url(p: &crate::config::ProjectCfg, fork: &str) -> String {
    git::run(&p.path, &["remote", "get-url", fork]).unwrap_or_else(|_| fork.to_string())
}

/// Landing without write access: push the task's branch to the fork and stop there. The task
/// is done as far as MASON's pipeline goes; the daemon then follows the branch.
pub fn land_on_fork(c: &Ctx, row: &ProposalRow) {
    let cfg = c.project.proposals();
    let Some(fork) = cfg.fork.clone() else {
        return c.fail("this project has no `fork` configured, so the branch cannot be pushed");
    };
    let refspec = format!("{}:refs/heads/{}", c.branch, row.branch);
    if let Err(e) = git::run(&c.wt, &["push", "--force", &fork, &refspec]) {
        return c.fail(format!("pushing {} to the fork failed: {e}", row.branch));
    }
    let _ =
        c.d.store
            .update_proposal(row.task, |p| p.pr_state = "branch".into());
    c.green();
    // The branch stays for publishing: it is the thing the owner may send upstream.
    crate::jobs::cleanup(&c.d, c.task, false);
    let (d, branch, task) = (c.d.clone(), row.branch.clone(), row.task);
    let _ = std::thread::Builder::new()
        .name("improve-switch".into())
        .spawn(move || {
            if let Err(e) = switch_channel(&d, &branch, task) {
                tracing::warn!(%e, "could not switch to the improvement's branch");
                d.record_bug("improve_switch", &e);
            }
        });
}

fn edit_update_toml(d: &Daemon, channel: &str, repo: Option<&str>) -> Result<UpdateCfg, String> {
    let path = d.cfg.config_dir.join("update.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|_| "there is no update.toml, so there is no channel to switch".to_string())?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("update.toml is not valid TOML: {e}"))?;
    doc["channel"] = toml_edit::value(channel);
    if let Some(r) = repo {
        doc["repo"] = toml_edit::value(r);
    }
    let new = doc.to_string();
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &new).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    UpdateCfg::parse(&new)
}

/// Point the update channel at the improvement's branch on the fork, remembering where it
/// was, then build and test it (and take over, unless the project says `switch = "check"`).
pub fn switch_channel(d: &Arc<Daemon>, branch: &str, task: u64) -> Result<(), String> {
    let (_, p) = proposals::self_project(d).ok_or("no project of MASON's own")?;
    let cfg = p.proposals();
    let fork = cfg.fork.clone().ok_or("no fork configured")?;
    let prev = UpdateCfg::load(&d.cfg.config_dir)?;
    let file = switch_file(d);
    let created = !file.exists();
    if created {
        // Remember the first place only: a second improvement must not overwrite the way home.
        let _ = std::fs::create_dir_all(file.parent().expect("has a parent"));
        std::fs::write(&file, json!({"task": task, "branch": branch, "repo": prev.repo, "channel": prev.channel.name()}).to_string())
            .map_err(|e| e.to_string())?;
    }
    let chan = Channel::Branch(branch.to_string());
    edit_update_toml(d, &chan.name(), Some(&fork_url(p, &fork)))?;
    let built = crate::update::run(
        d,
        &Request {
            channel: Some(chan),
            check_only: !cfg.switch_takes_over,
            ..Default::default()
        },
    );
    let sha = match built {
        Ok(sha) => sha,
        Err(e) => {
            // A branch that does not build or pass its self-test is not followed.
            let _ = edit_update_toml(d, &prev.channel.name(), Some(&prev.repo));
            if created {
                let _ = std::fs::remove_file(&file);
            }
            return Err(format!(
                "the branch {branch} was not followed, because it failed: {e}"
            ));
        }
    };
    if cfg.switch_takes_over {
        crate::update::drain_and_take_over(d, &sha, false);
    }
    Ok(())
}

/// The channel the daemon follows now, if it was switched to an improvement's branch.
pub fn switched(d: &Daemon) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(switch_file(d)).ok()?).ok()
}

/// Back to where the daemon was, on the main channel.
pub fn rejoin_main(d: &Arc<Daemon>) -> Result<(), String> {
    let Some(s) = switched(d) else { return Ok(()) };
    edit_update_toml(d, "main", s["repo"].as_str())?;
    let _ = std::fs::remove_file(switch_file(d));
    Ok(())
}

// ---- publishing -----------------------------------------------------------------------------

/// Names and paths of every project but MASON's own, which must not leave the machine.
fn client_terms(d: &Daemon) -> (Vec<String>, Vec<String>) {
    let (mut names, mut roots) = (vec![], vec![]);
    for p in d.projects().values().filter(|p| p.self_cfg.is_none()) {
        names.push(p.id.clone());
        if let Some(n) = p.path.file_name().and_then(|n| n.to_str()) {
            names.push(n.to_string());
        }
        if let Some(t) = &p.services.tracker {
            names.extend(t.repo.split('/').map(String::from));
        }
        roots.push(p.path.display().to_string());
        if let Ok(c) = std::fs::canonicalize(&p.path) {
            roots.push(c.display().to_string());
        }
    }
    names.retain(|n| n.trim().len() >= 3);
    names.sort();
    names.dedup();
    roots.sort();
    roots.dedup();
    (names, roots)
}

fn mentions(text: &str, names: &[String], roots: &[String]) -> usize {
    let lower = text.to_lowercase();
    names
        .iter()
        .map(|n| lower.matches(&n.to_lowercase()).count())
        .sum::<usize>()
        + roots
            .iter()
            .map(|r| text.matches(r.as_str()).count())
            .sum::<usize>()
}

/// Publish an improvement's branch upstream as a pull request. `confirm: false` only shows
/// what would be sent. Sending needs the owner to have read that preview, and the owner's
/// word (`confirm`).
pub fn publish(d: &Arc<Daemon>, task: u64, confirm: bool) -> Result<Value, Fail> {
    let row = d
        .store
        .proposal(task)
        .ok()
        .flatten()
        .ok_or_else(|| bad(404, format!("task #{task} is not a proposal")))?;
    let (_, sp) =
        proposals::self_project(d).ok_or_else(|| bad(409, "no project of MASON's own"))?;
    if row.route != "pipeline" {
        return Err(bad(409, format!("#{task} belongs to a client project and stays there; only MASON's own improvements can be published")));
    }
    if row.access != "fork" {
        return Err(bad(409, format!("#{task} lands in the repository directly (you have write access); there is nothing to publish")));
    }
    if row.pr.is_some() {
        return Err(bad(
            409,
            format!(
                "#{task} is already upstream as pull request #{}",
                row.pr.unwrap_or(0)
            ),
        ));
    }
    let cfg = sp.proposals();
    let (names, roots) = client_terms(d);
    let text = format!("{}\n\n{}", row.body, row.evidence);
    let (body, removed_body) = redact(text.trim(), &names, &roots);
    let (title, removed_title) = redact(&row.title, &names, &roots);
    let title = title.lines().next().unwrap_or("").to_string();
    let local_branch = sp.branch(task);
    let range = format!("{}...{}", sp.base_ref(), local_branch);
    let diff = git::run(&sp.path, &["diff", "--no-color", &range]);
    let log = git::run(&sp.path, &["log", "--format=%B", &range]);
    let files = git::run(&sp.path, &["diff", "--name-only", &range]).unwrap_or_default();
    let mut blocked = None;
    match (&diff, &log) {
        (Ok(diff), Ok(log)) => {
            let n = mentions(diff, &names, &roots) + mentions(log, &names, &roots);
            if n > 0 {
                blocked = Some(format!("the branch's changes mention a client project {n} time(s); it is not published, because client code is never attached. Remove those lines and run the preview again"));
            }
        }
        _ => blocked = Some(format!("cannot read the branch {local_branch} to check it for client material; it is not published")),
    }
    let head = match &cfg.fork_owner {
        Some(o) => format!("{o}:{}", row.branch),
        None => row.branch.clone(),
    };
    let preview = json!({
        "task": task, "title": title, "body": body, "head": head, "base": sp.base,
        "files": files.lines().collect::<Vec<_>>(), "redactions": removed_body + removed_title,
        "blocked": blocked, "attachments": [],
    });
    if !confirm {
        if blocked.is_none() && row.publish_state == "local" {
            let _ = d
                .store
                .update_proposal(task, |p| p.publish_state = "previewed".into());
        }
        return Ok(preview);
    }
    if let Some(b) = blocked {
        return Err(bad(409, b));
    }
    if row.publish_state != "previewed" {
        return Err(bad(
            409,
            format!(
                "look at the redacted version first: `mason proposal publish {task} --preview`"
            ),
        ));
    }
    let host = crate::services::host(sp)
        .map_err(|e| bad(500, e))?
        .ok_or_else(|| {
            bad(
                409,
                "the project has no [host] section, so there is nowhere to open a pull request",
            )
        })?;
    let pr = host
        .open_pr(&sp.base, &head, &title, &body)
        .map_err(|e| bad(502, format!("the host refused the pull request: {e}")))?;
    let _ = d.store.update_proposal(task, |p| {
        p.pr = Some(pr.number);
        p.pr_state = "open".into();
        p.publish_state = "published".into();
    });
    let mut out = preview;
    out["pr"] = json!(pr.number);
    out["url"] = json!(pr.url);
    Ok(out)
}

// ---- following the upstream pull request -------------------------------------------------

/// Look at each improvement that went upstream. Merged: rejoin the main channel. Closed
/// without merging, or no longer merging cleanly: ask the owner about carrying the patch.
pub fn poll(d: &Arc<Daemon>) {
    let Some((pid, sp)) = proposals::self_project(d) else {
        return;
    };
    let Ok(Some(host)) = crate::services::host(sp) else {
        return;
    };
    for row in d.store.proposals().unwrap_or_default() {
        let (Some(pr), "open" | "conflict") = (row.pr, row.pr_state.as_str()) else {
            continue;
        };
        let Ok(st) = host.pr_status(pr) else { continue };
        use mason_adapter_api::service::PrState;
        match st.state {
            PrState::Merged => {
                let _ = d
                    .store
                    .update_proposal(row.task, |p| p.pr_state = "merged".into());
                if let Err(e) = rejoin_main(d) {
                    d.record_bug("improve_rejoin", &e);
                }
            }
            PrState::Closed => {
                let _ = d
                    .store
                    .update_proposal(row.task, |p| p.pr_state = "closed".into());
                ask_about_carrying(
                    d,
                    pid,
                    &row,
                    &format!("upstream closed pull request #{pr} without merging it"),
                );
            }
            PrState::Open => {
                if row.pr_state == "open" && conflicts(d, sp, &row) {
                    let _ = d
                        .store
                        .update_proposal(row.task, |p| p.pr_state = "conflict".into());
                    ask_about_carrying(
                        d,
                        pid,
                        &row,
                        &format!(
                            "pull request #{pr} no longer merges cleanly into upstream's {}",
                            sp.base
                        ),
                    );
                }
            }
        }
    }
}

fn conflicts(d: &Daemon, sp: &crate::config::ProjectCfg, row: &ProposalRow) -> bool {
    let _ = d;
    let Some(remote) = &sp.remote else {
        return false;
    };
    let _ = git::run(&sp.path, &["fetch", "-q", remote, &sp.base]);
    git::run(
        &sp.path,
        &[
            "merge-tree",
            "--write-tree",
            &sp.base_ref(),
            &sp.branch(row.task),
        ],
    )
    .is_err()
        && git::rev_parse(&sp.path, &sp.branch(row.task)).is_ok()
}

fn ask_about_carrying(d: &Arc<Daemon>, pid: ProjectId, row: &ProposalRow, why: &str) {
    let title = format!("Keep carrying the patch from #{}?", row.task);
    let body = format!(
        "{why}. This machine is following the branch `{}` of your fork.\n\nApprove to keep carrying it (the daemon stays on the branch; rebase it or file a follow-up). Reject to rejoin the main channel.\n\nOriginal request: {}",
        row.branch, row.title
    );
    let Ok(appended) = d.emit(EventKind::TaskFiled {
        project: pid,
        title: title.clone(),
        body: body.clone(),
        priority: Priority::High,
        trust: TrustSource::Owner,
        explored: false,
        proposed: true,
        blockers: vec![],
        not_before: None,
    }) else {
        return;
    };
    if let Some(task) = appended.effects.iter().find_map(|e| {
        if let Effect::TaskCreated { task } = e {
            Some(*task)
        } else {
            None
        }
    }) {
        let _ = d.store.put_proposal(&ProposalRow {
            task: task.0,
            project: row.project.clone(),
            source: Source::Owner.name().into(),
            severity: "normal".into(),
            route: "pipeline".into(),
            filed_by: None,
            title,
            body,
            evidence: format!("{why} ({})", row.branch),
            key: format!("carry:{}", row.task),
            status: "open".into(),
            reject_reason: String::new(),
            publish_state: "local".into(),
            access: String::new(),
            branch: row.branch.clone(),
            pr: None,
            pr_state: String::new(),
            at: now_ms(),
        });
    }
}

/// The owner answered a "keep carrying it?" proposal. Rejecting rejoins the main channel.
pub fn carry_decided(d: &Arc<Daemon>, task: u64, approved: bool) {
    let Ok(Some(row)) = d.store.proposal(task) else {
        return;
    };
    if row.key.starts_with("carry:") && !approved {
        if let Err(e) = rejoin_main(d) {
            d.record_bug("improve_rejoin", &e);
        }
    }
}
