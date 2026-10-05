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

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use libspa::param::audio::{AudioFormat, AudioInfoRaw};
use libspa::param::ParamType;
use libspa::pod::serialize::PodSerializer;
use libspa::pod::{Object, Pod, Value};
use libspa::utils::{Direction, SpaTypes};
use pipewire::core::CoreRc;
use pipewire::properties::properties;
use pipewire::stream::{StreamFlags, StreamListener, StreamRc};

use crate::dsp;
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
/// One writer, one reader, with nothing to wait on between them: a lock
/// here would be the one thing that must not happen in those callbacks.
/// The two may run on different threads — PipeWire can give each stream a
/// data loop of its own — so every sample is an atomic, as plain to read and
/// write as a float, and the indices say which ones are ready.
struct Ring {
    /// Interleaved samples, as f32 bits.
    samples: Box<[AtomicU32]>,
    write: AtomicUsize,
    read: AtomicUsize,
}

impl Ring {
    fn new() -> Self {
        Self {
            samples: (0..RING_FRAMES * CHANNELS)
                .map(|_| AtomicU32::new(0))
                .collect(),
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
        // Only inside the span the reader has already passed, and published
        // by the release below.
        for frame in 0..frames {
            let slot = ((write + frame) % RING_FRAMES) * CHANNELS;
            for (channel, buffer) in channels.iter().enumerate().take(CHANNELS) {
                self.samples[slot + channel].store(buffer[frame].to_bits(), Ordering::Relaxed);
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
        // Only what the writer has published, by the acquire above.
        for frame in 0..frames {
            for (channel, buffer) in channels.iter_mut().enumerate().take(CHANNELS) {
                buffer[frame] = if frame < taken {
                    f32::from_bits(
                        self.samples[((read + frame) % RING_FRAMES) * CHANNELS + channel]
                            .load(Ordering::Relaxed),
                    )
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
    /// One of the mixer's own effects, with the settings it shares with
    /// the audio thread.
    Native {
        id: String,
        params: Arc<dsp::Params>,
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
            EffectKind::Native => {
                let spec = dsp::spec(&effect.label);
                if spec.is_none() {
                    log::error!("{} is not an effect this mixer has", effect.label);
                }
                spec.map(|spec| Request::Native {
                    id: effect.label.clone(),
                    params: dsp::Params::new(spec, &effect.controls),
                })
            }
            _ => None,
        }
    }
}

/// One stage of the chain, whatever hosts it.
#[allow(clippy::large_enum_variant)]
enum Processor {
    Vst3(Instance),
    StereoTool(stereotool::Instance),
    Native(String, Box<dyn dsp::Native>),
}

impl Processor {
    fn open(request: &Request, rate: f64, max_block: usize) -> Result<Self, String> {
        match request {
            Request::Vst3(plugin) => Instance::open(plugin, rate, max_block).map(Processor::Vst3),
            Request::StereoTool { preset, license } => {
                stereotool::Instance::with_block(preset.as_deref(), license.as_deref(), max_block)
                    .map(Processor::StereoTool)
            }
            Request::Native { id, params } => {
                let name = dsp::spec(id).map_or(id.as_str(), |spec| spec.name);
                dsp::open(id, params.clone(), CHANNELS)
                    .map(|effect| Processor::Native(name.to_owned(), effect))
                    .ok_or_else(|| format!("{id} is not an effect this mixer has"))
            }
        }
    }

    fn name(&self) -> &str {
        match self {
            Processor::Vst3(instance) => instance.name(),
            Processor::StereoTool(instance) => instance.name(),
            Processor::Native(name, _) => name,
        }
    }

    /// Run one block, in place. An error means the block did not go through,
    /// and the chain drops it rather than passing something half treated on.
    fn process(&mut self, channels: &mut [&mut [f32]]) -> Result<(), ()> {
        match self {
            Processor::Vst3(instance) => instance.process(channels).map_err(|_| ()),
            Processor::StereoTool(instance) => instance.process(channels).map_err(|_| ()),
            Processor::Native(_, effect) => {
                effect.process(channels);
                Ok(())
            }
        }
    }
}

/// What the capture side needs while it runs.
struct Processing {
    /// Each with whether it is bypassed, which the audio thread reads for
    /// every block: switching one off or on costs no gap.
    plugins: Vec<(Processor, Arc<AtomicBool>)>,
    ring: Arc<Ring>,
    /// One buffer per channel, allocated once: the real-time thread must not
    /// ask for memory.
    scratch: Vec<Vec<f32>>,
}

/// What the playback side needs.
struct Playing {
    ring: Arc<Ring>,
    /// One buffer per channel, allocated once, as on the capture side.
    scratch: Vec<Vec<f32>>,
}

/// The first `frames` of each of the two channels' buffers, as a block the
/// plug-ins and the ring take. A fixed array rather than a vector: the
/// real-time thread must not ask for memory, even this little.
fn block(planes: &mut [Vec<f32>], frames: usize) -> Option<[&mut [f32]; CHANNELS]> {
    match planes {
        [left, right] => Some([&mut left[..frames], &mut right[..frames]]),
        _ => None,
    }
}

/// A channel's or a mix's plug-ins, running in the graph.
pub struct PluginChain {
    // Field order matters: the listeners go before the streams they hang on.
    // The streams are disconnected before any of it; see the `Drop` below.
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
    /// The settings of the mixer's own effects, one slot per plug-in the
    /// chain was asked for, shared with the audio thread: writing one here
    /// is the effect taking it.
    params: Vec<Option<(String, Arc<dsp::Params>)>>,
    /// Whether each is bypassed, one slot per plug-in asked for, shared
    /// with the audio thread.
    bypass: Vec<Arc<AtomicBool>>,
}

impl Drop for PluginChain {
    /// Stop the audio thread using the plug-ins before they go.
    ///
    /// The streams run their process callbacks on PipeWire's data thread,
    /// and the plug-ins live in the callbacks' data, which goes with the
    /// listeners. Dropping a listener only unhooks it, without waiting for
    /// a callback running on the other thread: a chain dropped while sound
    /// flows would free plug-ins that are still processing. Disconnecting
    /// a stream takes it off the data thread and waits until it is off, so
    /// both are disconnected first, and the fields go after.
    fn drop(&mut self) {
        let _ = self._capture.disconnect();
        let _ = self._playback.disconnect();
    }
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

    /// Give the plug-in at `index` new settings, while it runs. Only the
    /// mixer's own effects take settings this way; `index` counts the
    /// plug-ins of the chain, as for windows.
    pub fn set_params(&self, index: usize, controls: &[crate::types::Control]) {
        if let Some(Some((id, params))) = self.params.get(index) {
            if let Some(spec) = dsp::spec(id) {
                params.set(spec, controls);
            }
        }
    }

    /// Bypass the plug-in at `index`, or bring it back, while it runs:
    /// the sound goes past it untouched. `index` counts the plug-ins of the
    /// chain, as for windows.
    pub fn set_bypass(&self, index: usize, bypassed: bool) {
        if let Some(flag) = self.bypass.get(index) {
            flag.store(bypassed, Ordering::Relaxed);
        }
    }

    /// The settings, and what it heard, of the mixer's own effect at
    /// `index`, which counts the plug-ins of the chain as for windows.
    pub fn params(&self, index: usize) -> Option<&Arc<dsp::Params>> {
        self.params
            .get(index)
            .and_then(|slot| slot.as_ref())
            .map(|(_, params)| params)
    }

    /// Close the windows whose close button has been pressed.
    ///
    /// The request waits on the window's own connection until someone reads
    /// it, so this is asked on the engine's tick rather than from a thread
    /// of its own.
    pub fn poll_windows(&self) {
        for slot in self.open.borrow_mut().iter_mut() {
            if slot.as_mut().is_some_and(|window| window.close_requested()) {
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
        plugins: &[Option<Request>],
        bypassed: &[bool],
        from: u32,
        from_sink: bool,
        into: u32,
        latency: &str,
    ) -> Result<Self, EngineError> {
        // What every plug-in is opened for, and what the streams are asked
        // to run at: see `format_param`.
        let rate = f64::from(dsp::SAMPLE_RATE);
        let bypass: Vec<Arc<AtomicBool>> = (0..plugins.len())
            .map(|slot| {
                Arc::new(AtomicBool::new(
                    bypassed.get(slot).copied().unwrap_or(false),
                ))
            })
            .collect();
        let (opened, names, windows) = open_plugins(plugins, &bypass, owner, rate);
        if opened.is_empty() {
            return Err(EngineError::NoPlugin);
        }

        let ring = Arc::new(Ring::new());
        let (capture_props, playback_props) = stream_properties(node, owner, latency, from_sink);

        let capture = StreamRc::new(core.clone(), "pipedeck-plugins-in", capture_props)?;
        let playback = StreamRc::new(core.clone(), "pipedeck-plugins-out", playback_props)?;

        let capture_listener = capture
            .add_local_listener_with_user_data(Processing {
                plugins: opened,
                ring: ring.clone(),
                scratch: vec![vec![0.0; MAX_BLOCK]; CHANNELS],
            })
            .process(run_plugins)
            .register()?;

        let playback_listener = playback
            .add_local_listener_with_user_data(Playing {
                ring,
                scratch: vec![vec![0.0; MAX_BLOCK]; CHANNELS],
            })
            .process(play_out)
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
        let params = plugins
            .iter()
            .map(|request| match request {
                Some(Request::Native { id, params }) => Some((id.clone(), params.clone())),
                _ => None,
            })
            .collect();
        Ok(Self {
            _playback_listener: playback_listener,
            _capture_listener: capture_listener,
            _playback: playback,
            _capture: capture,
            windows,
            open,
            params,
            bypass,
        })
    }
}

/// Open the plug-ins asked for: those that opened, each with its bypass
/// switch, their names, and one slot per plug-in asked for, whether or not
/// it opened, so the caller can point at "the third effect" and be
/// understood; a slot holds Stereo Tool's window when it is one.
#[allow(clippy::type_complexity)]
fn open_plugins(
    plugins: &[Option<Request>],
    bypass: &[Arc<AtomicBool>],
    owner: &str,
    rate: f64,
) -> (
    Vec<(Processor, Arc<AtomicBool>)>,
    Vec<String>,
    Vec<Option<Arc<stereotool::Handle>>>,
) {
    let mut opened = Vec::new();
    let mut names = Vec::new();
    let mut windows = Vec::new();
    for (slot, plugin) in plugins.iter().enumerate() {
        // Asked for and not to be had: its slot stays, empty.
        let Some(plugin) = plugin else {
            windows.push(None);
            continue;
        };
        match Processor::open(plugin, rate, MAX_BLOCK) {
            Ok(instance) => {
                names.push(instance.name().to_owned());
                windows.push(match &instance {
                    Processor::StereoTool(stereotool) => Some(stereotool.handle()),
                    Processor::Vst3(_) | Processor::Native(..) => None,
                });
                opened.push((instance, bypass[slot].clone()));
            }
            Err(e) => {
                log::error!("cannot open a plug-in of {owner}: {e}");
                windows.push(None);
            }
        }
    }
    (opened, names, windows)
}

/// The properties of the stream that reads into the plug-ins and of the one
/// that plays what they made.
fn stream_properties(
    node: &str,
    owner: &str,
    latency: &str,
    from_sink: bool,
) -> (
    pipewire::properties::PropertiesBox,
    pipewire::properties::PropertiesBox,
) {
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
    // Like the loopbacks' own: a stream inside the mixer, which the
    // desktop's volume controls leave out.
    playback_props.insert(*pipewire::keys::NODE_VIRTUAL, "true");
    (capture_props, playback_props)
}

/// The capture side's callback, on the real-time thread: what came in, run
/// through the plug-ins and into the ring.
fn run_plugins(stream: &pipewire::stream::Stream, state: &mut Processing) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let datas = buffer.datas_mut();
    if datas.len() < CHANNELS {
        return;
    }
    let stride = std::mem::size_of::<f32>();
    let total = datas[0].chunk().size() as usize / stride;

    // A quantum longer than the plug-ins were opened for is run
    // through them a block at a time, rather than cut short.
    let mut done = 0;
    while done < total {
        let frames = (total - done).min(MAX_BLOCK);
        for (channel, data) in datas.iter_mut().enumerate().take(CHANNELS) {
            let offset = data.chunk().offset() as usize;
            let Some(bytes) = data.data() else {
                return;
            };
            let plane = &mut state.scratch[channel];
            for (frame, sample) in plane.iter_mut().take(frames).enumerate() {
                let start = offset + (done + frame) * stride;
                *sample = bytes
                    .get(start..start + stride)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map_or(0.0, f32::from_le_bytes);
            }
        }

        let Some(mut block) = block(&mut state.scratch, frames) else {
            return;
        };
        for (plugin, bypassed) in &mut state.plugins {
            if bypassed.load(Ordering::Relaxed) {
                continue;
            }
            if plugin.process(&mut block).is_err() {
                return;
            }
        }
        state.ring.write(&block, frames);
        done += frames;
    }
}

/// The playback side's callback, on the real-time thread: what the plug-ins
/// made, out of the ring.
fn play_out(stream: &pipewire::stream::Stream, state: &mut Playing) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let requested = buffer.requested() as usize;
    let datas = buffer.datas_mut();

    if datas.len() < CHANNELS {
        return;
    }
    let stride = std::mem::size_of::<f32>();
    // As much as was asked for, as far as the buffers hold, a
    // block at a time.
    let room = datas
        .iter_mut()
        .take(CHANNELS)
        .map(|data| data.data().map_or(0, |bytes| bytes.len() / stride))
        .min()
        .unwrap_or(0);
    let total = requested.min(room);
    if total == 0 {
        return;
    }

    let mut done = 0;
    while done < total {
        let frames = (total - done).min(MAX_BLOCK);
        let Some(mut block) = block(&mut state.scratch, frames) else {
            return;
        };
        state.ring.read(&mut block, frames);
        for (channel, data) in datas.iter_mut().enumerate().take(CHANNELS) {
            if let Some(bytes) = data.data() {
                for frame in 0..frames {
                    let at = (done + frame) * stride;
                    let sample = state.scratch[channel][frame].to_le_bytes();
                    bytes[at..at + stride].copy_from_slice(&sample);
                }
            }
        }
        done += frames;
    }
    for data in datas.iter_mut().take(CHANNELS) {
        let chunk = data.chunk_mut();
        *chunk.offset_mut() = 0;
        *chunk.stride_mut() = stride as i32;
        *chunk.size_mut() = (total * stride) as u32;
    }
}

/// The format both streams accept: floats, at the rate the plug-ins run at.
///
/// The bytes are handed back with the pod because a pod is a view into them
/// and must not outlive them.
///
/// The rate is fixed at the one the plug-ins are opened for, and every
/// filter of the mixer's own effects is designed for: on a graph running at
/// another, PipeWire converts on the way in and out, rather than the
/// effects running at a rate their frequencies and times are wrong for.
fn format_param() -> Vec<u8> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32P);
    info.set_rate(dsp::SAMPLE_RATE as u32);
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
