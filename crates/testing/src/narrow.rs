// SPDX-License-Identifier: GPL-3.0-or-later

//! Which of a suite's tests a change needs, for a task's test step (s386).
//!
//! A suite with [`Narrow`] keys lists where its tests live and how to name them. In a task's
//! test step it runs only the test units the change touches: units in a test file the change
//! edits (for a file split into classes, the classes holding an added line), and units whose
//! text names a file the change edits (by its stem, as a whole word: `InitialView` for
//! `InitialView.kt`). The landing batch always runs the suite whole, so a test that reads a
//! changed file in a way this misses is still run before the change lands.
//!
//! Pure: the daemon hands it the test files' text, the changed paths and the added lines.

use crate::glob;
use crate::model::Narrow;
use regex::Regex;
use std::collections::BTreeMap;

/// One test unit: a whole test file, or one class of a file split by `narrow_split`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub file: String,
    /// First and last line of the unit in its file, 1-based, inclusive.
    pub lines: (u32, u32),
}

/// The units of the test files given (`path → text`), in path order.
pub fn units(n: &Narrow, files: &BTreeMap<String, String>) -> Vec<Unit> {
    let split = n.split.as_deref().and_then(|s| Regex::new(s).ok());
    let mut out = vec![];
    for (path, text) in files.iter().filter(|(p, _)| glob::matches(&n.tests, p)) {
        let total = text.lines().count().max(1) as u32;
        match &split {
            Some(re) => {
                let starts: Vec<(usize, String)> = re
                    .captures_iter(text)
                    .filter_map(|c| Some((c.get(0)?.start(), c.get(1)?.as_str().to_string())))
                    .collect();
                for (i, (at, name)) in starts.iter().enumerate() {
                    let first = line_of(text, *at);
                    let last = starts
                        .get(i + 1)
                        .map_or(total, |(next, _)| line_of(text, *next).saturating_sub(1));
                    out.push(Unit {
                        name: name.clone(),
                        file: path.clone(),
                        lines: (first, last.max(first)),
                    });
                }
            }
            None => out.push(Unit {
                name: name_from_path(path, n.root.as_deref()),
                file: path.clone(),
                lines: (1, total),
            }),
        }
    }
    out
}

/// What a narrowed run needs to know about the change.
pub struct Change<'a> {
    /// Every path the change touches.
    pub paths: &'a [String],
    /// Lines added or changed, per changed test file (1-based, in the new file).
    pub added: &'a BTreeMap<String, Vec<u32>>,
}

/// Why a pick is not a plain narrowing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    /// A changed path matches `narrow_never`.
    Never,
    /// Nothing is picked.
    Empty,
    /// More than half of the units are picked.
    Most,
}

impl Case {
    pub fn describe(self) -> &'static str {
        match self {
            Case::Never => "a changed path matches narrow_never",
            Case::Empty => "nothing in this change reaches its tests",
            Case::Most => "most of its tests are picked",
        }
    }
}

enum Picked {
    /// No test files found: nothing can be said, run whole.
    NoUnits,
    Plain(Vec<Unit>),
    Fallback(Case, Vec<Unit>),
}

fn pick(n: &Narrow, files: &BTreeMap<String, String>, change: &Change) -> Picked {
    if change
        .paths
        .iter()
        .any(|p| n.never.iter().any(|g| glob::matches(g, p)))
    {
        // The pick is still wanted, to run beside the smoke units.
        let all = units(n, files);
        if all.is_empty() {
            return Picked::NoUnits;
        }
        let picked = picked_units(n, files, change, &all);
        return Picked::Fallback(Case::Never, picked);
    }
    let all = units(n, files);
    if all.is_empty() {
        return Picked::NoUnits;
    }
    let picked = picked_units(n, files, change, &all);
    if picked.is_empty() {
        Picked::Fallback(Case::Empty, picked)
    } else if picked.len() * 2 > all.len() {
        Picked::Fallback(Case::Most, picked)
    } else {
        Picked::Plain(picked)
    }
}

fn picked_units(
    n: &Narrow,
    files: &BTreeMap<String, String>,
    change: &Change,
    all: &[Unit],
) -> Vec<Unit> {
    let stems: Vec<Regex> = change
        .paths
        .iter()
        .filter(|p| !glob::matches(&n.tests, p))
        .filter_map(|p| stem(p))
        .filter(|s| s.chars().count() >= 4)
        .filter_map(|s| Regex::new(&format!(r"\b{}\b", regex::escape(&s))).ok())
        .collect();
    all.iter()
        .filter(|u| {
            let edited = change.paths.contains(&u.file)
                && (n.split.is_none()
                    || change
                        .added
                        .get(&u.file)
                        .is_some_and(|ls| ls.iter().any(|l| (u.lines.0..=u.lines.1).contains(l))));
            edited || {
                let text = files.get(&u.file).map(String::as_str).unwrap_or("");
                let body: String = text
                    .lines()
                    .skip(u.lines.0.saturating_sub(1) as usize)
                    .take((u.lines.1 + 1 - u.lines.0) as usize)
                    .collect::<Vec<_>>()
                    .join("\n");
                stems.iter().any(|re| re.is_match(&body))
            }
        })
        .cloned()
        .collect()
}

