// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason adopt`: register a project that already has a `.mason/` folder, as it is.
//! It writes the project overlay and nothing else: `.mason/` is never written, so adopting
//! cannot overwrite a pipeline someone built. A repository with no `.mason/` is sent to
//! `mason init`, which makes one.

use crate::init::{self, detect};
use mason_onboard::adopt::{adopt, Args};
use std::path::Path;

pub fn run(mut args: Vec<String>, config_dir: &Path) -> i32 {
    let take = |name: &str, args: &mut Vec<String>| -> Result<Option<String>, String> {
        match args.iter().position(|a| a == name) {
            None => Ok(None),
            Some(i) => {
                args.remove(i);
                if i < args.len() {
                    Ok(Some(args.remove(i)))
                } else {
                    Err(format!("{name} needs a value"))
                }
            }
        }
    };
    let (id, base, test, bin) = match (
        take("--id", &mut args),
        take("--base", &mut args),
        take("--test", &mut args),
        take("--agent-bin", &mut args),
    ) {
        (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
        (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (.., Err(e)) => return fail(&e),
    };
    if let Some(extra) = args.first() {
        return fail(&format!("unexpected argument {extra:?}"));
    }
    let d = match detect::detect(&std::env::current_dir().unwrap_or_default()) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    let args = Args {
        id,
        base,
        test,
        agent_bin: bin,
        placement: None,
    };
    let done = match adopt(&d, &args, config_dir, &init::validate_overlay) {
        Ok(a) => a,
        Err(e) => {
            e.lines.iter().for_each(|l| eprintln!("{l}"));
            return fail(&e.message);
        }
    };
    println!(
        "Adopted {} as project {:?} ({}).",
        d.root.display(),
        done.id,
        done.overlay.display()
    );
    println!(
        "  .mason/ was not touched. Base branch {}, landing `{}`.",
        done.base,
        done.landing.mode()
    );
    if done.test_is_true {
        println!("  warning: its test step names no suites and no --test was given, so the test command is `true`. Edit `test` in the overlay.");
    }
    println!("  Restart masond so it reads the new project, then `mason doctor`.");
    0
}

fn fail(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    1
}
