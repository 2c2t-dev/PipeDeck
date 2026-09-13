//! The effects a channel can run, and what to call their controls.
//!
//! Every one of these is a filter PipeWire ships itself, so nothing has to be
//! installed for them to work. The shape carries plugins as well: an effect
//! is a kind, a plugin and a label, which is what an LV2 or LADSPA plug-in
//! needs too, and only this catalogue would grow.

use pipedeck_engine::{Control, Effect, EffectKind};

/// One knob of an effect, named the way the filter names it.
pub struct ControlSpec {
    /// What the filter calls it; it goes into the graph as is.
    pub name: &'static str,
    /// What the window calls it.
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f32,
    pub unit: &'static str,
}

/// An effect as the window offers it.
pub struct EffectSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The builtin filter behind it.
    pub label: &'static str,
    pub controls: &'static [ControlSpec],
}

const FREQ: ControlSpec = ControlSpec {
    name: "Freq",
    label: "Frequency",
    min: 20.0,
    max: 12_000.0,
    default: 90.0,
    unit: " Hz",
};

const GAIN: ControlSpec = ControlSpec {
    name: "Gain",
    label: "Gain",
    min: -15.0,
    max: 15.0,
    default: 3.0,
    unit: " dB",
};

pub const EFFECTS: &[EffectSpec] = &[
    EffectSpec {
        name: "Low cut",
        description: "Takes out the rumble under a voice",
        label: "bq_highpass",
        controls: &[FREQ],
    },
    EffectSpec {
        name: "Warmth",
        description: "Lifts or trims the low end",
        label: "bq_lowshelf",
        controls: &[
            ControlSpec {
                default: 250.0,
                ..FREQ
            },
            GAIN,
        ],
    },
    EffectSpec {
        name: "Presence",
        description: "Lifts or trims the top end",
        label: "bq_highshelf",
        controls: &[
            ControlSpec {
                default: 5_000.0,
                ..FREQ
            },
            GAIN,
        ],
    },
    EffectSpec {
        name: "Tone",
        description: "Lifts or trims one band",
        label: "bq_peaking",
        controls: &[
            ControlSpec {
                default: 900.0,
                ..FREQ
            },
            ControlSpec {
                default: -3.0,
                ..GAIN
            },
            ControlSpec {
                name: "Q",
                label: "Width",
                min: 0.2,
                max: 6.0,
                default: 1.0,
                unit: "",
            },
        ],
    },
    EffectSpec {
        name: "Gain",
        description: "Makes the channel louder or quieter before its faders",
        label: "linear",
        controls: &[ControlSpec {
            name: "Mult",
            label: "Multiplier",
            min: 0.0,
            max: 4.0,
            default: 1.0,
            unit: "",
        }],
    },
];

/// The catalogue entry behind an effect, when it is one of ours.
pub fn spec(effect: &Effect) -> Option<&'static EffectSpec> {
    EFFECTS.iter().find(|spec| spec.label == effect.label)
}

/// An effect ready to be added, with every control at its default.
pub fn build(spec: &EffectSpec) -> Effect {
    Effect {
        name: spec.name.to_owned(),
        kind: EffectKind::Builtin,
        plugin: None,
        label: spec.label.to_owned(),
        controls: spec
            .controls
            .iter()
            .map(|control| Control {
                name: control.name.to_owned(),
                value: control.default,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_effect_finds_its_way_back_to_its_spec() {
        for entry in EFFECTS {
            let effect = build(entry);
            assert_eq!(effect.controls.len(), entry.controls.len());
            let found = spec(&effect).expect("its own spec");
            assert_eq!(found.name, entry.name);
        }
    }

    #[test]
    fn every_control_starts_inside_its_range() {
        for entry in EFFECTS {
            for control in entry.controls {
                let value = f64::from(control.default);
                assert!(
                    value >= control.min && value <= control.max,
                    "{} {} starts at {value}",
                    entry.name,
                    control.label
                );
            }
        }
    }

    #[test]
    fn no_two_effects_share_a_filter() {
        let mut labels: Vec<&str> = EFFECTS.iter().map(|spec| spec.label).collect();
        labels.sort_unstable();
        let count = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), count, "two effects would be told apart wrong");
    }
}
