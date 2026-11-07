// SPDX-License-Identifier: GPL-3.0-or-later

//! `conformance`: parity tooling for the monitors.
//!
//! ```text
//! conformance features [FILE]                        check clients/features.toml
//! conformance tokens --write|--check [--root DIR]    generate / verify the design tokens
//! conformance expected --port P --out DIR [--actor NAME] [--now]
//!                                                      play a manual demo daemon, write each checkpoint
//!                                                      (--now: only the present state, for a real daemon)
//! conformance plugin-kit DIR [--timeout SECS]        run the plugin conformance kit on an extension folder
//! conformance check --expected DIR --actual DIR      compare a client's dumps with the expected ones
//! conformance resume --port P                        connect, disconnect, resume: only what was missed arrives
//! conformance latency --port P [--limit-ms N]        how long a state change takes to reach a client
//! ```

use mason_conformance::{conformance, features, tokens};
use mason_protocol::client::Client;
use mason_protocol::*;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Duration;

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    exit(2)
}

fn opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

fn flag(args: &mut Vec<String>, name: &str) -> bool {
    match args.iter().position(|a| a == name) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

fn generated(root: &Path) -> Vec<(PathBuf, String)> {
    let text = std::fs::read_to_string(root.join("clients/tokens.toml"))
        .unwrap_or_else(|e| die(&format!("clients/tokens.toml: {e}")));
    vec![
        (
            root.join("clients/swift/Sources/MasonTokens/Tokens.swift"),
            tokens::swift(&text).unwrap_or_else(|e| die(&e)),
        ),
        (
            root.join("clients/compose/core/src/main/kotlin/mason/Tokens.kt"),
            tokens::kotlin(&text).unwrap_or_else(|e| die(&e)),
        ),
    ]
}

/// Who a client acts as when nothing says: the same fallback the daemon's owner list has.
fn default_actor() -> String {
    std::env::var("MASON_AS")
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "owner".into())
}

