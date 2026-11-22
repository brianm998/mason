// SPDX-License-Identifier: GPL-3.0-or-later

//! Shims that keep a develop agent from running tests in its own shell when the task's runs go
//! to GitHub (#704).
//!
//! An agent's shell commands always run on this Mac, so `cargo test --workspace` there heats the
//! machine the switch was turned on to cool, and its result is no evidence. The daemon writes a
//! directory of wrappers under its data directory; the agent's PATH starts with it. A wrapper
//! refuses a test run (exit 69, with the message) and passes every other form straight to the
//! real tool, whose absolute path was resolved when the wrapper was written, never through PATH.
//! Builds for type-checking stay local. `MASON_REMOTE_SHIM=off` turns the wrappers off.

use std::path::{Path, PathBuf};

/// What a refused test run prints.
pub const MESSAGE: &str =
    "Tests run on GitHub for this task: call run_check (it runs them there and reports here)";

/// The tools that get a wrapper.
pub const TOOLS: [&str; 5] = ["cargo", "swift", "xcodebuild", "gradle", "gradlew"];

/// The directory the wrappers live in.
pub fn shim_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("shims")
}

/// The first executable file called `name` on `path` that is not in `skip`.
#[cfg(unix)]
fn resolve(name: &str, path: &str, skip: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .filter(|d| d.is_absolute() && d != skip)
        .map(|d| d.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// The shell test that says "these arguments are a test run" for `tool`.
fn refusal_condition(tool: &str) -> &'static str {
    match tool {
        // The first word that is not an option or a `+toolchain`, and for nextest the next one.
        "cargo" => {
            r#"sub=""; nxt=""
for a in "$@"; do
  case "$a" in -*|+*) continue ;; esac
  if [ -z "$sub" ]; then sub=$a; elif [ -z "$nxt" ]; then nxt=$a; fi
done
case "$sub" in test) refuse ;; nextest) case "$nxt" in run|r) refuse ;; esac ;; esac"#
        }
        "swift" => {
            r#"for a in "$@"; do
  case "$a" in -*) continue ;; test) refuse ;; esac
  break
done"#
        }
        "xcodebuild" => {
            r#"for a in "$@"; do
  case "$a" in test|test-without-building) refuse ;; esac
done"#
        }
        _ => {
            r#"for a in "$@"; do
  case "$a" in test|check|*:test|*:check) refuse ;; esac
done"#
        }
    }
}

/// The text of the wrapper for `tool`, passing everything else to `real`.
pub fn script(tool: &str, real: &str) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    let target = if tool == "gradlew" {
        // `./gradlew` belongs to the project: it is found where the agent is.
        "./gradlew".to_string()
    } else {
        q(real)
    };
    format!(
        "#!/bin/sh\n# SPDX-License-Identifier: GPL-3.0-or-later\n\
         # Written by masond (#704): refuses test runs for a task whose runs go to GitHub.\n\
         real={target}\n\
         refuse() {{ echo {msg} >&2; exit 69; }}\n\
         if [ \"${{MASON_REMOTE_SHIM:-}}\" != off ]; then\n{cond}\nfi\n\
         exec \"$real\" \"$@\"\n",
        msg = q(MESSAGE),
        cond = refusal_condition(tool),
    )
}

/// Write the wrappers into `dir` for every tool found on `path`. Returns the directory, or
/// nothing when it could not be made.
#[cfg(unix)]
pub fn write(dir: &Path, path: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).ok()?;
    for tool in TOOLS {
        let real = if tool == "gradlew" {
            Some(PathBuf::from("./gradlew"))
        } else {
            resolve(tool, path, dir)
        };
        let file = dir.join(tool);
        let Some(real) = real else {
            let _ = std::fs::remove_file(&file);
            continue;
        };
        let text = script(tool, &real.to_string_lossy());
        if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
            let tmp = dir.join(format!(".{tool}.{}.tmp", std::process::id()));
            std::fs::write(&tmp, text).ok()?;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).ok()?;
            std::fs::rename(&tmp, &file).ok()?;
        }
    }
    Some(dir.to_path_buf())
}

/// The wrappers are POSIX sh scripts and cannot run on Windows, so nothing is written there and
/// `apply` leaves `PATH` unchanged.
#[cfg(not(unix))]
pub fn write(_dir: &Path, _path: &str) -> Option<PathBuf> {
    None
}

/// The agent environment's `PATH` with the shim directory first, over `path`.
pub fn path_with(dir: &Path, path: &str) -> String {
    format!("{}:{path}", dir.display())
}

/// Add the shim directory to the front of `PATH` in `env` (a project's `agent_env` may set its
/// own PATH; otherwise the daemon's is used). Does nothing when the shims cannot be written.
pub fn apply(env: &mut Vec<(String, String)>, data_dir: &Path) {
    let base = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let dir = shim_dir(data_dir);
    if write(&dir, &base).is_none() {
        return;
    }
    env.retain(|(k, _)| k != "PATH");
    env.push(("PATH".into(), path_with(&dir, &base)));
}

