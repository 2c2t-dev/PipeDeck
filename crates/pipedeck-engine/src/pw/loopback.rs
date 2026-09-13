//! Argument string builder for `libpipewire-module-loopback`.
//!
//! The module takes its configuration as a SPA JSON dictionary. We only ever
//! need a handful of keys, so this is a tiny serializer rather than a
//! dependency on a JSON crate.

use crate::types::{MixBus, SourceId};

/// Channel layout used by every Pipedeck node. Stereo for the MVP.
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

impl LoopbackSpec {
    /// The loopback for one gain chain of one source.
    ///
    /// Both chains are strictly identical: they capture the monitor ports of
    /// the source's null sink and play back to their bus target. The stream
    /// chain targets the Stream Mix sink; the monitor chain leaves
    /// `target.object` unset so it follows the default output device.
    ///
    /// The capture side is classed `Stream/Input/Audio/Internal`, the
    /// convention WirePlumber uses for its own loopbacks, so pipewire-pulse
    /// does not list it as a recording stream. `state.restore-props` keeps
    /// WirePlumber from restoring a saved volume over ours, and
    /// `state.restore-target` keeps it from re-routing the stream chain.
    pub fn for_chain(id: SourceId, source_name: &str, bus: MixBus, stream_mix_node: &str) -> Self {
        let sink = id.sink_node_name();
        let playback_name = playback_node_name(id, bus);
        let capture_name = format!("{playback_name}.in");

        // WirePlumber's state-stream.lua checks these keys with
        // `stream_props["state.restore-props"] ~= "false"`: a *string*
        // comparison against "false". Module args go through SPA JSON and
        // end up as the string "false", which is why `Val::from(false)` works
        // here. If these props are ever set another way (create_object,
        // set_property, a future WirePlumber parser change), make sure the
        // value still reaches the node as the string "false", otherwise the
        // volume/target restore silently comes back.
        let capture = vec![
            ("node.name", Val::from(capture_name)),
            (
                "node.description",
                Val::from(format!("Pipedeck: {source_name} ({}) in", bus.label())),
            ),
            ("media.class", Val::from("Stream/Input/Audio/Internal")),
            ("stream.capture.sink", Val::from(true)),
            ("target.object", Val::from(sink)),
            ("node.dont-reconnect", Val::from(true)),
            ("state.restore-props", Val::from(false)),
            ("audio.position", Val::Raw(AUDIO_POSITION.into())),
        ];

        let mut playback = vec![
            ("node.name", Val::from(playback_name)),
            (
                "node.description",
                Val::from(format!("Pipedeck: {source_name} → {}", bus.label())),
            ),
            ("audio.position", Val::Raw(AUDIO_POSITION.into())),
            ("state.restore-props", Val::from(false)),
        ];
        if bus == MixBus::Stream {
            playback.push(("target.object", Val::from(stream_mix_node)));
            playback.push(("node.dont-reconnect", Val::from(true)));
            playback.push(("state.restore-target", Val::from(false)));
        }

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

/// `node.name` of the playback stream of one chain, the node carrying the
/// fader volume (e.g. `pipedeck.3.stream`).
pub fn playback_node_name(id: SourceId, bus: MixBus) -> String {
    format!("{}.{}", id.sink_node_name(), bus.suffix())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_valid_spa_json_dicts() {
        let spec =
            LoopbackSpec::for_chain(SourceId(3), "Ga\"me", MixBus::Stream, "pipedeck.stream_mix");
        let args = spec.to_args();
        assert!(args.starts_with("{ capture.props = { "));
        assert!(args.contains("node.name = \"pipedeck.3.stream.in\""));
        assert!(args.contains("stream.capture.sink = true"));
        assert!(args.contains("target.object = \"pipedeck.3\""));
        assert!(args.contains("node.description = \"Pipedeck: Ga\\\"me → Stream\""));
        assert!(args.contains("audio.position = [ FL FR ]"));
        assert!(args.contains("target.object = \"pipedeck.stream_mix\""));
        assert!(args.contains("media.class = \"Stream/Input/Audio/Internal\""));
        assert!(args.contains("state.restore-target = false"));
        assert_eq!(args.matches("state.restore-props = false").count(), 2);
        assert!(args.ends_with("} }"));
    }

    #[test]
    fn monitor_chain_follows_default_sink() {
        let spec =
            LoopbackSpec::for_chain(SourceId(1), "Mic", MixBus::Monitor, "pipedeck.stream_mix");
        assert!(!spec.playback.iter().any(|(k, _)| *k == "target.object"));
        assert!(!spec
            .playback
            .iter()
            .any(|(k, _)| *k == "state.restore-target"));
        assert_eq!(
            playback_node_name(SourceId(1), MixBus::Monitor),
            "pipedeck.1.monitor"
        );
    }
}
