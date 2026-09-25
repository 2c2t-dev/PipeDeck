//! Effects that turn a sound down when it gets loud: a compressor for the
//! whole of it, a de-esser for its hiss.
//!
//! Both follow the level with an envelope that rises fast and falls slowly,
//! and turn down by how far it is over a threshold. The channels are
//! followed together, so a stereo image does not lean when one side is
//! louder.

use std::sync::Arc;

use super::biquad::{Coeffs, State};
use super::{from_db, to_db, Params, HEARD_FLOOR, HEARD_LEVELS, SAMPLE_RATE};
use crate::types::Control;

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

/// What the compressor makes of a level: `input` in, the result out, both
/// in decibels. This is the curve its window draws, from the same sums the
/// audio goes through.
pub fn compressor_output(input: f32, threshold: f32, ratio: f32, makeup: f32) -> f32 {
    input - reduction(input - threshold, ratio) + makeup
}

/// How hard the de-esser turns its upper band down once over its
/// threshold.
const DEESSER_RATIO: f32 = 6.0;

/// Where the de-esser starts, in decibels of the band it listens to, for a
/// strength: at nothing, only the harshest hiss is touched; at full, most
/// of it is.
fn deesser_threshold(strength: f32) -> f32 {
    -10.0 - strength.clamp(0.0, 100.0) * 0.4
}

/// The strength that puts the de-esser's threshold at `threshold`.
fn strength_for(threshold: f32) -> f32 {
    ((-10.0 - threshold) / 0.4).clamp(0.0, 100.0)
}

/// What the de-esser does to what is over its frequency when an s reaches
/// `level` decibels in the band it listens to: the gain it applies there,
/// in decibels, never over zero. This is what its window draws.
pub fn deesser_gain(strength: f32, level: f32) -> f32 {
    -reduction(level - deesser_threshold(strength), DEESSER_RATIO)
}

/// A starting point for the de-esser: its frequency and strength.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeEsserPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub values: [f32; 2],
}

/// The starting points offered.
pub const DEESSER_PRESETS: &[DeEsserPreset] = &[
    DeEsserPreset {
        name: "Light",
        description: "Only the sharpest s, barely heard doing it",
        values: [7000.0, 30.0],
    },
    DeEsserPreset {
        name: "Normal",
        description: "The usual for a voice",
        values: [6500.0, 50.0],
    },
    DeEsserPreset {
        name: "Strong",
        description: "For a voice or a microphone that hisses a lot",
        values: [6000.0, 75.0],
    },
    DeEsserPreset {
        name: "Deep voice",
        description: "Its s sit lower",
        values: [5000.0, 50.0],
    },
    DeEsserPreset {
        name: "High voice",
        description: "Its s sit higher",
        values: [8000.0, 50.0],
    },
    DeEsserPreset {
        name: "Very sibilant",
        description: "Takes the s well down, lower in the range",
        values: [5500.0, 90.0],
    },
];

/// A starting point for the compressor: its threshold, ratio and makeup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompressorPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub values: [f32; 3],
}

/// The starting points offered, gentlest first.
pub const COMPRESSOR_PRESETS: &[CompressorPreset] = &[
    CompressorPreset {
        name: "Off",
        description: "Changes nothing at all",
        values: [0.0, 1.0, 0.0],
    },
    CompressorPreset {
        name: "Gentle",
        description: "Evens a voice out without being heard doing it",
        values: [-18.0, 2.0, 2.0],
    },
    CompressorPreset {
        name: "Voice",
        description: "The usual for talking: steady and natural",
        values: [-20.0, 3.0, 3.0],
    },
    CompressorPreset {
        name: "Podcast",
        description: "Close and even, for talking at length",
        values: [-24.0, 4.0, 6.0],
    },
    CompressorPreset {
        name: "Broadcast",
        description: "Dense and loud, the way radio sounds",
        values: [-28.0, 6.0, 10.0],
    },
    CompressorPreset {
        name: "Shouts in check",
        description: "Leaves speech alone and catches the shouts",
        values: [-14.0, 8.0, 2.0],
    },
    CompressorPreset {
        name: "Music",
        description: "A light hold on music, to keep it together",
        values: [-14.0, 2.0, 2.0],
    },
];

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
    /// Samples since the level was last counted, while listening.
    since_heard: usize,
}

/// How many samples apart a listening compressor counts its level.
const HEARD_EVERY: usize = SAMPLE_RATE as usize / 100;

