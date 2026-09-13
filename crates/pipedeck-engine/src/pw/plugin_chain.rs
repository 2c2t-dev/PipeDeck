//! Running hosted plug-ins inside the graph.
//!
//! A plug-in is not a PipeWire filter, so the mixer carries it: a capture
//! stream reads the channel, the plug-ins process the block, and a playback
//! stream hands the result on. That is the same pair of streams PipeWire's
//! own filter chain is made of, so it costs no more than one of those.
//!
//! The two streams are called on the real-time thread. Between them sits a
//! ring the writer and the reader reach without locking, and nothing else
//! crosses: the plug-ins themselves live with the capture side and are only
//! ever touched there.

use std::cell::{RefCell, UnsafeCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use libspa::param::audio::{AudioFormat, AudioInfoRaw};
use libspa::param::ParamType;
use libspa::pod::serialize::PodSerializer;
use libspa::pod::{Object, Pod, Value};
use libspa::utils::{Direction, SpaTypes};
use pipewire::core::CoreRc;
use pipewire::properties::properties;
use pipewire::stream::{StreamFlags, StreamListener, StreamRc};

use crate::error::EngineError;
use crate::stereotool;
use crate::types::{Effect, EffectKind};
use crate::vst3::{host::CHANNELS, Instance, Plugin};

use super::loopback::AUDIO_POSITION;

/// Frames the ring holds per channel. A handful of quanta, so a late reader
/// finds the block rather than a gap, and no more, so nothing drifts.
const RING_FRAMES: usize = 8192;
/// The longest block the plug-ins are opened for.
const MAX_BLOCK: usize = 2048;

/// A ring between the two streams.
///
/// One writer, one reader, both on the real-time thread, so the indices are
/// atomic and the data is left unguarded on purpose: a lock here would be
/// the one thing that must not happen in that callback.
struct Ring {
    samples: UnsafeCell<Vec<f32>>,
    write: AtomicUsize,
    read: AtomicUsize,
}

// SAFETY: the two streams touch it from the same real-time thread, one
// writing and one reading, and the indices order every access.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    fn new() -> Self {
        Self {
            samples: UnsafeCell::new(vec![0.0; RING_FRAMES * CHANNELS]),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }

    /// Write one block, interleaved. A block that would overrun the reader
    /// is dropped whole: half a block heard is worse than one missed.
    fn write(&self, channels: &[&mut [f32]], frames: usize) {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if write.wrapping_sub(read) + frames > RING_FRAMES {
            return;
        }
        // SAFETY: only this side writes, and only inside the span the reader
        // has already passed.
        let samples = unsafe { &mut *self.samples.get() };
        for frame in 0..frames {
            let slot = ((write + frame) % RING_FRAMES) * CHANNELS;
            for (channel, buffer) in channels.iter().enumerate().take(CHANNELS) {
                samples[slot + channel] = buffer[frame];
            }
        }
        self.write.store(write + frames, Ordering::Release);
    }

    /// Read one block into planar buffers, filling with silence what is not
    /// there yet.
    fn read(&self, channels: &mut [&mut [f32]], frames: usize) {
        let read = self.read.load(Ordering::Relaxed);
        let available = self.write.load(Ordering::Acquire).wrapping_sub(read);
        let taken = frames.min(available);
        // SAFETY: only this side reads, and only what the writer has
        // published.
        let samples = unsafe { &*self.samples.get() };
        for frame in 0..frames {
            for (channel, buffer) in channels.iter_mut().enumerate().take(CHANNELS) {
                buffer[frame] = if frame < taken {
                    samples[((read + frame) % RING_FRAMES) * CHANNELS + channel]
                } else {
                    0.0
                };
            }
        }
        self.read.store(read + taken, Ordering::Release);
    }
}

/// One plug-in the mixer was asked to run, resolved against what is
/// installed.
#[derive(Debug, Clone)]
pub enum Request {
    Vst3(Plugin),
    /// Thimeo's Stereo Tool, with the preset it was given and the licence
    /// key the settings hold.
    StereoTool {
        preset: Option<PathBuf>,
        license: Option<String>,
    },
}

