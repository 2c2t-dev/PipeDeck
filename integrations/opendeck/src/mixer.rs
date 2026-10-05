//! Pipedeck's control socket, followed for as long as the plugin runs.
//!
//! One connection, opened again whenever Pipedeck comes back: it subscribes
//! to the mixer's state, which comes as a line of JSON after every change,
//! and carries the deck's orders the other way. The protocol is described
//! in `crates/pipedeck-engine/src/control.rs`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
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
    /// The applications playing now.
    #[serde(default)]
    pub apps: Vec<App>,
    /// The one playing in the window that has the focus.
    #[serde(default)]
    pub focused: Option<App>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct App {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EffectState {
    pub name: String,
    pub bypassed: bool,
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
    #[serde(default)]
    pub effects: Vec<EffectState>,
    /// The applications put on it, by their keys.
    #[serde(default)]
    pub apps: Vec<String>,
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
    /// Their picture, a PNG file on this machine.
    #[serde(default)]
    pub avatar: Option<String>,
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
    /// The mix a channel's level is taken in, when it is not its own.
    pub within: Option<Mix>,
    /// A person's picture, a PNG file.
    pub avatar: Option<String>,
}

impl View {
    /// The person at a place in the call, from 0, and the channel carrying
    /// it: the people in the order the mixer first met them, closing up
    /// when someone leaves.
    pub fn in_call(&self, place: usize) -> Option<(u32, String)> {
        self.channels
            .iter()
            .flat_map(|c| c.voices.iter().map(move |v| (c.id, v.user.clone())))
            .nth(place)
    }

    /// How many people are in the call.
    pub fn people_in_call(&self) -> usize {
        self.channels.iter().map(|c| c.voices.len()).sum()
    }

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
                within: None,
                avatar: None,
            }),
            Target::Mix { id } => mix(*id).map(|m| Found {
                name: m.name.clone(),
                icon: m.icon.clone(),
                input: false,
                mix: true,
                volume: m.volume,
                muted: m.muted,
                listening: m.listening,
                within: None,
                avatar: None,
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
                    name: from.name.clone(),
                    icon: from.icon.clone(),
                    input: from.input,
                    mix: false,
                    volume: cell.volume,
                    muted: cell.muted,
                    listening: false,
                    within: Some(into.clone()),
                    avatar: None,
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
                    within: None,
                    avatar: voice.avatar.clone(),
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
                    within: None,
                    avatar: None,
                })
            }
        }
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

/// What the mixer says.
#[derive(Debug, Clone, PartialEq)]
pub enum News {
    /// Where things stand, or `None` while it is not running.
    State(Option<View>),
    /// What the meters read.
    Levels(Peaks),
}

/// The loudest each channel, mix and person of a call got lately, as a
/// linear amplitude.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Peaks {
    pub channels: Vec<(u32, f32)>,
    pub mixes: Vec<(u32, f32)>,
    pub voices: Vec<(u32, String, f32)>,
}

impl Peaks {
    /// What a target's meter reads: a channel's own, or its level in a
    /// mix, which is measured before the mix; a person's; a mix's.
    pub fn of(&self, target: &Target) -> Option<f32> {
        match target {
            Target::Channel { id } | Target::Cell { channel: id, .. } => {
                self.channels.iter().find(|(c, _)| c == id).map(|(_, p)| *p)
            }
            Target::Mix { id } => self.mixes.iter().find(|(m, _)| m == id).map(|(_, p)| *p),
            Target::Voice { channel, user } => self
                .voices
                .iter()
                .find(|(c, u, _)| c == channel && u == user)
                .map(|(_, _, p)| *p),
            Target::Output { .. } => None,
        }
    }
}

/// The connection to Pipedeck, for sending orders on.
#[derive(Clone, Default)]
pub struct Mixer {
    stream: Arc<Mutex<Option<UnixStream>>>,
}

