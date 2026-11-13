// SPDX-License-Identifier: GPL-3.0-or-later

//! A push the host refuses for credentials is not the change's fault, and every other task of the
//! batch would meet the same wall. So landing is held for the project, an alert says which account
//! the host saw, and the owner picks the SSH key (from the monitor or `mason ssh-key`). Choosing
//! writes the repository's `core.sshCommand`, unlocks the key if it has a passphrase, and probes
//! the remote; only a probe that gets through lifts the hold.
//!
//! The passphrase never passes through MASON: `ssh-add` asks for it through `SSH_ASKPASS`, a
//! dialog on the daemon's own Mac, and keeps it in the Keychain. It is not a request field, a log
//! line or a store row.

use crate::daemon::{now_ms, Daemon};
use mason_landing::git;
use mason_landing::proc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The owner of the pause that holds landing for a refused push.
pub const PAUSE_OWNER: &str = "masond:push-auth";
/// How long the hold lasts if nobody answers.
const HOLD_SECS: u64 = 7 * 24 * 3600;
const TOOL_TIMEOUT: Duration = Duration::from_secs(15);

/// What GitHub called us, per project: the alert's words. The pause row is the fact; this is
/// only what it says about it.
static SEEN: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

fn seen_map<T>(f: impl FnOnce(&mut HashMap<String, Option<String>>) -> T) -> T {
    f(SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Default::default))
}

pub fn alert_key(project: &str) -> String {
    format!("push-auth:{project}")
}

/// Hold landing for `project` after the host refused our credentials.
pub fn hold(d: &Arc<Daemon>, project: &str, seen_as: Option<String>) {
    seen_map(|m| m.insert(project.to_string(), seen_as));
    if !held(d, project) {
        let now = now_ms();
        let _ = d
            .store
            .add_pause(project, "landing", PAUSE_OWNER, now, now + HOLD_SECS * 1000);
    }
    d.poke();
}

/// Is landing held for `project` because of a refused push?
pub fn held(d: &Daemon, project: &str) -> bool {
    d.store
        .pauses(now_ms())
        .unwrap_or_default()
        .iter()
        .any(|p| p.project == project && p.mode == "landing" && p.owner == PAUSE_OWNER)
}

/// Lift the hold, let what waited go on.
pub fn lift(d: &Arc<Daemon>, project: &str) {
    let _ = d.store.remove_pauses_of(project, "landing", PAUSE_OWNER);
    seen_map(|m| m.remove(project));
    d.admit();
    d.landing().wake();
    d.poke();
}

/// The text of the alert, naming the account the host saw and the one the repository belongs to.
pub fn alert_text(d: &Daemon, project: &str, seen_as: Option<&str>, landing_held: bool) -> String {
    let owner = d
        .project_id_by_name(project)
        .and_then(|p| d.project(p))
        .and_then(|p| p.remote.clone().map(|r| (p.path.clone(), r)))
        .and_then(|(path, r)| remote_url(&path, &r))
        .and_then(|u| github_repo(&u))
        .and_then(|r| r.split('/').next().map(String::from));
    let saw = match seen_as {
        Some(u) => format!("it saw {u}"),
        None => "it did not accept the credentials".to_string(),
    };
    let belongs = owner.map_or(String::new(), |o| format!(", the repo belongs to {o}"));
    let held = if landing_held {
        " Landing is held."
    } else {
        ""
    };
    format!("{project}: GitHub refused the push: {saw}{belongs}.{held} Choose the SSH key.")
}

/// The `push-auth:<project>` alerts that are true now: a hold, with what the host said.
pub fn alerts(d: &Daemon) -> Vec<(String, String, String)> {
    let mut out = vec![];
    for p in d.store.pauses(now_ms()).unwrap_or_default() {
        if p.mode != "landing" || p.owner != PAUSE_OWNER {
            continue;
        }
        let seen = seen_map(|m| m.get(&p.project).cloned().flatten());
        out.push((
            alert_key(&p.project),
            "landing".to_string(),
            alert_text(d, &p.project, seen.as_deref(), true),
        ));
    }
    out
}

