//! The soundboard's files, read: what they are, and their audio, decoded
//! as it is played.
//!
//! A sound is opened on the engine thread, which only reads its headers,
//! then decoded on a thread of its own a little ahead of the audio thread,
//! through a ring buffer: a long file is never held whole in memory, and
//! the audio thread never waits on a disk or a lock.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CodecRegistry, Decoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// What a sound is played as: stereo, interleaved.
pub const CHANNELS: usize = 2;
/// How far ahead of the audio thread the decoding runs.
const AHEAD: Duration = Duration::from_millis(500);
/// How long the decoding waits for room in the buffer before looking again.
const NAP: Duration = Duration::from_millis(10);

/// The decoders: symphonia's own, and the system's libopus for Opus,
/// which symphonia does not decode and Discord's soundboards are often in.
fn codecs() -> &'static CodecRegistry {
    static CODECS: std::sync::OnceLock<CodecRegistry> = std::sync::OnceLock::new();
    CODECS.get_or_init(|| {
        let mut codecs = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut codecs);
        codecs.register_all::<symphonia_adapter_libopus::OpusDecoder>();
        codecs
    })
}

/// A file opened, ready to be decoded.
pub struct Opened {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    /// Its sample rate, which the stream playing it takes: PipeWire
    /// resamples it to the graph's.
    pub rate: u32,
}

