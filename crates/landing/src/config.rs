// SPDX-License-Identifier: GPL-3.0-or-later

//! `.mason/landing.toml` (Part 9): the mode, the base, the window and size of a
//! batch, how many may run at once, the tiers of suites, and the red ladder.
//!
//! ```toml
//! mode     = "direct"
//! base     = "develop"
//! window   = "20m"        # collect from the oldest ready task
//! max_size = 0            # 0 means no cap
//! parallel = 2            # extra batches only if files and suites don't overlap
//!
//! [[tier]]  name = "batch";     suites = ["integration"]
//! [[tier]]  name = "post-land"; suites = ["smoke"]; on_red = "freeze"
//!
//! [on_red]
//! ladder = ["rerun-failed", "blame-by-impact", "bisect", "ask"]
//! ```

use std::time::Duration;
use toml_edit::{DocumentMut, Item};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Direct,
    DirectCi,
    LocalOnly,
    LocalOnlyCi,
    PrEach,
    PrBatch,
    HostQueue,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Direct => "direct",
            Mode::DirectCi => "direct-ci",
            Mode::LocalOnly => "local-only",
            Mode::LocalOnlyCi => "local-only-ci",
            Mode::PrEach => "pr-each",
            Mode::PrBatch => "pr-batch",
            Mode::HostQueue => "host-queue",
        }
    }

    pub const ALL: [Mode; 7] = [
        Mode::Direct,
        Mode::DirectCi,
        Mode::LocalOnly,
        Mode::LocalOnlyCi,
        Mode::PrEach,
        Mode::PrBatch,
        Mode::HostQueue,
    ];

    pub fn from_name(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == s)
    }

    /// The modes this version can land with.
    pub fn runnable(self) -> bool {
        matches!(self, Mode::Direct | Mode::LocalOnly)
    }

    /// Does this mode stop at the local base branch, leaving the push to the owner?
    pub fn local_only(self) -> bool {
        matches!(self, Mode::LocalOnly | Mode::LocalOnlyCi)
    }

    /// Does this mode use CI? Only these may name `ci` in a tier.
    pub fn uses_ci(self) -> bool {
        !matches!(self, Mode::Direct | Mode::LocalOnly)
    }

    /// The same mode with CI, for the error that points to it.
    fn with_ci(self) -> &'static str {
        match self {
            Mode::LocalOnly => "local-only-ci",
            _ => "direct-ci",
        }
    }
}

/// One rung of the red ladder, in the order they are tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rung {
    RerunFailed,
    BlameByImpact,
    Bisect,
    Ask,
}

impl Rung {
    pub fn name(self) -> &'static str {
        match self {
            Rung::RerunFailed => "rerun-failed",
            Rung::BlameByImpact => "blame-by-impact",
            Rung::Bisect => "bisect",
            Rung::Ask => "ask",
        }
    }

    pub fn from_name(s: &str) -> Option<Rung> {
        [
            Rung::RerunFailed,
            Rung::BlameByImpact,
            Rung::Bisect,
            Rung::Ask,
        ]
        .into_iter()
        .find(|r| r.name() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tier {
    pub name: String,
    pub suites: Vec<String>,
    /// A CI system, for the CI modes (Stage 15).
    pub ci: Option<String>,
    /// `freeze` on the post-land tier: a red stops landing but for fixes.
    pub on_red: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandingConfig {
    pub mode: Mode,
    /// Overrides the project's base branch.
    pub base: Option<String>,
    /// How long to collect from the oldest ready task. Zero: go at once.
    pub window: Duration,
    /// 0 means no cap.
    pub max_size: usize,
    /// Batches running at once (they must not overlap in files or suites).
    pub parallel: usize,
    pub tiers: Vec<Tier>,
    pub ladder: Vec<Rung>,
    /// A command that resolves a merge conflict in the integration worktree:
    /// exit 0 with the conflict resolved and the tree clean, or the task is
    /// ejected. (The resolving agent plugs in here.)
    pub resolver: Option<String>,
    /// How many times the base may move under a batch before it gives up.
    pub max_fold: u32,
}

impl Default for LandingConfig {
    fn default() -> Self {
        LandingConfig {
            mode: Mode::Direct,
            base: None,
            window: Duration::ZERO,
            max_size: 0,
            parallel: 1,
            tiers: vec![],
            ladder: vec![
                Rung::RerunFailed,
                Rung::BlameByImpact,
                Rung::Bisect,
                Rung::Ask,
            ],
            resolver: None,
            max_fold: 3,
        }
    }
}

impl LandingConfig {
    pub fn tier(&self, name: &str) -> Option<&Tier> {
        self.tiers.iter().find(|t| t.name == name)
    }

    /// Suites of the tier a batch runs over its union diff.
    pub fn batch_suites(&self) -> Option<&[String]> {
        self.tier("batch").map(|t| t.suites.as_slice())
    }

    /// Suites run on the base after landing.
    pub fn post_land(&self) -> Option<&Tier> {
        self.tier("post-land")
    }
}

/// `4h`, `20m`, `90s`, `1h30m`.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let (mut total, mut num, mut any) = (0u64, String::new(), false);
    for c in s.trim().chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            let n: u64 = num.parse().ok()?;
            num.clear();
            total += n * match c {
                'h' => 3600,
                'm' => 60,
                's' => 1,
                'd' => 86400,
                _ => return None,
            };
            any = true;
        }
    }
    (num.is_empty() && any).then(|| Duration::from_secs(total))
}

