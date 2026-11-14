// SPDX-License-Identifier: GPL-3.0-or-later
//! `mason service install|uninstall|status`: run `masond --managed` under the platform's
//! service manager (launchd on macOS, a systemd user unit on Linux).
//!
//! One function per platform per verb (`install_launchd`, `install_systemd`, ...) is the seam
//! a Windows service can add a third to. Every call to `launchctl`/`systemctl` goes through a
//! [`Runner`], so tests never touch the real service manager.

use std::path::{Path, PathBuf};

/// The launchd label (and plist file stem).
pub const LABEL: &str = "io.github.brianm998.masond";
/// The systemd unit file name.
pub const UNIT: &str = "masond.service";
/// Every unit this command writes contains this text; it is how we know a unit is ours.
pub const MARKER: &str = "Written by `mason service install`";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Launchd,
    Systemd,
}

impl Kind {
    pub fn current() -> Option<Kind> {
        if cfg!(target_os = "macos") {
            Some(Kind::Launchd)
        } else if cfg!(target_os = "linux") {
            Some(Kind::Systemd)
        } else {
            None
        }
    }
}

/// Runs a program and returns (exit code, stdout+stderr). Err when it could not start.
pub trait Runner {
    fn run(&self, prog: &str, args: &[String]) -> Result<(i32, String), String>;
}

pub struct RealRunner;

impl Runner for RealRunner {
    fn run(&self, prog: &str, args: &[String]) -> Result<(i32, String), String> {
        let o = std::process::Command::new(prog)
            .args(args)
            .output()
            .map_err(|e| format!("{prog}: {e}"))?;
        let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&o.stderr));
        Ok((o.status.code().unwrap_or(-1), text))
    }
}

pub struct Opts {
    pub home: PathBuf,
    pub masond: PathBuf,
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub force: bool,
}