/// The units this change needs, or `None` when the suite should run whole: a changed path
/// matches `narrow_never`, nothing is picked, or more than half of the units would be.
pub fn select(n: &Narrow, files: &BTreeMap<String, String>, change: &Change) -> Option<Vec<Unit>> {
    match pick(n, files, change) {
        Picked::Plain(p) => Some(p),
        _ => None,
    }
}

/// What a test step does with a suite that runs whole later (`whole_later`, #676).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Later {
    /// Run the suite whole (no test files were found to narrow by).
    Whole,
    /// Run these units: the pick, plus `narrow_smoke`, which `case` says why (`None`: a plain
    /// narrowing).
    Run(Vec<Unit>, Option<Case>),
    /// Run nothing: nothing reaches the tests and there is no smoke list.
    NotRun,
}

/// [`select`] for a suite that runs whole again later: no fallback runs it whole. An empty pick
/// runs `narrow_smoke` alone (nothing, with no smoke list); a `narrow_never` hit runs the smoke
/// units and the pick; a pick over half runs the pick.
pub fn select_later(n: &Narrow, files: &BTreeMap<String, String>, change: &Change) -> Later {
    let smoke = |mut picked: Vec<Unit>| {
        for name in &n.smoke {
            if !picked.iter().any(|u| &u.name == name) {
                picked.push(Unit {
                    name: name.clone(),
                    file: String::new(),
                    lines: (1, 1),
                });
            }
        }
        picked
    };
    match pick(n, files, change) {
        Picked::NoUnits => Later::Whole,
        Picked::Plain(p) => Later::Run(smoke(p), None),
        Picked::Fallback(Case::Most, p) => Later::Run(p, Some(Case::Most)),
        Picked::Fallback(case, p) => match smoke(p) {
            v if v.is_empty() => Later::NotRun,
            v => Later::Run(v, Some(case)),
        },
    }
}

/// The suite's narrowed command: `{tests}` becomes `narrow_each` for each unit, joined by
/// spaces. In it `{}` is the unit's name, `{file}` its file's path under `narrow_root` and
/// `{path}` its path from the repository root (#924). A smoke unit has no file; both give its
/// name.
pub fn command(n: &Narrow, picked: &[Unit]) -> String {
    let each: Vec<String> = picked
        .iter()
        .map(|u| {
            let (file, path) = if u.file.is_empty() {
                (u.name.as_str(), u.name.as_str())
            } else {
                let rel = n
                    .root
                    .as_deref()
                    .and_then(|r| u.file.strip_prefix(r))
                    .unwrap_or(&u.file);
                (rel, u.file.as_str())
            };
            n.each
                .replace("{file}", file)
                .replace("{path}", path)
                .replace("{}", &u.name)
        })
        .collect();
    n.command.replace("{tests}", &each.join(" "))
}

fn line_of(text: &str, byte: usize) -> u32 {
    text[..byte.min(text.len())].matches('\n').count() as u32 + 1
}

fn name_from_path(path: &str, root: Option<&str>) -> String {
    let rest = root.and_then(|r| path.strip_prefix(r)).unwrap_or(path);
    let rest = rest.rsplit_once('.').map_or(rest, |(a, _)| a);
    rest.replace('/', ".")
}

