// SPDX-License-Identifier: GPL-3.0-or-later

//! The Settings page's side of the daemon (s430): read and change the settings that used to
//! be file-only, for the monitors.
//!
//! - A project's proposal policy and caps (`[proposals]`, `[proposals.introspection]`), the
//!   cadence and title of its existing schedules (`[schedule.<name>]`) and its tracker's
//!   interval, label, import mode and label requirement (`[tracker]`) live in the project's
//!   registration file, `<config>/projects/<name>.toml`.
//! - Notification routes, quiet hours, reminders, the speech switch and the stall thresholds
//!   live in `<config>/notify.toml`.
//!
//! A change is applied to the file text with `toml_edit` (comments and layout survive), checked
//! by the very parsers that load the file, written through a temporary file and a rename, and
//! then taken live (`reload_registration` for a project; `notify.toml` is read on every tick).
//! A change the parsers or the reload refuse leaves the file as it was. Only a fixed list of
//! keys can be changed; a secret is never read back and never accepted as a value, only as an
//! `env:NAME` or `file:/path` reference.

use crate::daemon::Daemon;
use serde_json::{json, Map, Value};
use std::path::Path;
use toml_edit::{value, DocumentMut, Item, Table};

/// An HTTP status and message for the caller.
pub type Refused = (u16, String);

/// One edit at a time: two requests must not read the same text and each write their own.
static EDITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

const PROPOSAL_DISPOSITIONS: [&str; 4] = [
    "agent_normal",
    "agent_blocks",
    "outside_normal",
    "outside_blocks",
];
const PROPOSAL_CAPS: [&str; 2] = ["per_task", "per_day"];
const INTROSPECTION_KEYS: [&str; 4] = ["low_small", "low_large", "high_small", "high_large"];
/// The only secrets a tracker section holds, and they are references.
const TRACKER_REFS: [&str; 2] = ["token", "webhook_secret"];
const SCHEDULE_KEYS: [&str; 2] = ["every", "title"];
const SECRET_NAMES: [&str; 5] = ["password", "token", "secret", "password_env", "token_env"];

fn bad(m: impl Into<String>) -> Refused {
    (400, m.into())
}

fn project_file(d: &Daemon, name: &str) -> std::path::PathBuf {
    d.cfg
        .config_dir
        .join("projects")
        .join(format!("{name}.toml"))
}

/// What a secret reference looks like to the page: the reference, or only that it is set.
fn secret_view(v: Option<&str>) -> Value {
    match v {
        None => Value::Null,
        Some(r) if r.starts_with("env:") || r.starts_with("file:") => json!(r),
        Some(_) => json!("set in file, not shown"),
    }
}

/// The project's settings, read from the file as it stands.
pub fn get_project(d: &Daemon, name: &str) -> Result<Value, Refused> {
    get_project_as(d, name, &crate::projsettings::Caller::local(""))
}

/// The project's settings plus the catalogue of every key (`catalogue`, s1232), as this caller
/// may use it.
pub fn get_project_as(
    d: &Daemon,
    name: &str,
    caller: &crate::projsettings::Caller,
) -> Result<Value, Refused> {
    let mut v = get_project_legacy(d, name)?;
    let cat = crate::projsettings::catalogue(d, name, caller, &v)?;
    v["catalogue"] = cat;
    Ok(v)
}

