// SPDX-License-Identifier: GPL-3.0-or-later

//! The questions of `mason init` (Part 21), as data, and everything that turns the answers
//! into text. Nothing here touches the disk, the network or a terminal: the caller supplies
//! what was detected and a function that asks, so every rule is a unit test.
//!
//! The rule for a question: if its flag is on the command line it is not asked; otherwise it
//! is asked on a terminal; otherwise `--yes` takes the recommended answer; otherwise the run
//! stops and names the flag, because guessing on behalf of someone who cannot answer would
//! write files they did not choose.

use mason_definition::filing::AGENT_NOTES;
use std::path::PathBuf;

pub const STARTING_POINT: &str = "This is only a starting point. You can change anything, any time, in the monitor or in `.mason/`.";
pub const SUITES_MARKER: &str = "# suites: mason init fills this in";
pub const SETUP_BRANCH: &str = "mason/setup";

macro_rules! template {
    ($d:literal, $f:literal) => {
        include_str!(concat!("../../../../templates/", $d, "/", $f))
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workflow {
    SoloFast,
    ReviewedPr,
    BatchedCi,
    DeviceMobile,
    HumanGated,
}

impl Workflow {
    pub const ALL: [Workflow; 5] = [
        Workflow::SoloFast,
        Workflow::ReviewedPr,
        Workflow::BatchedCi,
        Workflow::DeviceMobile,
        Workflow::HumanGated,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Workflow::SoloFast => "solo-fast",
            Workflow::ReviewedPr => "reviewed-pr",
            Workflow::BatchedCi => "batched-ci",
            Workflow::DeviceMobile => "device-mobile",
            Workflow::HumanGated => "human-gated",
        }
    }
    /// The directory name, or the short form (`solo`, `reviewed`, `batched`, `device`, `gated`).
    pub fn from_name(s: &str) -> Option<Workflow> {
        Workflow::ALL.into_iter().find(|w| {
            w.name() == s
                || w.name().split('-').next() == Some(s)
                || (s == "mobile" && *w == Workflow::DeviceMobile)
                || (s == "gated" && *w == Workflow::HumanGated)
        })
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Workflow::SoloFast => "explore, develop, fast tests, land directly (side projects, low risk)",
            Workflow::ReviewedPr => "adds an LLM review and one pull request per task (open source, or working with others)",
            Workflow::BatchedCi => "as reviewed-pr, with batches sharing one integration PR and CI run (slow or costly CI)",
            Workflow::DeviceMobile => "adds simulators and devices, with batched landing (iOS and Android apps)",
            Workflow::HumanGated => "you approve the plan and the change (getting comfortable, or sensitive code)",
        }
    }
    pub fn mason_toml(self) -> &'static str {
        match self {
            Workflow::SoloFast => template!("solo-fast", "mason.toml"),
            Workflow::ReviewedPr => template!("reviewed-pr", "mason.toml"),
            Workflow::BatchedCi => template!("batched-ci", "mason.toml"),
            Workflow::DeviceMobile => template!("device-mobile", "mason.toml"),
            Workflow::HumanGated => template!("human-gated", "mason.toml"),
        }
    }
    pub fn landing_toml(self) -> &'static str {
        match self {
            Workflow::SoloFast => template!("solo-fast", "landing.toml"),
            Workflow::ReviewedPr => template!("reviewed-pr", "landing.toml"),
            Workflow::BatchedCi => template!("batched-ci", "landing.toml"),
            Workflow::DeviceMobile => template!("device-mobile", "landing.toml"),
            Workflow::HumanGated => template!("human-gated", "landing.toml"),
        }
    }
    pub fn default_landing(self) -> Landing {
        match self {
            Workflow::SoloFast | Workflow::HumanGated => Landing::Direct,
            Workflow::ReviewedPr => Landing::Pr,
            Workflow::BatchedCi | Workflow::DeviceMobile => Landing::Batched,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    Direct,
    DirectCi,
    Pr,
    Batched,
}

impl Landing {
    pub fn name(self) -> &'static str {
        match self {
            Landing::Direct => "direct",
            Landing::DirectCi => "direct-ci",
            Landing::Pr => "pr",
            Landing::Batched => "batched",
        }
    }
    pub fn from_name(s: &str) -> Option<Landing> {
        [
            Landing::Direct,
            Landing::DirectCi,
            Landing::Pr,
            Landing::Batched,
        ]
        .into_iter()
        .find(|l| l.name() == s)
    }
    /// The `mode` of landing.toml.
    pub fn mode(self) -> &'static str {
        match self {
            Landing::Direct => "direct",
            Landing::DirectCi => "direct-ci",
            Landing::Pr => "pr-each",
            Landing::Batched => "pr-batch",
        }
    }
    pub fn needs_host(self) -> bool {
        matches!(self, Landing::Pr | Landing::Batched)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracker {
    Local,
    Github,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    Sound,
    Speech,
    Email,
    Push,
}

impl Alert {
    pub fn name(self) -> &'static str {
        match self {
            Alert::Sound => "sound",
            Alert::Speech => "speech",
            Alert::Email => "email",
            Alert::Push => "push",
        }
    }
    fn from_name(s: &str) -> Option<Alert> {
        [Alert::Sound, Alert::Speech, Alert::Email, Alert::Push]
            .into_iter()
            .find(|a| a.name() == s)
    }
}

