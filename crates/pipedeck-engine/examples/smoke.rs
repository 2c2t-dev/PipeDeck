//! End-to-end smoke test against the live PipeWire daemon.
//!
//! Builds a matrix, checks every node it should create, moves a fader, sends
//! the mix to a real device, then tears everything down and checks that no
//! Pipedeck node is left behind. Run with
//! `cargo run -p pipedeck-engine --example smoke` (needs a running PipeWire).

use std::process::{Command as Process, ExitCode};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pipedeck_engine::{spawn, Command, Device, Event, MixId, SourceId, StateSnapshot};

fn pw_dump() -> String {
    let out = Process::new("pw-dump")
        .output()
        .expect("pw-dump must be installed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Names of the nodes we own.
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

/// The `channelVolumes` and `mute` of the node named `name`.
fn node_volume(dump: &str, name: &str) -> Option<(Vec<f32>, bool)> {
    let needle = format!("\"node.name\": \"{name}\"");
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
        return Some((vols, mute_line.contains("true")));
    }
    None
}

fn wait_for<F: Fn(&Event) -> bool>(rx: &mpsc::Receiver<Event>, what: &str, pred: F) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(ev) => {
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

fn wait_state<F: Fn(&StateSnapshot) -> bool>(
    rx: &mpsc::Receiver<Event>,
    what: &str,
    pred: F,
) -> StateSnapshot {
    match wait_for(rx, what, |e| match e {
        Event::State(s) => pred(s),
        _ => false,
    }) {
        Event::State(s) => s,
        _ => unreachable!(),
    }
}

fn check(ok: bool, what: &str, failures: &mut u32) {
    println!("[{}] {what}", if ok { " ok " } else { "FAIL" });
    if !ok {
        *failures += 1;
    }
}

fn settle() {
    std::thread::sleep(Duration::from_millis(900));
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

    // A fresh config starts with one mix and no source.
    let state = wait_state(&rx, "initial state", |_| true);
    let mix: MixId = state.mixes.first().expect("one mix by default").id;
    // The first Devices event is empty: the registry has not replied yet, and
    // the real list follows on the next housekeeping tick.
    let devices = match wait_for(
        &rx,
        "devices",
        |e| matches!(e, Event::Devices { outputs, .. } if !outputs.is_empty()),
    ) {
        Event::Devices { outputs, .. } => outputs,
        _ => unreachable!(),
    };
    settle();
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        nodes == [format!("pipedeck.mix.{mix}")],
        &format!("a fresh config yields one mix sink and nothing else: {nodes:?}"),
        &mut failures,
    );
    check(
        !devices.is_empty(),
        &format!("output devices were discovered: {}", devices.len()),
        &mut failures,
    );

    // A source on its own creates a sink but no cell.
    engine
        .send(Command::AddSource {
            name: "Smoke".into(),
            device: None,
        })
        .unwrap();
    let state = wait_state(&rx, "source added", |s| !s.sources.is_empty());
    let source: SourceId = state.sources[0].id;
    settle();
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        nodes
            == [
                format!("pipedeck.mix.{mix}"),
                format!("pipedeck.src.{source}"),
            ],
        &format!("the source sink appears, unlinked: {nodes:?}"),
        &mut failures,
    );

    // Linking creates the cell and its fader.
    engine
        .send(Command::SetLink {
            source,
            mix,
            linked: true,
        })
        .unwrap();
    wait_state(&rx, "link created", |s| !s.links.is_empty());
    engine
        .send(Command::SetLinkGain {
            source,
            mix,
            gain: 0.5,
        })
        .unwrap();
    engine
        .send(Command::SetLinkMute {
            source,
            mix,
            muted: true,
        })
        .unwrap();
    settle();
    let dump = pw_dump();
    let link_node = format!("pipedeck.link.{source}.{mix}");
    check(
        pipedeck_nodes(&dump).contains(&link_node),
        &format!("the cell node exists: {link_node}"),
        &mut failures,
    );
    let volume = node_volume(&dump, &link_node);
    check(
        volume
            .as_ref()
            .is_some_and(|(v, m)| v.len() == 2 && v.iter().all(|x| (x - 0.125).abs() < 1e-3) && *m),
        &format!("the cell carries gain 0.125 (0.5 cubic) and mute: {volume:?}"),
        &mut failures,
    );

    // Sending the mix to a real device adds one output loopback.
    let device: Device = devices[0].clone();
    engine
        .send(Command::SetMixOutputs {
            id: mix,
            devices: vec![device.name.clone()],
        })
        .unwrap();
    wait_state(&rx, "outputs set", |s| {
        s.mixes.first().is_some_and(|m| m.outputs.len() == 1)
    });
    settle();
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        nodes.contains(&format!("pipedeck.out.{mix}.0")),
        &format!("the mix output node exists: {nodes:?}"),
        &mut failures,
    );

    let cfg = std::fs::read_to_string(&config_path).unwrap_or_default();
    check(
        cfg.contains("[[link]]") && cfg.contains("gain = 0.5") && cfg.contains(&device.name),
        "the matrix was saved to the config",
        &mut failures,
    );

    // Unlinking removes the cell and nothing else.
    engine
        .send(Command::SetLink {
            source,
            mix,
            linked: false,
        })
        .unwrap();
    wait_state(&rx, "link removed", |s| s.links.is_empty());
    settle();
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        !nodes.iter().any(|n| n.starts_with("pipedeck.link.")),
        &format!("the cell is gone, the rest stays: {nodes:?}"),
        &mut failures,
    );

    engine.send(Command::RemoveSource(source)).unwrap();
    wait_state(&rx, "source removed", |s| s.sources.is_empty());
    engine.send(Command::RemoveMix(mix)).unwrap();
    wait_state(&rx, "mix removed", |s| s.mixes.is_empty());
    settle();
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        nodes.is_empty(),
        &format!("removing the mix takes its output with it: {nodes:?}"),
        &mut failures,
    );

    engine.shutdown();
    wait_for(&rx, "Stopped", |e| matches!(e, Event::Stopped));
    settle();
    let nodes = pipedeck_nodes(&pw_dump());
    check(
        nodes.is_empty(),
        &format!("no pipedeck node left after shutdown: {nodes:?}"),
        &mut failures,
    );

    let _ = std::fs::remove_dir_all(&dir);
    if failures == 0 {
        println!("SMOKE PASS");
        ExitCode::SUCCESS
    } else {
        println!("SMOKE FAIL ({failures})");
        ExitCode::FAILURE
    }
}
