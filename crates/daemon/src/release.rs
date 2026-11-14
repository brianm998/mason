// SPDX-License-Identifier: GPL-3.0-or-later

//! The `release` update channel: a packaged install has no source and no toolchain, so it
//! updates from the newest published GitHub release. This module is only the look, the
//! download, the checksum and the unpacking; the takeover is `update.rs`'s and is not touched
//! here. Transport is the system `curl` (as git is the system `git`): the daemon has no HTTP
//! client, and one that does TLS would add a dependency tree to a file (deny.toml) that only
//! the owner changes. Nothing is installed unless the SHA-256 in `SHA256SUMS` matches.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The release API of the repository; `/releases/latest` and `/releases/tags/v<ver>` follow.
pub const DEFAULT_API: &str = "https://api.github.com/repos/brianm998/mason";
/// The page that picks the right installer for a visitor.
pub const DEFAULT_PAGE: &str = "https://brianm998.github.io/mason/";

/// A version, `X.Y.Z` with an optional `-pre` part (which sorts before the release).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    nums: (u64, u64, u64),
    /// A pre-release is older than the same numbers without one.
    final_release: bool,
    pre: String,
}

impl Version {
    /// `1.2.3`, `v1.2.3`, `1.2.3-rc1`. Anything else, including build metadata, is not one.
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        if let Some(p) = pre {
            if p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
                return None;
            }
        }
        let mut it = core.split('.');
        let mut n = || -> Option<u64> {
            let p = it.next()?;
            if p.is_empty() || p.len() > 9 || !p.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        };
        let nums = (n()?, n()?, n()?);
        if it.next().is_some() {
            return None;
        }
        Some(Version {
            nums,
            final_release: pre.is_none(),
            pre: pre.unwrap_or_default().into(),
        })
    }

    /// The name used for directories and `--runtime-sha`: without the `v`.
    pub fn text(&self) -> String {
        let (a, b, c) = self.nums;
        if self.final_release {
            format!("{a}.{b}.{c}")
        } else {
            format!("{a}.{b}.{c}-{}", self.pre)
        }
    }
}

/// This build's own version, for a daemon that does not run from a runtime yet.
pub fn own_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The asset for a platform, per docs/RELEASING.md; `None` where there is no tarball
/// (Windows ships a zip and its takeover is another task's).
pub fn asset_name(version: &str, os: &str, arch: &str) -> Option<String> {
    let target = match (os, arch) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return None,
    };
    Some(format!("mason-{version}-{target}.tar.gz"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// `(name, download url)`.
    pub assets: Vec<(String, String)>,
}

impl Release {
    pub fn parse(json: &str) -> Result<Release, String> {
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("the release is not JSON: {e}"))?;
        let tag = v["tag_name"]
            .as_str()
            .ok_or("the release has no tag_name")?;
        let version =
            Version::parse(tag).ok_or_else(|| format!("the release tag {tag:?} is not vX.Y.Z"))?;
        let assets = v["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some((
                            x["name"].as_str()?.to_string(),
                            x["browser_download_url"].as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Release { version, assets })
    }

    fn asset(&self, name: &str) -> Option<&str> {
        self.assets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, u)| u.as_str())
    }
}

/// The scheme curl may use: the one the configured API uses, nothing else. A redirect may
/// only go to https. The real API is https, so a release cannot point a daemon at a file.
fn scheme_of(url: &str) -> Result<&'static str, String> {
    if url.starts_with("https://") {
        Ok("https")
    } else if url.starts_with("http://") {
        Ok("http")
    } else if url.starts_with("file://") {
        Ok("file")
    } else {
        Err(format!("{url:?} is not an https, http or file URL"))
    }
}

fn curl(url: &str, scheme: &str, out: &Path) -> Result<(), String> {
    let o = Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "30",
            // A stalled transfer ends; a slow one may take as long as it needs.
            "--speed-limit",
            "1",
            "--speed-time",
            "60",
            "--proto",
            &format!("={scheme}"),
            "--proto-redir",
            "=https",
            "-H",
            "Accept: application/vnd.github+json",
            "-A",
            "mason-update",
            "-o",
        ])
        .arg(out)
        .arg(url)
        .output()
        .map_err(|e| format!("cannot run curl: {e} (updates from a release need it)"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "downloading {url} failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

/// Ask for a release: the newest, or the one tagged `v<want>`.
pub fn lookup(api: &str, want: Option<&str>, scratch: &Path) -> Result<Release, String> {
    let api = api.trim_end_matches('/');
    let scheme = scheme_of(api)?;
    let url = match want {
        Some(w) => {
            let v = Version::parse(w).ok_or_else(|| format!("{w:?} is not a release version"))?;
            format!("{api}/releases/tags/v{}", v.text())
        }
        None => format!("{api}/releases/latest"),
    };
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let f = scratch.join("release.json");
    curl(&url, scheme, &f)?;
    let text = std::fs::read_to_string(&f).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&f);
    Release::parse(&text)
}

/// Pure: the SHA-256 listed for `name` in a `SHA256SUMS` file.
pub fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (h, n) = l.split_once(char::is_whitespace)?;
        let n = n.trim_start().trim_start_matches('*');
        (n == name).then(|| h.to_ascii_lowercase())
    })
}

