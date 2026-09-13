//! End-to-end smoke test against the live PipeWire daemon.
//!
//! Creates a source, sets its faders, checks the nodes and their volumes with
//! `pw-dump`, removes the source, shuts the engine down and checks that no
//! Pipedeck node is left behind. Run with `cargo run -p pipedeck-engine
//! --example smoke` (needs a running PipeWire).

use std::process::{Command as Process, ExitCode};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pipedeck_engine::{spawn, Command, Event, MixBus, SourceId};

fn pw_dump() -> String {
    let out = Process::new("pw-dump")
        .output()
        .expect("pw-dump must be installed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Names of nodes whose node.name starts with `pipedeck.`.
fn pipedeck_nodes(dump: &str) -> Vec<String> {
    let mut names: Vec<String> = dump
        .lines()
        .filter_map(|l| l.trim().strip_prefix("\"node.name\": \"pipedeck."))
        .map(|rest| {
            format!(
                "pipedeck.{}",
                rest.trim_end_matches("\",").trim_end_matches('"')
            )
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Very small extractor: the `channelVolumes` and `mute` lines of the
/// `Props` param of the object whose node.name is `name`.
fn node_volume(dump: &str, name: &str) -> Option<(Vec<f32>, bool)> {
    let needle = format!("\"node.name\": \"{name}\"");
    // pw-dump prints one top-level object per `  {` ... `  }` block.
    for block in dump.split("\n  },\n  {") {
        if !block.contains(&needle) {
            continue;
        }
        let vol_line = block.lines().find(|l| l.contains("\"channelVolumes\":"))?;
        let mute_line = block.lines().find(|l| l.contains("\"mute\":"))?;
        let vols = vol_line
            .split(['[', ']'])
            .nth(1)?
            .split(',')
            .filter_map(|v| v.trim().parse::<f32>().ok())
            .collect();
        let muted = mute_line.contains("true");
        return Some((vols, muted));
    }
    None
}

fn wait_for<F: Fn(&Event) -> bool>(rx: &mpsc::Receiver<Event>, what: &str, pred: F) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(ev) => {
                println!("event: {ev:?}");
                if pred(&ev) {
                    return ev;
                }
                if matches!(ev, Event::Stopped) {
                    panic!("engine stopped while waiting for {what}");
                }
            }
            Err(_) => panic!("timeout waiting for {what}"),
        }
    }
}

fn check(ok: bool, what: &str, failures: &mut u32) {
    println!("[{}] {what}", if ok { " ok " } else { "FAIL" });
    if !ok {
        *failures += 1;
    }
}

fn main() -> ExitCode {
    env_logger::init();
    let mut failures = 0;

    let dir = std::env::temp_dir().join(format!("pipedeck-smoke-{}", std::process::id()));
    let config_path = dir.join("config.toml");

    let (tx, rx) = mpsc::channel();
    let engine = spawn(config_path.clone(), move |ev| {
        let _ = tx.send(ev);
    });

    wait_for(&rx, "Ready", |e| matches!(e, Event::Ready { .. }));
    std::thread::sleep(Duration::from_millis(500));
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        nodes == ["pipedeck.stream_mix"],
        &format!("only the stream mix exists at start: {nodes:?}"),
        &mut failures,
    );

    engine
        .send(Command::AddSource {
            name: "Smoke".into(),
        })
        .unwrap();
    let id = match wait_for(&rx, "SourceAdded", |e| matches!(e, Event::SourceAdded(_))) {
        Event::SourceAdded(cfg) => cfg.id,
        _ => unreachable!(),
    };
    engine
        .send(Command::SetGain {
            id,
            bus: MixBus::Stream,
            gain: 0.5,
        })
        .unwrap();
    engine
        .send(Command::SetMute {
            id,
            bus: MixBus::Monitor,
            muted: true,
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));

    let dump = pw_dump();
    let nodes = pipedeck_nodes(&dump);
    let expected: Vec<String> = [
        format!("pipedeck.{id}"),
        format!("pipedeck.{id}.monitor"),
        format!("pipedeck.{id}.monitor.in"),
        format!("pipedeck.{id}.stream"),
        format!("pipedeck.{id}.stream.in"),
        "pipedeck.stream_mix".to_string(),
    ]
    .into();
    check(
        nodes == expected,
        &format!("source nodes exist: {nodes:?}"),
        &mut failures,
    );

    let stream = node_volume(&dump, &format!("pipedeck.{id}.stream"));
    check(
        stream
            .as_ref()
            .is_some_and(|(v, m)| v.len() == 2 && v.iter().all(|x| (x - 0.125).abs() < 1e-3) && !m),
        &format!("stream chain volume is 0.125 (0.5 cubic), unmuted: {stream:?}"),
        &mut failures,
    );
    let monitor = node_volume(&dump, &format!("pipedeck.{id}.monitor"));
    check(
        monitor
            .as_ref()
            .is_some_and(|(v, m)| v.iter().all(|x| (x - 1.0).abs() < 1e-3) && *m),
        &format!("monitor chain volume is 1.0, muted: {monitor:?}"),
        &mut failures,
    );

    let cfg = std::fs::read_to_string(&config_path).unwrap_or_default();
    check(
        cfg.contains("name = \"Smoke\"") && cfg.contains("gain = 0.5"),
        "config was saved",
        &mut failures,
    );

    engine.send(Command::RemoveSource(id)).unwrap();
    wait_for(&rx, "SourceRemoved", |e| {
        matches!(e, Event::SourceRemoved(_))
    });
    std::thread::sleep(Duration::from_millis(700));
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        nodes == ["pipedeck.stream_mix"],
        &format!("source nodes gone after removal: {nodes:?}"),
        &mut failures,
    );

    engine.shutdown();
    wait_for(&rx, "Stopped", |e| matches!(e, Event::Stopped));
    std::thread::sleep(Duration::from_millis(700));
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        nodes.is_empty(),
        &format!("no pipedeck node left after shutdown: {nodes:?}"),
        &mut failures,
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = SourceId(0);
    if failures == 0 {
        println!("SMOKE PASS");
        ExitCode::SUCCESS
    } else {
        println!("SMOKE FAIL ({failures})");
        ExitCode::FAILURE
    }
}