fn get_project_legacy(d: &Daemon, name: &str) -> Result<Value, Refused> {
    let text = std::fs::read_to_string(project_file(d, name))
        .map_err(|e| (404, format!("{name}: {e}")))?;
    let doc: DocumentMut = text.parse().map_err(|e| bad(format!("{name}: {e}")))?;
    let cfg = crate::proposals::parse_cfg(name, &doc).map_err(bad)?;
    let disp = |x: mason_core::proposal::Disposition| match x {
        mason_core::proposal::Disposition::Proposed => "proposed",
        mason_core::proposal::Disposition::Queued => "queued",
    };
    let p = &cfg.policy;
    let i = &p.introspection;
    let proposals = json!({
        "agent_normal": disp(p.agent_normal),
        "agent_blocks": disp(p.agent_blocks),
        "outside_normal": disp(p.outside_normal),
        "outside_blocks": disp(p.outside_blocks),
        "per_task": cfg.caps.per_task,
        "per_day": cfg.caps.per_day,
        "introspection": {
            "low_small": disp(i.low_small), "low_large": disp(i.low_large),
            "high_small": disp(i.high_small), "high_large": disp(i.high_large),
        },
    });
    let text_of =
        |t: &dyn toml_edit::TableLike, k: &str| t.get(k).and_then(|v| v.as_str()).map(String::from);
    let mut schedules = vec![];
    if let Some(t) = doc.get("schedule").and_then(|v| v.as_table_like()) {
        for spec in crate::schedule::parse(name, &doc).map_err(bad)? {
            let every = t
                .get(&spec.name)
                .and_then(|v| v.as_table_like())
                .and_then(|s| text_of(s, "every"));
            schedules.push(json!({
                "name": spec.name, "kind": spec.kind,
                "every": every, "every_s": spec.every_ms / 1000, "title": spec.title,
            }));
        }
    }
    let tracker = doc
        .get("tracker")
        .and_then(|v| v.as_table_like())
        .map(|t| {
            json!({
                "kind": text_of(t, "kind"),
                "interval_s": t.get("interval_s").and_then(|v| v.as_integer()).unwrap_or(60),
                "label": text_of(t, "label").unwrap_or_else(|| "mason".into()),
                "import_as": text_of(t, "import_as").unwrap_or_else(|| "proposed".into()),
                "require_label": t.get("require_label").and_then(|v| v.as_bool()).unwrap_or(false),
                "token": secret_view(text_of(t, "token").as_deref()),
                "webhook_secret": secret_view(text_of(t, "webhook_secret").as_deref()),
            })
        })
        .unwrap_or(Value::Null);
    Ok(json!({
        "project": name,
        "proposals": proposals,
        "schedules": schedules,
        "tracker": tracker,
    }))
}

fn obj<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, Refused> {
    v.as_object()
        .ok_or_else(|| bad(format!("`{what}` must be an object")))
}

fn table_mut<'a>(t: &'a mut Table, key: &str) -> Result<&'a mut Table, Refused> {
    t.entry(key)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| bad(format!("`{key}` in the file is not a table")))
}

fn set_string(t: &mut Table, k: &str, v: &Value) -> Result<(), Refused> {
    let s = v
        .as_str()
        .ok_or_else(|| bad(format!("`{k}` must be a string")))?;
    set_item(t, k, value(s));
    Ok(())
}

/// Replace the value but keep the key's comments and position.
fn set_item(t: &mut Table, k: &str, new: Item) {
    if let Some(old) = t.get_mut(k).and_then(|i| i.as_value_mut()) {
        if let Some(nv) = new.as_value() {
            let mut nv = nv.clone();
            *nv.decor_mut() = old.decor().clone();
            *old = nv;
            return;
        }
    }
    t.insert(k, new);
}

