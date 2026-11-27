// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason prompts eval` (task 1139): replay the read-only steps of past tasks (a review, a
//! diagnosis, an exploration) under the prompt of a base revision and of a candidate revision,
//! and have a grader session compare each result with what the case expects.
//!
//! The result is information for the owner. No gate reads it, and it never runs in the test
//! step or in landing. Cases are files under `.mason/evals/<step>/` on the base revision, so they
//! change only with the owner's approval (`.mason/**` is protected).

use crate::config::ProjectCfg;
use crate::prompts::{self, PackVars};
use mason_adapter_api::contract::{ContractHandle, NoChecks, StepKind};
use mason_adapter_api::{AgentAdapter, AgentEvent, SessionSpec};
use mason_core::TrustSource;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use toml_edit::{value, Array, DocumentMut};

/// Where a repository keeps its cases.
pub const DIR: &str = ".mason/evals";

/// The steps a case can replay: the read-only ones.
pub const STEPS: [&str; 3] = ["review", "diagnose", "explore"];

/// One recorded step of a past task and what a good result says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    /// The path the case was read from (relative to the repository), or will be written to.
    pub file: String,
    pub task: u64,
    pub project: String,
    /// The commit the task's branch was at: the snapshot the sessions read.
    pub head: String,
    pub step: String,
    pub title: String,
    pub body: String,
    pub plan: Option<String>,
    /// Diagnose: why the task failed.
    pub reason: Option<String>,
    /// Diagnose: the end of the test logs.
    pub logs: Option<String>,
    /// Plain statements the result is judged against.
    pub expect: Vec<String>,
}

impl Case {
    pub fn to_toml(&self) -> String {
        let mut doc = DocumentMut::new();
        doc["task"] = value(self.task as i64);
        doc["project"] = value(&self.project);
        doc["head"] = value(&self.head);
        doc["step"] = value(&self.step);
        doc["title"] = value(&self.title);
        doc["body"] = value(&self.body);
        for (k, v) in [
            ("plan", &self.plan),
            ("reason", &self.reason),
            ("logs", &self.logs),
        ] {
            if let Some(v) = v {
                doc[k] = value(v);
            }
        }
        let mut a = Array::new();
        for e in &self.expect {
            a.push(e.as_str());
        }
        doc["expect"] = value(a);
        format!("# SPDX-License-Identifier: GPL-3.0-or-later\n{doc}")
    }

    pub fn parse(file: &str, text: &str) -> Result<Case, String> {
        let doc: DocumentMut = text.parse().map_err(|e| format!("{file}: not TOML: {e}"))?;
        let s = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(String::from);
        let need = |k: &str| {
            s(k).filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("{file}: `{k}` is required"))
        };
        let step = need("step")?;
        if !STEPS.contains(&step.as_str()) {
            return Err(format!(
                "{file}: step {step:?} must be one of review, diagnose, explore"
            ));
        }
        let task = doc
            .get("task")
            .and_then(|v| v.as_integer())
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("{file}: `task` is required"))? as u64;
        let expect: Vec<String> = doc
            .get("expect")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if expect.is_empty() {
            return Err(format!("{file}: `expect` needs at least one statement"));
        }
        Ok(Case {
            file: file.to_string(),
            task,
            project: need("project")?,
            head: need("head")?,
            step,
            title: need("title")?,
            body: s("body").unwrap_or_default(),
            plan: s("plan"),
            reason: s("reason"),
            logs: s("logs"),
            expect,
        })
    }

    /// `.mason/evals/<step>/<task>.toml`
    pub fn path(&self) -> String {
        format!("{DIR}/{}/{}.toml", self.step, self.task)
    }
}

