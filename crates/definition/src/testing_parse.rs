// SPDX-License-Identifier: GPL-3.0-or-later

//! The testing tables of `.mason/mason.toml`: `[suite.*]`, `[environment.*]`,
//! `[binding.*]`, `[impact]` and `[signature.*]` (Part 8).

use crate::model::{parse_duration, Problem};
use crate::parse::unknown_key;
use mason_testing::{
    Binding, DeviceType, EnvKind, Environment, ImpactMap, ParserKind, Signature, Suite, Testing,
    When,
};
use std::collections::BTreeMap;
use toml_edit::{DocumentMut, Item};

pub const SUITE_KEYS: &[&str] = &[
    "command",
    "parser",
    "min_tests",
    "level",
    "uses",
    "env",
    "results_file",
    "rerun",
    "pass_regex",
    "fail_regex",
    "timeout",
    "artifacts",
    "progress",
    "test_paths",
    "pty",
];
const ENV_KEYS: &[&str] = &[
    "type",
    "setup",
    "health",
    "teardown",
    "file",
    "os",
    "uses",
    "health_timeout",
];

fn string(item: &Item, path: &str, out: &mut Vec<Problem>) -> Option<String> {
    match item.as_str() {
        Some(s) => Some(s.to_string()),
        None => {
            out.push(Problem::new(path, "must be a string"));
            None
        }
    }
}

fn strings(item: &Item, path: &str, out: &mut Vec<Problem>) -> Vec<String> {
    let Some(a) = item.as_array() else {
        out.push(Problem::new(path, "must be a list of strings"));
        return vec![];
    };
    a.iter()
        .enumerate()
        .filter_map(|(i, x)| match x.as_str() {
            Some(s) => Some(s.to_string()),
            None => {
                out.push(Problem::new(format!("{path}[{i}]"), "must be a string"));
                None
            }
        })
        .collect()
}

fn duration(item: &Item, path: &str, out: &mut Vec<Problem>) -> Option<std::time::Duration> {
    let s = string(item, path, out)?;
    let d = parse_duration(&s);
    if d.is_none() {
        out.push(Problem::new(
            path,
            format!("`{s}` is not a duration; use forms like \"20m\" or \"90s\""),
        ));
    }
    d
}

fn uses(item: &Item, path: &str, out: &mut Vec<Problem>) -> BTreeMap<String, u32> {
    let mut m = BTreeMap::new();
    match item.as_table_like() {
        Some(t) => {
            for (r, n) in t.iter() {
                match n.as_integer() {
                    Some(n) if n >= 1 => {
                        m.insert(r.to_string(), n as u32);
                    }
                    _ => out.push(Problem::new(
                        format!("{path}.{r}"),
                        "must be a whole number, 1 or more",
                    )),
                }
            }
        }
        None => out.push(Problem::new(path, "must be a table like { compile = 1 }")),
    }
    m
}

fn tables<'a>(doc: &'a DocumentMut, key: &str, out: &mut Vec<Problem>) -> Vec<(&'a str, &'a Item)> {
    match doc.get(key) {
        None => vec![],
        Some(v) => match v.as_table_like() {
            Some(t) => t.iter().collect(),
            None => {
                out.push(Problem::new(
                    key,
                    format!("must be a table, like [{key}.name]"),
                ));
                vec![]
            }
        },
    }
}

