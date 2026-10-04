//! Pipedeck on a Stream Deck, as an OpenDeck plugin: keys and dials that
//! mute and move the levels of channels, mixes, cells and the people of a
//! call, choose the mix heard in the headphones and the device it is heard
//! on, and show what they control as the mixer draws it.
//!
//! OpenDeck starts it with the port of its WebSocket, as the Stream Deck
//! SDK has it; Pipedeck is reached through its control socket. Both are
//! read on this thread, OpenDeck's with a short timeout so the mixer's news
//! is passed on as it comes.
//!
//! `pipedeck-opendeck --icons DIR` writes the icons the plugin is listed
//! with, which `install.sh` does.

mod deck;
mod draw;
mod mixer;
#[path = "../../../crates/pipedeck/src/presets.rs"]
#[allow(dead_code)]
mod presets;

use std::net::TcpStream;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::{Message, WebSocket};

/// How long a read of OpenDeck waits before the mixer's news is looked at.
const POLL: Duration = Duration::from_millis(20);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    if let Some(dir) = arg("--icons") {
        return write_icons(&dir);
    }

    let (Some(port), Some(uuid), Some(register)) =
        (arg("-port"), arg("-pluginUUID"), arg("-registerEvent"))
    else {
        eprintln!(
            "pipedeck-opendeck is started by OpenDeck, with -port, -pluginUUID and -registerEvent"
        );
        return ExitCode::FAILURE;
    };

    match run(&port, &uuid, &register) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pipedeck-opendeck: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(port: &str, uuid: &str, register: &str) -> Result<(), Box<dyn std::error::Error>> {
    let stream = TcpStream::connect(("127.0.0.1", port.parse::<u16>()?))?;
    let (mut socket, _) = tungstenite::client(format!("ws://127.0.0.1:{port}"), stream)?;
    send(&mut socket, &json!({ "event": register, "uuid": uuid }))?;
    socket.get_mut().set_read_timeout(Some(POLL))?;

    let (changed, news) = mpsc::channel();
    let mut deck = deck::Deck::new(mixer::Mixer::start(changed));

    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                if let Ok(message) = serde_json::from_str::<Value>(&text) {
                    for reply in deck.hear(&message) {
                        send(&mut socket, &reply)?;
                    }
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(tungstenite::Error::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        deck.tick();
        // Only the latest state matters.
        if let Some(view) = news.try_iter().last() {
            for message in deck.mixer_changed(view) {
                send(&mut socket, &message)?;
            }
        }
    }
}

fn send(socket: &mut WebSocket<TcpStream>, message: &Value) -> tungstenite::Result<()> {
    socket.send(Message::text(message.to_string()))
}

fn write_icons(dir: &str) -> ExitCode {
    for (name, svg) in draw::catalogue_icons() {
        let path = std::path::Path::new(dir).join(format!("{name}.svg"));
        if let Err(e) = std::fs::write(&path, svg) {
            eprintln!("cannot write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