fn stem(path: &str) -> Option<String> {
    let file = path.rsplit('/').next()?;
    let s = file.split_once('.').map_or(file, |(a, _)| a);
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop() -> Narrow {
        Narrow {
            tests: "app/src/test/kotlin/**/*Test.kt".into(),
            root: Some("app/src/test/kotlin/".into()),
            split: None,
            command: "gradlew test {tests}".into(),
            each: "--tests {}".into(),
            never: vec!["app/*.kts".into()],
            smoke: vec![],
        }
    }

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect()
    }

    fn paths(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_ui_change_runs_its_own_test_and_the_tests_that_name_the_changed_file() {
        // Star #216 on 2026-11-14: a 48-line drop-zone change ran all 808 desktop tests.
        let f = files(&[
            (
                "app/src/test/kotlin/a/DropZoneArtTest.kt",
                "class DropZoneArtTest",
            ),
            (
                "app/src/test/kotlin/a/StartScreenTest.kt",
                r#"read("app/src/main/kotlin/a/InitialView.kt")"#,
            ),
            ("app/src/test/kotlin/a/ColorsTest.kt", "class ColorsTest"),
            ("app/src/test/kotlin/a/MenuTest.kt", "class MenuTest"),
            (
                "app/src/test/kotlin/a/ToolbarTest.kt",
                "class ToolbarTest // InitialViewer",
            ),
        ]);
        let p = paths(&[
            "app/src/main/kotlin/a/InitialView.kt",
            "app/src/test/kotlin/a/DropZoneArtTest.kt",
        ]);
        let added = BTreeMap::new();
        let picked = select(
            &desktop(),
            &f,
            &Change {
                paths: &p,
                added: &added,
            },
        )
        .expect("narrowed");
        let names: Vec<&str> = picked.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(
            names,
            ["a.DropZoneArtTest", "a.StartScreenTest"],
            "a whole word only: not InitialViewer"
        );
        assert_eq!(
            command(&desktop(), &picked),
            "gradlew test --tests a.DropZoneArtTest --tests a.StartScreenTest"
        );
    }

    #[test]
    fn a_build_file_change_nothing_picked_or_most_picked_runs_the_suite_whole() {
        let f = files(&[
            ("app/src/test/kotlin/a/OneTest.kt", "One"),
            ("app/src/test/kotlin/a/TwoTest.kt", "Two"),
            ("app/src/test/kotlin/a/ThreeTest.kt", "Three"),
        ]);
        let added = BTreeMap::new();
        let whole = |p: &[&str]| {
            let p = paths(p);
            select(
                &desktop(),
                &f,
                &Change {
                    paths: &p,
                    added: &added,
                },
            )
            .is_none()
        };
        assert!(
            whole(&["app/build.gradle.kts", "app/src/test/kotlin/a/OneTest.kt"]),
            "narrow_never"
        );
        assert!(whole(&["docs/notes.md"]), "nothing picked");
        assert!(
            whole(&[
                "app/src/test/kotlin/a/OneTest.kt",
                "app/src/test/kotlin/a/TwoTest.kt"
            ]),
            "most picked"
        );
        assert!(!whole(&["app/src/test/kotlin/a/OneTest.kt"]));
    }

    #[test]
    fn a_file_split_into_classes_picks_the_classes_that_hold_an_added_line() {
        let gui = Narrow {
            tests: "gui/starTests/*.swift".into(),
            root: None,
            split: Some(r"(?m)^(?:final )?class (\w+)\s*:\s*XCTestCase".into()),
            command: "xcodebuild test {tests}".into(),
            each: "-only-testing:starTests/{}".into(),
            never: vec![],
            smoke: vec![],
        };
        let text = "import XCTest\nclass AlphaTests: XCTestCase {\n  func testA() {}\n}\nclass DropZoneTests: XCTestCase {\n  func testDrop() {}\n  func testNew() {}\n}\nclass GammaTests: XCTestCase {\n  func testG() {}\n}\nclass DeltaTests: XCTestCase {\n  func testD() { FinderStyleDropZone() }\n}\nclass EpsilonTests: XCTestCase {\n}\n";
        let f = files(&[("gui/starTests/starTests.swift", text)]);
        let p = paths(&[
            "gui/starTests/starTests.swift",
            "gui/star/FinderStyleDropZone.swift",
        ]);
        let added = BTreeMap::from([("gui/starTests/starTests.swift".to_string(), vec![7])]);
        let picked = select(
            &gui,
            &f,
            &Change {
                paths: &p,
                added: &added,
            },
        )
        .expect("narrowed");
        let names: Vec<&str> = picked.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, ["DropZoneTests", "DeltaTests"]);
        assert_eq!(
            command(&gui, &picked),
            "xcodebuild test -only-testing:starTests/DropZoneTests -only-testing:starTests/DeltaTests"
        );
    }

    fn pytest(each: &str) -> Narrow {
        Narrow {
            tests: "pipeline/tests/test_*.py".into(),
            root: Some("pipeline/".into()),
            split: None,
            command: "uv run pytest -v {tests}".into(),
            each: each.into(),
            never: vec![],
            smoke: vec![],
        }
    }

    fn unit(name: &str, file: &str) -> Unit {
        Unit {
            name: name.into(),
            file: file.into(),
            lines: (1, 1),
        }
    }

    #[test]
    fn file_and_path_placeholders_name_the_unit_by_its_file() {
        let u = [unit("tests.test_engine", "pipeline/tests/test_engine.py")];
        let run = |each: &str| command(&pytest(each), &u);
        assert_eq!(run("{file}"), "uv run pytest -v tests/test_engine.py");
        assert_eq!(
            run("{path}"),
            "uv run pytest -v pipeline/tests/test_engine.py"
        );
        assert_eq!(run("{}"), "uv run pytest -v tests.test_engine");
    }

    #[test]
    fn a_split_unit_gives_its_file_and_its_class() {
        let u = [unit("TestA", "pipeline/tests/test_x.py")];
        assert_eq!(
            command(&pytest("{file}::{}"), &u),
            "uv run pytest -v tests/test_x.py::TestA"
        );
    }
}