/// The URL of `remote` in the repository at `path` (`remote` may already be a URL).
fn remote_url(path: &Path, remote: &str) -> Option<String> {
    if remote.contains(':') || remote.contains('/') {
        return Some(remote.to_string());
    }
    git::run(path, &["remote", "get-url", remote]).ok()
}

/// `owner/repo` of a GitHub remote URL (https, scp-like or ssh), if it is one.
pub fn github_repo(url: &str) -> Option<String> {
    let u = url.trim();
    let rest = ["https://", "http://", "ssh://", "git://"]
        .iter()
        .find_map(|s| u.strip_prefix(s))
        .map(|r| r.rsplit_once('@').map_or(r, |(_, h)| h))
        .map(|r| r.split_once('/'))
        .unwrap_or_else(|| {
            // scp-like: git@github.com:owner/repo.git
            let r = u.rsplit_once('@').map_or(u, |(_, h)| h);
            r.split_once(':')
        })?;
    let (host, path) = rest;
    let host = host.split(':').next().unwrap_or(host);
    if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut it = path.split('/');
    match (it.next(), it.next(), it.next()) {
        (Some(o), Some(r), None) if !o.is_empty() && !r.is_empty() => Some(format!("{o}/{r}")),
        _ => None,
    }
}

// ---- the candidates ----------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Key {
    /// The private key's file, if there is one.
    pub path: Option<PathBuf>,
    pub fingerprint: String,
    pub comment: String,
    pub in_agent: bool,
    pub encrypted: bool,
    pub on_owner_account: bool,
    pub seen_as: Option<String>,
}

impl Key {
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path.as_ref().map(|p| p.display().to_string()),
            "fingerprint": self.fingerprint,
            "comment": self.comment,
            "in_agent": self.in_agent,
            "encrypted": self.encrypted,
            "on_owner_account": self.on_owner_account,
            "seen_as": self.seen_as,
        })
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `ssh-keygen -lf` of public key text: `(fingerprint, comment)`.
fn fingerprint_of(pub_text: &str) -> Option<(String, String)> {
    let mut c = Command::new("ssh-keygen")
        .args(["-lf", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    use std::io::Write;
    c.stdin.take()?.write_all(pub_text.as_bytes()).ok()?;
    let out = c.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let mut it = line.split_whitespace();
    it.next()?; // bits
    let fp = it.next()?.to_string();
    let rest: Vec<&str> = it.collect();
    // The last word is the type, "(ED25519)"; what is before it is the comment.
    let comment = rest
        .split_last()
        .map_or(String::new(), |(_, c)| c.join(" "));
    Some((fp, comment))
}

fn agent_keys() -> Vec<(String, String)> {
    let Ok(o) = proc::output(Command::new("ssh-add").arg("-L"), TOOL_TIMEOUT) else {
        return vec![];
    };
    if !o.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| l.starts_with("ssh-") || l.starts_with("ecdsa-") || l.starts_with("sk-"))
        .filter_map(fingerprint_of)
        .collect()
}

/// Is the private key at `path` protected by a passphrase? `ssh-keygen -y -P ''` fails only then.
fn is_encrypted(path: &Path) -> bool {
    proc::output(
        Command::new("ssh-keygen")
            .args(["-y", "-P", "", "-f"])
            .arg(path),
        TOOL_TIMEOUT,
    )
    .map_or(true, |o| !o.status.success())
}

/// The fingerprints on the account's public key page (`<base>/<owner>.keys`). `None` if the page
/// could not be read. The base is `https://github.com` unless `MASON_GITHUB_URL` says otherwise.
fn account_fingerprints(owner: &str) -> Option<Vec<String>> {
    let base = std::env::var("MASON_GITHUB_URL").unwrap_or_else(|_| "https://github.com".into());
    let o = proc::output(
        Command::new("curl")
            .args(["-fsS", "--max-time", "10"])
            .arg(format!("{base}/{owner}.keys")),
        TOOL_TIMEOUT,
    )
    .ok()
    .filter(|o| o.status.success())?;
    Some(
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| fingerprint_of(l).map(|f| f.0))
            .collect(),
    )
}