/// Write a case into `repo`'s working tree. An existing case for the task is not overwritten.
pub fn write_case(repo: &Path, case: &Case) -> Result<PathBuf, String> {
    let rel = case.path();
    let path = repo.join(&rel);
    if path.exists() {
        return Err(format!("{rel} exists already"));
    }
    std::fs::create_dir_all(path.parent().unwrap_or(repo)).map_err(|e| e.to_string())?;
    std::fs::write(&path, case.to_toml()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The cases on `rev`: every `.mason/evals/<step>/*.toml`, narrowed to a step and a project.
/// Files that do not parse are returned as the second list, with why.
pub fn load_cases(
    repo: &Path,
    rev: &str,
    step: Option<&str>,
    project: Option<&str>,
) -> (Vec<Case>, Vec<String>) {
    let mut cases = vec![];
    let mut bad = vec![];
    let sha = match mason_landing::git::rev_parse(repo, rev) {
        Ok(s) => s,
        Err(e) => return (cases, vec![format!("{rev}: not a revision ({e})")]),
    };
    let listing = mason_landing::git::run(repo, &["ls-tree", "-r", "--name-only", &sha, "--", DIR])
        .unwrap_or_default();
    for f in listing.lines().filter(|f| f.ends_with(".toml")) {
        let text = match mason_landing::git::run(repo, &["show", &format!("{sha}:{f}")]) {
            Ok(t) => t,
            Err(e) => {
                bad.push(format!("{f}: {e}"));
                continue;
            }
        };
        match Case::parse(f, &text) {
            Ok(c) if step.is_none_or(|s| s == c.step) && project.is_none_or(|p| p == c.project) => {
                cases.push(c)
            }
            Ok(_) => {}
            Err(e) => bad.push(e),
        }
    }
    cases.sort_by(|a, b| a.file.cmp(&b.file));
    (cases, bad)
}

/// The prompt of `step` for `case` under the pack on `rev` (if it has `.mason/prompts/<step>.md`)
/// or else the built-in default of the running binary. The default text of another revision needs
/// that revision's own binary: see `--base-bin` and `--candidate-bin`.
pub fn render(case: &Case, p: &ProjectCfg, rev: &str) -> Result<String, String> {
    let plan: Option<Value> = case
        .plan
        .as_ref()
        .and_then(|t| serde_json::from_str(t).ok());
    let pack = mason_landing::git::rev_parse(&p.path, rev)
        .ok()
        .and_then(|sha| {
            mason_landing::git::run(
                &p.path,
                &["show", &format!("{sha}:.mason/prompts/{}.md", case.step)],
            )
            .ok()
        })
        .filter(|t| !t.trim().is_empty());
    if let Some(pack) = pack {
        return Ok(prompts::from_pack(
            &pack,
            &PackVars {
                number: case.task,
                title: &case.title,
                body: &case.body,
                trust: TrustSource::Owner,
                project: p,
                plan: plan.as_ref(),
                feedback: None,
                tried: None,
            },
        ));
    }
    Ok(match case.step.as_str() {
        "review" => prompts::review_prompt(
            case.task,
            &case.title,
            &case.body,
            TrustSource::Owner,
            p,
            plan.as_ref(),
        ),
        "explore" => {
            prompts::explore_prompt(case.task, &case.title, &case.body, TrustSource::Owner, p)
        }
        "diagnose" => {
            let diffstat = mason_landing::git::run(
                &p.path,
                &[
                    "diff",
                    "--stat",
                    &format!("{}...{}", p.base_ref(), case.head),
                ],
            )
            .unwrap_or_default();
            crate::diagnose::prompt(
                case.task,
                &case.title,
                &case.body,
                plan.as_ref(),
                case.reason.as_deref().unwrap_or("(not recorded)"),
                "",
                None,
                case.logs.as_deref().unwrap_or("(not recorded)"),
                &diffstat,
                &["exploring", "developing", "testing", "review"].map(String::from),
            )
        }
        other => return Err(format!("no prompt for step {other}")),
    })
}

/// What one session came to.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    /// `approve`/`changes` for a review, the recommended action for a diagnosis, `plan` for an
    /// exploration, or `none`.
    pub verdict: String,
    /// The submission, as lines the grader and the owner read.
    pub text: String,
    pub tokens: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Judgement {
    pub pass: bool,
    pub judgement: String,
    pub quotes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaseResult {
    pub file: String,
    pub task: u64,
    pub step: String,
    /// Why the case was not run.
    pub skipped: Option<String>,
    pub base: Option<Side>,
    pub candidate: Option<Side>,
    pub base_judgement: Option<Judgement>,
    pub candidate_judgement: Option<Judgement>,
    pub weak_assertions: Vec<String>,
    pub grader_tokens: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub base: String,
    pub candidate: String,
    pub cases: Vec<CaseResult>,
}

impl Report {
    pub fn base_passes(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.base_judgement.as_ref().is_some_and(|j| j.pass))
            .count()
    }
    pub fn candidate_passes(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.candidate_judgement.as_ref().is_some_and(|j| j.pass))
            .count()
    }
    fn judged(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.base_judgement.is_some() && c.candidate_judgement.is_some())
            .count()
    }
    /// `base passes 2/3, candidate passes 3/3 (1 skipped)`
    pub fn summary(&self) -> String {
        let n = self.judged();
        let skipped = self.cases.iter().filter(|c| c.skipped.is_some()).count();
        let failed = self.cases.len() - n - skipped;
        let mut s = format!(
            "base passes {}/{n}, candidate passes {}/{n}",
            self.base_passes(),
            self.candidate_passes()
        );
        if skipped > 0 {
            s.push_str(&format!(" ({skipped} skipped)"));
        }
        if failed > 0 {
            s.push_str(&format!(" ({failed} not judged)"));
        }
        s
    }

    pub fn to_json(&self) -> Value {
        let side = |s: &Option<Side>| {
            s.as_ref().map_or(Value::Null, |s| {
                json!({"verdict": s.verdict, "text": s.text, "tokens": s.tokens, "error": s.error})
            })
        };
        let judge = |j: &Option<Judgement>| {
            j.as_ref().map_or(
                Value::Null,
                |j| json!({"pass": j.pass, "judgement": j.judgement, "quotes": j.quotes}),
            )
        };
        json!({
            "base": self.base,
            "candidate": self.candidate,
            "summary": self.summary(),
            "base_passes": self.base_passes(),
            "candidate_passes": self.candidate_passes(),
            "cases": self.cases.iter().map(|c| json!({
                "file": c.file, "task": c.task, "step": c.step, "skipped": c.skipped,
                "base": side(&c.base), "candidate": side(&c.candidate),
                "base_judgement": judge(&c.base_judgement),
                "candidate_judgement": judge(&c.candidate_judgement),
                "weak_assertions": c.weak_assertions,
                "grader_tokens": c.grader_tokens, "error": c.error,
            })).collect::<Vec<_>>(),
        })
    }

    /// The lines `mason prompts eval` prints: per case both verdicts and the judgement, then
    /// the summary line last.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for c in &self.cases {
            out.push_str(&format!("#{} {} ({})\n", c.task, c.step, c.file));
            if let Some(why) = &c.skipped {
                out.push_str(&format!("  skipped: {why}\n"));
                continue;
            }
            for (label, s, j) in [
                ("base", &c.base, &c.base_judgement),
                ("candidate", &c.candidate, &c.candidate_judgement),
            ] {
                let Some(s) = s else { continue };
                out.push_str(&format!(
                    "  {label}: verdict {} ({} tokens){}\n",
                    s.verdict,
                    s.tokens,
                    s.error.as_ref().map_or(String::new(), |e| format!(", {e}"))
                ));
                if let Some(j) = j {
                    out.push_str(&format!(
                        "    {}: {}\n",
                        if j.pass { "pass" } else { "fail" },
                        j.judgement
                    ));
                    for q in &j.quotes {
                        out.push_str(&format!("      > {q}\n"));
                    }
                }
            }
            for w in &c.weak_assertions {
                out.push_str(&format!("  weak expectation: {w}\n"));
            }
            if let Some(e) = &c.error {
                out.push_str(&format!("  not judged: {e}\n"));
            }
            if c.grader_tokens > 0 {
                out.push_str(&format!("  grader: {} tokens\n", c.grader_tokens));
            }
        }
        out.push_str(&self.summary());
        out.push('\n');
        out
    }
}

