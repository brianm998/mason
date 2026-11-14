// SPDX-License-Identifier: GPL-3.0-or-later

//! Tracker, git host and CI adapters that are a protocol 3 plugin (decision 0286).
//!
//! Each wrapper implements the service trait from `mason-adapter-api` by calling the
//! plugin's JSON-RPC methods (see [`crate::rpc`]), so everything that takes a
//! `dyn TrackerAdapter`, `dyn GitHostAdapter` or `dyn BuildSystemAdapter` (the pollers, the
//! landing code, the conformance kits) works with a plugin unchanged.
//!
//! The plugin process is started on first use and checked with `initialize`: its name must
//! be the manifest's and it must list what the wrapper needs in `provides`. A call that
//! times out or finds the process gone kills it; the next call starts a fresh one. A call
//! never holds more than one process at a time (a mutex), because the protocol is
//! request and answer on one pipe.

use crate::host::{self, CallError, Launch, Plugin};
use crate::rpc;
use mason_adapter_api::service::*;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// A plugin process on demand, speaking for one `provides` entry.
pub struct Link {
    launch: Launch,
    /// The manifest's `name`.
    plugin: String,
    protocol: u32,
    /// What `initialize` must list, e.g. `adapter:tracker:acme`.
    provides: String,
    timeout: Duration,
    live: Mutex<Option<Plugin>>,
}

impl Link {
    pub fn new(
        launch: Launch,
        plugin: &str,
        protocol: u32,
        provides: &str,
        timeout: Duration,
    ) -> Link {
        Link {
            launch,
            plugin: plugin.into(),
            protocol,
            provides: provides.into(),
            timeout,
            live: Mutex::new(None),
        }
    }

    fn start(&self) -> Result<Plugin, ApiError> {
        let down = |d: String| ApiError::new(ErrorKind::Unavailable, d);
        let mut p = Plugin::spawn(&self.launch)
            .map_err(|e| down(format!("cannot start plugin `{}`: {e}", self.plugin)))?;
        let init = host::initialize_as(&mut p, self.timeout, self.protocol).map_err(down)?;
        if init.name != self.plugin {
            return Err(ApiError::new(
                ErrorKind::Invalid,
                format!(
                    "the plugin says it is `{}`, but its manifest says `{}`",
                    init.name, self.plugin
                ),
            ));
        }
        if !init.provides.contains(&self.provides) {
            return Err(ApiError::new(
                ErrorKind::Invalid,
                format!(
                    "plugin `{}` does not say it provides `{}` (it says {:?})",
                    self.plugin, self.provides, init.provides
                ),
            ));
        }
        Ok(p)
    }

    /// One call. An error answer is mapped to an [`ApiError`] by its code.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, ApiError> {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if live.is_none() {
            *live = Some(self.start()?);
        }
        let p = live.as_mut().expect("just started");
        match p.call(method, &params, self.timeout) {
            Ok(v) => Ok(v),
            Err(CallError::Rpc(e)) => Err(map_error(&e)),
            Err(e) => {
                // Timed out or gone: the process was killed; start a fresh one next time.
                *live = None;
                Err(ApiError::new(
                    ErrorKind::Unavailable,
                    format!("{method}: {e}"),
                ))
            }
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Some(mut p) = self.live.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if p.shutdown(self.timeout).is_err() {
                p.kill();
            }
        }
    }
}

fn map_error(e: &rpc::RpcError) -> ApiError {
    let kind = match rpc::error_kind(e.code) {
        "not_found" => ErrorKind::NotFound,
        "invalid" => ErrorKind::Invalid,
        "auth" => ErrorKind::Auth,
        "rate_limited" => ErrorKind::RateLimited {
            retry_after_s: None,
        },
        _ => ErrorKind::Unavailable,
    };
    ApiError::new(kind, e.message.clone())
}

fn bad(what: &str, why: &str) -> ApiError {
    ApiError::new(
        ErrorKind::Unavailable,
        format!("the plugin's `{what}` answer is not what the protocol says: {why}"),
    )
}

fn flag(v: &Value, k: &str) -> bool {
    v[k].as_bool().unwrap_or(false)
}