pub fn sha256_hex(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| e.to_string())?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// What stands in for the owner's minisign public key until they pin the real one. It is not a
/// key and is never handed to minisign: while it is here no release is checked for a signature.
pub const PUBKEY_PLACEHOLDER: &str = "REPLACE_WITH_MINISIGN_PUBLIC_KEY";
/// The owner's minisign public key (the second line of `minisign.pub`). Releases are refused
/// unless `SHA256SUMS.minisig` verifies against it. scripts/install.sh pins the same key.
pub const PINNED_PUBKEY: &str = PUBKEY_PLACEHOLDER;

/// A minisign public key is 42 bytes in base64: 56 characters starting `RW`.
fn looks_like_key(k: &str) -> bool {
    k.len() == 56
        && k.starts_with("RW")
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// Pure: the key releases must be signed with. `from_env` is `MASON_MINISIGN_PUBKEY` (it
/// overrides the pinned key, as it does in install.sh); `pinned` is the compiled-in key.
/// `Ok(None)` only when the placeholder is still pinned and nothing overrides it; anything
/// that is set but is not a key (the placeholder included) is an error, never "unsigned ok".
pub fn trusted_key(from_env: Option<&str>, pinned: &str) -> Result<Option<String>, String> {
    let given = from_env.map(str::trim).filter(|k| !k.is_empty());
    match given {
        Some(k) if looks_like_key(k) => Ok(Some(k.to_string())),
        Some(k) => Err(format!(
            "MASON_MINISIGN_PUBKEY {k:?} is not a minisign public key; refusing to update"
        )),
        None if pinned == PUBKEY_PLACEHOLDER => Ok(None),
        None if looks_like_key(pinned) => Ok(Some(pinned.to_string())),
        None => Err("the pinned minisign public key is malformed; refusing to update".into()),
    }
}

/// Check `sums` against its `sig` with the system `minisign` and `key`.
pub fn verify_signature(sums: &Path, sig: &Path, key: &str) -> Result<(), String> {
    let o = Command::new("minisign")
        .arg("-Vm")
        .arg(sums)
        .args(["-P", key, "-x"])
        .arg(sig)
        .output()
        .map_err(|e| {
            format!("cannot run minisign: {e} (a signed release needs it; refusing the download)")
        })?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "the signature of SHA256SUMS does not verify; refusing the download: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}

/// What the tarball is unpacked from: the files of `mason-<ver>-<target>/` we install.
pub const INSTALL: [&str; 6] = [
    "masond",
    "step-host",
    "mason-mcp",
    "fake-claude",
    "mason",
    "mason-askpass.sh",
];

/// Download the tarball for this platform and `SHA256SUMS`, check the one against the other,
/// and unpack it into `dest` (which is made fresh). With a `pubkey`, the release's
/// `SHA256SUMS.minisig` must be there and verify against it first; unsigned is refused. Returns the names installed. On any
/// failure `dest` is removed; nothing outside `work` and `dest` is touched.
pub fn fetch_and_unpack(
    api: &str,
    rel: &Release,
    os: &str,
    arch: &str,
    work: &Path,
    dest: &Path,
    pubkey: Option<&str>,
) -> Result<Vec<String>, String> {
    let scheme = scheme_of(api.trim_end_matches('/'))?;
    let ver = rel.version.text();
    let name = asset_name(&ver, os, arch)
        .ok_or_else(|| format!("there is no release tarball for {os} on {arch}"))?;
    let url = rel.asset(&name).ok_or_else(|| {
        format!("release {ver} has no {name}; it was not built for this platform")
    })?;
    let sums_url = rel.asset("SHA256SUMS").ok_or_else(|| {
        format!("release {ver} has no SHA256SUMS; refusing an unverifiable download")
    })?;
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let result = (|| {
        let tarball = work.join(&name);
        let sums = work.join("SHA256SUMS");
        curl(sums_url, scheme, &sums)?;
        if let Some(key) = pubkey {
            let sig_url = rel.asset("SHA256SUMS.minisig").ok_or_else(|| {
                format!("release {ver} has no SHA256SUMS.minisig but a key is pinned; refusing an unsigned release")
            })?;
            let sig = work.join("SHA256SUMS.minisig");
            curl(sig_url, scheme, &sig)?;
            verify_signature(&sums, &sig, key)?;
        }
        curl(url, scheme, &tarball)?;
        let sums = std::fs::read_to_string(&sums).map_err(|e| e.to_string())?;
        let want = checksum_for(&sums, &name)
            .ok_or_else(|| format!("SHA256SUMS does not list {name}; refusing it"))?;
        let got = sha256_hex(&tarball)?;
        if got != want {
            return Err(format!(
                "the checksum of {name} is {got}, SHA256SUMS says {want}; refusing the download"
            ));
        }
        unpack(&tarball, &format!("mason-{ver}-"), work, dest)
    })();
    let _ = std::fs::remove_dir_all(work);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dest);
    }
    result
}