/// How a prompt is made for a case at a revision.
pub type Renderer<'a> = &'a dyn Fn(&Case, &str) -> Result<String, String>;

fn submission_text(step: &str, a: &Value) -> (String, String) {
    match step {
        "review" => {
            let mut t = format!(
                "verdict {}\n{}\n",
                a["verdict"].as_str().unwrap_or("?"),
                a["summary"].as_str().unwrap_or("")
            );
            for f in a["findings"].as_array().into_iter().flatten() {
                t.push_str(&format!(
                    "- [{}] {} ({})\n",
                    f["severity"].as_str().unwrap_or("?"),
                    f["issue"].as_str().unwrap_or(""),
                    f["file"].as_str().unwrap_or("")
                ));
            }
            (a["verdict"].as_str().unwrap_or("none").to_string(), t)
        }
        "diagnose" => {
            let (verdict, _) = crate::diagnose::recommended(a);
            let mut t = format!(
                "recommended {verdict}\n{}\n",
                a["summary"].as_str().unwrap_or("")
            );
            if a["no_code_change"].as_bool() == Some(true) {
                t.push_str("no_code_change: true\n");
            }
            for o in a["options"].as_array().into_iter().flatten() {
                t.push_str(&format!(
                    "- {}\n",
                    o.to_string().chars().take(400).collect::<String>()
                ));
            }
            (verdict, t)
        }
        _ => ("plan".to_string(), a.to_string()),
    }
}