/// The user GitHub says a key belongs to (`Hi <user>!`). Only possible when the key can sign
/// without asking: in the agent, or without a passphrase.
fn github_user_of(path: &Path) -> Option<String> {
    let host = std::env::var("MASON_GITHUB_SSH").unwrap_or_else(|_| "git@github.com".into());
    let o = proc::output(
        Command::new("ssh")
            .args(["-T", "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes"])
            .args(["-o", "ConnectTimeout=10", "-i"])
            .arg(path)
            .arg(host),
        TOOL_TIMEOUT,
    )
    .ok()?;
    let text = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
    let rest = text.split("Hi ").nth(1)?;
    let user: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!user.is_empty()).then_some(user)
}

/// The keys in `ssh_dir` (every `*.pub` with its private key beside it) and in the agent, without
/// the questions that cost a round trip.
fn candidates(ssh_dir: &Path) -> Vec<Key> {
    let agent = agent_keys();
    let mut out: Vec<Key> = vec![];
    let mut files: Vec<PathBuf> = std::fs::read_dir(ssh_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pub"))
        .collect();
    files.sort();
    for pubf in files {
        let private = pubf.with_extension("");
        if !private.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&pubf) else {
            continue;
        };
        let Some((fp, comment)) = fingerprint_of(&text) else {
            continue;
        };
        out.push(Key {
            in_agent: agent.iter().any(|(f, _)| *f == fp),
            path: Some(private),
            fingerprint: fp,
            comment,
            encrypted: false,
            on_owner_account: false,
            seen_as: None,
        });
    }
    for (fp, comment) in agent {
        if !out.iter().any(|k| k.fingerprint == fp) {
            out.push(Key {
                path: None,
                fingerprint: fp,
                comment,
                in_agent: true,
                encrypted: false,
                on_owner_account: false,
                seen_as: None,
            });
        }
    }
    out
}

/// The account a project's remote belongs to, for GitHub remotes.
fn remote_owner(d: &Daemon, project: &str) -> Option<String> {
    let p = d.project(d.project_id_by_name(project)?)?;
    let url = remote_url(&p.path, p.remote.as_deref()?)?;
    github_repo(&url)?.split('/').next().map(String::from)
}

/// Every candidate key for `project`, the owner's account first.
pub fn list(d: &Daemon, project: &str) -> Result<Vec<Key>, String> {
    d.project_id_by_name(project)
        .ok_or_else(|| format!("no project {project:?}"))?;
    let dir = home().ok_or("no home directory")?.join(".ssh");
    Ok(list_in(&dir, remote_owner(d, project).as_deref()))
}

pub fn list_in(ssh_dir: &Path, owner: Option<&str>) -> Vec<Key> {
    let mut keys = candidates(ssh_dir);
    let on_account = owner.and_then(account_fingerprints);
    std::thread::scope(|s| {
        for k in keys.iter_mut() {
            let on_account = on_account.as_ref();
            s.spawn(move || {
                if let Some(p) = k.path.as_deref() {
                    k.encrypted = is_encrypted(p);
                }
                k.on_owner_account = on_account.is_some_and(|f| f.contains(&k.fingerprint));
                if owner.is_some() && (k.in_agent || !k.encrypted) {
                    if let Some(p) = k.path.as_deref() {
                        k.seen_as = github_user_of(p);
                    }
                }
            });
        }
    });
    keys.sort_by_key(|k| (!k.on_owner_account, k.path.is_none()));
    keys
}

// ---- choosing ----------------------------------------------------------------------------