impl Compressor {
    pub fn new(params: Arc<Params>) -> Self {
        Self {
            params,
            values: [0.0; 3],
            envelope: Envelope::new(0.010, 0.150),
            since_heard: 0,
        }
    }
}

impl super::Native for Compressor {
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        self.params.read(&mut self.values);
        let [threshold, ratio, makeup] = self.values;
        let listening = self.params.heard.is_listening();
        let frames = channels.first().map_or(0, |c| c.len());
        for frame in 0..frames {
            let peak = channels
                .iter()
                .fold(0.0f32, |loudest, channel| loudest.max(channel[frame].abs()));
            let level = to_db(self.envelope.follow(peak));
            // What is counted is what the threshold is compared with, so
            // the settings learnt from it mean what they say.
            if listening {
                self.since_heard += 1;
                if self.since_heard >= HEARD_EVERY {
                    self.since_heard = 0;
                    self.params.heard.count(level);
                }
            }
            let gain = from_db(makeup - reduction(level - threshold, ratio));
            for channel in channels.iter_mut() {
                channel[frame] *= gain;
            }
        }
    }
}

/// Settings for the compressor from a few seconds of a voice, as
/// [`super::Heard`] counted them: the threshold where the voice usually
/// is, a ratio as firm as it is uneven, and the makeup that brings its loud
/// moments back up to a level made for streaming.
///
/// Silence is left out: the moments much quieter than the loudest are the
/// room between words, not the voice.
pub fn learn_compressor(counts: &[u32]) -> Result<Vec<Control>, String> {
    /// Where the loud moments of a voice are brought, once compressed and
    /// made up: loud enough to stream, with room left for its peaks.
    const TARGET: f32 = -10.0;

    let level = |at: usize| (at as i32 + HEARD_FLOOR) as f32 + 0.5;
    // The loudest the voice got, leaving out a stray pop.
    let total: u32 = counts.iter().sum();
    let loudest = percentile(counts, 0, total, 0.99).map(level);
    let Some(loudest) = loudest.filter(|db| *db > -60.0) else {
        return Err("Nothing was heard. Check the microphone and speak through the count.".into());
    };
    let floor = ((loudest - 30.0).max(-60.0) - HEARD_FLOOR as f32).floor() as usize;
    let voiced: u32 = counts.get(floor..).unwrap_or(&[]).iter().sum();
    // A second of voice, in hundredths.
    if voiced < 100 {
        return Err("Too little was heard. Speak through the whole count.".into());
    }
    let at = |fraction| percentile(counts, floor, voiced, fraction).map_or(loudest, level);
    let (quiet, usual, loud) = (at(0.10), at(0.50), at(0.90));

    let half = |x: f32| (x * 2.0).round() / 2.0;
    let threshold = half(usual).clamp(-60.0, 0.0);
    let ratio = half(1.5 + (loud - quiet) / 10.0).clamp(2.0, 6.0);
    let squeezed = threshold + (loud - threshold).max(0.0) / ratio;
    let makeup = half(TARGET - squeezed).clamp(0.0, 24.0);
    Ok(vec![
        Control {
            name: "threshold".into(),
            value: threshold,
        },
        Control {
            name: "ratio".into(),
            value: ratio,
        },
        Control {
            name: "makeup".into(),
            value: makeup,
        },
    ])
}

/// The frequencies a listening de-esser tries, where the s and sh of a
/// voice can be.
pub(super) const PROBES: [f32; 12] = [
    3000.0, 3500.0, 4000.0, 4500.0, 5000.0, 5600.0, 6300.0, 7100.0, 8000.0, 9000.0, 10000.0,
    11200.0,
];

