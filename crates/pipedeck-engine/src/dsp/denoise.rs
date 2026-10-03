//! Noise suppression, by RNNoise: a small neural network that tells a voice
//! from what is behind it, frame by frame.
//!
//! This is `nnnoiseless`, RNNoise ported to Rust with its model built in, so
//! nothing is installed for it. It works on frames of ten milliseconds, so
//! it holds each one back until it is whole: that is the delay it adds.

use std::sync::Arc;

use nnnoiseless::DenoiseState;

use super::Params;

const FRAME: usize = DenoiseState::FRAME_SIZE;

/// A starting point for noise suppression: its strength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DenoisePreset {
    pub name: &'static str,
    pub description: &'static str,
    pub values: [f32; 1],
}

/// The starting points offered, lightest first.
pub const DENOISE_PRESETS: &[DenoisePreset] = &[
    DenoisePreset {
        name: "Off",
        description: "The sound goes through untouched",
        values: [0.0],
    },
    DenoisePreset {
        name: "Light",
        description: "Takes the edge off a fan or a hiss, and stays natural",
        values: [50.0],
    },
    DenoisePreset {
        name: "Medium",
        description: "Most of the room gone, a trace left so the voice breathes",
        values: [80.0],
    },
    DenoisePreset {
        name: "Full",
        description: "Everything but the voice",
        values: [100.0],
    },
];

/// RNNoise takes samples on the scale of 16-bit integers, not of floats.
const SCALE: f32 = 32768.0;

struct Channel {
    state: Box<DenoiseState<'static>>,
    /// The frame being filled, already scaled.
    input: Box<[f32; FRAME]>,
    filled: usize,
    /// The last frame done, and the one it was done from, so the untouched
    /// sound can be mixed back in step with the treated one.
    output: Box<[f32; FRAME]>,
    dry: Box<[f32; FRAME]>,
    scratch: Box<[f32; FRAME]>,
    /// How sure RNNoise was that the last frame done was a voice, from 0
    /// to 1, and how far that frame was turned down once mixed with the
    /// untouched sound, in decibels: what the window draws.
    voice: f32,
    removed: f32,
}

impl Channel {
    fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            input: Box::new([0.0; FRAME]),
            filled: 0,
            output: Box::new([0.0; FRAME]),
            dry: Box::new([0.0; FRAME]),
            scratch: Box::new([0.0; FRAME]),
            voice: 0.0,
            removed: 0.0,
        }
    }

    /// One sample in, one sample out, a frame later.
    #[inline]
    fn run(&mut self, x: f32, wet: f32) -> f32 {
        // What comes out now is from the frame done last, at the place the
        // one coming in is being written: the two move in step, one frame
        // apart.
        let treated = self.output[self.filled] / SCALE;
        let untouched = self.dry[self.filled] / SCALE;
        self.input[self.filled] = x * SCALE;
        self.filled += 1;
        if self.filled == FRAME {
            self.voice = self
                .state
                .process_frame(&mut self.scratch[..], &self.input[..]);
            // How much quieter the frame comes out than it went in, as it is
            // heard: the treated sound mixed back with the untouched one.
            let (mut before, mut after) = (0.0f32, 0.0f32);
            for (out, dry) in self.scratch.iter().zip(self.input.iter()) {
                let heard = out * wet + dry * (1.0 - wet);
                before += dry * dry;
                after += heard * heard;
            }
            // Under some -70 dB there is nothing to take out.
            let quiet = FRAME as f32 * (SCALE * 3e-4).powi(2);
            self.removed = if before > quiet {
                (10.0 * (before / after.max(1e-9)).log10()).max(0.0)
            } else {
                0.0
            };
            std::mem::swap(&mut self.output, &mut self.scratch);
            std::mem::swap(&mut self.dry, &mut self.input);
            self.filled = 0;
        }
        treated * wet + untouched * (1.0 - wet)
    }
}

pub(super) struct Denoise {
    params: Arc<Params>,
    values: [f32; 1],
    channels: Vec<Channel>,
}

impl Denoise {
    pub fn new(params: Arc<Params>, channels: usize) -> Self {
        Self {
            params,
            values: [100.0],
            channels: (0..channels).map(|_| Channel::new()).collect(),
        }
    }
}

impl super::Native for Denoise {
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        self.params.read(&mut self.values);
        let wet = (self.values[0] / 100.0).clamp(0.0, 1.0);
        for (channel, state) in channels.iter_mut().zip(self.channels.iter_mut()) {
            for sample in channel.iter_mut() {
                *sample = state.run(*sample, wet);
            }
        }
        let voice = self
            .channels
            .iter()
            .fold(0.0f32, |most, c| most.max(c.voice));
        let removed = self
            .channels
            .iter()
            .fold(0.0f32, |most, c| most.max(c.removed));
        self.params.live.report(voice, removed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::spec;
    use crate::dsp::testing::{run, settled_rms};
    use crate::types::Control;

    /// Noise that is the same every run, so the test is too.
    fn hiss(seconds: f32) -> Vec<f32> {
        let mut seed: u32 = 0x9e37_79b9;
        (0..(super::super::SAMPLE_RATE * seconds) as usize)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                // Background noise as a room has it, some thirty-five
                // decibels down. RNNoise takes loud broadband noise for a
                // voice and leaves it, which is what it was trained to do.
                (seed as f32 / u32::MAX as f32 - 0.5) * 0.06
            })
            .collect()
    }

    #[test]
    fn it_says_how_far_it_takes_a_room_down() {
        let spec = spec("denoise").expect("noise suppression");
        let params = Params::new(spec, &[]);
        let mut denoise = Denoise::new(params.clone(), 2);
        run(&mut denoise, &hiss(1.0));
        let (voice, removed) = params.live.take();
        assert!(removed > 6.0, "the room only went down {removed} dB");
        assert!(
            (0.0..=1.0).contains(&voice),
            "a chance of a voice of {voice}"
        );
    }

    #[test]
    fn steady_noise_is_taken_down() {
        let spec = spec("denoise").expect("noise suppression");
        let mut denoise = Denoise::new(Params::new(spec, &[]), 2);
        let noise = hiss(2.0);
        let out = run(&mut denoise, &noise);
        let taken = 20.0 * (settled_rms(&out) / settled_rms(&noise)).log10();
        assert!(taken < -10.0, "noise only taken down {taken} dB");
    }

    #[test]
    fn at_no_strength_it_is_the_sound_one_frame_late() {
        let spec = spec("denoise").expect("noise suppression");
        let params = Params::new(
            spec,
            &[Control {
                name: "strength".into(),
                value: 0.0,
            }],
        );
        let mut denoise = Denoise::new(params, 2);
        let signal: Vec<f32> = (0..FRAME * 4)
            .map(|i| (i as f32 * 0.01).sin() * 0.3)
            .collect();
        let out = run(&mut denoise, &signal);
        for i in FRAME..signal.len() {
            assert!((out[i] - signal[i - FRAME]).abs() < 1e-4, "sample {i}");
        }
    }
}
