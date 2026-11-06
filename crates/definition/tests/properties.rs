// SPDX-License-Identifier: GPL-3.0-or-later

//! Property tests: the validator is *sound*. Whatever definition it accepts,
//! every possible run through it, whatever the steps report, ends in `landed`
//! or `failed` within a bound, and the real reducer accepts every transition on
//! the way. (A validator that accepted a graph that could run forever, or
//! strand a task, would fail here.)

use mason_core::{Route, State};
use mason_definition::dryrun::{walk, End};
use mason_definition::graph::Outcome;
use mason_definition::{parse, validate, Capabilities};
use proptest::prelude::*;

#[derive(Clone, Debug)]
struct Gen {
    cats: Vec<u8>,                // category index per step, ascending; the last is landing
    kinds: Vec<u8>,               // type choice per step
    next_mut: Vec<Option<u8>>,    // occasionally break `next`: Some(code)
    red: Vec<Option<(u8, bool)>>, // on_red target pick and whether max_rounds is given
    rounds: Vec<u8>,
    explored_entry: Option<u8>,
}

fn arb_gen() -> impl Strategy<Value = Gen> {
    (2usize..=7).prop_flat_map(|n| {
        (
            proptest::collection::vec(0u8..4, n - 1), // exploring, developing, testing, review
            proptest::collection::vec(0u8..3, n),
            proptest::collection::vec(proptest::option::weighted(0.07, 0u8..8), n),
            proptest::collection::vec(
                proptest::option::weighted(0.6, (0u8..8, proptest::bool::weighted(0.85))),
                n,
            ),
            proptest::collection::vec(1u8..4, n),
            proptest::option::weighted(0.5, 0u8..8),
        )
            .prop_map(|(mut cats, kinds, next_mut, red, rounds, explored_entry)| {
                cats.sort_unstable();
                Gen {
                    cats,
                    kinds,
                    next_mut,
                    red,
                    rounds,
                    explored_entry,
                }
            })
    })
}

fn render(g: &Gen) -> String {
    let n = g.cats.len() + 1;
    let cat_name = |c: u8| ["exploring", "developing", "testing", "review"][c as usize];
    let mut s = String::from("schema = 1\nstart = \"s0\"\n");
    if let Some(e) = g.explored_entry {
        let later: Vec<usize> = (0..n).filter(|&j| j == n - 1 || g.cats[j] >= 1).collect();
        let pick = if e % 8 == 0 {
            e as usize % n
        } else {
            later[e as usize % later.len()]
        };
        s.push_str(&format!("explored_entry = \"s{pick}\"\n"));
    }
    for i in 0..n {
        let last = i == n - 1;
        let cat = if last { "landing" } else { cat_name(g.cats[i]) };
        // A type that fits the category.
        let kind = if last {
            "landing"
        } else {
            match (g.cats[i], g.kinds[i]) {
                (2, 0) => "test",
                (2, _) => "command",
                (_, 0) => "agent",
                (_, 1) => "command",
                _ => "approval",
            }
        };
        s.push_str(&format!(
            "[step.s{i}]\ntype = \"{kind}\"\ncategory = \"{cat}\"\n"
        ));
        if kind == "command" {
            s.push_str("command = \"true\"\n");
        }
        if !last {
            let target = match g.next_mut[i] {
                None => Some(i + 1),
                Some(c) if c % 4 == 0 => None, // no next at all
                Some(c) if c % 4 == 1 => Some(c as usize % n), // anywhere, maybe backward or itself
                Some(c) if c % 4 == 2 => Some(n + 3), // a step that does not exist
                Some(_) => Some(i + 1),
            };
            match target {
                Some(t) if t < n => s.push_str(&format!("next = \"s{t}\"\n")),
                Some(t) => s.push_str(&format!("next = \"s{t}\"\n")), // dangling
                None => {}
            }
            if let Some((pick, with_rounds)) = g.red[i] {
                if matches!(g.cats[i], 2 | 3) {
                    // Mostly aim at a developing step; sometimes anywhere, and the
                    // validator decides.
                    let developing: Vec<usize> = (0..n - 1).filter(|&j| g.cats[j] == 1).collect();
                    let target = if developing.is_empty() || pick % 10 == 0 {
                        pick as usize % n
                    } else {
                        developing[pick as usize % developing.len()]
                    };
                    s.push_str(&format!("on_red = \"s{target}\"\n"));
                    if with_rounds {
                        s.push_str(&format!("max_rounds = {}\n", g.rounds[i]));
                    }
                }
            }
        }
    }
    s
}

