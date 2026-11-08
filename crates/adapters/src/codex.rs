// SPDX-License-Identifier: GPL-3.0-or-later

//! The Codex CLI adapter: the second agent, here to show the agent interface is
//! neutral. It runs `codex exec --json`, gives the CLI the step contract through
//! an MCP server entry, and turns its JSONL events into [`AgentEvent`]s.
//!
//! The event format below is written from the shape of `codex exec --json` as
//! its documentation describes it; Codex was not installed where this was
//! built, so the parser is checked against fixtures in that shape and not
//! against the real CLI (docs/stages/s20.md says so). Everything the step
//! runner needs (resume, the outcome classes, the sandbox) goes through the
//! same code as Claude's.

use crate::claude::{
    clip, launch_process, num, sanitized_env_for, AgentCommand, Dialect, ParseState, ResultInfo,
};
use mason_adapter_api::contract::preamble;
use mason_adapter_api::{
    netguard, AdapterError, AgentAdapter, AgentCaps, AgentEvent, SessionHandle, SessionSpec,
    TokenReport,
};
use serde_json::Value;
use std::path::PathBuf;

const PREFIXES: &[&str] = &["LC_", "OPENAI_", "CODEX_"];

/// The efforts this provider's CLI accepts.
pub const EFFORT_LEVELS: [&str; 4] = ["minimal", "low", "medium", "high"];

#[derive(Clone, Debug)]
pub struct CodexAdapter {
    /// The `codex` executable (or a scripted stand-in).
    pub bin: PathBuf,
    /// The MCP bridge executable that exposes the step contract.
    pub mcp_bin: PathBuf,
    pub state_dir: PathBuf,
    pub parent_env: Option<Vec<(String, String)>>,
    pub host: Option<crate::hosted::HostLaunch>,
    pub sandbox: Option<crate::sandbox::SandboxPolicy>,
}

/// A string as a TOML basic string, for `-c key=value`.
fn toml_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

impl CodexAdapter {
    pub fn new(
        bin: impl Into<PathBuf>,
        mcp_bin: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
    ) -> Self {
        CodexAdapter {
            bin: bin.into(),
            mcp_bin: mcp_bin.into(),
            state_dir: state_dir.into(),
            parent_env: None,
            host: None,
            sandbox: None,
        }
    }

    pub fn with_host(mut self, h: crate::hosted::HostLaunch) -> Self {
        self.host = Some(h);
        self
    }

    pub fn build_command(
        &self,
        spec: &SessionSpec,
        resume: Option<(&str, &str)>,
    ) -> Result<AgentCommand, AdapterError> {
        let parent = self
            .parent_env
            .clone()
            .unwrap_or_else(|| std::env::vars().collect());
        let sock = spec.contract.as_ref().and_then(|c| c.sock());
        let env = sanitized_env_for(spec, parent, sock.as_deref(), PREFIXES);
        let base_url = env
            .iter()
            .find(|(k, _)| k == "OPENAI_BASE_URL")
            .map(|(_, v)| v.as_str());
        netguard::check_endpoint("OPENAI_BASE_URL", base_url)?;

        let mut args: Vec<String> = vec!["exec".into(), "--json".into()];
        args.extend(["-C".into(), spec.workdir.display().to_string()]);
        // Read-only steps get no way to change the tree; the others may write
        // inside the worktree and nowhere else. Network stays off in both.
        args.extend([
            "-s".into(),
            if spec.step.read_only() {
                "read-only"
            } else {
                "workspace-write"
            }
            .into(),
        ]);
        let mut cfg = |k: &str, v: String| args.extend(["-c".into(), format!("{k}={v}")]);
        cfg("approval_policy", toml_str("never"));
        if let Some(m) = &spec.effort {
            cfg("model_reasoning_effort", toml_str(m));
        }
        cfg(
            "mcp_servers.mason.command",
            toml_str(&self.mcp_bin.display().to_string()),
        );
        cfg("mcp_servers.mason.args", "[]".into());
        let mut menv = vec![format!("MASON_STEP_KIND={}", toml_str(spec.step.name()))];
        if let Some(t) = &spec.tools {
            menv.push(format!("MASON_TOOLS={}", toml_str(&t.join(","))));
        }
        if let Some(s) = &sock {
            menv.push(format!(
                "MASON_CONTRACT_SOCK={}",
                toml_str(&s.display().to_string())
            ));
        }
        cfg(
            "mcp_servers.mason.env",
            format!("{{ {} }}", menv.join(", ")),
        );
        cfg(
            "mcp_servers.mason.tool_timeout_sec",
            spec.tool_timeout.as_secs().max(1).to_string(),
        );
        if let Some(m) = &spec.model {
            args.extend(["-m".into(), m.clone()]);
        }
        let stdin = match resume {
            Some((id, note)) => {
                args.extend(["resume".into(), id.into()]);
                note.to_string()
            }
            // Codex has no system-prompt flag: the contract's preamble leads the prompt.
            None => format!("{}\n\n{}", preamble(spec.step), spec.prompt),
        };
        args.push("-".into());
        let _ = &self.state_dir;
        Ok(AgentCommand {
            program: self.bin.clone(),
            args,
            env,
            cwd: spec.workdir.clone(),
            stdin,
        })
    }
}

