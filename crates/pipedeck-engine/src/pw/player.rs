//! A sound of the soundboard, playing: a playback stream into the
//! soundboard's row, fed by the sound's decoding.
//!
//! The stream plays at the file's own rate and PipeWire resamples it. On
//! the real-time thread it only copies what the decoding left in the ring
//! buffer, and plays silence when there is nothing yet; once the decoding is
//! done and the buffer is empty, it says the sound has ended, and the
//! engine takes it off the graph on its next tick.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use libspa::param::audio::{AudioFormat, AudioInfoRaw, MAX_CHANNELS};
use libspa::param::ParamType;
use libspa::pod::serialize::PodSerializer;
use libspa::pod::{Object, Pod, Value};
use libspa::utils::{Direction, SpaTypes};
use pipewire::core::CoreRc;
use pipewire::properties::properties;
use pipewire::stream::{StreamFlags, StreamListener, StreamRc, StreamState};

use crate::error::EngineError;
use crate::sound::{self, Feed, Opened, CHANNELS};

/// What the stream needs while it runs.
struct State {
    samples: rtrb::Consumer<f32>,
    done: Arc<AtomicBool>,
    ended: Arc<AtomicBool>,
}

/// A sound playing. Dropping it takes its stream off the graph and stops
/// its decoding.
pub struct Player {
    ended: Arc<AtomicBool>,
    // The listener borrows the stream, so it is declared first and dropped
    // first; the decoding goes last, once nothing reads from it.
    _listener: StreamListener<State>,
    _stream: StreamRc,
    _feed: Feed,
}

impl Player {
    /// Play an opened sound into `target`, one of our sinks, by its name and
    /// the id the server gave it when it is known.
    pub fn new(
        core: &CoreRc,
        name: &str,
        description: &str,
        opened: Opened,
        target: &str,
        target_id: Option<u32>,
        latency: &str,
    ) -> Result<Self, EngineError> {
        let rate = opened.rate;
        let mut props = properties! {
            *pipewire::keys::MEDIA_TYPE => "Audio",
            *pipewire::keys::MEDIA_CATEGORY => "Playback",
            *pipewire::keys::MEDIA_ROLE => "Production",
            *pipewire::keys::NODE_NAME => name,
            *pipewire::keys::NODE_DESCRIPTION => description,
            *pipewire::keys::NODE_LATENCY => latency,
            "pipedeck.instance" => crate::pw::instance(),
            // Played into the soundboard's row and nowhere else: not moved
            // by the session manager, nor kept on another sink if the row
            // goes.
            *pipewire::keys::NODE_DONT_RECONNECT => "true",
            "state.restore-props" => "false",
            "state.restore-target" => "false",
        };
        // A name and an id would disagree when another mixer holds the same
        // name, and the name wins, so only one of the two is ever set.
        if target_id.is_none() {
            props.insert(*pipewire::keys::TARGET_OBJECT, target);
        }

        let stream = StreamRc::new(core.clone(), name, props)?;
        let (feed, samples) = sound::feed(opened);
        let ended = Arc::new(AtomicBool::new(false));
        let listener = stream
            .add_local_listener_with_user_data(State {
                samples,
                done: feed.done.clone(),
                ended: ended.clone(),
            })
            // Cut off, its row made again or gone: it will play no more.
            .state_changed(|_, state, _, now| {
                if matches!(now, StreamState::Error(_) | StreamState::Unconnected) {
                    state.ended.store(true, Ordering::Relaxed);
                }
            })
            .process(|stream, state| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let requested = buffer.requested() as usize;
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else {
                    return;
                };
                let stride = std::mem::size_of::<f32>() * CHANNELS;
                let frames = match data.data() {
                    Some(bytes) => {
                        let room = bytes.len() / stride;
                        let frames = if requested > 0 {
                            requested.min(room)
                        } else {
                            room
                        };
                        let wanted = frames * CHANNELS;
                        let ready = state.samples.slots().min(wanted);
                        if ready == 0 && state.done.load(Ordering::Acquire) {
                            state.ended.store(true, Ordering::Relaxed);
                        }
                        let mut out = bytes[..frames * stride].as_chunks_mut::<4>().0.iter_mut();
                        if let Ok(chunk) = state.samples.read_chunk(ready) {
                            for (sample, slot) in chunk.into_iter().zip(out.by_ref()) {
                                *slot = sample.to_le_bytes();
                            }
                        }
                        // Silence for what the decoding has not caught up
                        // with, or after the end.
                        for slot in out {
                            *slot = 0f32.to_le_bytes();
                        }
                        frames
                    }
                    None => 0,
                };
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as i32;
                *chunk.size_mut() = (frames * stride) as u32;
            })
            .register()?;

        let mut info = AudioInfoRaw::new();
        info.set_format(AudioFormat::F32LE);
        info.set_rate(rate);
        info.set_channels(CHANNELS as u32);
        let mut position = [0; MAX_CHANNELS];
        position[0] = libspa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = libspa::sys::SPA_AUDIO_CHANNEL_FR;
        info.set_position(position);
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
            Direction::Output,
            target_id,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )?;

        log::debug!("{name} plays into {target}");
        Ok(Self {
            ended,
            _listener: listener,
            _stream: stream,
            _feed: feed,
        })
    }

    /// Whether the whole sound has been played.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }
}
