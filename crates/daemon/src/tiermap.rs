// SPDX-License-Identifier: GPL-3.0-or-later

//! What `deep`/`light` and `low`/`medium`/`high` mean for each provider: the
//! built-in table, then `<config dir>/providers/<provider>.toml` (machine-wide),
//! then `[provider.<provider>]` in the project file, entry by entry. (A project file
//! that has such tables names its provider with `use` in `[provider]`, because TOML
//! cannot also hold `provider = "codex"`.) The pure
//! parts live in `mason_definition::tiers`; this module only reads the files.

use crate::config::Provider;
use mason_definition::tiers::{parse_table, MapSource, Mapping};
use std::path::Path;
use toml_edit::DocumentMut;

pub fn name(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    }
}

/// The efforts the provider's CLI accepts.
pub fn effort_levels(p: Provider) -> Vec<String> {
    match p {
        Provider::Claude => mason_adapters::claude::EFFORT_LEVELS
            .map(String::from)
            .to_vec(),
        Provider::Codex => mason_adapters::codex::EFFORT_LEVELS
            .map(String::from)
            .to_vec(),
    }
}

/// A provider's mapping as it is in force, and what was wrong in the files.
#[derive(Clone, Debug)]
pub struct Effective {
    pub mapping: Mapping,
    pub problems: Vec<String>,
}

/// Build the mapping for `provider`. A provider file with any problem is
/// ignored whole (the built-in table stays for that provider until it is
/// fixed); a bad entry in the project file is ignored on its own.
pub fn effective(config_dir: &Path, provider: Provider, project_text: &str) -> Effective {
    let pname = name(provider);
    let efforts = effort_levels(provider);
    let mut mapping = Mapping::builtin(pname);
    let mut problems = vec![];

    let path = config_dir.join("providers").join(format!("{pname}.toml"));
    match std::fs::read_to_string(&path) {
        Ok(text) => match text.parse::<DocumentMut>() {
            Ok(doc) => {
                let (good, bad) = parse_table(doc.as_table(), &efforts);
                if bad.is_empty() {
                    mapping.overlay(good, MapSource::ProviderFile);
                } else {
                    problems.extend(bad.into_iter().map(|b| {
                        format!(
                            "{}: {b}; the built-in mapping is used for {pname} until it is fixed",
                            path.display()
                        )
                    }));
                }
            }
            Err(e) => problems.push(format!(
                "{}: not valid TOML ({e}); the built-in mapping is used for {pname} until it is fixed",
                path.display()
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => problems.push(format!("{}: cannot be read ({e})", path.display())),
    }

    if let Some(t) = project_text
        .parse::<DocumentMut>()
        .ok()
        .as_ref()
        .and_then(|d| d.get("provider"))
        .and_then(|p| p.as_table_like())
        .and_then(|p| p.get(pname))
        .and_then(|p| p.as_table_like())
    {
        let (good, bad) = parse_table(t, &efforts);
        mapping.overlay(good, MapSource::ProjectFile);
        problems.extend(
            bad.into_iter()
                .map(|b| format!("project file [provider.{pname}]: {b}; that entry is ignored")),
        );
    }
    Effective { mapping, problems }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mason_definition::tiers::{Level, Tier};

    fn dir_with(file: Option<(&str, &str)>) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        if let Some((n, t)) = file {
            std::fs::create_dir_all(d.path().join("providers")).unwrap();
            std::fs::write(d.path().join("providers").join(n), t).unwrap();
        }
        d
    }

    #[test]
    fn no_files_means_the_builtin_mapping() {
        let d = dir_with(None);
        let e = effective(d.path(), Provider::Claude, "");
        assert!(e.problems.is_empty());
        assert_eq!(e.mapping, Mapping::builtin("claude"));
    }

    #[test]
    fn the_provider_file_then_the_project_file_override_entry_by_entry() {
        let d = dir_with(Some((
            "claude.toml",
            "[deep]\nhigh = { model = \"opus\", effort = \"max\" }\nlow = { model = \"o2\" }\n",
        )));
        let e = effective(
            d.path(),
            Provider::Claude,
            "[provider.claude.deep]\nlow = { model = \"proj\" }\n",
        );
        assert!(e.problems.is_empty(), "{:?}", e.problems);
        let (hi, s) = e.mapping.get(Tier::Deep, Level::High).unwrap();
        assert_eq!(
            (hi.model.as_deref(), hi.effort.as_deref(), s),
            (Some("opus"), Some("max"), &MapSource::ProviderFile)
        );
        let (lo, s) = e.mapping.get(Tier::Deep, Level::Low).unwrap();
        assert_eq!(
            (lo.model.as_deref(), s),
            (Some("proj"), &MapSource::ProjectFile)
        );
        assert_eq!(
            e.mapping.get(Tier::Light, Level::Low).unwrap().1,
            MapSource::Builtin
        );
    }

    #[test]
    fn a_bad_provider_file_is_reported_and_ignored_whole() {
        let d = dir_with(Some((
            "claude.toml",
            "[deep]\nhigh = { model = \"opus\", effort = \"ultra\" }\nlow = { model = \"fine\" }\n",
        )));
        let e = effective(d.path(), Provider::Claude, "");
        assert!(
            e.problems[0].contains("`ultra`") && e.problems[0].contains("built-in mapping is used")
        );
        assert_eq!(e.mapping, Mapping::builtin("claude"));
        // Codex's file is a different file, and its efforts are its own.
        let e = effective(d.path(), Provider::Codex, "");
        assert!(e.problems.is_empty());
    }

    #[test]
    fn a_bad_project_entry_is_dropped_alone() {
        let d = dir_with(None);
        let e = effective(
            d.path(),
            Provider::Codex,
            "[provider.codex.deep]\nlow = { effort = \"max\" }\nhigh = { effort = \"low\" }\n",
        );
        assert_eq!(e.problems.len(), 1);
        assert_eq!(
            e.mapping.entry(Tier::Deep, Level::Low).effort.as_deref(),
            Some("low"),
            "built-in stays"
        );
        assert_eq!(
            e.mapping.get(Tier::Deep, Level::High).unwrap().1,
            MapSource::ProjectFile
        );
    }
}
