// SPDX-License-Identifier: GPL-3.0-or-later

//! The shell that runs a project's commands (check commands, hooks, gate scripts).
//!
//! On Unix it is `sh`. Windows has no `sh`, so MASON uses the `bash.exe` of Git for Windows,
//! which MASON needs anyway. It is looked for, in order: `MASON_SHELL`, the install that
//! `git --exec-path` points into, the usual `Program Files` folders, then PATH (skipping
//! `System32`, whose `bash.exe` is the WSL launcher and runs a different machine's files).
//! The Windows registry is not consulted. Discovery takes its inputs as arguments so a test
//! can give it any environment; [`shell_program`] gathers the real ones once.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// What the Windows search looks at.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// `MASON_SHELL`: a full path that wins over everything.
    pub override_path: Option<PathBuf>,
    /// The output of `git --exec-path`.
    pub git_exec_path: Option<PathBuf>,
    /// `ProgramFiles`, `ProgramFiles(x86)` and `LocalAppData\Programs`.
    pub program_files: Vec<PathBuf>,
    /// The folders of PATH.
    pub path_dirs: Vec<PathBuf>,
}

/// The message when Git Bash cannot be found.
pub const MISSING: &str = "no shell to run project commands: install Git for Windows (https://git-scm.com/download/win), which provides bash.exe, or set MASON_SHELL to the full path of a bash.exe";

/// Find Git Bash from `inputs`; `exists` says whether a file is there.
pub fn find_windows_bash(
    inputs: &Inputs,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, String> {
    if let Some(p) = &inputs.override_path {
        return if exists(p) {
            Ok(p.clone())
        } else {
            Err(format!(
                "MASON_SHELL is {} but no file is there",
                p.display()
            ))
        };
    }
    if let Some(g) = &inputs.git_exec_path {
        // <root>\mingw64\libexec\git-core -> <root>\bin\bash.exe
        for root in g.ancestors().skip(1) {
            for rel in ["bin/bash.exe", "usr/bin/bash.exe"] {
                let c = root.join(rel);
                if exists(&c) {
                    return Ok(c);
                }
            }
        }
    }
    for pf in &inputs.program_files {
        for rel in ["Git/bin/bash.exe", "Git/usr/bin/bash.exe"] {
            let c = pf.join(rel);
            if exists(&c) {
                return Ok(c);
            }
        }
    }
    for d in &inputs.path_dirs {
        if d.to_string_lossy()
            .to_ascii_lowercase()
            .contains("system32")
        {
            continue;
        }
        let c = d.join("bash.exe");
        if exists(&c) {
            return Ok(c);
        }
    }
    Err(MISSING.to_string())
}

/// Backslashes to forward slashes, which Git Bash reads (`C:\a\b` becomes `C:/a/b`).
pub fn windows_to_shell_path(p: &str) -> String {
    p.replace('\\', "/")
}

/// A path as it should appear inside a command for [`shell_command`]'s shell.
pub fn to_shell_path(p: &Path) -> String {
    let s = p.display().to_string();
    if cfg!(windows) {
        windows_to_shell_path(&s)
    } else {
        s
    }
}

fn real_inputs() -> Inputs {
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let git_exec_path = Command::new("git")
        .arg("--exec-path")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    let mut program_files: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|k| var(k))
        .collect();
    if let Some(l) = var("LocalAppData") {
        program_files.push(l.join("Programs"));
    }
    Inputs {
        override_path: var("MASON_SHELL"),
        git_exec_path,
        program_files,
        path_dirs: std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default(),
    }
}

/// The shell program for project commands: `sh` on Unix, Git Bash on Windows. Found once.
pub fn shell_program() -> Result<PathBuf, String> {
    if !cfg!(windows) {
        return Ok(PathBuf::from("sh"));
    }
    static FOUND: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    FOUND
        .get_or_init(|| find_windows_bash(&real_inputs(), &|p| p.is_file()))
        .clone()
}

/// `<shell> -c cmd`, ready for the caller to set its directory, environment and pipes.
pub fn shell_command(cmd: &str) -> Result<Command, String> {
    let mut c = Command::new(shell_program()?);
    c.args(["-c", cmd]);
    Ok(c)
}

/// One line for `mason doctor`: the shell commands will run under, or why there is none.
pub fn describe() -> Result<String, String> {
    shell_program().map(|p| format!("project commands run under {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(files: &[&str]) -> impl Fn(&Path) -> bool {
        let set: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        move |p| set.iter().any(|f| f == p)
    }

    #[test]
    fn the_override_wins_and_must_exist() {
        let i = Inputs {
            override_path: Some("/x/bash.exe".into()),
            path_dirs: vec!["/p".into()],
            ..Inputs::default()
        };
        assert_eq!(
            find_windows_bash(&i, &with(&["/x/bash.exe", "/p/bash.exe"])).unwrap(),
            PathBuf::from("/x/bash.exe")
        );
        let e = find_windows_bash(&i, &with(&["/p/bash.exe"])).unwrap_err();
        assert!(e.contains("MASON_SHELL"), "{e}");
    }

    #[test]
    fn git_exec_path_leads_to_the_install_root() {
        let i = Inputs {
            git_exec_path: Some("/G/Git/mingw64/libexec/git-core".into()),
            program_files: vec!["/other".into()],
            ..Inputs::default()
        };
        let f = with(&["/G/Git/bin/bash.exe", "/other/Git/bin/bash.exe"]);
        assert_eq!(
            find_windows_bash(&i, &f).unwrap(),
            PathBuf::from("/G/Git/bin/bash.exe")
        );
    }

    #[test]
    fn program_files_then_path_and_system32_is_skipped() {
        let i = Inputs {
            program_files: vec!["/pf".into()],
            path_dirs: vec!["/C/Windows/System32".into(), "/tools".into()],
            ..Inputs::default()
        };
        assert_eq!(
            find_windows_bash(&i, &with(&["/pf/Git/bin/bash.exe"])).unwrap(),
            PathBuf::from("/pf/Git/bin/bash.exe")
        );
        let f = with(&["/C/Windows/System32/bash.exe", "/tools/bash.exe"]);
        assert_eq!(
            find_windows_bash(&i, &f).unwrap(),
            PathBuf::from("/tools/bash.exe")
        );
        let only_wsl = with(&["/C/Windows/System32/bash.exe"]);
        assert!(find_windows_bash(&i, &only_wsl).is_err());
    }

    #[test]
    fn nothing_found_names_what_to_install() {
        let e = find_windows_bash(&Inputs::default(), &|_| false).unwrap_err();
        assert!(e.contains("Git for Windows"), "{e}");
        assert!(e.contains("MASON_SHELL"), "{e}");
    }

    #[test]
    fn paths_become_forward_slashes() {
        assert_eq!(windows_to_shell_path(r"C:\a\b c\d"), "C:/a/b c/d");
    }

    #[test]
    fn the_real_shell_runs_a_command() {
        if cfg!(windows) && shell_program().is_err() {
            return;
        }
        let out = shell_command("echo hi").unwrap().output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    }
}
