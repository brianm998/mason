// SPDX-License-Identifier: GPL-3.0-or-later

//! `step-host --dir DIR --id ID [--linger-ms N] [--hb-ms N] [--cwd D] [--env K=V]... -- CMD ARGS...`

use mason_step_host::host::{run, HostConfig};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg = HostConfig {
        dir: Default::default(),
        id: String::new(),
        cmd: vec![],
        cwd: None,
        env: vec![],
        linger: Duration::from_secs(600),
        heartbeat: Duration::from_secs(1),
    };
    let mut i = 0;
    while i < args.len() {
        let next = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_else(|| usage())
        };
        match args[i].as_str() {
            "--dir" => cfg.dir = next(&mut i).into(),
            "--id" => cfg.id = next(&mut i),
            "--cwd" => cfg.cwd = Some(next(&mut i).into()),
            "--linger-ms" => {
                cfg.linger = Duration::from_millis(next(&mut i).parse().unwrap_or_else(|_| usage()))
            }
            "--hb-ms" => {
                cfg.heartbeat =
                    Duration::from_millis(next(&mut i).parse().unwrap_or_else(|_| usage()))
            }
            "--env" => {
                let kv = next(&mut i);
                let (k, v) = kv.split_once('=').unwrap_or_else(|| usage());
                cfg.env.push((k.into(), v.into()));
            }
            "--" => {
                cfg.cmd = args[i + 1..].to_vec();
                break;
            }
            _ => usage(),
        }
        i += 1;
    }
    if cfg.id.is_empty() || cfg.dir.as_os_str().is_empty() || cfg.cmd.is_empty() {
        usage();
    }
    if let Err(e) = run(cfg) {
        eprintln!("step-host: {e}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!("usage: step-host --dir DIR --id ID [--linger-ms N] [--hb-ms N] [--cwd D] [--env K=V]... -- CMD ARGS...");
    std::process::exit(2);
}
