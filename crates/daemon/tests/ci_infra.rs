// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 153: a red CI run is rerun only when its failed jobs are infrastructure. The fake
//! CI says so with an `infra:` line after its failed jobs. No timing is asserted: only how
//! many times CI was triggered and where the task ended up.

mod support;
use std::path::{Path, PathBuf};
use support::lw::*;
use support::*;

fn triggers(log: &Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |t| t.lines().count())
}

/// A fake CI whose first run is lost by the runner when `flaky`, and whose status names
/// a run whose output says so as infrastructure.
fn world(flaky: bool) -> (W, PathBuf) {
    let log = std::sync::Mutex::new(PathBuf::new());
    let toml = "mode = \"direct-ci\"\nwindow = \"30s\"\nmax_size = 2\nmax_fold = 4\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n";
    let w = world_with(
        toml,
        &|s| {
            let (trigger, status, l) = ci_scripts(s.root.path(), &s.origin);
            let st = s.root.path().join("ci-state");
            let lost = format!(
                "d='{st}/'$MASON_SHA; rm -rf $d; mkdir -p $d; echo \"$MASON_REF $MASON_SHA\" >> '{l}'; \
                 echo 'The runner has lost communication with the server' > $d/out; echo 1 > $d/rc; echo $MASON_SHA",
                st = st.display(),
                l = l.display()
            );
            let trigger = if flaky {
                format!(
                    "if [ -f '{l}' ]; then {trigger}; else {lost}; fi",
                    l = l.display()
                )
            } else {
                trigger
            };
            let status = status.replace(
                "echo integration;",
                "echo integration; if grep -q 'lost communication' $d/out; then echo 'infra: integration'; else echo 'infra:'; fi;",
            );
            *log.lock().unwrap() = l;
            format!(
                "[ci]\nkind = \"command\"\ntrigger = \"{}\"\nstatus = \"{}\"\npoll_s = 1\ntimeout_s = 120\n",
                q(&trigger),
                q(&status)
            )
        },
        &[],
    );
    let l = log.lock().unwrap().clone();
    (w, l)
}

#[test]
fn a_runner_lost_red_is_rerun_once_and_lands_green() {
    let (w, log) = world(true);
    let t = w.task("fine", &[("x/ok.txt", "1")]);
    wait_for("it to land after the rerun", 120, || w.landed(t));
    assert_eq!(triggers(&log), 2, "one run lost, one rerun");
    assert!(w.on_main("x/ok.txt"));
}

#[test]
fn a_real_red_is_sent_back_without_a_second_run() {
    let (w, log) = world(false);
    let t = w.task("breaks CI", &[("red.integration", "x")]);
    wait_for("it to be sent back", 90, || w.sent_back(t).is_some());
    let why = w.sent_back(t).unwrap();
    assert!(why.contains("real failure"), "{why}");
    assert_eq!(triggers(&log), 1, "no rerun of a real failure");
    assert!(!w.on_main("red.integration"));
}
