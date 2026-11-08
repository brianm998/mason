// SPDX-License-Identifier: GPL-3.0-or-later

//! A stand-in for the daemon, used by tests and scripts/exit/s04.sh.
//!
//!   step-host-ctl run ROOT ID [--linger-ms N] [--hb-ms N] -- CMD...
//!       start a host and collect its output into ROOT/ID/received
//!   step-host-ctl attach ROOT ID
//!   step-host-ctl reap ROOT
//!       kill the verified hosts under ROOT, and only those
//!       reattach and finish collecting (resumes from the size of `received`)
//!
//! On completion it writes ROOT/ID/exit_status (`code=` and `signal=`).

use mason_step_host::client::collect_to_file;
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 && !(args.len() == 2 && args[0] == "reap") {
        eprintln!("usage: step-host-ctl run ROOT ID [opts] -- CMD... | attach ROOT ID");
        std::process::exit(2);
    }
    if args[0] == "reap" {
        // Kill only the hosts under ROOT that are verifiably the recorded processes.
        for r in mason_step_host::registry::reap_verified(&PathBuf::from(&args[1])) {
            println!(
                "{} host_killed={} child_group_killed={}",
                r.id, r.host_killed, r.child_group_killed
            );
        }
        return;
    }
    let root = PathBuf::from(&args[1]);
    let id = &args[2];
    let dir = root.join(id);
    if args[0] == "run" {
        let host_bin = std::env::var_os("STEP_HOST_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .expect("exe")
                    .with_file_name("step-host")
            });
        let mut o = SpawnOptions::new(host_bin, &root, id, &[]);
        let mut i = 3;
        while i < args.len() {
            match args[i].as_str() {
                "--linger-ms" => {
                    o.linger = Duration::from_millis(args[i + 1].parse().expect("ms"));
                    i += 2;
                }
                "--hb-ms" => {
                    o.heartbeat = Duration::from_millis(args[i + 1].parse().expect("ms"));
                    i += 2;
                }
                "--" => {
                    o.cmd = args[i + 1..].to_vec();
                    break;
                }
                _ => {
                    eprintln!("bad option {}", args[i]);
                    std::process::exit(2);
                }
            }
        }
        if let Err(e) = spawn_host(&o) {
            eprintln!("spawn failed: {e}");
            std::process::exit(1);
        }
    }
    match collect_to_file(&dir, &dir.join("received"), &dir.join("exit_status")) {
        Ok(rec) => println!(
            "EXIT code={} signal={} len={}",
            rec.code, rec.signal, rec.output_len
        ),
        Err(e) => {
            eprintln!("collect failed: {e}");
            std::process::exit(3);
        }
    }
}
