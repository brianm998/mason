// SPDX-License-Identifier: GPL-3.0-or-later

//! Daemon settings and per-project configuration.
//!
//! A project is one file in the local overlay,
//! `<config dir>/projects/<id>.toml`. (The versioned `.mason/mason.toml`
//! definition arrives in Stage 7; machine facts like these stay local.)

use std::path::{Path, PathBuf};
use std::time::Duration;
use toml_edit::DocumentMut;

#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    /// Tasks in a step at once. Admission waits when this many are running.
    pub max_concurrent: usize,
    pub sweep_interval: Duration,
    pub heartbeat: Duration,
    pub host_bin: PathBuf,
    pub mcp_bin: PathBuf,
    /// 0 picks a free loopback port (written to `daemon.json`).
    pub port: u16,
    pub stall_threshold: Duration,
    /// How long a provider hold waits before the session is resumed.
    pub hold_retry: Duration,
    /// Enables `POST /debug/stall`.
    pub debug: bool,
    /// How often the definition watcher looks for a changed base branch or
    /// overlay file.
    pub reload_interval: Duration,
    /// How often the watcher fetches each project's remote to see whether the
    /// base branch (and so its definition) changed. Local changes are seen at
    /// `reload_interval`; an explicit reload always fetches.
    pub fetch_interval: Duration,
    /// How long a suite waits before trying again after an environmental
    /// failure (no device, the stack would not come up). Charged to no task.
    pub env_retry: Duration,
    /// Identities whose answers count (Part 10). Default: the user running the
    /// daemon.
    pub owner_names: Vec<String>,
}

impl DaemonConfig {
    pub fn new(
        data_dir: impl Into<PathBuf>,
        config_dir: impl Into<PathBuf>,
        host_bin: PathBuf,
        mcp_bin: PathBuf,
    ) -> Self {
        DaemonConfig {
            data_dir: data_dir.into(),
            config_dir: config_dir.into(),
            max_concurrent: 2,
            sweep_interval: Duration::from_secs(120),
            heartbeat: Duration::from_secs(1),
            host_bin,
            mcp_bin,
            port: 0,
            stall_threshold: Duration::from_millis(500),
            hold_retry: Duration::from_secs(60),
            debug: false,
            reload_interval: Duration::from_secs(5),
            fetch_interval: Duration::from_secs(60),
            env_retry: Duration::from_secs(30),
            owner_names: vec![std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_else(|_| "owner".into())],
        }
    }

    pub fn store_path(&self) -> PathBuf {
        self.data_dir.join("store.db")
    }
    /// Step hosts. Short on purpose: each host's socket lives under it.
    pub fn hosts_dir(&self) -> PathBuf {
        self.data_dir.join("h")
    }
    /// Contract sockets, also short.
    pub fn sockets_dir(&self) -> PathBuf {
        self.data_dir.join("s")
    }
    pub fn worktrees_dir(&self) -> PathBuf {
        self.data_dir.join("w")
    }
    pub fn landing_dir(&self) -> PathBuf {
        self.data_dir.join("l")
    }
    pub fn task_dir(&self, task: u64) -> PathBuf {
        self.data_dir.join("t").join(task.to_string())
    }
    pub fn agent_state_dir(&self) -> PathBuf {
        self.data_dir.join("a")
    }
    pub fn info_path(&self) -> PathBuf {
        self.data_dir.join("daemon.json")
    }
}

#[derive(Clone, Debug)]
pub struct ProjectCfg {
    pub id: String,
    pub path: PathBuf,
    pub base: String,
    pub remote: Option<String>,
    pub test_cmd: String,
    pub claude_bin: Option<PathBuf>,
    pub model: Option<String>,
    pub agent_env: Vec<(String, String)>,
    pub tool_timeout: Duration,
    pub step_timeout: Duration,
    pub test_timeout: Duration,
    /// Test rounds before the task fails, if the overlay sets one (layer 3).
    pub max_test_rounds: Option<u32>,
    /// A command that reads a task body on stdin and prints `clear` or
    /// `unclear`. Optional, and only consulted when the deterministic check is
    /// unsure. Anything but `clear`, or a failure, means explore.
    pub clarity_classifier: Option<String>,
    /// Opt in to letting top-priority tasks jump the "fewest steps remaining"
    /// term of the scheduling rank (Part 7). Off by default.
    pub priority_jump: bool,
    /// A cheap command that rewrites a decision card that does not fit: it reads
    /// `{"card": ..., "problems": [...]}` on stdin and prints a card as JSON.
    /// Optional; without it (or if it fails) a deterministic fallback is used.
    pub card_rewriter: Option<String>,
    /// The overlay file's text, for the configuration layers.
    pub overlay_text: String,
    /// The outside services: git host, build system, issue tracker.
    pub services: ServicesCfg,
}

