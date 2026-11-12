// SPDX-License-Identifier: GPL-3.0-or-later

//! The project's outside services, built from its `[host]`, `[ci]` and
//! `[tracker]` sections. One GitHub object serves all three roles.

use crate::config::{CiCfg, CiKind, HostCfg, ProjectCfg, TrackerCfg};
use mason_adapter_api::http::Curl;
use mason_adapter_api::service::*;
use mason_adapters::fakehost::{CommandCi, FakeHost};
use mason_adapters::generic::Generic;
use mason_adapters::github::{GitHub, TokenRef};
use mason_adapters::jenkins::Jenkins;
use std::sync::Arc;

pub fn token(r: &str) -> Result<TokenRef, String> {
    TokenRef::parse(r)
}

fn github(api: &str, repo: &str, tok: &str, workflow: Option<String>) -> Result<GitHub, String> {
    let mut g = GitHub::new(Arc::new(Curl::default()), api, repo, token(tok)?);
    g.workflow = workflow;
    Ok(g)
}

fn infra_of(c: Option<&CiCfg>) -> Option<Vec<String>> {
    c.filter(|c| c.infra_judge)
        .map(|c| c.infra_patterns.clone())
}

pub fn host(p: &ProjectCfg) -> Result<Option<Arc<dyn GitHostAdapter>>, String> {
    Ok(match &p.services.host {
        None => None,
        Some(HostCfg::Github { api, repo, token }) => {
            let mut g = github(api, repo, token, None)?;
            // Pull-request checks are judged by the same `[ci]` setting.
            g.infra = infra_of(p.services.ci.as_ref());
            Some(Arc::new(g))
        }
        Some(HostCfg::Fake {
            dir,
            remote,
            checks,
            require_approval,
            merge_queue,
        }) => Some(Arc::new(FakeHost {
            dir: dir.clone(),
            remote: remote.clone(),
            checks_cmd: checks.clone(),
            require_approval: *require_approval,
            merge_queue: *merge_queue,
        })),
    })
}

pub type Ci = (Arc<dyn BuildSystemAdapter>, CiCfg);
pub type Tracker = (Arc<dyn TrackerAdapter>, TrackerCfg);

pub fn ci(p: &ProjectCfg) -> Result<Option<Ci>, String> {
    Ok(match &p.services.ci {
        None => None,
        Some(c) => {
            let a: Arc<dyn BuildSystemAdapter> = match &c.kind {
                CiKind::GithubActions {
                    api,
                    repo,
                    token,
                    workflow,
                } => {
                    let mut g = github(api, repo, token, workflow.clone())?;
                    g.infra = infra_of(Some(c));
                    Arc::new(g)
                }
                CiKind::Jenkins {
                    url,
                    job,
                    token: tok,
                    user,
                    crumb,
                } => {
                    let mut j = Jenkins::new(Arc::new(Curl::default()), url, job, token(tok)?);
                    j.user = user.clone();
                    j.crumb = *crumb;
                    Arc::new(j)
                }
                CiKind::Generic {
                    trigger,
                    status,
                    cwd,
                    token: tok,
                } => {
                    let mut g = Generic::new(trigger.clone(), status.clone(), cwd.clone())
                        .with_http(Arc::new(Curl::default()));
                    g.token = tok.as_deref().map(token).transpose()?;
                    Arc::new(g)
                }
                CiKind::Command {
                    trigger,
                    status,
                    cwd,
                } => Arc::new(CommandCi {
                    trigger: trigger.clone(),
                    status: status.clone(),
                    cwd: cwd.clone(),
                }),
            };
            Some((a, c.clone()))
        }
    })
}

pub fn tracker(p: &ProjectCfg) -> Result<Option<Tracker>, String> {
    Ok(match &p.services.tracker {
        None => None,
        Some(t) => Some((
            Arc::new(github(&t.api, &t.repo, &t.token, None)?),
            t.clone(),
        )),
    })
}