fn all_views(c: &mut Client) -> Vec<View> {
    c.subscribe(
        &[ViewKind::Header, ViewKind::Sidebar, ViewKind::Home],
        "",
        0,
    )
    .unwrap_or_else(|e| die(&e));
    // The three views for the current state: wait until all have arrived.
    let mut got: Vec<View> = vec![];
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while got.len() < 3 && std::time::Instant::now() < end {
        if let Ok(Some(ServerMessage {
            body: Some(server_message::Body::View(u)),
        })) = c.recv(Duration::from_millis(200))
        {
            if let Some(v) = u.view {
                got.push(v);
            }
        }
    }
    got
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        die("usage: conformance features|tokens|expected|check ...");
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "features" => {
            let file = args
                .first()
                .cloned()
                .unwrap_or_else(|| "clients/features.toml".into());
            let text =
                std::fs::read_to_string(&file).unwrap_or_else(|e| die(&format!("{file}: {e}")));
            let r = features::check(&text).unwrap_or_else(|e| die(&format!("{file}: {e}")));
            println!(
                "{} features; done in swift {}, in compose {}",
                r.features, r.done[0], r.done[1]
            );
            for p in &r.problems {
                eprintln!("  {p}");
            }
            if !r.problems.is_empty() {
                eprintln!("the feature check failed: {} problem(s)", r.problems.len());
                exit(1);
            }
            println!("the clients are in parity");
        }
        "tokens" => {
            let write = flag(&mut args, "--write");
            let check = flag(&mut args, "--check");
            let root = PathBuf::from(opt(&mut args, "--root").unwrap_or_else(|| ".".into()));
            if write == check {
                die("give --write or --check");
            }
            let mut stale = false;
            for (path, text) in generated(&root) {
                if write {
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir).unwrap_or_else(|e| die(&e.to_string()));
                    }
                    std::fs::write(&path, text)
                        .unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
                    println!("wrote {}", path.display());
                } else if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                    eprintln!(
                        "{} is out of date: run `conformance tokens --write`",
                        path.display()
                    );
                    stale = true;
                }
            }
            if stale {
                exit(1);
            }
            if check {
                println!("the design tokens are current in both clients");
            }
        }
        "expected" => {
            let port: u16 = opt(&mut args, "--port")
                .and_then(|p| p.parse().ok())
                .unwrap_or_else(|| die("--port P"));
            let out = PathBuf::from(opt(&mut args, "--out").unwrap_or_else(|| die("--out DIR")));
            std::fs::create_dir_all(&out).unwrap_or_else(|e| die(&e.to_string()));
            let actor = opt(&mut args, "--actor").unwrap_or_else(default_actor);
            let now_only = flag(&mut args, "--now");
            let mut c = Client::connect(port, Encoding::Json).unwrap_or_else(|e| die(&e));
            c.acting_as(&actor);
            let owner = c.hello(0, false).unwrap_or_else(|e| die(&e)).owner;
            // Checkpoint 0 is the state before the script; then one per step.
            let mut k = 0;
            loop {
                let views = all_views(&mut c);
                let items: Vec<serde_json::Value> = conformance::visible(&views, owner)
                    .iter()
                    .map(|i| i.to_json())
                    .collect();
                let path = out.join(format!("checkpoint-{k}.json"));
                std::fs::write(
                    &path,
                    serde_json::to_string_pretty(&items).unwrap_or_default(),
                )
                .unwrap_or_else(|e| die(&e.to_string()));
                println!("checkpoint {k}: {} visible items", items.len());
                if now_only {
                    break;
                }
                let (r, _) = c
                    .command(
                        Command {
                            id: String::new(),
                            body: Some(command::Body::Raw(Raw {
                                method: "POST".into(),
                                path: "/demo/next".into(),
                                json: String::new(),
                            })),
                            actor: String::new(),
                            reason: String::new(),
                        },
                        Duration::from_secs(10),
                    )
                    .unwrap_or_else(|e| die(&e));
                let v: serde_json::Value = serde_json::from_str(&r.json).unwrap_or_default();
                if v["advanced"] != serde_json::json!(true) {
                    break;
                }
                k += 1;
            }
        }
        "plugin-kit" => {
            // The conformance kit for plugins (Stage 19): run it on any extension folder.
            let secs: u64 = opt(&mut args, "--timeout")
                .and_then(|t| t.parse().ok())
                .unwrap_or(10);
            let dir = args
                .first()
                .cloned()
                .unwrap_or_else(|| die("plugin-kit DIR"));
            let checks = mason_plugins::kit::plugin_kit(Path::new(&dir), Duration::from_secs(secs));
            let mut failed = 0;
            for c in &checks {
                println!(
                    "{:<4} {:<20} {}",
                    if c.ok { "ok" } else { "FAIL" },
                    c.name,
                    c.detail
                );
                failed += usize::from(!c.ok);
            }
            if failed > 0 {
                eprintln!("the plugin failed {failed} of {} checks", checks.len());
                exit(1);
            }
            println!("the plugin passes all {} checks", checks.len());
        }
        "editor-expected" => {
            // What the daemon says the pipeline editor shows for a project's draft (a real daemon:
            // the demo has no repositories), for the clients to be compared with.
            let port: u16 = opt(&mut args, "--port")
                .and_then(|p| p.parse().ok())
                .unwrap_or_else(|| die("--port P"));
            let project = opt(&mut args, "--project").unwrap_or_else(|| die("--project NAME"));
            let step = opt(&mut args, "--step").unwrap_or_default();
            let out = PathBuf::from(opt(&mut args, "--out").unwrap_or_else(|| die("--out DIR")));
            std::fs::create_dir_all(&out).unwrap_or_else(|e| die(&e.to_string()));
            let mut c = Client::connect(port, Encoding::Json).unwrap_or_else(|e| die(&e));
            c.hello(0, false).unwrap_or_else(|e| die(&e));
            c.subscribe_editor(&[ViewKind::Editor], &project, 0, &step)
                .unwrap_or_else(|e| die(&e));
            let end = std::time::Instant::now() + Duration::from_secs(10);
            let mut views = vec![];
            while views.is_empty() && std::time::Instant::now() < end {
                if let Ok(Some(ServerMessage {
                    body: Some(server_message::Body::View(u)),
                })) = c.recv(Duration::from_millis(200))
                {
                    if let Some(v) = u.view {
                        views.push(v);
                    }
                }
            }
            let items: Vec<serde_json::Value> = conformance::visible(&views, true)
                .iter()
                .map(|i| i.to_json())
                .collect();
            std::fs::write(
                out.join("checkpoint-0.json"),
                serde_json::to_string_pretty(&items).unwrap_or_default(),
            )
            .unwrap_or_else(|e| die(&e.to_string()));
            println!("the editor shows {} items", items.len());
        }
        "check" => {
            let expected = PathBuf::from(
                opt(&mut args, "--expected").unwrap_or_else(|| die("--expected DIR")),
            );
            let actual =
                PathBuf::from(opt(&mut args, "--actual").unwrap_or_else(|| die("--actual DIR")));
            let load = |p: &Path| -> Vec<conformance::Item> {
                let v: serde_json::Value = serde_json::from_str(
                    &std::fs::read_to_string(p)
                        .unwrap_or_else(|e| die(&format!("{}: {e}", p.display()))),
                )
                .unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
                v.as_array()
                    .map(|a| a.iter().filter_map(conformance::Item::from_json).collect())
                    .unwrap_or_default()
            };
            let mut files: Vec<PathBuf> = std::fs::read_dir(&expected)
                .unwrap_or_else(|e| die(&e.to_string()))
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect();
            files.sort();
            let mut bad = 0;
            for f in &files {
                let name = f.file_name().unwrap_or_default();
                let problems = conformance::compare(&load(f), &load(&actual.join(name)));
                for p in &problems {
                    eprintln!("{}: {p}", name.to_string_lossy());
                }
                bad += problems.len();
            }
            if bad > 0 {
                eprintln!("conformance failed: {bad} difference(s)");
                exit(1);
            }
            println!("conformance passed at {} checkpoints", files.len());
        }
        "resume" => resume(&mut args),
        "latency" => latency(&mut args),
        other => die(&format!("unknown command {other:?}")),
    }
}