fn shell_word(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+=@%~".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The `core.sshCommand` that uses only `key`.
pub fn ssh_command(key: &Path) -> String {
    let mut c = format!(
        "/usr/bin/ssh -i {} -o IdentitiesOnly=yes",
        shell_word(&key.display().to_string())
    );
    if cfg!(target_os = "macos") {
        c.push_str(" -o UseKeychain=yes -o AddKeysToAgent=yes");
    }
    c
}

/// Where the askpass helper is: `MASON_ASKPASS`, else beside the running binary.
pub fn askpass_script() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MASON_ASKPASS") {
        return Some(PathBuf::from(p));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join("mason-askpass.sh"),
        dir.join("../packaging/mason-askpass.sh"),
        dir.join("../../packaging/mason-askpass.sh"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Add `key` to the agent and the Keychain. `ssh-add` asks for the passphrase through `askpass`
/// (a dialog on this Mac); the passphrase is never seen here. `ssh_add` is `/usr/bin/ssh-add`
/// unless `MASON_SSH_ADD` says otherwise.
pub fn unlock(key: &Path, askpass: Option<&Path>) -> Result<(), String> {
    if !cfg!(target_os = "macos") && std::env::var_os("MASON_SSH_ADD").is_none() {
        return Err(format!(
            "this key has a passphrase and this host cannot ask for it: run `ssh-add {}` on the daemon's host by hand",
            key.display()
        ));
    }
    let askpass = askpass.ok_or(
        "the passphrase dialog (mason-askpass.sh) is not installed beside masond; run `ssh-add --apple-use-keychain <key>` by hand",
    )?;
    let bin = std::env::var("MASON_SSH_ADD").unwrap_or_else(|_| "/usr/bin/ssh-add".into());
    let mut cmd = Command::new(bin);
    if cfg!(target_os = "macos") && std::env::var_os("MASON_SSH_ADD").is_none() {
        cmd.arg("--apple-use-keychain");
    }
    cmd.arg(key)
        .env("SSH_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env(
            "DISPLAY",
            std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()),
        );
    // The owner has to see the dialog and answer it; that is not a step to hurry.
    let o = proc::output(&mut cmd, Duration::from_secs(180))
        .map_err(|e| format!("ssh-add did not finish: {e:?}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ssh-add did not add the key (cancelled, or the passphrase was wrong): {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

/// Use `path` for `project`'s remote. `Ok` has what happened; `Err` is why the key was not
/// accepted (the hold stays).
pub fn choose(d: &Arc<Daemon>, project: &str, path: &str) -> Result<Value, String> {
    let pid = d
        .project_id_by_name(project)
        .ok_or_else(|| format!("no project {project:?}"))?;
    let p = d.project(pid).ok_or("no such project")?;
    let remote = p.remote.clone().ok_or("the project has no remote")?;
    let dir = home().ok_or("no home directory")?.join(".ssh");
    choose_in(
        d,
        project,
        &p.path,
        &remote,
        &dir,
        path,
        askpass_script().as_deref(),
    )
}

pub fn choose_in(
    d: &Arc<Daemon>,
    project: &str,
    repo: &Path,
    remote: &str,
    ssh_dir: &Path,
    path: &str,
    askpass: Option<&Path>,
) -> Result<Value, String> {
    let wanted = std::fs::canonicalize(path).map_err(|e| format!("{path}: {e}"))?;
    // Only a key that was offered: a request cannot point git at an arbitrary file.
    let key = candidates(ssh_dir)
        .into_iter()
        .find(|k| {
            k.path
                .as_ref()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .is_some_and(|p| p == wanted)
        })
        .ok_or_else(|| format!("{path} is not one of the keys in {}", ssh_dir.display()))?;
    let key_path = key.path.clone().unwrap_or(wanted);
    git::run(
        repo,
        &[
            "config",
            "--local",
            "core.sshCommand",
            &ssh_command(&key_path),
        ],
    )
    .map_err(|e| e.to_string())?;
    let mut unlocked = false;
    if !key.in_agent && is_encrypted(&key_path) {
        unlock(&key_path, askpass)?;
        unlocked = true;
    }
    match git::run(repo, &["ls-remote", remote, "HEAD"]) {
        Ok(_) => {
            lift(d, project);
            crate::attention::sync_alerts(d);
            Ok(json!({"ok": true, "unlocked": unlocked, "path": key_path.display().to_string()}))
        }
        Err(e) => Err(format!(
            "the remote still refuses this key; landing stays held: {}",
            e.stderr.trim()
        )),
    }
}

/// Raise the hold for a failure whose text the host's refusal is in; `true` if it was one.
pub fn hold_if_refused(d: &Arc<Daemon>, project: &str, text: &str) -> bool {
    match git::auth_failure(text) {
        Some(a) => {
            hold(d, project, a.seen_as);
            true
        }
        None => false,
    }
}
