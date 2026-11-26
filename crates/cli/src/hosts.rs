// SPDX-License-Identifier: GPL-3.0-or-later

//! What runs where (#1155): `mason hosts`, and the `running on:` line of `mason task show`.
//! Both print what the daemon sends (`hosts` of `/status`, `running_on` of `/tasks/{id}`).

use serde_json::Value;

/// `local`, `node:<name>` and `github:<runs_on>` in words.
fn host_words(host: &str) -> String {
    match host.split_once(':') {
        Some(("node", n)) => format!("node {n}"),
        Some(("github", r)) => format!("GitHub {r}").trim_end().to_string(),
        _ if host == "local" => "this Mac".to_string(),
        _ => host.to_string(),
    }
}

fn took(run: &Value) -> Option<String> {
    let s = run["elapsed_s"].as_u64()?;
    Some(if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min", s / 60)
    })
}

/// "running on: GitHub macos-15 (check cargo-workspace, 12 min) · this Mac (agent, develop)",
/// `None` when nothing of the task runs.
pub fn running_line(running_on: &Value) -> Option<String> {
    let mut runs: Vec<&Value> = running_on.as_array()?.iter().collect();
    // Off this machine first: that is what the owner cannot see otherwise.
    runs.sort_by_key(|r| r["host"] == "local");
    let parts: Vec<String> = runs
        .iter()
        .map(|r| {
            let (kind, name) = (
                r["kind"].as_str().unwrap_or("?"),
                r["name"].as_str().unwrap_or("?"),
            );
            let what = if kind == "agent" {
                format!("agent, {name}")
            } else {
                match took(r) {
                    Some(t) => format!("{kind} {name}, {t}"),
                    None => format!("{kind} {name}"),
                }
            };
            format!("{} ({what})", host_words(r["host"].as_str().unwrap_or("?")))
        })
        .collect();
    (!parts.is_empty()).then(|| format!("running on: {}", parts.join(" · ")))
}

/// The host list as `mason hosts` prints it.
pub fn lines(hosts: &Value) -> Vec<String> {
    let mut out = vec![];
    for h in hosts.as_array().into_iter().flatten() {
        let name = h["name"].as_str().unwrap_or("?");
        let head = match h["kind"].as_str() {
            Some("node") => format!("node:{name}"),
            _ => name.to_string(),
        };
        let status = h["status"].as_str().unwrap_or("");
        out.push(format!("{head}  {status}").trim_end().to_string());
        let runs = h["runs"].as_array().map_or(&[][..], Vec::as_slice);
        if runs.is_empty() {
            out.push("  idle".to_string());
        }
        for r in runs {
            let task = match r["task"].as_u64() {
                Some(n) => {
                    let state = r["state"].as_str().unwrap_or("?");
                    match r["activity"].as_str().filter(|a| !a.is_empty()) {
                        Some(a) => format!("#{n} {state} ({a})"),
                        None => format!("#{n} {state}"),
                    }
                }
                None => "no task".to_string(),
            };
            let mut line = format!(
                "  {task}  {} {}",
                r["kind"].as_str().unwrap_or("?"),
                r["name"].as_str().unwrap_or("?")
            );
            if let Some(("github", runs_on)) = r["host"].as_str().and_then(|h| h.split_once(':')) {
                if !runs_on.is_empty() {
                    line += &format!(" on {runs_on}");
                }
            }
            for extra in [
                took(r),
                r["title"].as_str().map(String::from),
                r["url"]
                    .as_str()
                    .filter(|u| !u.is_empty())
                    .map(String::from),
            ]
            .into_iter()
            .flatten()
            {
                line += &format!("  {extra}");
            }
            out.push(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_running_on_line_names_each_host_off_machine_first() {
        let v = json!([
            {"host": "local", "kind": "agent", "name": "develop", "elapsed_s": 1000},
            {"host": "github:macos-15", "kind": "check", "name": "cargo-workspace",
             "elapsed_s": 725},
        ]);
        assert_eq!(
            running_line(&v).as_deref(),
            Some(
                "running on: GitHub macos-15 (check cargo-workspace, 12 min) · this Mac (agent, develop)"
            )
        );
        assert_eq!(running_line(&json!([])), None);
        assert_eq!(running_line(&Value::Null), None);
    }

    #[test]
    fn every_host_is_printed_with_its_runs_or_idle() {
        let v = json!([
            {"kind": "local", "name": "this Mac", "status": "compile 0 of 6", "runs": []},
            {"kind": "node", "name": "marmot", "status": "accepting", "runs": [
                {"task": 5, "title": "Fix it", "state": "developing", "activity": "running",
                 "kind": "check", "name": "unit", "host": "node:marmot", "elapsed_s": 3,
                 "url": ""},
            ]},
        ]);
        assert_eq!(
            lines(&v),
            [
                "this Mac  compile 0 of 6",
                "  idle",
                "node:marmot  accepting",
                "  #5 developing (running)  check unit  3 s  Fix it",
            ]
        );
    }
}