/// Open a sound, reading as much of it as tells what it is.
pub fn open(path: &Path) -> Result<Opened, String> {
    let file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let unknown =
        |e: SymphoniaError| format!("{} is not a sound Pipedeck reads: {e}", file_name(path));
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(unknown)?;
    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| format!("{} holds no sound", file_name(path)))?;
    let decoder = codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(unknown)?;
    Ok(Opened {
        rate: track.codec_params.sample_rate.unwrap_or(48_000),
        track: track.id,
        decoder,
        format,
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// The decoding of one sound being played, and what it tells the audio
/// thread. Dropping it stops the decoding.
pub struct Feed {
    /// Set by the decoding once the whole file is in the buffer.
    pub done: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start decoding a sound into a buffer, the audio thread reading the
/// other end, which this hands back.
pub fn feed(opened: Opened) -> (Feed, rtrb::Consumer<f32>) {
    let room = (opened.rate as f32 * AHEAD.as_secs_f32()) as usize * CHANNELS;
    let (producer, consumer) = rtrb::RingBuffer::new(room.max(4096));
    let done = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let spawned = std::thread::Builder::new()
        .name("pipedeck-sound".into())
        .spawn({
            let done = done.clone();
            let stop = stop.clone();
            move || {
                decode(opened, producer, &stop);
                done.store(true, Ordering::Release);
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot decode a sound: {e}");
        done.store(true, Ordering::Release);
    }
    (Feed { done, stop }, consumer)
}

/// Decode the whole sound into the buffer, waiting for room as it goes,
/// unless told to stop.
fn decode(mut opened: Opened, mut producer: rtrb::Producer<f32>, stop: &AtomicBool) {
    let mut stereo: Vec<f32> = Vec::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let packet = match opened.format.next_packet() {
            Ok(packet) => packet,
            // The end of the file, or of what could be read of it.
            Err(_) => return,
        };
        if packet.track_id() != opened.track {
            continue;
        }
        let decoded = match opened.decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A damaged packet is skipped, as a player does.
            Err(SymphoniaError::DecodeError(e)) => {
                log::debug!("a packet of a sound was skipped: {e}");
                continue;
            }
            Err(e) => {
                log::warn!("a sound stopped short: {e}");
                return;
            }
        };
        // Off the audio thread: a buffer a packet costs nothing there.
        let spec = *decoded.spec();
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        to_stereo(buffer.samples(), spec.channels.count(), &mut stereo);
        if !push(&mut producer, &stereo, stop) {
            return;
        }
    }
}

/// Interleaved samples of any number of channels as stereo: one channel is
/// heard on both sides, and past two only the first two are kept.
fn to_stereo(samples: &[f32], channels: usize, into: &mut Vec<f32>) {
    into.clear();
    match channels {
        0 => {}
        1 => into.extend(samples.iter().flat_map(|&s| [s, s])),
        _ => into.extend(
            samples
                .chunks_exact(channels)
                .flat_map(|frame| [frame[0], frame[1]]),
        ),
    }
}

/// Put samples in the buffer as room comes, or say the sound was stopped
/// meanwhile.
fn push(producer: &mut rtrb::Producer<f32>, mut samples: &[f32], stop: &AtomicBool) -> bool {
    while !samples.is_empty() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        // Whole frames only, so the sides never swap.
        let room = producer.slots().min(samples.len()) / CHANNELS * CHANNELS;
        if room == 0 {
            std::thread::sleep(NAP);
            continue;
        }
        if let Ok(chunk) = producer.write_chunk_uninit(room) {
            chunk.fill_from_iter(samples[..room].iter().copied());
        }
        samples = &samples[room..];
    }
    true
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A WAV file of a sine, `seconds` long, in a scratch folder.
    pub(crate) fn sine_wav(
        dir: &Path,
        name: &str,
        rate: u32,
        channels: u16,
        seconds: f32,
    ) -> std::path::PathBuf {
        let frames = (rate as f32 * seconds) as u32;
        let data_len = frames * channels as u32 * 2;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for frame in 0..frames {
            let value = (frame as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin();
            for _ in 0..channels {
                bytes.extend_from_slice(&((value * 16_000.0) as i16).to_le_bytes());
            }
        }
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn scratch() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pipedeck-sound-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_is_decoded_whole_as_stereo() {
        let dir = scratch();
        let path = sine_wav(&dir, "mono.wav", 22_050, 1, 1.5);
        let opened = open(&path).unwrap();
        assert_eq!(opened.rate, 22_050);
        let (feed, mut consumer) = feed(opened);
        let mut read = Vec::new();
        let start = std::time::Instant::now();
        loop {
            let available = consumer.slots();
            if let Ok(chunk) = consumer.read_chunk(available) {
                read.extend(chunk);
            }
            if feed.done.load(Ordering::Acquire) && consumer.slots() == 0 {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the decoding never ended"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // Every frame, each heard on both sides.
        assert_eq!(read.len(), (22_050.0 * 1.5) as usize * CHANNELS);
        assert!(read.as_chunks::<2>().0.iter().all(|[l, r]| l == r));
        assert!(read.iter().any(|s| s.abs() > 0.4));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_stopped_sound_stops_decoding() {
        let dir = scratch();
        let path = sine_wav(&dir, "long.wav", 48_000, 2, 5.0);
        let (feed, consumer) = feed(open(&path).unwrap());
        let done = feed.done.clone();
        drop(feed);
        let start = std::time::Instant::now();
        while !done.load(Ordering::Acquire) {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "the decoding went on"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        // It stopped well before the end.
        assert!(consumer.slots() < 48_000 * 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn opus_is_decoded_as_discord_hands_it_out() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/tone.opus");
        let opened = open(&path).unwrap();
        let (feed, mut consumer) = feed(opened);
        let mut read = Vec::new();
        let start = std::time::Instant::now();
        while !(feed.done.load(Ordering::Acquire) && consumer.slots() == 0) {
            let available = consumer.slots();
            if let Ok(chunk) = consumer.read_chunk(available) {
                read.extend(chunk);
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "the decoding never ended"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // Half a second at Opus's 48 kHz, give or take its padding, of a
        // tone an eighth of full scale, as ffmpeg's sine makes it.
        assert!(
            read.len() / CHANNELS > 20_000,
            "{} frames",
            read.len() / CHANNELS
        );
        let peak = read.iter().fold(0f32, |peak, s| peak.max(s.abs()));
        assert!(peak > 0.08 && peak < 0.2, "peak {peak}");
    }

    #[test]
    fn what_is_not_a_sound_is_said_so() {
        let dir = scratch();
        let path = dir.join("notes.txt");
        std::fs::write(&path, "not a sound").unwrap();
        let error = open(&path).err().unwrap();
        assert!(error.contains("notes.txt"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn more_channels_keep_the_first_two() {
        let mut out = Vec::new();
        to_stereo(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 3, &mut out);
        assert_eq!(out, [1.0, 2.0, 4.0, 5.0]);
        to_stereo(&[0.5, -0.5], 1, &mut out);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);
    }
}