fn apply_project(doc: &mut DocumentMut, change: &Value) -> Result<(), Refused> {
    let c = obj(change, "settings")?;
    for (section, body) in c {
        match section.as_str() {
            "as" => {}
            "proposals" => {
                let b = obj(body, "proposals")?;
                let t = table_mut(doc.as_table_mut(), "proposals")?;
                for (k, v) in b {
                    if PROPOSAL_DISPOSITIONS.contains(&k.as_str()) {
                        set_string(t, k, v)?;
                    } else if PROPOSAL_CAPS.contains(&k.as_str()) {
                        let n = v
                            .as_i64()
                            .ok_or_else(|| bad(format!("`{k}` must be a whole number")))?;
                        set_item(t, k, value(n));
                    } else if k == "introspection" {
                        let sub = table_mut(t, "introspection")?;
                        for (ik, iv) in obj(v, "introspection")? {
                            if !INTROSPECTION_KEYS.contains(&ik.as_str()) {
                                return Err(bad(format!(
                                    "unknown introspection key `{ik}`; use low_small, low_large, high_small or high_large"
                                )));
                            }
                            set_string(sub, ik, iv)?;
                        }
                    } else {
                        return Err(bad(format!("`proposals.{k}` cannot be changed here")));
                    }
                }
            }
            "schedules" => {
                for (name, v) in obj(body, "schedules")? {
                    let exists = doc
                        .get("schedule")
                        .and_then(|s| s.get(name.as_str()))
                        .is_some();
                    if !exists {
                        return Err(bad(format!(
                            "no schedule {name:?}; add it to the project file (the page changes existing ones)"
                        )));
                    }
                    let t = table_mut(table_mut(doc.as_table_mut(), "schedule")?, name)?;
                    for (k, kv) in obj(v, name)? {
                        if !SCHEDULE_KEYS.contains(&k.as_str()) {
                            return Err(bad(format!(
                                "`schedule.{name}.{k}` cannot be changed here (every, title)"
                            )));
                        }
                        set_string(t, k, kv)?;
                    }
                }
            }
            "tracker" => {
                if doc.get("tracker").is_none() {
                    return Err(bad(
                        "this project has no [tracker]; add its kind and repository to the project file",
                    ));
                }
                let t = table_mut(doc.as_table_mut(), "tracker")?;
                for (k, v) in obj(body, "tracker")? {
                    match k.as_str() {
                        "interval_s" => {
                            let n = v
                                .as_i64()
                                .ok_or_else(|| bad("`interval_s` must be a whole number"))?;
                            if n < 1 {
                                return Err(bad("`interval_s` must be at least 1"));
                            }
                            set_item(t, k, value(n));
                        }
                        "label" | "import_as" => set_string(t, k, v)?,
                        "require_label" => {
                            let b = v
                                .as_bool()
                                .ok_or_else(|| bad("`require_label` must be true or false"))?;
                            set_item(t, k, value(b));
                        }
                        r if TRACKER_REFS.contains(&r) => {
                            let s = v.as_str().unwrap_or_default();
                            if !(s.starts_with("env:") || s.starts_with("file:")) {
                                return Err(bad(format!(
                                    "`{r}` is a reference, `env:NAME` or `file:/path`, never the secret itself"
                                )));
                            }
                            set_string(t, k, v)?;
                        }
                        other => {
                            return Err(bad(format!("`tracker.{other}` cannot be changed here")));
                        }
                    }
                }
            }
            other => return Err(bad(format!("unknown settings section `{other}`"))),
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, text: &str) -> Result<(), Refused> {
    let tmp = path.with_extension("toml.settings-tmp");
    std::fs::write(&tmp, text).map_err(|e| (500, format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        (500, format!("{}: {e}", path.display()))
    })
}

/// Change a project's settings. The file is untouched unless the whole change validates and
/// the daemon accepts the reload.
pub fn set_project(d: &Daemon, name: &str, change: &Value) -> Result<Value, Refused> {
    set_project_as(
        d,
        name,
        change,
        &crate::projsettings::Caller::local(owner_of(d)),
    )
}

fn owner_of(d: &Daemon) -> &str {
    d.cfg.owner_names.first().map_or("", String::as_str)
}

/// `set_project` for a known caller. `{"set": {key: value|null}}` edits any catalogued key (null
/// unsets it); `"preview": true` returns the diff and writes nothing; a key that runs a program
/// or weakens a guard also needs `"confirm": true`, and is refused from a paired device.
pub fn set_project_as(
    d: &Daemon,
    name: &str,
    change: &Value,
    caller: &crate::projsettings::Caller,
) -> Result<Value, Refused> {
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = project_file(d, name);
    let old = std::fs::read_to_string(&path).map_err(|e| (404, format!("{name}: {e}")))?;
    let mut doc: DocumentMut = old.parse().map_err(|e| bad(format!("{name}: {e}")))?;
    let mut rest = change.clone();
    let (mut set, mut preview, mut confirm) = (Map::new(), false, false);
    if let Some(o) = rest.as_object_mut() {
        if let Some(Value::Object(s)) = o.remove("set") {
            set = s;
        }
        preview = o
            .remove("preview")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        confirm = o
            .remove("confirm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    }
    apply_project(&mut doc, &rest)?;
    let touched = crate::projsettings::apply_set(d, caller, &mut doc, &set)?;
    let new = doc.to_string();
    if new == old {
        return Ok(json!({"result": "unchanged", "changed": []}));
    }
    crate::projsettings::check_text(d, name, &new)?;
    let diff = crate::projsettings::diff(&old, &new);
    if preview {
        return Ok(json!({"result": "preview", "diff": diff}));
    }
    if !confirm && touched.iter().any(|k| k.sens.local_only()) {
        return Err((
            409,
            format!("this change runs a program or weakens a guard; send it again with confirm: true after reading the diff:\n{diff}"),
        ));
    }
    write_atomic(&path, &new)?;
    match d.reload_registration(name) {
        Ok(changed) => {
            if crate::projsettings::touches_overlay(&set) {
                if let Some(pid) = d.project_id_by_name(name) {
                    let _ = d.reload_definition(pid);
                }
            }
            Ok(json!({"result": "applied", "changed": changed, "diff": diff}))
        }
        Err(errors) => {
            write_atomic(&path, &old)?;
            Err(bad(errors.join("; ")))
        }
    }
}

// ---- notify.toml --------------------------------------------------------------------------

fn notify_path(d: &Daemon) -> std::path::PathBuf {
    d.cfg.config_dir.join("notify.toml")
}

/// Routes, quiet hours, reminders, the speech switch and the stall thresholds as the file
/// has them. Endpoints (email, push, webhook) show only whether they are configured.
pub fn get_notify(d: &Daemon) -> Value {
    let mut v = crate::notify::status(d);
    let doc: DocumentMut = std::fs::read_to_string(notify_path(d))
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    let section = |k: &str| -> Value {
        doc.get(k)
            .and_then(|s| s.as_table_like())
            .map(|t| {
                let mut m = Map::new();
                for (key, item) in t.iter() {
                    if let Some(x) = item.as_value() {
                        m.insert(
                            key.to_string(),
                            match x {
                                toml_edit::Value::Boolean(b) => json!(b.value()),
                                toml_edit::Value::Integer(i) => json!(i.value()),
                                other => json!(other.as_str().unwrap_or_default()),
                            },
                        );
                    }
                }
                Value::Object(m)
            })
            .unwrap_or_else(|| json!({}))
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("quiet".into(), section("quiet"));
        o.insert("reminder".into(), section("reminder"));
        o.insert("stall".into(), section("stall"));
        let mut sp = section("speech");
        if let Some(m) = sp.as_object_mut() {
            // The speech command runs a program, so it is not editable here and not echoed.
            m.remove("command");
        }
        o.insert("speech".into(), sp);
        o.insert("conditions".into(), json!(mason_core::notify::CONDITIONS));
        o.insert(
            "channels".into(),
            json!(["monitor", "sound", "speech", "push", "email", "webhook"]),
        );
    }
    v
}

fn apply_notify(doc: &mut DocumentMut, change: &Value) -> Result<(), Refused> {
    for (section, body) in obj(change, "settings")? {
        if section == "as" {
            continue;
        }
        if SECRET_NAMES.contains(&section.as_str()) {
            return Err(bad(format!(
                "`{section}` is a secret; the page never accepts one (use an env or file reference in notify.toml)"
            )));
        }
        match section.as_str() {
            "routing" => {
                for (cond, chans) in obj(body, "routing")? {
                    if mason_core::notify::MONITOR_ONLY.contains(&cond.as_str()) {
                        return Err(bad(format!(
                            "`{cond}` is monitor-only: it is shown on the monitor and has no routes to edit"
                        )));
                    }
                    if !mason_core::notify::CONDITIONS.contains(&cond.as_str()) {
                        return Err(bad(format!("unknown condition `{cond}`")));
                    }
                    let routing = table_mut(doc.as_table_mut(), "routing")?;
                    let t = table_mut(routing, cond)?;
                    for (ch, delay) in obj(chans, cond)? {
                        match delay {
                            Value::Null => {
                                t.remove(ch);
                            }
                            Value::String(s) => set_item(t, ch, value(s.as_str())),
                            Value::Number(n) if n.is_u64() => {
                                set_item(t, ch, value(n.as_i64().unwrap_or(-1)))
                            }
                            _ => {
                                return Err(bad(format!(
                                    "routing.{cond}.{ch} is a delay such as \"0\", \"5m\" or null"
                                )))
                            }
                        }
                    }
                }
            }
            "quiet" => {
                let t = table_mut(doc.as_table_mut(), "quiet")?;
                for (ch, w) in obj(body, "quiet")? {
                    match w {
                        Value::Null => {
                            t.remove(ch);
                        }
                        Value::String(s) => set_item(t, ch, value(s.as_str())),
                        _ => {
                            return Err(bad(format!("quiet.{ch} is a window like \"22:00-07:00\"")))
                        }
                    }
                }
            }
            "reminder" | "stall" => {
                let allowed: &[&str] = if section == "reminder" {
                    &["after", "repeat", "retry", "attempts"]
                } else {
                    &["after", "long_wait"]
                };
                let t = table_mut(doc.as_table_mut(), section)?;
                for (k, v) in obj(body, section)? {
                    if !allowed.contains(&k.as_str()) {
                        return Err(bad(format!("`{section}.{k}` cannot be changed here")));
                    }
                    match v {
                        Value::String(s) => set_item(t, k, value(s.as_str())),
                        Value::Number(n) if n.is_u64() => {
                            set_item(t, k, value(n.as_i64().unwrap_or(-1)))
                        }
                        _ => return Err(bad(format!("`{section}.{k}` is a time like \"10m\""))),
                    }
                }
            }
            "speech" => {
                let t = table_mut(doc.as_table_mut(), "speech")?;
                for (k, v) in obj(body, "speech")? {
                    match (k.as_str(), v) {
                        ("daemon", Value::Bool(b)) => set_item(t, k, value(*b)),
                        ("stale", Value::String(s)) => set_item(t, k, value(s.as_str())),
                        ("preferred_client", Value::String(s)) => set_item(t, k, value(s.as_str())),
                        ("preferred_client", Value::Null) => {
                            t.remove(k);
                        }
                        _ => {
                            return Err(bad(format!(
                            "`speech.{k}` cannot be changed here (daemon, stale, preferred_client)"
                        )))
                        }
                    }
                }
            }
            other => {
                return Err(bad(format!(
                    "`{other}` cannot be changed here (routing, quiet, reminder, speech, stall)"
                )))
            }
        }
    }
    Ok(())
}

/// Change `notify.toml`. The next notify tick reads it, so there is nothing to reload.
pub fn set_notify(d: &Daemon, change: &Value) -> Result<Value, Refused> {
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = notify_path(d);
    let old = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err((500, format!("notify.toml: {e}"))),
    };
    let mut doc: DocumentMut = old.parse().map_err(|e| bad(format!("notify.toml: {e}")))?;
    apply_notify(&mut doc, change)?;
    let new = doc.to_string();
    if new == old {
        return Ok(json!({"result": "unchanged"}));
    }
    crate::notify::parse(&new).map_err(bad)?;
    write_atomic(&path, &new)?;
    Ok(json!({"result": "applied"}))
}
