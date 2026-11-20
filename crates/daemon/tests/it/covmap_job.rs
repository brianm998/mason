// SPDX-License-Identifier: GPL-3.0-or-later

//! #656: the coverage job attributes a spawned binary's source files to the module that spawned
//! it, and selection then runs only that module. A scratch workspace with a two-module
//! integration target; each module runs the same tiny binary with a different argument.
//! Needs the `llvm-tools` rustup component; without it the test says so and returns (the job
//! itself reports `NoLlvmTools` then, which is checked here).

use mason_daemon::affected::{render, select_with, Pick, Pkg, TestTarget};
use mason_daemon::covmap::{llvm_tools, run_job, Job, JobError};
use std::path::Path;

fn put(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn scratch(root: &Path) {
    put(
        root,
        "Cargo.toml",
        "[workspace]\nresolver = \"2\"\nmembers = [\"crates/scratch\"]\n",
    );
    put(
        root,
        "crates/scratch/Cargo.toml",
        "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"tiny\"\npath = \"src/main.rs\"\n",
    );
    put(
        root,
        "crates/scratch/src/main.rs",
        "mod worker_a;\nmod worker_b;\nfn main() {\n    match std::env::args().nth(1).as_deref() {\n        Some(\"a\") => worker_a::run(),\n        _ => worker_b::run(),\n    }\n}\n",
    );
    put(
        root,
        "crates/scratch/src/worker_a.rs",
        "pub fn run() { println!(\"a\"); }\n",
    );
    put(
        root,
        "crates/scratch/src/worker_b.rs",
        "pub fn run() { println!(\"b\"); }\n",
    );
    put(
        root,
        "crates/scratch/tests/it/main.rs",
        "mod mod_a;\nmod mod_b;\n",
    );
    for (m, arg) in [("mod_a", "a"), ("mod_b", "b")] {
        put(
            root,
            &format!("crates/scratch/tests/it/{m}.rs"),
            &format!(
                "#[test]\nfn runs() {{\n    let s = std::process::Command::new(env!(\"CARGO_BIN_EXE_tiny\")).arg(\"{arg}\").status().unwrap();\n    assert!(s.success());\n}}\n"
            ),
        );
    }
}

#[test]
fn the_job_attributes_a_spawned_binarys_files_to_the_module_that_spawned_it() {
    let d = tempfile::tempdir().unwrap();
    let wt = d.path().join("wt");
    scratch(&wt);
    let job = Job {
        wt: &wt,
        pkg_dir: "crates/scratch",
        package: "scratch",
        test: "it",
        main_rs: "crates/scratch/tests/it/main.rs",
        commit: "abcdef0123",
        target_dir: &d.path().join("target"),
        work_dir: &d.path().join("prof"),
        now_secs: 1,
        go_on: &|| true,
    };
    if llvm_tools().is_none() {
        assert_eq!(run_job(&job).unwrap_err(), JobError::NoLlvmTools);
        eprintln!("llvm-tools is not installed: the attribution is not checked here");
        return;
    }
    let map = run_job(&job).unwrap();
    let a = &map.modules["mod_a"];
    let b = &map.modules["mod_b"];
    assert!(a.contains("crates/scratch/src/worker_a.rs"), "{a:?}");
    assert!(!a.contains("crates/scratch/src/worker_b.rs"), "{a:?}");
    assert!(b.contains("crates/scratch/src/worker_b.rs"), "{b:?}");
    assert!(!b.contains("crates/scratch/src/worker_a.rs"), "{b:?}");

    // Selection: a change to worker_a.rs runs mod_a only, though mod_b spawns the same binary.
    let pkgs = vec![Pkg {
        name: "scratch".into(),
        dir: "crates/scratch".into(),
        bins: vec!["tiny".into()],
        tests: vec![TestTarget {
            name: "it".into(),
            src: "crates/scratch/tests/it/main.rs".into(),
        }],
        ..Default::default()
    }];
    let files: Vec<String> = [
        "crates/scratch/src/main.rs",
        "crates/scratch/src/worker_a.rs",
        "crates/scratch/src/worker_b.rs",
        "crates/scratch/tests/it/main.rs",
        "crates/scratch/tests/it/mod_a.rs",
        "crates/scratch/tests/it/mod_b.rs",
    ]
    .map(String::from)
    .to_vec();
    let read = |p: &str| std::fs::read_to_string(wt.join(p)).ok();
    let changed = vec!["crates/scratch/src/worker_a.rs".to_string()];
    let Pick::Some(g) = select_with(&pkgs, &changed, &files, &read, "cargo test", Some(&map))
    else {
        panic!("everything selected")
    };
    assert_eq!(
        render(&g),
        vec!["-p scratch --lib --bins --test it -- mod_a::"]
    );
}
