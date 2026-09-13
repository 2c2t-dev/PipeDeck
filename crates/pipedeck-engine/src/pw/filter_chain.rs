//! Building the arguments of `libpipewire-module-filter-chain`.
//!
//! A channel's effects are one filter chain: it captures the channel's sink,
//! runs the effects in order, and plays the result into a second sink the
//! cells capture instead. The chain is built per channel rather than per
//! cell, so every mix hears the same treated signal, which is what a mixer
//! means by an effect on a channel.
//!
//! The graph is mono, so each effect is instantiated once per channel of the
//! stream and the channels are wired in parallel: a filter node has one input
//! and one output, and stereo needs two of them.

use crate::types::{Effect, SourceConfig};

use super::args::{render, Val};
use super::loopback::{AUDIO_POSITION, CHANNELS};

/// Names of the channels, in the order they are wired.
const CHANNEL_SUFFIX: [&str; CHANNELS] = ["l", "r"];

/// One node of the filter graph.
fn node(name: String, effect: &Effect) -> Val {
    let mut entries = vec![
        ("type".to_owned(), Val::Raw(effect.kind.as_str().to_owned())),
        ("name".to_owned(), Val::from(name)),
        ("label".to_owned(), Val::from(effect.label.clone())),
    ];
    if let Some(plugin) = &effect.plugin {
        entries.push(("plugin".to_owned(), Val::from(plugin.clone())));
    }
    if !effect.controls.is_empty() {
        entries.push((
            "control".to_owned(),
            Val::dict(
                effect
                    .controls
                    .iter()
                    .map(|control| (control.name.clone(), Val::from(control.value))),
            ),
        ));
    }
    Val::Dict(entries)
}

/// The arguments loading a channel's effects.
///
/// Returns nothing when the channel has no effect: a chain of none would be
/// a node, a quantum and a name for nothing.
pub fn args(source: &SourceConfig, latency: &str) -> Option<String> {
    // Plug-ins are hosted by the mixer, not by PipeWire, so they are not
    // part of this graph.
    let effects: Vec<&Effect> = source
        .effects
        .iter()
        .filter(|effect| !effect.is_plugin())
        .collect();
    if effects.is_empty() {
        return None;
    }

    let mut nodes = Vec::new();
    let mut links = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();

    for (channel, suffix) in CHANNEL_SUFFIX.iter().enumerate() {
        let names: Vec<String> = (0..effects.len())
            .map(|step| format!("fx{step}{suffix}"))
            .collect();
        for (effect, name) in effects.iter().zip(&names) {
            nodes.push(node(name.clone(), effect));
        }
        for pair in names.windows(2) {
            links.push(Val::dict([
                ("output", Val::from(format!("{}:Out", pair[0]))),
                ("input", Val::from(format!("{}:In", pair[1]))),
            ]));
        }
        let _ = channel;
        inputs.push(Val::from(format!("{}:In", names[0])));
        outputs.push(Val::from(format!(
            "{}:Out",
            names.last().expect("at least one effect")
        )));
    }

    let stream = |name: String, description: String| {
        Val::dict([
            ("node.name", Val::from(name)),
            ("node.description", Val::from(description)),
            ("pipedeck.instance", Val::from(super::instance())),
            ("audio.position", Val::Raw(AUDIO_POSITION.to_owned())),
            ("node.latency", Val::from(latency.to_owned())),
            ("node.dont-reconnect", Val::from(true)),
            ("state.restore-props", Val::from(false)),
            ("state.restore-target", Val::from(false)),
        ])
    };

    let capture_name = format!("{}.in", source.id.effects_node_name());
    let mut capture = stream(
        capture_name,
        format!("Pipedeck: {} effects in", source.name),
    );
    if let Val::Dict(entries) = &mut capture {
        entries.push((
            "media.class".to_owned(),
            Val::from("Stream/Input/Audio/Internal"),
        ));
        entries.push(("stream.capture.sink".to_owned(), Val::from(true)));
        entries.push((
            "target.object".to_owned(),
            Val::from(source.id.sink_node_name()),
        ));
    }

    let mut playback = stream(
        format!("{}.out", source.id.effects_node_name()),
        format!("Pipedeck: {} effects out", source.name),
    );
    if let Val::Dict(entries) = &mut playback {
        // The chain plays into a sink of ours rather than being one: asking
        // a filter chain to be a sink crashes PipeWire 1.6, and the cells
        // need a monitor to capture anyway.
        entries.push((
            "target.object".to_owned(),
            Val::from(source.id.effects_node_name()),
        ));
    }

    Some(render(vec![
        (
            "node.description",
            Val::from(format!("Pipedeck: {} effects", source.name)),
        ),
        ("capture.props", capture),
        ("playback.props", playback),
        (
            "filter.graph",
            Val::dict([
                ("nodes", Val::array(nodes)),
                ("links", Val::array(links)),
                ("inputs", Val::array(inputs)),
                ("outputs", Val::array(outputs)),
            ]),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Control, EffectKind, SourceId};

    fn low_cut() -> Effect {
        Effect {
            name: "Low cut".into(),
            kind: EffectKind::Builtin,
            plugin: None,
            label: "bq_highpass".into(),
            controls: vec![Control {
                name: "Freq".into(),
                value: 90.0,
            }],
        }
    }

    #[test]
    fn a_channel_without_effects_loads_nothing() {
        let source = SourceConfig::virtual_sink(SourceId(1), "Game");
        assert!(args(&source, "512/48000").is_none());
    }

    #[test]
    fn every_channel_of_the_stream_gets_the_chain() {
        let mut source = SourceConfig::virtual_sink(SourceId(3), "Mic");
        source.effects = vec![low_cut(), low_cut()];
        let rendered = args(&source, "512/48000").expect("a chain");

        // Two effects across two channels, wired in series within each.
        assert_eq!(rendered.matches("label = \"bq_highpass\"").count(), 4);
        assert!(rendered.contains("{ output = \"fx0l:Out\" input = \"fx1l:In\" }"));
        assert!(rendered.contains("{ output = \"fx0r:Out\" input = \"fx1r:In\" }"));
        assert!(rendered.contains("inputs = [ \"fx0l:In\" \"fx0r:In\" ]"));
        assert!(rendered.contains("outputs = [ \"fx1l:Out\" \"fx1r:Out\" ]"));
        assert!(rendered.contains("control = { Freq = 90 }"));
        // It reads the channel's own sink and becomes a sink of its own.
        assert!(rendered.contains("target.object = \"pipedeck.src.3\""));
        // The chain reads the channel and plays into the sink named after
        // it, which is created alongside.
        assert!(rendered.contains("node.name = \"pipedeck.fx.3.out\""));
        assert!(rendered.contains("target.object = \"pipedeck.fx.3\""));
        assert!(!rendered.contains("Audio/Sink"));
    }
}