impl Mixer {
    /// Follow the mixer on a thread of its own, telling `changed` its state
    /// after every change, and `None` while it is not running, and what its
    /// meters read.
    pub fn start(changed: Sender<News>) -> Self {
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
                if changed.send(News::State(None)).is_err() {
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

fn follow(stream: &Arc<Mutex<Option<UnixStream>>>, changed: &Sender<News>) -> std::io::Result<()> {
    let mut socket = connect()?;
    socket.write_all(b"{\"subscribe\": true}\n{\"meters\": true}\n")?;
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
            if changed.send(News::State(view)).is_err() {
                return Ok(());
            }
        } else if let Some(levels) = message.get("levels") {
            let Ok(peaks) = serde_json::from_value::<Peaks>(levels.clone()) else {
                continue;
            };
            if changed.send(News::Levels(peaks)).is_err() {
                return Ok(());
            }
        } else if let Some(error) = message.get("error") {
            eprintln!("pipedeck: {error}");
        }
    }
    Ok(())
}

/// Ask the mixer once where things stand. `None` while it is not up yet.
pub fn fetch() -> std::io::Result<Option<View>> {
    fetch_from(&socket_path())
}

/// Ask the mixer behind the socket at `path` where things stand.
fn fetch_from(path: &Path) -> std::io::Result<Option<View>> {
    let mut socket = connect_to(path)?;
    socket.write_all(b"{\"get\": \"state\"}\n")?;
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line)?;
    let message: Value = serde_json::from_str(&line)?;
    Ok(serde_json::from_value(message["state"].clone())?)
}

/// Reach the mixer, if its socket is where Pipedeck makes it: in a folder
/// that is the user's own and closed to others. Without a runtime
/// directory it is in the shared temporary one, where a folder that is not
/// was made by someone else, who could pose as the mixer.
fn connect() -> std::io::Result<UnixStream> {
    connect_to(&socket_path())
}

/// Reach the mixer behind the socket at `path`, on the same condition.
fn connect_to(path: &Path) -> std::io::Result<UnixStream> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let folder = std::fs::symlink_metadata(path.parent().unwrap_or(path))?;
    // The process's own folder belongs to whoever runs it.
    let me = std::fs::metadata("/proc/self")?.uid();
    if !folder.is_dir() || folder.uid() != me || folder.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} is not in a folder of this user's", path.display()),
        ));
    }
    UnixStream::connect(path)
}

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pipedeck")
        .join("control.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::os::unix::net::UnixListener;

    /// A folder of the test's own, with the given mode, and the socket's
    /// path in it.
    fn folder(name: &str, mode: u32) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("pipedeck-opendeck-{}-{name}", std::process::id()))
            .join("pipedeck");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
        dir.join("control.sock")
    }

    fn clean(path: &Path) {
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    /// A mixer that answers the first line it is told with `answer`.
    fn mixer(path: &Path, answer: &'static str) -> std::thread::JoinHandle<String> {
        let listener = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut asked = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut asked)
                .unwrap();
            (&stream).write_all(answer.as_bytes()).unwrap();
            asked
        })
    }

    #[test]
    fn the_mixer_is_asked_where_things_stand() {
        let path = folder("asked", 0o700);
        let answering = mixer(&path, "{\"state\": null}\n");
        assert!(matches!(fetch_from(&path), Ok(None)));
        assert_eq!(answering.join().unwrap(), "{\"get\": \"state\"}\n");
        clean(&path);
    }

    #[test]
    fn an_answer_not_understood_is_said_so() {
        let path = folder("unclear", 0o700);
        let answering = mixer(&path, "not json\n");
        assert!(fetch_from(&path).is_err());
        answering.join().unwrap();
        clean(&path);
    }

    #[test]
    fn a_socket_in_a_folder_open_to_others_is_not_trusted() {
        let path = folder("open", 0o755);
        let _listener = UnixListener::bind(&path).unwrap();
        let refused = connect_to(&path).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
        clean(&path);
    }

    #[test]
    fn no_folder_no_mixer() {
        let path = std::env::temp_dir().join("pipedeck-opendeck-none/pipedeck/control.sock");
        assert!(connect_to(&path).is_err());
    }
}