fn text(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    v[k].as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// A name for the trait's `&'static str`: made once per wrapper and kept for the process.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

// ---- tracker -----------------------------------------------------------------------

pub struct PluginTracker {
    link: Link,
    name: &'static str,
}

impl PluginTracker {
    /// `plugin` is the manifest's name; `adapter` is the name after `adapter:tracker:`.
    pub fn new(
        launch: Launch,
        plugin: &str,
        protocol: u32,
        adapter: &str,
        timeout: Duration,
    ) -> PluginTracker {
        PluginTracker {
            link: Link::new(
                launch,
                plugin,
                protocol,
                &format!("adapter:tracker:{adapter}"),
                timeout,
            ),
            name: leak(format!("plugin:{adapter}")),
        }
    }
}

fn item_json(i: &Item) -> Value {
    json!({
        "id": i.id, "number": i.number, "title": i.title, "body": i.body,
        "priority": i.priority, "labels": i.labels, "author": i.author,
        "open": i.open, "is_pr": i.is_pr,
    })
}

fn read_item(v: &Value) -> Result<Item, ApiError> {
    let number = v["number"]
        .as_u64()
        .ok_or_else(|| bad("tracker.list", "an item's `number` must be a whole number"))?;
    Ok(Item {
        id: v["id"]
            .as_str()
            .map_or_else(|| number.to_string(), String::from),
        number,
        title: text(v, "title"),
        body: text(v, "body"),
        priority: v["priority"].as_str().unwrap_or("normal").to_string(),
        labels: strings(v, "labels"),
        author: text(v, "author"),
        open: v.get("open").and_then(Value::as_bool).unwrap_or(true),
        is_pr: flag(v, "is_pr"),
    })
}

impl TrackerAdapter for PluginTracker {
    fn name(&self) -> &'static str {
        self.name
    }
    fn capabilities(&self) -> TrackerCaps {
        match self.link.call("tracker.capabilities", json!({})) {
            Ok(v) => TrackerCaps {
                conditional_polling: flag(&v, "conditional_polling"),
                comments: flag(&v, "comments"),
                labels: flag(&v, "labels"),
                webhooks: flag(&v, "webhooks"),
                dependencies: flag(&v, "dependencies"),
            },
            Err(_) => TrackerCaps::default(),
        }
    }
    fn list(&self, etag: Option<&str>) -> Result<Listing, ApiError> {
        let v = self.link.call("tracker.list", json!({"etag": etag}))?;
        if flag(&v, "not_modified") {
            return Ok(Listing::NotModified);
        }
        let items = v["items"]
            .as_array()
            .ok_or_else(|| bad("tracker.list", "`items` must be a list"))?
            .iter()
            .map(read_item)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Listing::Items {
            items,
            etag: v["etag"].as_str().map(String::from),
        })
    }
    fn mirror(&self, item: &Item, note: &str) -> Result<(), ApiError> {
        self.link
            .call(
                "tracker.mirror",
                json!({"item": item_json(item), "note": note}),
            )
            .map(|_| ())
    }
    fn close(&self, item: &Item, note: &str) -> Result<(), ApiError> {
        self.link
            .call(
                "tracker.close",
                json!({"item": item_json(item), "note": note}),
            )
            .map(|_| ())
    }
    fn blocked_by(&self, number: u64) -> Result<Vec<Blocker>, ApiError> {
        let v = self
            .link
            .call("tracker.blocked_by", json!({"number": number}))?;
        Ok(v["blockers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|b| {
                        Some(Blocker {
                            number: b["number"].as_u64()?,
                            open: b.get("open").and_then(Value::as_bool).unwrap_or(true),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
    fn set_priority(&self, number: u64, priority: &str) -> Result<(), ApiError> {
        self.link
            .call(
                "tracker.set_priority",
                json!({"number": number, "priority": priority}),
            )
            .map(|_| ())
    }
}

// ---- git host ----------------------------------------------------------------------

pub struct PluginHost {
    link: Link,
    name: &'static str,
}

impl PluginHost {
    /// `plugin` is the manifest's name; `adapter` is the name after `adapter:host:`.
    pub fn new(
        launch: Launch,
        plugin: &str,
        protocol: u32,
        adapter: &str,
        timeout: Duration,
    ) -> PluginHost {
        PluginHost {
            link: Link::new(
                launch,
                plugin,
                protocol,
                &format!("adapter:host:{adapter}"),
                timeout,
            ),
            name: leak(format!("plugin:{adapter}")),
        }
    }
}

impl GitHostAdapter for PluginHost {
    fn name(&self) -> &'static str {
        self.name
    }
    fn capabilities(&self) -> HostCaps {
        match self.link.call("host.capabilities", json!({})) {
            Ok(v) => HostCaps {
                pull_requests: flag(&v, "pull_requests"),
                labels: flag(&v, "labels"),
                checks: flag(&v, "checks"),
                merge_queue: flag(&v, "merge_queue"),
            },
            Err(_) => HostCaps::default(),
        }
    }
    fn open_pr(&self, base: &str, head: &str, title: &str, body: &str) -> Result<PrRef, ApiError> {
        let v = self.link.call(
            "host.open_pr",
            json!({"base": base, "head": head, "title": title, "body": body}),
        )?;
        Ok(PrRef {
            number: v["number"]
                .as_u64()
                .ok_or_else(|| bad("host.open_pr", "`number` must be a whole number"))?,
            url: text(&v, "url"),
        })
    }
    fn pr_status(&self, number: u64) -> Result<PrStatus, ApiError> {
        let v = self
            .link
            .call("host.pr_status", json!({"number": number}))?;
        let state = match v["state"].as_str() {
            Some("open") => PrState::Open,
            Some("merged") => PrState::Merged,
            Some("closed") => PrState::Closed,
            _ => return Err(bad("host.pr_status", "`state` is open, merged or closed")),
        };
        let checks = match v["checks"].as_str() {
            Some("pending") | None => Checks::Pending,
            Some("green") => Checks::Green,
            Some("red") => Checks::Red,
            _ => return Err(bad("host.pr_status", "`checks` is pending, green or red")),
        };
        let review = match v["review"].as_str() {
            Some("satisfied") | None => Review::Satisfied,
            Some("required") => Review::Required,
            Some("changes_requested") => Review::ChangesRequested,
            _ => {
                return Err(bad(
                    "host.pr_status",
                    "`review` is satisfied, required or changes_requested",
                ))
            }
        };
        Ok(PrStatus {
            state,
            checks,
            failed: strings(&v, "failed"),
            review,
            head: text(&v, "head"),
            counts: None,
            jobs: vec![],
        })
    }
    fn merge_pr(&self, number: u64) -> Result<(), ApiError> {
        self.link
            .call("host.merge_pr", json!({"number": number}))
            .map(|_| ())
    }
    fn enqueue(&self, number: u64) -> Result<(), ApiError> {
        self.link
            .call("host.enqueue", json!({"number": number}))
            .map(|_| ())
    }
    fn close_pr(&self, number: u64) -> Result<(), ApiError> {
        self.link
            .call("host.close_pr", json!({"number": number}))
            .map(|_| ())
    }
}

// ---- CI ----------------------------------------------------------------------------

pub struct PluginCi {
    link: Link,
    name: &'static str,
}

impl PluginCi {
    /// `plugin` is the manifest's name; `adapter` is the name after `adapter:ci:`.
    pub fn new(
        launch: Launch,
        plugin: &str,
        protocol: u32,
        adapter: &str,
        timeout: Duration,
    ) -> PluginCi {
        PluginCi {
            link: Link::new(
                launch,
                plugin,
                protocol,
                &format!("adapter:ci:{adapter}"),
                timeout,
            ),
            name: leak(format!("plugin:{adapter}")),
        }
    }
}

impl BuildSystemAdapter for PluginCi {
    fn name(&self) -> &'static str {
        self.name
    }
    fn capabilities(&self) -> BuildCaps {
        match self.link.call("ci.capabilities", json!({})) {
            Ok(v) => BuildCaps {
                arbitrary_ref: flag(&v, "arbitrary_ref"),
                per_job_results: flag(&v, "per_job_results"),
            },
            Err(_) => BuildCaps::default(),
        }
    }
    fn trigger(&self, git_ref: &str, sha: &str) -> Result<String, ApiError> {
        let v = self
            .link
            .call("ci.trigger", json!({"ref": git_ref, "sha": sha}))?;
        v["run"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| bad("ci.trigger", "`run` must be a string"))
    }
    fn status(&self, run: &str) -> Result<CiState, ApiError> {
        let v = self.link.call("ci.status", json!({"run": run}))?;
        match v["state"].as_str() {
            Some("pending") => Ok(CiState::Pending),
            Some("green") => Ok(CiState::Green),
            Some("red") => Ok(CiState::Red {
                failed: strings(&v, "failed"),
                summary: text(&v, "summary"),
                jobs: vec![],
            }),
            _ => Err(bad("ci.status", "`state` is pending, green or red")),
        }
    }
    fn doctor(&self, repo: &Path) -> Vec<Problem> {
        match self
            .link
            .call("ci.doctor", json!({"repo": repo.to_string_lossy()}))
        {
            Ok(v) => strings(&v, "problems").into_iter().map(Problem).collect(),
            Err(e) => vec![Problem(format!("the plugin could not be asked: {e}"))],
        }
    }
}