/// Drive a session to its end: the contract's accepted submission, all the text said, and the
/// tokens the stream reported.
fn drive(
    adapter: &dyn AgentAdapter,
    mut spec: SessionSpec,
    contract: Option<&ContractHandle>,
) -> Result<(Option<Value>, String, u64), String> {
    if let Some(c) = contract {
        spec.accepted = c.flag();
        spec.contract = Some(c.clone());
    }
    let mut h = adapter
        .start(spec)
        .map_err(|e| format!("the session could not be started: {e}"))?;
    let (mut text, mut tokens) = (String::new(), 0u64);
    while let Some(ev) = h.next_event() {
        match ev {
            AgentEvent::Text(t) => {
                text.push_str(&t);
                text.push('\n');
            }
            AgentEvent::Tokens(t) => {
                tokens = t.input.unwrap_or(0) + t.output.unwrap_or(0);
            }
            AgentEvent::Finished(_) => break,
            _ => {}
        }
    }
    let accepted = contract.and_then(|c| c.state().accepted).map(|(_, a)| a);
    Ok((accepted, text, tokens))
}

fn kind_of(step: &str) -> StepKind {
    match step {
        "diagnose" => StepKind::Diagnose,
        "explore" => StepKind::Explore,
        _ => StepKind::Review,
    }
}

