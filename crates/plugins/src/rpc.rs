// SPDX-License-Identifier: GPL-3.0-or-later

//! JSON-RPC 2.0, one JSON object per line. The host sends requests; the plugin
//! answers each with a result or an error. Anything a plugin writes to stderr is
//! its log, not protocol.
//!
//! Methods (protocol 1):
//!
//! * `initialize {protocol, host}` -> `{protocol, name, version, provides, config_schema}`
//! * `step.run {step, type, config, task, title, worktree, env}` -> `{passed, summary, data?}`
//! * `shutdown {}` -> `null`, after which the process exits.

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub id: Option<u64>,
    pub result: Result<Value, RpcError>,
}

/// One request, as a line (without the newline).
pub fn request(id: u64, method: &str, params: &Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// Read one line a plugin wrote. Notifications and other lines without a result or
/// an error are `Ok(None)`: they are ignored.
pub fn parse_response(line: &str) -> Result<Option<Response>, String> {
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| {
        format!(
            "not a JSON line ({e}): {}",
            line.trim().chars().take(80).collect::<String>()
        )
    })?;
    if !v.is_object() {
        return Err("a message must be a JSON object".into());
    }
    if v["jsonrpc"] != "2.0" {
        return Err("a message must have \"jsonrpc\": \"2.0\"".into());
    }
    let id = v["id"].as_u64();
    if let Some(e) = v.get("error") {
        return Ok(Some(Response {
            id,
            result: Err(RpcError {
                code: e["code"].as_i64().unwrap_or(-32000),
                message: e["message"].as_str().unwrap_or("(no message)").to_string(),
            }),
        }));
    }
    match v.get("result") {
        Some(r) => Ok(Some(Response {
            id,
            result: Ok(r.clone()),
        })),
        None => Ok(None),
    }
}

/// What `initialize` says, checked for shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Initialized {
    pub protocol: u32,
    pub name: String,
    pub version: String,
    pub provides: Vec<String>,
    pub config_schema: Value,
}

pub fn read_initialized(v: &Value) -> Result<Initialized, String> {
    let s = |k: &str| {
        v[k].as_str()
            .map(String::from)
            .ok_or_else(|| format!("initialize: `{k}` must be a string"))
    };
    let provides = v["provides"]
        .as_array()
        .ok_or("initialize: `provides` must be a list of strings")?
        .iter()
        .map(|x| {
            x.as_str()
                .map(String::from)
                .ok_or("initialize: `provides` must be a list of strings")
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Initialized {
        protocol: v["protocol"]
            .as_u64()
            .ok_or("initialize: `protocol` must be a number")? as u32,
        name: s("name")?,
        version: s("version")?,
        provides,
        config_schema: v
            .get("config_schema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"})),
    })
}

/// What `step.run` answers.
#[derive(Clone, Debug, PartialEq)]
pub struct StepResult {
    pub passed: bool,
    pub summary: String,
    pub data: Value,
}

pub fn read_step_result(v: &Value) -> Result<StepResult, String> {
    let passed = v["passed"]
        .as_bool()
        .ok_or("step.run: `passed` must be true or false")?;
    let summary = v["summary"]
        .as_str()
        .ok_or("step.run: `summary` must be a string")?
        .to_string();
    Ok(StepResult {
        passed,
        summary,
        data: v.get("data").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_one_line_and_responses_are_read() {
        let r = request(7, "step.run", &json!({"a": 1}));
        assert!(!r.contains('\n'));
        assert!(r.contains("\"jsonrpc\":\"2.0\""));
        let ok = parse_response(r#"{"jsonrpc":"2.0","id":7,"result":{"x":1}}"#)
            .unwrap()
            .unwrap();
        assert_eq!((ok.id, ok.result), (Some(7), Ok(json!({"x": 1}))));
        let err =
            parse_response(r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32602,"message":"bad"}}"#)
                .unwrap()
                .unwrap();
        assert_eq!(err.result.unwrap_err().message, "bad");
        assert_eq!(
            parse_response(r#"{"jsonrpc":"2.0","method":"log"}"#).unwrap(),
            None
        );
        assert!(parse_response("hello").is_err());
        assert!(parse_response(r#"{"id":1,"result":1}"#).is_err());
    }

    #[test]
    fn results_are_checked_for_shape() {
        assert!(read_step_result(&json!({"passed": true, "summary": "ok"})).is_ok());
        assert!(read_step_result(&json!({"passed": "yes", "summary": "ok"})).is_err());
        assert!(read_step_result(&json!({"passed": true})).is_err());
        assert!(read_initialized(
            &json!({"protocol":1,"name":"a","version":"1","provides":["step-type:x"]})
        )
        .is_ok());
        assert!(read_initialized(&json!({"protocol":1,"name":"a","provides":[]})).is_err());
    }
}
