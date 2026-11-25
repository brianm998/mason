// SPDX-License-Identifier: GPL-3.0-or-later

//! The coordinator's stable app bundle on macOS (s1118).
//!
//! macOS enforces Local Network permission by the executable's Mach-O UUID, and keeps one rule
//! per executable path. A runtime in `runtimes/<sha>/` is a new bare path at every update, and on
//! an external disk the system never finds its UUID, so an allowed rule is never applied: every
//! connection to the LAN fails with "no route to host". So the coordinator execs its `masond`
//! from one signed bundle whose path never changes, `<app dir>/MASON Coordinator.app`: each
//! takeover installs that runtime's binaries into it. The runtimes stay where they are; they are
//! what is installed from, and what the self-test runs.

use crate::config::DaemonConfig;
use crate::update::{Runtimes, UpdateCfg};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The bundle's directory name. Not `MASON.app`: that is the monitor (`scripts/bundle-monitor.sh`).
pub const BUNDLE_NAME: &str = "MASON Coordinator.app";
/// The bundle identifier, equal to the identifier `masond` is signed with.
pub const IDENTIFIER: &str = "com.brianm998.mason.masond";

const PREVIOUS: &str = ".MASON Coordinator.previous";
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

// ---- which volume --------------------------------------------------------------------------

/// Where the volume holding a path is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    Internal,
    /// An external disk, or a volume with no local device (a network share).
    External,
    Unknown,
}

/// The device of the file system in `df -P PATH` output.
pub fn parse_df(text: &str) -> Option<&str> {
    text.lines().nth(1)?.split_whitespace().next()
}

/// What `diskutil info DEVICE` says about where the device is.
pub fn parse_diskutil(text: &str) -> Location {
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match (key.trim(), value.trim()) {
            ("Device Location", "Internal") | ("Internal", "Yes") => return Location::Internal,
            ("Device Location", "External") | ("Internal", "No") => return Location::External,
            _ => {}
        }
    }
    Location::Unknown
}

/// [`location_of`] with the system tools given: `run(program, args)` is its output, or `None`.
pub fn location_with(path: &Path, run: &dyn Fn(&str, &[&str]) -> Option<String>) -> Location {
    let Some(df) = run("/bin/df", &["-P", &path.to_string_lossy()]) else {
        return Location::Unknown;
    };
    match parse_df(&df) {
        Some(dev) if dev.starts_with("/dev/") => run("/usr/sbin/diskutil", &["info", dev])
            .map_or(Location::Unknown, |t| parse_diskutil(&t)),
        Some(_) => Location::External,
        None => Location::Unknown,
    }
}

fn tool(program: &str, args: &[&str]) -> Option<String> {
    let out = mason_landing::proc::output(Command::new(program).args(args), TOOL_TIMEOUT).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Where the volume holding `path` is. Always [`Location::Unknown`] off macOS.
pub fn location_of(path: &Path) -> Location {
    if !cfg!(target_os = "macos") {
        return Location::Unknown;
    }
    // A path that does not exist yet is on its nearest ancestor's volume.
    let there = path.ancestors().find(|p| p.exists()).unwrap_or(path);
    location_with(there, &tool)
}

/// [`location_of`] this process's executable, asked once.
pub fn own_location() -> Location {
    static OWN: std::sync::OnceLock<Location> = std::sync::OnceLock::new();
    *OWN.get_or_init(|| std::env::current_exe().map_or(Location::Unknown, |e| location_of(&e)))
}

// ---- where the bundle goes -----------------------------------------------------------------

/// Where the bundle is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The directory holding the bundle.
    pub root: PathBuf,
    /// Tell LaunchServices about it (only for the default place: a configured one may be a
    /// scratch directory).
    pub register: bool,
}

impl Target {
    pub fn app(&self) -> PathBuf {
        self.root.join(BUNDLE_NAME)
    }

    pub fn masond(&self) -> PathBuf {
        self.app().join("Contents").join("MacOS").join("masond")
    }
}