impl Request {
    /// What the user asked for, when it is something the mixer hosts.
    ///
    /// `installed` is the VST3 scan; a plug-in that has been uninstalled
    /// since it was chosen resolves to nothing and is reported.
    pub fn resolve(effect: &Effect, installed: &[Plugin], license: Option<&str>) -> Option<Self> {
        match effect.kind {
            EffectKind::Vst3 => {
                let found = installed
                    .iter()
                    .find(|plugin| plugin.class_id == effect.label)
                    .cloned();
                if found.is_none() {
                    log::error!("{} is not installed any more", effect.name);
                }
                found.map(Request::Vst3)
            }
            EffectKind::StereoTool => Some(Request::StereoTool {
                preset: effect.preset().map(PathBuf::from),
                license: license.map(str::to_owned),
            }),
            _ => None,
        }
    }
}

/// One stage of the chain, whatever hosts it.
#[allow(clippy::large_enum_variant)]
enum Processor {
    Vst3(Instance),
    StereoTool(stereotool::Instance),
}

impl Processor {
    fn open(request: &Request, rate: f64, max_block: usize) -> Result<Self, String> {
        match request {
            Request::Vst3(plugin) => Instance::open(plugin, rate, max_block).map(Processor::Vst3),
            Request::StereoTool { preset, license } => {
                stereotool::Instance::with_block(preset.as_deref(), license.as_deref(), max_block)
                    .map(Processor::StereoTool)
            }
        }
    }

    fn name(&self) -> &str {
        match self {
            Processor::Vst3(instance) => instance.name(),
            Processor::StereoTool(instance) => instance.name(),
        }
    }

    /// Run one block, in place. An error means the block did not go through,
    /// and the chain drops it rather than passing something half treated on.
    fn process(&mut self, channels: &mut [&mut [f32]]) -> Result<(), ()> {
        match self {
            Processor::Vst3(instance) => instance.process(channels).map_err(|_| ()),
            Processor::StereoTool(instance) => instance.process(channels).map_err(|_| ()),
        }
    }
}

/// What the capture side needs while it runs.
struct Processing {
    plugins: Vec<Processor>,
    ring: Arc<Ring>,
    /// One buffer per channel, allocated once: the real-time thread must not
    /// ask for memory.
    scratch: Vec<Vec<f32>>,
}

/// What the playback side needs.
struct Playing {
    ring: Arc<Ring>,
}

/// A channel's or a mix's plug-ins, running in the graph.
pub struct PluginChain {
    // Field order matters: the listeners go before the streams they hang on.
    _playback_listener: StreamListener<Playing>,
    _capture_listener: StreamListener<Processing>,
    _playback: StreamRc,
    _capture: StreamRc,
    /// The processors that have an interface of their own, one slot per
    /// plug-in the chain was asked for, and the windows currently up.
    ///
    /// The audio thread runs the processor while this side opens its window,
    /// which is how the vendor's own plug-in works and why the processor is
    /// shared rather than owned by either.
    windows: Vec<Option<Arc<stereotool::Handle>>>,
    open: RefCell<Vec<Option<stereotool::Window>>>,
}

impl PluginChain {
    /// Put the interface of one plug-in of this chain on the screen, or take
    /// it away.
    ///
    /// `index` counts the plug-ins of the chain, not the effects: the caller
    /// knows which effect it means and how many of the ones before it the
    /// mixer hosts.
    pub fn set_window(&self, index: usize, open: bool) -> Result<(), String> {
        if !open {
            // Dropping it takes the window off the screen and gives back
            // what drawing it costs; the processor carries on either way.
            if let Some(slot) = self.open.borrow_mut().get_mut(index) {
                *slot = None;
            }
            return Ok(());
        }
        let Some(handle) = self.windows.get(index).and_then(|slot| slot.clone()) else {
            return Err("that effect has no window of its own".into());
        };
        let mut windows = self.open.borrow_mut();
        match windows.get_mut(index).and_then(|slot| slot.as_ref()) {
            // Already up: bring it to the front rather than making a second.
            Some(window) => window.raise(),
            None => {
                let window = stereotool::Window::open(handle)?;
                windows[index] = Some(window);
            }
        }
        Ok(())
    }