/// Settings for the de-esser from a few seconds of a voice, as it heard
/// them at each of its [`PROBES`].
///
/// Its frequency goes where the s is loudest, a step under so the split
/// leaves the whole of it above; its strength puts the threshold under the
/// loudest s but over the rest of the voice there, so the hiss is turned
/// down and the words are not.
pub fn learn_deesser(counts: &[u32]) -> Result<Vec<Control>, String> {
    let level = |at: usize| (at as i32 + HEARD_FLOOR) as f32 + 0.5;
    let rows: Vec<&[u32]> = counts.chunks(HEARD_LEVELS).take(PROBES.len()).collect();
    if rows.len() < PROBES.len() {
        return Err("Nothing was heard. Check the microphone and speak through the count.".into());
    }
    let at = |row: &[u32], fraction: f32| {
        let total = row.iter().sum();
        percentile(row, 0, total, fraction).map_or(HEARD_FLOOR as f32, level)
    };
    // Where the hiss peaks, among the probes a voice's s can be at.
    let (loudest, peak) = rows
        .iter()
        .enumerate()
        .skip(2)
        .map(|(probe, row)| (probe, at(row, 0.98)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("there are probes");
    if peak < -60.0 {
        return Err("Nothing was heard. Check the microphone and speak through the count.".into());
    }
    let split = loudest - 1;
    let row = rows[split];
    let hiss = at(row, 0.98);
    // What the rest of the voice usually reaches there: an s is a small
    // part of speech, so the middle of what was heard is the rest of it.
    let voice = at(row, 0.50);
    let threshold = (hiss - 6.0).max(voice + 3.0);
    let strength = strength_for(threshold);
    Ok(vec![
        Control {
            name: "freq".into(),
            value: PROBES[split],
        },
        Control {
            name: "strength".into(),
            value: strength.round(),
        },
    ])
}

/// The bucket under which `fraction` of the `total` counts from `from` on
/// lie.
fn percentile(counts: &[u32], from: usize, total: u32, fraction: f32) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let wanted = (total as f32 * fraction).ceil().max(1.0) as u32;
    let mut seen = 0;
    for (at, count) in counts.iter().enumerate().skip(from) {
        seen += count;
        if seen >= wanted {
            return Some(at);
        }
    }
    None
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
    /// While listening: a band-pass at each of the [`PROBES`], per channel,
    /// and how loud each one is.
    probes: Vec<Coeffs>,
    probe_states: Vec<Vec<State>>,
    probe_envelopes: Vec<Envelope>,
    since_heard: usize,
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
            probes: PROBES.iter().map(|f| Coeffs::bandpass(*f, 1.5)).collect(),
            probe_states: vec![vec![State::default(); PROBES.len()]; channels],
            probe_envelopes: PROBES.iter().map(|_| Envelope::new(0.001, 0.060)).collect(),
            since_heard: 0,
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
        let threshold = deesser_threshold(strength);
        let listening = self.params.heard.is_listening();

        let frames = channels.first().map_or(0, |c| c.len());
        for frame in 0..frames {
            // Listening, each probe hears the band it is at the way the
            // de-esser would there: the same filter, the same envelope.
            if listening {
                self.since_heard += 1;
                let tick = self.since_heard >= HEARD_EVERY;
                if tick {
                    self.since_heard = 0;
                }
                for (probe, (coeffs, envelope)) in self
                    .probes
                    .iter()
                    .zip(self.probe_envelopes.iter_mut())
                    .enumerate()
                {
                    let mut loudest = 0.0f32;
                    for (channel, states) in channels.iter().zip(self.probe_states.iter_mut()) {
                        loudest = loudest.max(states[probe].run(coeffs, channel[frame]).abs());
                    }
                    let level = envelope.follow(loudest);
                    if tick {
                        self.params.heard.count_in(probe, to_db(level));
                    }
                }
            }
            let mut hiss = 0.0f32;
            for (channel, state) in channels.iter().zip(self.listens.iter_mut()) {
                hiss = hiss.max(state.run(&self.listen, channel[frame]).abs());
            }
            let level = to_db(self.envelope.follow(hiss));
            let gain = from_db(-reduction(level - threshold, DEESSER_RATIO));
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
    fn every_compressor_preset_is_within_range() {
        let spec = spec("compressor").expect("the compressor");
        for preset in COMPRESSOR_PRESETS {
            for (value, param) in preset.values.iter().zip(spec.params) {
                assert!(
                    (param.min..=param.max).contains(value),
                    "{} sets {} to {value}",
                    preset.name,
                    param.label
                );
            }
        }
        // Off is off: the curve is the diagonal.
        for input in [-60.0, -20.0, 0.0] {
            assert_eq!(compressor_output(input, 0.0, 1.0, 0.0), input);
        }
    }

    #[test]
    fn every_deesser_preset_is_within_range_and_stronger_goes_deeper() {
        let spec = spec("deesser").expect("the de-esser");
        for preset in DEESSER_PRESETS {
            for (value, param) in preset.values.iter().zip(spec.params) {
                assert!(
                    (param.min..=param.max).contains(value),
                    "{} sets {} to {value}",
                    preset.name,
                    param.label
                );
            }
        }
        assert!(deesser_gain(100.0, -12.0) < deesser_gain(20.0, -12.0));
        assert!(deesser_gain(0.0, -40.0) == 0.0);
        assert!((strength_for(deesser_threshold(42.0)) - 42.0).abs() < 1e-3);
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

    fn value(controls: &[Control], name: &str) -> f32 {
        controls
            .iter()
            .find(|control| control.name == name)
            .map(|control| control.value)
            .expect(name)
    }

    #[test]
    fn a_voice_heard_sets_the_compressor_around_it() {
        // Two seconds of silence and three of a voice going between -32 and
        // -14 dB, as the compressor itself hears it.
        let spec = spec("compressor").expect("the compressor");
        let params = Params::new(spec, &[]);
        params.heard.listen(true);
        let mut compressor = Compressor::new(params.clone());
        let mut signal = vec![0.0; 2 * SAMPLE_RATE as usize];
        for (i, db) in [-32.0, -26.0, -20.0, -14.0, -22.0, -28.0]
            .iter()
            .enumerate()
        {
            let part = sine(220.0 + 40.0 * i as f32, from_db(*db), 0.5);
            signal.extend(part);
        }
        run(&mut compressor, &signal);
        params.heard.listen(false);

        let learnt = learn_compressor(&params.heard.counts()).expect("a voice was heard");
        let threshold = value(&learnt, "threshold");
        let ratio = value(&learnt, "ratio");
        let makeup = value(&learnt, "makeup");
        assert!(
            (-28.0..=-18.0).contains(&threshold),
            "threshold {threshold}"
        );
        assert!((2.0..=6.0).contains(&ratio), "ratio {ratio}");
        // The loudest part, squeezed and made up, lands near the target.
        let loud = -14.0;
        let out = threshold + (loud - threshold) / ratio + makeup;
        assert!((out + 10.0).abs() < 2.5, "the loud part comes out at {out}");
    }

    #[test]
    fn silence_teaches_nothing() {
        let spec = spec("compressor").expect("the compressor");
        let params = Params::new(spec, &[]);
        params.heard.listen(true);
        let mut compressor = Compressor::new(params.clone());
        run(&mut compressor, &vec![0.0; SAMPLE_RATE as usize * 3]);
        assert!(learn_compressor(&params.heard.counts()).is_err());
        assert!(learn_compressor(&[]).is_err());
    }

    /// A voice at 300 Hz, with an s at `hiss` hertz for a tenth of every
    /// half second.
    fn sibilant_voice(hiss: f32, seconds: f32) -> Vec<f32> {
        let voice = sine(300.0, from_db(-12.0), seconds);
        let s = sine(hiss, from_db(-10.0), seconds);
        let half = SAMPLE_RATE as usize / 2;
        voice
            .iter()
            .zip(&s)
            .enumerate()
            .map(|(i, (v, s))| if i % half < half / 5 { v + s } else { *v })
            .collect()
    }

    #[test]
    fn a_voice_heard_sets_the_deesser_on_its_s() {
        let spec = spec("deesser").expect("the de-esser");
        let params = Params::new(spec, &[]);
        params.heard.listen(true);
        let mut deesser = DeEsser::new(params.clone(), 2);
        run(&mut deesser, &sibilant_voice(7100.0, 5.0));
        params.heard.listen(false);

        let learnt = learn_deesser(&params.heard.counts()).expect("a voice was heard");
        let freq = value(&learnt, "freq");
        assert!((5000.0..7100.0).contains(&freq), "split at {freq}");

        // Set that way, it takes the s down and leaves the voice.
        let learnt_params = Params::new(spec, &learnt);
        let mut deesser = DeEsser::new(learnt_params.clone(), 2);
        let hiss = sine(7100.0, from_db(-10.0), 1.0);
        let turned = db(&run(&mut deesser, &hiss), &hiss);
        assert!(turned < -3.0, "the s only went down {turned} dB");
        let mut deesser = DeEsser::new(learnt_params, 2);
        let voice = sine(300.0, from_db(-12.0), 1.0);
        let left = db(&run(&mut deesser, &voice), &voice);
        assert!(left.abs() < 0.5, "the voice moved {left} dB");
    }

    #[test]
    fn a_deesser_that_heard_nothing_learns_nothing() {
        let spec = spec("deesser").expect("the de-esser");
        let params = Params::new(spec, &[]);
        params.heard.listen(true);
        let mut deesser = DeEsser::new(params.clone(), 2);
        run(&mut deesser, &vec![0.0; SAMPLE_RATE as usize]);
        assert!(learn_deesser(&params.heard.counts()).is_err());
        assert!(learn_deesser(&[]).is_err());
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