pub fn parse(doc: &DocumentMut, out: &mut Vec<Problem>) -> Testing {
    let mut t = Testing::default();

    for (name, item) in tables(doc, "suite", out) {
        let base = format!("suite.{name}");
        let Some(tbl) = item.as_table_like() else {
            out.push(Problem::new(&base, "must be a table"));
            continue;
        };
        let mut s = Suite::new(name, "", ParserKind::Junit);
        let (mut have_cmd, mut have_parser) = (false, false);
        for (k, v) in tbl.iter() {
            let path = format!("{base}.{k}");
            match k {
                "command" => {
                    if let Some(c) = string(v, &path, out) {
                        s.command = c;
                        have_cmd = true;
                    }
                }
                "parser" => {
                    if let Some(p) = string(v, &path, out) {
                        match ParserKind::from_name(&p) {
                            Some(k) => {
                                s.parser = k;
                                have_parser = true;
                            }
                            None => out.push(Problem::new(
                                &path,
                                format!(
                                    "unknown parser `{p}`; the built-in parsers are {}",
                                    ParserKind::ALL.map(ParserKind::name).join(", ")
                                ),
                            )),
                        }
                    }
                }
                "min_tests" => match v.as_integer() {
                    Some(n) if (0..=i64::from(u32::MAX)).contains(&n) => s.min_tests = n as u32,
                    _ => out.push(Problem::new(&path, "must be a whole number")),
                },
                "level" => s.level = string(v, &path, out).unwrap_or_default(),
                "uses" => s.uses = uses(v, &path, out),
                "env" => s.env = string(v, &path, out).unwrap_or_default(),
                "results_file" => s.results_file = string(v, &path, out),
                "rerun" => s.rerun = string(v, &path, out),
                "pass_regex" => s.pass_regex = string(v, &path, out),
                "fail_regex" => s.fail_regex = string(v, &path, out),
                "timeout" => s.timeout = duration(v, &path, out),
                "artifacts" => s.artifacts = strings(v, &path, out),
                "progress" => s.progress = string(v, &path, out),
                "test_paths" => s.test_paths = strings(v, &path, out),
                "pty" => match v.as_bool() {
                    Some(b) => s.pty = b,
                    None => out.push(Problem::new(&path, "must be true or false")),
                },
                other => out.push(unknown_key(&base, other, SUITE_KEYS)),
            }
        }
        if !have_cmd {
            out.push(Problem::new(format!("{base}.command"), "required"));
        }
        if !have_parser {
            out.push(Problem::new(
                format!("{base}.parser"),
                "required: how its output is read, so a pass can be proven",
            ));
        }
        t.suites.push(s);
    }

    for (name, item) in tables(doc, "environment", out) {
        let base = format!("environment.{name}");
        let Some(tbl) = item.as_table_like() else {
            out.push(Problem::new(&base, "must be a table"));
            continue;
        };
        let mut e = Environment::local(name);
        for (k, v) in tbl.iter() {
            let path = format!("{base}.{k}");
            match k {
                "type" => {
                    if let Some(s) = string(v, &path, out) {
                        match s.as_str() {
                            "local" => e.kind = EnvKind::Local,
                            "compose" => e.kind = EnvKind::Compose,
                            other => match DeviceType::from_name(other) {
                                Some(t) => e.kind = EnvKind::Device(t),
                                None => out.push(Problem::new(
                                    &path,
                                    "must be \"local\", \"compose\", \"ios-simulator\", \"android-emulator\", \"ios-device\" or \"android-device\"",
                                )),
                            },
                        }
                    }
                }
                "setup" => e.setup = string(v, &path, out),
                "health" => e.health = string(v, &path, out),
                "teardown" => e.teardown = string(v, &path, out),
                "file" => e.file = string(v, &path, out),
                "os" => e.os = string(v, &path, out),
                "uses" => e.uses = uses(v, &path, out),
                "health_timeout" => {
                    if let Some(d) = duration(v, &path, out) {
                        e.health_timeout = d;
                    }
                }
                other => out.push(unknown_key(&base, other, ENV_KEYS)),
            }
        }
        t.environments.push(e);
    }

    for (level, item) in tables(doc, "binding", out) {
        let base = format!("binding.{level}");
        let Some(tbl) = item.as_table_like() else {
            out.push(Problem::new(&base, "must be a table"));
            continue;
        };
        let mut when = std::collections::BTreeSet::new();
        for (k, v) in tbl.iter() {
            let path = format!("{base}.{k}");
            if k != "when" {
                out.push(unknown_key(&base, k, &["when"]));
                continue;
            }
            for w in strings(v, &path, out) {
                match When::from_name(&w) {
                    Some(x) => {
                        when.insert(x);
                    }
                    None => out.push(Problem::new(
                        &path,
                        format!("unknown `{w}`; use development, test_step, batch, after_landing or schedule"),
                    )),
                }
            }
        }
        t.bindings.push(Binding {
            level: level.to_string(),
            when,
        });
    }

    if let Some(v) = doc.get("impact") {
        match v.as_table_like() {
            Some(tbl) => {
                let mut entries = vec![];
                for (glob, item) in tbl.iter() {
                    entries.push((
                        glob.to_string(),
                        strings(item, &format!("impact.{glob}"), out),
                    ));
                }
                t.impact = ImpactMap { entries };
            }
            None => out.push(Problem::new(
                "impact",
                "must be a table of path globs to suite lists",
            )),
        }
    }

    for (name, item) in tables(doc, "signature", out) {
        let base = format!("signature.{name}");
        let Some(tbl) = item.as_table_like() else {
            out.push(Problem::new(&base, "must be a table"));
            continue;
        };
        let (mut pattern, mut advice) = (None, String::new());
        for (k, v) in tbl.iter() {
            let path = format!("{base}.{k}");
            match k {
                "pattern" => pattern = string(v, &path, out),
                "advice" => advice = string(v, &path, out).unwrap_or_default(),
                other => out.push(unknown_key(&base, other, &["pattern", "advice"])),
            }
        }
        match pattern {
            Some(p) => {
                if let Err(e) = regex_ok(&p) {
                    out.push(Problem::new(format!("{base}.pattern"), e));
                }
                t.signatures.push(Signature {
                    name: name.to_string(),
                    pattern: p,
                    advice,
                });
            }
            None => out.push(Problem::new(format!("{base}.pattern"), "required")),
        }
    }
    mason_testing::signatures::add_device_signatures(&mut t);
    t
}

/// Is this a pattern the regex engine accepts?
pub fn regex_ok(p: &str) -> Result<(), String> {
    mason_testing::parse::check_regex(p)
}
