// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason remove`: take MASON out of a project, cleanly. Deletes the project overlay and
//! `.mason/`, and removes the "Filing work" notes only if they are exactly what `mason init`
//! wrote. Never touches tracker tasks, task branches, the setup branch, or machine-wide files,
//! and says so. Without `--yes` it only lists what it would do.

use crate::init::{self, detect, questions};
use std::path::{Path, PathBuf};

/// The overlay files whose `path` is this repository.
pub fn overlays_for(config_dir: &Path, root: &Path) -> Vec<(String, PathBuf)> {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = canon(root);
    let mut out = vec![];
    for e in std::fs::read_dir(config_dir.join("projects"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "toml") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let path = text
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .and_then(|d| d.get("path").and_then(|v| v.as_str()).map(PathBuf::from));
        if path.is_some_and(|q| canon(&q) == root) {
            out.push((
                p.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                p,
            ));
        }
    }
    out
}

pub fn run(mut args: Vec<String>, config_dir: &Path) -> i32 {
    let yes = args
        .iter()
        .position(|a| a == "--yes" || a == "-y")
        .map(|i| args.remove(i))
        .is_some();
    let id = match args.iter().position(|a| a == "--id") {
        Some(i) if i + 1 < args.len() => {
            args.remove(i);
            Some(args.remove(i))
        }
        Some(_) => return fail("--id needs a value"),
        None => None,
    };
    if let Some(extra) = args.first() {
        return fail(&format!("unexpected argument {extra:?}"));
    }
    let d = match detect::detect(&std::env::current_dir().unwrap_or_default()) {
        Ok(d) => d,
        Err(e) => return fail(&e),
    };
    let overlays: Vec<_> = overlays_for(config_dir, &d.root)
        .into_iter()
        .filter(|(n, _)| id.as_ref().is_none_or(|i| i == n))
        .collect();
    let mason_dir = d.root.join(".mason");
    // The Filing section, where it is exactly ours.
    let mut notes = vec![];
    for f in init::FILING_FILES {
        let p = d.root.join(f);
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        match questions::without_filing(&text) {
            Some(rest) => notes.push((p, Some(rest))),
            None if text
                .lines()
                .any(|l| l.trim() == "## Filing work for the pipeline") =>
            {
                notes.push((p, None))
            }
            None => {}
        }
    }
    if overlays.is_empty() && !mason_dir.exists() && notes.is_empty() {
        println!(
            "Nothing to remove: no .mason/, no registered project and no Filing notes for {}.",
            d.root.display()
        );
        return 0;
    }
    println!(
        "{} MASON from {}:",
        if yes { "Removing" } else { "Would remove" },
        d.root.display()
    );
    for (n, p) in &overlays {
        println!("  project {n}: {}", p.display());
    }
    if mason_dir.exists() {
        println!("  {}", mason_dir.display());
    }
    for (p, rest) in &notes {
        match rest {
            Some(r) if r.trim().is_empty() => println!(
                "  {} (only the Filing notes were in it, so the file goes)",
                p.display()
            ),
            Some(_) => println!("  the Filing notes in {}", p.display()),
            None => println!(
                "  kept: the Filing section in {} was edited, so it is yours",
                p.display()
            ),
        }
    }
    if !yes {
        println!("Nothing was changed. Run `mason remove --yes` to do it.");
        return 2;
    }
    for (_, p) in &overlays {
        if let Err(e) = std::fs::remove_file(p) {
            return fail(&format!("{}: {e}", p.display()));
        }
    }
    if mason_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&mason_dir) {
            return fail(&format!("{}: {e}", mason_dir.display()));
        }
    }
    for (p, rest) in notes {
        let Some(rest) = rest else { continue };
        let r = if rest.trim().is_empty() {
            std::fs::remove_file(&p)
        } else {
            std::fs::write(&p, rest)
        };
        if let Err(e) = r {
            return fail(&format!("{}: {e}", p.display()));
        }
    }
    println!("Done. Left alone, on purpose:");
    println!("  tracker tasks and issues (they are yours; MASON only mirrors them)");
    println!("  git branches (mason/setup, mason/<project>/t*): delete them with `git branch -D` if you want them gone");
    println!(
        "  machine-wide settings (resources.toml, notify.toml, providers.toml) in {}",
        config_dir.display()
    );
    println!("  the deletion of a tracked .mason/ is in your working tree: commit it");
    println!("A running masond keeps the project loaded until it restarts.");
    0
}

fn fail(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    1
}
