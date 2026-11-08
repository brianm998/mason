// SPDX-License-Identifier: GPL-3.0-or-later

//! Tiers and levels: what a pipeline says (`deep`, `light`; `low`, `medium`,
//! `high`) and, kept apart in each provider's config, the real model and real
//! effort that means for that provider. Everything here is pure: the daemon
//! reads the provider files and passes the parsed [`Mapping`] in.

use crate::layers::Resolved;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Deep,
    Light,
}

impl Tier {
    pub const ALL: [Tier; 2] = [Tier::Deep, Tier::Light];

    /// `fast` is another name for `light`.
    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "deep" => Some(Tier::Deep),
            "light" | "fast" => Some(Tier::Light),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::Deep => "deep",
            Tier::Light => "light",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Level {
    Low,
    #[default]
    Medium,
    High,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Low, Level::Medium, Level::High];

    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "low" => Some(Level::Low),
            "medium" => Some(Level::Medium),
            "high" => Some(Level::High),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::Low => "low",
            Level::Medium => "medium",
            Level::High => "high",
        }
    }
}

/// The abstract levels a definition may use for `effort`.
pub const ABSTRACT_LEVELS: [&str; 3] = ["low", "medium", "high"];

/// A real model and a real effort. `None` model: the CLI's own default;
/// `None` effort: no effort flag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl Entry {
    fn of(model: Option<&str>, effort: Option<&str>) -> Entry {
        Entry {
            model: model.map(String::from),
            effort: effort.map(String::from),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapSource {
    Builtin,
    ProviderFile,
    ProjectFile,
}

impl MapSource {
    pub fn label(self) -> &'static str {
        match self {
            MapSource::Builtin => "built-in",
            MapSource::ProviderFile => "provider file",
            MapSource::ProjectFile => "project file",
        }
    }
}

/// Every (tier, level) of one provider, each with where it came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapping {
    entries: BTreeMap<(Tier, Level), (Entry, MapSource)>,
}

impl Mapping {
    /// The built-in table for a provider (`claude`, `codex`); empty for any other.
    ///
    /// Claude at `medium` is what the profiles always were (`deep` = sonnet,
    /// `light` = haiku, no effort), so nothing costs more after an upgrade.
    /// Codex leaves the model to its CLI and maps onto its own efforts.
    pub fn builtin(provider: &str) -> Mapping {
        use Level::*;
        use Tier::*;
        let rows: Vec<((Tier, Level), Entry)> = match provider {
            "claude" => vec![
                ((Deep, Low), Entry::of(Some("sonnet"), Some("low"))),
                ((Deep, Medium), Entry::of(Some("sonnet"), None)),
                ((Deep, High), Entry::of(Some("sonnet"), Some("high"))),
                ((Light, Low), Entry::of(Some("haiku"), None)),
                ((Light, Medium), Entry::of(Some("haiku"), None)),
                ((Light, High), Entry::of(Some("sonnet"), Some("low"))),
            ],
            "codex" => vec![
                ((Deep, Low), Entry::of(None, Some("low"))),
                ((Deep, Medium), Entry::of(None, Some("medium"))),
                ((Deep, High), Entry::of(None, Some("high"))),
                ((Light, Low), Entry::of(None, Some("minimal"))),
                ((Light, Medium), Entry::of(None, Some("low"))),
                ((Light, High), Entry::of(None, Some("medium"))),
            ],
            _ => vec![],
        };
        Mapping {
            entries: rows
                .into_iter()
                .map(|(k, e)| (k, (e, MapSource::Builtin)))
                .collect(),
        }
    }

    pub fn get(&self, tier: Tier, level: Level) -> Option<&(Entry, MapSource)> {
        self.entries.get(&(tier, level))
    }

    pub fn entry(&self, tier: Tier, level: Level) -> Entry {
        self.get(tier, level)
            .map(|(e, _)| e.clone())
            .unwrap_or_default()
    }

    pub fn set(&mut self, tier: Tier, level: Level, entry: Entry, source: MapSource) {
        self.entries.insert((tier, level), (entry, source));
    }

    /// Lay `entries` over this mapping, entry by entry.
    pub fn overlay(&mut self, entries: Vec<(Tier, Level, Entry)>, source: MapSource) {
        for (t, l, e) in entries {
            self.set(t, l, e, source);
        }
    }

    pub fn rows(&self) -> impl Iterator<Item = (Tier, Level, &Entry, MapSource)> {
        self.entries.iter().map(|((t, l), (e, s))| (*t, *l, e, *s))
    }
}

