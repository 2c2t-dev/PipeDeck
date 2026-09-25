//! Effects that turn a sound down when it gets loud: a compressor for the
//! whole of it, a de-esser for its hiss.
//!
//! Both follow the level with an envelope that rises fast and falls slowly,
//! and turn down by how far it is over a threshold. The channels are
//! followed together, so a stereo image does not lean when one side is
//! louder.

use std::sync::Arc;

use super::biquad::{Coeffs, State};
use super::{from_db, to_db, Params, SAMPLE_RATE};

/// How much of the envelope is kept from one sample to the next, for a
/// given time to settle.
fn smoothing(seconds: f32) -> f32 {
    (-1.0 / (seconds * SAMPLE_RATE)).exp()
}

/// Follows a level: quick to rise, slower to fall.
struct Envelope {
    attack: f32,
    release: f32,
    level: f32,
}

impl Envelope {
    fn new(attack: f32, release: f32) -> Self {
        Self {
            attack: smoothing(attack),
            release: smoothing(release),
            level: 0.0,
        }
    }

    #[inline]
    fn follow(&mut self, x: f32) -> f32 {
        let keep = if x > self.level {
            self.attack
        } else {
            self.release
        };
        self.level = keep * self.level + (1.0 - keep) * x;
        self.level
    }
}

/// How many decibels to turn down a level that is `over` its threshold, with
/// a soft knee so the turn starts gradually.
#[inline]
fn reduction(over: f32, ratio: f32) -> f32 {
    const KNEE: f32 = 6.0;
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if over <= -KNEE / 2.0 {
        0.0
    } else if over >= KNEE / 2.0 {
        over * slope
    } else {
        let x = over + KNEE / 2.0;
        slope * x * x / (2.0 * KNEE)
    }
}

/// A compressor with the three controls that matter: where it starts, how
/// hard it pushes, and how much is given back after.
///
/// Its timing is fixed at what suits a voice — ten milliseconds to act, a
/// hundred and fifty to let go — since those are the two most people would
/// otherwise have to learn.
pub(super) struct Compressor {
    params: Arc<Params>,
    values: [f32; 3],
    envelope: Envelope,
}

impl Compressor {
    pub fn new(params: Arc<Params>) -> Self {
        Self {
            params,
            values: [0.0; 3],
            envelope: Envelope::new(0.010, 0.150),
        }
    }
}

impl super::Native for Compressor {
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        self.params.read(&mut self.values);
        let [threshold, ratio, makeup] = self.values;
        let frames = channels.first().map_or(0, |c| c.len());
        for frame in 0..frames {
            let peak = channels
                .iter()
                .fold(0.0f32, |loudest, channel| loudest.max(channel[frame].abs()));
            let level = to_db(self.envelope.follow(peak));
            let gain = from_db(makeup - reduction(level - threshold, ratio));
            for channel in channels.iter_mut() {
                channel[frame] *= gain;
            }
        }
    }
}

/// A de-esser: listens to one band, where s and sh are, and turns that band
/// down when it is too loud, leaving everything under it alone.
///
/// The signal is split in two at the frequency — what is under it, and what
/// is over — the upper part is turned down, and the two are put back
/// together. The split is a Linkwitz-Riley crossover: each side is two
/// Butterworth filters in a row, and the two sides stay in phase at every
/// frequency, so turning one down turns it down. A split made by taking a
/// filtered copy away from the sound does not: near the frequency the two
/// parts are out of phase, and the hiss comes back through the other one.
pub(super) struct DeEsser {
    params: Arc<Params>,
    values: [f32; 2],
    designed: f32,
    low: Coeffs,
    high: Coeffs,
    listen: Coeffs,
    /// Per channel: the two stages of each side of the crossover.
    lows: Vec<[State; 2]>,
    highs: Vec<[State; 2]>,
    listens: Vec<State>,
    envelope: Envelope,
}

impl DeEsser {
    pub fn new(params: Arc<Params>, channels: usize) -> Self {
        Self {
            params,
            values: [0.0; 2],
            designed: 0.0,
            low: Coeffs::IDENTITY,
            high: Coeffs::IDENTITY,
            listen: Coeffs::IDENTITY,
            lows: vec![[State::default(); 2]; channels],
            highs: vec![[State::default(); 2]; channels],
            listens: vec![State::default(); channels],
            envelope: Envelope::new(0.001, 0.060),
        }
    }
}

