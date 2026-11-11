// SPDX-License-Identifier: GPL-3.0-or-later

//! The mascot art manifest (`clients/mascot.toml`): the ids and files of the one copy of the art,
//! generated into name constants for both clients and checked against the files on disk.

use std::collections::BTreeSet;
use std::path::Path;
use toml_edit::DocumentMut;

/// The groups, in the order the manifest and the generated code list them: (table, constant).
const GROUPS: [(&str, &str); 4] = [
    ("icon", "stateIds"),
    ("banner", "bannerIds"),
    ("illustration", "illustrationIds"),
    ("brand", "brandIds"),
];

pub struct Manifest {
    /// Per group (same order as `GROUPS`): each id with its files.
    pub groups: Vec<Vec<(String, Vec<String>)>>,
}

impl Manifest {
    pub fn files(&self) -> BTreeSet<String> {
        self.groups
            .iter()
            .flatten()
            .flat_map(|(_, f)| f.iter().cloned())
            .collect()
    }
}

pub fn parse(text: &str) -> Result<Manifest, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
    let mut groups = vec![];
    let mut seen = BTreeSet::new();
    for (table, _) in GROUPS {
        let mut ids = vec![];
        if let Some(t) = doc.get(table) {
            let t = t
                .as_table_like()
                .ok_or_else(|| format!("{table}: must be a table of ids"))?;
            for (id, v) in t.iter() {
                if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    return Err(format!("{table}.{id}: an id is lower-case letters and _"));
                }
                let arr = v
                    .get("files")
                    .and_then(|f| f.as_array())
                    .ok_or_else(|| format!("{table}.{id}: needs files = [..]"))?;
                let mut files = vec![];
                for f in arr.iter() {
                    let f = f
                        .as_str()
                        .ok_or_else(|| format!("{table}.{id}: files are strings"))?;
                    if !f.starts_with(&format!("{}s/", table))
                        && !f.starts_with(&format!("{table}/"))
                    {
                        return Err(format!("{table}.{id}: {f} is not under its folder"));
                    }
                    if f.contains("..") || f.contains('"') || f.contains('\\') {
                        return Err(format!("{table}.{id}: bad path {f}"));
                    }
                    if !seen.insert(f.to_string()) {
                        return Err(format!("{f} is listed twice"));
                    }
                    files.push(f.to_string());
                }
                if files.is_empty() {
                    return Err(format!("{table}.{id}: lists no files"));
                }
                ids.push((id.to_string(), files));
            }
        }
        groups.push(ids);
    }
    Ok(Manifest { groups })
}

const HEADER: &str =
    "// Generated from clients/mascot.toml by `conformance mascot --write`. Do not edit.\n";

fn list(m: &Manifest, ind: &str, open: &str, close: &str, kt: bool) -> String {
    let mut s = String::new();
    for (g, (_, name)) in GROUPS.iter().enumerate() {
        let ids: Vec<String> = m.groups[g]
            .iter()
            .map(|(i, _)| format!("\"{i}\""))
            .collect();
        if kt {
            s.push_str(&format!("{ind}val {name}: List<String> = listOf(\n"));
        } else {
            s.push_str(&format!("{ind}public static let {name}: [String] = [\n"));
        }
        for i in ids {
            s.push_str(&format!("{ind}    {i},\n"));
        }
        s.push_str(&format!("{ind}{close}\n"));
    }
    let files: Vec<String> = m.files().into_iter().collect();
    if kt {
        s.push_str(&format!("{ind}val files: List<String> = listOf(\n"));
    } else {
        s.push_str(&format!("{ind}public static let files: [String] = [\n"));
    }
    for f in files {
        s.push_str(&format!("{ind}    \"{f}\",\n"));
    }
    s.push_str(&format!("{ind}{close}\n"));
    let _ = open;
    s
}

pub fn swift(text: &str) -> Result<String, String> {
    let m = parse(text)?;
    Ok(format!(
        "// SPDX-License-Identifier: GPL-3.0-or-later\n{HEADER}\npublic enum MascotManifest {{\n{}}}\n",
        list(&m, "    ", "[", "]", false)
    ))
}

pub fn kotlin(text: &str) -> Result<String, String> {
    let m = parse(text)?;
    Ok(format!(
        "// SPDX-License-Identifier: GPL-3.0-or-later\n{HEADER}\npackage mason\n\nobject MascotManifest {{\n{}}}\n",
        list(&m, "    ", "(", ")", true)
    ))
}

fn walk(dir: &Path, base: &Path, out: &mut BTreeSet<String>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, base, out)?;
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.insert(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Files the manifest lists that are not in `art`, and files in `art` it does not list
/// (`NOTICE.md` is the one file that is not art).
pub fn check_files(m: &Manifest, art: &Path) -> Result<Vec<String>, String> {
    let mut on_disk = BTreeSet::new();
    walk(art, art, &mut on_disk)?;
    on_disk.remove("NOTICE.md");
    let listed = m.files();
    let mut problems = vec![];
    for f in listed.difference(&on_disk) {
        problems.push(format!("{f} is listed but missing"));
    }
    for f in on_disk.difference(&listed) {
        problems.push(format!("{f} is not listed in clients/mascot.toml"));
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "[icon.queued]\nfiles = [\"icons/queued-20@1x.png\"]\n[banner.paused]\nfiles = [\"banners/paused-20@1x.png\"]\n[illustration.no_tasks]\nfiles = [\"illustrations/no_tasks.png\"]\n";

    #[test]
    fn both_languages_get_the_same_ids() {
        let sw = swift(T).unwrap();
        let kt = kotlin(T).unwrap();
        assert!(sw.contains("public static let stateIds: [String] = [\n        \"queued\","));
        assert!(kt.contains("val bannerIds: List<String> = listOf(\n        \"paused\","));
        assert!(kt.contains("package mason"));
        assert!(sw.contains("\"illustrations/no_tasks.png\""));
    }

    #[test]
    fn bad_manifests_are_refused() {
        assert!(parse("[icon.Q]\nfiles=[\"icons/a.png\"]").is_err());
        assert!(parse("[icon.q]\nfiles=[\"banners/a.png\"]").is_err());
        assert!(parse("[icon.q]\nfiles=[]").is_err());
        assert!(
            parse("[icon.q]\nfiles=[\"icons/a.png\"]\n[icon.r]\nfiles=[\"icons/a.png\"]").is_err()
        );
    }

    #[test]
    fn missing_and_unlisted_files_are_found() {
        let d = std::env::temp_dir().join(format!("mascot-test-{}", std::process::id()));
        std::fs::create_dir_all(d.join("icons")).unwrap();
        std::fs::write(d.join("icons/a.png"), b"x").unwrap();
        std::fs::write(d.join("icons/b.png"), b"x").unwrap();
        std::fs::write(d.join("NOTICE.md"), b"x").unwrap();
        let m = parse("[icon.a]\nfiles=[\"icons/a.png\",\"icons/c.png\"]").unwrap();
        let p = check_files(&m, &d).unwrap();
        std::fs::remove_dir_all(&d).ok();
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p
            .iter()
            .any(|x| x.contains("c.png") && x.contains("missing")));
        assert!(p
            .iter()
            .any(|x| x.contains("b.png") && x.contains("not listed")));
    }
}
