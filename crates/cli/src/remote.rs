// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason remote setup --project P [--runs-on L]`: write `.github/workflows/mason-run.yml` on a
//! `mason/setup-remote` branch, from the template MASON ships, with the setup block filled in
//! from what the project has (#703). Nothing is pushed and the checked-out branch and worktree
//! are not touched: the file is written in a temporary worktree, as `mason init` does.

use mason_daemon::config::load_projects;
use mason_landing::git;
use std::path::{Path, PathBuf};

pub const TEMPLATE: &str = include_str!("../../../templates/github/mason-run.yml");
pub const BRANCH: &str = "mason/setup-remote";
const MARK: &str = "      # setup: the project's toolchains and caches go here (see above).";

/// What was found in the project and what to put in the workflow's setup block.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// The steps, indented for the `run` job.
    pub steps: String,
    /// The runner label the project needs.
    pub runs_on: String,
    /// What the owner should look at before landing it.
    pub review: Vec<String>,
}

/// Directories to look in for build files: the root and up to two levels below it, with hidden
/// and build-output folders skipped, in name order.
fn dirs_to_search(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut at = 0;
    while at < out.len() {
        let depth = out[at]
            .strip_prefix(root)
            .map_or(0, |r| r.components().count());
        if depth < 3 {
            let mut kids: Vec<PathBuf> = std::fs::read_dir(&out[at])
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_dir()
                        && !p.file_name().is_some_and(|n| {
                            let n = n.to_string_lossy();
                            n.starts_with('.')
                                || matches!(&*n, "target" | "node_modules" | "build" | "Pods")
                                || n.ends_with(".xcodeproj")
                        })
                })
                .collect();
            kids.sort();
            out.extend(kids);
        }
        at += 1;
    }
    out
}

/// `Xcode_16.4`, `Xcode-16.4` or `Xcode 16.4` in a command: `16.4`.
pub fn xcode_version(cmd: &str) -> Option<String> {
    let at = cmd.find("Xcode")?;
    let rest = &cmd[at + "Xcode".len()..];
    let rest = rest.strip_prefix(['_', '-', ' '])?;
    let v: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let v = v.trim_end_matches('.').to_string();
    (!v.is_empty()).then_some(v)
}

/// The `channel` and `components` of a `rust-toolchain.toml`.
fn toolchain(text: &str) -> (Option<String>, Vec<String>) {
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return (None, vec![]);
    };
    let t = doc.get("toolchain");
    let channel = t
        .and_then(|t| t.get("channel"))
        .and_then(|c| c.as_str())
        .map(String::from);
    let comps: Vec<String> = t
        .and_then(|t| t.get("components"))
        .and_then(|c| c.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    (channel, comps)
}