fn port_of(args: &mut Vec<String>) -> u16 {
    opt(args, "--port")
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| die("--port P"))
}

fn step(c: &mut Client) -> Vec<ServerMessage> {
    let (r, other) = c
        .command(
            Command {
                id: String::new(),
                body: Some(command::Body::Raw(Raw {
                    method: "POST".into(),
                    path: "/demo/next".into(),
                    json: String::new(),
                })),
                actor: String::new(),
                reason: String::new(),
            },
            Duration::from_secs(10),
        )
        .unwrap_or_else(|e| die(&e));
    if !r.ok {
        die(&format!("the demo would not advance: {}", r.error));
    }
    other
}

fn events(msgs: &[ServerMessage]) -> Vec<EventRecord> {
    msgs.iter()
        .filter_map(|m| match &m.body {
            Some(server_message::Body::Events(e)) => Some(e.events.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn drain(c: &mut Client, quiet: Duration) -> Vec<ServerMessage> {
    let mut got = vec![];
    while let Ok(Some(m)) = c.recv(quiet) {
        got.push(m);
    }
    got
}

/// Connect, see some events, disconnect, miss some more, reconnect with the last
/// sequence number: exactly the missed events arrive, and no others.
fn resume(args: &mut Vec<String>) {
    let port = port_of(args);
    let mut a = Client::connect(port, Encoding::Json).unwrap_or_else(|e| die(&e));
    let w = a.hello(0, false).unwrap_or_else(|e| die(&e));
    println!(
        "first connection: {:?}",
        ResumeKind::try_from(w.resume).unwrap_or_default()
    );
    let mut seen = drain(&mut a, Duration::from_millis(300));
    for _ in 0..2 {
        seen.extend(step(&mut a));
    }
    seen.extend(drain(&mut a, Duration::from_millis(400)));
    let had = events(&seen);
    let last = had.last().map_or(w.seq, |e| e.seq);
    println!(
        "it saw {} event(s), the last is #{last}; disconnecting",
        had.len()
    );
    drop(a);
    // While it is away, three more steps.
    let mut b = Client::connect(port, Encoding::Json).unwrap_or_else(|e| die(&e));
    b.hello(0, true).unwrap_or_else(|e| die(&e));
    for _ in 0..3 {
        step(&mut b);
    }
    b.close();
    let mut c = Client::connect(port, Encoding::Json).unwrap_or_else(|e| die(&e));
    let welcome = c.hello(last, false).unwrap_or_else(|e| die(&e));
    let got = drain(&mut c, Duration::from_millis(500));
    let missed = events(&got);
    let want = welcome.seq - last;
    println!(
        "resumed from #{last}: {:?}, {} event(s) arrived, the first is #{}, the last #{}",
        ResumeKind::try_from(welcome.resume).unwrap_or_default(),
        missed.len(),
        missed.first().map_or(0, |e| e.seq),
        missed.last().map_or(0, |e| e.seq)
    );
    let ok = welcome.resume == ResumeKind::Events as i32
        && missed.len() as u64 == want
        && missed.iter().all(|e| e.seq > last)
        && missed.first().map(|e| e.seq) == Some(last + 1)
        && missed.windows(2).all(|p| p[1].seq == p[0].seq + 1);
    if !ok {
        eprintln!("the client did not receive exactly what it missed ({want} expected)");
        exit(1);
    }
    println!("only what was missed arrived, in order, with nothing it already had");
    // Up to date: nothing is replayed.
    drop(c);
    let mut d = Client::connect(port, Encoding::Json).unwrap_or_else(|e| die(&e));
    let wd = d.hello(welcome.seq, false).unwrap_or_else(|e| die(&e));
    let extra = drain(&mut d, Duration::from_millis(400));
    if wd.resume != ResumeKind::UpToDate as i32 || !extra.is_empty() {
        eprintln!("an up-to-date client was sent something");
        exit(1);
    }
    println!("a client that was up to date was sent nothing");
}

/// Time from the daemon appending an event to the client holding a view that
/// shows it, over the demo script.
fn latency(args: &mut Vec<String>) {
    let port = port_of(args);
    let limit: u128 = opt(args, "--limit-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(250);
    let mut driver = Client::connect(port, Encoding::Json).unwrap_or_else(|e| die(&e));
    driver.hello(0, true).unwrap_or_else(|e| die(&e));
    let mut watcher = Client::connect(port, Encoding::Protobuf).unwrap_or_else(|e| die(&e));
    watcher.hello(0, false).unwrap_or_else(|e| die(&e));
    drain(&mut watcher, Duration::from_millis(300));
    let mut times = vec![];
    loop {
        let t0 = std::time::Instant::now();
        // The driver asks for the step; the watcher, a different client, must see it.
        let sent = std::thread::scope(|s| {
            let h = s.spawn(|| {
                let mut views = 0;
                while views < 3 {
                    match watcher.recv(Duration::from_secs(5)) {
                        Ok(Some(ServerMessage {
                            body: Some(server_message::Body::View(_)),
                        })) => views += 1,
                        Ok(Some(_)) => {}
                        _ => return None,
                    }
                }
                Some(t0.elapsed())
            });
            let r = driver
                .command(
                    Command {
                        id: String::new(),
                        body: Some(command::Body::Raw(Raw {
                            method: "POST".into(),
                            path: "/demo/next".into(),
                            json: String::new(),
                        })),
                        actor: String::new(),
                        reason: String::new(),
                    },
                    Duration::from_secs(10),
                )
                .map(|(r, _)| r)
                .unwrap_or_else(|e| die(&e));
            let advanced = serde_json::from_str::<serde_json::Value>(&r.json).unwrap_or_default()
                ["advanced"]
                == serde_json::json!(true);
            (advanced, h.join().ok().flatten())
        });
        match sent {
            (true, Some(t)) => times.push(t),
            _ => break,
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    if times.is_empty() {
        die("the demo had nothing to play");
    }
    times.sort();
    let median = times[times.len() / 2];
    println!(
        "state change to client over {} steps: median {} ms, worst {} ms (target under {limit} ms)",
        times.len(),
        median.as_millis(),
        times.last().map_or(0, |t| t.as_millis())
    );
    if median.as_millis() >= limit {
        eprintln!("too slow");
        exit(1);
    }
}
