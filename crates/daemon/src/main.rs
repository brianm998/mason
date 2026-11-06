// SPDX-License-Identifier: GPL-3.0-or-later

//! `masond`: the MASON daemon.
//!
//! `masond [--data-dir D] [--config-dir C] [--max-concurrent N] [--sweep-ms N]
//!         [--port P] [--host-bin B] [--mcp-bin B] [--reload-interval-ms N] [--fetch-interval-ms N]
//!         [--demo | --demo-manual] [--demo-step-ms N] [--debug]`

use mason_daemon::config::DaemonConfig;
use mason_daemon::{http, Daemon};
use std::path::PathBuf;
use std::time::Duration;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn usage() -> ! {
    eprintln!("usage: masond [--data-dir D] [--config-dir C] [--max-concurrent N] [--sweep-ms N] [--port P] [--host-bin B] [--mcp-bin B] [--debug]");
    std::process::exit(2);
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let exe = std::env::current_exe().unwrap_or_default();
    let sibling = |n: &str| exe.with_file_name(n);
    let default_data = if cfg!(target_os = "macos") {
        home().join("Library/Application Support/mason")
    } else {
        home().join(".local/share/mason")
    };
    let mut cfg = DaemonConfig::new(
        default_data,
        home().join(".config/mason"),
        sibling("step-host"),
        sibling("mason-mcp"),
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut owners_set = false;
    let (mut demo, mut demo_manual, mut demo_step) = (false, false, Duration::from_millis(1500));
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].clone();
        let mut val = || {
            i += 1;
            args.get(i).cloned().unwrap_or_else(|| usage())
        };
        match flag.as_str() {
            "--data-dir" => cfg.data_dir = val().into(),
            "--config-dir" => cfg.config_dir = val().into(),
            "--max-concurrent" => cfg.max_concurrent = val().parse().unwrap_or_else(|_| usage()),
            "--sweep-ms" => {
                cfg.sweep_interval =
                    Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--port" => cfg.port = val().parse().unwrap_or_else(|_| usage()),
            "--host-bin" => cfg.host_bin = val().into(),
            "--mcp-bin" => cfg.mcp_bin = val().into(),
            "--heartbeat-ms" => {
                cfg.heartbeat = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--hold-retry-s" => {
                cfg.hold_retry = Duration::from_secs(val().parse().unwrap_or_else(|_| usage()))
            }
            "--reload-ms" | "--reload-interval-ms" => {
                cfg.reload_interval =
                    Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--fetch-interval-ms" => {
                cfg.fetch_interval =
                    Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--env-retry-ms" => {
                cfg.env_retry = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--owner" => {
                // The first --owner replaces the default identity; more add to it.
                if !owners_set {
                    cfg.owner_names.clear();
                    owners_set = true;
                }
                cfg.owner_names.push(val());
            }
            "--demo" => demo = true,
            "--demo-manual" => {
                demo = true;
                demo_manual = true;
            }
            "--demo-step-ms" => {
                demo_step = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--debug" => cfg.debug = true,
            _ => usage(),
        }
        i += 1;
    }
    // One event loop, as the design says (Part 2): it only dispatches. Anything
    // that blocks belongs on a pool, and the watchdog proves nothing blocks it.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    if demo {
        // A demo keeps nothing: its own scratch data directory, no projects.
        if !args.iter().any(|a| a == "--data-dir") {
            cfg.data_dir = std::env::temp_dir().join(format!("mason-demo-{}", std::process::id()));
        }
        cfg.max_concurrent = 0;
    }
    if let Err(e) = http::ensure_single(&cfg) {
        eprintln!("masond: {e}");
        std::process::exit(1);
    }
    let result = rt.block_on(async {
        // Starting the daemon touches git and the store: keep it off the loop.
        let d = tokio::task::spawn_blocking(move || {
            if demo {
                Daemon::start_demo(cfg)
            } else {
                Daemon::start(cfg)
            }
        })
        .await
        .map_err(|e| e.to_string())??;
        if demo && !demo_manual {
            d.spawn_demo_player(demo_step);
        }
        http::serve(d).await.map_err(|e| e.to_string())
    });
    if let Err(e) = result {
        eprintln!("masond: {e}");
        std::process::exit(1);
    }
}
