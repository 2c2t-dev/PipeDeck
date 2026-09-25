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
///
/// Meter taps are left out: they measure the graph rather than carry it, and
/// naming them in every expectation would say nothing about the routing.
fn node_names(dump: &[serde_json::Value]) -> Vec<String> {
    let mut names: Vec<String> = our_nodes(dump)
        .iter()
        .filter_map(|n| props(n)["node.name"].as_str())
        .filter(|name| !name.starts_with("pipedeck.meter."))
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

/// Is another Pipedeck holding the names this test measures?
///
/// Node names come from ids private to each mixer, so a second one answers
/// to the same names, and the loopbacks that carry the audio pick their
/// target by name. The routing checks still hold in that case, but what a
/// meter hears stops being ours to predict.
fn another_mixer_running(dump: &[serde_json::Value]) -> bool {
    let ours: Vec<i64> = our_nodes(dump)
        .iter()
        .filter_map(|o| o["id"].as_i64())
        .collect();
    dump.iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Node")))
        .filter(|o| {
            props(o)["node.name"]
                .as_str()
                .is_some_and(|name| name.starts_with("pipedeck."))
        })
        .any(|o| o["id"].as_i64().is_some_and(|id| !ours.contains(&id)))
}

/// One of our own nodes, by `node.name`.
///
/// Another Pipedeck on the same graph answers to the same names, so a name
/// is not enough to say which sink a player should feed or which node a
/// volume change should land on.
fn our_node<'d>(dump: &'d [serde_json::Value], name: &str) -> Option<&'d serde_json::Value> {
    our_nodes(dump)
        .into_iter()
        .find(|o| props(o)["node.name"].as_str() == Some(name))
}

fn node_id(dump: &[serde_json::Value], name: &str) -> Option<i64> {
    our_node(dump, name)?["id"].as_i64()
}

/// The id of a node belonging to anyone, for the ones that are not ours to
/// begin with: the player this test starts is its own process, so looking
/// for it among our own nodes never finds it.
fn any_node_id(dump: &[serde_json::Value], name: &str) -> Option<i64> {
    dump.iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Node")))
        .find(|o| props(o)["node.name"].as_str() == Some(name))?["id"]
        .as_i64()
}

/// The serial a player uses to name a target, which is not the object id.
fn node_serial(dump: &[serde_json::Value], name: &str) -> Option<i64> {
    props(our_node(dump, name)?)["object.serial"].as_i64()
}

/// What a recorder hears on a node, over `seconds`.
///
/// This is a capture client doing what OBS does: naming the device and
/// taking what comes out of it.
fn record_peak(serial: i64, seconds: u32, into: &std::path::Path) -> f32 {
    let _ = std::fs::remove_file(into);
    let recorded = Process::new("pw-record")
        .arg(format!("--target={serial}"))
        .arg("--rate=48000")
        .arg("--channels=2")
        .arg("--format=s16")
        .arg(into)
        .spawn();
    let Ok(mut recorder) = recorded else {
        return 0.0;
    };
    std::thread::sleep(Duration::from_secs(seconds.into()));
    let _ = recorder.kill();
    let _ = recorder.wait();

    let Ok(bytes) = std::fs::read(into) else {
        return 0.0;
    };
    if bytes.len() <= 44 {
        return 0.0;
    }
    bytes[44..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).unsigned_abs() as f32 / 32768.0)
        .fold(0.0f32, f32::max)
}

/// A quiet tone, loud enough to move a meter. Nothing is attached to the
/// mix in this test, so it reaches no device and makes no sound.
fn tone_wav(path: &std::path::Path) {
    const RATE: u32 = 48_000;
    const SECONDS: u32 = 20;
    let frames = RATE * SECONDS;
    let data = frames * 4;
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
    for frame in 0..frames {
        let phase = frame as f32 / RATE as f32 * 440.0 * std::f32::consts::TAU;
        let sample = (phase.sin() * 12_000.0) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, wav).expect("cannot write the tone");
}

