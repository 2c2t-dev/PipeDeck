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

/// The PipeWire graph, as objects.
fn pw_dump() -> Vec<serde_json::Value> {
    let out = Process::new("pw-dump")
        .output()
        .expect("pw-dump must be installed");
    serde_json::from_slice(&out.stdout).expect("pw-dump returns JSON")
}

fn props(object: &serde_json::Value) -> &serde_json::Value {
    &object["info"]["props"]
}

/// Only the nodes this process owns.
///
/// Another Pipedeck may well be running on the same graph, and it uses the
/// same node names, since they are derived from ids that are private to each
/// mixer. Matching on the client's pid is what keeps this test honest.
fn our_nodes(dump: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    let pid = std::process::id() as i64;
    let clients: Vec<i64> = dump
        .iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Client")))
        .filter(|o| props(o)["application.process.id"].as_i64() == Some(pid))
        .filter_map(|o| o["id"].as_i64())
        .collect();
    dump.iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Node")))
        .filter(|o| {
            props(o)["client.id"]
                .as_i64()
                .is_some_and(|id| clients.contains(&id))
        })
        .collect()
}

/// Names of the nodes this process owns, sorted.
fn node_names(dump: &[serde_json::Value]) -> Vec<String> {
    let mut names: Vec<String> = our_nodes(dump)
        .iter()
        .filter_map(|n| props(n)["node.name"].as_str())
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The `channelVolumes` and `mute` of one of our nodes.
fn node_volume(dump: &[serde_json::Value], name: &str) -> Option<(Vec<f32>, bool)> {
    let node = our_nodes(dump)
        .into_iter()
        .find(|n| props(n)["node.name"].as_str() == Some(name))?;
    let props_param = node["info"]["params"]["Props"].as_array()?.first()?;
    let volumes = props_param["channelVolumes"]
        .as_array()?
        .iter()
        .filter_map(|v| v.as_f64().map(|v| v as f32))
        .collect();
    Some((volumes, props_param["mute"].as_bool().unwrap_or(false)))
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

/// A few seconds of silence, so the test can own a playback stream without
/// making a sound on the machine it runs on.
fn silent_wav(path: &std::path::Path) {
    const RATE: u32 = 48_000;
    const SECONDS: u32 = 30;
    let data = RATE * SECONDS * 4;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    wav.resize(44 + data as usize, 0);
    std::fs::write(path, wav).expect("cannot write the silent file");
}

/// Ids of the nodes on both ends of every link on the graph.
fn links(dump: &[serde_json::Value]) -> Vec<(i64, i64)> {
    dump.iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Link")))
        .filter_map(|o| {
            Some((
                o["info"]["output-node-id"].as_i64()?,
                o["info"]["input-node-id"].as_i64()?,
            ))
        })
        .collect()
}

/// The id of a node by `node.name`, whoever owns it.
fn node_id(dump: &[serde_json::Value], name: &str) -> Option<i64> {
    dump.iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Node")))
        .find(|o| props(o)["node.name"].as_str() == Some(name))
        .and_then(|o| o["id"].as_i64())
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
    let nodes = node_names(&pw_dump());
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
    let nodes = node_names(&pw_dump());
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
        node_names(&dump).contains(&link_node),
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
    let nodes = node_names(&pw_dump());
    check(
        nodes.contains(&format!("pipedeck.out.{mix}.0")),
        &format!("the mix output node exists: {nodes:?}"),
        &mut failures,
    );

    // The master level lands on the mix sink, the output level on the
    // loopback that feeds the device.
    engine
        .send(Command::SetMixGain { id: mix, gain: 0.5 })
        .unwrap();
    engine
        .send(Command::SetOutputGain {
            id: mix,
            index: 0,
            gain: 0.8,
        })
        .unwrap();
    engine
        .send(Command::SetOutputMute {
            id: mix,
            index: 0,
            muted: true,
        })
        .unwrap();
    settle();
    let dump = pw_dump();
    let master = node_volume(&dump, &format!("pipedeck.mix.{mix}"));
    check(
        master
            .as_ref()
            .is_some_and(|(v, _)| v.iter().all(|x| (x - 0.125).abs() < 1e-3)),
        &format!("the mix sink carries the master level: {master:?}"),
        &mut failures,
    );
    let output = node_volume(&dump, &format!("pipedeck.out.{mix}.0"));
    check(
        output
            .as_ref()
            .is_some_and(|(v, m)| v.iter().all(|x| (x - 0.512).abs() < 1e-3) && *m),
        &format!("the output carries its own level and mute: {output:?}"),
        &mut failures,
    );

    // A channel trim rides on its own sink, ahead of every cell.
    engine
        .send(Command::SetSourceGain {
            id: source,
            gain: 0.5,
        })
        .unwrap();
    settle();
    let trim = node_volume(&pw_dump(), &format!("pipedeck.src.{source}"));
    check(
        trim.as_ref()
            .is_some_and(|(v, _)| v.iter().all(|x| (x - 0.125).abs() < 1e-3)),
        &format!("the channel sink carries its trim: {trim:?}"),
        &mut failures,
    );

    // An application assigned to a channel plays into it, and goes back to
    // the session manager when released. The stream is silence, so nothing
    // is heard on the machine running the test.
    let silence = dir.join("silence.wav");
    silent_wav(&silence);
    let mut player = Process::new("pw-play")
        .arg(&silence)
        .spawn()
        .expect("pw-play must be installed");
    let app = match wait_for(
        &rx,
        "an application",
        |e| matches!(e, Event::Apps { running } if running.iter().any(|a| a.name.contains("pw-play") || a.key.contains("pw-play"))),
    ) {
        Event::Apps { running } => running
            .into_iter()
            .find(|a| a.name.contains("pw-play") || a.key.contains("pw-play"))
            .expect("the player was just seen"),
        _ => unreachable!(),
    };
    engine
        .send(Command::AssignApp {
            id: source,
            app: app.key.clone(),
        })
        .unwrap();
    settle();
    let dump = pw_dump();
    let player_node = node_id(&dump, "pw-play");
    let sink_node = node_id(&dump, &format!("pipedeck.src.{source}"));
    let plugged = match (player_node, sink_node) {
        (Some(player), Some(sink)) => links(&dump).contains(&(player, sink)),
        _ => false,
    };
    check(
        plugged,
        &format!("the assigned application plays into the channel: {app:?}"),
        &mut failures,
    );

    engine
        .send(Command::ReleaseApp {
            id: source,
            app: app.key.clone(),
        })
        .unwrap();
    settle();
    let dump = pw_dump();
    let released = match (node_id(&dump, "pw-play"), sink_node) {
        (Some(player), Some(sink)) => !links(&dump).contains(&(player, sink)),
        _ => true,
    };
    check(
        released,
        "releasing the application takes it out of the channel",
        &mut failures,
    );
    let _ = player.kill();
    let _ = player.wait();

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
    let nodes = node_names(&pw_dump());
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
    let nodes = node_names(&pw_dump());
    check(
        nodes.is_empty(),
        &format!("removing the mix takes its output with it: {nodes:?}"),
        &mut failures,
    );

    engine.shutdown();
    wait_for(&rx, "Stopped", |e| matches!(e, Event::Stopped));
    settle();
    let nodes = node_names(&pw_dump());
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