fn outcomes(choices: &[u8]) -> impl FnMut(&mason_definition::Step, u32) -> Outcome + '_ {
    let mut i = 0;
    move |_, _| {
        let c = choices[i % choices.len()];
        i += 1;
        match c % 20 {
            0 => Outcome::Fail,
            1..=10 => Outcome::Red,
            _ => Outcome::Green,
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, max_global_rejects: 50_000, .. ProptestConfig::default() })]

    /// Soundness: valid implies every walk terminates, through the real reducer.
    #[test]
    fn a_valid_definition_always_ends_in_landed_or_failed(g in arb_gen(), choices in proptest::collection::vec(any::<u8>(), 1..200)) {
        let text = render(&g);
        let def = parse(&text).expect("the generator writes well-formed TOML");
        prop_assume!(validate(&def, &Capabilities::now()).is_empty());

        let routes: &[Route] = if def.explored_entry.is_some() { &[Route::Explore, Route::Flag] } else { &[Route::Explore] };
        for &route in routes {
            let t = walk(&def, route, &mut outcomes(&choices));
            let t = t.unwrap_or_else(|e| panic!("a valid definition misbehaved: {e}\n{text}"));
            prop_assert!(matches!(t.end, End::Landed | End::Failed(_)));
            // The bound the validator promises: no step runs more than its rounds allow in total.
            let total_rounds: usize = def.steps.iter().filter(|s| s.on_red.is_some()).map(|s| s.max_rounds.unwrap_or(1) as usize).sum();
            prop_assert!(t.steps.len() <= (def.steps.len() + 1) * (total_rounds + 2), "{} steps for {} steps and {} rounds", t.steps.len(), def.steps.len(), total_rounds);
            // The very first step is the entry for the route.
            let first = def.entry(route.skipped_exploring()).unwrap();
            prop_assert_eq!(&t.steps[0].step, &first.name);
        }
    }

    /// Validating anything never panics, and a rejected definition says why.
    #[test]
    fn validation_never_panics_and_always_explains(g in arb_gen()) {
        let text = render(&g);
        let def = parse(&text).unwrap();
        for caps in [Capabilities::now(), Capabilities::everything()] {
            for p in validate(&def, &caps) {
                prop_assert!(!p.message.is_empty());
            }
        }
    }
}

/// The generator must mostly write good graphs, or the soundness property is
/// mostly vacuous. Measured over a fixed sample, not left to chance.
#[test]
fn a_healthy_share_of_generated_graphs_are_valid_so_the_property_is_not_hollow() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        Config::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[7u8; 32]),
    );
    let strategy = arb_gen();
    let (mut valid, total) = (0, 2000);
    for _ in 0..total {
        let g = strategy.new_tree(&mut runner).unwrap().current();
        if validate(&parse(&render(&g)).unwrap(), &Capabilities::now()).is_empty() {
            valid += 1;
        }
    }
    let share = valid as f64 / total as f64;
    println!(
        "{valid} of {total} generated definitions were valid ({:.0}%)",
        share * 100.0
    );
    assert!(
        share >= 0.30,
        "only {:.0}% valid: the soundness property would be mostly vacuous",
        share * 100.0
    );
    assert!(
        share <= 0.95,
        "{:.0}% valid: the generator never breaks anything, so rejection is untested",
        share * 100.0
    );
}

#[test]
fn a_known_hard_case_the_validator_must_reject() {
    // red work loops back to a step that loops forward again with no bound on the loop.
    let text = "schema = 1\nstart = \"a\"\n[step.a]\ntype = \"agent\"\ncategory = \"developing\"\nnext = \"t\"\n[step.t]\ntype = \"test\"\ncategory = \"testing\"\non_red = \"a\"\nnext = \"l\"\n[step.l]\ntype = \"landing\"\ncategory = \"landing\"\n";
    let def = parse(text).unwrap();
    let p = validate(&def, &Capabilities::now());
    assert!(p.iter().any(|x| x.path == "step.t.max_rounds"), "{p:?}");
    let _ = State::Landed;
}
