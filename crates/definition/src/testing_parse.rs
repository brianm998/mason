// SPDX-License-Identifier: GPL-3.0-or-later

//! The testing tables of `.mason/mason.toml`: `[suite.*]`, `[environment.*]`,
//! `[binding.*]`, `[impact]` and `[signature.*]` (Part 8).

use crate::model::{parse_duration, Problem};
use crate::parse::unknown_key;
use mason_testing::{
    Binding, DeviceType, EnvKind, Environment, ImpactMap, Narrow, ParserKind, Signature, Suite,
    Testing, When,
};
use std::collections::BTreeMap;
use toml_edit::{DocumentMut, Item};

/// GitHub allows 256 jobs in a matrix; a suite is split far less than that.
pub const MAX_SHARDS: i64 = 64;

pub const SUITE_KEYS: &[&str] = &[
    "command",
    "build",
    "parser",
    "min_tests",
    "level",
    "uses",
    "env",
    "results_file",
    "rerun",
    "dev_command",
    "dev_build",
    "fast_command",
    "pass_regex",
    "fail_regex",
    "timeout",
    "artifacts",
    "progress",
    "test_paths",
    "pty",
    "narrow_command",
    "narrow_tests",
    "narrow_root",
    "narrow_split",
    "narrow_each",
    "narrow_never",
    "narrow_packages",
    "narrow_smoke",
    "whole_later",
    "resume_after_loss",
    "remote",
    "runs_on",
    "shards",
    "shard_command",
    "dev_shard_command",
    "ci_jobs",
];
const ENV_KEYS: &[&str] = &[
    "type",
    "setup",
    "health",
    "teardown",
    "file",
    "os",
    "name",
    "exclude",
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

/// The extension named by `plugin:<name>`, if that is what the value says.
fn plugin_name(v: &str) -> Option<&str> {
    v.strip_prefix("plugin:").filter(|n| {
        n.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && n.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    })
}

/// A suite's `narrow_*` keys as one [`Narrow`] (s386), or `None` when it has none. Any of them
/// without `narrow_command` and `narrow_tests`, a command without `{tests}`, an `each` without
/// `{}` or a `split` that is no regex is refused.
fn narrow(
    base: &str,
    [command, tests, root, split, each]: [Option<String>; 5],
    never: Vec<String>,
    smoke: Vec<String>,
    out: &mut Vec<Problem>,
) -> Option<Narrow> {
    let any = command.is_some()
        || tests.is_some()
        || root.is_some()
        || split.is_some()
        || each.is_some()
        || !never.is_empty();
    if !any && !smoke.is_empty() {
        out.push(Problem::new(
            format!("{base}.narrow_smoke"),
            "needs `narrow_command` and `narrow_tests`: it names units of a narrowed run",
        ));
    }
    if !any {
        return None;
    }
    let (Some(command), Some(tests)) = (command, tests) else {
        out.push(Problem::new(
            format!("{base}.narrow_command"),
            "narrowing needs both `narrow_command` and `narrow_tests`",
        ));
        return None;
    };
    if !command.contains("{tests}") {
        out.push(Problem::new(
            format!("{base}.narrow_command"),
            "must contain `{tests}`, which becomes the picked tests",
        ));
        return None;
    }
    let each = each.unwrap_or_else(|| "{}".into());
    if !["{}", "{file}", "{path}"].iter().any(|k| each.contains(k)) {
        out.push(Problem::new(
            format!("{base}.narrow_each"),
            "must contain `{}` (one test's name), `{file}` (its file under narrow_root) or `{path}` (its file from the repository root)",
        ));
        return None;
    }
    if let Some(Err(e)) = split.as_deref().map(mason_testing::parse::check_regex) {
        out.push(Problem::new(format!("{base}.narrow_split"), e));
        return None;
    }
    Some(Narrow {
        tests,
        root,
        split,
        command,
        each,
        never,
        smoke,
    })
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
        // The `narrow_*` keys (s386), put together once the table is read.
        let mut nw: [Option<String>; 5] = Default::default();
        let mut never: Vec<String> = vec![];
        let mut smoke: Vec<String> = vec![];
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
                            None if plugin_name(&p).is_some() => {
                                s.plugin_parser = plugin_name(&p).map(String::from);
                                have_parser = true;
                            }
                            Some(k) => {
                                s.parser = k;
                                have_parser = true;
                            }
                            None => out.push(Problem::new(
                                &path,
                                format!(
                                    "unknown parser `{p}`; the built-in parsers are {}, or `plugin:<name>` for a parser an approved extension provides",
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
                "build" => s.build = string(v, &path, out),
                "level" => s.level = string(v, &path, out).unwrap_or_default(),
                "uses" => s.uses = uses(v, &path, out),
                "env" => s.env = string(v, &path, out).unwrap_or_default(),
                "results_file" => s.results_file = string(v, &path, out),
                "rerun" => s.rerun = string(v, &path, out),
                "skip" => s.skip = string(v, &path, out),
                "dev_command" => s.dev_command = string(v, &path, out),
                "dev_build" => s.dev_build = string(v, &path, out),
                "fast_command" => s.fast_command = string(v, &path, out),
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
                "narrow_packages" => match v.as_bool() {
                    Some(b) => s.narrow_packages = b,
                    None => out.push(Problem::new(&path, "must be true or false")),
                },
                "remote" => match v.as_bool() {
                    Some(b) => s.remote = b,
                    None => out.push(Problem::new(&path, "must be true or false")),
                },
                "shards" => match v.as_integer() {
                    Some(n) if (1..=MAX_SHARDS).contains(&n) => s.shards = n as u32,
                    _ => out.push(Problem::new(
                        &path,
                        format!("must be a whole number from 1 to {MAX_SHARDS}"),
                    )),
                },
                "shard_command" => s.shard_command = string(v, &path, out),
                "dev_shard_command" => s.dev_shard_command = string(v, &path, out),
                "ci_jobs" => s.ci_jobs = strings(v, &path, out),
                "runs_on" => {
                    if let Some(r) = string(v, &path, out) {
                        if r.trim().is_empty() {
                            out.push(Problem::new(
                                &path,
                                "must name a runner, like \"macos-15\"; leave it out to use the project's [remote_runs] runs_on",
                            ));
                        } else {
                            s.runs_on = Some(r);
                        }
                    }
                }
                "resume_after_loss" => match v.as_bool() {
                    Some(b) => s.resume_after_loss = b,
                    None => out.push(Problem::new(&path, "must be true or false")),
                },
                "narrow_command" => nw[0] = string(v, &path, out),
                "narrow_tests" => nw[1] = string(v, &path, out),
                "narrow_root" => nw[2] = string(v, &path, out),
                "narrow_split" => nw[3] = string(v, &path, out),
                "narrow_each" => nw[4] = string(v, &path, out),
                "narrow_never" => never = strings(v, &path, out),
                "narrow_smoke" => smoke = strings(v, &path, out),
                "whole_later" => {
                    if let Some(w) = string(v, &path, out) {
                        match mason_testing::WholeLater::from_name(&w) {
                            Some(w) => s.whole_later = Some(w),
                            None => out.push(Problem::new(
                                &path,
                                "must be \"batch\" (a landing tier runs the suite whole) or \"ci\" (the base's CI does)",
                            )),
                        }
                    }
                }
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
        if s.shards > 1 && s.shard_command.is_none() {
            out.push(Problem::new(
                format!("{base}.shards"),
                "needs `shard_command`, what each runner runs for its share of the tests (it reads \
                 `.mason/run.json` and `MASON_SHARD`, as scripts/test-shard.py does)",
            ));
        }
        s.narrow = narrow(&base, nw, never, smoke, out);
        if s.narrow_packages && s.dev_command.is_none() {
            out.push(Problem::new(
                format!("{base}.narrow_packages"),
                "needs `dev_command`, the command that takes `{packages}`",
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
                            other if plugin_name(other).is_some() => {
                                e.plugin = plugin_name(other).map(String::from)
                            }
                            other => match DeviceType::from_name(other) {
                                Some(t) => e.kind = EnvKind::Device(t),
                                None => out.push(Problem::new(
                                    &path,
                                    "must be \"local\", \"compose\", \"ios-simulator\", \"android-emulator\", \"ios-device\" or \"android-device\" (or \"plugin:<name>\" for an environment an approved extension provides)",
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
                "name" => e.device_name = string(v, &path, out),
                "exclude" => e.exclude = strings(v, &path, out),
                "uses" => e.uses = uses(v, &path, out),
                "health_timeout" => {
                    if let Some(d) = duration(v, &path, out) {
                        e.health_timeout = d;
                    }
                }
                other => out.push(unknown_key(&base, other, ENV_KEYS)),
            }
        }
        if !matches!(e.kind, EnvKind::Device(_)) {
            for (k, set) in [
                ("name", e.device_name.is_some()),
                ("exclude", !e.exclude.is_empty()),
            ] {
                if set {
                    out.push(Problem::new(
                        format!("{base}.{k}"),
                        "applies only to a device environment (ios-simulator, android-emulator, ios-device, android-device)",
                    ));
                }
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
                let mut overrides = vec![];
                for (glob, item) in tbl.iter() {
                    let path = format!("impact.{glob}");
                    if let Some(o) = item.as_table_like() {
                        let (mut suites, mut flag) = (None, None);
                        for (k, v) in o.iter() {
                            match k {
                                "suites" => suites = Some(strings(v, &format!("{path}.suites"), out)),
                                "override" => match v.as_bool() {
                                    Some(b) => flag = Some(b),
                                    None => out.push(Problem::new(
                                        format!("{path}.override"),
                                        "must be true or false",
                                    )),
                                },
                                other => out.push(unknown_key(&path, other, &["suites", "override"])),
                            }
                        }
                        match suites {
                            None => out.push(Problem::new(format!("{path}.suites"), "required")),
                            Some(s) if flag == Some(true) => overrides.push((glob.to_string(), s)),
                            Some(s) => entries.push((glob.to_string(), s)),
                        }
                    } else {
                        entries.push((glob.to_string(), strings(item, &path, out)));
                    }
                }
                t.impact = ImpactMap { entries, overrides };
            }
            None => out.push(Problem::new(
                "impact",
                "must be a table of path globs to suite lists or `{ suites = [...], override = true }`",
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