/// What decides whether there is a bundle, and where.
#[derive(Clone, Debug)]
pub struct Choice<'a> {
    pub macos: bool,
    /// `app_dir` in `update.toml`.
    pub app_dir: Option<&'a str>,
    /// This daemon serves as a node.
    pub node: bool,
    /// `update.toml` has a `sign_identity`.
    pub signed: bool,
    /// The data directory is the user's own default one.
    pub default_data_dir: bool,
    pub home: Option<&'a Path>,
}

/// The rule. `app_dir = "off"` turns the bundle off; an absolute (or `~/`) `app_dir` is where it
/// goes. Without the setting only the owner's own signed coordinator has one, in
/// `~/Applications`: a scratch daemon must never replace that, an ad-hoc signature would drop
/// the privacy grants, and a node is a launch daemon, which Local Network does not cover.
pub fn choose(c: &Choice) -> Option<Target> {
    if !c.macos {
        return None;
    }
    if let Some(dir) = c.app_dir {
        let root = match dir.strip_prefix("~/") {
            Some(rest) => c.home?.join(rest),
            None => PathBuf::from(dir),
        };
        return root.is_absolute().then_some(Target {
            root,
            register: false,
        });
    }
    if c.node || !c.signed || !c.default_data_dir {
        return None;
    }
    Some(Target {
        root: c.home?.join("Applications"),
        register: true,
    })
}

fn is_default_data_dir(data_dir: &Path, home: &Path) -> bool {
    let default = home.join("Library/Application Support/mason");
    match (data_dir.canonicalize(), default.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The bundle this daemon uses, if any, and the identity it is signed with.
pub fn configured(cfg: &DaemonConfig) -> (Option<Target>, Option<String>) {
    let update = UpdateCfg::load(&cfg.config_dir).ok();
    let identity = update.as_ref().and_then(|u| u.sign_identity.clone());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let target = choose(&Choice {
        macos: cfg!(target_os = "macos"),
        app_dir: update.as_ref().and_then(|u| u.app_dir.as_deref()),
        node: cfg.node.serve,
        signed: identity.is_some(),
        default_data_dir: home
            .as_deref()
            .is_some_and(|h| is_default_data_dir(&cfg.data_dir, h)),
        home: home.as_deref(),
    });
    (target, identity)
}

// ---- installing ----------------------------------------------------------------------------

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn info_plist(sha: &str, identity: Option<&str>) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
  <key>CFBundleIdentifier</key><string>{IDENTIFIER}</string>\n\
  <key>CFBundleName</key><string>MASON Coordinator</string>\n\
  <key>CFBundleDisplayName</key><string>MASON Coordinator</string>\n\
  <key>CFBundleExecutable</key><string>masond</string>\n\
  <key>CFBundlePackageType</key><string>APPL</string>\n\
  <key>CFBundleShortVersionString</key><string>1.0</string>\n\
  <key>CFBundleVersion</key><string>1</string>\n\
  <key>LSUIElement</key><true/>\n\
  <key>LSMinimumSystemVersion</key><string>13.0</string>\n\
  <key>NSLocalNetworkUsageDescription</key><string>MASON connects to your other Macs to run its work on them.</string>\n\
  {}\n</dict>\n</plist>\n",
        marks(sha, identity)
    )
}

/// The lines of the `Info.plist` that say what is installed.
fn marks(sha: &str, identity: Option<&str>) -> String {
    format!(
        "<key>MASONRuntime</key><string>{}</string>\n  <key>MASONSigner</key><string>{}</string>",
        xml(sha),
        xml(identity.unwrap_or("-"))
    )
}

