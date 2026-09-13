//! Argument string builder for `libpipewire-module-loopback`.
//!
//! The module takes its configuration as a SPA JSON dictionary. We only need
//! a handful of keys, so this is a tiny serializer rather than a dependency
//! on a JSON crate.

use crate::types::{MixConfig, MixId, SourceConfig, SourceId};

/// Channel layout used by every Pipedeck node. Stereo for now.
pub const AUDIO_POSITION: &str = "[ FL FR ]";
pub const CHANNELS: usize = 2;

/// A property value in the module args.
#[derive(Debug, Clone)]
pub enum Val {
    /// Quoted and escaped.
    Str(String),
    /// Emitted verbatim (arrays, booleans).
    Raw(String),
}

impl From<&str> for Val {
    fn from(s: &str) -> Self {
        Val::Str(s.to_owned())
    }
}

impl From<String> for Val {
    fn from(s: String) -> Self {
        Val::Str(s)
    }
}

impl From<bool> for Val {
    fn from(b: bool) -> Self {
        Val::Raw(b.to_string())
    }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_dict(out: &mut String, entries: &[(&str, Val)]) {
    out.push_str("{ ");
    for (k, v) in entries {
        out.push_str(k);
        out.push_str(" = ");
        match v {
            Val::Str(s) => out.push_str(&quote(s)),
            Val::Raw(r) => out.push_str(r),
        }
        out.push(' ');
    }
    out.push('}');
}

/// Description of one loopback: a capture stream and a playback stream.
#[derive(Debug, Clone)]
pub struct LoopbackSpec {
    pub capture: Vec<(&'static str, Val)>,
    pub playback: Vec<(&'static str, Val)>,
}

/// Properties every stream of ours carries.
///
/// WirePlumber's `state-stream.lua` tests these keys with a *string*
/// comparison against "false". Module args go through SPA JSON and end up as
/// the string "false", which is why `Val::from(false)` works here. If these
/// props ever get set another way (create_object, set_property, a future
/// WirePlumber parser change), make sure the value still reaches the node as
/// the string "false", otherwise the volume and target restore silently come
/// back and fight the user's faders.
fn common(name: String, latency: &str) -> Vec<(&'static str, Val)> {
    vec![
        ("node.name", Val::from(name)),
        ("pipedeck.instance", Val::from(crate::pw::instance())),
        ("audio.position", Val::Raw(AUDIO_POSITION.into())),
        ("node.latency", Val::from(latency)),
        ("node.dont-reconnect", Val::from(true)),
        ("state.restore-props", Val::from(false)),
        ("state.restore-target", Val::from(false)),
    ]
}

/// The capture side is classed like WirePlumber's own loopbacks so that
/// pipewire-pulse does not list it as a recording stream in pavucontrol.
fn capture_props(
    name: String,
    latency: &str,
    target: String,
    from_sink: bool,
) -> Vec<(&'static str, Val)> {
    let mut props = common(name, latency);
    props.push(("media.class", Val::from("Stream/Input/Audio/Internal")));
    if from_sink {
        props.push(("stream.capture.sink", Val::from(true)));
    }
    props.push(("target.object", Val::from(target)));
    props
}

impl LoopbackSpec {
    /// One cell of the matrix: the source feeds the mix.
    ///
    /// A virtual row is captured from the monitor ports of its sink, an input
    /// row straight from its device. Both end on the mix sink, so every cell
    /// is the same object with the same latency whatever the row is.
    pub fn for_link(source: &SourceConfig, mix: &MixConfig, latency: &str) -> Self {
        let node_name = link_node_name(source.id, mix.id);
        // A row ends on the last node of its chain, and that is the one
        // every mix should hear: the sink its effects play into, or, with
        // none, its own sink or the device it captures.
        let (target, from_sink) = crate::pw::channel_output(source);

        let mut capture = capture_props(format!("{node_name}.in"), latency, target, from_sink);
        capture.push((
            "node.description",
            Val::from(format!("Pipedeck: {} in", source.name)),
        ));

        let mut playback = common(node_name, latency);
        playback.push((
            "node.description",
            Val::from(format!("Pipedeck: {} to {}", source.name, mix.name)),
        ));
        // A column may be treated on the way in, so a cell ends on the node
        // the column starts with rather than on its sink.
        playback.push(("target.object", Val::from(crate::pw::mix_input(mix))));

        Self { capture, playback }
    }

    /// A mix as an input device: it reads the mix and *is* a source, rather
    /// than playing into one.
    ///
    /// A mix is something you record, so it belongs in the microphone list
    /// of whatever records it — OBS, Discord, a browser — and not in the
    /// list of things to play into. A monitor would do for the audio, but a
    /// monitor is not what those lists show. The playback side of this
    /// loopback carries `Audio/Source` and so is the device itself: nothing
    /// routes into it, which is just as well, since the session manager
    /// refuses to route anything into a source.
    ///
    /// `Audio/Source/Virtual`, which is what the documentation calls a
    /// source made up rather than found, segfaults libspa's audioconvert on
    /// PipeWire 1.6.8 when the other side of the loopback is a stream, and
    /// takes the mixer down with it. `Audio/Source` is listed the same way
    /// and survives.
    pub fn for_capture(mix: &MixConfig, latency: &str) -> Self {
        let node_name = mix.id.source_node_name();

        let mut capture = capture_props(
            format!("{node_name}.in"),
            latency,
            mix.id.sink_node_name(),
            true,
        );
        capture.push((
            "node.description",
            Val::from(format!("Pipedeck: {} capture", mix.name)),
        ));

        let mut playback = common(node_name, latency);
        playback.push(("media.class", Val::from("Audio/Source")));
        playback.push((
            "node.description",
            Val::from(format!("Pipedeck {}", mix.name)),
        ));
        Self { capture, playback }
    }

    /// One output of a mix: the mix sink feeds a device.
    pub fn for_output(mix: &MixConfig, index: usize, device: &str, latency: &str) -> Self {
        let node_name = output_node_name(mix.id, index);

        let mut capture = capture_props(
            format!("{node_name}.in"),
            latency,
            mix.id.sink_node_name(),
            true,
        );
        capture.push((
            "node.description",
            Val::from(format!("Pipedeck: {} out", mix.name)),
        ));

        let mut playback = common(node_name, latency);
        playback.push((
            "node.description",
            Val::from(format!("Pipedeck: {} output", mix.name)),
        ));
        playback.push(("target.object", Val::from(device)));

        Self { capture, playback }
    }

    /// Render the SPA JSON args expected by the module.
    pub fn to_args(&self) -> String {
        let mut out = String::from("{ ");
        out.push_str("capture.props = ");
        write_dict(&mut out, &self.capture);
        out.push_str(" playback.props = ");
        write_dict(&mut out, &self.playback);
        out.push_str(" }");
        out
    }
}

/// `node.name` of the playback node of a cell, the node carrying its fader.
pub fn link_node_name(source: SourceId, mix: MixId) -> String {
    format!("pipedeck.link.{source}.{mix}")
}

/// `node.name` of the playback node of one output of a mix.
pub fn output_node_name(mix: MixId, index: usize) -> String {
    format!("pipedeck.out.{mix}.{index}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix() -> MixConfig {
        MixConfig::new(MixId(2), "Stream Mix")
    }

    #[test]
    fn a_virtual_row_is_captured_from_its_sink() {
        let source = SourceConfig::virtual_sink(SourceId(3), "Ga\"me");
        let args = LoopbackSpec::for_link(&source, &mix(), "512/48000").to_args();
        assert!(args.starts_with("{ capture.props = { "));
        assert!(args.contains("node.name = \"pipedeck.link.3.2.in\""));
        assert!(args.contains("stream.capture.sink = true"));
        assert!(args.contains("target.object = \"pipedeck.src.3\""));
        assert!(args.contains("target.object = \"pipedeck.mix.2\""));
        assert!(args.contains("media.class = \"Stream/Input/Audio/Internal\""));
        assert!(args.contains("node.latency = \"512/48000\""));
        assert!(args.contains("node.description = \"Pipedeck: Ga\\\"me to Stream Mix\""));
        assert_eq!(args.matches("state.restore-props = false").count(), 2);
        assert!(args.ends_with("} }"));
    }

    #[test]
    fn a_cell_ends_on_what_the_mix_starts_with() {
        let source = SourceConfig::virtual_sink(SourceId(3), "Game");
        let mut treated = mix();
        treated.effects = vec![crate::types::Effect {
            name: "Low cut".into(),
            kind: crate::types::EffectKind::Builtin,
            plugin: None,
            label: "bq_highpass".into(),
            controls: Vec::new(),
        }];
        let args = LoopbackSpec::for_link(&source, &treated, "512/48000").to_args();
        assert!(args.contains("target.object = \"pipedeck.mixfx.2\""));
        assert!(!args.contains("target.object = \"pipedeck.mix.2\""));
    }

    #[test]
    fn an_input_row_is_captured_from_its_device() {
        let source = SourceConfig::input(SourceId(1), "Mic", "alsa_input.usb");
        let spec = LoopbackSpec::for_link(&source, &mix(), "512/48000");
        let args = spec.to_args();
        assert!(args.contains("target.object = \"alsa_input.usb\""));
        assert!(!args.contains("stream.capture.sink"));
        assert_eq!(link_node_name(SourceId(1), MixId(2)), "pipedeck.link.1.2");
    }

    #[test]
    fn a_mix_is_an_input_device_of_its_own() {
        let args = LoopbackSpec::for_capture(&mix(), "512/48000").to_args();
        // It reads the mix, and what it offers is a source rather than
        // something that plays into one.
        assert!(args.contains("node.name = \"pipedeck.in.2.in\""));
        assert!(args.contains("stream.capture.sink = true"));
        assert!(args.contains("target.object = \"pipedeck.mix.2\""));
        assert!(args.contains("node.name = \"pipedeck.in.2\""));
        assert!(args.contains("media.class = \"Audio/Source\""));
        // Nothing routes into a source, so it names no target of its own.
        assert_eq!(args.matches("target.object").count(), 1);
        // What the microphone list shows.
        assert!(args.contains("node.description = \"Pipedeck Stream Mix\""));
    }

    #[test]
    fn an_output_goes_from_the_mix_sink_to_a_device() {
        let args = LoopbackSpec::for_output(&mix(), 0, "alsa_output.usb", "512/48000").to_args();
        assert!(args.contains("node.name = \"pipedeck.out.2.0.in\""));
        assert!(args.contains("stream.capture.sink = true"));
        assert!(args.contains("target.object = \"pipedeck.mix.2\""));
        assert!(args.contains("target.object = \"alsa_output.usb\""));
        assert_eq!(output_node_name(MixId(2), 1), "pipedeck.out.2.1");
    }
}
