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
        .filter(|name| !name.starts_with("pipedeck-smoke.meter."))
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
                .is_some_and(|name| name.starts_with("pipedeck-smoke."))
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
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair).unsigned_abs() as f32 / 32768.0)
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
            Ok(Event::Levels { sources, mixes, .. }) => {
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

/// A client of the control socket, a line of JSON each way.
struct Control {
    writer: std::os::unix::net::UnixStream,
    reader: std::io::BufReader<std::os::unix::net::UnixStream>,
}

impl Control {
    fn open(path: &std::path::Path) -> Self {
        let stream = std::os::unix::net::UnixStream::connect(path).unwrap_or_else(|e| {
            panic!("cannot reach the control socket at {}: {e}", path.display())
        });
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a read timeout");
        Control {
            reader: std::io::BufReader::new(stream.try_clone().expect("a second handle")),
            writer: stream,
        }
    }

    fn say(&mut self, line: &str) {
        use std::io::Write;
        writeln!(self.writer, "{line}").expect("the socket takes a line");
    }

    fn read(&mut self) -> serde_json::Value {
        use std::io::BufRead;
        let mut line = String::new();
        let _ = self.reader.read_line(&mut line);
        serde_json::from_str(&line).unwrap_or_default()
    }

    /// Say something, and read the answer.
    fn ask(&mut self, line: &str) -> serde_json::Value {
        self.say(line);
        self.read()
    }
}

fn settle() {
    std::thread::sleep(Duration::from_millis(900));
}

/// The prefix this test's nodes go by, which the user's own mixer, running
/// beside it, does not: neither is taken for the other.
const PREFIX: &str = "pipedeck-smoke";
/// The application this test plays a call as, which is not the user's own
/// Vesktop, so the call's streams are neither moved by the user's mixer
/// nor the user's moved by this one.
const VOICE_APP: &str = "pipedeck-smoke-vesktop";

fn main() -> ExitCode {
    std::env::set_var("PIPEDECK_NODE_PREFIX", PREFIX);
    std::env::set_var("PIPEDECK_VOICE_APP", VOICE_APP);
    // A control socket of its own, beside the user's mixer's.
    let control_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("pipedeck-smoke-{}", std::process::id()));
    let control_path = control_dir.join("control.sock");
    std::env::set_var("PIPEDECK_CONTROL_SOCKET", &control_path);
    env_logger::init();
    let mut failures = 0;

    let dir = std::env::temp_dir().join(format!("pipedeck-smoke-{}", std::process::id()));
    let config_path = dir.join("config.toml");

    let (tx, rx) = mpsc::channel();
    let engine = spawn(config_path.clone(), move |ev| {
        let _ = tx.send(ev);
    });
    engine.serve_control();

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
        nodes == [format!("pipedeck-smoke.mix.{mix}")],
        &format!("a fresh config yields one mix and nothing else: {nodes:?}"),
        &mut failures,
    );
    let offered = our_node(&pw_dump(), &format!("pipedeck-smoke.mix.{mix}")).map(|node| {
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
                format!("pipedeck-smoke.mix.{mix}"),
                format!("pipedeck-smoke.src.{source}"),
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
    let link_node = format!("pipedeck-smoke.link.{source}.{mix}");
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
        nodes.contains(&format!("pipedeck-smoke.out.{mix}.0")),
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
    let master = node_volume(&dump, &format!("pipedeck-smoke.mix.{mix}"));
    check(
        master
            .as_ref()
            .is_some_and(|(v, _)| v.iter().all(|x| (x - 0.125).abs() < 1e-3)),
        &format!("the mix sink carries the master level: {master:?}"),
        &mut failures,
    );
    let output = node_volume(&dump, &format!("pipedeck-smoke.out.{mix}.0"));
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
        node_serial(&pw_dump(), &format!("pipedeck-smoke.src.{source}")).expect("the channel sink");
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
    let recorded = node_serial(&pw_dump(), &format!("pipedeck-smoke.mix.{mix}"))
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

    // The control socket, as a Stream Deck uses it: where things stand,
    // orders that land on the graph, and the meters while the tone plays.
    let mut control = Control::open(&control_path);
    let told = control.ask(r#"{"get": "state"}"#);
    let cell = told["state"]["cells"]
        .as_array()
        .and_then(|cells| {
            cells
                .iter()
                .find(|c| c["channel"] == source.0 && c["mix"] == mix.0)
        })
        .cloned();
    check(
        told["state"]["channels"][0]["name"] == "Smoke"
            && cell
                .as_ref()
                .is_some_and(|c| c["muted"] == true && c["volume"] == 0.5),
        &format!("the socket tells the channel and its muted cell: {cell:?}"),
        &mut failures,
    );
    let done = control.ask(&format!(
        r#"{{"do": {{"what": "channel", "id": {source}, "volume": 0.5}}}}"#
    ));
    settle();
    let volume = node_volume(&pw_dump(), &format!("pipedeck-smoke.src.{source}"));
    check(
        done["ok"] == true
            && volume
                .as_ref()
                .is_some_and(|(v, _)| v.iter().all(|x| (x - 0.125).abs() < 1e-3)),
        &format!("a level set on the socket lands on the channel's sink: {done} {volume:?}"),
        &mut failures,
    );
    let done = control.ask(&format!(
        r#"{{"do": {{"what": "cell", "channel": {source}, "mix": {mix}, "mute": "toggle", "nudge": -0.25}}}}"#
    ));
    settle();
    let volume = node_volume(&pw_dump(), &format!("pipedeck-smoke.link.{source}.{mix}"));
    check(
        done["ok"] == true
            && volume
                .as_ref()
                .is_some_and(|(v, m)| v.iter().all(|x| (x - 0.015625).abs() < 1e-3) && !*m),
        &format!("a cell turned over and nudged on the socket follows: {done} {volume:?}"),
        &mut failures,
    );
    let refused = control.ask(r#"{"do": {"what": "mix", "id": 9999, "mute": true}}"#);
    check(
        refused["error"]
            .as_str()
            .is_some_and(|e| e.contains("no mix")),
        &format!("an order for nothing is refused: {refused}"),
        &mut failures,
    );
    // Back as it was, for what follows.
    control.ask(&format!(
        r#"{{"do": {{"what": "channel", "id": {source}, "volume": 1.0}}}}"#
    ));
    control.ask(&format!(
        r#"{{"do": {{"what": "cell", "channel": {source}, "mix": {mix}, "mute": true, "volume": 0.5}}}}"#
    ));
    let mut meters = Control::open(&control_path);
    meters.say(r#"{"meters": true}"#);
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut heard, mut lines) = (0.0f64, 0);
    while Instant::now() < deadline {
        let line = meters.read();
        if let Some(channels) = line["levels"]["channels"].as_array() {
            lines += 1;
            for reading in channels {
                if reading[0] == source.0 {
                    heard = heard.max(reading[1].as_f64().unwrap_or(0.0));
                }
            }
        }
    }
    check(
        heard > 0.05 && (15..=40).contains(&lines),
        &format!("the socket's meters hear the tone, ten times a second: {heard:.3} in {lines} lines over 3 s"),
        &mut failures,
    );
    drop(meters);
    settle();

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
    let sink_id =
        node_id(&pw_dump(), &format!("pipedeck-smoke.src.{source}")).expect("the channel sink");
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
    let trim = node_volume(&pw_dump(), &format!("pipedeck-smoke.src.{source}"));
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
    let sink_node = node_id(&dump, &format!("pipedeck-smoke.src.{source}"));
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
                bypassed: false,
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
        names.contains(&format!("pipedeck-smoke.fx.{source}")),
        &format!("the effects chain offers a sink of its own: {names:?}"),
        &mut failures,
    );
    let reading = our_node(&dump, &format!("pipedeck-smoke.link.{source}.{mix}.in")).map(|node| {
        props(node)["target.object"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    check(
        reading.as_deref() == Some(format!("pipedeck-smoke.fx.{source}").as_str()),
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
                bypassed: false,
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
        !node_names(&dump).contains(&format!("pipedeck-smoke.fx.{source}")),
        "removing the last effect takes the chain with it",
        &mut failures,
    );
    let reading = our_node(&dump, &format!("pipedeck-smoke.link.{source}.{mix}.in")).map(|node| {
        props(node)["target.object"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    check(
        reading.as_deref() == Some(format!("pipedeck-smoke.src.{source}").as_str()),
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
                    bypassed: false,
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
            names.contains(&format!("pipedeck-smoke.vst.{source}")),
            &format!("{} runs on its own sink: {names:?}", plugin.name),
            &mut failures,
        );
        let reading =
            our_node(&dump, &format!("pipedeck-smoke.link.{source}.{mix}.in")).map(|node| {
                props(node)["target.object"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned()
            });
        check(
            reading.as_deref() == Some(format!("pipedeck-smoke.vst.{source}").as_str()),
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

    // The mixer's own effects run in the same chain as the plug-ins it
    // hosts, all four at once. Noise suppression is at no strength here: a
    // steady tone is exactly what it is made to take out.
    let native = |id: &str, changes: &[(&str, f32)]| {
        let spec = pipedeck_engine::dsp::spec(id).expect("an effect the mixer has");
        let mut controls = pipedeck_engine::dsp::defaults(spec);
        for (name, value) in changes {
            if let Some(control) = controls.iter_mut().find(|c| c.name == *name) {
                control.value = *value;
            }
        }
        pipedeck_engine::Effect {
            name: spec.name.to_owned(),
            kind: pipedeck_engine::EffectKind::Native,
            plugin: None,
            label: id.to_owned(),
            controls,
            bypassed: false,
        }
    };
    engine
        .send(Command::SetEffects {
            id: source,
            effects: vec![
                native("denoise", &[("strength", 0.0)]),
                native("eq", &[]),
                native("deesser", &[]),
                native("compressor", &[]),
            ],
        })
        .unwrap();
    wait_state(&rx, "the mixer's own effects", |s| {
        s.sources.iter().any(|row| row.effects.len() == 4)
    });
    std::thread::sleep(Duration::from_secs(2));
    let dump = pw_dump();
    let chain_in = node_id(&dump, &format!("pipedeck-smoke.vst.{source}.in"));
    check(
        chain_in.is_some(),
        &format!(
            "the four run in the mixer's own chain: {:?}",
            node_names(&dump)
                .into_iter()
                .filter(|n| n.contains(".vst."))
                .collect::<Vec<_>>()
        ),
        &mut failures,
    );
    // Played into the channel's own sink, so it goes through the chain:
    // the sink named after the chain is what comes out of it.
    match node_serial(&dump, &format!("pipedeck-smoke.src.{source}")) {
        Some(serial) => {
            let mut player = Process::new("pw-play")
                .arg(format!("--target={serial}"))
                .arg(&tone)
                .spawn()
                .expect("pw-play must be installed");
            drain(&rx);
            let heard = wait_levels(&rx, source, mix, Duration::from_secs(5));
            check(
                heard.0 > 0.01,
                &format!("the tone comes out of all four: {:.3}", heard.0),
                &mut failures,
            );

            // The compressor says what it is doing as it runs, for its window
            // to draw: the level it hears, as its threshold is compared with.
            let live = wait_for(&rx, "the compressor's own level", |e| {
                matches!(e, Event::Levels { effects, .. }
                    if effects.iter().any(|fx| fx.source == source && fx.index == 3 && fx.level > -60.0))
            });
            let level = match &live {
                Event::Levels { effects, .. } => effects
                    .iter()
                    .find(|fx| fx.index == 3)
                    .map(|fx| (fx.level, fx.reduction)),
                _ => None,
            };
            check(
                level.is_some_and(|(level, reduction)| level < 0.0 && reduction >= 0.0),
                &format!("the compressor says what it hears as it runs: {level:?}"),
                &mut failures,
            );
            // So do the de-esser, of the s it hears, and noise suppression,
            // of how sure it is of a voice: a chance, between 0 and 1.
            let said = wait_for(&rx, "the de-esser and noise suppression say", |e| {
                matches!(e, Event::Levels { effects, .. }
                    if effects.iter().any(|fx| fx.index == 0 && fx.level.is_finite())
                        && effects.iter().any(|fx| fx.index == 2 && fx.level.is_finite()))
            });
            let (denoise, deesser) = match &said {
                Event::Levels { effects, .. } => (
                    effects
                        .iter()
                        .find(|fx| fx.index == 0)
                        .map(|fx| (fx.level, fx.reduction)),
                    effects
                        .iter()
                        .find(|fx| fx.index == 2)
                        .map(|fx| (fx.level, fx.reduction)),
                ),
                _ => (None, None),
            };
            check(
                denoise
                    .is_some_and(|(voice, removed)| (0.0..=1.0).contains(&voice) && removed >= 0.0)
                    && deesser.is_some_and(|(level, reduction)| level < 0.0 && reduction >= 0.0),
                &format!(
                    "the de-esser and noise suppression say what they do: {deesser:?} {denoise:?}"
                ),
                &mut failures,
            );

            // The compressor listens to the tone still playing and sets
            // itself from it. A steady tone is as loud at its loudest as it
            // usually is, so the threshold lands on it, nothing is squeezed,
            // and the makeup is what brings it to -10 dB.
            let learn = |step| {
                engine
                    .send(Command::LearnEffect {
                        id: source,
                        index: 3,
                        step,
                    })
                    .unwrap()
            };
            learn(pipedeck_engine::Learning::Start);
            std::thread::sleep(Duration::from_secs(2));
            learn(pipedeck_engine::Learning::Finish);
            let learnt = wait_for(
                &rx,
                "the compressor set from what it heard",
                |e| matches!(e, Event::SourceEffects { effects, .. } if effects.len() == 4),
            );
            let learnt = |name: &str| match &learnt {
                Event::SourceEffects { effects, .. } => effects[3]
                    .controls
                    .iter()
                    .find(|c| c.name == name)
                    .map_or(f32::NAN, |c| c.value),
                _ => f32::NAN,
            };
            let (threshold, makeup) = (learnt("threshold"), learnt("makeup"));
            check(
                threshold != -20.0 && (threshold + makeup + 10.0).abs() <= 1.0,
                &format!(
                    "the compressor learns the tone's level: threshold {threshold}, makeup \
                     {makeup}"
                ),
                &mut failures,
            );
            let _ = player.kill();
            let _ = player.wait();
        }
        None => check(false, "the chain has no sink to play into", &mut failures),
    }

    // A control moved is a setting written where the effect runs: the chain
    // is not made again, so its nodes are the same ones after.
    engine
        .send(Command::SetEffectParams {
            id: source,
            index: 1,
            controls: native("eq", &[("mid_gain", 9.0)]).controls,
        })
        .unwrap();
    wait_for(&rx, "the setting taken", |e| {
        matches!(e, Event::SourceEffects { effects, .. }
            if effects.get(1).is_some_and(|eq| eq.controls.iter().any(|c| c.name == "mid_gain" && c.value == 9.0)))
    });
    settle();
    let after = node_id(&pw_dump(), &format!("pipedeck-smoke.vst.{source}.in"));
    check(
        chain_in.is_some() && after == chain_in,
        &format!("a control moved reloads nothing: {chain_in:?} then {after:?}"),
        &mut failures,
    );

    // An effect switched off is skipped where it runs, the chain left as it
    // is: the compressor, bypassed, hears nothing of the tone going past
    // it, and hears it again once back on.
    match node_serial(&pw_dump(), &format!("pipedeck-smoke.src.{source}")) {
        Some(serial) => {
            let mut player = Process::new("pw-play")
                .arg(format!("--target={serial}"))
                .arg(&tone)
                .spawn()
                .expect("pw-play must be installed");
            let compressor_hears = |rx: &mpsc::Receiver<Event>| {
                let deadline = Instant::now() + Duration::from_secs(2);
                let mut loudest = f32::NEG_INFINITY;
                while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                    if let Ok(Event::Levels { effects, .. }) = rx.recv_timeout(left) {
                        for fx in effects
                            .iter()
                            .filter(|fx| fx.source == source && fx.index == 3)
                        {
                            loudest = loudest.max(fx.level);
                        }
                    }
                }
                loudest
            };
            std::thread::sleep(Duration::from_secs(1));
            engine
                .send(Command::SetEffectBypass {
                    id: source,
                    index: 3,
                    bypassed: true,
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(500));
            drain(&rx);
            let off = compressor_hears(&rx);
            engine
                .send(Command::SetEffectBypass {
                    id: source,
                    index: 3,
                    bypassed: false,
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(500));
            drain(&rx);
            let on = compressor_hears(&rx);
            let kept = node_id(&pw_dump(), &format!("pipedeck-smoke.vst.{source}.in"));
            check(
                off < -60.0 && on > -60.0 && kept == chain_in,
                &format!(
                    "a bypassed effect hears nothing, back on it hears the tone, and the chain stays: {off:.1} then {on:.1} dB, {chain_in:?} then {kept:?}"
                ),
                &mut failures,
            );
            let _ = player.kill();
            let _ = player.wait();
        }
        None => check(false, "the chain has no sink to play into", &mut failures),
    }

    // A chain made and dropped while sound goes through it, over and over:
    // the plug-ins must not go while the audio thread is still running them.
    match node_serial(&pw_dump(), &format!("pipedeck-smoke.src.{source}")) {
        Some(serial) => {
            let mut player = Process::new("pw-play")
                .arg(format!("--target={serial}"))
                .arg(&tone)
                .spawn()
                .expect("pw-play must be installed");
            for round in 0..20 {
                let effects = if round % 2 == 0 {
                    vec![native("compressor", &[])]
                } else {
                    Vec::new()
                };
                engine
                    .send(Command::SetEffects {
                        id: source,
                        effects,
                    })
                    .unwrap();
                std::thread::sleep(Duration::from_millis(400));
            }
            let _ = player.kill();
            let _ = player.wait();
            drain(&rx);
            let alive = engine
                .send(Command::SetEffects {
                    id: source,
                    effects: Vec::new(),
                })
                .is_ok();
            wait_state(&rx, "the chain dropped for good", |s| {
                s.sources.iter().all(|row| row.effects.is_empty())
            });
            check(
                alive,
                "a chain made and dropped twenty times with sound going through",
                &mut failures,
            );
        }
        None => check(false, "the channel has no sink to play into", &mut failures),
    }

    // A plug-in uninstalled since it was chosen keeps its place in the chain:
    // the compressor after it is still the second effect, and says so.
    engine
        .send(Command::SetEffects {
            id: source,
            effects: vec![
                pipedeck_engine::Effect {
                    name: "Gone".into(),
                    kind: pipedeck_engine::EffectKind::Vst3,
                    plugin: None,
                    label: "00000000000000000000000000000000".into(),
                    controls: Vec::new(),
                    bypassed: false,
                },
                native("compressor", &[]),
            ],
        })
        .unwrap();
    wait_state(&rx, "a missing plug-in and a compressor", |s| {
        s.sources.iter().any(|row| row.effects.len() == 2)
    });
    std::thread::sleep(Duration::from_secs(2));
    let kept = match node_serial(&pw_dump(), &format!("pipedeck-smoke.src.{source}")) {
        Some(serial) => {
            let mut player = Process::new("pw-play")
                .arg(format!("--target={serial}"))
                .arg(&tone)
                .spawn()
                .expect("pw-play must be installed");
            drain(&rx);
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut found = false;
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                if let Ok(Event::Levels { effects, .. }) = rx.recv_timeout(left) {
                    if effects
                        .iter()
                        .any(|fx| fx.source == source && fx.index == 1 && fx.level.is_finite())
                    {
                        found = true;
                        break;
                    }
                }
            }
            let _ = player.kill();
            let _ = player.wait();
            found
        }
        None => false,
    };
    check(
        kept,
        "an effect after a missing plug-in keeps its place",
        &mut failures,
    );
    engine
        .send(Command::SetEffects {
            id: source,
            effects: Vec::new(),
        })
        .unwrap();
    wait_state(&rx, "the mixer's own effects removed", |s| {
        s.sources.iter().all(|row| row.effects.is_empty())
    });
    settle();

    // A call in Vesktop needs Vesktop assigned to a channel of this test's,
    // and another mixer on the graph would have Vesktop assigned too: both
    // would move its streams, and the real ones of whoever runs the test
    // with it.
    if alone {
        // A call in Vesktop: the channel Vesktop is assigned to gets a sink for
        // each person, playing into its own, and loses them when the call ends.
        let voice = |user: &str| format!("pipedeck-smoke.voice.{source}.{user}");
        engine
            .send(Command::AssignApp {
                id: source,
                app: VOICE_APP.into(),
            })
            .unwrap();
        engine
            .send(Command::SetCall {
                members: vec![
                    pipedeck_engine::CallMember {
                        id: "111".into(),
                        name: "Alice".into(),
                        avatar: None,
                    },
                    pipedeck_engine::CallMember {
                        id: "222".into(),
                        name: "Bob".into(),
                        avatar: None,
                    },
                ],
            })
            .unwrap();
        wait_state(&rx, "the call's voices", |s| {
            s.sources
                .iter()
                .any(|row| row.voices.iter().filter(|v| v.present).count() == 2)
        });
        std::thread::sleep(Duration::from_secs(2));
        let dump = pw_dump();
        let (alice, bob) = (node_id(&dump, &voice("111")), node_id(&dump, &voice("222")));
        let row = node_id(&dump, &format!("pipedeck-smoke.src.{source}"));
        let labelled = our_node(&dump, &voice("111"))
            .and_then(|node| props(node)["node.description"].as_str())
            == Some("Alice (Discord)");
        check(
            alice.is_some() && bob.is_some() && labelled,
            &format!("each person of the call has a sink of their own: {alice:?} {bob:?}"),
            &mut failures,
        );
        check(
            matches!((alice, row), (Some(a), Some(r)) if links(&dump).contains(&(a, r))),
            "a person's sink plays into the channel's",
            &mut failures,
        );
        match node_serial(&dump, &voice("111")) {
            Some(serial) => {
                let mut player = Process::new("pw-play")
                    .arg(format!("--target={serial}"))
                    .arg(&tone)
                    .spawn()
                    .expect("pw-play must be installed");
                drain(&rx);
                let heard = wait_levels(&rx, source, mix, Duration::from_secs(4));
                check(
                    heard.0 > 0.005,
                    &format!("the channel hears Alice: {:.3}", heard.0),
                    &mut failures,
                );
                engine
                    .send(Command::SetVoiceMute {
                        id: source,
                        user: "111".into(),
                        muted: true,
                    })
                    .unwrap();
                settle();
                drain(&rx);
                let muted = wait_levels(&rx, source, mix, Duration::from_secs(3));
                let _ = player.kill();
                let _ = player.wait();
                check(
                    muted.0 < heard.0 / 10.0,
                    &format!("muting Alice takes her out: {:.3}", muted.0),
                    &mut failures,
                );
            }
            None => check(false, "Alice's sink has no serial", &mut failures),
        }
        engine
            .send(Command::SetVoiceMute {
                id: source,
                user: "111".into(),
                muted: false,
            })
            .unwrap();

        // Vesktop sends each person to their sink by name, and its own mix
        // wherever the user put Vesktop: a stream of Vesktop's aimed at a voice
        // sink is left there, not moved to the channel with the rest of it.
        // A player passes for Vesktop by the name it runs under, which is
        // what the server takes an application's binary to be: a link to
        // pw-cat called as this test's Vesktop, told to play since its name
        // no longer says so.
        let player_path = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("pw-cat"))
                    .find(|candidate| candidate.is_file())
            })
            .expect("pw-cat must be installed");
        let disguised = dir.join(VOICE_APP);
        let _ = std::fs::remove_file(&disguised);
        std::os::unix::fs::symlink(&player_path, &disguised).expect("a link to pw-play");
        let mut aimed = Process::new(&disguised)
            .arg("--playback")
            .arg(format!("--target={}", voice("111")))
            .arg("-P")
            .arg(r#"{ "node.name": "smoke-voice-player" }"#)
            .arg(&tone)
            .spawn()
            .expect("the disguised player starts");
        std::thread::sleep(Duration::from_secs(3));
        let dump = pw_dump();
        let player = any_node_id(&dump, "smoke-voice-player");
        let passes = dump
            .iter()
            .find(|o| props(o)["node.name"].as_str() == Some("smoke-voice-player"))
            .and_then(|o| {
                // What the mixer matches an application by: its binary, or
                // its name when the server does not say the binary.
                let props = props(o);
                props["application.process.binary"]
                    .as_str()
                    .or_else(|| props["application.name"].as_str())
            })
            == Some(VOICE_APP);
        let alice = node_id(&dump, &voice("111"));
        let stayed =
            matches!((player, alice), (Some(p), Some(a)) if links(&dump).contains(&(p, a)));
        let _ = aimed.kill();
        let _ = aimed.wait();
        check(
            passes && stayed,
            &format!(
                "Vesktop's stream for Alice stays on her sink: passes for Vesktop {passes}, \
                 {player:?} -> {alice:?}"
            ),
            &mut failures,
        );

        engine
            .send(Command::SetCall {
                members: Vec::new(),
            })
            .unwrap();
        wait_state(&rx, "the call ended", |s| {
            s.sources
                .iter()
                .all(|row| row.voices.iter().all(|v| !v.present))
        });
        // A call that drops and comes back says everyone left, so a person's
        // sink outlives them a while.
        std::thread::sleep(Duration::from_secs(1));
        check(
            node_id(&pw_dump(), &voice("111")).is_some(),
            "a person's sink outlives their leaving for a moment",
            &mut failures,
        );
        std::thread::sleep(Duration::from_secs(17));
        let dump = pw_dump();
        check(
            node_id(&dump, &voice("111")).is_none() && node_id(&dump, &voice("222")).is_none(),
            "the voices go a while after the call ends",
            &mut failures,
        );
        engine
            .send(Command::ReleaseApp {
                id: source,
                app: VOICE_APP.into(),
            })
            .unwrap();
        settle();
    } else {
        println!("[skip] a call in Vesktop: another mixer moves Vesktop's streams too");
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
                    bypassed: false,
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
            names.contains(&format!("pipedeck-smoke.vst.{source}")),
            &format!("Stereo Tool runs on a sink of its own: {names:?}"),
            &mut failures,
        );

        // Played into the channel's own sink, so it goes through the chain:
        // the sink named after the chain is what comes out of it. Found by
        // our pid, so another mixer answering to the same names is left
        // alone.
        match node_serial(&dump, &format!("pipedeck-smoke.src.{source}")) {
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
                    bypassed: false,
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
            names.contains(&format!("pipedeck-smoke.fx.{mic}")),
            &format!("a microphone's effects run on a sink of their own: {names:?}"),
            &mut failures,
        );
        let reading = our_node(&dump, &format!("pipedeck-smoke.fx.{mic}.in")).map(|node| {
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
            !node_names(&pw_dump()).contains(&format!("pipedeck-smoke.fx.{mic}")),
            "and go when the row does",
            &mut failures,
        );
    } else {
        println!("[skip] no capture device on this machine, so no microphone row");
    }

    // The quantum is a setting, not a fader: changing it reloads every
    // loopback, and the new value has to show on the nodes that come back,
    // a row's plug-ins included.
    engine
        .send(Command::SetEffects {
            id: source,
            effects: vec![native("compressor", &[])],
        })
        .unwrap();
    wait_state(&rx, "a compressor before the new quantum", |s| {
        s.sources.iter().any(|row| row.effects.len() == 1)
    });
    std::thread::sleep(Duration::from_secs(2));
    engine
        .send(Command::SetLatency {
            latency: "1024/48000".into(),
        })
        .unwrap();
    wait_state(&rx, "the new quantum", |s| s.latency == "1024/48000");
    settle();
    let reloaded =
        our_node(&pw_dump(), &format!("pipedeck-smoke.link.{source}.{mix}")).map(|node| {
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
    std::thread::sleep(Duration::from_secs(2));
    let chain = our_node(&pw_dump(), &format!("pipedeck-smoke.vst.{source}.in")).map(|node| {
        props(node)["node.latency"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    check(
        chain.as_deref() == Some("1024/48000"),
        &format!("the plug-ins came back at the new quantum: {chain:?}"),
        &mut failures,
    );
    engine
        .send(Command::SetEffects {
            id: source,
            effects: Vec::new(),
        })
        .unwrap();
    wait_state(&rx, "the compressor taken off", |s| {
        s.sources.iter().all(|row| row.effects.is_empty())
    });
    settle();

    // A row renamed is made again under its new name, which the system
    // lists it by, and its cells with it.
    engine
        .send(Command::RenameSource {
            id: source,
            name: "Smoke Renamed".into(),
        })
        .unwrap();
    wait_state(&rx, "the row renamed", |s| {
        s.sources.iter().any(|row| row.name == "Smoke Renamed")
    });
    std::thread::sleep(Duration::from_secs(2));
    let dump = pw_dump();
    let described = our_node(&dump, &format!("pipedeck-smoke.src.{source}"))
        .and_then(|node| props(node)["node.description"].as_str())
        .map(str::to_owned);
    let cell = our_node(&dump, &format!("pipedeck-smoke.link.{source}.{mix}")).is_some();
    check(
        described.as_deref() == Some("Pipedeck: Smoke Renamed") && cell,
        &format!("a row renamed is listed by its new name: {described:?}, cell {cell}"),
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
        !names.contains(&format!("pipedeck-smoke.out.{mix}.0")),
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
    let back = our_node(&dump, &format!("pipedeck-smoke.out.{mix}.0")).map(|node| {
        props(node)["target.object"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    let level = node_volume(&dump, &format!("pipedeck-smoke.out.{mix}.0"));
    check(
        back.as_deref() == Some(device.name.as_str())
            && level
                .as_ref()
                .is_some_and(|(v, muted)| v.iter().all(|x| (x - 0.512).abs() < 1e-3) && *muted),
        &format!("switched on, it plays there again with the level it had: {back:?} {level:?}"),
        &mut failures,
    );

    // The ear: whether this mix is heard on the device you listen on. Off,
    // nothing of it plays there; on, it plays there again.
    engine
        .send(Command::SetListening {
            id: mix,
            listening: false,
        })
        .unwrap();
    wait_state(&rx, "the ear off", |s| {
        s.mixes.iter().any(|m| m.outputs.iter().all(|o| !o.enabled))
    });
    settle();
    let names = node_names(&pw_dump());
    check(
        !names
            .iter()
            .any(|n| n.starts_with(&format!("pipedeck-smoke.out.{mix}."))),
        &format!("with its ear off, the mix is not heard: {names:?}"),
        &mut failures,
    );
    engine
        .send(Command::SetListening {
            id: mix,
            listening: true,
        })
        .unwrap();
    wait_state(&rx, "the ear on", |s| {
        s.mixes.iter().any(|m| m.outputs.iter().any(|o| o.enabled))
    });
    settle();
    let back = our_node(&pw_dump(), &format!("pipedeck-smoke.out.{mix}.0")).map(|node| {
        props(node)["target.object"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    });
    check(
        back.as_deref() == Some(device.name.as_str()),
        &format!("with its ear on, it is heard there again: {back:?}"),
        &mut failures,
    );

    // Switching sound cards moves every mix you hear onto the new device,
    // and off the old one.
    if let Some(other) = devices.iter().find(|d| d.name != device.name) {
        engine
            .send(Command::SetListenDevice(other.name.clone()))
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
            .filter_map(|index| our_node(&dump, &format!("pipedeck-smoke.out.{mix}.{index}")))
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
            .send(Command::SetListenDevice(device.name.clone()))
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
        !nodes.iter().any(|n| n.starts_with("pipedeck-smoke.link.")),
        &format!("the cell is gone, the rest stays: {nodes:?}"),
        &mut failures,
    );

    // A row removed hands its applications back: none stays aimed at the
    // name of a sink that is gone.
    let mut player = Process::new("pw-play")
        .arg("-P")
        .arg(r#"{ "node.name": "smoke-removed-player" }"#)
        .arg(&tone)
        .spawn()
        .expect("pw-play must be installed");
    std::thread::sleep(Duration::from_secs(1));
    engine
        .send(Command::AssignApp {
            id: source,
            app: "pw-play".into(),
        })
        .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let player_id = any_node_id(&pw_dump(), "smoke-removed-player");
    let aimed = |id: Option<i64>| {
        let listed = Process::new("pw-metadata")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
            .unwrap_or_default();
        id.is_some_and(|id| {
            listed.lines().any(|line| {
                line.contains(&format!("id:{id} ")) && line.contains("pipedeck-smoke.src.")
            })
        })
    };
    let before = aimed(player_id);

    engine.send(Command::RemoveSource(source)).unwrap();
    wait_state(&rx, "source removed", |s| s.sources.is_empty());
    settle();
    let after = aimed(player_id);
    let _ = player.kill();
    let _ = player.wait();
    check(
        before && !after,
        &format!("a row removed hands its applications back: aimed before {before}, after {after}"),
        &mut failures,
    );
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
    let _ = std::fs::remove_dir_all(&control_dir);
    if failures == 0 {
        println!("SMOKE PASS");
        ExitCode::SUCCESS
    } else {
        println!("SMOKE FAIL ({failures})");
        ExitCode::FAILURE
    }
}
