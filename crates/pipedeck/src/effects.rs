//! The effects a channel can run, as the window offers them.
//!
//! These are the mixer's own — noise suppression, an equaliser, a de-esser,
//! a compressor — described in the engine, which runs them; this only says
//! how the window builds one and writes its values.

use pipedeck_engine::dsp::{self, EffectSpec, ParamSpec};
use pipedeck_engine::{Control, Effect, EffectKind};

/// Every effect offered, in order.
pub fn catalogue() -> &'static [EffectSpec] {
    dsp::EFFECTS
}

/// The description behind an effect, when it is one of the mixer's own.
pub fn spec(effect: &Effect) -> Option<&'static EffectSpec> {
    if effect.kind == EffectKind::Native {
        dsp::spec(&effect.label)
    } else {
        None
    }
}

/// An effect ready to be added, with every control at its default.
pub fn build(spec: &EffectSpec) -> Effect {
    Effect {
        name: spec.name.to_owned(),
        kind: EffectKind::Native,
        plugin: None,
        label: spec.id.to_owned(),
        controls: dsp::defaults(spec),
    }
}

/// What an effect has a control set to, or its default if it says nothing.
pub fn value_of(effect: &Effect, param: &ParamSpec) -> f32 {
    effect
        .controls
        .iter()
        .find(|control| control.name == param.name)
        .map_or(param.default, |control| control.value)
}

/// Set one control of an effect, adding it if the effect did not have it.
pub fn set_value(effect: &mut Effect, name: &str, value: f32) {
    match effect
        .controls
        .iter_mut()
        .find(|control| control.name == name)
    {
        Some(control) => control.value = value,
        None => effect.controls.push(Control {
            name: name.to_owned(),
            value,
        }),
    }
}

/// A value as the window writes it, with its unit.
pub fn format(param: &ParamSpec, value: f32) -> String {
    match param.unit {
        " Hz" if value >= 1000.0 => format!("{:.1} kHz", value / 1000.0),
        " Hz" => format!("{value:.0} Hz"),
        " dB" => format!("{value:+.1} dB"),
        ":1" => format!("{value:.1}:1"),
        " %" => format!("{value:.0} %"),
        unit => format!("{value:.1}{unit}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_effect_finds_its_way_back_to_its_description() {
        for entry in catalogue() {
            let effect = build(entry);
            assert_eq!(effect.controls.len(), entry.params.len());
            assert_eq!(spec(&effect).map(|s| s.id), Some(entry.id));
            for param in entry.params {
                assert_eq!(value_of(&effect, param), param.default);
            }
        }
    }

    #[test]
    fn a_value_reads_with_its_unit() {
        let freq = dsp::spec("deesser").expect("the de-esser").params[0];
        assert_eq!(format(&freq, 6500.0), "6.5 kHz");
        assert_eq!(format(&freq, 800.0), "800 Hz");
        let threshold = dsp::spec("compressor").expect("the compressor").params[0];
        assert_eq!(format(&threshold, -20.0), "-20.0 dB");
        let ratio = dsp::spec("compressor").expect("the compressor").params[1];
        assert_eq!(format(&ratio, 3.0), "3.0:1");
    }

    #[test]
    fn setting_a_control_it_lacked_adds_it() {
        let mut effect = build(&catalogue()[0]);
        effect.controls.clear();
        set_value(&mut effect, "strength", 40.0);
        set_value(&mut effect, "strength", 60.0);
        assert_eq!(effect.controls.len(), 1);
        assert_eq!(effect.controls[0].value, 60.0);
    }
}