pub fn unit_path(kind: Kind, home: &Path) -> PathBuf {
    match kind {
        Kind::Launchd => home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
        Kind::Systemd => home.join(".config/systemd/user").join(UNIT),
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn plist(o: &Opts) -> String {
    let arg = |s: &str| format!("    <string>{}</string>\n", xml(s));
    let mut args = String::new();
    args.push_str(&arg(&o.masond.display().to_string()));
    for a in ["--managed", "--data-dir"] {
        args.push_str(&arg(a));
    }
    args.push_str(&arg(&o.data_dir.display().to_string()));
    args.push_str(&arg("--config-dir"));
    args.push_str(&arg(&o.config_dir.display().to_string()));
    let log = |n: &str| xml(&o.data_dir.join(n).display().to_string());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<!-- {MARKER} -->\n\
<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{LABEL}</string>\n\
  <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n\
  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <true/>\n\
  <key>StandardOutPath</key>\n  <string>{}</string>\n\
  <key>StandardErrorPath</key>\n  <string>{}</string>\n\
</dict>\n</plist>\n",
        log("masond.out.log"),
        log("masond.err.log")
    )
}

fn quote(p: &Path) -> String {
    let s = p.display().to_string();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn systemd_unit(o: &Opts) -> String {
    format!(
        "# {MARKER}\n[Unit]\nDescription=MASON daemon (masond)\n\n[Service]\n\
ExecStart={} --managed --data-dir {} --config-dir {}\n\
Restart=always\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
        quote(&o.masond),
        quote(&o.data_dir),
        quote(&o.config_dir)
    )
}

pub fn render(kind: Kind, o: &Opts) -> String {
    match kind {
        Kind::Launchd => plist(o),
        Kind::Systemd => systemd_unit(o),
    }
}

/// Is the file at `path` a unit this command wrote?
pub fn is_ours(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.contains(MARKER))
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn check(r: Result<(i32, String), String>, what: &str) -> Result<String, String> {
    match r {
        Ok((0, out)) => Ok(out),
        Ok((c, out)) => Err(format!("{what} failed (exit {c}): {}", out.trim())),
        Err(e) => Err(format!("{what}: {e}")),
    }
}

fn write_unit(kind: Kind, o: &Opts) -> Result<PathBuf, String> {
    let path = unit_path(kind, &o.home);
    if path.exists() && !is_ours(&path) && !o.force {
        return Err(format!(
            "{} exists and was not written by `mason service install`; use --force to replace it",
            path.display()
        ));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::create_dir_all(&o.data_dir).map_err(|e| format!("{}: {e}", o.data_dir.display()))?;
    std::fs::write(&path, render(kind, o)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn uid(r: &dyn Runner) -> Result<String, String> {
    Ok(check(r.run("id", &s(&["-u"])), "id -u")?.trim().to_string())
}

pub fn install_launchd(o: &Opts, r: &dyn Runner) -> Result<Vec<String>, String> {
    let path = write_unit(Kind::Launchd, o)?;
    let domain = format!("gui/{}", uid(r)?);
    // Replace a loaded copy; it is fine when none was loaded.
    let _ = r.run(
        "launchctl",
        &["bootout".into(), format!("{domain}/{LABEL}")],
    );
    check(
        r.run(
            "launchctl",
            &["bootstrap".into(), domain, path.display().to_string()],
        ),
        "launchctl bootstrap",
    )?;
    Ok(vec![format!("wrote {} and loaded it", path.display())])
}

pub fn install_systemd(o: &Opts, r: &dyn Runner) -> Result<Vec<String>, String> {
    let path = write_unit(Kind::Systemd, o)?;
    check(
        r.run("systemctl", &s(&["--user", "daemon-reload"])),
        "systemctl daemon-reload",
    )?;
    check(
        r.run("systemctl", &s(&["--user", "enable", "--now", UNIT])),
        "systemctl enable --now",
    )?;
    Ok(vec![format!(
        "wrote {}, enabled and started {UNIT}",
        path.display()
    )])
}

pub fn install(kind: Kind, o: &Opts, r: &dyn Runner) -> Result<Vec<String>, String> {
    if !o.masond.is_file() {
        return Err(format!(
            "{} is not a file: masond must sit beside mason",
            o.masond.display()
        ));
    }
    match kind {
        Kind::Launchd => install_launchd(o, r),
        Kind::Systemd => install_systemd(o, r),
    }
}

fn refuse_foreign(path: &Path) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    if !is_ours(path) {
        return Err(format!(
            "{} was not written by `mason service install`; leaving it alone",
            path.display()
        ));
    }
    Ok(true)
}

pub fn uninstall_launchd(home: &Path, r: &dyn Runner) -> Result<Vec<String>, String> {
    let path = unit_path(Kind::Launchd, home);
    if !refuse_foreign(&path)? {
        return Ok(vec!["nothing to uninstall".into()]);
    }
    let _ = r.run(
        "launchctl",
        &["bootout".into(), format!("gui/{}/{LABEL}", uid(r)?)],
    );
    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(vec![format!("stopped and removed {}", path.display())])
}

pub fn uninstall_systemd(home: &Path, r: &dyn Runner) -> Result<Vec<String>, String> {
    let path = unit_path(Kind::Systemd, home);
    if !refuse_foreign(&path)? {
        return Ok(vec!["nothing to uninstall".into()]);
    }
    let _ = r.run("systemctl", &s(&["--user", "disable", "--now", UNIT]));
    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = r.run("systemctl", &s(&["--user", "daemon-reload"]));
    Ok(vec![format!("stopped and removed {}", path.display())])
}

pub fn uninstall(kind: Kind, home: &Path, r: &dyn Runner) -> Result<Vec<String>, String> {
    match kind {
        Kind::Launchd => uninstall_launchd(home, r),
        Kind::Systemd => uninstall_systemd(home, r),
    }
}

/// (loaded, running) according to the service manager.
fn manager_state(kind: Kind, r: &dyn Runner) -> (bool, bool) {
    match kind {
        Kind::Launchd => {
            let Ok(u) = uid(r) else { return (false, false) };
            match r.run("launchctl", &["print".into(), format!("gui/{u}/{LABEL}")]) {
                Ok((0, out)) => (true, out.contains("state = running")),
                _ => (false, false),
            }
        }
        Kind::Systemd => {
            let enabled = matches!(
                r.run("systemctl", &s(&["--user", "is-enabled", UNIT])),
                Ok((0, _))
            );
            let active = matches!(
                r.run("systemctl", &s(&["--user", "is-active", UNIT])),
                Ok((0, _))
            );
            (enabled, active)
        }
    }
}

pub fn status(kind: Kind, home: &Path, data_dir: &Path, r: &dyn Runner) -> Vec<String> {
    let path = unit_path(kind, home);
    let installed = path.exists();
    let ours = installed && is_ours(&path);
    let (loaded, running) = if installed {
        manager_state(kind, r)
    } else {
        (false, false)
    };
    let pid = std::fs::read_to_string(data_dir.join("daemon.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["pid"].as_u64())
        .map(|p| p as u32);
    let pid_text = match pid {
        Some(p) if mason_step_host::registry::pid_alive(p) => format!("pid {p}"),
        Some(p) => format!("none alive (daemon.json names pid {p}, which is gone)"),
        None => "none (no daemon.json)".into(),
    };
    vec![
        format!(
            "installed: {} ({})",
            if installed { "yes" } else { "no" },
            path.display()
        ),
        format!(
            "written by mason service install: {}",
            if ours { "yes" } else { "no" }
        ),
        format!("loaded: {}", if loaded { "yes" } else { "no" }),
        format!("running: {}", if running { "yes" } else { "no" }),
        format!("daemon: {pid_text}"),
    ]
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

/// `mason service ...`; returns the exit code.
pub fn run(mut args: Vec<String>, data_dir: &Path, config_dir: &Path) -> i32 {
    let take = |args: &mut Vec<String>, f: &str| {
        args.iter()
            .position(|a| a == f)
            .map(|i| args.remove(i))
            .is_some()
    };
    let print = take(&mut args, "--print");
    let force = take(&mut args, "--force");
    let sub = if args.is_empty() {
        String::new()
    } else {
        args.remove(0)
    };
    if !args.is_empty() || !matches!(sub.as_str(), "install" | "uninstall" | "status") {
        eprintln!("usage: mason service install [--print] [--force] | uninstall | status");
        return 2;
    }
    let Some(kind) = Kind::current() else {
        eprintln!("error: mason service supports launchd (macOS) and systemd (Linux) only");
        return 1;
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let masond = std::env::current_exe()
        .map(|e| e.with_file_name("masond"))
        .unwrap_or_else(|_| PathBuf::from("masond"));
    let o = Opts {
        home: home.clone(),
        masond: absolute(&masond),
        data_dir: absolute(data_dir),
        config_dir: absolute(config_dir),
        force,
    };
    let res = match sub.as_str() {
        "install" if print => {
            print!("{}", render(kind, &o));
            return 0;
        }
        "install" => install(kind, &o, &RealRunner),
        "uninstall" => uninstall(kind, &home, &RealRunner),
        _ => {
            for l in status(kind, &home, &o.data_dir, &RealRunner) {
                println!("{l}");
            }
            return 0;
        }
    };
    match res {
        Ok(lines) => {
            for l in lines {
                println!("{l}");
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<String>>,
    }
    impl Runner for Fake {
        fn run(&self, prog: &str, args: &[String]) -> Result<(i32, String), String> {
            self.calls
                .borrow_mut()
                .push(format!("{prog} {}", args.join(" ")));
            if prog == "id" {
                return Ok((0, "501\n".into()));
            }
            Ok((0, String::new()))
        }
    }

    fn opts(home: &Path) -> Opts {
        let masond = home.join("bin/masond");
        std::fs::create_dir_all(masond.parent().unwrap()).unwrap();
        std::fs::write(&masond, "x").unwrap();
        Opts {
            home: home.to_path_buf(),
            masond,
            data_dir: home.join("data dir"),
            config_dir: home.join("cfg"),
            force: false,
        }
    }

    #[test]
    fn plist_names_masond_managed_and_dirs() {
        let t = tempfile::tempdir().unwrap();
        let o = opts(t.path());
        let p = plist(&o);
        assert!(p.contains(&format!("<string>{LABEL}</string>")));
        assert!(p.contains(&format!("<string>{}</string>", o.masond.display())));
        assert!(p.contains("<string>--managed</string>"));
        assert!(p.contains(&format!("<string>{}</string>", o.data_dir.display())));
        assert!(p.contains(&format!("<string>{}</string>", o.config_dir.display())));
        assert!(p.contains("<key>KeepAlive</key>"));
        assert!(p.contains("masond.err.log"));
        assert!(p.contains(MARKER));
    }

    #[test]
    fn systemd_unit_quotes_paths() {
        let t = tempfile::tempdir().unwrap();
        let o = opts(t.path());
        let u = systemd_unit(&o);
        assert!(u.contains(&format!(
            "ExecStart=\"{}\" --managed --data-dir \"{}\"",
            o.masond.display(),
            o.data_dir.display()
        )));
        assert!(u.contains("Restart=always"));
        assert!(u.contains(MARKER));
    }

    #[test]
    fn install_runs_launchctl_and_systemctl() {
        let t = tempfile::tempdir().unwrap();
        let o = opts(t.path());
        let f = Fake::default();
        install(Kind::Launchd, &o, &f).unwrap();
        assert!(unit_path(Kind::Launchd, t.path()).is_file());
        let calls = f.calls.borrow().join("\n");
        assert!(calls.contains("launchctl bootstrap gui/501"), "{calls}");
        let f = Fake::default();
        install(Kind::Systemd, &o, &f).unwrap();
        let calls = f.calls.borrow().join("\n");
        assert!(calls.contains("systemctl --user daemon-reload"), "{calls}");
        assert!(
            calls.contains("systemctl --user enable --now masond.service"),
            "{calls}"
        );
    }

    #[test]
    fn refuses_foreign_unit_without_force() {
        let t = tempfile::tempdir().unwrap();
        let mut o = opts(t.path());
        let path = unit_path(Kind::Systemd, t.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[Service]\nExecStart=/bin/true\n").unwrap();
        let f = Fake::default();
        let e = install(Kind::Systemd, &o, &f).unwrap_err();
        assert!(e.contains("--force"), "{e}");
        assert!(f.calls.borrow().is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[Service]\nExecStart=/bin/true\n"
        );
        // Uninstall leaves it alone too.
        assert!(uninstall(Kind::Systemd, t.path(), &f).is_err());
        assert!(path.exists());
        o.force = true;
        install(Kind::Systemd, &o, &f).unwrap();
        assert!(is_ours(&path));
    }

    #[test]
    fn reinstall_over_our_own_unit_and_uninstall() {
        let t = tempfile::tempdir().unwrap();
        let o = opts(t.path());
        let f = Fake::default();
        install(Kind::Systemd, &o, &f).unwrap();
        install(Kind::Systemd, &o, &f).unwrap();
        uninstall(Kind::Systemd, t.path(), &f).unwrap();
        assert!(!unit_path(Kind::Systemd, t.path()).exists());
        assert!(f
            .calls
            .borrow()
            .iter()
            .any(|c| c.contains("disable --now masond.service")));
        let again = uninstall(Kind::Systemd, t.path(), &f).unwrap();
        assert_eq!(again, vec!["nothing to uninstall".to_string()]);
    }

    #[test]
    fn status_reports_daemon_pid_from_daemon_json() {
        let t = tempfile::tempdir().unwrap();
        let o = opts(t.path());
        std::fs::create_dir_all(&o.data_dir).unwrap();
        std::fs::write(
            o.data_dir.join("daemon.json"),
            format!("{{\"pid\":{},\"port\":1}}", std::process::id()),
        )
        .unwrap();
        let f = Fake::default();
        let before = status(Kind::Systemd, t.path(), &o.data_dir, &f).join("\n");
        assert!(before.contains("installed: no"), "{before}");
        assert!(
            before.contains(&format!("pid {}", std::process::id())),
            "{before}"
        );
        install(Kind::Systemd, &o, &f).unwrap();
        let after = status(Kind::Systemd, t.path(), &o.data_dir, &f).join("\n");
        assert!(after.contains("installed: yes"), "{after}");
        assert!(
            after.contains("loaded: yes") && after.contains("running: yes"),
            "{after}"
        );
    }

    #[test]
    fn missing_masond_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let mut o = opts(t.path());
        o.masond = t.path().join("nope");
        assert!(install(Kind::Launchd, &o, &Fake::default()).is_err());
    }
}