impl AgentAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn capabilities(&self) -> AgentCaps {
        AgentCaps {
            resume: true,
            mcp: true,
            effort_levels: EFFORT_LEVELS.map(String::from).to_vec(),
            usage_reports: true,
        }
    }

    fn start(&self, spec: SessionSpec) -> Result<Box<dyn SessionHandle>, AdapterError> {
        let cmd = self.build_command(&spec, None)?;
        let cmd = crate::sandbox::apply(cmd, self.sandbox.as_ref())?;
        launch_process(self.host.as_ref(), cmd, &spec, Dialect::Codex)
    }

    fn resume(
        &self,
        id: &str,
        note: String,
        spec: SessionSpec,
    ) -> Result<Box<dyn SessionHandle>, AdapterError> {
        let cmd = self.build_command(&spec, Some((id, &note)))?;
        let cmd = crate::sandbox::apply(cmd, self.sandbox.as_ref())?;
        launch_process(self.host.as_ref(), cmd, &spec, Dialect::Codex)
    }

    fn reattach(
        &self,
        run_key: &str,
        spec: SessionSpec,
    ) -> Result<Option<Box<dyn SessionHandle>>, AdapterError> {
        let Some(h) = &self.host else {
            return Ok(None);
        };
        Ok(crate::hosted::latest_invocation(&h.root, run_key)
            .map(|id| crate::hosted::attach_as(h, id, &spec, Dialect::Codex)))
    }
}

/// What a failure message says about the provider, in the terms the shared
/// classifier knows: a status code, and a reset time spelled `resets <when>`.
fn failure(msg: &str) -> ResultInfo {
    let low = msg.to_lowercase();
    let status = if low.contains("usage limit") || low.contains("rate limit") || low.contains("429")
    {
        Some(429)
    } else if low.contains("stream disconnected")
        || low.contains("overloaded")
        || low.contains("service unavailable")
    {
        Some(503)
    } else {
        // `unexpected status 502 Bad Gateway`
        low.split_once("status ")
            .and_then(|(_, r)| r.get(..3))
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|n| (400..600).contains(n))
    };
    let text = match low.split_once("try again at ") {
        Some((_, when)) => format!("{msg} · resets {}", when.trim_end_matches('.')),
        None => msg.to_string(),
    };
    ResultInfo {
        is_error: true,
        text,
        api_error_status: status,
    }
}