/// A git host the remote points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub host: String,
    /// `owner/name`
    pub repo: String,
}

/// `git@github.com:o/r.git`, `https://github.com/o/r`, `ssh://git@github.com/o/r.git`.
pub fn parse_remote(url: &str) -> Option<Remote> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((_, r)) => r.to_string(),
        None => url.replace(':', "/"),
    };
    let rest = rest.rsplit_once('@').map_or(rest.as_str(), |(_, r)| r);
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let (host, path) = rest.split_once('/')?;
    let host = host.split(':').next()?.to_string();
    let (owner, name) = path.rsplit_once('/')?;
    (!host.is_empty() && !owner.is_empty() && !name.is_empty()).then(|| Remote {
        host,
        repo: format!("{owner}/{name}"),
    })
}

/// What question 1 found.
#[derive(Clone, Debug, Default)]
pub struct Detected {
    pub root: PathBuf,
    pub id: String,
    pub remote_name: Option<String>,
    pub remote: Option<Remote>,
    pub branch: String,
    /// `rust`, `python`, `swift`, `go`, `node`, `gradle`, ...
    pub languages: Vec<String>,
    pub ci: Vec<String>,
    /// Agent CLIs found on PATH.
    pub agents: Vec<(Agent, PathBuf)>,
    pub existing_mason: bool,
}

impl Detected {
    pub fn github(&self) -> Option<&Remote> {
        self.remote.as_ref().filter(|r| r.host == "github.com")
    }
}

/// A project id: lowercase letters, digits, `-` and `_`.
pub fn project_id(dir_name: &str) -> String {
    let id: String = dir_name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "project".into()
    } else {
        id
    }
}

pub fn valid_project_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Every flag of `mason init`. A `Some` stops that question being asked.
#[derive(Clone, Debug, Default)]
pub struct Flags {
    pub yes: bool,
    pub force: bool,
    pub id: Option<String>,
    pub tracker: Option<String>,
    pub agent: Option<String>,
    pub agent_bin: Option<String>,
    pub sessions: Option<String>,
    pub workflow: Option<String>,
    pub suites: Option<String>,
    pub base: Option<String>,
    pub landing: Option<String>,
    pub alerts: Option<String>,
    pub ci_workflow: Option<String>,
    pub no_prove: bool,
    pub prove_only: bool,
}

/// Takes the init flags out of `args`; anything left over is the caller's to refuse.
pub fn parse_flags(args: &mut Vec<String>) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        let take = |args: &mut Vec<String>| -> Result<String, String> {
            args.remove(i);
            if i < args.len() {
                Ok(args.remove(i))
            } else {
                Err(format!("{a} needs a value"))
            }
        };
        match a.as_str() {
            "--yes" | "-y" => {
                args.remove(i);
                f.yes = true;
            }
            "--force" => {
                args.remove(i);
                f.force = true;
            }
            "--no-prove" => {
                args.remove(i);
                f.no_prove = true;
            }
            "--prove-only" => {
                args.remove(i);
                f.prove_only = true;
            }
            "--id" => f.id = Some(take(args)?),
            "--tracker" => f.tracker = Some(take(args)?),
            "--agent" => f.agent = Some(take(args)?),
            "--agent-bin" => f.agent_bin = Some(take(args)?),
            "--sessions" => f.sessions = Some(take(args)?),
            "--workflow" => f.workflow = Some(take(args)?),
            "--suites" => f.suites = Some(take(args)?),
            "--base" => f.base = Some(take(args)?),
            "--landing" => f.landing = Some(take(args)?),
            "--alerts" => f.alerts = Some(take(args)?),
            "--ci-workflow" => f.ci_workflow = Some(take(args)?),
            _ => i += 1,
        }
    }
    Ok(f)
}

/// One question, for a person at a terminal.
#[derive(Clone, Debug)]
pub struct Question {
    pub id: &'static str,
    pub text: String,
    /// The answers on offer (empty: free text).
    pub options: Vec<String>,
    pub default: String,
    pub flag: &'static str,
}