fn session(
    adapter: &dyn AgentAdapter,
    p: &ProjectCfg,
    step: &str,
    prompt: &str,
    wt: &Path,
    sock: &Path,
) -> Side {
    let fail = |e: String| Side {
        verdict: "none".into(),
        text: String::new(),
        tokens: 0,
        error: Some(e),
    };
    let mut spec = if step == "diagnose" {
        let eff = crate::tiermap::effective(&p.config_dir, p.agent().provider, &p.overlay_text);
        crate::diagnose::session_spec(prompt, wt, p, &eff).0
    } else {
        let mut s = SessionSpec::new(kind_of(step), prompt, wt);
        s.model = p.model.clone();
        s
    };
    spec.env = p.agent_env.clone();
    spec.env.retain(|(k, _)| k != "MASON_TASK");
    spec.tool_timeout = Duration::from_secs(120);
    spec.deadline = Some(Duration::from_secs(900));
    let contract = ContractHandle::new(kind_of(step), Box::new(NoChecks));
    if let Err(e) = contract.serve(sock) {
        return fail(format!("the contract socket could not be made: {e}"));
    }
    let r = drive(adapter, spec, Some(&contract));
    contract.shutdown();
    match r {
        Err(e) => fail(e),
        Ok((Some(a), _, tokens)) => {
            let (verdict, text) = submission_text(step, &a);
            Side {
                verdict,
                text,
                tokens,
                error: None,
            }
        }
        Ok((None, _, tokens)) => Side {
            verdict: "none".into(),
            text: String::new(),
            tokens,
            error: Some("the session ended without a submission".into()),
        },
    }
}