/// Look at the tree and the project's test command and decide the setup block.
pub fn plan(root: &Path, test_cmd: &str, runs_on: Option<&str>) -> Plan {
    let dirs = dirs_to_search(root);
    let exists = |name: &str| dirs.iter().any(|d| d.join(name).is_file());
    let rust = root.join("Cargo.toml").is_file();
    let swift = exists("Package.swift")
        || dirs.iter().any(|d| {
            std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "xcodeproj"))
        });
    let gradle = [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ]
    .iter()
    .any(|n| exists(n));

    let mut steps = String::new();
    let mut review = vec![];
    let mut runner = "ubuntu-latest".to_string();
    if rust {
        let (channel, comps) = std::fs::read_to_string(root.join("rust-toolchain.toml"))
            .map(|t| toolchain(&t))
            .unwrap_or_default();
        let channel = channel.unwrap_or_else(|| {
            review.push("Cargo.toml but no rust-toolchain.toml: the workflow uses `stable`. Pin a toolchain if the project needs one.".into());
            "stable".into()
        });
        steps.push_str(&format!(
            "      - uses: dtolnay/rust-toolchain@master\n        with:\n          toolchain: {channel}\n"
        ));
        if !comps.is_empty() {
            steps.push_str(&format!("          components: {}\n", comps.join(", ")));
        }
        steps.push_str(
            "      - uses: Swatinem/rust-cache@v2\n        with:\n          cache-on-failure: true\n",
        );
    }
    if swift {
        runner = "macos-15".into();
        match xcode_version(test_cmd) {
            Some(v) => steps.push_str(&format!(
                "      - uses: maxim-lobanov/setup-xcode@v1\n        if: runner.os == 'macOS'\n        with:\n          xcode-version: '{v}'\n"
            )),
            None => review.push("the project's test command names no Xcode version, so the runner's default Xcode is used. Name one (for example `DEVELOPER_DIR=/Applications/Xcode_16.4.app`) and set it up again if the project needs a particular one.".into()),
        }
    }
    if gradle {
        steps.push_str(
            "      - uses: actions/setup-java@v4\n        with:\n          distribution: temurin\n          java-version: '17'\n      - uses: gradle/actions/setup-gradle@v4\n",
        );
        review.push("the workflow sets up Java 17; change `java-version` if the project builds with another.".into());
    }
    if steps.is_empty() {
        steps.push_str("      # No Cargo.toml, Package.swift, .xcodeproj or Gradle build was found: add the toolchains the tests need here.\n");
        review.push("nothing was recognised, so the setup block is empty. Add the toolchains and caches the suite needs.".into());
    }
    let runs_on = runs_on.map(String::from).unwrap_or(runner);
    Plan {
        steps,
        runs_on,
        review,
    }
}

/// The template with the setup block filled in.
pub fn render(steps: &str) -> String {
    TEMPLATE.replacen(MARK, steps.trim_end_matches('\n'), 1)
}

/// Every `mason remote` subcommand, for the unknown-subcommand errors.
pub const COMMANDS: &str = "the remote commands are `setup`, `on`, `off`, `status` and `runs`";

/// The runs on GitHub, one line each, as the monitor's GitHub section lists them (#767).
pub fn print_runs(r: &serde_json::Value) {
    if r["enabled"].as_bool() != Some(true) {
        println!("No project runs on GitHub.");
        return;
    }
    let runs = r["runs"].as_array().cloned().unwrap_or_default();
    println!(
        "MASON {} of {} runners, {} waiting",
        r["poolUsed"].as_u64().unwrap_or(0),
        r["poolLimit"].as_u64().unwrap_or(0),
        r["waiting"].as_u64().unwrap_or(0)
    );
    if runs.is_empty() {
        println!("Nothing on GitHub.");
    }
    for x in runs {
        let s = |k: &str| x[k].as_str().unwrap_or("");
        println!(
            "{:<10} {:<14} {:<9} {:<14} {}{}{}",
            s("owner"),
            s("taskState"),
            s("state"),
            s("kind"),
            s("suite"),
            if s("progress").is_empty() {
                String::new()
            } else {
                format!("  {}", s("progress"))
            },
            if s("url").is_empty() {
                String::new()
            } else {
                format!("  {}", s("url"))
            }
        );
    }
}

fn fail(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    1
}