/// Read the tables of a mapping: `[deep]` and `[light]`, each holding `low`,
/// `medium` and `high` as `{ model = "...", effort = "..." }`. Returns the
/// good entries and every problem; the caller decides whether problems void
/// the lot (a provider file) or only the entries concerned (a project file).
pub fn parse_table(
    table: &dyn toml_edit::TableLike,
    efforts: &[String],
) -> (Vec<(Tier, Level, Entry)>, Vec<String>) {
    let mut good = vec![];
    let mut bad = vec![];
    for (tname, tv) in table.iter() {
        let Some(tier) = Tier::parse(tname).filter(|_| tname != "fast") else {
            bad.push(format!("`{tname}` is not a tier; tiers are deep, light"));
            continue;
        };
        let Some(lt) = tv.as_table_like() else {
            bad.push(format!("{tname} must be a table of low, medium, high"));
            continue;
        };
        for (lname, lv) in lt.iter() {
            let path = format!("{tname}.{lname}");
            let Some(level) = Level::parse(lname) else {
                bad.push(format!(
                    "{path}: `{lname}` is not a level; levels are low, medium, high"
                ));
                continue;
            };
            let Some(et) = lv.as_table_like() else {
                bad.push(format!(
                    "{path} must be a table like {{ model = \"...\", effort = \"...\" }}"
                ));
                continue;
            };
            let mut entry = Entry::default();
            let mut ok = true;
            for (k, v) in et.iter() {
                match (k, v.as_str()) {
                    ("model", Some(m)) if !m.trim().is_empty() => entry.model = Some(m.into()),
                    ("effort", Some(e)) if efforts.iter().any(|x| x == e) => {
                        entry.effort = Some(e.into())
                    }
                    ("effort", Some(e)) => {
                        ok = false;
                        bad.push(format!(
                            "{path}.effort: `{e}` is not an effort this provider has; it has {}",
                            efforts.join(", ")
                        ));
                    }
                    ("model" | "effort", _) => {
                        ok = false;
                        bad.push(format!("{path}.{k} must be a non-empty string"));
                    }
                    _ => {
                        ok = false;
                        bad.push(format!("{path}: unknown key `{k}`; keys are model, effort"));
                    }
                }
            }
            if ok && entry == Entry::default() {
                ok = false;
                bad.push(format!("{path} sets neither model nor effort"));
            }
            if ok {
                good.push((tier, level, entry));
            }
        }
    }
    (good, bad)
}

/// What will be launched, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub tier: Option<Tier>,
    pub level: Level,
    /// What the mapping alone would have launched.
    pub mapped: Entry,
    pub model_from: String,
    pub effort_from: String,
    /// The owner (a runtime or per-task override) named the model or the effort.
    pub overridden: bool,
}

