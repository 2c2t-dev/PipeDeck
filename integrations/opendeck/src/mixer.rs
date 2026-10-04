//! Pipedeck's control socket, followed for as long as the plugin runs.
//!
//! One connection, opened again whenever Pipedeck comes back: it subscribes
//! to the mixer's state, which comes as a line of JSON after every change,
//! and carries the deck's orders the other way. The protocol is described
//! in `crates/pipedeck-engine/src/control.rs`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// How long to wait before trying again when Pipedeck is not running.
const RETRY: Duration = Duration::from_secs(2);

/// Where things stand, as the socket tells it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct View {
    pub channels: Vec<Channel>,
    pub mixes: Vec<Mix>,
    pub cells: Vec<Cell>,
    pub listen: Option<String>,
    pub outputs: Vec<Output>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Channel {
    pub id: u32,
    pub name: String,
    pub icon: Option<String>,
    pub volume: f32,
    pub muted: bool,
    pub input: bool,
    pub voices: Vec<Voice>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Mix {
    pub id: u32,
    pub name: String,
    pub icon: Option<String>,
    pub volume: f32,
    pub muted: bool,
    pub listening: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Cell {
    pub channel: u32,
    pub mix: u32,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Voice {
    pub user: String,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Output {
    pub name: String,
    pub description: String,
}

/// What a key can be aimed at, as it is kept in the key's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "lowercase")]
pub enum Target {
    Channel { id: u32 },
    Mix { id: u32 },
    Cell { channel: u32, mix: u32 },
    Voice { channel: u32, user: String },
    Output { device: String },
}

/// The kinds of target, to say which an action takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Channel,
    Mix,
    Cell,
    Voice,
    Output,
}

/// What a target is now, as a key shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub name: String,
    /// A preset's key, see `crates/pipedeck/src/presets.rs`.
    pub icon: Option<String>,
    pub input: bool,
    pub mix: bool,
    pub volume: f32,
    pub muted: bool,
    pub listening: bool,
}

impl View {
    pub fn find(&self, target: &Target) -> Option<Found> {
        let channel = |id: u32| self.channels.iter().find(|c| c.id == id);
        let mix = |id: u32| self.mixes.iter().find(|m| m.id == id);
        match target {
            Target::Channel { id } => channel(*id).map(|c| Found {
                name: c.name.clone(),
                icon: c.icon.clone(),
                input: c.input,
                mix: false,
                volume: c.volume,
                muted: c.muted,
                listening: false,
            }),
            Target::Mix { id } => mix(*id).map(|m| Found {
                name: m.name.clone(),
                icon: m.icon.clone(),
                input: false,
                mix: true,
                volume: m.volume,
                muted: m.muted,
                listening: m.listening,
            }),
            Target::Cell {
                channel: from,
                mix: into,
            } => {
                let cell = self
                    .cells
                    .iter()
                    .find(|c| c.channel == *from && c.mix == *into)?;
                let (from, into) = (channel(*from)?, mix(*into)?);
                Some(Found {
                    name: format!("{} → {}", from.name, into.name),
                    icon: from.icon.clone(),
                    input: from.input,
                    mix: false,
                    volume: cell.volume,
                    muted: cell.muted,
                    listening: false,
                })
            }
            Target::Voice { channel: on, user } => {
                let voice = channel(*on)?.voices.iter().find(|v| v.user == *user)?;
                Some(Found {
                    name: voice.name.clone(),
                    icon: Some("people".to_owned()),
                    input: false,
                    mix: false,
                    volume: voice.volume,
                    muted: voice.muted,
                    listening: false,
                })
            }
            Target::Output { device } => {
                let output = self.outputs.iter().find(|o| o.name == *device)?;
                Some(Found {
                    name: output.description.clone(),
                    icon: Some("headset".to_owned()),
                    input: false,
                    mix: false,
                    volume: 0.0,
                    muted: false,
                    listening: self.listen.as_deref() == Some(device.as_str()),
                })
            }
        }
    }

    /// Everything of the kinds asked that can be aimed at, with what to
    /// call it, in the mixer's order.
    pub fn targets(&self, kinds: &[Kind]) -> Vec<(Target, String)> {
        let mut found = Vec::new();
        if kinds.contains(&Kind::Channel) {
            for c in &self.channels {
                found.push((
                    Target::Channel { id: c.id },
                    format!("Channel · {}", c.name),
                ));
            }
        }
        if kinds.contains(&Kind::Mix) {
            for m in &self.mixes {
                found.push((Target::Mix { id: m.id }, format!("Mix · {}", m.name)));
            }
        }
        if kinds.contains(&Kind::Cell) {
            for cell in &self.cells {
                let target = Target::Cell {
                    channel: cell.channel,
                    mix: cell.mix,
                };
                if let Some(found_cell) = self.find(&target) {
                    found.push((target, found_cell.name));
                }
            }
        }
        if kinds.contains(&Kind::Voice) {
            for c in &self.channels {
                for v in &c.voices {
                    found.push((
                        Target::Voice {
                            channel: c.id,
                            user: v.user.clone(),
                        },
                        format!("Voice · {} ({})", v.name, c.name),
                    ));
                }
            }
        }
        if kinds.contains(&Kind::Output) {
            for o in &self.outputs {
                found.push((
                    Target::Output {
                        device: o.name.clone(),
                    },
                    o.description.clone(),
                ));
            }
        }
        found
    }
}

/// An order for the mixer, as the `do` of a message.
pub fn order(target: &Target, change: Value) -> Value {
    let mut order = serde_json::to_value(target).unwrap_or_default();
    if let (Some(order), Value::Object(change)) = (order.as_object_mut(), change) {
        order.extend(change);
    }
    order
}

/// The connection to Pipedeck, for sending orders on.
#[derive(Clone, Default)]
pub struct Mixer {
    stream: Arc<Mutex<Option<UnixStream>>>,
}

impl Mixer {
    /// Follow the mixer on a thread of its own, telling `changed` its state
    /// after every change, and `None` while it is not running.
    pub fn start(changed: Sender<Option<View>>) -> Self {
        let mixer = Mixer::default();
        let stream = mixer.stream.clone();
        std::thread::Builder::new()
            .name("pipedeck".into())
            .spawn(move || loop {
                if let Err(e) = follow(&stream, &changed) {
                    if e.kind() != std::io::ErrorKind::NotFound
                        && e.kind() != std::io::ErrorKind::ConnectionRefused
                    {
                        eprintln!("pipedeck: {e}");
                    }
                }
                if let Ok(mut stream) = stream.lock() {
                    *stream = None;
                }
                if changed.send(None).is_err() {
                    return;
                }
                std::thread::sleep(RETRY);
            })
            .expect("a thread for the mixer");
        mixer
    }

    /// Send an order. Says whether it went.
    pub fn send(&self, order: Value) -> bool {
        let mut line = json!({ "do": order }).to_string();
        line.push('\n');
        let Ok(mut stream) = self.stream.lock() else {
            return false;
        };
        match stream.as_mut() {
            Some(stream) => stream.write_all(line.as_bytes()).is_ok(),
            None => false,
        }
    }
}

fn follow(
    stream: &Arc<Mutex<Option<UnixStream>>>,
    changed: &Sender<Option<View>>,
) -> std::io::Result<()> {
    let mut socket = UnixStream::connect(socket_path())?;
    socket.write_all(b"{\"subscribe\": true}\n")?;
    if let Ok(mut stream) = stream.lock() {
        *stream = Some(socket.try_clone()?);
    }
    for line in BufReader::new(socket).lines() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(state) = message.get("state") {
            // Null until the mixer is up.
            let view = match serde_json::from_value::<Option<View>>(state.clone()) {
                Ok(view) => view,
                Err(e) => {
                    eprintln!("pipedeck: a state not understood: {e}");
                    continue;
                }
            };
            if changed.send(view).is_err() {
                return Ok(());
            }
        } else if let Some(error) = message.get("error") {
            eprintln!("pipedeck: {error}");
        }
    }
    Ok(())
}

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pipedeck")
        .join("control.sock")
}