/// `[host]`, `[ci]` and `[tracker]` of the project file. Credentials are
/// references (`env:NAME`, `file:/path`), never values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServicesCfg {
    pub host: Option<HostCfg>,
    pub ci: Option<CiCfg>,
    pub tracker: Option<TrackerCfg>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCfg {
    Github {
        api: String,
        repo: String,
        token: String,
    },
    /// A bare repository and a directory of files standing in for a host (tests).
    Fake {
        dir: PathBuf,
        remote: PathBuf,
        checks: Option<String>,
        require_approval: bool,
        merge_queue: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiCfg {
    pub kind: CiKind,
    /// Seconds between looks at a run.
    pub poll_s: u64,
    /// Give up on a run after this long.
    pub timeout_s: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiKind {
    GithubActions {
        api: String,
        repo: String,
        token: String,
        workflow: Option<String>,
    },
    Command {
        trigger: String,
        status: String,
        cwd: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerCfg {
    pub api: String,
    pub repo: String,
    pub token: String,
    /// The label the owner puts on an outside issue to have it imported.
    pub label: String,
    pub interval_s: u64,
    /// A reference to the webhook secret (`env:`/`file:`), when a relay delivers.
    pub webhook_secret: Option<String>,
}

fn parse_services(id: &str, doc: &DocumentMut) -> Result<ServicesCfg, String> {
    let table = |k: &str| doc.get(k).and_then(|v| v.as_table_like());
    let get =
        |t: &dyn toml_edit::TableLike, k: &str| t.get(k).and_then(|v| v.as_str()).map(String::from);
    let need = |t: &dyn toml_edit::TableLike, sec: &str, k: &str| {
        get(t, k)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("{id}: `[{sec}] {k}` is required"))
    };
    let token = |t: &dyn toml_edit::TableLike, sec: &str| -> Result<String, String> {
        let v = need(t, sec, "token")?;
        if v.starts_with("env:") || v.starts_with("file:") {
            Ok(v)
        } else {
            Err(format!("{id}: `[{sec}] token` is a reference, `env:NAME` or `file:/path`, never the token itself"))
        }
    };
    let int = |t: &dyn toml_edit::TableLike, k: &str, d: u64| {
        t.get(k)
            .and_then(|v| v.as_integer())
            .map_or(d, |n| n.max(1) as u64)
    };
    let api = |t: &dyn toml_edit::TableLike| {
        get(t, "api").unwrap_or_else(|| "https://api.github.com".into())
    };
    let mut out = ServicesCfg::default();
    if let Some(t) = table("host") {
        out.host = Some(match need(t, "host", "kind")?.as_str() {
            "github" => HostCfg::Github {
                api: api(t),
                repo: need(t, "host", "repo")?,
                token: token(t, "host")?,
            },
            "fake" => HostCfg::Fake {
                dir: need(t, "host", "dir")?.into(),
                remote: need(t, "host", "remote")?.into(),
                checks: get(t, "checks"),
                require_approval: t
                    .get("require_approval")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                merge_queue: t
                    .get("merge_queue")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            },
            o => {
                return Err(format!(
                    "{id}: `[host] kind` is `github` or `fake`, not {o:?}"
                ))
            }
        });
    }
    if let Some(t) = table("ci") {
        let kind = match need(t, "ci", "kind")?.as_str() {
            "github-actions" => CiKind::GithubActions {
                api: api(t),
                repo: need(t, "ci", "repo")?,
                token: token(t, "ci")?,
                workflow: get(t, "workflow"),
            },
            "command" => CiKind::Command {
                trigger: need(t, "ci", "trigger")?,
                status: need(t, "ci", "status")?,
                cwd: get(t, "cwd").map_or_else(|| PathBuf::from("."), PathBuf::from),
            },
            o => {
                return Err(format!(
                    "{id}: `[ci] kind` is `github-actions` or `command`, not {o:?}"
                ))
            }
        };
        out.ci = Some(CiCfg {
            kind,
            poll_s: int(t, "poll_s", 20),
            timeout_s: int(t, "timeout_s", 7200),
        });
    }
    if let Some(t) = table("tracker") {
        match need(t, "tracker", "kind")?.as_str() {
            "github" => {}
            o => return Err(format!("{id}: `[tracker] kind` is `github` (the local tracker needs no section), not {o:?}")),
        }
        let secret = get(t, "webhook_secret");
        if secret
            .as_ref()
            .is_some_and(|s| !(s.starts_with("env:") || s.starts_with("file:")))
        {
            return Err(format!(
                "{id}: `[tracker] webhook_secret` is a reference, `env:NAME` or `file:/path`"
            ));
        }
        out.tracker = Some(TrackerCfg {
            api: api(t),
            repo: need(t, "tracker", "repo")?,
            token: token(t, "tracker")?,
            label: get(t, "label").unwrap_or_else(|| "mason".into()),
            interval_s: int(t, "interval_s", 60),
            webhook_secret: secret,
        });
    }
    Ok(out)
}

impl ProjectCfg {
    pub fn branch(&self, task: u64) -> String {
        format!("mason/{}/t{task}", self.id)
    }
    pub fn worktree(&self, cfg: &DaemonConfig, task: u64) -> PathBuf {
        cfg.worktrees_dir().join(&self.id).join(task.to_string())
    }
    pub fn landing_worktree(&self, cfg: &DaemonConfig) -> PathBuf {
        cfg.landing_dir().join(&self.id)
    }
    /// The ref work starts from and lands onto.
    pub fn base_ref(&self) -> String {
        match &self.remote {
            Some(r) => format!("{r}/{}", self.base),
            None => self.base.clone(),
        }
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn parse_project(id: &str, text: &str) -> Result<ProjectCfg, String> {
    if !valid_id(id) {
        return Err(format!(
            "project id {id:?} must be lowercase letters, digits, - and _"
        ));
    }
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("{id}: not valid TOML: {e}"))?;
    let s = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(String::from);
    let need = |k: &str| {
        s(k).filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("{id}: `{k}` is required"))
    };
    let secs = |k: &str, d: u64| {
        doc.get(k)
            .and_then(|v| v.as_integer())
            .map_or(d, |n| n.max(1) as u64)
    };
    let mut agent_env = vec![];
    if let Some(t) = doc.get("agent_env").and_then(|v| v.as_table_like()) {
        for (k, v) in t.iter() {
            let v = v
                .as_str()
                .ok_or_else(|| format!("{id}: agent_env.{k} must be a string"))?;
            agent_env.push((k.to_string(), v.to_string()));
        }
    }
    let base = need("base")?;
    if !mason_landing::git::valid_branch(&base) {
        return Err(format!(
            "{id}: base branch {base:?} is not a plain branch name"
        ));
    }
    Ok(ProjectCfg {
        id: id.to_string(),
        path: PathBuf::from(need("path")?),
        base,
        remote: s("remote").filter(|r| !r.is_empty()),
        test_cmd: need("test")?,
        claude_bin: s("claude_bin").map(PathBuf::from),
        model: s("model"),
        agent_env,
        tool_timeout: Duration::from_secs(secs("tool_timeout_s", 600)),
        step_timeout: Duration::from_secs(secs("step_timeout_s", 4 * 3600)),
        test_timeout: Duration::from_secs(secs("test_timeout_s", 1800)),
        max_test_rounds: doc
            .get("max_test_rounds")
            .and_then(|v| v.as_integer())
            .map(|n| n.max(1) as u32),
        clarity_classifier: s("clarity_classifier").filter(|c| !c.trim().is_empty()),
        card_rewriter: s("card_rewriter").filter(|c| !c.trim().is_empty()),
        priority_jump: doc
            .get("priority_jumps_progress")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        overlay_text: text.to_string(),
        services: parse_services(id, &doc)?,
    })
}

/// Every `*.toml` under `<config_dir>/projects`. Errors name the file.
pub fn load_projects(config_dir: &Path) -> Result<Vec<ProjectCfg>, Vec<String>> {
    let dir = config_dir.join("projects");
    let mut out = vec![];
    let mut errs = vec![];
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    for p in files {
        let id = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        match std::fs::read_to_string(&p) {
            Ok(t) => match parse_project(&id, &t) {
                Ok(c) => out.push(c),
                Err(e) => errs.push(e),
            },
            Err(e) => errs.push(format!("{}: {e}", p.display())),
        }
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_project_parses_with_defaults() {
        let p = parse_project(
            "web-api",
            "path = \"/r\"\nbase = \"develop\"\ntest = \"make test\"\n",
        )
        .unwrap();
        assert_eq!(
            (p.base.as_str(), p.test_cmd.as_str(), p.remote.clone()),
            ("develop", "make test", None)
        );
        assert_eq!(p.max_test_rounds, None);
        assert_eq!(p.branch(7), "mason/web-api/t7");
        assert_eq!(p.base_ref(), "develop");
    }

    #[test]
    fn remote_env_and_limits_are_read() {
        let t = "path=\"/r\"\nbase=\"main\"\ntest=\"t\"\nremote=\"origin\"\nmodel=\"haiku\"\nstep_timeout_s=60\n[agent_env]\nFOO=\"bar\"\n";
        let p = parse_project("p", t).unwrap();
        assert_eq!(p.base_ref(), "origin/main");
        assert_eq!(p.agent_env, vec![("FOO".to_string(), "bar".to_string())]);
        assert_eq!(p.step_timeout, Duration::from_secs(60));
        assert_eq!(p.model.as_deref(), Some("haiku"));
    }

    #[test]
    fn mistakes_are_named() {
        for (id, t, want) in [
            ("p", "base=\"m\"\ntest=\"t\"\n", "`path` is required"),
            ("p", "path=\"/r\"\ntest=\"t\"\n", "`base` is required"),
            ("p", "path=\"/r\"\nbase=\"m\"\n", "`test` is required"),
            (
                "p",
                "path=\"/r\"\nbase=\"-rf\"\ntest=\"t\"\n",
                "not a plain branch name",
            ),
            (
                "P x",
                "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n",
                "must be lowercase",
            ),
            ("p", "path = = =", "not valid TOML"),
        ] {
            let e = parse_project(id, t).unwrap_err();
            assert!(e.contains(want), "{e:?} should contain {want:?}");
        }
    }
}