impl super::Native for DeEsser {
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        self.params.read(&mut self.values);
        let [freq, strength] = self.values;
        if freq != self.designed {
            self.designed = freq;
            self.low = Coeffs::lowpass(freq);
            self.high = Coeffs::highpass(freq);
            self.listen = Coeffs::bandpass(freq, 1.5);
        }
        // Strength is where it starts: at nothing, only the harshest hiss is
        // touched; at full, most of it is.
        let threshold = -10.0 - strength.clamp(0.0, 100.0) * 0.4;
        const RATIO: f32 = 6.0;

        let frames = channels.first().map_or(0, |c| c.len());
        for frame in 0..frames {
            let mut hiss = 0.0f32;
            for (channel, state) in channels.iter().zip(self.listens.iter_mut()) {
                hiss = hiss.max(state.run(&self.listen, channel[frame]).abs());
            }
            let level = to_db(self.envelope.follow(hiss));
            let gain = from_db(-reduction(level - threshold, RATIO));
            let sides = self.lows.iter_mut().zip(self.highs.iter_mut());
            for (channel, (low, high)) in channels.iter_mut().zip(sides) {
                let x = channel[frame];
                let under = low[0].run(&self.low, x);
                let under = low[1].run(&self.low, under);
                let over = high[0].run(&self.high, x);
                let over = high[1].run(&self.high, over);
                channel[frame] = under + over * gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::spec;
    use crate::dsp::testing::{run, settled_rms, sine};
    use crate::types::Control;

    fn controls(pairs: &[(&str, f32)]) -> Vec<Control> {
        pairs
            .iter()
            .map(|(name, value)| Control {
                name: (*name).to_owned(),
                value: *value,
            })
            .collect()
    }

    fn db(out: &[f32], input: &[f32]) -> f32 {
        20.0 * (settled_rms(out) / settled_rms(input)).log10()
    }

    #[test]
    fn a_loud_sound_is_turned_down_by_the_ratio_and_a_quiet_one_is_not() {
        let spec = spec("compressor").expect("the compressor");
        let params = Params::new(
            spec,
            &controls(&[("threshold", -20.0), ("ratio", 4.0), ("makeup", 0.0)]),
        );
        let mut compressor = Compressor::new(params.clone());

        // A peak of -6 dB is 14 over; four to one leaves 3.5, so 10.5 off.
        let loud = sine(440.0, from_db(-6.0), 1.0);
        let turned = db(&run(&mut compressor, &loud), &loud);
        assert!((turned + 10.5).abs() < 1.5, "turned down {turned} dB");

        let mut compressor = Compressor::new(params);
        let quiet = sine(440.0, from_db(-40.0), 1.0);
        let left = db(&run(&mut compressor, &quiet), &quiet);
        assert!(left.abs() < 0.3, "a quiet sound moved {left} dB");
    }

    #[test]
    fn makeup_gives_back_what_it_says() {
        let spec = spec("compressor").expect("the compressor");
        let params = Params::new(
            spec,
            &controls(&[("threshold", 0.0), ("ratio", 1.0), ("makeup", 6.0)]),
        );
        let mut compressor = Compressor::new(params);
        let tone = sine(440.0, 0.1, 0.5);
        let gain = db(&run(&mut compressor, &tone), &tone);
        assert!((gain - 6.0).abs() < 0.2, "made up {gain} dB");
    }

    #[test]
    fn hiss_is_turned_down_and_a_voice_is_left_alone() {
        let spec = spec("deesser").expect("the de-esser");
        let params = Params::new(spec, &controls(&[("freq", 6500.0), ("strength", 100.0)]));

        let mut deesser = DeEsser::new(params.clone(), 2);
        let hiss = sine(7000.0, from_db(-6.0), 1.0);
        let turned = db(&run(&mut deesser, &hiss), &hiss);
        assert!(turned < -6.0, "hiss only turned down {turned} dB");

        let mut deesser = DeEsser::new(params, 2);
        let voice = sine(300.0, from_db(-6.0), 1.0);
        let left = db(&run(&mut deesser, &voice), &voice);
        assert!(left.abs() < 0.5, "a voice moved {left} dB");
    }
}