/// The line the develop prompt gets when the shims are on.
pub const PROMPT_LINE: &str = "Tests for this task run on GitHub: run them with `run_check`; \
     `cargo test`, `swift test` and the like are refused in your shell. Local builds \
     (`cargo check`, `cargo build`, `swift build`) are fine.";

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    /// A directory with a fake tool of each name that records its argv.
    fn world() -> (tempfile::TempDir, PathBuf, PathBuf, String) {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let log = t.path().join("argv.log");
        for tool in ["cargo", "swift", "xcodebuild", "gradle"] {
            let f = real.join(tool);
            // Write, close, then rename into place: a fork from another test thread can hold a
            // copy of a still-open writable fd, which makes exec fail with ETXTBSY.
            let tmp = real.join(format!(".{tool}.tmp"));
            std::fs::write(
                &tmp,
                format!("#!/bin/sh\necho \"{tool} $*\" >> '{}'\n", log.display()),
            )
            .unwrap();
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::rename(&tmp, &f).unwrap();
        }
        let shims = t.path().join("shims");
        let path = format!("{}:/usr/bin:/bin", real.display());
        write(&shims, &path).unwrap();
        (t, shims, log, path)
    }

    fn run(shims: &Path, tool: &str, args: &[&str], off: bool) -> (i32, String) {
        // Another test thread's fork can briefly hold a writable copy of a script's fd, so exec
        // fails with "Text file busy" (here, or in the shim's own exec: exit 126). Retry that.
        let mut tries = 0;
        loop {
            let mut c = Command::new(shims.join(tool));
            c.args(args);
            if off {
                c.env("MASON_REMOTE_SHIM", "off");
            }
            tries += 1;
            let o = match c.output() {
                Ok(o) => o,
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 100 => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            let err = String::from_utf8_lossy(&o.stderr).into_owned();
            if o.status.code() == Some(126) && err.contains("busy") && tries < 100 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            return (o.status.code().unwrap(), err);
        }
    }

    #[test]
    fn test_runs_are_refused_with_exit_69_and_the_message() {
        let (_t, s, log, _) = world();
        let refused: &[(&str, &[&str])] = &[
            ("cargo", &["test", "-p", "x"]),
            ("cargo", &["+nightly", "test"]),
            ("cargo", &["nextest", "run"]),
            ("swift", &["test"]),
            ("swift", &["test", "--filter", "A"]),
            ("xcodebuild", &["-scheme", "A", "test"]),
            ("xcodebuild", &["test-without-building"]),
            ("gradle", &["test"]),
            ("gradle", &[":app:check"]),
        ];
        for (tool, args) in refused {
            let (code, err) = run(&s, tool, args, false);
            assert_eq!(code, 69, "{tool} {args:?}");
            assert!(err.contains(MESSAGE), "{tool} {args:?}: {err}");
        }
        assert!(!log.exists(), "a refused run reached the real tool");
    }

    #[test]
    fn other_forms_reach_the_real_tool() {
        let (_t, s, log, _) = world();
        let ok: &[(&str, &[&str])] = &[
            ("cargo", &["check", "-p", "x"]),
            ("cargo", &["build"]),
            ("cargo", &["nextest", "list"]),
            ("cargo", &["--version"]),
            ("swift", &["build"]),
            ("xcodebuild", &["build"]),
            ("gradle", &["assemble"]),
        ];
        for (tool, args) in ok {
            let (code, _) = run(&s, tool, args, false);
            assert_eq!(code, 0, "{tool} {args:?}");
        }
        let seen = std::fs::read_to_string(&log).unwrap();
        assert!(seen.contains("cargo check -p x\n") && seen.contains("swift build\n"));
        assert_eq!(seen.lines().count(), ok.len());
    }

    #[test]
    fn the_off_switch_lets_a_test_run_through() {
        let (_t, s, log, _) = world();
        assert_eq!(run(&s, "cargo", &["test"], true).0, 0);
        assert!(std::fs::read_to_string(&log)
            .unwrap()
            .contains("cargo test"));
    }

    #[test]
    fn apply_puts_the_shim_directory_first_on_path() {
        let (t, _s, _l, path) = world();
        let mut env = vec![("PATH".to_string(), path.clone())];
        apply(&mut env, t.path());
        let p = &env.iter().find(|(k, _)| k == "PATH").unwrap().1;
        assert!(p.starts_with(&format!("{}:", shim_dir(t.path()).display())));
        assert!(p.ends_with(&path));
        assert_eq!(env.iter().filter(|(k, _)| k == "PATH").count(), 1);
    }
}