fn unpack(
    tarball: &Path,
    top_prefix: &str,
    work: &Path,
    dest: &Path,
) -> Result<Vec<String>, String> {
    let list = Command::new("tar")
        .arg("-tzf")
        .arg(tarball)
        .output()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    if !list.status.success() {
        return Err("the tarball cannot be read".into());
    }
    let listing = String::from_utf8_lossy(&list.stdout);
    let top = listing
        .lines()
        .next()
        .map(|l| l.split('/').next().unwrap_or_default().to_string())
        .unwrap_or_default();
    let inside = |l: &str| l.trim_end_matches('/') == top || l.starts_with(&format!("{top}/"));
    let escapes = |l: &str| l.starts_with('/') || l.split('/').any(|c| c == "..");
    if !top.starts_with(top_prefix) || listing.lines().any(|l| escapes(l) || !inside(l)) {
        return Err(
            "the tarball does not have one top-level mason-<version>-<target> folder".into(),
        );
    }
    let ex: PathBuf = work.join("x");
    std::fs::create_dir_all(&ex).map_err(|e| e.to_string())?;
    let st = Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(&ex)
        .status()
        .map_err(|e| format!("cannot run tar: {e}"))?;
    if !st.success() {
        return Err("unpacking the tarball failed".into());
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut got = vec![];
    for f in INSTALL {
        let from = ex.join(&top).join(f);
        if from.is_file() {
            std::fs::copy(&from, dest.join(f)).map_err(|e| format!("copying {f}: {e}"))?;
            got.push(f.to_string());
        }
    }
    for need in ["masond", "step-host", "mason-mcp", "fake-claude"] {
        if !got.iter().any(|g| g == need) {
            return Err(format!("the tarball has no {need}"));
        }
    }
    Ok(got)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_placeholder_is_never_a_key() {
        const K: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        assert_eq!(trusted_key(None, PUBKEY_PLACEHOLDER), Ok(None));
        assert!(trusted_key(Some(PUBKEY_PLACEHOLDER), K).is_err());
        assert!(trusted_key(Some("RWshort"), K).is_err());
        assert!(trusted_key(None, "garbage").is_err());
        assert_eq!(trusted_key(None, K), Ok(Some(K.to_string())));
        assert_eq!(
            trusted_key(Some(K), PUBKEY_PLACEHOLDER),
            Ok(Some(K.to_string()))
        );
        assert_eq!(trusted_key(Some("  "), PUBKEY_PLACEHOLDER), Ok(None));
    }

    use super::*;

    #[test]
    fn versions_compare_as_numbers_and_prereleases_sort_first() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("v1.0.0") > v("1.0.0-rc1"));
        assert!(v("1.0.0-rc2") > v("1.0.0-rc1"));
        assert_eq!(v("v1.2.3").text(), "1.2.3");
        for bad in [
            "", "1.2", "1.2.3.4", "../1.2.3", "1.2.x", "1.2.3-", "1.2.3+b",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_checksum_is_found_by_exact_name() {
        let s = "AA11  mason-1.0.0-x.tar.gz\nbb22  other\n";
        assert_eq!(
            checksum_for(s, "mason-1.0.0-x.tar.gz").as_deref(),
            Some("aa11")
        );
        assert_eq!(checksum_for(s, "mason-1.0.0-x"), None);
    }

    #[test]
    fn asset_names_follow_the_contract() {
        assert_eq!(
            asset_name("1.0.0", "macos", "aarch64").as_deref(),
            Some("mason-1.0.0-aarch64-apple-darwin.tar.gz")
        );
        assert_eq!(asset_name("1.0.0", "windows", "x86_64"), None);
    }
}
