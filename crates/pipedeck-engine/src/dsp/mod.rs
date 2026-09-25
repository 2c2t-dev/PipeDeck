//! The effects the mixer runs itself: noise suppression, an equaliser, a
//! de-esser and a compressor.
//!
//! None of them needs anything installed. They run in the same chain as the
//! plug-ins the mixer hosts, block by block on the audio thread, and their
//! settings reach them there without the chain being made again: a knob
//! turned is a number written, not a reload, so the audio never stops for
//! it.
//!
//! Every effect is described here — what it is called, what its controls
//! are and where they go — and the interface draws its controls from the
//! same description.

pub mod biquad;
mod denoise;
mod dynamics;
pub mod eq;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use crate::types::Control;

/// The rate the graph runs at, which every filter here is designed for.
pub const SAMPLE_RATE: f32 = 48_000.0;

/// One control of an effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// What the config calls it.
    pub name: &'static str,
    /// What the window calls it.
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: &'static str,
}

/// One effect as it is offered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectSpec {
    /// What the config calls it: the effect's `label`.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamSpec],
}

const fn param(
    name: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
) -> ParamSpec {
    ParamSpec {
        name,
        label,
        min,
        max,
        default,
        unit,
    }
}

pub use dynamics::{
    compressor_output, deesser_gain, learn_compressor, learn_deesser, CompressorPreset,
    DeEsserPreset, COMPRESSOR_PRESETS, DEESSER_PRESETS,
};

/// Whether an effect can be set from a voice it listens to.
pub fn learns(id: &str) -> bool {
    matches!(id, "compressor" | "deesser")
}

/// The settings an effect that listened calls for, from what it heard.
pub fn learn(id: &str, counts: &[u32]) -> Result<Vec<Control>, String> {
    match id {
        "compressor" => learn_compressor(counts),
        "deesser" => learn_deesser(counts),
        _ => Err(format!("{id} does not learn")),
    }
}

/// Every effect the mixer runs itself, in the order they are offered.
pub const EFFECTS: &[EffectSpec] = &[
    EffectSpec {
        id: "denoise",
        name: "Noise suppression",
        description: "Takes out fans, keyboards and hiss behind a voice",
        params: &[param("strength", "Strength", 0.0, 100.0, 100.0, " %")],
    },
    EffectSpec {
        id: "eq",
        name: "Equaliser",
        description: "Shapes the tone, band by band",
        params: eq::PARAMS,
    },
    EffectSpec {
        id: "deesser",
        name: "De-esser",
        description: "Tames the hiss of s and sh",
        params: &[
            param("freq", "Frequency", 3000.0, 12000.0, 6500.0, " Hz"),
            param("strength", "Strength", 0.0, 100.0, 50.0, " %"),
        ],
    },
    EffectSpec {
        id: "compressor",
        name: "Compressor",
        description: "Evens out loud and quiet moments",
        params: &[
            param("threshold", "Threshold", -60.0, 0.0, -20.0, " dB"),
            param("ratio", "Ratio", 1.0, 20.0, 3.0, ":1"),
            param("makeup", "Makeup", 0.0, 24.0, 3.0, " dB"),
        ],
    },
];

/// The description of an effect, by the id the config keeps.
pub fn spec(id: &str) -> Option<&'static EffectSpec> {
    EFFECTS.iter().find(|spec| spec.id == id)
}

/// Every control of an effect at its default, as a new one starts.
pub fn defaults(spec: &EffectSpec) -> Vec<Control> {
    spec.params
        .iter()
        .map(|param| Control {
            name: param.name.to_owned(),
            value: param.default,
        })
        .collect()
}

/// An effect's settings, shared between the engine and the audio thread.
///
/// Each is a float kept in an atomic, so the engine writes and the audio
/// thread reads without either waiting on the other. A block reads them all
/// once as it starts.
pub struct Params {
    values: Box<[AtomicU32]>,
    /// What the effect has heard, when asked to listen.
    pub heard: Heard,
}

/// The quietest level [`Heard`] counts, in decibels; anything under it is
/// counted there.
pub const HEARD_FLOOR: i32 = -90;

/// How many levels [`Heard`] tells apart, a decibel apart.
pub const HEARD_LEVELS: usize = (1 - HEARD_FLOOR) as usize;

/// How often an effect that listens has stood at each level: one count
/// for every hundredth of a second, a decibel apart from [`HEARD_FLOOR`] up
/// to 0 dB, in as many rows as it has things to listen to.
///
/// The compressor and the de-esser listen, so they can be set from a voice
/// rather than by hand: the compressor to its level, the de-esser to each
/// of the frequencies it could work at. It is written on the audio thread
/// and read by the engine, and neither waits on the other.
pub struct Heard {
    listening: AtomicBool,
    counts: Box<[AtomicU32]>,
}

impl Heard {
    fn new(rows: usize) -> Self {
        Self {
            listening: AtomicBool::new(false),
            counts: (0..rows * HEARD_LEVELS)
                .map(|_| AtomicU32::new(0))
                .collect(),
        }
    }

    /// Start counting again from nothing, or stop.
    pub fn listen(&self, listening: bool) {
        if listening {
            for count in self.counts.iter() {
                count.store(0, Ordering::Relaxed);
            }
        }
        self.listening.store(listening, Ordering::Release);
    }

    pub fn is_listening(&self) -> bool {
        self.listening.load(Ordering::Acquire)
    }

