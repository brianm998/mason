// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason prompts eval` (task 1139): replay the read-only steps of past tasks under the prompt of
//! a base and a candidate revision, and have a grader compare the results with what each case
//! expects. The work is in `mason_daemon::prompts_eval`; this reads the arguments, builds the
//! agent adapter and prints.

use mason_adapter_api::AgentAdapter;
use mason_daemon::config::{self, ProjectCfg};
use mason_daemon::prompts_eval::{self as pe, Case};
use std::path::{Path, PathBuf};

pub const COMMANDS: &str = "mason prompts eval [--project P] [--step review|diagnose|explore] [--against REV] [--candidate REV] [--base-bin PATH] [--candidate-bin PATH] [--json]\n       mason prompts eval add <task> --step S --expect \"...\" [--expect ...] [--project P] [--head REV] [--reason R] [--logs-file F] [--plan-file F] [--repo DIR]\n       mason prompts render <case-file> --project P --rev REV";

fn opt(rest: &mut Vec<String>, name: &str) -> Result<Option<String>, String> {
    let Some(i) = rest.iter().position(|a| a == name) else {
        return Ok(None);
    };
    rest.remove(i);
    if i < rest.len() {
        Ok(Some(rest.remove(i)))
    } else {
        Err(format!("{name} needs a value"))
    }
}

fn opts(rest: &mut Vec<String>, name: &str) -> Result<Vec<String>, String> {
    let mut v = vec![];
    while let Some(x) = opt(rest, name)? {
        v.push(x);
    }
    Ok(v)
}

fn flag(rest: &mut Vec<String>, name: &str) -> bool {
    rest.iter()
        .position(|a| a == name)
        .map(|i| rest.remove(i))
        .is_some()
}