/// The loudest level reported for a channel and a mix over a window of time.
fn wait_levels(
    rx: &mpsc::Receiver<Event>,
    source: SourceId,
    mix: MixId,
    window: Duration,
) -> (f32, f32) {
    let deadline = Instant::now() + window;
    let (mut loudest_source, mut loudest_mix) = (0.0f32, 0.0f32);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(Event::Levels { sources, mixes }) => {
                for (id, level) in sources {
                    if id == source {
                        loudest_source = loudest_source.max(level);
                    }
                }
                for (id, level) in mixes {
                    if id == mix {
                        loudest_mix = loudest_mix.max(level);
                    }
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    (loudest_source, loudest_mix)
}

/// The first complaint the engine makes inside `window`, if it makes one.
fn wait_error(rx: &mpsc::Receiver<Event>, window: Duration) -> Option<String> {
    let deadline = Instant::now() + window;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(Event::Error(e)) => return Some(e),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    None
}

/// Throw away what queued while something was settling.
///
/// Levels are sent several times a second whatever anyone is waiting for, so
/// a window opened after a change would otherwise take in the moment before
/// it as well, and report the loudest of the two. What is being measured
/// here is what is true now.
fn drain(rx: &mpsc::Receiver<Event>) {
    while rx.try_recv().is_ok() {}
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
    let (devices, capture_devices) = match wait_for(
        &rx,
        "devices",
        |e| matches!(e, Event::Devices { outputs, .. } if !outputs.is_empty()),
    ) {
        Event::Devices { outputs, inputs } => (outputs, inputs),
        _ => unreachable!(),
    };
    settle();
    let nodes = node_names(&pw_dump());
    check(
        nodes == [format!("pipedeck.mix.{mix}")],
        &format!("a fresh config yields one mix and nothing else: {nodes:?}"),
        &mut failures,
    );
    let offered = our_node(&pw_dump(), &format!("pipedeck.mix.{mix}")).map(|node| {
        (
            props(node)["media.class"].as_str().unwrap_or("").to_owned(),
            props(node)["node.description"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
        )
    });
    check(
        offered.as_ref().is_some_and(|(class, description)| {
            class == "Audio/Source/Virtual" && description.contains("Personal Mix")
        }),
        &format!("and it is a microphone to the system, not a speaker: {offered:?}"),
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
            icon: None,
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

    // A meter follows every channel and every mix, and reads what actually
    // goes through them. Playing a tone into the channel has to move both,
    // and muting the cell has to stop the mix hearing it while the channel
    // still does.
    let tone = dir.join("tone.wav");
    tone_wav(&tone);
    let alone = !another_mixer_running(&pw_dump());
    if !alone {
        println!("[skip] another Pipedeck holds the same node names, so what the meters hear is not this test's to predict");
    }
    engine
        .send(Command::SetLinkMute {
            source,
            mix,
            muted: false,
        })
        .unwrap();
    let sink_serial =
        node_serial(&pw_dump(), &format!("pipedeck.src.{source}")).expect("the channel sink");
    let mut player = Process::new("pw-play")
        .arg(format!("--target={sink_serial}"))
        .arg(&tone)
        .spawn()
        .expect("pw-play must be installed");
    let open = wait_levels(&rx, source, mix, Duration::from_secs(5));
    check(
        open.0 > 0.05,
        &format!("the channel meter hears the tone: {:.3}", open.0),
        &mut failures,
    );

    // The cell sits at 0.5, a linear 0.125, and the mix master is there too
    // by now, which its sink applies to the monitor ports this meter reads.
    // So the mix hears the tone through both. This is the faders themselves
    // under test, not the properties they write.
    let expected = open.0 * 0.125 * 0.125;
    let what = format!(
        "the mix hears the tone through the cell fader: {:.3}, around {expected:.3}",
        open.1
    );
    if alone {
        check(
            open.1 > expected * 0.4 && open.1 < expected * 2.5,
            &what,
            &mut failures,
        );
    } else {
        println!("[skip] {what}");
    }

    // What a capture client gets, taken the way one takes it: by naming the
    // device and recording it. The mix is heard through the cell and the
    // master alike, so this is the same signal its meter reads.
    let recorded = node_serial(&pw_dump(), &format!("pipedeck.mix.{mix}"))
        .map(|serial| record_peak(serial, 3, &dir.join("recorded.wav")))
        .unwrap_or(0.0);
    let what = format!("a recorder hears the mix on its input device: {recorded:.3}");
    if alone {
        check(recorded > expected * 0.4, &what, &mut failures);
    } else {
        println!("[skip] {what}");
    }

    engine
        .send(Command::SetLinkMute {
            source,
            mix,
            muted: true,
        })
        .unwrap();
    settle();
    drain(&rx);
    let muted = wait_levels(&rx, source, mix, Duration::from_secs(3));
    check(
        muted.0 > 0.05,
        &format!(
            "the channel still hears itself when muted in a mix: {:.3}",
            muted.0
        ),
        &mut failures,
    );
    let what = format!(
        "muting the cell stops the mix hearing it: {:.3} against {:.3} open",
        muted.1, open.1
    );
    if alone {
        check(muted.1 < open.1 * 0.2, &what, &mut failures);
    } else {
        println!("[skip] {what}");
    }

    let _ = player.kill();
    let _ = player.wait();
    // The peaks are maxima over a window, so the tail of the tone would
    // still show in one that starts the moment the player dies.
    settle();
    drain(&rx);
    let quiet = wait_levels(&rx, source, mix, Duration::from_secs(2));
    let what = format!("the meters fall back to silence: {:.3}", quiet.0);
    if alone {
        check(quiet.0 < 0.01, &what, &mut failures);
    } else {
        // Another mixer's loopbacks capture and feed nodes by the names we
        // share with it, so silence here is not ours to promise either.
        println!("[skip] {what}");
    }

    // A channel's trim is the volume of its sink, which is also the volume
    // the system shows. Moving it from outside has to reach the mixer.
    let sink_id = node_id(&pw_dump(), &format!("pipedeck.src.{source}")).expect("the channel sink");
    let outside = Process::new("wpctl")
        .args(["set-volume", &sink_id.to_string(), "0.4"])
        .status();
    if outside.is_ok_and(|status| status.success()) {
        let told = wait_for(
            &rx,
            "the trim set from outside",
            |e| matches!(e, Event::SourceChanged { id, state } if *id == source && (state.gain - 0.4).abs() < 0.05),
        );
        check(
            matches!(told, Event::SourceChanged { .. }),
            "a level set outside the mixer comes back to it",
            &mut failures,
        );
    } else {
        println!("[skip] wpctl is not installed, the outside-change check needs it");
    }

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
    let player_node = any_node_id(&dump, "pw-play");
    let sink_node = node_id(&dump, &format!("pipedeck.src.{source}"));
    let plugged = match (player_node, sink_node) {
        (Some(player), Some(sink)) => links(&dump).contains(&(player, sink)),
        _ => false,
    };
    let what = format!("the assigned application plays into the channel: {app:?}");
    if alone {
        check(plugged, &what, &mut failures);
    } else {
        // The move names the target sink, and another mixer answers to that
        // name too, so where the stream lands is not ours to promise.
        println!("[skip] {what}");
    }

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

    // An effect on a channel is a filter chain between its sink and its
    // cells, so the cell has to start reading the chain instead.
    engine
        .send(Command::SetEffects {
            id: source,
            effects: vec![pipedeck_engine::Effect {
                name: "Low cut".into(),
                kind: pipedeck_engine::EffectKind::Builtin,
                plugin: None,
                label: "bq_highpass".into(),
                controls: vec![pipedeck_engine::Control {
                    name: "Freq".into(),
                    value: 90.0,
                }],
            }],
        })
        .unwrap();
    wait_state(&rx, "the effect", |s| {
        s.sources.iter().any(|row| !row.effects.is_empty())
    });
    settle();
    let dump = pw_dump();
    let names = node_names(&dump);
    check(
        names.contains(&format!("pipedeck.fx.{source}")),
        &format!("the effects chain offers a sink of its own: {names:?}"),
        &mut failures,
    );
    let reading = our_node(&dump, &format!("pipedeck.link.{source}.{mix}.in")).map(|node| {
        props(node)["target.object"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    check(
        reading.as_deref() == Some(format!("pipedeck.fx.{source}").as_str()),
        &format!("the cell reads the chain rather than the raw channel: {reading:?}"),
        &mut failures,
    );

    // And the effect has to be audible, not merely present: a low cut well
    // above the tone takes it out, and the channel meter sits at the end of
    // the chain, so it is the meter that says so.
    let mut player = Process::new("pw-play")
        .arg(format!("--target={sink_serial}"))
        .arg(&tone)
        .spawn()
        .expect("pw-play must be installed");
    let through = wait_levels(&rx, source, mix, Duration::from_secs(5));
    // The filter chain names the node it reads, and another mixer answers to
    // that name, so what it captures is not this test's to promise. The
    // plug-in chain below names ids instead and is checked either way.
    // The channel carries its trim by now, so this is about hearing the tone
    // at all rather than at any particular level; the low cut below is what
    // puts a number on it.
    let what = format!("the tone comes through the chain: {:.3}", through.0);
    if alone {
        check(through.0 > 0.01, &what, &mut failures);
    } else {
        println!("[skip] {what}");
    }

    engine
        .send(Command::SetEffects {
            id: source,
            effects: vec![pipedeck_engine::Effect {
                name: "Low cut".into(),
                kind: pipedeck_engine::EffectKind::Builtin,
                plugin: None,
                label: "bq_highpass".into(),
                controls: vec![pipedeck_engine::Control {
                    name: "Freq".into(),
                    value: 8_000.0,
                }],
            }],
        })
        .unwrap();
    // The chain is reloaded and the meter follows it, so the window has to
    // start after all of that rather than across it.
    // The chain is reloaded under the tone, which throws a transient or two,
    // so what counts is the quietest of several windows once it has settled
    // rather than the loudest moment of one.
    std::thread::sleep(Duration::from_secs(2));
    drain(&rx);
    let cut = (0..4)
        .map(|_| wait_levels(&rx, source, mix, Duration::from_millis(500)).0)
        .fold(f32::INFINITY, f32::min);
    let what = format!(
        "a low cut above the tone takes it out: {cut:.3} against {:.3}",
        through.0
    );
    if alone {
        check(cut < through.0 * 0.3, &what, &mut failures);
    } else {
        println!("[skip] {what}");
    }
    let _ = player.kill();
    let _ = player.wait();

    engine
        .send(Command::SetEffects {
            id: source,
            effects: Vec::new(),
        })
        .unwrap();
    wait_state(&rx, "the effect removed", |s| {
        s.sources.iter().all(|row| row.effects.is_empty())
    });
    settle();
    let dump = pw_dump();
    check(
        !node_names(&dump).contains(&format!("pipedeck.fx.{source}")),
        "removing the last effect takes the chain with it",
        &mut failures,
    );
    let reading = our_node(&dump, &format!("pipedeck.link.{source}.{mix}.in")).map(|node| {
        props(node)["target.object"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    check(
        reading.as_deref() == Some(format!("pipedeck.src.{source}").as_str()),
        &format!("the cell reads the channel again: {reading:?}"),
        &mut failures,
    );

    // A plug-in the mixer hosts itself sits after whatever PipeWire runs,
    // on a sink of its own, and the cells read that one.
    let plugins = pipedeck_engine::vst3::installed();
    if let Some(plugin) = plugins.first() {
        engine
            .send(Command::SetEffects {
                id: source,
                effects: vec![pipedeck_engine::Effect {
                    name: plugin.name.clone(),
                    kind: pipedeck_engine::EffectKind::Vst3,
                    plugin: None,
                    label: plugin.class_id.clone(),
                    controls: Vec::new(),
                }],
            })
            .unwrap();
        wait_state(&rx, "the plug-in", |s| {
            s.sources.iter().any(|row| !row.effects.is_empty())
        });
        std::thread::sleep(Duration::from_secs(2));
        let dump = pw_dump();
        let names = node_names(&dump);
        check(
            names.contains(&format!("pipedeck.vst.{source}")),
            &format!("{} runs on its own sink: {names:?}", plugin.name),
            &mut failures,
        );
        let reading = our_node(&dump, &format!("pipedeck.link.{source}.{mix}.in")).map(|node| {
            props(node)["target.object"]
                .as_str()
                .unwrap_or("")
                .to_owned()
        });
        check(
            reading.as_deref() == Some(format!("pipedeck.vst.{source}").as_str()),
            &format!("the cell reads the plug-in: {reading:?}"),
            &mut failures,
        );

        // And it has to pass audio, not merely exist.
        let mut player = Process::new("pw-play")
            .arg(format!("--target={sink_serial}"))
            .arg(&tone)
            .spawn()
            .expect("pw-play must be installed");
        let heard = wait_levels(&rx, source, mix, Duration::from_secs(5));
        let _ = player.kill();
        let _ = player.wait();
        check(
            heard.0 > 0.01,
            &format!("the tone comes out of the plug-in: {:.3}", heard.0),
            &mut failures,
        );

        engine
            .send(Command::SetEffects {
                id: source,
                effects: Vec::new(),
            })
            .unwrap();
        wait_state(&rx, "the plug-in removed", |s| {
            s.sources.iter().all(|row| row.effects.is_empty())
        });
        settle();
    } else {
        println!("[skip] no VST3 effect installed, so nothing to host");
    }

    // Stereo Tool is hosted the same way, and is the one plug-in most likely
    // to be installed. It is proprietary, so this runs only where it is.
    if pipedeck_engine::stereotool::installed() {
        engine
            .send(Command::SetEffects {
                id: source,
                effects: vec![pipedeck_engine::Effect {
                    name: "Stereo Tool".into(),
                    kind: pipedeck_engine::EffectKind::StereoTool,
                    plugin: None,
                    label: "stereotool".into(),
                    controls: Vec::new(),
                }],
            })
            .unwrap();
        wait_state(&rx, "Stereo Tool on the channel", |s| {
            s.sources.iter().any(|row| {
                row.effects
                    .iter()
                    .any(|effect| effect.kind == pipedeck_engine::EffectKind::StereoTool)
            })
        });
        std::thread::sleep(Duration::from_secs(2));
        let dump = pw_dump();
        let names = node_names(&dump);
        check(
            names.contains(&format!("pipedeck.vst.{source}")),
            &format!("Stereo Tool runs on a sink of its own: {names:?}"),
            &mut failures,
        );

        // Fed straight into the chain's own sink rather than through the
        // channel: the chain names ids, so this holds whether or not another
        // mixer answers to our node names.
        match node_serial(&dump, &format!("pipedeck.vst.{source}")) {
            Some(serial) => {
                let mut player = Process::new("pw-play")
                    .arg(format!("--target={serial}"))
                    .arg(&tone)
                    .spawn()
                    .expect("pw-play must be installed");
                drain(&rx);
                let heard = wait_levels(&rx, source, mix, Duration::from_secs(6));
                let _ = player.kill();
                let _ = player.wait();
                check(
                    heard.0 > 0.005,
                    &format!("the channel hears the tone through it: {:.3}", heard.0),
                    &mut failures,
                );
            }
            None => check(false, "its sink has no serial to play into", &mut failures),
        }

        // Its own window, opened the way the interface asks for it: from the
        // engine thread, not the one that started the process. X11 is
        // particular about that, and a library that minded would take the
        // whole mixer down rather than report anything. Off by default
        // because it puts a window on the screen of whoever runs the test.
        if std::env::var_os("PIPEDECK_SMOKE_WINDOW").is_some() {
            for open in [true, false] {
                engine
                    .send(Command::SetEffectWindow {
                        id: source,
                        index: 0,
                        open,
                    })
                    .unwrap();
                let complaint = wait_error(&rx, Duration::from_secs(4));
                check(
                    complaint.is_none(),
                    &format!(
                        "its window {} from the engine thread: {complaint:?}",
                        if open { "opens" } else { "closes" }
                    ),
                    &mut failures,
                );
            }
        }

        engine
            .send(Command::SetEffects {
                id: source,
                effects: Vec::new(),
            })
            .unwrap();
        wait_state(&rx, "Stereo Tool removed", |s| {
            s.sources.iter().all(|row| row.effects.is_empty())
        });
        settle();
    } else {
        println!("[skip] Stereo Tool is not installed, so nothing to host");
    }

    // A row bound to a microphone has no sink of its own, so its effects
    // read the device itself rather than a monitor. It is left unlinked on
    // purpose: the mix here plays to a speaker, and what a microphone hears
    // has no business coming back out of it.
    if let Some(microphone) = capture_devices.first() {
        engine
            .send(Command::AddSource {
                name: "Microphone".into(),
                device: Some(microphone.name.clone()),
                icon: None,
            })
            .unwrap();
        let mic = wait_state(&rx, "the microphone row", |s| s.sources.len() == 2)
            .sources
            .iter()
            .find(|row| row.device.is_some())
            .map(|row| row.id)
            .expect("the row that was just made");
        engine
            .send(Command::SetEffects {
                id: mic,
                effects: vec![pipedeck_engine::Effect {
                    name: "Low cut".into(),
                    kind: pipedeck_engine::EffectKind::Builtin,
                    plugin: None,
                    label: "bq_highpass".into(),
                    controls: vec![pipedeck_engine::Control {
                        name: "Freq".into(),
                        value: 90.0,
                    }],
                }],
            })
            .unwrap();
        wait_state(&rx, "the microphone effect", |s| {
            s.sources
                .iter()
                .any(|row| row.device.is_some() && !row.effects.is_empty())
        });
        settle();
        let dump = pw_dump();
        let names = node_names(&dump);
        check(
            names.contains(&format!("pipedeck.fx.{mic}")),
            &format!("a microphone's effects run on a sink of their own: {names:?}"),
            &mut failures,
        );
        let reading = our_node(&dump, &format!("pipedeck.fx.{mic}.in")).map(|node| {
            (
                props(node)["target.object"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
                props(node)["stream.capture.sink"].as_str().is_some(),
            )
        });
        check(
            reading
                .as_ref()
                .is_some_and(|(target, from_sink)| target == &microphone.name && !from_sink),
            &format!("they read the microphone itself, not a monitor: {reading:?}"),
            &mut failures,
        );
        engine.send(Command::RemoveSource(mic)).unwrap();
        wait_state(&rx, "the microphone row removed", |s| s.sources.len() == 1);
        settle();
        check(
            !node_names(&pw_dump()).contains(&format!("pipedeck.fx.{mic}")),
            "and go when the row does",
            &mut failures,
        );
    } else {
        println!("[skip] no capture device on this machine, so no microphone row");
    }

    // The quantum is a setting, not a fader: changing it reloads every
    // loopback, and the new value has to show on the nodes that come back.
    engine
        .send(Command::SetLatency {
            latency: "1024/48000".into(),
        })
        .unwrap();
    wait_state(&rx, "the new quantum", |s| s.latency == "1024/48000");
    settle();
    let reloaded = our_node(&pw_dump(), &format!("pipedeck.link.{source}.{mix}")).map(|node| {
        props(node)["node.latency"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    check(
        reloaded.as_deref() == Some("1024/48000"),
        &format!("the routes came back at the new quantum: {reloaded:?}"),
        &mut failures,
    );

    // An output switched off lets go of its device and stays in the list;
    // switched on, it comes back with the level it had.
    engine
        .send(Command::SetOutputEnabled {
            id: mix,
            index: 0,
            enabled: false,
        })
        .unwrap();
    wait_state(&rx, "the output switched off", |s| {
        s.mixes
            .iter()
            .any(|m| m.outputs.first().is_some_and(|o| !o.enabled))
    });
    settle();
    let names = node_names(&pw_dump());
    check(
        !names.contains(&format!("pipedeck.out.{mix}.0")),
        &format!("an output switched off lets go of its device: {names:?}"),
        &mut failures,
    );
    engine
        .send(Command::SetOutputEnabled {
            id: mix,
            index: 0,
            enabled: true,
        })
        .unwrap();
    wait_state(&rx, "the output switched on", |s| {
        s.mixes
            .iter()
            .any(|m| m.outputs.first().is_some_and(|o| o.enabled))
    });
    settle();
    let dump = pw_dump();
    let back = our_node(&dump, &format!("pipedeck.out.{mix}.0")).map(|node| {
        props(node)["target.object"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    let level = node_volume(&dump, &format!("pipedeck.out.{mix}.0"));
    check(
        back.as_deref() == Some(device.name.as_str())
            && level
                .as_ref()
                .is_some_and(|(v, muted)| v.iter().all(|x| (x - 0.512).abs() < 1e-3) && *muted),
        &format!("switched on, it plays there again with the level it had: {back:?} {level:?}"),
        &mut failures,
    );

    // Switching sound cards is one device on and every other off.
    if let Some(other) = devices.iter().find(|d| d.name != device.name) {
        engine
            .send(Command::SwitchOutput {
                id: mix,
                device: other.name.clone(),
            })
            .unwrap();
        let state = wait_state(&rx, "the switch", |s| {
            s.mixes.iter().any(|m| {
                m.outputs
                    .iter()
                    .any(|o| o.device == other.name && o.enabled)
            })
        });
        settle();
        let outputs = state
            .mixes
            .iter()
            .find(|m| m.id == mix)
            .map(|m| m.outputs.clone())
            .unwrap_or_default();
        let dump = pw_dump();
        let playing: Vec<String> = (0..outputs.len())
            .filter_map(|index| our_node(&dump, &format!("pipedeck.out.{mix}.{index}")))
            .filter_map(|node| props(node)["target.object"].as_str().map(str::to_owned))
            .collect();
        check(
            playing == [other.name.clone()],
            &format!(
                "switching sound cards leaves one playing: {playing:?} of {:?}",
                outputs
                    .iter()
                    .map(|o| (&o.device, o.enabled))
                    .collect::<Vec<_>>()
            ),
            &mut failures,
        );
        // Back to the one the rest of this test expects.
        engine
            .send(Command::SwitchOutput {
                id: mix,
                device: device.name.clone(),
            })
            .unwrap();
        wait_state(&rx, "the switch back", |s| {
            s.mixes.iter().any(|m| {
                m.outputs
                    .iter()
                    .any(|o| o.device == device.name && o.enabled)
            })
        });
        settle();
    } else {
        println!("[skip] only one output device, so nothing to switch to");
    }

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