    /// Count one moment at `db`, in the first row.
    pub fn count(&self, db: f32) {
        self.count_in(0, db);
    }

    /// Count one moment at `db`, in a row of its own.
    pub fn count_in(&self, row: usize, db: f32) {
        let at = (db.floor() as i32).clamp(HEARD_FLOOR, 0) - HEARD_FLOOR;
        if let Some(count) = self.counts.get(row * HEARD_LEVELS + at as usize) {
            count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Every count, row after row, quietest first in each.
    pub fn counts(&self) -> Vec<u32> {
        self.counts
            .iter()
            .map(|count| count.load(Ordering::Relaxed))
            .collect()
    }
}

impl std::fmt::Debug for Params {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut values = vec![0.0; self.len()];
        self.read(&mut values);
        f.debug_tuple("Params").field(&values).finish()
    }
}

impl Params {
    /// The settings an effect starts with: what the config says, and the
    /// default for anything it does not.
    pub fn new(spec: &EffectSpec, controls: &[Control]) -> Arc<Self> {
        let params = Self {
            values: spec
                .params
                .iter()
                .map(|param| AtomicU32::new(param.default.to_bits()))
                .collect(),
            heard: Heard::new(match spec.id {
                "compressor" => 1,
                "deesser" => dynamics::PROBES.len(),
                _ => 0,
            }),
        };
        params.set(spec, controls);
        Arc::new(params)
    }

    /// Take new settings. Controls the effect does not have are ignored,
    /// and every value is kept within its range.
    pub fn set(&self, spec: &EffectSpec, controls: &[Control]) {
        for (slot, param) in self.values.iter().zip(spec.params) {
            if let Some(control) = controls.iter().find(|c| c.name == param.name) {
                let value = control.value.clamp(param.min, param.max);
                slot.store(value.to_bits(), Ordering::Relaxed);
            }
        }
    }

    /// Read every setting into `into`, which is as long as the effect has
    /// controls.
    pub fn read(&self, into: &mut [f32]) {
        for (value, slot) in into.iter_mut().zip(self.values.iter()) {
            *value = f32::from_bits(slot.load(Ordering::Relaxed));
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// An effect running on the audio thread.
///
/// `process` is called with one buffer per channel, all the same length, and
/// must neither allocate nor wait: everything it needs is made in `open`.
pub trait Native: Send {
    fn process(&mut self, channels: &mut [&mut [f32]]);
}

/// Make an effect ready to run, with its settings.
pub fn open(id: &str, params: Arc<Params>, channels: usize) -> Option<Box<dyn Native>> {
    Some(match id {
        "denoise" => Box::new(denoise::Denoise::new(params, channels)),
        "eq" => Box::new(eq::Equaliser::new(params, channels)),
        "deesser" => Box::new(dynamics::DeEsser::new(params, channels)),
        "compressor" => Box::new(dynamics::Compressor::new(params)),
        _ => return None,
    })
}

/// Decibels from an amplitude, never minus infinity.
pub(crate) fn to_db(amplitude: f32) -> f32 {
    20.0 * amplitude.max(1e-9).log10()
}

/// An amplitude from decibels.
pub(crate) fn from_db(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
pub(crate) mod testing {
    //! Signals to put through an effect, and how loud what comes out is.

    use super::SAMPLE_RATE;

    pub fn sine(freq: f32, amplitude: f32, seconds: f32) -> Vec<f32> {
        (0..(SAMPLE_RATE * seconds) as usize)
            .map(|i| amplitude * (i as f32 / SAMPLE_RATE * freq * std::f32::consts::TAU).sin())
            .collect()
    }

    /// Loudness of the second half, where an effect has settled.
    pub fn settled_rms(signal: &[f32]) -> f32 {
        let tail = &signal[signal.len() / 2..];
        (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt()
    }

    /// Put a mono signal through an effect as a stereo pair, in blocks as
    /// the graph hands them, and give back the left channel.
    pub fn run(effect: &mut dyn super::Native, signal: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(signal.len());
        for block in signal.chunks(512) {
            let mut left = block.to_vec();
            let mut right = block.to_vec();
            {
                let mut channels: Vec<&mut [f32]> = vec![&mut left, &mut right];
                effect.process(&mut channels);
            }
            out.extend_from_slice(&left);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effect_opens_and_starts_inside_its_ranges() {
        for spec in EFFECTS {
            for param in spec.params {
                assert!(
                    param.default >= param.min && param.default <= param.max,
                    "{} {} starts at {}",
                    spec.name,
                    param.label,
                    param.default
                );
            }
            let params = Params::new(spec, &defaults(spec));
            assert!(open(spec.id, params, 2).is_some(), "{}", spec.name);
        }
        assert!(open("nothing", Params::new(&EFFECTS[0], &[]), 2).is_none());
    }

    #[test]
    fn settings_are_kept_within_range_and_unknown_ones_ignored() {
        let spec = spec("compressor").expect("the compressor");
        let params = Params::new(
            spec,
            &[
                Control {
                    name: "ratio".into(),
                    value: 500.0,
                },
                Control {
                    name: "nonsense".into(),
                    value: 1.0,
                },
            ],
        );
        let mut values = vec![0.0; params.len()];
        params.read(&mut values);
        assert_eq!(values, [-20.0, 20.0, 3.0]);
    }
}
