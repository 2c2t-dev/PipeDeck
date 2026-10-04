//! A local socket other programs tell the mixer things through, and hear
//! from it on.
//!
//! One JSON object to a line, each way. Who is in the call Vesktop is in,
//! from Pipedeck's plugin for it:
//!
//! ```json
//! {"call": [{"id": "235088799074484224", "name": "Alice"}]}
//! ```
//!
//! answered with the name of the sink made for each of them, which is what
//! the plugin looks their output up by:
//!
//! ```json
//! {"labels": {"235088799074484224": "Alice (Discord)"}}
//! ```
//!
//! A connection that has said who is in a call and closes ends the call:
//! Vesktop has gone, and so have the voices it was playing.
//!
//! Where things stand, for a remote control such as a Stream Deck: asked
//! once with `{"get": "state"}`, or as it changes with
//! `{"subscribe": true}`, which answers at once and again after every
//! change. Either way it comes as `{"state": {...}}`: the channels, the
//! mixes, the cells joining them, and the device listened on, every level a
//! fader position from 0 to 1.
//!
//! And what to do, answered `{"ok": true}` or `{"error": "..."}`:
//!
//! ```json
//! {"do": {"what": "channel", "id": 2, "mute": "toggle"}}
//! {"do": {"what": "mix", "id": 1, "volume": 0.8}}
//! {"do": {"what": "cell", "channel": 2, "mix": 1, "nudge": -0.05}}
//! {"do": {"what": "voice", "channel": 3, "user": "2350…", "mute": true}}
//! {"do": {"what": "listen", "device": "alsa_output.usb-…"}}
//! ```
//!
//! `mute` takes true, false or "toggle"; `volume` sets a fader, `nudge`
//! moves it by so much. Several nudges in a row add up, each counted from
//! where the last one left the fader, whatever the engine has said back yet.
//!
//! The socket is `$XDG_RUNTIME_DIR/pipedeck/control.sock`, in a folder
//! only the user can reach. A second mixer finds it answering and leaves it
//! to the first.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};

use pipewire as pw;
use serde::{Deserialize, Serialize};

use crate::engine::{Command, Event, StateSnapshot, Watch};
use crate::types::{voice_labels, CallMember, Device, MixId, SourceId};

/// Where the socket is.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("pipedeck").join("control.sock")
}

#[derive(Debug, Default, Deserialize)]
struct Message {
    call: Option<Vec<CallMember>>,
    get: Option<String>,
    subscribe: Option<bool>,
    #[serde(rename = "do")]
    action: Option<Action>,
}

#[derive(Debug, Serialize)]
struct Answer {
    labels: HashMap<String, String>,
}

