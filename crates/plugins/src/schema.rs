// SPDX-License-Identifier: GPL-3.0-or-later

//! The part of JSON Schema that plugin settings use: enough to check a step's
//! settings and to let a client build a form. Supported keywords: `type`, `enum`,
//! `required`, `properties`, `additionalProperties: false`, `items`, `minimum`,
//! `maximum`, `minLength`. Other keywords (titles, descriptions, defaults and
//! `x-*` hints) are for forms and are ignored here.

use serde_json::{json, Map, Value};

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "null" => v.is_null(),
        _ => true,
    }
}

/// Is this usable as a plugin's settings schema? (An object with properties.)
pub fn check_schema(schema: &Value) -> Vec<String> {
    let mut p = vec![];
    if !schema.is_object() {
        return vec!["the schema must be a JSON object".into()];
    }
    if schema["type"] != "object" {
        p.push("the schema's `type` must be \"object\": a step's settings are an object".into());
    }
    if let Some(props) = schema.get("properties") {
        if !props.is_object() {
            p.push("`properties` must be an object".into());
        }
    }
    if let Some(r) = schema.get("required") {
        if !r.as_array().is_some_and(|a| a.iter().all(Value::is_string)) {
            p.push("`required` must be a list of names".into());
        }
    }
    p
}

/// What is wrong with `value` against `schema`, each mistake naming where (`path`).
pub fn validate(schema: &Value, value: &Value) -> Vec<String> {
    let mut out = vec![];
    walk(schema, value, "", &mut out);
    out
}

fn walk(schema: &Value, v: &Value, path: &str, out: &mut Vec<String>) {
    let at = if path.is_empty() { "settings" } else { path };
    if let Some(t) = schema["type"].as_str() {
        if !type_ok(t, v) {
            out.push(format!("{at} must be a {t}"));
            return;
        }
    }
    if let Some(e) = schema["enum"].as_array() {
        if !e.contains(v) {
            let names: Vec<String> = e.iter().map(|x| x.to_string()).collect();
            out.push(format!("{at} must be one of {}", names.join(", ")));
        }
    }
    if let (Some(min), Some(n)) = (schema["minimum"].as_f64(), v.as_f64()) {
        if n < min {
            out.push(format!("{at} must be at least {min}"));
        }
    }
    if let (Some(max), Some(n)) = (schema["maximum"].as_f64(), v.as_f64()) {
        if n > max {
            out.push(format!("{at} must be at most {max}"));
        }
    }
    if let (Some(min), Some(s)) = (schema["minLength"].as_u64(), v.as_str()) {
        if (s.chars().count() as u64) < min {
            out.push(format!("{at} must have at least {min} characters"));
        }
    }
    if let Some(o) = v.as_object() {
        for r in schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !o.contains_key(r) {
                out.push(format!("{} is required", join(path, r)));
            }
        }
        let props = schema["properties"].as_object();
        for (k, x) in o {
            match props.and_then(|p| p.get(k)) {
                Some(s) => walk(s, x, &join(path, k), out),
                None if schema["additionalProperties"] == false => {
                    let known: Vec<&str> = props
                        .map(|p| p.keys().map(String::as_str).collect())
                        .unwrap_or_default();
                    out.push(format!(
                        "unknown setting `{}`; the settings are {}",
                        join(path, k),
                        if known.is_empty() {
                            "(none)".into()
                        } else {
                            known.join(", ")
                        }
                    ));
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(a)) = (schema.get("items"), v.as_array()) {
        for (i, x) in a.iter().enumerate() {
            walk(items, x, &format!("{at}[{i}]"), out);
        }
    }
}

fn join(path: &str, k: &str) -> String {
    if path.is_empty() {
        k.to_string()
    } else {
        format!("{path}.{k}")
    }
}

fn sample_of(schema: &Value) -> Value {
    if let Some(d) = schema.get("default") {
        return d.clone();
    }
    if let Some(e) = schema["enum"].as_array().and_then(|e| e.first()) {
        return e.clone();
    }
    match schema["type"].as_str() {
        Some("string") => {
            json!("x".repeat(schema["minLength"].as_u64().unwrap_or(1).max(1) as usize))
        }
        Some("integer") | Some("number") => json!(schema["minimum"].as_i64().unwrap_or(1)),
        Some("boolean") => json!(true),
        Some("array") => json!([]),
        Some("object") => sample_config(schema),
        _ => Value::Null,
    }
}

/// A settings object that satisfies `schema`: every required property, with a
/// default, the first allowed value, or a plain value of the right type.
pub fn sample_config(schema: &Value) -> Value {
    let mut m = Map::new();
    let props = schema["properties"].as_object();
    for r in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let s = props.and_then(|p| p.get(r)).cloned().unwrap_or(Value::Null);
        m.insert(r.to_string(), sample_of(&s));
    }
    Value::Object(m)
}

/// A settings object `schema` forbids, if one can be made: a property of the wrong
/// type, a missing required one, or (if unknown settings are forbidden) an unknown one.
pub fn bad_config(schema: &Value) -> Option<Value> {
    let good = sample_config(schema);
    let props = schema["properties"].as_object();
    if let Some((k, s)) = props.and_then(|p| p.iter().find(|(_, s)| s["type"].is_string())) {
        let mut bad = good.clone();
        let wrong = match s["type"].as_str() {
            Some("string") => json!(12345),
            _ => json!("not the right type"),
        };
        bad[k] = wrong;
        if !validate(schema, &bad).is_empty() {
            return Some(bad);
        }
    }
    if let Some(r) = schema["required"]
        .as_array()
        .and_then(|r| r.first())
        .and_then(Value::as_str)
    {
        let mut bad = good.clone();
        bad.as_object_mut()?.remove(r);
        return Some(bad);
    }
    if schema["additionalProperties"] == false {
        let mut bad = good;
        bad["no_such_setting"] = json!(true);
        return Some(bad);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["tables"],
            "properties": {
                "tables": {"type": "string", "minLength": 1},
                "limit": {"type": "integer", "minimum": 1},
                "mode": {"type": "string", "enum": ["fast", "full"]},
                "tags": {"type": "array", "items": {"type": "string"}}
            }
        })
    }

    #[test]
    fn values_are_checked() {
        let s = schema();
        assert!(validate(
            &s,
            &json!({"tables": "a", "limit": 3, "mode": "fast", "tags": ["x"]})
        )
        .is_empty());
        let e = validate(
            &s,
            &json!({"limit": 0, "mode": "slow", "tags": [1], "extra": 1}),
        );
        let has = |f: &str| e.iter().any(|x| x.contains(f));
        assert!(has("tables is required"), "{e:?}");
        assert!(has("limit must be at least 1"), "{e:?}");
        assert!(has("mode must be one of"), "{e:?}");
        assert!(has("tags[0] must be a string"), "{e:?}");
        assert!(has("unknown setting `extra`"), "{e:?}");
    }

    #[test]
    fn samples_are_valid_and_bad_configs_are_not() {
        let s = schema();
        assert!(validate(&s, &sample_config(&s)).is_empty());
        assert!(!validate(&s, &bad_config(&s).unwrap()).is_empty());
        let open = json!({"type": "object"});
        assert!(bad_config(&open).is_none());
    }

    #[test]
    fn schemas_are_checked_for_shape() {
        assert!(check_schema(&schema()).is_empty());
        assert!(!check_schema(&json!([])).is_empty());
        assert!(!check_schema(&json!({"type": "string"})).is_empty());
    }
}