/// Parse one `codex exec --json` line. Unknown or malformed lines yield nothing.
pub fn parse_line(line: &str, st: &mut ParseState) -> Vec<AgentEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![];
    };
    let mut out = vec![];
    match v["type"].as_str() {
        Some("thread.started") => {
            st.session_id = v["thread_id"].as_str().map(String::from);
        }
        Some("item.completed") => {
            let item = &v["item"];
            match item["type"].as_str() {
                Some("agent_message") => {
                    if let Some(t) = item["text"].as_str() {
                        out.push(AgentEvent::Text(t.to_string()));
                    }
                }
                Some("mcp_tool_call") => out.push(AgentEvent::ToolCall {
                    name: format!(
                        "mcp__{}__{}",
                        item["server"].as_str().unwrap_or("?"),
                        item["tool"].as_str().unwrap_or("?")
                    ),
                    summary: clip(&item["arguments"].to_string(), 200),
                }),
                Some("command_execution") => out.push(AgentEvent::ToolCall {
                    name: "command".into(),
                    summary: clip(item["command"].as_str().unwrap_or(""), 200),
                }),
                Some("file_change") => out.push(AgentEvent::ToolCall {
                    name: "file_change".into(),
                    summary: clip(&item["changes"].to_string(), 200),
                }),
                _ => {}
            }
        }
        Some("turn.completed") => {
            let u = &v["usage"];
            // Codex reports tokens and no cost: cost stays unreported, never zero.
            out.push(AgentEvent::Tokens(TokenReport {
                input: num(u, "input_tokens"),
                output: num(u, "output_tokens"),
                cache_read: num(u, "cached_input_tokens"),
                cache_create: None,
                cost_usd: None,
            }));
            // A reconnect that worked leaves an earlier bare `error` behind: the
            // turn finishing is what counts.
            st.result = Some(ResultInfo {
                is_error: false,
                text: String::new(),
                api_error_status: None,
            });
        }
        Some("turn.failed") => {
            let m = v["error"]["message"].as_str().unwrap_or("the turn failed");
            st.result = Some(failure(m));
        }
        // `turn.failed` follows with the same message and wins; this covers a
        // stream that ends after the bare error.
        Some("error") if st.result.is_none() => {
            st.result = Some(failure(v["message"].as_str().unwrap_or("error")));
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::final_outcome;
    use mason_adapter_api::{Outcome, StepKind};

    fn parse(name: &str) -> (Vec<AgentEvent>, ParseState) {
        let text = std::fs::read_to_string(format!(
            "{}/fixtures/codex/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let mut st = ParseState::new(Dialect::Codex);
        let ev = text
            .lines()
            .flat_map(|l| crate::claude::parse_line(l, &mut st))
            .collect();
        (ev, st)
    }

    #[test]
    fn a_plain_run_has_text_tokens_and_no_cost() {
        let (ev, st) = parse("plain.jsonl");
        assert_eq!(st.session_id.as_deref(), Some("0199-codex-thread-1"));
        assert!(ev
            .iter()
            .any(|e| matches!(e, AgentEvent::Text(t) if t == "pong")));
        let t = ev
            .iter()
            .find_map(|e| match e {
                AgentEvent::Tokens(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            (t.input, t.output, t.cache_read),
            (Some(120), Some(4), Some(100))
        );
        assert_eq!(t.cost_usd, None);
        assert_eq!(final_outcome(&st, false, Some(0), ""), Outcome::NoResult);
        assert_eq!(final_outcome(&st, true, Some(0), ""), Outcome::Accepted);
    }

    #[test]
    fn a_tool_run_reports_command_and_mcp_calls() {
        let (ev, st) = parse("tool_run.jsonl");
        let names: Vec<&str> = ev
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolCall { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["command", "mcp__mason__submit_plan"]);
        assert_eq!(final_outcome(&st, true, Some(0), ""), Outcome::Accepted);
    }

    #[test]
    fn a_usage_limit_is_a_hold_with_a_reset_time() {
        let (_, st) = parse("usage_limit.jsonl");
        assert_eq!(
            final_outcome(&st, false, Some(1), ""),
            Outcome::UsageLimit {
                resets_at: Some("3:00 pm".into())
            }
        );
    }

    #[test]
    fn a_provider_outage_is_a_hold_not_a_failure() {
        let (_, st) = parse("provider_down.jsonl");
        assert!(matches!(
            final_outcome(&st, false, Some(1), ""),
            Outcome::ProviderDown { .. }
        ));
    }

    #[test]
    fn no_result_at_all_is_a_crash() {
        let st = ParseState::new(Dialect::Codex);
        assert!(matches!(
            final_outcome(&st, false, Some(139), "segfault"),
            Outcome::Crash { .. }
        ));
    }

    #[test]
    fn junk_lines_are_ignored() {
        let mut st = ParseState::new(Dialect::Codex);
        assert!(crate::claude::parse_line("not json", &mut st).is_empty());
        assert!(crate::claude::parse_line("{\"type\":\"turn.started\"}", &mut st).is_empty());
    }

    fn spec(step: StepKind) -> SessionSpec {
        let mut s = SessionSpec::new(step, "do it", "/work");
        s.model = Some("gpt-5-codex".into());
        s.effort = Some("high".into());
        s
    }

    fn adapter() -> CodexAdapter {
        let mut a = CodexAdapter::new("codex", "/bin/mason-mcp", "/state");
        a.parent_env = Some(vec![
            ("PATH".into(), "/usr/bin".into()),
            ("OPENAI_API_KEY".into(), "k".into()),
            ("ANTHROPIC_API_KEY".into(), "no".into()),
            ("GITHUB_TOKEN".into(), "no".into()),
        ]);
        a
    }

    #[test]
    fn the_command_is_sandboxed_by_step_and_never_asks() {
        let c = adapter()
            .build_command(&spec(StepKind::Explore), None)
            .unwrap();
        let a = c.args.join(" ");
        assert!(a.starts_with("exec --json -C /work -s read-only"), "{a}");
        assert!(a.contains("approval_policy=\"never\""));
        assert!(a.contains("-m gpt-5-codex"));
        assert!(a.contains("model_reasoning_effort=\"high\""));
        assert!(a.contains("mcp_servers.mason.command=\"/bin/mason-mcp\""));
        assert!(a.contains("MASON_STEP_KIND=\"explore\""));
        assert!(a.ends_with(" -"));
        assert!(c.stdin.ends_with("do it"));
        let d = adapter()
            .build_command(&spec(StepKind::Develop), None)
            .unwrap();
        assert!(d.args.join(" ").contains("-s workspace-write"));
    }

    #[test]
    fn resume_continues_the_thread_with_the_note_alone() {
        let c = adapter()
            .build_command(&spec(StepKind::Develop), Some(("thr-9", "carry on")))
            .unwrap();
        let a = c.args.join(" ");
        assert!(a.ends_with("resume thr-9 -"), "{a}");
        assert_eq!(c.stdin, "carry on");
    }

    #[test]
    fn only_the_providers_variables_get_through() {
        let c = adapter()
            .build_command(&spec(StepKind::Explore), None)
            .unwrap();
        let has = |k: &str| c.env.iter().any(|(e, _)| e == k);
        assert!(has("PATH") && has("OPENAI_API_KEY"));
        assert!(!has("ANTHROPIC_API_KEY") && !has("GITHUB_TOKEN"));
    }
}
