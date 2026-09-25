//! A five-band equaliser: a low cut, a low shelf, two bells and a high shelf.
//!
//! That is the shape a voice is usually worked in — rumble out, body, mud or
//! honk, presence, air — and five bands are few enough to set by dragging
//! them on a curve. `response` draws that curve from the same filters the
//! audio goes through.

use std::sync::Arc;

use super::biquad::{Coeffs, State};
use super::{ParamSpec, Params};

/// The low cut is off at its lowest setting: nothing a mixer carries is
/// that low, and a switch would be one more control for the same thing.
pub const LOW_CUT_OFF: f32 = 20.0;

pub const PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "low_cut",
        label: "Low cut",
        min: LOW_CUT_OFF,
        max: 1000.0,
        default: 80.0,
        unit: " Hz",
    },
    ParamSpec {
        name: "low_freq",
        label: "Low",
        min: 20.0,
        max: 5000.0,
        default: 150.0,
        unit: " Hz",
    },
    ParamSpec {
        name: "low_gain",
        label: "Low gain",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: " dB",
    },
    ParamSpec {
        name: "mid_freq",
        label: "Mid",
        min: 20.0,
        max: 20000.0,
        default: 800.0,
        unit: " Hz",
    },
    ParamSpec {
        name: "mid_gain",
        label: "Mid gain",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: " dB",
    },
    ParamSpec {
        name: "mid_q",
        label: "Mid width",
        min: 0.3,
        max: 6.0,
        default: 1.0,
        unit: "",
    },
    ParamSpec {
        name: "pres_freq",
        label: "Presence",
        min: 20.0,
        max: 20000.0,
        default: 3500.0,
        unit: " Hz",
    },
    ParamSpec {
        name: "pres_gain",
        label: "Presence gain",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: " dB",
    },
    ParamSpec {
        name: "pres_q",
        label: "Presence width",
        min: 0.3,
        max: 6.0,
        default: 1.0,
        unit: "",
    },
    ParamSpec {
        name: "high_freq",
        label: "Air",
        min: 1000.0,
        max: 20000.0,
        default: 10000.0,
        unit: " Hz",
    },
    ParamSpec {
        name: "high_gain",
        label: "Air gain",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: " dB",
    },
];

const BANDS: usize = 5;

/// Where each band is set, in the order of `PARAMS`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    /// What the curve calls it.
    pub label: &'static str,
    /// The index in `PARAMS` of its frequency, its gain and its width, for
    /// the ones it has.
    pub freq: usize,
    pub gain: Option<usize>,
    pub q: Option<usize>,
}

/// The bands, as the curve draws them and a pointer moves them.
pub const BAND_LAYOUT: [Band; BANDS] = [
    Band {
        label: "Low cut",
        freq: 0,
        gain: None,
        q: None,
    },
    Band {
        label: "Low",
        freq: 1,
        gain: Some(2),
        q: None,
    },
    Band {
        label: "Mid",
        freq: 3,
        gain: Some(4),
        q: Some(5),
    },
    Band {
        label: "Presence",
        freq: 6,
        gain: Some(7),
        q: Some(8),
    },
    Band {
        label: "Air",
        freq: 9,
        gain: Some(10),
        q: None,
    },
];

/// The five filters for a set of values, in the order of `PARAMS`.
pub fn design(values: &[f32]) -> [Coeffs; BANDS] {
    let v = |i: usize| values.get(i).copied().unwrap_or(PARAMS[i].default);
    let low_cut = if v(0) <= LOW_CUT_OFF {
        Coeffs::IDENTITY
    } else {
        Coeffs::highpass(v(0))
    };
    [
        low_cut,
        Coeffs::low_shelf(v(1), v(2)),
        Coeffs::peaking(v(3), v(5), v(4)),
        Coeffs::peaking(v(6), v(8), v(7)),
        Coeffs::high_shelf(v(9), v(10)),
    ]
}

/// What the equaliser does at `freq`, in decibels.
pub fn response(values: &[f32], freq: f32) -> f32 {
    design(values).iter().map(|c| c.response_db(freq)).sum()
}

pub(super) struct Equaliser {
    params: Arc<Params>,
    values: [f32; PARAMS.len()],
    /// What the filters were designed from, to design them again only when
    /// something moved.
    designed: [f32; PARAMS.len()],
    coeffs: [Coeffs; BANDS],
    states: Vec<[State; BANDS]>,
}

impl Equaliser {
    pub fn new(params: Arc<Params>, channels: usize) -> Self {
        let mut values = [0.0; PARAMS.len()];
        params.read(&mut values);
        Self {
            params,
            values,
            designed: values,
            coeffs: design(&values),
            states: vec![[State::default(); BANDS]; channels],
        }
    }
}

impl super::Native for Equaliser {
    fn process(&mut self, channels: &mut [&mut [f32]]) {
        self.params.read(&mut self.values);
        if self.values != self.designed {
            self.designed = self.values;
            self.coeffs = design(&self.values);
        }
        for (channel, states) in channels.iter_mut().zip(self.states.iter_mut()) {
            for sample in channel.iter_mut() {
                let mut x = *sample;
                for (state, coeffs) in states.iter_mut().zip(self.coeffs.iter()) {
                    x = state.run(coeffs, x);
                }
                *sample = x;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::{run, settled_rms, sine};
    use crate::dsp::{defaults, spec};
    use crate::types::Control;

    fn equaliser(changes: &[(&str, f32)]) -> (Arc<Params>, Equaliser) {
        let spec = spec("eq").expect("the equaliser");
        let mut controls = defaults(spec);
        for (name, value) in changes {
            if let Some(control) = controls.iter_mut().find(|c| c.name == *name) {
                control.value = *value;
            }
        }
        let params = Params::new(spec, &controls);
        (params.clone(), Equaliser::new(params, 2))
    }

    #[test]
    fn a_band_lifted_lifts_what_is_played_there() {
        let (_, mut eq) = equaliser(&[("mid_freq", 1000.0), ("mid_gain", 6.0)]);
        let tone = sine(1000.0, 0.25, 1.0);
        let out = run(&mut eq, &tone);
        let gain = 20.0 * (settled_rms(&out) / settled_rms(&tone)).log10();
        assert!((gain - 6.0).abs() < 0.5, "lifted by {gain} dB");
    }

    #[test]
    fn at_rest_it_changes_nothing_but_the_rumble() {
        let (_, mut eq) = equaliser(&[]);
        let voice = sine(440.0, 0.25, 1.0);
        let out = run(&mut eq, &voice);
        let gain = 20.0 * (settled_rms(&out) / settled_rms(&voice)).log10();
        assert!(gain.abs() < 0.3, "a flat equaliser moved it {gain} dB");
        assert!(response(&[], 30.0) < -6.0, "the low cut starts on");
    }

    #[test]
    fn a_setting_changed_while_running_is_heard() {
        let (params, mut eq) = equaliser(&[("mid_freq", 1000.0)]);
        let tone = sine(1000.0, 0.25, 0.5);
        let before = settled_rms(&run(&mut eq, &tone));
        let spec = spec("eq").expect("the equaliser");
        params.set(
            spec,
            &[Control {
                name: "mid_gain".into(),
                value: -12.0,
            }],
        );
        let after = settled_rms(&run(&mut eq, &tone));
        assert!(after < before * 0.35, "{before} then {after}");
    }
}
