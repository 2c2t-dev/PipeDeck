//! Watches the mixer mend itself when WirePlumber restarts.
//!
//! A WirePlumber restart leaves PipeWire and every node in place, and still
//! undoes two things this mixer depends on: the metadata applications are
//! routed through is made again, empty, and the streams at either end of a
//! cell are renegotiated, taking the links the mixer made with them. This
//! runs one cell and one assigned application, and says, whenever it
//! changes, whether the cell is joined to its mix and the application plays
//! into its channel.
//!
//! `cargo run -p pipedeck-engine --example relink_check [seconds]`, against
//! a server whose WirePlumber you are willing to restart — a private one,
//! since restarting the session's own disturbs everybody's audio.

use std::collections::HashMap;
use std::process::{Command as Process, ExitCode};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pipedeck_engine::{spawn, Command, Event};

fn pw_dump() -> Vec<serde_json::Value> {
    Process::new("pw-dump")
        .output()
        .ok()
        .and_then(|out| serde_json::from_slice(&out.stdout).ok())
        .unwrap_or_default()
}

/// Node name by id, and the pairs of nodes a link joins.
fn graph() -> (HashMap<i64, String>, Vec<(i64, i64)>) {
    let dump = pw_dump();
    let names = dump
        .iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Node")))
        .filter_map(|o| {
            Some((
                o["id"].as_i64()?,
                o["info"]["props"]["node.name"].as_str()?.to_owned(),
            ))
        })
        .collect();
    let links = dump
        .iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Link")))
        .filter_map(|o| {
            Some((
                o["info"]["output-node-id"].as_i64()?,
                o["info"]["input-node-id"].as_i64()?,
            ))
        })
        .collect();
    (names, links)
}

fn joined(names: &HashMap<i64, String>, links: &[(i64, i64)], from: &str, into: &str) -> bool {
    links.iter().any(|(out, inp)| {
        names.get(out).is_some_and(|n| n == from) && names.get(inp).is_some_and(|n| n == into)
    })
}

/// A quiet tone to keep an application playing.
fn tone(path: &std::path::Path) {
    const RATE: u32 = 48_000;
    let frames = RATE * 120;
    let data = frames * 2;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample =
            ((i as f32 / RATE as f32 * 440.0 * std::f32::consts::TAU).sin() * 3000.0) as i16;
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, wav).expect("a tone to play");
}

fn main() -> ExitCode {
    env_logger::init();
    let patience = Duration::from_secs(
        std::env::args()
            .nth(1)
            .and_then(|arg| arg.parse().ok())
            .unwrap_or(40),
    );

    let dir = std::env::temp_dir().join(format!("pipedeck-relink-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory of its own");
    let (tx, rx) = mpsc::channel();
    let engine = spawn(dir.join("config.toml"), move |event| {
        let _ = tx.send(event);
    });

    // One mix, one channel, and the cell between them.
    let mut mixes = 0;
    let mut sources = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    engine.send(Command::AddMix).expect("the engine is up");
    engine
        .send(Command::AddSource {
            name: "Music".into(),
            device: None,
            icon: None,
        })
        .expect("the engine is up");
    while Instant::now() < deadline && (mixes == 0 || sources == 0) {
        if let Ok(Event::State(state)) = rx.recv_timeout(Duration::from_millis(200)) {
            mixes = state.mixes.len();
            sources = state.sources.len();
        }
    }
    engine
        .send(Command::SetLink {
            source: pipedeck_engine::SourceId(1),
            mix: pipedeck_engine::MixId(1),
            linked: true,
        })
        .expect("the engine is up");

    // An application to route, sent to the channel by the name it plays as.
    let tone_path = dir.join("tone.wav");
    tone(&tone_path);
    let mut player = Process::new("pw-play")
        .arg(&tone_path)
        .spawn()
        .expect("pw-play must be installed");
    std::thread::sleep(Duration::from_secs(2));
    engine
        .send(Command::AssignApp {
            id: pipedeck_engine::SourceId(1),
            app: "pw-play".into(),
        })
        .expect("the engine is up");

    let start = Instant::now();
    let mut last = (false, false);
    let mut history = Vec::new();
    while start.elapsed() < patience {
        while rx.try_recv().is_ok() {}
        let (names, links) = graph();
        let now = (
            joined(&names, &links, "pipedeck.link.1.1", "pipedeck.mix.1"),
            joined(&names, &links, "pw-play", "pipedeck.src.1"),
        );
        if now != last || history.is_empty() {
            println!(
                "[{:>5.1}s] cell joined to its mix: {:<5}  application in its channel: {}",
                start.elapsed().as_secs_f32(),
                now.0,
                now.1
            );
            history.push(now);
            last = now;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let _ = player.kill();
    let _ = player.wait();
    engine.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    // Mended means: both were true, something took one away, and both came
    // back.
    let broke = history.iter().any(|(cell, app)| !cell || !app);
    let whole_at_end = last == (true, true);
    let whole_before = history.contains(&(true, true));
    if whole_before && broke && whole_at_end {
        println!("[ ok ] it came apart and the mixer put it back together");
        ExitCode::SUCCESS
    } else if whole_at_end && !broke {
        println!("[----] nothing came apart, so nothing was tested");
        ExitCode::FAILURE
    } else {
        println!("[FAIL] it is not whole at the end: {last:?}");
        ExitCode::FAILURE
    }
}