pub fn run(mut args: Vec<String>, config_dir: &Path) -> i32 {
    if args.first().map(String::as_str) != Some("setup") {
        return fail(&format!(
            "{COMMANDS}: `mason remote setup --project P [--runs-on L]`, `mason remote on|off|status|runs [--json]`"
        ));
    }
    args.remove(0);
    let mut take = |name: &str| -> Result<Option<String>, String> {
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
    let (project, runs_on) = match (take("--project"), take("--runs-on")) {
        (Ok(p), Ok(r)) => (p, r),
        (Err(e), _) | (_, Err(e)) => return fail(&e),
    };
    if let Some(x) = args.first() {
        return fail(&format!("unexpected argument {x:?}"));
    }
    let Some(project) = project else {
        return fail("which project? pass --project P");
    };
    let ps = match load_projects(config_dir) {
        Ok(ps) => ps,
        Err(e) => return fail(&format!("project config: {}", e.join("; "))),
    };
    let Some(p) = ps.iter().find(|p| p.id == project) else {
        return fail(&format!(
            "no project {project:?} in {}",
            config_dir.display()
        ));
    };
    match setup(
        &p.path,
        &p.base_ref(),
        &p.base,
        &p.test_cmd,
        runs_on.as_deref(),
    ) {
        Ok(said) => {
            print!("{said}");
            if p.services.remote.is_none() {
                println!("\nThe project has no [remote_runs] section yet; add one to {}/projects/{project}.toml:\n\n  [remote_runs]\n  kind = \"github-actions\"\n  repo = \"owner/name\"\n  token = \"env:GITHUB_TOKEN\"", config_dir.display());
            }
            0
        }
        Err(e) => fail(&e),
    }
}

/// Write the workflow on [`BRANCH`], cut from `start` (the base as the remote has it, else the
/// local `base`). Returns what to tell the owner.
pub fn setup(
    repo: &Path,
    start: &str,
    base: &str,
    test_cmd: &str,
    runs_on: Option<&str>,
) -> Result<String, String> {
    let g =
        |args: &[&str]| git::run(repo, args).map_err(|e| format!("git {}: {e}", args.join(" ")));
    let start = if git::ok(repo, &["rev-parse", "--verify", "-q", start]) {
        start
    } else if git::ok(repo, &["rev-parse", "--verify", "-q", base]) {
        base
    } else {
        return Err(format!("the base branch {base:?} does not exist here"));
    };
    if git::ok(
        repo,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{BRANCH}"),
        ],
    ) {
        return Err(format!(
            "the branch {BRANCH} already exists from an earlier setup; merge or delete it first"
        ));
    }
    if git::ok(
        repo,
        &[
            "cat-file",
            "-e",
            &format!("{start}:.github/workflows/mason-run.yml"),
        ],
    ) {
        return Err(format!(
            "{start} already has .github/workflows/mason-run.yml; edit it there (`mason doctor` checks it)"
        ));
    }
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "mason-remote-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    let tmp_s = tmp.display().to_string();
    g(&["worktree", "prune"])?;
    g(&["worktree", "add", "-q", "-b", BRANCH, &tmp_s, start])?;
    let built = (|| -> Result<Plan, String> {
        let plan = plan(&tmp, test_cmd, runs_on);
        let file = tmp.join(".github/workflows/mason-run.yml");
        std::fs::create_dir_all(file.parent().unwrap_or(&tmp)).map_err(|e| e.to_string())?;
        std::fs::write(&file, render(&plan.steps)).map_err(|e| e.to_string())?;
        git::run(&tmp, &["add", ".github/workflows/mason-run.yml"]).map_err(|e| e.to_string())?;
        let mut c: Vec<&str> = vec![];
        let ident = !git::run(&tmp, &["config", "user.email"]).is_ok_and(|e| !e.trim().is_empty());
        if ident {
            c.extend(["-c", "user.name=MASON", "-c", "user.email=mason@localhost"]);
        }
        c.extend([
            "commit",
            "-q",
            "-m",
            "Add the remote run workflow (mason remote setup)",
            "-m",
            "Written by `mason remote setup`. Review it, merge it to the base branch, and runs can go to GitHub Actions.",
        ]);
        git::run(&tmp, &c).map_err(|e| e.to_string())?;
        Ok(plan)
    })();
    let _ = git::run(repo, &["worktree", "remove", "--force", &tmp_s]);
    let _ = std::fs::remove_dir_all(&tmp);
    let plan = match built {
        Ok(p) => p,
        Err(e) => {
            let _ = git::run(repo, &["branch", "-D", BRANCH]);
            return Err(e);
        }
    };
    let mut said = format!(
        "Wrote .github/workflows/mason-run.yml on the branch {BRANCH} (cut from {start}). Nothing was pushed, and your checked-out branch and files are untouched.\n\nRunner: {} (set `runs_on = \"{}\"` under [remote_runs] if it is not the default).\n",
        plan.runs_on, plan.runs_on
    );
    if !plan.review.is_empty() {
        said.push_str("\nReview:\n");
        for r in &plan.review {
            said.push_str(&format!("  - {r}\n"));
        }
    }
    said.push_str(&format!(
        "\nLook at it:  git diff {start}..{BRANCH}\nLand it:     merge {BRANCH} into {base} the way you land other changes (a pull request, or `git merge` and push), then run `mason doctor`.\nThe workflow must be on {base} on GitHub before a run can use it.\n"
    ));
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let run = |a: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(a)
                .current_dir(d.path())
                .status()
                .unwrap()
                .success());
        };
        run(&["init", "-q", "-b", "main"]);
        for (p, t) in files {
            let f = d.path().join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        }
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
        d
    }

    fn head(d: &Path) -> String {
        git::run(d, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string()
    }

    #[test]
    fn a_cargo_project_gets_its_toolchain_and_a_cache_that_saves_on_failure() {
        let d = repo(&[
            ("Cargo.toml", "[workspace]\n"),
            (
                "rust-toolchain.toml",
                "[toolchain]\nchannel = \"1.96.0\"\ncomponents = [\"rustfmt\", \"clippy\"]\n",
            ),
        ]);
        let before = head(d.path());
        let said = setup(d.path(), "origin/main", "main", "cargo test", None).unwrap();
        assert!(said.contains("Nothing was pushed"), "{said}");
        let f = git::run(
            d.path(),
            &["show", &format!("{BRANCH}:.github/workflows/mason-run.yml")],
        )
        .unwrap();
        assert!(f.contains("toolchain: 1.96.0"), "{f}");
        assert!(f.contains("components: rustfmt, clippy"));
        assert!(f.contains("cache-on-failure: true"));
        assert!(!f.contains("# setup: the project's"));
        // The current branch, its files and its index are as they were.
        assert_eq!(head(d.path()), before);
        assert_eq!(
            git::run(d.path(), &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap()
                .trim(),
            "main"
        );
        assert!(!d.path().join(".github").exists());
        assert!(git::run(d.path(), &["status", "--porcelain"])
            .unwrap()
            .trim()
            .is_empty());
        // Setting up twice is refused, not overwritten.
        assert!(setup(d.path(), "main", "main", "cargo test", None).is_err());
    }

    #[test]
    fn a_swiftpm_project_gets_a_mac_runner_and_the_xcode_its_command_names() {
        let d = repo(&[("Package.swift", "// swift-tools-version:5.9\n")]);
        let cmd = "DEVELOPER_DIR=/Applications/Xcode_16.4.app/Contents/Developer swift test";
        let said = setup(d.path(), "main", "main", cmd, None).unwrap();
        assert!(said.contains("Runner: macos-15"), "{said}");
        let f = git::run(
            d.path(),
            &["show", &format!("{BRANCH}:.github/workflows/mason-run.yml")],
        )
        .unwrap();
        assert!(f.contains("xcode-version: '16.4'"), "{f}");
        assert!(!f.contains("rust-cache"));
        assert!(!d.path().join(".github").exists());
    }

    #[test]
    fn xcode_versions_are_read_from_a_command() {
        assert_eq!(
            xcode_version("DEVELOPER_DIR=/Applications/Xcode_16.2.app/x"),
            Some("16.2".into())
        );
        assert_eq!(xcode_version("xcodebuild -version"), None);
        assert_eq!(xcode_version("swift test"), None);
    }

    #[test]
    fn gradle_gets_java_and_gradle_setup_and_the_runner_can_be_chosen() {
        let d = repo(&[("app/build.gradle.kts", "")]);
        let p = plan(d.path(), "./gradlew test", Some("ubuntu-24.04"));
        assert!(p.steps.contains("setup-java"));
        assert!(p.steps.contains("setup-gradle"));
        assert_eq!(p.runs_on, "ubuntu-24.04");
    }
}
