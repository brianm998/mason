// SPDX-License-Identifier: GPL-3.0-or-later

//! `masond`: the MASON daemon.
//!
//! `masond [--data-dir D] [--config-dir C] [--max-concurrent N] [--sweep-ms N]
//!         [--port P (0: any free port)] [--lan] [--lan-port P] [--host-bin B] [--mcp-bin B] [--reload-interval-ms N] [--fetch-interval-ms N]
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
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!(
            "{}",
            mason_daemon::version::line(env!("CARGO_PKG_VERSION"), env!("MASON_COMMIT"))
        );
        return;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    mason_daemon::fdlimit::raise();
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
    // `masond selftest --db COPY`: what a new build proves before it takes over.
    if args.first().map(String::as_str) == Some("selftest") {
        let db = args
            .iter()
            .position(|a| a == "--db")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from);
        let Some(db) = db else { usage() };
        match mason_daemon::selftest::run(&db) {
            Ok(said) => {
                println!("{said}\nself-test passed");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("self-test FAILED: {e}");
                std::process::exit(1);
            }
        }
    }
    let managed = args.iter().any(|a| a == "--managed");
    let mut owners_set = false;
    let (mut demo, mut demo_manual, mut demo_step) = (false, false, Duration::from_millis(1500));
    let mut cli_port: Option<u16> = None;
    let (mut cli_lan, mut cli_lan_port) = (false, None::<u16>);
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
            "--port" => cli_port = Some(val().parse().unwrap_or_else(|_| usage())),
            "--host-bin" => cfg.host_bin = val().into(),
            "--mcp-bin" => cfg.mcp_bin = val().into(),
            "--heartbeat-ms" => {
                cfg.heartbeat = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--watch-tick-ms" => {
                cfg.watch_tick = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
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
            "--attention-ms" => {
                cfg.attention_interval =
                    Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--stuck-after-ms" => {
                cfg.stuck_after = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--fetch-alert-after-ms" => {
                cfg.fetch_alert_after =
                    Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--introspect-ms" => {
                cfg.introspect_interval =
                    Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--analyst-ms" => {
                cfg.analyst_interval =
                    Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--schedule-ms" => {
                cfg.schedule_tick = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--env-retry-ms" => {
                cfg.env_retry = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            "--base-red-poll-ms" => {
                cfg.base_red_poll = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
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
            "--lan" => cli_lan = true,
            "--lan-port" => cli_lan_port = Some(val().parse().unwrap_or_else(|_| usage())),
            "--debug" => cfg.debug = true,
            "--managed" => {}
            "--runtime-sha" => cfg.runtime_sha = Some(val()),
            "--healthy-after-ms" => {
                cfg.healthy_after = Duration::from_millis(val().parse().unwrap_or_else(|_| usage()))
            }
            _ => usage(),
        }
        i += 1;
    }
    // The port: --port, then daemon.toml, then the fixed default (0 for a demo).
    let file_port = match mason_daemon::config::load_daemon_port(&cfg.config_dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("masond: {}: {e}", cfg.config_dir.display());
            std::process::exit(1);
        }
    };
    match mason_daemon::config::load_retention_days(&cfg.config_dir) {
        Ok(Some(n)) => cfg.retention_days = n,
        Ok(None) => {}
        Err(e) => {
            eprintln!("masond: {}: {e}", cfg.config_dir.display());
            std::process::exit(1);
        }
    }
    cfg.port = mason_daemon::config::resolve_port(cli_port, file_port, demo);
    // LAN access: daemon.toml `[lan]`, or --lan. Off by default.
    cfg.lan = match mason_daemon::config::load_lan(&cfg.config_dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("masond: {}: {e}", cfg.config_dir.display());
            std::process::exit(1);
        }
    };
    cfg.lan.enabled |= cli_lan;
    if let Some(p) = cli_lan_port {
        cfg.lan.port = p;
    }
    // What to start again with when a new runtime takes over.
    {
        let mut keep = vec![];
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--managed" => {}
                "--runtime-sha" => {
                    it.next();
                }
                _ => keep.push(a.clone()),
            }
        }
        // Explicit paths, so the new runtime finds the same data and configuration.
        if !keep.iter().any(|a| a == "--data-dir") {
            keep.extend(["--data-dir".into(), cfg.data_dir.display().to_string()]);
        }
        if !keep.iter().any(|a| a == "--config-dir") {
            keep.extend(["--config-dir".into(), cfg.config_dir.display().to_string()]);
        }
        cfg.exec_args = keep;
    }
    // Run from `runtimes/<sha>/`: the loader picks the runtime, rolling back by itself
    // after three crashes in five minutes. Unix replaces itself with it (exec); Windows
    // has no exec, so the loader stays and supervises the daemon as a child.
    if managed && cfg.runtime_sha.is_none() {
        #[cfg(unix)]
        {
            let _ = managed_once(&cfg);
        }
        #[cfg(windows)]
        supervise(&cfg);
    }
    // A successor started by a takeover on Windows: the old daemon still holds the port
    // until it has gone. Wait for that (a condition, bounded), then carry on as usual.
    if let Some(pid) = mason_platform::process::predecessor() {
        mason_platform::process::wait_until(Duration::from_secs(60), || {
            !mason_platform::process::is_alive(pid)
        });
    }
    if let Some(sha) = &cfg.runtime_sha {
        use mason_daemon::update::{Boots, Runtimes};
        let boots = Boots::new(&Runtimes::new(&cfg.data_dir));
        let (sha, after) = (sha.clone(), cfg.healthy_after);
        std::thread::spawn(move || {
            std::thread::sleep(after);
            boots.record(mason_daemon::daemon::now_ms(), &sha, "healthy");
        });
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
    // A refusal is not a crash: say so in boots.log before exiting, so the
    // loader does not count the start it recorded as a build that died.
    let refuse = |cfg: &mason_daemon::config::DaemonConfig, e: &dyn std::fmt::Display| -> ! {
        eprintln!("masond: {e}");
        if let Some(sha) = &cfg.runtime_sha {
            use mason_daemon::update::{Boots, Runtimes};
            Boots::new(&Runtimes::new(&cfg.data_dir)).record(
                mason_daemon::daemon::now_ms(),
                sha,
                "refused",
            );
        }
        std::process::exit(1);
    };
    if let Err(e) = http::ensure_single(&cfg) {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            refuse(&cfg, &e);
        }
        eprintln!("masond: {e}");
        std::process::exit(1);
    }
    let cfg_after = cfg.clone();
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
        // A deliberate stop (a restart, ctrl-C) is logged, and the server is
        // shut down gracefully, bounded so launchd's 5 s exit timeout is not hit.
        // Step hosts are separate processes and keep running.
        // Installed once the daemon is up, so a stuck start can still be killed.
        if let Err(e) = mason_daemon::stopsig::install() {
            tracing::warn!("cannot handle SIGTERM: {e}");
        }
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let what = mason_daemon::stopsig::wait().await;
            tracing::info!("masond stopping: {what}");
            let _ = tx.send(());
        });
        let stopped = std::sync::Arc::new(tokio::sync::Notify::new());
        let stopped2 = stopped.clone();
        let serving = http::serve_until(d, async move {
            if rx.await.is_ok() {
                stopped2.notify_one();
            } else {
                std::future::pending::<()>().await;
            }
        });
        tokio::pin!(serving);
        let r = tokio::select! {
            r = &mut serving => r,
            _ = async {
                stopped.notified().await;
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            } => Ok(()),
        };
        Ok::<_, String>(r)
    });
    match result {
        Ok(Ok(())) => {
            // Serve only returns when told to stop: say so in boots.log, so
            // the next start does not count this one as a crash.
            if let Some(sha) = &cfg_after.runtime_sha {
                use mason_daemon::update::{Boots, Runtimes};
                Boots::new(&Runtimes::new(&cfg_after.data_dir)).record(
                    mason_daemon::daemon::now_ms(),
                    sha,
                    "stopped",
                );
            }
            std::process::exit(0);
        }
        // The port is held by another program: refused, not crashed.
        Ok(Err(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::AlreadyExists
            ) =>
        {
            refuse(&cfg_after, &e)
        }
        Ok(Err(e)) => {
            eprintln!("masond: {e}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("masond: {e}");
            std::process::exit(1);
        }
    }
}

/// What one try at starting the chosen runtime came to.
enum Started {
    /// Nothing usable to start (no current runtime, or it would not start): run this build.
    Fallback,
    /// Windows: the child ran and exited with this code (`None`: killed), and had this pid.
    #[cfg(windows)]
    Exited(Option<i32>, u32),
}

/// Pick the runtime (rolling back after three crashes in five minutes) and start it:
/// by `exec` on Unix, which only returns if that fails; as a child on Windows.
fn managed_once(cfg: &DaemonConfig) -> Started {
    use mason_daemon::update::{decide, Boots, Decision, Runtimes};
    let rt = Runtimes::new(&cfg.data_dir);
    let Some(cur) = rt.current().filter(|c| rt.is_ready(c)) else {
        return Started::Fallback;
    };
    let boots = Boots::new(&rt);
    let now = mason_daemon::daemon::now_ms();
    let chosen = match decide(&cur, rt.previous().as_deref(), &boots.all(), now) {
        Decision::Run(s) => s,
        Decision::RollBack { from, to } => {
            rt.mark_bad(&from);
            rt.record_rollback(&from, &to, now);
            let _ = rt.set_current(&to);
            rt.leave_alert(&format!(
                "{} crashed three times in five minutes and was rolled back to {}",
                &from[..from.len().min(12)],
                &to[..to.len().min(12)]
            ));
            eprintln!("masond: {from} crashed three times in five minutes: rolling back to {to}");
            to
        }
        Decision::RunAnyway { sha, crashes } => {
            rt.leave_alert(&format!(
                "{} has crashed {crashes} times and there is no earlier runtime to go back to",
                &sha[..sha.len().min(12)]
            ));
            sha
        }
    };
    if !rt.is_ready(&chosen) {
        return Started::Fallback;
    }
    // Recorded here, not by the daemon: a build that dies at once never would.
    boots.record(now, &chosen, "start");
    let mut a = cfg.exec_args.clone();
    a.extend(["--runtime-sha".into(), chosen.clone()]);
    let mut cmd = std::process::Command::new(rt.masond(&chosen));
    cmd.args(&a);
    #[cfg(unix)]
    {
        let err = mason_platform::process::replace_process(&mut cmd);
        eprintln!("masond: cannot start runtime {chosen}: {err}; running this build instead");
        Started::Fallback
    }
    #[cfg(windows)]
    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            match child.wait() {
                Ok(st) => Started::Exited(st.code(), pid),
                Err(_) => Started::Exited(None, pid),
            }
        }
        Err(err) => {
            eprintln!("masond: cannot start runtime {chosen}: {err}; running this build instead");
            Started::Fallback
        }
    }
}