#[derive(Clone, Debug)]
pub struct Answers {
    pub id: String,
    pub tracker: Tracker,
    pub agent: Agent,
    pub agent_bin: Option<String>,
    pub sessions: u32,
    pub workflow: Workflow,
    pub accept_suites: bool,
    pub base: String,
    pub landing: Landing,
    /// `None`: keep the built-in routing.
    pub alerts: Option<Vec<Alert>>,
    pub ci_workflow: bool,
    pub prove: bool,
    /// Said while resolving, for the report.
    pub notes: Vec<String>,
}

/// How a question gets its answer.
pub struct Mode<'a> {
    pub interactive: bool,
    pub ask: &'a mut dyn FnMut(&Question) -> Option<String>,
}

fn yes_no(s: &str, flag: &str) -> Result<bool, String> {
    match s {
        "yes" | "y" | "true" | "accept" => Ok(true),
        "no" | "n" | "false" | "skip" => Ok(false),
        o => Err(format!("{flag} takes yes or no, not {o:?}")),
    }
}

/// Answer every question in Part 21's order.
pub fn resolve(
    f: &Flags,
    d: &Detected,
    mode: &mut Mode,
    say: &mut dyn FnMut(&str),
) -> Result<Answers, String> {
    let mut notes = vec![];
    let mut answer = |flag: Option<&String>, q: Question| -> Result<String, String> {
        if let Some(v) = flag {
            return Ok(v.clone());
        }
        if f.yes {
            return Ok(q.default);
        }
        if mode.interactive {
            return Ok((mode.ask)(&q)
                .filter(|a| !a.trim().is_empty())
                .unwrap_or(q.default));
        }
        Err(format!(
            "this is not a terminal, so `{}` cannot be asked: pass {} VALUE, or --yes to take the recommended answer ({})",
            q.id, q.flag, q.default
        ))
    };
    let q = |id, text: &str, options: &[&str], default: &str, flag| Question {
        id,
        text: text.into(),
        options: options.iter().map(|s| s.to_string()).collect(),
        default: default.into(),
        flag,
    };

    // 2. Tracker.
    let tracker = match answer(
        f.tracker.as_ref(),
        q(
            "tracker",
            "Where do tasks come from?",
            &["local", "github"],
            "local",
            "--tracker",
        ),
    )?
    .as_str()
    {
        "local" => Tracker::Local,
        "github" => {
            if d.github().is_none() {
                return Err("--tracker github needs a GitHub remote, and this repository has none; use --tracker local".into());
            }
            Tracker::Github
        }
        o => return Err(format!("--tracker is local or github, not {o:?}")),
    };

    // 3. LLM.
    let found: Vec<&str> = d.agents.iter().map(|(a, _)| a.name()).collect();
    let first = found.first().copied().unwrap_or("claude");
    let agent = match answer(
        f.agent.as_ref(),
        q(
            "agent",
            "Which agent CLI?",
            &["claude", "codex"],
            first,
            "--agent",
        ),
    )?
    .as_str()
    {
        "claude" => Agent::Claude,
        "codex" => Agent::Codex,
        o => return Err(format!("--agent is claude or codex, not {o:?}")),
    };
    if f.agent_bin.is_none() && !d.agents.iter().any(|(a, _)| *a == agent) {
        notes.push(format!(
            "`{}` is not on PATH. Install it, or pass --agent-bin PATH; agent steps cannot run until then",
            agent.name()
        ));
    }
    let sessions: u32 = answer(
        f.sessions.as_ref(),
        q(
            "sessions",
            "How many agent sessions at once?",
            &[],
            "1",
            "--sessions",
        ),
    )?
    .parse()
    .ok()
    .filter(|n| (1..=64).contains(n))
    .ok_or("--sessions is a number from 1 to 64")?;

    // 4. Workflow.
    let names: Vec<&str> = Workflow::ALL.iter().map(|w| w.name()).collect();
    let wf = answer(
        f.workflow.as_ref(),
        q(
            "workflow",
            "Which example workflow?",
            &names,
            "solo-fast",
            "--workflow",
        ),
    )?;
    let workflow = Workflow::from_name(&wf)
        .ok_or_else(|| format!("--workflow is one of {}, not {wf:?}", names.join(", ")))?;
    say(&format!(
        "Workflow: {}, {}.",
        workflow.name(),
        workflow.blurb()
    ));
    say(STARTING_POINT);

    // 5. Tests.
    let accept_suites = yes_no(
        &answer(
            f.suites.as_ref(),
            q(
                "suites",
                "Run the proposed test suites once to confirm them?",
                &["accept", "skip"],
                "accept",
                "--suites",
            ),
        )?,
        "--suites",
    )
    .map_err(|_| "--suites is accept or skip".to_string())?;

    // 6. Landing.
    let base = answer(
        f.base.as_ref(),
        q(
            "base",
            "Which branch do finished tasks land on?",
            &[],
            &d.branch,
            "--base",
        ),
    )?;
    if base.is_empty() || base.starts_with('-') || base.contains(char::is_whitespace) {
        return Err(format!("--base {base:?} is not a plain branch name"));
    }
    let mut default_landing = workflow.default_landing();
    if default_landing.needs_host() && d.github().is_none() {
        default_landing = Landing::Direct;
        notes.push(format!(
            "{} lands by pull request, which needs a GitHub remote; none found, so landing is `direct`. Add a remote and run `mason init --force --landing pr` to change it",
            workflow.name()
        ));
    }
    let landing_in = answer(
        f.landing.as_ref(),
        q(
            "landing",
            "How do tasks land? (direct, direct-ci, pr, batched)",
            &["direct", "direct-ci", "pr", "batched"],
            default_landing.name(),
            "--landing",
        ),
    )?;
    let landing = Landing::from_name(&landing_in).ok_or_else(|| {
        format!("--landing is direct, direct-ci, pr or batched, not {landing_in:?}")
    })?;
    if landing.needs_host() && d.github().is_none() {
        return Err(format!("--landing {} opens pull requests, which needs a GitHub remote, and this repository has none; use --landing direct", landing.name()));
    }
    if landing == Landing::DirectCi && d.github().is_none() {
        return Err("--landing direct-ci waits for GitHub Actions, and this repository has no GitHub remote; use --landing direct".into());
    }

    // 7. Alerts.
    let alerts_in = answer(f.alerts.as_ref(), q("alerts", "Which alerts besides the monitor? (sound, speech, email, push; or none; empty keeps the defaults)", &[], "", "--alerts"))?;
    let alerts = match alerts_in.trim() {
        "" => None,
        "none" => Some(vec![]),
        list => Some(
            list.split(',')
                .map(|s| {
                    Alert::from_name(s.trim()).ok_or_else(|| {
                        format!("--alerts takes sound, speech, email, push or none, not {s:?}")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };

    // The CI trigger offer (only asked when it can matter).
    let ci_workflow = if landing == Landing::DirectCi {
        yes_no(
            &answer(
                f.ci_workflow.as_ref(),
                q(
                    "ci-workflow",
                    "Add a workflow that runs on mason/ci/** if CI does not already?",
                    &["yes", "no"],
                    "no",
                    "--ci-workflow",
                ),
            )?,
            "--ci-workflow",
        )?
    } else {
        f.ci_workflow
            .as_deref()
            .map(|v| yes_no(v, "--ci-workflow"))
            .transpose()?
            .unwrap_or(false)
    };

    // 8. Prove it.
    let prove = !f.no_prove;

    let id = f.id.clone().unwrap_or_else(|| d.id.clone());
    if !valid_project_id(&id) {
        return Err(format!(
            "project id {id:?} must be lowercase letters, digits, - and _; pass --id"
        ));
    }
    Ok(Answers {
        id,
        tracker,
        agent,
        agent_bin: f.agent_bin.clone(),
        sessions,
        workflow,
        accept_suites,
        base,
        landing,
        alerts,
        ci_workflow,
        prove,
        notes,
    })
}

// ---- rendering ------------------------------------------------------------------------------

/// The template's `mason.toml` with the suites filled in: `suites = [..]` on the test step and
/// one `[suite.*]` table each. Without suites the marker line is dropped and the test step runs
/// the overlay's `test` command.
pub fn render_mason_toml(workflow: Workflow, suites: &[SuiteSpec]) -> String {
    let t = workflow.mason_toml();
    let names: Vec<String> = suites.iter().map(|s| format!("{:?}", s.name)).collect();
    let mut out = String::new();
    for l in t.lines() {
        if l == SUITES_MARKER {
            if !names.is_empty() {
                out.push_str(&format!("suites = [{}]\n", names.join(", ")));
            }
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    for s in suites {
        out.push('\n');
        out.push_str(&s.toml());
    }
    out
}

/// A suite as proposed: what to run and how to read what it prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuiteSpec {
    pub name: String,
    pub command: String,
    /// A built-in parser name, `regex` included.
    pub parser: String,
    pub pass_regex: Option<String>,
    pub fail_regex: Option<String>,
    pub min_tests: u32,
}

impl SuiteSpec {
    pub fn toml(&self) -> String {
        // Quoted by toml_edit, so a command with a `'` in it (xcodebuild's
        // `-destination 'platform=macOS'`) or a regex with backslashes stays one string.
        let q = |v: &str| toml_edit::Value::from(v).to_string().trim().to_string();
        let mut s = format!(
            "[suite.{}]\ncommand = {}\nparser = {}\nmin_tests = {}\nlevel = \"unit\"\n",
            self.name,
            q(&self.command),
            q(&self.parser),
            self.min_tests
        );
        if let Some(p) = &self.pass_regex {
            s.push_str(&format!("pass_regex = {}\n", q(p)));
        }
        if let Some(p) = &self.fail_regex {
            s.push_str(&format!("fail_regex = {}\n", q(p)));
        }
        s
    }
}

/// The template's `landing.toml` with the chosen mode and base.
pub fn render_landing_toml(workflow: Workflow, landing: Landing, base: &str) -> String {
    let mut out = String::new();
    let mut wrote = false;
    for l in workflow.landing_toml().lines() {
        if l.starts_with("mode = ") && !wrote {
            out.push_str(&format!(
                "mode = \"{}\"\nbase = \"{base}\"\n",
                landing.mode()
            ));
            wrote = true;
        } else if l.starts_with("base = ") {
            continue;
        } else if (l.starts_with("window = ") || l.starts_with("max_size = "))
            && !matches!(landing, Landing::Batched)
        {
            continue; // batching is off
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// The project overlay `<config>/projects/<id>.toml`.
pub fn render_overlay(a: &Answers, d: &Detected, test_cmd: &str) -> String {
    let mut s = String::from(
        "# Written by `mason init`. Change anything, any time; `mason doctor` rechecks it.\n",
    );
    s.push_str(&format!(
        "path = {:?}\nbase = {:?}\n",
        d.root.display().to_string(),
        a.base
    ));
    if let Some(r) = &d.remote_name {
        s.push_str(&format!("remote = {r:?}\n"));
    }
    s.push_str(&format!("test = {test_cmd:?}\n"));
    if a.agent == Agent::Codex {
        s.push_str("provider = \"codex\"\n");
    }
    if let Some(b) = &a.agent_bin {
        match a.agent {
            Agent::Claude => s.push_str(&format!("claude_bin = {b:?}\n")),
            Agent::Codex => s.push_str(&format!("codex_bin = {b:?}\n")),
        }
    }
    let section = |name: &str, kind: &str, repo: &str, token: &str| {
        format!("\n[{name}]\nkind = \"{kind}\"\nrepo = {repo:?}\ntoken = \"env:{token}\"\n")
    };
    if let Some(r) = d.github() {
        if a.landing.needs_host() {
            s.push_str(&section("host", "github", &r.repo, "GITHUB_TOKEN"));
        }
        if a.landing == Landing::DirectCi {
            s.push_str(&section("ci", "github-actions", &r.repo, "GITHUB_TOKEN"));
        }
        if a.tracker == Tracker::Github {
            s.push_str(&section("tracker", "github", &r.repo, "GITHUB_TOKEN"));
        }
    }
    s
}

/// `notify.toml` routing for the conditions that need a person, with the channels asked for.
/// `email` and `push` are routed only when their sections exist, which `mason init` cannot
/// write for the user (they hold addresses and secrets), so they are left out and named.
pub fn render_notify(channels: &[Alert]) -> String {
    let mut s = String::from("# Written by `mason init`. Email and push need an [email] or [ntfy] section; see `mason notify`.\n");
    for kind in ["needs_you", "failed", "landing"] {
        s.push_str(&format!("\n[routing.{kind}]\nmonitor = 0\n"));
        for c in channels
            .iter()
            .filter(|c| matches!(c, Alert::Sound | Alert::Speech))
        {
            s.push_str(&format!("{} = 0\n", c.name()));
        }
    }
    s
}

/// A workflow that runs the tests on the candidate ref the pipeline pushes for CI.
pub fn render_ci_workflow(test_cmd: &str) -> String {
    format!(
        "# SPDX-License-Identifier: MIT\n# Added by `mason init`: MASON pushes a candidate to mason/ci/** and waits for this.\nname: mason-ci\non:\n  push:\n    branches: [\"mason/ci/**\"]\n  workflow_dispatch:\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - run: {test_cmd}\n"
    )
}

// ---- the Filing section -----------------------------------------------------------------

fn filing_heading() -> &'static str {
    AGENT_NOTES.lines().next().unwrap_or_default()
}

/// `existing` with the Filing section appended, or `None` if it is already there.
pub fn with_filing(existing: &str) -> Option<String> {
    if existing.lines().any(|l| l.trim() == filing_heading()) {
        return None;
    }
    let mut out = existing.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(AGENT_NOTES);
    Some(out)
}

/// `existing` without the section, if the section is there exactly as `init` wrote it.
/// `None` if it is absent or has been edited (then it is the owner's text, not ours).
pub fn without_filing(existing: &str) -> Option<String> {
    let at = existing.find(AGENT_NOTES)?;
    let before = existing[..at].trim_end_matches('\n');
    let after = existing[at + AGENT_NOTES.len()..].trim_start_matches('\n');
    Some(match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => after.to_string(),
        (false, true) => format!("{before}\n"),
        (false, false) => format!("{before}\n\n{after}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suite_with_quotes_and_backslashes_writes_toml_that_reads_back() {
        let s = SuiteSpec {
            name: "swift".into(),
            command: "cd 'My App' && xcodebuild test -destination 'platform=macOS' 2>&1".into(),
            parser: "regex".into(),
            pass_regex: Some(r"^ok (\S+) 'x'$".into()),
            fail_regex: Some(r#"^FAIL "(\S+)"$"#.into()),
            min_tests: 1,
        };
        let doc: toml_edit::DocumentMut = s.toml().parse().expect("valid TOML");
        let t = &doc["suite"]["swift"];
        assert_eq!(t["command"].as_str(), Some(s.command.as_str()));
        assert_eq!(t["pass_regex"].as_str(), s.pass_regex.as_deref());
        assert_eq!(t["fail_regex"].as_str(), s.fail_regex.as_deref());
        assert_eq!(t["parser"].as_str(), Some("regex"));
    }

    fn detected() -> Detected {
        Detected {
            root: "/r".into(),
            id: "r".into(),
            remote_name: Some("origin".into()),
            remote: parse_remote("git@github.com:o/r.git"),
            branch: "main".into(),
            ..Default::default()
        }
    }

    fn run(
        f: &Flags,
        d: &Detected,
        interactive: bool,
    ) -> (Result<Answers, String>, Vec<&'static str>) {
        let mut asked = vec![];
        let mut said: Vec<String> = vec![];
        let r = {
            let mut ask = |q: &Question| {
                asked.push(q.id);
                None
            };
            resolve(
                f,
                d,
                &mut Mode {
                    interactive,
                    ask: &mut ask,
                },
                &mut |t| said.push(t.to_string()),
            )
        };
        if r.is_ok() {
            assert!(
                said.iter().any(|t| t == STARTING_POINT),
                "the starting-point sentence is said at the workflow step"
            );
        }
        (r, asked)
    }

    #[test]
    fn remotes_parse_in_every_common_shape() {
        for u in [
            "git@github.com:o/r.git",
            "https://github.com/o/r",
            "https://x@github.com/o/r.git/",
            "ssh://git@github.com/o/r.git",
        ] {
            assert_eq!(
                parse_remote(u),
                Some(Remote {
                    host: "github.com".into(),
                    repo: "o/r".into()
                }),
                "{u}"
            );
        }
        assert_eq!(parse_remote("/srv/git/r.git"), None);
        assert_eq!(parse_remote("").map(|r| r.host), None);
    }

    #[test]
    fn yes_takes_the_recommended_answers_and_asks_nothing() {
        let (r, asked) = run(
            &Flags {
                yes: true,
                ..Default::default()
            },
            &detected(),
            true,
        );
        let a = r.unwrap();
        assert!(asked.is_empty());
        assert_eq!(
            (a.tracker, a.workflow, a.landing, a.sessions),
            (Tracker::Local, Workflow::SoloFast, Landing::Direct, 1)
        );
        assert!(a.accept_suites && a.prove && a.alerts.is_none());
        assert_eq!(a.base, "main");
    }

    #[test]
    fn a_terminal_is_asked_in_part_21s_order_and_only_what_flags_leave() {
        let (r, asked) = run(&Flags::default(), &detected(), true);
        r.unwrap();
        assert_eq!(
            asked,
            ["tracker", "agent", "sessions", "workflow", "suites", "base", "landing", "alerts"]
        );
        let f = Flags {
            tracker: Some("local".into()),
            workflow: Some("solo".into()),
            landing: Some("direct".into()),
            alerts: Some("none".into()),
            ..Default::default()
        };
        let (r, asked) = run(&f, &detected(), true);
        r.unwrap();
        assert_eq!(asked, ["agent", "sessions", "suites", "base"]);
    }

    #[test]
    fn without_a_terminal_or_yes_the_run_stops_and_names_the_flag() {
        let (r, asked) = run(&Flags::default(), &detected(), false);
        let e = r.unwrap_err();
        assert!(
            asked.is_empty() && e.contains("--tracker") && e.contains("--yes"),
            "{e}"
        );
    }

    #[test]
    fn each_flag_is_read_and_each_bad_value_is_refused() {
        let ok = |f: Flags| run(&Flags { yes: true, ..f }, &detected(), false).0;
        assert_eq!(
            ok(Flags {
                tracker: Some("github".into()),
                ..Default::default()
            })
            .unwrap()
            .tracker,
            Tracker::Github
        );
        assert_eq!(
            ok(Flags {
                agent: Some("codex".into()),
                ..Default::default()
            })
            .unwrap()
            .agent,
            Agent::Codex
        );
        assert_eq!(
            ok(Flags {
                sessions: Some("3".into()),
                ..Default::default()
            })
            .unwrap()
            .sessions,
            3
        );
        assert_eq!(
            ok(Flags {
                workflow: Some("human-gated".into()),
                ..Default::default()
            })
            .unwrap()
            .workflow,
            Workflow::HumanGated
        );
        assert!(
            !ok(Flags {
                suites: Some("skip".into()),
                ..Default::default()
            })
            .unwrap()
            .accept_suites
        );
        assert_eq!(
            ok(Flags {
                base: Some("develop".into()),
                ..Default::default()
            })
            .unwrap()
            .base,
            "develop"
        );
        assert_eq!(
            ok(Flags {
                landing: Some("batched".into()),
                ..Default::default()
            })
            .unwrap()
            .landing,
            Landing::Batched
        );
        assert_eq!(
            ok(Flags {
                alerts: Some("sound,speech".into()),
                ..Default::default()
            })
            .unwrap()
            .alerts,
            Some(vec![Alert::Sound, Alert::Speech])
        );
        assert_eq!(
            ok(Flags {
                alerts: Some("none".into()),
                ..Default::default()
            })
            .unwrap()
            .alerts,
            Some(vec![])
        );
        assert!(
            !run(
                &Flags {
                    yes: true,
                    no_prove: true,
                    ..Default::default()
                },
                &detected(),
                false
            )
            .0
            .unwrap()
            .prove
        );
        for (f, want) in [
            (
                Flags {
                    tracker: Some("jira".into()),
                    ..Default::default()
                },
                "--tracker",
            ),
            (
                Flags {
                    agent: Some("gpt".into()),
                    ..Default::default()
                },
                "--agent",
            ),
            (
                Flags {
                    sessions: Some("0".into()),
                    ..Default::default()
                },
                "--sessions",
            ),
            (
                Flags {
                    sessions: Some("many".into()),
                    ..Default::default()
                },
                "--sessions",
            ),
            (
                Flags {
                    workflow: Some("fancy".into()),
                    ..Default::default()
                },
                "--workflow",
            ),
            (
                Flags {
                    suites: Some("maybe".into()),
                    ..Default::default()
                },
                "--suites",
            ),
            (
                Flags {
                    base: Some("a b".into()),
                    ..Default::default()
                },
                "--base",
            ),
            (
                Flags {
                    landing: Some("yolo".into()),
                    ..Default::default()
                },
                "--landing",
            ),
            (
                Flags {
                    alerts: Some("carrier-pigeon".into()),
                    ..Default::default()
                },
                "--alerts",
            ),
            (
                Flags {
                    id: Some("Bad Id".into()),
                    ..Default::default()
                },
                "project id",
            ),
        ] {
            let e = ok(f).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn github_things_are_refused_without_a_github_remote() {
        let d = Detected {
            remote: None,
            remote_name: None,
            ..detected()
        };
        let ok = |f: Flags| run(&Flags { yes: true, ..f }, &d, false).0;
        for f in [
            Flags {
                tracker: Some("github".into()),
                ..Default::default()
            },
            Flags {
                landing: Some("pr".into()),
                ..Default::default()
            },
            Flags {
                landing: Some("batched".into()),
                ..Default::default()
            },
            Flags {
                landing: Some("direct-ci".into()),
                ..Default::default()
            },
        ] {
            assert!(ok(f).unwrap_err().contains("GitHub remote"));
        }
        // A workflow that would want a PR falls back to direct, and says so.
        let a = ok(Flags {
            workflow: Some("reviewed-pr".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(a.landing, Landing::Direct);
        assert!(a.notes.iter().any(|n| n.contains("GitHub remote")));
    }

    #[test]
    fn flags_are_taken_from_the_arguments_and_the_rest_is_left() {
        let mut args: Vec<String> = [
            "--workflow",
            "solo",
            "--yes",
            "stray",
            "--sessions",
            "2",
            "--no-prove",
        ]
        .map(String::from)
        .to_vec();
        let f = parse_flags(&mut args).unwrap();
        assert!(
            f.yes
                && f.no_prove
                && f.workflow.as_deref() == Some("solo")
                && f.sessions.as_deref() == Some("2")
        );
        assert_eq!(args, ["stray"]);
        assert!(parse_flags(&mut vec!["--base".to_string()])
            .unwrap_err()
            .contains("needs a value"));
    }

    #[test]
    fn every_template_renders_with_and_without_suites() {
        let suite = SuiteSpec {
            name: "unit".into(),
            command: "cargo test 2>&1".into(),
            parser: "regex".into(),
            pass_regex: Some(r"^test (.+) \.\.\. ok$".into()),
            fail_regex: None,
            min_tests: 1,
        };
        for w in Workflow::ALL {
            let without = render_mason_toml(w, &[]);
            assert!(
                !without.contains("mason init fills") && !without.contains("suites ="),
                "{}",
                w.name()
            );
            let with = render_mason_toml(w, std::slice::from_ref(&suite));
            assert!(
                with.contains("suites = [\"unit\"]") && with.contains("[suite.unit]"),
                "{}",
                w.name()
            );
            assert!(
                mason_definition::parse(&with).is_ok(),
                "{}: {:?}",
                w.name(),
                mason_definition::parse(&with).err()
            );
            assert!(mason_definition::parse(&without).is_ok());
            for l in [
                Landing::Direct,
                Landing::Pr,
                Landing::Batched,
                Landing::DirectCi,
            ] {
                let t = render_landing_toml(w, l, "develop");
                let c = mason_landing::config::parse(&t)
                    .unwrap_or_else(|e| panic!("{} {}: {e:?}", w.name(), l.name()));
                assert_eq!(c.mode.name(), l.mode());
                assert_eq!(c.base.as_deref(), Some("develop"));
            }
        }
    }

    #[test]
    fn the_overlay_has_what_the_chosen_landing_needs() {
        let d = detected();
        let a = |landing, tracker| Answers {
            id: "r".into(),
            tracker,
            agent: Agent::Claude,
            agent_bin: Some("/x/claude".into()),
            sessions: 1,
            workflow: Workflow::SoloFast,
            accept_suites: true,
            base: "main".into(),
            landing,
            alerts: None,
            ci_workflow: false,
            prove: true,
            notes: vec![],
        };
        let direct = render_overlay(&a(Landing::Direct, Tracker::Local), &d, "true");
        assert!(
            direct.contains("remote = \"origin\"")
                && direct.contains("claude_bin = \"/x/claude\"")
                && !direct.contains("[host]")
        );
        let pr = render_overlay(&a(Landing::Pr, Tracker::Github), &d, "true");
        assert!(
            pr.contains("[host]")
                && pr.contains("[tracker]")
                && pr.contains("token = \"env:GITHUB_TOKEN\"")
        );
        assert!(render_overlay(&a(Landing::DirectCi, Tracker::Local), &d, "true").contains("[ci]"));
        // Parses as a project, and a token is only ever a reference.
        let p = mason_daemon::config::parse_project("r", &pr).unwrap();
        assert!(p.services.host.is_some() && p.services.tracker.is_some());
    }

    #[test]
    fn notify_routing_parses() {
        let t = render_notify(&[Alert::Sound, Alert::Email]);
        assert!(mason_daemon::notify::parse(&t).is_ok());
        assert!(t.contains("sound = 0") && !t.contains("email = 0"));
    }

    #[test]
    fn the_filing_section_is_added_once_and_removed_only_when_untouched() {
        let once = with_filing("# Notes\n\nHello.\n").unwrap();
        assert!(
            once.starts_with("# Notes\n\nHello.\n\n## Filing work")
                && once.contains("explored: true")
        );
        assert_eq!(with_filing(&once), None);
        assert_eq!(without_filing(&once).unwrap(), "# Notes\n\nHello.\n");
        let fresh = with_filing("").unwrap();
        assert_eq!(fresh, AGENT_NOTES);
        assert_eq!(without_filing(&fresh).unwrap(), "");
        // Edited: ours no more.
        assert_eq!(
            without_filing(&once.replace("explored: true", "explored: yes")),
            None
        );
        // Text after it survives.
        let mid = format!("{}\n## Other\n\nx\n", with_filing("# A\n").unwrap());
        assert_eq!(without_filing(&mid).unwrap(), "# A\n\n## Other\n\nx\n");
    }
}
