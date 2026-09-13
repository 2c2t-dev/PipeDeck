//! Level measurement.
//!
//! A meter is a capture stream tapping the monitor ports of one of our sinks,
//! or the device behind an input channel. It reads the peak of every buffer
//! and leaves it in an atomic; nothing else happens on the real-time thread,
//! no allocation and no lock, and the engine picks the value up on its own
//! timer.
//!
//! Measuring once per channel and once per mix is enough for the whole
//! matrix: a cell carries no signal of its own, it carries its channel's
//! signal scaled by its fader, which is what the interface draws.

use std::sync::atomic::{AtomicU32, Ordering};
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

/// The peak of the last buffer, as f32 bits.
#[derive(Clone, Default)]
pub struct Level(Arc<AtomicU32>);

impl Level {
    fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }

    /// Peak in `0.0..=1.0`, read from any thread.
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// What the stream needs while it runs.
struct State {
    channels: u32,
    level: Level,
}

/// A running measurement. Dropping it takes the stream off the graph.
pub struct Meter {
    level: Level,
    // The listener borrows the stream, so it is declared first and dropped
    // first.
    _listener: StreamListener<State>,
    _stream: StreamRc,
}

impl Meter {
    /// Tap `target`. A sink is captured through its monitor ports, a device
    /// straight through its own.
    pub fn new(
        core: &CoreRc,
        name: &str,
        target: &str,
        target_id: Option<u32>,
        from_sink: bool,
    ) -> Result<Self, EngineError> {
        let mut props = properties! {
            *pipewire::keys::MEDIA_TYPE => "Audio",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Production",
            *pipewire::keys::NODE_NAME => name,
            *pipewire::keys::NODE_DESCRIPTION => "Pipedeck level meter",
            "pipedeck.instance" => crate::pw::instance(),
            // Same class as our other capture streams, so a meter does not
            // show up as a recording application.
            *pipewire::keys::MEDIA_CLASS => "Stream/Input/Audio/Internal",
            // A meter must never be the reason a device stays busy, and it
            // must not follow the default sink when its target disappears.
            *pipewire::keys::NODE_PASSIVE => "true",
            *pipewire::keys::NODE_DONT_RECONNECT => "true",
            "state.restore-props" => "false",
            "state.restore-target" => "false",
        };
        if from_sink {
            props.insert(*pipewire::keys::STREAM_CAPTURE_SINK, "true");
        }
        // A name and an id would disagree when another mixer holds the same
        // name, and the name wins, so only one of the two is ever set.
        if target_id.is_none() {
            props.insert(*pipewire::keys::TARGET_OBJECT, target);
        }

        let stream = StreamRc::new(core.clone(), name, props)?;
        let level = Level::default();
        let listener = stream
            .add_local_listener_with_user_data(State {
                channels: 2,
                level: level.clone(),
            })
            .param_changed(|_, state, id, param| {
                let Some(param) = param else {
                    return;
                };
                if id != ParamType::Format.as_raw() {
                    return;
                }
                let mut info = AudioInfoRaw::new();
                if info.parse(param).is_ok() {
                    state.channels = info.channels().max(1);
                }
            })
            .process(|stream, state| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else {
                    return;
                };
                let size = data.chunk().size() as usize;
                let Some(samples) = data.data() else {
                    return;
                };
                let frames = size.min(samples.len()) / std::mem::size_of::<f32>();
                let mut peak = 0.0f32;
                for frame in 0..frames {
                    let start = frame * std::mem::size_of::<f32>();
                    let bytes: [u8; 4] = match samples[start..start + 4].try_into() {
                        Ok(bytes) => bytes,
                        Err(_) => break,
                    };
                    peak = peak.max(f32::from_le_bytes(bytes).abs());
                }
                state.level.store(peak.min(1.0));
            })
            .register()?;

        let mut info = AudioInfoRaw::new();
        info.set_format(AudioFormat::F32LE);
        let object = Object {
            type_: SpaTypes::ObjectParamFormat.as_raw(),
            id: ParamType::EnumFormat.as_raw(),
            properties: info.into(),
        };
        let bytes =
            PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
                .expect("serializing an in-memory pod cannot fail")
                .0
                .into_inner();
        let mut params = [Pod::from_bytes(&bytes).expect("the format pod is valid")];

        stream.connect(
            Direction::Input,
            target_id,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        log::debug!("{name} is measuring {target}");
        Ok(Self {
            level,
            _listener: listener,
            _stream: stream,
        })
    }

    /// Peak of the last buffer, and forget it, so a meter that stops being
    /// fed falls back to silence instead of holding its last value.
    pub fn take(&self) -> f32 {
        let peak = self.level.get();
        self.level.store(0.0);
        peak
    }
}