    /// Close the windows whose close button has been pressed.
    ///
    /// The request waits on the window's own connection until someone reads
    /// it, so this is asked on the engine's tick rather than from a thread
    /// of its own.
    pub fn poll_windows(&self) {
        for slot in self.open.borrow_mut().iter_mut() {
            if slot.as_ref().is_some_and(|window| window.close_requested()) {
                *slot = None;
            }
        }
    }
}

impl PluginChain {
    /// Open the plug-ins and put them between two nodes of the graph.
    ///
    /// `node` is the base `node.name` of the pair of streams, `owner` what
    /// the window calls the object they belong to, `from` the node the
    /// plug-ins read and `into` the sink they play into. Both ends are node
    /// ids rather than names, which is what makes a channel and a mix the
    /// same object here although the audio runs through them the other way
    /// round. `from_sink` says whether the first is a sink, whose monitor is
    /// what gets captured, rather than a microphone to read straight.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        core: &CoreRc,
        node: &str,
        owner: &str,
        plugins: &[Request],
        from: u32,
        from_sink: bool,
        into: u32,
        latency: &str,
    ) -> Result<Self, EngineError> {
        let rate = 48_000.0;
        let mut opened = Vec::new();
        let mut names = Vec::new();
        // One slot per plug-in asked for, whether or not it opened, so the
        // caller can point at "the third effect" and be understood.
        let mut windows: Vec<Option<Arc<stereotool::Handle>>> = Vec::new();
        for plugin in plugins {
            match Processor::open(plugin, rate, MAX_BLOCK) {
                Ok(instance) => {
                    names.push(instance.name().to_owned());
                    windows.push(match &instance {
                        Processor::StereoTool(stereotool) => Some(stereotool.handle()),
                        Processor::Vst3(_) => None,
                    });
                    opened.push(instance);
                }
                Err(e) => {
                    log::error!("cannot open a plug-in of {owner}: {e}");
                    windows.push(None);
                }
            }
        }
        if opened.is_empty() {
            return Err(EngineError::NoPlugin);
        }

        let ring = Arc::new(Ring::new());
        let common = |name: String, description: String| {
            properties! {
                *pipewire::keys::MEDIA_TYPE => "Audio",
                *pipewire::keys::NODE_NAME => name,
                *pipewire::keys::NODE_DESCRIPTION => description,
                "audio.position" => AUDIO_POSITION,
                *pipewire::keys::NODE_LATENCY => latency,
                *pipewire::keys::NODE_DONT_RECONNECT => "true",
                "pipedeck.instance" => super::instance(),
                "state.restore-props" => "false",
                "state.restore-target" => "false",
            }
        };

        let mut capture_props = common(
            format!("{node}.in"),
            format!("Pipedeck: {owner} plug-ins in"),
        );
        capture_props.insert(*pipewire::keys::MEDIA_CATEGORY, "Capture");
        capture_props.insert(*pipewire::keys::MEDIA_CLASS, "Stream/Input/Audio/Internal");
        if from_sink {
            capture_props.insert(*pipewire::keys::STREAM_CAPTURE_SINK, "true");
        }

        let mut playback_props = common(
            format!("{node}.out"),
            format!("Pipedeck: {owner} plug-ins out"),
        );
        playback_props.insert(*pipewire::keys::MEDIA_CATEGORY, "Playback");

        let capture = StreamRc::new(core.clone(), "pipedeck-plugins-in", capture_props)?;
        let playback = StreamRc::new(core.clone(), "pipedeck-plugins-out", playback_props)?;

        let capture_listener = capture
            .add_local_listener_with_user_data(Processing {
                plugins: opened,
                ring: ring.clone(),
                scratch: vec![vec![0.0; MAX_BLOCK]; CHANNELS],
            })
            .process(|stream, state| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let datas = buffer.datas_mut();
                if datas.len() < CHANNELS {
                    return;
                }
                let frames = datas[0].chunk().size() as usize / std::mem::size_of::<f32>();
                let frames = frames.min(MAX_BLOCK);

                if frames == 0 {
                    return;
                }

                for (channel, data) in datas.iter_mut().enumerate().take(CHANNELS) {
                    let Some(bytes) = data.data() else {
                        return;
                    };
                    let plane = &mut state.scratch[channel];
                    for (frame, sample) in plane.iter_mut().take(frames).enumerate() {
                        let start = frame * std::mem::size_of::<f32>();
                        *sample = match bytes[start..start + 4].try_into() {
                            Ok(bytes) => f32::from_le_bytes(bytes),
                            Err(_) => 0.0,
                        };
                    }
                }

                let (first, rest) = state.scratch.split_at_mut(1);
                let mut block: Vec<&mut [f32]> = vec![&mut first[0][..frames]];
                for plane in rest.iter_mut().take(CHANNELS - 1) {
                    block.push(&mut plane[..frames]);
                }
                for plugin in &mut state.plugins {
                    if plugin.process(&mut block).is_err() {
                        return;
                    }
                }
                state.ring.write(&block, frames);
            })
            .register()?;

        let playback_listener = playback
            .add_local_listener_with_user_data(Playing { ring })
            .process(|stream, state| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let requested = buffer.requested() as usize;
                let datas = buffer.datas_mut();

                if datas.len() < CHANNELS {
                    return;
                }
                let frames = requested.min(MAX_BLOCK);
                if frames == 0 {
                    return;
                }

                let mut planes: Vec<Vec<f32>> = vec![vec![0.0; frames]; CHANNELS];
                {
                    let mut block: Vec<&mut [f32]> =
                        planes.iter_mut().map(|plane| &mut plane[..]).collect();
                    state.ring.read(&mut block, frames);
                }
                for (channel, data) in datas.iter_mut().enumerate().take(CHANNELS) {
                    let stride = std::mem::size_of::<f32>();
                    let wrote = frames * stride;
                    if let Some(bytes) = data.data() {
                        for frame in 0..frames {
                            let sample = planes[channel][frame].to_le_bytes();
                            bytes[frame * stride..frame * stride + stride].copy_from_slice(&sample);
                        }
                    }
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = stride as i32;
                    *chunk.size_mut() = wrote as u32;
                }
            })
            .register()?;

        let format = format_param();
        let mut params = [Pod::from_bytes(&format).expect("the format pod is valid")];
        // The nodes are named by the ids the server gave them rather than by
        // their names: another mixer on the same graph carries the same
        // names, and a chain pointed by name lands on its channel as easily
        // as on ours, which is silent from here.
        capture.connect(
            Direction::Input,
            Some(from),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )?;
        let mut params = [Pod::from_bytes(&format).expect("the format pod is valid")];
        playback.connect(
            Direction::Output,
            Some(into),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        log::info!(
            "{owner} runs {} plug-in(s): {}",
            names.len(),
            names.join(", ")
        );
        let open = RefCell::new((0..windows.len()).map(|_| None).collect());
        Ok(Self {
            _playback_listener: playback_listener,
            _capture_listener: capture_listener,
            _playback: playback,
            _capture: capture,
            windows,
            open,
        })
    }
}

/// The format both streams accept: whatever the graph runs, in floats.
///
/// The bytes are handed back with the pod because a pod is a view into them
/// and must not outlive them.
fn format_param() -> Vec<u8> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32P);
    let object = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .expect("serializing an in-memory pod cannot fail")
        .0
        .into_inner()
}