/// Mute on, off, or the other way from how it is.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(untagged)]
enum Mute {
    Set(bool),
    Toggle(Toggle),
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Toggle {
    Toggle,
}

/// What a level is to do: be muted or not, be set, or be moved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
struct Change {
    mute: Option<Mute>,
    volume: Option<f32>,
    nudge: Option<f32>,
}

/// What a remote control asks for.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "what", rename_all = "lowercase")]
enum Action {
    Channel {
        id: u32,
        #[serde(flatten)]
        change: Change,
    },
    Mix {
        id: u32,
        #[serde(flatten)]
        change: Change,
    },
    Cell {
        channel: u32,
        mix: u32,
        #[serde(flatten)]
        change: Change,
    },
    Voice {
        channel: u32,
        user: String,
        #[serde(flatten)]
        change: Change,
    },
    Listen {
        device: String,
    },
}

/// Where things stand, as a remote control is told.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct View {
    pub channels: Vec<ChannelView>,
    pub mixes: Vec<LevelView>,
    pub cells: Vec<CellView>,
    /// The device listened on, and the ones that could be.
    pub listen: Option<String>,
    pub outputs: Vec<DeviceView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChannelView {
    pub id: u32,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    /// Bound to a microphone, which has no level of its own here.
    pub input: bool,
    /// The people of the call it carries, when it does.
    pub voices: Vec<VoiceView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LevelView {
    pub id: u32,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellView {
    pub channel: u32,
    pub mix: u32,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceView {
    pub user: String,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeviceView {
    pub name: String,
    pub description: String,
}

/// The mixer as the socket knows it: what the engine last said, with what
/// the socket's own clients have asked since laid over it.
#[derive(Default)]
struct Model {
    state: Option<StateSnapshot>,
    outputs: Vec<Device>,
    /// The clients that asked to hear of every change.
    subscribers: Vec<mpsc::Sender<()>>,
}

impl Model {
    /// Take in what the engine said.
    fn hear(&mut self, event: Event) {
        match event {
            Event::State(state) => self.state = Some(state),
            Event::Devices { outputs, .. } => self.outputs = outputs,
            Event::SourceChanged { id, state } => {
                if let Some(source) = self.source_mut(id.0) {
                    source.set_state(state);
                }
            }
            Event::MixChanged { id, state } => {
                if let Some(mix) = self
                    .state
                    .as_mut()
                    .and_then(|s| s.mixes.iter_mut().find(|mix| mix.id == id))
                {
                    mix.set_state(state);
                }
            }
            Event::LinkChanged { source, mix, state } => {
                if let Some(link) = self.state.as_mut().and_then(|s| {
                    s.links
                        .iter_mut()
                        .find(|link| link.source == source && link.mix == mix)
                }) {
                    link.gain = state.gain;
                    link.muted = state.muted;
                }
            }
            _ => return,
        }
        self.tell();
    }

    /// Tell every subscriber something changed.
    fn tell(&mut self) {
        self.subscribers
            .retain(|subscriber| subscriber.send(()).is_ok());
    }

    fn source_mut(&mut self, id: u32) -> Option<&mut crate::types::SourceConfig> {
        self.state
            .as_mut()?
            .sources
            .iter_mut()
            .find(|source| source.id.0 == id)
    }

    fn view(&self) -> Option<View> {
        let state = self.state.as_ref()?;
        Some(View {
            channels: state
                .sources
                .iter()
                .map(|source| ChannelView {
                    id: source.id.0,
                    name: source.name.clone(),
                    volume: source.gain,
                    muted: source.muted,
                    input: source.is_input(),
                    voices: source
                        .voices
                        .iter()
                        .filter(|voice| voice.present)
                        .map(|voice| VoiceView {
                            user: voice.id.clone(),
                            name: voice.name.clone(),
                            volume: voice.gain,
                            muted: voice.muted,
                        })
                        .collect(),
                })
                .collect(),
            mixes: state
                .mixes
                .iter()
                .map(|mix| LevelView {
                    id: mix.id.0,
                    name: mix.name.clone(),
                    volume: mix.gain,
                    muted: mix.muted,
                })
                .collect(),
            cells: state
                .links
                .iter()
                .map(|link| CellView {
                    channel: link.source.0,
                    mix: link.mix.0,
                    volume: link.gain,
                    muted: link.muted,
                })
                .collect(),
            listen: state.listen_device.clone(),
            outputs: self
                .outputs
                .iter()
                .map(|device| DeviceView {
                    name: device.name.clone(),
                    description: device.description.clone(),
                })
                .collect(),
        })
    }

    /// What an action comes to: the commands for the engine, with the
    /// model moved on as the engine will be, so the next action counts from
    /// there rather than from what the engine has not said back yet.
    fn act(&mut self, action: Action) -> Result<Vec<Command>, String> {
        let state = self.state.as_mut().ok_or("the mixer is not up yet")?;
        let mut commands = Vec::new();
        match action {
            Action::Channel { id, change } => {
                let source = state
                    .sources
                    .iter_mut()
                    .find(|source| source.id.0 == id)
                    .ok_or(format!("no channel {id}"))?;
                let (gain, muted) = apply(change, source.gain, source.muted);
                let id = SourceId(id);
                if gain != source.gain {
                    source.gain = gain;
                    commands.push(Command::SetSourceGain { id, gain });
                }
                if muted != source.muted {
                    source.muted = muted;
                    commands.push(Command::SetSourceMute { id, muted });
                }
            }
            Action::Mix { id, change } => {
                let mix = state
                    .mixes
                    .iter_mut()
                    .find(|mix| mix.id.0 == id)
                    .ok_or(format!("no mix {id}"))?;
                let (gain, muted) = apply(change, mix.gain, mix.muted);
                let id = MixId(id);
                if gain != mix.gain {
                    mix.gain = gain;
                    commands.push(Command::SetMixGain { id, gain });
                }
                if muted != mix.muted {
                    mix.muted = muted;
                    commands.push(Command::SetMixMute { id, muted });
                }
            }
            Action::Cell {
                channel,
                mix,
                change,
            } => {
                let link = state
                    .links
                    .iter_mut()
                    .find(|link| link.source.0 == channel && link.mix.0 == mix)
                    .ok_or(format!("channel {channel} does not feed mix {mix}"))?;
                let (gain, muted) = apply(change, link.gain, link.muted);
                let (source, mix) = (SourceId(channel), MixId(mix));
                if gain != link.gain {
                    link.gain = gain;
                    commands.push(Command::SetLinkGain { source, mix, gain });
                }
                if muted != link.muted {
                    link.muted = muted;
                    commands.push(Command::SetLinkMute { source, mix, muted });
                }
            }
            Action::Voice {
                channel,
                user,
                change,
            } => {
                let voice = state
                    .sources
                    .iter_mut()
                    .find(|source| source.id.0 == channel)
                    .and_then(|source| source.voices.iter_mut().find(|voice| voice.id == user))
                    .ok_or(format!("nobody called {user} on channel {channel}"))?;
                let (gain, muted) = apply(change, voice.gain, voice.muted);
                let id = SourceId(channel);
                if gain != voice.gain {
                    voice.gain = gain;
                    commands.push(Command::SetVoiceGain {
                        id,
                        user: user.clone(),
                        gain,
                    });
                }
                if muted != voice.muted {
                    voice.muted = muted;
                    commands.push(Command::SetVoiceMute { id, user, muted });
                }
            }
            Action::Listen { device } => {
                state.listen_device = Some(device.clone());
                commands.push(Command::SetListenDevice(device));
            }
        }
        Ok(commands)
    }
}

/// A level and a mute after a change: set, moved and kept in range, muted
/// or the other way.
fn apply(change: Change, gain: f32, muted: bool) -> (f32, bool) {
    let mut gain = gain;
    if let Some(volume) = change.volume {
        gain = volume;
    }
    if let Some(nudge) = change.nudge {
        gain += nudge;
    }
    let muted = match change.mute {
        Some(Mute::Set(muted)) => muted,
        Some(Mute::Toggle(_)) => !muted,
        None => muted,
    };
    (gain.clamp(0.0, 1.0), muted)
}

/// Listen on the socket, on a thread of its own, handing what comes in to
/// the engine and telling clients what the engine says. Does nothing when
/// another mixer already listens.
pub fn listen(commands: pw::channel::Sender<Command>, watch: Watch) {
    let path = socket_path();
    // The folder first: a socket answering in a folder that is not the
    // user's is not another mixer of theirs, and is not to be left to.
    if let Some(dir) = path.parent() {
        if let Err(e) = private_dir(dir) {
            log::error!("not listening on {}: {e}", path.display());
            return;
        }
    }
    if UnixStream::connect(&path).is_ok() {
        log::info!("another mixer answers on {}", path.display());
        return;
    }
    // Nobody answered, so a file left there is from a mixer that is gone.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            log::error!("cannot listen on {}: {e}", path.display());
            return;
        }
    };
    log::info!("listening on {}", path.display());

    // What the engine says, kept for the clients.
    let model = Arc::new(Mutex::new(Model::default()));
    let heard = watch.listen();
    let spawned = std::thread::Builder::new()
        .name("pipedeck-watch".into())
        .spawn({
            let model = model.clone();
            move || {
                for event in heard {
                    if let Ok(mut model) = model.lock() {
                        model.hear(event);
                    }
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot follow the engine: {e}");
    }

    let spawned = std::thread::Builder::new()
        .name("pipedeck-control".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let commands = commands.clone();
                        let model = model.clone();
                        let _ = std::thread::Builder::new()
                            .name("pipedeck-client".into())
                            .spawn(move || serve(stream, commands, model));
                    }
                    Err(e) => log::warn!("a client could not connect: {e}"),
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot start listening: {e}");
    }
}

/// Make the socket's folder for the user alone, or check one already
/// there is: under the runtime directory it is anyway, but without one it
/// is in the shared temporary directory, where someone else could have made
/// it first.
fn private_dir(dir: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let me = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != me {
        return Err(format!("{} is not a folder of this user's", dir.display()));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("cannot close {} to others: {e}", dir.display()))?;
    }
    Ok(())
}

/// Write one line to a client, from whichever thread has something to say:
/// the one answering it, or the one telling it of changes.
fn say(writer: &Mutex<UnixStream>, value: &impl Serialize) -> bool {
    let Ok(mut text) = serde_json::to_string(value) else {
        return true;
    };
    text.push('\n');
    writer
        .lock()
        .map(|mut writer| writer.write_all(text.as_bytes()).is_ok())
        .unwrap_or(false)
}

#[derive(Serialize)]
struct StateLine {
    state: Option<View>,
}

#[derive(Serialize)]
struct Done {
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Answer one client until it goes.
fn serve(stream: UnixStream, commands: pw::channel::Sender<Command>, model: Arc<Mutex<Model>>) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let writer = Arc::new(Mutex::new(writer));
    let view = |model: &Arc<Mutex<Model>>| model.lock().ok().and_then(|model| model.view());
    let mut in_call = false;
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Message = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(e) => {
                log::warn!("a client said something unclear: {e}");
                let error = Some(format!("not understood: {e}"));
                if !say(&writer, &Done { ok: None, error }) {
                    break;
                }
                continue;
            }
        };

        if let Some(members) = message.call {
            let labels = voice_labels(&members);
            let answer = Answer {
                labels: members
                    .iter()
                    .map(|member| member.id.clone())
                    .zip(labels)
                    .collect(),
            };
            in_call = !members.is_empty();
            if commands.send(Command::SetCall { members }).is_err() || !say(&writer, &answer) {
                break;
            }
        }

        if message.get.as_deref() == Some("state")
            && !say(
                &writer,
                &StateLine {
                    state: view(&model),
                },
            )
        {
            break;
        }

        if message.subscribe == Some(true) {
            // Told now, and then on a thread of its own after every change,
            // as one line however many changes came meanwhile.
            let (tx, rx) = mpsc::channel();
            if let Ok(mut model) = model.lock() {
                model.subscribers.push(tx);
            }
            let mut told = view(&model);
            if !say(
                &writer,
                &StateLine {
                    state: told.clone(),
                },
            ) {
                break;
            }
            let (writer, model) = (writer.clone(), model.clone());
            let _ = std::thread::Builder::new()
                .name("pipedeck-subscriber".into())
                .spawn(move || {
                    while rx.recv().is_ok() {
                        while rx.try_recv().is_ok() {}
                        // A change made here is told once when made and
                        // again when the engine says it was done.
                        let now = view(&model);
                        if now == told {
                            continue;
                        }
                        if !say(&writer, &StateLine { state: now.clone() }) {
                            break;
                        }
                        told = now;
                    }
                });
        }

        if let Some(action) = message.action {
            let acted = model
                .lock()
                .map_err(|_| "the mixer's state is poisoned".to_owned())
                .and_then(|mut model| {
                    let commands = model.act(action)?;
                    model.tell();
                    Ok(commands)
                });
            let done = match acted {
                Ok(acts) => {
                    for command in acts {
                        if commands.send(command).is_err() {
                            return;
                        }
                    }
                    Done {
                        ok: Some(true),
                        error: None,
                    }
                }
                Err(e) => Done {
                    ok: None,
                    error: Some(e),
                },
            };
            if !say(&writer, &done) {
                break;
            }
        }
    }
    if in_call {
        let _ = commands.send(Command::SetCall {
            members: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::types::{LinkConfig, MixConfig, SourceConfig};

    fn model() -> Model {
        let mut state = StateSnapshot {
            latency: String::new(),
            stereotool_license: None,
            listen_device: None,
            mixes: vec![MixConfig::new(MixId(1), "Stream")],
            sources: vec![SourceConfig::virtual_sink(SourceId(2), "Music")],
            links: vec![LinkConfig::new(SourceId(2), MixId(1))],
        };
        state.sources[0].gain = 0.5;
        Model {
            state: Some(state),
            ..Model::default()
        }
    }

    fn act(model: &mut Model, json: &str) -> Result<Vec<Command>, String> {
        let message: Message = serde_json::from_str(json).expect("a message");
        model.act(message.action.expect("an action"))
    }

    #[test]
    fn a_mute_is_set_or_turned_over() {
        let mut model = model();
        assert_eq!(
            act(
                &mut model,
                r#"{"do":{"what":"channel","id":2,"mute":"toggle"}}"#
            ),
            Ok(vec![Command::SetSourceMute {
                id: SourceId(2),
                muted: true
            }])
        );
        assert_eq!(
            act(
                &mut model,
                r#"{"do":{"what":"channel","id":2,"mute":"toggle"}}"#
            ),
            Ok(vec![Command::SetSourceMute {
                id: SourceId(2),
                muted: false
            }])
        );
        // Set to what it is already: nothing to send.
        assert_eq!(
            act(
                &mut model,
                r#"{"do":{"what":"channel","id":2,"mute":false}}"#
            ),
            Ok(vec![])
        );
    }

    #[test]
    fn nudges_add_up_before_the_engine_answers() {
        let mut model = model();
        for _ in 0..3 {
            act(
                &mut model,
                r#"{"do":{"what":"channel","id":2,"nudge":0.1}}"#,
            )
            .unwrap();
        }
        let view = model.view().expect("a view");
        assert!((view.channels[0].volume - 0.8).abs() < 1e-5);
        // And are kept within the fader.
        act(
            &mut model,
            r#"{"do":{"what":"channel","id":2,"nudge":1.0}}"#,
        )
        .unwrap();
        assert_eq!(model.view().unwrap().channels[0].volume, 1.0);
    }

    #[test]
    fn a_cell_that_is_not_there_is_said_so() {
        let mut model = model();
        assert!(act(
            &mut model,
            r#"{"do":{"what":"cell","channel":2,"mix":1,"volume":0.3}}"#
        )
        .is_ok());
        assert!(act(
            &mut model,
            r#"{"do":{"what":"cell","channel":2,"mix":9,"volume":0.3}}"#
        )
        .unwrap_err()
        .contains("does not feed"));
        assert!(act(&mut model, r#"{"do":{"what":"mix","id":7,"mute":true}}"#).is_err());
    }

    #[test]
    fn what_the_engine_says_comes_through() {
        let mut model = model();
        model.hear(Event::LinkChanged {
            source: SourceId(2),
            mix: MixId(1),
            state: crate::types::ChainState {
                gain: 0.25,
                muted: true,
            },
        });
        let view = model.view().unwrap();
        assert_eq!(view.cells[0].volume, 0.25);
        assert!(view.cells[0].muted);
    }

    #[test]
    fn a_call_is_read_and_answered_with_unique_labels() {
        let message: Message = serde_json::from_str(
            r#"{"call":[{"id":"1","name":"Alice","avatar":"/tmp/a.png"},{"id":"2","name":"Alice"},{"id":"3","name":"Bob"}]}"#,
        )
        .expect("a call");
        let members = message.call.expect("members");
        assert_eq!(members[0].avatar.as_deref(), Some("/tmp/a.png"));
        assert_eq!(
            voice_labels(&members),
            ["Alice (Discord)", "Alice (Discord) 2", "Bob (Discord)"]
        );
    }
}