fn verify(app: &Path) -> Result<(), String> {
    let out = mason_landing::proc::output(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(app),
        crate::update::SIGN_TIMEOUT,
    )
    .map_err(|_| format!("codesign could not verify {}", app.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} does not verify: {}",
            app.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn is_mach_o(p: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(p)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && matches!(
            u32::from_be_bytes(magic),
            // thin (either byte order, 32 and 64 bit) and universal
            0xfeed_face | 0xfeed_facf | 0xcefa_edfe | 0xcffa_edfe | 0xcafe_babe
        )
}

/// A bundle made ready beside the one in place, or the one in place when it already holds the
/// runtime. Nothing a running daemon uses changes until [`Staged::commit`].
#[derive(Debug)]
pub struct Staged {
    target: Target,
    /// The new bundle; `None` when the one in place is already right.
    partial: Option<PathBuf>,
}

/// Copy runtime `sha`'s binaries into a new bundle, sign it and verify it.
pub fn prepare(
    target: &Target,
    rt: &Runtimes,
    sha: &str,
    identity: Option<&str>,
) -> Result<Staged, String> {
    let app = target.app();
    if app.exists() {
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap_or_default();
        if !plist.contains(&format!("<string>{IDENTIFIER}</string>")) {
            return Err(format!(
                "{} is not MASON's bundle; it is left alone",
                app.display()
            ));
        }
        if plist.contains(&marks(sha, identity)) && verify(&app).is_ok() {
            return Ok(Staged {
                target: target.clone(),
                partial: None,
            });
        }
    }
    let partial = target
        .root
        .join(format!(".MASON Coordinator.partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&partial);
    let made = (|| -> Result<(), String> {
        let macos = partial.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).map_err(|e| format!("{}: {e}", macos.display()))?;
        let from = rt.dir(sha);
        if !is_mach_o(&from.join("masond")) {
            return Err(format!(
                "{} is not a Mach-O executable",
                from.join("masond").display()
            ));
        }
        for entry in std::fs::read_dir(&from)
            .map_err(|e| format!("{}: {e}", from.display()))?
            .flatten()
        {
            let path = entry.path();
            // Only code goes in `Contents/MacOS`: anything else would not verify.
            if !path.is_file() || !is_mach_o(&path) {
                continue;
            }
            let to = macos.join(entry.file_name());
            std::fs::copy(&path, &to).map_err(|e| format!("copying {}: {e}", path.display()))?;
            if entry.file_name() != "masond" {
                crate::update::sign_installed(&to, identity)?;
            }
        }
        std::fs::write(
            partial.join("Contents/Info.plist"),
            info_plist(sha, identity),
        )
        .map_err(|e| e.to_string())?;
        // Signing the bundle signs its main executable, `masond`.
        crate::update::sign_as(&partial, IDENTIFIER, identity)?;
        verify(&partial)
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    Ok(Staged {
        target: target.clone(),
        partial: Some(partial),
    })
}

impl Staged {
    /// Put the new bundle in place (two renames) and say what to exec. The replaced one is kept
    /// aside, not named `.app`, until the next time: step hosts may still be running its files.
    pub fn commit(&self) -> Result<PathBuf, String> {
        let Some(partial) = &self.partial else {
            return Ok(self.target.masond());
        };
        let app = self.target.app();
        let previous = self.target.root.join(PREVIOUS);
        let _ = std::fs::remove_dir_all(&previous);
        let had = app.exists();
        if had {
            std::fs::rename(&app, &previous).map_err(|e| format!("{}: {e}", app.display()))?;
        }
        if let Err(e) = std::fs::rename(partial, &app) {
            if had {
                let _ = std::fs::rename(&previous, &app);
            }
            return Err(format!("{}: {e}", app.display()));
        }
        if self.target.register {
            let _ = tool(LSREGISTER, &["-f", &app.to_string_lossy()]);
        }
        Ok(self.target.masond())
    }

    /// Undo a [`commit`](Staged::commit) whose exec failed: the running daemon's tools go back.
    pub fn revert(&self) {
        let Some(partial) = &self.partial else {
            return;
        };
        let previous = self.target.root.join(PREVIOUS);
        if !previous.is_dir() {
            return;
        }
        let app = self.target.app();
        if std::fs::rename(&app, partial).is_ok() {
            let _ = std::fs::rename(&previous, &app);
            let _ = std::fs::remove_dir_all(partial);
        }
    }
}

/// [`prepare`] for this daemon, if it uses a bundle. A failure is logged and means `None`: the
/// runtime then starts from `runtimes/<sha>/`, so an update never depends on the bundle.
pub fn stage(cfg: &DaemonConfig, rt: &Runtimes, sha: &str) -> Option<Staged> {
    let (target, identity) = configured(cfg);
    let target = target?;
    match prepare(&target, rt, sha, identity.as_deref()) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(
                "cannot install {sha} into {}: {e}; it runs from its runtime directory, where \
                 macOS may not apply the Local Network permission (docs/nodes.md)",
                target.app().display()
            );
            None
        }
    }
}

/// What to exec for runtime `sha`: the staged bundle's `masond`, else the runtime's own.
pub fn executable(staged: Option<&Staged>, rt: &Runtimes, sha: &str) -> PathBuf {
    match staged.map(Staged::commit) {
        Some(Ok(exe)) => exe,
        Some(Err(e)) => {
            tracing::warn!("cannot put the new bundle in place: {e}");
            rt.masond(sha)
        }
        None => rt.masond(sha),
    }
}

/// The warning for a coordinator whose `masond` is not on the internal disk. Pure.
pub fn external_warning(exe: &Path, location: Location) -> Option<String> {
    (location == Location::External).then(|| {
        format!(
            "masond runs from {}, which is not on the internal disk: macOS cannot apply its \
             Local Network permission there, so nodes may be unreachable. Set `sign_identity` \
             (and, if the home directory is external too, `app_dir`) in update.toml; see \
             docs/nodes.md",
            exe.display()
        )
    })
}

/// At start-up, on its own thread: say so if this coordinator's executable is on an external disk.
pub fn check_at_start(cfg: &DaemonConfig) {
    if !cfg!(target_os = "macos") || cfg.node.serve || cfg.runtime_sha.is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("volume-check".into())
        .spawn(|| {
            let Ok(exe) = std::env::current_exe() else {
                return;
            };
            if let Some(w) = external_warning(&exe, own_location()) {
                tracing::warn!("{w}");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stub(df: &'static str, info: &'static str) -> impl Fn(&str, &[&str]) -> Option<String> {
        move |program, _| {
            Some(match program {
                "/bin/df" => df.to_string(),
                _ => info.to_string(),
            })
        }
    }

    const DF: &str = "Filesystem 512-blocks Used Available Capacity Mounted on\n/dev/disk4s1 100 1 99 1% /Volumes/sp\n";

    #[test]
    fn a_volume_is_internal_or_external_as_its_description_says() {
        let at = Path::new("/Volumes/sp/x");
        let external = "   Device Identifier:  disk4s1\n   Device Location:           External\n";
        let internal = "   Device Location:           Internal\n";
        assert_eq!(location_with(at, &stub(DF, external)), Location::External);
        assert_eq!(location_with(at, &stub(DF, internal)), Location::Internal);
        assert_eq!(
            location_with(at, &stub(DF, "   Internal:   No\n")),
            Location::External
        );
        assert_eq!(
            location_with(at, &stub(DF, "   Volume Name: sp\n")),
            Location::Unknown
        );
    }

    #[test]
    fn a_network_share_is_not_internal_and_a_failed_tool_says_nothing() {
        let share = "Filesystem 512-blocks Used Available Capacity Mounted on\n//u@host/share 1 1 0 100% /Volumes/share\n";
        assert_eq!(
            location_with(Path::new("/Volumes/share"), &stub(share, "")),
            Location::External
        );
        assert_eq!(
            location_with(Path::new("/x"), &|_, _| None),
            Location::Unknown
        );
    }

    #[test]
    fn only_an_external_executable_is_warned_about() {
        let exe = Path::new("/Volumes/sp/masond");
        assert!(external_warning(exe, Location::External)
            .unwrap()
            .contains("/Volumes/sp/masond"));
        assert_eq!(external_warning(exe, Location::Internal), None);
        assert_eq!(external_warning(exe, Location::Unknown), None);
    }

    #[test]
    fn the_default_bundle_is_only_for_the_owners_signed_coordinator() {
        let home = Path::new("/Users/o");
        let own = Choice {
            macos: true,
            app_dir: None,
            node: false,
            signed: true,
            default_data_dir: true,
            home: Some(home),
        };
        assert_eq!(
            choose(&own),
            Some(Target {
                root: home.join("Applications"),
                register: true
            })
        );
        for other in [
            Choice {
                macos: false,
                ..own.clone()
            },
            Choice {
                node: true,
                ..own.clone()
            },
            Choice {
                signed: false,
                ..own.clone()
            },
            Choice {
                default_data_dir: false,
                ..own.clone()
            },
            Choice {
                app_dir: Some("off"),
                ..own.clone()
            },
            Choice {
                app_dir: Some("relative/dir"),
                ..own.clone()
            },
        ] {
            assert_eq!(choose(&other), None, "{other:?}");
        }
    }

    #[test]
    fn a_configured_directory_is_used_and_not_registered() {
        let home = Path::new("/Users/o");
        let c = Choice {
            macos: true,
            app_dir: Some("/opt/apps"),
            node: false,
            signed: false,
            default_data_dir: false,
            home: Some(home),
        };
        let t = choose(&c).unwrap();
        assert_eq!(
            t.masond(),
            Path::new("/opt/apps/MASON Coordinator.app/Contents/MacOS/masond")
        );
        assert!(!t.register);
        let tilde = Choice {
            app_dir: Some("~/Apps"),
            ..c
        };
        assert_eq!(choose(&tilde).unwrap().root, home.join("Apps"));
    }

    /// A fake runtime: a copy of an Apple-signed binary, signed ad hoc, so no unsigned
    /// Mach-O is ever created or deleted.
    #[cfg(target_os = "macos")]
    fn runtime(rt: &Runtimes, sha: &str, binary: &str) {
        let dir = rt.dir(sha);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["masond", "step-host"] {
            let to = dir.join(name);
            std::fs::copy(binary, &to).unwrap();
            crate::update::sign_installed(&to, None).unwrap();
        }
        std::fs::write(dir.join("SHA"), sha).unwrap();
        std::fs::write(dir.join("READY"), "").unwrap();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn two_runtimes_install_into_one_path_that_verifies_and_a_revert_goes_back() {
        let d = tempfile::tempdir().unwrap();
        let rt = Runtimes::new(&d.path().join("data"));
        runtime(&rt, "one", "/usr/bin/true");
        runtime(&rt, "two", "/usr/bin/false");
        let target = Target {
            root: d.path().join("apps"),
            register: false,
        };
        let one = prepare(&target, &rt, "one", None).unwrap();
        assert!(!target.app().exists(), "nothing changes before the commit");
        let first = one.commit().unwrap();
        verify(&target.app()).unwrap();
        assert!(Command::new(&first).status().unwrap().success());
        assert!(first.with_file_name("step-host").is_file());
        assert!(
            !target.app().join("Contents/MacOS/SHA").exists(),
            "only code goes in"
        );
        // Installed already: nothing to do.
        assert!(prepare(&target, &rt, "one", None)
            .unwrap()
            .partial
            .is_none());
        let two = prepare(&target, &rt, "two", None).unwrap();
        let second = two.commit().unwrap();
        assert_eq!(first, second);
        verify(&target.app()).unwrap();
        assert!(!Command::new(&second).status().unwrap().success());
        two.revert();
        verify(&target.app()).unwrap();
        assert!(Command::new(&first).status().unwrap().success());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn another_apps_bundle_and_a_runtime_that_is_not_code_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let rt = Runtimes::new(&d.path().join("data"));
        let target = Target {
            root: d.path().join("apps"),
            register: false,
        };
        std::fs::create_dir_all(rt.dir("script")).unwrap();
        std::fs::write(rt.masond("script"), "#!/bin/sh\n").unwrap();
        assert!(prepare(&target, &rt, "script", None)
            .unwrap_err()
            .contains("not a Mach-O"));
        std::fs::create_dir_all(target.app().join("Contents")).unwrap();
        std::fs::write(target.app().join("Contents/Info.plist"), "someone else's").unwrap();
        assert!(prepare(&target, &rt, "script", None)
            .unwrap_err()
            .contains("left alone"));
    }
}
