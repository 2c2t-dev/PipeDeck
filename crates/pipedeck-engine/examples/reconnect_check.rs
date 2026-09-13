//! Watches the engine lose PipeWire and put itself back together.
//!
//! `cargo run -p pipedeck-engine --example reconnect_check`, having pointed
//! `PIPEWIRE_REMOTE` at a server you are willing to restart — a private one,
//! since restarting the session's own cuts everybody's audio. The script in
//! the README does that; this is the half that watches.
//!
//! It reports what the engine says, and exits when the mixer is back on the
//! graph after a server has gone away under it.

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pipedeck_engine::{spawn, Command, Event};

fn main() -> ExitCode {
    env_logger::init();
    let patience = Duration::from_secs(
        std::env::args()
            .nth(1)
            .and_then(|arg| arg.parse().ok())
            .unwrap_or(60),
    );

    let dir = std::env::temp_dir().join(format!("pipedeck-reconnect-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory of its own");
    let (tx, rx) = mpsc::channel();
    let engine = spawn(dir.join("config.toml"), move |event| {
        let _ = tx.send(event);
    });

    // One mix, so there is something to put back.
    engine.send(Command::AddMix).expect("the engine is up");

    let deadline = Instant::now() + patience;
    let mut states = 0;
    let mut lost = false;
    let mut verdict = ExitCode::FAILURE;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(Event::State(state)) => {
                states += 1;
                let mixes = state.mixes.len();
                println!("[state] {states} mix(es): {mixes}");
                if lost && mixes > 0 {
                    println!("[ ok ] the mixer is back on the graph, {mixes} mix(es) and all");
                    verdict = ExitCode::SUCCESS;
                    break;
                }
            }
            Ok(Event::Notice(message)) | Ok(Event::Error(message)) => {
                println!("[said] {message}");
                if message.contains("went away") {
                    lost = true;
                }
            }
            Ok(Event::Stopped) => {
                println!("[FAIL] the engine stopped instead of waiting for the server");
                break;
            }
            Ok(_) => {}
            Err(_) => {
                println!("[FAIL] nothing more came out of the engine");
                break;
            }
        }
    }
    if !lost {
        println!("[FAIL] the server never went away, so nothing was tested");
        verdict = ExitCode::FAILURE;
    }

    engine.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
    verdict
}