fn project(config_dir: &Path, want: Option<String>) -> Result<ProjectCfg, String> {
    match want {
        Some(id) => config::load_project(config_dir, &id),
        None => {
            let mut all = config::load_projects(config_dir).map_err(|e| e.join("; "))?;
            match all.len() {
                1 => Ok(all.remove(0)),
                0 => Err("no projects are configured (see `mason doctor`)".into()),
                _ => Err(format!(
                    "which project? use --project with one of: {}",
                    all.iter()
                        .map(|p| p.id.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
    }
}

pub fn run(mut rest: Vec<String>, config_dir: &Path, data_dir: &Path) -> i32 {
    match go(&mut rest, config_dir, data_dir) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}\n\n{COMMANDS}");
            1
        }
    }
}

fn go(rest: &mut Vec<String>, config_dir: &Path, data_dir: &Path) -> Result<i32, String> {
    if rest.is_empty() {
        return Err("prompts what? eval or render".into());
    }
    match rest.remove(0).as_str() {
        "render" => render(rest, config_dir),
        "eval" if rest.first().map(String::as_str) == Some("add") => {
            rest.remove(0);
            add(rest, config_dir, data_dir)
        }
        "eval" => eval(rest, config_dir, data_dir),
        other => Err(format!("unknown prompts command {other:?}")),
    }
}

fn render(rest: &mut Vec<String>, config_dir: &Path) -> Result<i32, String> {
    let p = project(config_dir, opt(rest, "--project")?)?;
    let rev = opt(rest, "--rev")?.ok_or("--rev is needed")?;
    let file = rest.first().cloned().ok_or("which case file?")?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?;
    let case = Case::parse(&file, &text)?;
    print!("{}", pe::render(&case, &p, &rev)?);
    Ok(0)
}

fn adapter(p: &ProjectCfg, data_dir: &Path) -> Box<dyn AgentAdapter> {
    use mason_adapters::{claude::ClaudeAdapter, codex::CodexAdapter};
    let mcp = std::env::current_exe()
        .map(|e| e.with_file_name("mason-mcp"))
        .unwrap_or_else(|_| PathBuf::from("mason-mcp"));
    let state = data_dir.join("a");
    let agent = p.agent();
    match agent.provider {
        config::Provider::Claude => {
            let bin = p
                .claude_bin
                .clone()
                .unwrap_or_else(|| PathBuf::from("claude"));
            let mut a = ClaudeAdapter::new(bin, mcp, state);
            a.env_file = Some(config_env(p));
            Box::new(a)
        }
        config::Provider::Codex => {
            let bin = agent.codex_bin.unwrap_or_else(|| PathBuf::from("codex"));
            Box::new(CodexAdapter::new(bin, mcp, state))
        }
    }
}

fn config_env(p: &ProjectCfg) -> PathBuf {
    p.config_dir.join("claude.env")
}

fn eval(rest: &mut Vec<String>, config_dir: &Path, data_dir: &Path) -> Result<i32, String> {
    let p = project(config_dir, opt(rest, "--project")?)?;
    let step = opt(rest, "--step")?;
    if let Some(s) = &step {
        if !pe::STEPS.contains(&s.as_str()) {
            return Err(format!("--step must be one of {}", pe::STEPS.join(", ")));
        }
    }
    let base = opt(rest, "--against")?.unwrap_or_else(|| p.base_ref());
    let cand = opt(rest, "--candidate")?.unwrap_or_else(|| "HEAD".into());
    let base_bin = opt(rest, "--base-bin")?.map(PathBuf::from);
    let cand_bin = opt(rest, "--candidate-bin")?.map(PathBuf::from);
    let json = flag(rest, "--json");
    if let Some(extra) = rest.first() {
        return Err(format!("unexpected argument {extra:?}"));
    }
    let (cases, bad) = pe::load_cases(&p.path, &base, step.as_deref(), Some(&p.id));
    for b in &bad {
        eprintln!("warning: {b}");
    }
    let scratch = std::env::temp_dir().join(format!("mpe-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    // A revision's built-in prompt text is its binary's: without one, this binary's is used
    // for both, and only the packs (`.mason/prompts`) differ between the revisions.
    let renderer = |c: &Case, rev: &str| -> Result<String, String> {
        let bin = if rev == base { &base_bin } else { &cand_bin };
        let Some(bin) = bin else {
            return pe::render(c, &p, rev);
        };
        let f = scratch.join(format!("case-{}.toml", c.task));
        std::fs::write(&f, c.to_toml()).map_err(|e| e.to_string())?;
        let out = std::process::Command::new(bin)
            .args(["--config-dir"])
            .arg(config_dir)
            .args(["prompts", "render", "--project", &p.id, "--rev", rev])
            .arg(&f)
            .output()
            .map_err(|e| format!("{}: {e}", bin.display()))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    let adapter = adapter(&p, data_dir);
    let report = pe::run(
        adapter.as_ref(),
        &p,
        &cases,
        &base,
        &cand,
        &renderer,
        &scratch,
    );
    let _ = std::fs::remove_dir_all(&scratch);
    match pe::save(data_dir, &report) {
        Ok(path) => eprintln!("result kept in {}", path.display()),
        Err(e) => eprintln!("warning: the result could not be kept: {e}"),
    }
    if json {
        println!("{:#}", report.to_json());
    } else {
        print!("{}", report.text());
    }
    Ok(0)
}

fn add(rest: &mut Vec<String>, config_dir: &Path, data_dir: &Path) -> Result<i32, String> {
    let step = opt(rest, "--step")?.ok_or("--step is needed")?;
    let expect = opts(rest, "--expect")?;
    let want = opt(rest, "--project")?;
    let head = opt(rest, "--head")?;
    let reason = opt(rest, "--reason")?;
    let read = |f: Option<String>| -> Result<Option<String>, String> {
        f.map(|f| std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}")))
            .transpose()
    };
    let logs = read(opt(rest, "--logs-file")?)?;
    let plan = read(opt(rest, "--plan-file")?)?;
    let repo = opt(rest, "--repo")?.map(PathBuf::from);
    let task: u64 = rest
        .first()
        .and_then(|t| t.parse().ok())
        .ok_or("which task? give its number")?;
    let d = crate::client::find(data_dir).map_err(|e| e.to_string())?;
    let t = d
        .call("GET", &format!("/tasks/{task}"), None)
        .map_err(|e| e.to_string())?;
    let p = project(
        config_dir,
        want.or_else(|| t["project"].as_str().map(String::from)),
    )?;
    let head = match head {
        Some(h) => h,
        None => {
            let branch = t["branch"]
                .as_str()
                .ok_or("the task has no branch; give --head")?;
            mason_landing::git::rev_parse(&p.path, branch)
                .map_err(|e| format!("branch {branch} could not be read ({e}); give --head"))?
        }
    };
    let head = mason_landing::git::rev_parse(&p.path, &head)
        .map_err(|e| format!("{head}: not a commit ({e})"))?;
    let case = Case {
        file: String::new(),
        task,
        project: p.id.clone(),
        head,
        step,
        title: t["title"].as_str().unwrap_or("").to_string(),
        body: t["body"].as_str().unwrap_or("").to_string(),
        plan,
        reason,
        logs,
        expect,
    };
    // The same checks a case gets when it is read.
    Case::parse("new case", &case.to_toml())?;
    let repo = repo.unwrap_or_else(|| PathBuf::from("."));
    let path = pe::write_case(&repo, &case)?;
    println!(
        "wrote {} (commit it with the owner's approval: .mason/** is protected)",
        path.display()
    );
    Ok(0)
}