const TOP: &[&str] = &[
    "mode", "base", "window", "max_size", "parallel", "tier", "on_red", "resolver", "max_fold",
];

/// Read a `landing.toml`. `Err` lists every problem, each naming its key.
pub fn parse(text: &str) -> Result<LandingConfig, Vec<String>> {
    let doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| vec![format!("not valid TOML: {e}")])?;
    let mut c = LandingConfig::default();
    let mut p: Vec<String> = vec![];
    for (k, v) in doc.iter() {
        match k {
            "mode" => match v.as_str().map(|s| (s, Mode::from_name(s))) {
                Some((_, Some(m))) => c.mode = m,
                Some((s, None)) => p.push(format!(
                    "mode: unknown `{s}`; the modes are {}",
                    Mode::ALL.map(Mode::name).join(", ")
                )),
                None => p.push("mode: must be a string".into()),
            },
            "base" => match v.as_str() {
                Some(s) if crate::git::valid_branch(s) => c.base = Some(s.into()),
                Some(s) => p.push(format!("base: {s:?} is not a plain branch name")),
                None => p.push("base: must be a string".into()),
            },
            "window" => match v.as_str().and_then(parse_duration) {
                Some(d) => c.window = d,
                None => p.push("window: must be a duration like \"20m\" (or \"0s\")".into()),
            },
            "max_size" => match v.as_integer() {
                Some(n) if n >= 0 => c.max_size = n as usize,
                _ => p.push("max_size: must be a whole number (0 means no cap)".into()),
            },
            "parallel" => match v.as_integer() {
                Some(n) if (1..=16).contains(&n) => c.parallel = n as usize,
                _ => p.push("parallel: must be a whole number from 1 to 16".into()),
            },
            "max_fold" => match v.as_integer() {
                Some(n) if (1..=20).contains(&n) => c.max_fold = n as u32,
                _ => p.push("max_fold: must be a whole number from 1 to 20".into()),
            },
            "resolver" => match v.as_str() {
                Some(s) if !s.trim().is_empty() => c.resolver = Some(s.into()),
                _ => p.push("resolver: must be a command".into()),
            },
            "tier" => match v.as_array_of_tables() {
                Some(tables) => {
                    for (i, t) in tables.iter().enumerate() {
                        let mut tier = Tier {
                            name: String::new(),
                            suites: vec![],
                            ci: None,
                            on_red: None,
                        };
                        for (tk, tv) in t.iter() {
                            match tk {
                                "name" => tier.name = tv.as_str().unwrap_or_default().to_string(),
                                "suites" => match tv.as_array() {
                                    Some(a) => {
                                        tier.suites = a
                                            .iter()
                                            .filter_map(|x| x.as_str().map(String::from))
                                            .collect()
                                    }
                                    None => p.push(format!(
                                        "tier[{i}].suites: must be a list of suite names"
                                    )),
                                },
                                "ci" => tier.ci = tv.as_str().map(String::from),
                                "on_red" => tier.on_red = tv.as_str().map(String::from),
                                other => p.push(format!("tier[{i}]: unknown key `{other}`")),
                            }
                        }
                        if tier.name.is_empty() {
                            p.push(format!("tier[{i}].name: required"));
                        }
                        if let Some(r) = &tier.on_red {
                            if r != "freeze" {
                                p.push(format!(
                                    "tier `{}`.on_red: only \"freeze\" is known, not {r:?}",
                                    tier.name
                                ));
                            }
                        }
                        c.tiers.push(tier);
                    }
                }
                None => p.push("tier: write each as [[tier]]".into()),
            },
            "on_red" => match v.as_table_like().and_then(|t| t.get("ladder")) {
                Some(l) => match l.as_array() {
                    Some(a) => {
                        let mut ladder = vec![];
                        for x in a.iter() {
                            match x.as_str().and_then(Rung::from_name) {
                                Some(r) => ladder.push(r),
                                None => p.push(format!(
                                    "on_red.ladder: unknown rung {x}; the rungs are rerun-failed, blame-by-impact, bisect, ask"
                                )),
                            }
                        }
                        c.ladder = ladder;
                    }
                    None => p.push("on_red.ladder: must be a list".into()),
                },
                None => p.push("on_red: only `ladder` is known".into()),
            },
            other => {
                let close = TOP
                    .iter()
                    .find(|t| t.starts_with(&other[..other.len().min(2)]));
                p.push(format!(
                    "unknown key `{other}`{}",
                    close.map_or(String::new(), |c| format!("; did you mean `{c}`?"))
                ));
            }
        }
    }
    let _: Option<&Item> = None;
    p.extend(validate(&c));
    if p.is_empty() {
        Ok(c)
    } else {
        Err(p)
    }
}