/// Turn the requested tier and level into the real model and effort.
///
/// Model, highest first: a runtime or per-task override, the step's own real
/// model, the project's `model` key (all three are `res.model`, which the
/// layers have already ordered), the mapping, the ladder's bottom, then the
/// CLI's default (`None`). Effort: an explicit real effort, else the mapped one.
pub fn launch(res: &Resolved, mapping: &Mapping, ladder: &[String]) -> Launch {
    let tier = res.tier.as_ref().map(|a| a.value);
    let level = res.level.as_ref().map(|a| a.value).unwrap_or_default();
    let mapped = tier.map(|t| mapping.entry(t, level)).unwrap_or_default();
    let mapped_from = tier
        .and_then(|t| mapping.get(t, level))
        .map_or("built-in", |(_, s)| s.label());
    let (model, model_from) = if let Some(a) = &res.model {
        (Some(a.value.clone()), a.source.label())
    } else if let Some(m) = &mapped.model {
        (Some(m.clone()), format!("the {mapped_from} mapping"))
    } else if let Some(m) = ladder.first() {
        (Some(m.clone()), "the bottom of the ladder".to_string())
    } else {
        (None, "the CLI's default".to_string())
    };
    let (effort, effort_from) = if let Some(a) = &res.effort {
        (Some(a.value.clone()), a.source.label())
    } else if let Some(e) = &mapped.effort {
        (Some(e.clone()), format!("the {mapped_from} mapping"))
    } else {
        (None, "the CLI's default".to_string())
    };
    let by_owner = |s: &crate::layers::Source| {
        matches!(
            s,
            crate::layers::Source::Runtime { .. } | crate::layers::Source::Task
        )
    };
    let overridden = res.model.as_ref().is_some_and(|a| by_owner(&a.source))
        || res.effort.as_ref().is_some_and(|a| by_owner(&a.source));
    Launch {
        model,
        effort,
        tier,
        level,
        mapped,
        model_from,
        effort_from,
        overridden,
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// What a display string says besides the model: the ladder, an override, and
/// whether the step has started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    /// `(n, of)`: this is ladder retry `n` of at most `of`.
    pub retry: Option<(u32, u32)>,
    pub overridden: bool,
    /// Not launched yet: this is what the step is expected to use.
    pub expected: bool,
}

/// The real model and effort alone: `Opus max`, `Haiku`, `Codex default, high`.
fn real(provider: &str, model: Option<&str>, effort: Option<&str>) -> String {
    match (model, effort) {
        (Some(m), Some(e)) => format!("{} {e}", cap(m)),
        (Some(m), None) => cap(m),
        (None, Some(e)) => format!("{} default, {e}", cap(provider)),
        (None, None) => format!("{} default", cap(provider)),
    }
}

/// What a step asks for and what that becomes, for the editor's inspector:
/// `deep at high → Opus max`. Without a tier only the real model shows.
pub fn describe_request(
    tier: Option<Tier>,
    level: Level,
    provider: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> String {
    let base = real(provider, model, effort);
    match tier {
        Some(t) => format!("{} at {} \u{2192} {base}", t.name(), level.name()),
        None => base,
    }
}

/// The one display string, for every client: `deep (Opus medium)`.
///
/// `model` / `effort` are the real ones passed to the CLI (`None` model: the
/// CLI's own default, named for its `provider`). With no tier the model stands
/// alone: `Opus medium`. Only the first letter of a name is capitalised; the
/// name is otherwise as the provider writes it.
pub fn describe(
    tier: Option<Tier>,
    provider: &str,
    model: Option<&str>,
    effort: Option<&str>,
    note: &Note,
) -> String {
    let base = real(provider, model, effort);
    let mut notes: Vec<String> = vec![];
    if let Some((n, of)) = note.retry {
        notes.push(format!("retry {n} of {of}"));
    }
    if note.overridden {
        notes.push("override".into());
    }
    if note.expected {
        notes.push("expected".into());
    }
    match (tier, notes.is_empty()) {
        (Some(t), true) => format!("{} ({base})", t.name()),
        (Some(t), false) => format!("{} ({base}, {})", t.name(), notes.join(", ")),
        (None, true) => base,
        (None, false) => format!("{base} ({})", notes.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn efforts(of: &[&str]) -> Vec<String> {
        of.iter().map(|s| s.to_string()).collect()
    }
    fn table(text: &str) -> toml_edit::DocumentMut {
        text.parse().unwrap()
    }

    #[test]
    fn names_and_the_fast_alias() {
        assert_eq!(Tier::parse("fast"), Some(Tier::Light));
        assert_eq!(Tier::parse("light"), Some(Tier::Light));
        assert_eq!(Tier::parse("deep"), Some(Tier::Deep));
        assert_eq!(Tier::parse("huge"), None);
        assert_eq!(Level::default(), Level::Medium);
        assert_eq!(Level::parse("max"), None);
    }

    #[test]
    fn builtin_claude_keeps_todays_models_at_medium() {
        let m = Mapping::builtin("claude");
        assert_eq!(
            m.entry(Tier::Deep, Level::Medium),
            Entry::of(Some("sonnet"), None)
        );
        assert_eq!(
            m.entry(Tier::Light, Level::Medium),
            Entry::of(Some("haiku"), None)
        );
        let c = Mapping::builtin("codex");
        for (t, l, e, s) in c.rows() {
            assert_eq!(
                (e.model.is_none(), s),
                (true, MapSource::Builtin),
                "{t:?} {l:?}"
            );
            assert!(e.effort.is_some());
        }
        assert_eq!(c.rows().count(), 6);
        assert_eq!(Mapping::builtin("claude").rows().count(), 6);
    }

    #[test]
    fn a_table_is_read_and_every_problem_reported() {
        let d = table(
            "[deep]\nlow = { model = \"a\", effort = \"high\" }\nhigh = { effort = \"ultra\" }\n[light]\nmedium = {}\nextra = { model = \"x\" }\n[huge]\n",
        );
        let (good, bad) = parse_table(d.as_table(), &efforts(&["low", "high"]));
        assert_eq!(
            good,
            vec![(Tier::Deep, Level::Low, Entry::of(Some("a"), Some("high")))]
        );
        let all = bad.join("\n");
        assert!(
            all.contains("deep.high.effort: `ultra`") && all.contains("it has low, high"),
            "{all}"
        );
        assert!(all.contains("light.medium sets neither"), "{all}");
        assert!(all.contains("`extra` is not a level"), "{all}");
        assert!(all.contains("`huge` is not a tier"), "{all}");
    }

    #[test]
    fn entries_layer_by_source() {
        let mut m = Mapping::builtin("claude");
        m.overlay(
            vec![(
                Tier::Deep,
                Level::High,
                Entry::of(Some("opus"), Some("max")),
            )],
            MapSource::ProviderFile,
        );
        m.overlay(
            vec![(Tier::Deep, Level::High, Entry::of(Some("big"), None))],
            MapSource::ProjectFile,
        );
        let (e, s) = m.get(Tier::Deep, Level::High).unwrap();
        assert_eq!(
            (e.model.as_deref(), s),
            (Some("big"), &MapSource::ProjectFile)
        );
        assert_eq!(m.get(Tier::Deep, Level::Low).unwrap().1, MapSource::Builtin);
    }

    fn resolved(tier: Option<Tier>, level: Level) -> Resolved {
        use crate::layers::{Attr, Source};
        Resolved {
            tier: tier.map(|value| Attr {
                value,
                source: Source::Builtin,
            }),
            level: Some(Attr {
                value: level,
                source: Source::Builtin,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn launch_uses_the_mapped_model_and_effort_and_keeps_effort_with_an_override() {
        let m = Mapping::builtin("claude");
        let r = resolved(Some(Tier::Deep), Level::High);
        let l = launch(&r, &m, &[]);
        assert_eq!(
            (l.model.as_deref(), l.effort.as_deref()),
            (Some("sonnet"), Some("high"))
        );
        let mut r2 = r.clone();
        r2.model = Some(crate::layers::Attr {
            value: "opus".into(),
            source: crate::layers::Source::Task,
        });
        let l = launch(&r2, &m, &[]);
        assert_eq!(
            (l.model.as_deref(), l.effort.as_deref()),
            (Some("opus"), Some("high"))
        );
        let c = launch(
            &resolved(Some(Tier::Light), Level::Low),
            &Mapping::builtin("codex"),
            &[],
        );
        assert_eq!((c.model, c.effort.as_deref()), (None, Some("minimal")));
    }

    #[test]
    fn describe_covers_every_form() {
        let n = Note::default();
        let d = |t, p, m, e, note: &Note| describe(t, p, m, e, note);
        let deep = Some(Tier::Deep);
        assert_eq!(
            d(deep, "claude", Some("opus"), Some("medium"), &n),
            "deep (Opus medium)"
        );
        assert_eq!(
            d(Some(Tier::Light), "claude", Some("haiku"), None, &n),
            "light (Haiku)"
        );
        assert_eq!(
            d(deep, "codex", None, Some("high"), &n),
            "deep (Codex default, high)"
        );
        assert_eq!(d(deep, "codex", None, None, &n), "deep (Codex default)");
        let retry = Note {
            retry: Some((1, 2)),
            ..Default::default()
        };
        assert_eq!(
            d(deep, "claude", Some("opus"), Some("medium"), &retry),
            "deep (Opus medium, retry 1 of 2)"
        );
        let over = Note {
            overridden: true,
            ..Default::default()
        };
        assert_eq!(
            d(deep, "claude", Some("opus"), Some("medium"), &over),
            "deep (Opus medium, override)"
        );
        assert_eq!(
            d(None, "claude", Some("opus"), Some("medium"), &n),
            "Opus medium"
        );
        assert_eq!(
            d(None, "claude", Some("opus"), Some("medium"), &over),
            "Opus medium (override)"
        );
        assert_eq!(
            d(None, "codex", None, Some("low"), &n),
            "Codex default, low"
        );
        let both = Note {
            retry: Some((2, 2)),
            overridden: true,
            expected: false,
        };
        assert_eq!(
            d(deep, "claude", Some("fable"), None, &both),
            "deep (Fable, retry 2 of 2, override)"
        );
        let exp = Note {
            expected: true,
            ..Default::default()
        };
        assert_eq!(
            d(deep, "claude", Some("opus"), Some("max"), &exp),
            "deep (Opus max, expected)"
        );
        // Names are never translated: only the first letter is capitalised.
        assert_eq!(
            d(None, "claude", Some("claude-opus-4-1"), None, &n),
            "Claude-opus-4-1"
        );
    }

    #[test]
    fn the_inspector_line_says_what_is_asked_and_what_it_becomes() {
        assert_eq!(
            describe_request(
                Some(Tier::Deep),
                Level::High,
                "claude",
                Some("opus"),
                Some("max")
            ),
            "deep at high \u{2192} Opus max"
        );
        assert_eq!(
            describe_request(None, Level::High, "claude", Some("opus"), None),
            "Opus"
        );
    }

    #[test]
    fn launch_marks_an_owner_override_only() {
        use crate::layers::{Attr, Source};
        let m = Mapping::builtin("claude");
        let mut r = resolved(Some(Tier::Deep), Level::Medium);
        assert!(!launch(&r, &m, &[]).overridden);
        r.model = Some(Attr {
            value: "opus".into(),
            source: Source::Project,
        });
        assert!(!launch(&r, &m, &[]).overridden);
        r.model = Some(Attr {
            value: "opus".into(),
            source: Source::Task,
        });
        assert!(launch(&r, &m, &[]).overridden);
    }
}
