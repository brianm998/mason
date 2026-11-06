// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason definition check` and `dry-run`: look at a pipeline definition
//! without applying it, and without a daemon.

use mason_definition::dryrun::{default_scenarios, dry_run, parse_scenarios, End, Trace};
use mason_definition::graph::Outcome;
use mason_definition::{parse, validate, Capabilities};
use std::path::Path;

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn describe(t: &Trace) -> String {
    let path: Vec<String> = t
        .steps
        .iter()
        .map(|s| {
            if s.outcome == Outcome::Green {
                s.step.clone()
            } else {
                format!("{} ({})", s.step, s.outcome.name())
            }
        })
        .collect();
    let end = match &t.end {
        End::Landed => "landed".to_string(),
        End::Failed(why) => format!("failed: {why}"),
    };
    format!("{}  =>  {end}", path.join(" > "))
}

/// Validate a definition file: the language, then what this version can run,
/// then a dry run of the default scenarios. Returns the exit status.
pub fn check(path: &Path) -> i32 {
    let text = match read(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let def = match parse(&text) {
        Ok(d) => d,
        Err(problems) => {
            problems.iter().for_each(|p| println!("error  {p}"));
            return 1;
        }
    };
    let language = validate(&def, &Capabilities::everything());
    language.iter().for_each(|p| println!("error  {p}"));
    let runnable = validate(&def, &Capabilities::now());
    for p in runnable.iter().filter(|p| !language.contains(p)) {
        println!("cannot run yet  {p}");
    }
    if !language.is_empty() || !runnable.is_empty() {
        return 1;
    }
    println!(
        "ok  {} steps, definition {}",
        def.steps.len(),
        &def.hash[..12]
    );
    let mut status = 0;
    for sc in default_scenarios(&def) {
        match dry_run(&def, &sc) {
            Ok(t) => println!("    {:<45} {}", sc.name, describe(&t)),
            Err(e) => {
                println!("error  dry run `{}`: {e}", sc.name);
                status = 1;
            }
        }
    }
    status
}

/// Walk scripted tasks through a definition. With `scenarios`, those from the
/// file; otherwise the default ones.
pub fn dry(path: &Path, scenarios: Option<&Path>) -> i32 {
    let go = || -> Result<(), String> {
        let def = parse(&read(path)?).map_err(|e| {
            e.iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        let problems = validate(&def, &Capabilities::now());
        if !problems.is_empty() {
            return Err(problems
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let list = match scenarios {
            Some(p) => parse_scenarios(&read(p)?)?,
            None => default_scenarios(&def),
        };
        for sc in list {
            let t = dry_run(&def, &sc)?;
            println!(
                "{}\n    started by {}: {}",
                sc.name,
                t.route.name(),
                describe(&t)
            );
        }
        Ok(())
    };
    match go() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