/// The grader's prompt: the case's expectations and both results.
pub fn grader_prompt(case: &Case, base: &Side, cand: &Side) -> String {
    let expect = case
        .expect
        .iter()
        .map(|e| format!("- {e}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are grading two results of the same {step} of task #{task} ({title}) against what a \
         good result says. Do not run any command and do not change any file.\n\n\
         # Expected outcome\n{expect}\n\n\
         # Result A (base prompt)\n{a}\n\n# Result B (candidate prompt)\n{b}\n\n\
         Judge each result on its own: it passes only if every expected statement holds. Quote the \
         words of the result your judgement rests on. Also name any expected statement too vague to \
         tell a good result from a bad one.\n\
         End with one JSON object and nothing after it: \
         {{\"base\": {{\"pass\": true|false, \"judgement\": \"...\", \"quotes\": [\"...\"]}}, \
         \"candidate\": {{\"pass\": true|false, \"judgement\": \"...\", \"quotes\": [\"...\"]}}, \
         \"weak_assertions\": [\"...\"]}}",
        step = case.step,
        task = case.task,
        title = case.title,
        a = if base.error.is_some() { format!("(no result: {})", base.error.clone().unwrap_or_default()) } else { base.text.clone() },
        b = if cand.error.is_some() { format!("(no result: {})", cand.error.clone().unwrap_or_default()) } else { cand.text.clone() },
    )
}

/// Read the grader's closing JSON object out of what it said.
pub fn parse_grade(text: &str) -> Result<(Judgement, Judgement, Vec<String>), String> {
    let (i, j) = (text.find('{'), text.rfind('}'));
    let (Some(i), Some(j)) = (i, j) else {
        return Err("the grader gave no JSON object".into());
    };
    let v: Value = serde_json::from_str(&text[i..=j])
        .map_err(|e| format!("the grader's JSON is invalid: {e}"))?;
    let one = |k: &str| -> Result<Judgement, String> {
        let o = &v[k];
        let pass = o["pass"]
            .as_bool()
            .ok_or_else(|| format!("the grader gave no pass for {k}"))?;
        Ok(Judgement {
            pass,
            judgement: o["judgement"].as_str().unwrap_or("").to_string(),
            quotes: o["quotes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|q| q.as_str().map(String::from))
                .collect(),
        })
    };
    let weak = v["weak_assertions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| q.as_str().map(String::from))
        .collect();
    Ok((one("base")?, one("candidate")?, weak))
}

/// Replay `cases` under the prompts of `base_rev` and `cand_rev`. Each case gets a detached
/// worktree at its recorded head, in `scratch`, that both sessions read; a case whose head is
/// gone is skipped with a reason. Cases run one after the other.
pub fn run(
    adapter: &dyn AgentAdapter,
    p: &ProjectCfg,
    cases: &[Case],
    base_rev: &str,
    cand_rev: &str,
    renderer: Renderer,
    scratch: &Path,
) -> Report {
    let sha = |r: &str| mason_landing::git::rev_parse(&p.path, r).unwrap_or_else(|_| r.to_string());
    let mut report = Report {
        base: sha(base_rev),
        candidate: sha(cand_rev),
        cases: vec![],
    };
    for (i, c) in cases.iter().enumerate() {
        let mut r = CaseResult {
            file: c.file.clone(),
            task: c.task,
            step: c.step.clone(),
            skipped: None,
            base: None,
            candidate: None,
            base_judgement: None,
            candidate_judgement: None,
            weak_assertions: vec![],
            grader_tokens: 0,
            error: None,
        };
        if mason_landing::git::rev_parse(&p.path, &c.head).is_err() {
            r.skipped = Some(format!(
                "the head commit {} is gone from the repository",
                c.head
            ));
            report.cases.push(r);
            continue;
        }
        let wt = scratch.join(format!("wt{i}"));
        let wts = wt.to_string_lossy().to_string();
        if let Err(e) =
            mason_landing::git::run(&p.path, &["worktree", "add", "--detach", &wts, &c.head])
        {
            r.skipped = Some(format!("the snapshot could not be made: {e}"));
            report.cases.push(r);
            continue;
        }
        let sock = scratch.join(format!("s{i}.sock"));
        let mut sides = vec![];
        for rev in [base_rev, cand_rev] {
            sides.push(match renderer(c, rev) {
                Ok(prompt) => session(adapter, p, &c.step, &prompt, &wt, &sock),
                Err(e) => Side {
                    verdict: "none".into(),
                    text: String::new(),
                    tokens: 0,
                    error: Some(format!("no prompt at {rev}: {e}")),
                },
            });
        }
        mason_landing::git::remove_worktree(&p.path, &wt);
        let (b, k) = (sides.remove(0), sides.remove(0));
        // The grader reads nothing from the repository: an empty directory.
        let gdir = scratch.join(format!("g{i}"));
        let _ = std::fs::create_dir_all(&gdir);
        let mut gspec = SessionSpec::new(StepKind::Chat, &grader_prompt(c, &b, &k), &gdir);
        gspec.env = p.agent_env.clone();
        gspec.env.retain(|(k, _)| k != "MASON_TASK");
        gspec.deadline = Some(Duration::from_secs(600));
        match drive(adapter, gspec, None) {
            Ok((_, said, tokens)) => {
                r.grader_tokens = tokens;
                match parse_grade(&said) {
                    Ok((bj, kj, weak)) => {
                        r.base_judgement = Some(bj);
                        r.candidate_judgement = Some(kj);
                        r.weak_assertions = weak;
                    }
                    Err(e) => r.error = Some(e),
                }
            }
            Err(e) => r.error = Some(e),
        }
        r.base = Some(b);
        r.candidate = Some(k);
        report.cases.push(r);
    }
    report
}

/// Is this path one a prompt change touches: `prompts.rs`, the tool text and preamble in
/// `contract.rs`, the diagnosis prompt, or a prompt pack?
pub fn is_prompt_file(path: &str) -> bool {
    matches!(
        path,
        "crates/daemon/src/prompts.rs"
            | "crates/adapter-api/src/contract.rs"
            | "crates/daemon/src/diagnose.rs"
    ) || path.starts_with(".mason/prompts/")
}

/// Where the latest result for a revision is kept.
pub fn result_path(data_dir: &Path, sha: &str) -> PathBuf {
    data_dir.join("evals").join(format!("{sha}.json"))
}

pub fn save(data_dir: &Path, report: &Report) -> Result<PathBuf, String> {
    let path = result_path(data_dir, &report.candidate);
    std::fs::create_dir_all(path.parent().unwrap_or(data_dir)).map_err(|e| e.to_string())?;
    std::fs::write(&path, format!("{:#}\n", report.to_json())).map_err(|e| e.to_string())?;
    Ok(path)
}

/// For the owner-approval card: when the change touches a prompt file, a line linking the latest
/// eval result for `sha` if there is one, and saying so if there is not.
pub fn approval_note(data_dir: &Path, files: &[String], sha: &str) -> Option<String> {
    if !files.iter().any(|f| is_prompt_file(f)) {
        return None;
    }
    let path = result_path(data_dir, sha);
    let summary = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["summary"].as_str().map(String::from));
    Some(match summary {
        Some(s) => format!(
            "prompt eval for {}: {s} ({})",
            &sha[..sha.len().min(10)],
            path.display()
        ),
        None => "no prompt eval result for this revision (`mason prompts eval`)".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mason_adapters::fake::{FakeAction, FakeAdapter};

    fn sh(dir: &Path, cmd: &str) {
        let st = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success(), "{cmd}");
    }

    fn project(dir: &Path) -> ProjectCfg {
        let mut p = crate::config::parse_project(
            "p",
            &format!(
                "path = \"{}\"\nbase = \"main\"\ntest = \"true\"\n",
                dir.display()
            ),
        )
        .unwrap();
        p.path = dir.to_path_buf();
        p
    }

    fn case(head: &str) -> Case {
        Case {
            file: ".mason/evals/review/7.toml".into(),
            task: 7,
            project: "p".into(),
            head: head.into(),
            step: "review".into(),
            title: "Fix it".into(),
            body: "Make it \"work\".\nSecond line.".into(),
            plan: None,
            reason: None,
            logs: None,
            expect: vec![
                "verdict approve".into(),
                "no blocking finding about docs".into(),
            ],
        }
    }

    fn repo() -> (tempfile::TempDir, String) {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        sh(d, "git init -q -b main && git config user.email a@b && git config user.name n && git config commit.gpgsign false");
        std::fs::write(d.join("a.txt"), "one\n").unwrap();
        sh(d, "git add -A && git commit -q -m one");
        let head = mason_landing::git::run(d, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let c = case(&head);
        write_case(d, &c).unwrap();
        sh(d, "git add -A && git commit -q -m case");
        (t, head)
    }

    fn review(verdict: &str) -> Vec<FakeAction> {
        vec![FakeAction::Call {
            tool: "submit_review".into(),
            args: json!({"verdict": verdict, "summary": "fine", "findings": []}),
        }]
    }

    fn grade(base: bool, cand: bool) -> Vec<FakeAction> {
        vec![FakeAction::Say(
            json!({"base": {"pass": base, "judgement": "as expected", "quotes": ["fine"]},
                   "candidate": {"pass": cand, "judgement": "also", "quotes": []},
                   "weak_assertions": []})
            .to_string(),
        )]
    }

    #[test]
    fn a_case_file_round_trips_and_a_bad_one_is_refused() {
        let c = case("abc");
        let back = Case::parse(&c.path(), &c.to_toml()).unwrap();
        assert_eq!(back.body, c.body);
        assert_eq!(back.expect, c.expect);
        assert!(Case::parse("f.toml", "step = \"review\"\n").is_err());
        let no_expect = c.to_toml().replace("expect", "other");
        assert!(Case::parse("f.toml", &no_expect)
            .unwrap_err()
            .contains("expect"));
    }

    #[test]
    fn both_prompts_are_built_from_the_given_revisions_and_both_verdicts_and_the_judgement_are_recorded(
    ) {
        let (t, head) = repo();
        let p = project(t.path());
        // The candidate revision carries a pack the base does not.
        sh(t.path(), "mkdir -p .mason/prompts && echo 'CANDIDATE PACK {task}' > .mason/prompts/review.md && git add -A && git commit -q -m pack");
        let (cases, bad) = load_cases(t.path(), "HEAD~1", Some("review"), Some("p"));
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].head, head);
        let seen = std::sync::Mutex::new(vec![]);
        let renderer = |c: &Case, rev: &str| {
            let r = render(c, &p, rev)?;
            seen.lock().unwrap().push(r.clone());
            Ok(r)
        };
        let adapter = FakeAdapter::new(vec![
            review("approve"),
            review("changes"),
            grade(true, false),
        ]);
        let scratch = tempfile::tempdir().unwrap();
        let report = run(
            &adapter,
            &p,
            &cases,
            "HEAD~1",
            "HEAD",
            &renderer,
            scratch.path(),
        );
        let seen = seen.into_inner().unwrap();
        assert!(
            seen[0].contains("You are reviewing the change for task #7"),
            "{}",
            seen[0]
        );
        assert!(seen[1].starts_with("CANDIDATE PACK 7"), "{}", seen[1]);
        let c = &report.cases[0];
        assert_eq!(c.base.as_ref().unwrap().verdict, "approve");
        assert_eq!(c.candidate.as_ref().unwrap().verdict, "changes");
        assert!(c.base_judgement.as_ref().unwrap().pass);
        assert!(!c.candidate_judgement.as_ref().unwrap().pass);
        assert_eq!(c.base_judgement.as_ref().unwrap().quotes, ["fine"]);
        assert_eq!(report.summary(), "base passes 1/1, candidate passes 0/1");
        let text = report.text();
        assert!(
            text.contains("base: verdict approve") && text.contains("candidate: verdict changes"),
            "{text}"
        );
        assert_eq!(text.lines().last().unwrap(), report.summary());
    }

    #[test]
    fn a_case_whose_head_is_gone_is_skipped_with_a_reason() {
        let (t, _) = repo();
        let p = project(t.path());
        let c = case("0123456789012345678901234567890123456789");
        let adapter = FakeAdapter::new(vec![]);
        let scratch = tempfile::tempdir().unwrap();
        let renderer = |c: &Case, rev: &str| render(c, &p, rev);
        let report = run(
            &adapter,
            &p,
            &[c],
            "HEAD",
            "HEAD",
            &renderer,
            scratch.path(),
        );
        let r = &report.cases[0];
        assert!(r.skipped.as_ref().unwrap().contains("is gone"));
        assert!(r.error.is_none() && r.base.is_none());
        assert_eq!(
            report.summary(),
            "base passes 0/0, candidate passes 0/0 (1 skipped)"
        );
    }

    #[test]
    fn a_finished_tasks_case_is_written_as_a_valid_file_and_not_over_written() {
        let t = tempfile::tempdir().unwrap();
        let c = case("abc");
        let path = write_case(t.path(), &c).unwrap();
        assert_eq!(path, t.path().join(".mason/evals/review/7.toml"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# SPDX-License-Identifier"));
        assert_eq!(
            Case::parse("x", &text).unwrap(),
            Case {
                file: "x".into(),
                ..c.clone()
            }
        );
        assert!(write_case(t.path(), &c).is_err());
    }

    #[test]
    fn the_grader_reply_is_read_and_the_approval_note_links_the_result() {
        let said = format!(
            "Here you go.\n{}",
            grade(true, true)
                .iter()
                .map(|a| match a {
                    FakeAction::Say(s) => s.clone(),
                    _ => String::new(),
                })
                .collect::<String>()
        );
        let (b, k, weak) = parse_grade(&said).unwrap();
        assert!(b.pass && k.pass && weak.is_empty());
        assert!(parse_grade("no json").is_err());
        let d = tempfile::tempdir().unwrap();
        let files = vec!["crates/daemon/src/prompts.rs".to_string()];
        assert!(approval_note(d.path(), &["README.md".into()], "abcdef1234567").is_none());
        assert!(approval_note(d.path(), &files, "abcdef1234567")
            .unwrap()
            .contains("no prompt eval result"));
        let report = Report {
            base: "b".into(),
            candidate: "abcdef1234567".into(),
            cases: vec![],
        };
        save(d.path(), &report).unwrap();
        assert!(approval_note(d.path(), &files, "abcdef1234567")
            .unwrap()
            .contains("base passes 0/0"));
    }
}