/// What must hold of a config: tiers that name CI need a CI mode, and the
/// two non-CI modes refuse them, pointing to the CI version of themselves.
pub fn validate(c: &LandingConfig) -> Vec<String> {
    let mut p = vec![];
    for t in &c.tiers {
        if t.ci.is_some() && !c.mode.uses_ci() {
            p.push(format!(
                "tier `{}` names ci, which mode `{}` does not use: use `{}` to run CI on the batch",
                t.name,
                c.mode.name(),
                c.mode.with_ci()
            ));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for t in &c.tiers {
        if !seen.insert(&t.name) {
            p.push(format!("tier `{}` is defined twice", t.name));
        }
    }
    p
}

/// Can this version land with the config? Modes beyond `direct` and
/// `local-only` parse and validate but do not run yet.
pub fn check_runnable(c: &LandingConfig) -> Result<(), String> {
    if c.mode.runnable() {
        return Ok(());
    }
    Err(format!(
        "mode `{}` arrives in Stage 15 (GitHub, CI and PR landing); this version lands with `direct` and `local-only`",
        c.mode.name()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
mode = "direct"
base = "develop"
window = "20m"
max_size = 5
parallel = 2
resolver = "true"

[[tier]]
name = "batch"
suites = ["integration"]

[[tier]]
name = "post-land"
suites = ["smoke"]
on_red = "freeze"

[on_red]
ladder = ["rerun-failed", "blame-by-impact", "bisect", "ask"]
"#;

    #[test]
    fn the_whole_file_reads() {
        let c = parse(FULL).unwrap();
        assert_eq!(c.mode, Mode::Direct);
        assert_eq!(c.base.as_deref(), Some("develop"));
        assert_eq!(c.window, Duration::from_secs(1200));
        assert_eq!((c.max_size, c.parallel), (5, 2));
        assert_eq!(c.batch_suites().unwrap(), ["integration"]);
        assert_eq!(c.post_land().unwrap().on_red.as_deref(), Some("freeze"));
        assert_eq!(c.ladder.len(), 4);
        assert!(check_runnable(&c).is_ok());
    }

    #[test]
    fn an_empty_file_is_the_default_direct_landing() {
        let c = parse("").unwrap();
        assert_eq!(c, LandingConfig::default());
        assert_eq!(c.window, Duration::ZERO);
    }

    #[test]
    fn the_non_ci_modes_refuse_a_tier_that_names_ci_and_point_to_the_ci_version() {
        let mk = |mode: &str| {
            format!("mode = \"{mode}\"\n[[tier]]\nname = \"batch\"\nsuites = [\"x\"]\nci = \"github-actions\"\n")
        };
        let e = parse(&mk("direct")).unwrap_err();
        assert!(
            e[0].contains("names ci") && e[0].contains("use `direct-ci`"),
            "{e:?}"
        );
        let e = parse(&mk("local-only")).unwrap_err();
        assert!(e[0].contains("use `local-only-ci`"), "{e:?}");
        // The CI modes accept it (and do not run yet).
        let c = parse(&mk("direct-ci")).unwrap();
        assert!(check_runnable(&c).unwrap_err().contains("Stage 15"));
    }

    #[test]
    fn modes_beyond_this_stage_parse_but_do_not_run() {
        for m in [
            "direct-ci",
            "local-only-ci",
            "pr-each",
            "pr-batch",
            "host-queue",
        ] {
            let c = parse(&format!("mode = \"{m}\"")).unwrap();
            assert!(check_runnable(&c).unwrap_err().contains(m), "{m}");
        }
        for m in ["direct", "local-only"] {
            assert!(check_runnable(&parse(&format!("mode = \"{m}\"")).unwrap()).is_ok());
        }
    }

    #[test]
    fn every_mistake_is_named() {
        for (text, want) in [
            ("mode = \"yolo\"", "unknown `yolo`"),
            ("mode = 3", "mode: must be a string"),
            ("window = \"soon\"", "window:"),
            ("max_size = -1", "max_size:"),
            ("parallel = 0", "parallel:"),
            ("base = \"a b\"", "base:"),
            (
                "modee = \"direct\"",
                "unknown key `modee`; did you mean `mode`?",
            ),
            ("[on_red]\nladder = [\"pray\"]", "unknown rung"),
            ("[[tier]]\nsuites = [\"a\"]", "tier[0].name: required"),
            (
                "[[tier]]\nname = \"post-land\"\non_red = \"explode\"",
                "only \"freeze\"",
            ),
            (
                "[[tier]]\nname = \"a\"\n[[tier]]\nname = \"a\"",
                "defined twice",
            ),
            ("not toml [", "not valid TOML"),
        ] {
            let e = parse(text).unwrap_err();
            assert!(
                e.iter().any(|m| m.contains(want)),
                "{text:?} wanted `{want}` in {e:?}"
            );
        }
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("20m"), Some(Duration::from_secs(1200)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("0s"), Some(Duration::ZERO));
        assert_eq!(parse_duration("20"), None);
        assert_eq!(parse_duration("m"), None);
    }
}