/// Windows loader: keep one daemon running. A clean exit ends the loader; a handover
/// (the daemon started a successor and left) is followed to the successor's end; a crash
/// starts again, and `managed_once` counts it and rolls back. Returns only to fall back
/// to running this build.
#[cfg(windows)]
fn supervise(cfg: &DaemonConfig) {
    use mason_platform::process::{is_alive, wait_until, HANDOVER_EXIT};
    loop {
        match managed_once(cfg) {
            Started::Fallback => return,
            Started::Exited(Some(0), _) => std::process::exit(0),
            Started::Exited(Some(HANDOVER_EXIT), old) => {
                // The successor announces itself in daemon.json once it holds the port.
                let info = cfg.info_path();
                let mut succ = None;
                let found = wait_until(Duration::from_secs(120), || {
                    succ = std::fs::read_to_string(&info)
                        .ok()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                        .and_then(|v| v["pid"].as_u64())
                        .map(|p| p as u32)
                        .filter(|p| *p != old && is_alive(*p));
                    succ.is_some()
                });
                if let (true, Some(p)) = (found, succ) {
                    wait_until(Duration::from_secs(u64::MAX / 4), || !is_alive(p));
                }
            }
            Started::Exited(..) => {
                // Not a clean stop: a short breath so a build that dies at once does not spin.
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}
