//! Plain data types shared between the engine and its clients.
//!
//! The mixer is a matrix: sources are rows, mixes are columns, and a link is
//! one cell carrying its own fader and mute. Nothing in here touches PipeWire
//! or GTK; these types travel through the command/event channels and are
//! persisted verbatim in the TOML config.

use serde::{Deserialize, Serialize};

/// Hard cap on the number of mixes, mirroring what hardware mixers expose.
pub const MAX_MIXES: usize = 5;

/// What each mix is called when it is created, by rank, with the look that
/// goes with it.
///
/// A mixer's columns are the same handful every time, so asking for a name
/// on the way in would be a dialog in the way of a button. Naming happens
/// here rather than in the interface because only this side knows how many
/// mixes there already are: four quick clicks would otherwise all read the
/// same stale count and produce the same name. The look is an opaque key,
/// meaningless to the engine, passed on to whoever draws the mixer.
pub const NEW_MIXES: [(&str, &str); MAX_MIXES] = [
    ("Personal Mix", "headset"),
    ("Chat Mix", "voice"),
    ("Stream Mix", "stream"),
    ("Record Mix", "record"),
    ("Aux Mix", "aux1"),
];

/// The name and look for the mix about to be created.
pub fn new_mix(existing: usize) -> (String, Option<String>) {
    match NEW_MIXES.get(existing) {
        Some((name, look)) => (name.to_string(), Some(look.to_string())),
        // Past the list, which the cap makes unreachable, a plain name still
        // beats no mix at all.
        None => (format!("Mix {}", existing + 1), None),
    }
}

/// Stable identifier of a source (a row). Persisted, never reused, and used
/// to derive node names so renaming a source never touches the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub u32);

/// Stable identifier of a mix (a column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MixId(pub u32);

impl SourceId {
    /// `node.name` of the virtual sink backing this source.
    pub fn sink_node_name(self) -> String {
        format!("pipedeck.src.{}", self.0)
    }

    /// `node.name` of the sink carrying this source once its plug-ins have
    /// run, which is what the cells read when it has any.
    pub fn plugins_node_name(self) -> String {
        format!("pipedeck.vst.{}", self.0)
    }

    /// `node.name` of the sink carrying this source once its effects have
    /// been applied. Cells capture this one instead when there are any.
    pub fn effects_node_name(self) -> String {
        format!("pipedeck.fx.{}", self.0)
    }
}

impl MixId {
    /// `node.name` of the sink this mix collects into. This is the node a
    /// capture client such as OBS picks, and the last of the column: what
    /// the mix runs happens before it, so a capture client hears the treated
    /// signal and not the raw one.
    pub fn sink_node_name(self) -> String {
        format!("pipedeck.mix.{}", self.0)
    }
}

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for MixId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A row of the matrix.
///
/// A source with no `device` is a virtual sink applications can select. A
/// source with a `device` captures that node instead, which is how a
/// microphone becomes a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceConfig {
    pub id: SourceId,
    pub name: String,
    /// Trim applied to the row's sink before any cell. The sink passes it on
    /// to its monitor ports, so it scales every mix this row feeds at once.
    /// An input row has no sink and ignores it.
    #[serde(default = "unity_gain")]
    pub gain: f32,
    #[serde(default)]
    pub muted: bool,
    /// `node.name` of the capture device, for an input row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Which look the interface gives this row. The engine only carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Effects applied to this row, in order, before any mix hears it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// Applications whose audio belongs to this row. An application is
    /// matched by [`App::key`], and its streams are moved onto the row's sink
    /// as they appear. An input row has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
}

impl Effect {
    /// Is the other the same effect, whatever its settings? Two chains that
    /// are the same effect for effect differ only in settings, which a
    /// running chain can take without being made again.
    pub fn same_effect(&self, other: &Effect) -> bool {
        self.kind == other.kind && self.label == other.label && self.plugin == other.plugin
    }

    /// Is this one hosted by the mixer rather than by PipeWire?
    pub fn is_plugin(&self) -> bool {
        matches!(
            self.kind,
            EffectKind::Vst3 | EffectKind::StereoTool | EffectKind::Native
        )
    }

    /// The preset file a Stereo Tool stage was given, if it has one. It is
    /// kept in `plugin`, which is where a filter's library would go.
    pub fn preset(&self) -> Option<&str> {
        (self.kind == EffectKind::StereoTool).then_some(self.plugin.as_deref())?
    }
}

impl SourceConfig {
    pub fn virtual_sink(id: SourceId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            gain: 1.0,
            muted: false,
            device: None,
            icon: None,
            effects: Vec::new(),
            apps: Vec::new(),
        }
    }

    pub fn input(id: SourceId, name: impl Into<String>, device: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            gain: 1.0,
            muted: false,
            device: Some(device.into()),
            icon: None,
            effects: Vec::new(),
            apps: Vec::new(),
        }
    }

    pub fn state(&self) -> ChainState {
        ChainState {
            gain: self.gain,
            muted: self.muted,
        }
    }

    pub fn set_state(&mut self, state: ChainState) {
        self.gain = state.gain;
        self.muted = state.muted;
    }

    /// True when this row captures a device instead of exposing a sink.
    /// Give this row a look, by preset key.
    pub fn with_icon(mut self, icon: Option<String>) -> Self {
        self.icon = icon;
        self
    }

    pub fn is_input(&self) -> bool {
        self.device.is_some()
    }
}

/// Fader state of one cell of the matrix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChainState {
    /// Fader position in `0.0..=1.0`, on the cubic scale users expect from a
    /// mixer. Converted to a linear amplitude by [`ChainState::linear_volume`].
    pub gain: f32,
    pub muted: bool,
}

impl Default for ChainState {
    fn default() -> Self {
        Self {
            gain: 1.0,
            muted: false,
        }
    }
}

impl ChainState {
    /// Linear amplitude to write into `channelVolumes`.
    pub fn linear_volume(&self) -> f32 {
        self.gain.clamp(0.0, 1.0).powi(3)
    }
}

pub(crate) fn unity_gain() -> f32 {
    1.0
}

fn switched_on() -> bool {
    true
}

/// One device a mix plays to, with its own level.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MixOutput {
    /// `node.name` of the device.
    pub device: String,
    pub gain: f32,
    pub muted: bool,
    /// Whether the mix plays there at all. An output switched off stays in
    /// the list, with its level, and lets go of the device: switching sound
    /// cards is switching one off and another on, not forgetting either.
    pub enabled: bool,
}

impl MixOutput {
    pub fn new(device: impl Into<String>) -> Self {
        Self {
            device: device.into(),
            gain: 1.0,
            muted: false,
            enabled: true,
        }
    }

    pub fn state(&self) -> ChainState {
        ChainState {
            gain: self.gain,
            muted: self.muted,
        }
    }

    pub fn set_state(&mut self, state: ChainState) {
        self.gain = state.gain;
        self.muted = state.muted;
    }
}

/// Accepts both the table an output is written as today and the bare device
/// name earlier versions wrote, so a config keeps loading across upgrades.
impl<'de> Deserialize<'de> for MixOutput {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Name(String),
            Full {
                device: String,
                #[serde(default = "unity_gain")]
                gain: f32,
                #[serde(default)]
                muted: bool,
                // Every output written before there was a switch was on.
                #[serde(default = "switched_on")]
                enabled: bool,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Name(device) => MixOutput::new(device),
            Repr::Full {
                device,
                gain,
                muted,
                enabled,
            } => MixOutput {
                device,
                gain,
                muted,
                enabled,
            },
        })
    }
}

/// A column of the matrix. A mix starts with no output: it exists as a sink
/// a capture client can read, and the user attaches devices to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MixConfig {
    pub id: MixId,
    pub name: String,
    /// Which look the interface gives this column. The engine only carries
    /// it, the same way it carries a row's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Master level, applied to the mix sink, so it scales what the outputs
    /// play and what a capture client reads alike.
    #[serde(default = "unity_gain")]
    pub gain: f32,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub outputs: Vec<MixOutput>,
}

impl MixConfig {
    pub fn new(id: MixId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            icon: None,
            gain: 1.0,
            muted: false,
            outputs: Vec::new(),
        }
    }

    pub fn state(&self) -> ChainState {
        ChainState {
            gain: self.gain,
            muted: self.muted,
        }
    }

    pub fn set_state(&mut self, state: ChainState) {
        self.gain = state.gain;
        self.muted = state.muted;
    }

    pub fn output(&self, device: &str) -> Option<&MixOutput> {
        self.outputs.iter().find(|o| o.device == device)
    }
}

/// One cell of the matrix. Its presence means the source feeds the mix.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LinkConfig {
    pub source: SourceId,
    pub mix: MixId,
    #[serde(default = "unity_gain")]
    pub gain: f32,
    #[serde(default)]
    pub muted: bool,
}

impl LinkConfig {
    pub fn new(source: SourceId, mix: MixId) -> Self {
        Self {
            source,
            mix,
            gain: 1.0,
            muted: false,
        }
    }

    pub fn state(&self) -> ChainState {
        ChainState {
            gain: self.gain,
            muted: self.muted,
        }
    }

    pub fn set_state(&mut self, state: ChainState) {
        self.gain = state.gain;
        self.muted = state.muted;
    }
}

/// One control of an effect, by the name the plugin gives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    pub name: String,
    pub value: f32,
}

/// Where an effect comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectKind {
    /// Shipped with PipeWire, nothing to install.
    Builtin,
    Lv2,
    Ladspa,
    /// A VST3 plug-in installed on the machine, hosted by the mixer itself.
    Vst3,
    /// Thimeo's Stereo Tool, hosted by the mixer through the shared library
    /// the vendor ships for exactly that.
    StereoTool,
    /// One of the effects the mixer runs itself, by its id in
    /// [`crate::dsp::EFFECTS`], which the effect keeps as its `label`.
    Native,
}

impl EffectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EffectKind::Builtin => "builtin",
            EffectKind::Lv2 => "lv2",
            EffectKind::Ladspa => "ladspa",
            EffectKind::Vst3 => "vst3",
            EffectKind::StereoTool => "stereotool",
            EffectKind::Native => "native",
        }
    }
}

/// One effect in a channel's chain.
///
/// This is what PipeWire's filter chain needs to load it, not what the
/// interface calls it: the engine passes `name` through for display and uses
/// the rest to build the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub name: String,
    pub kind: EffectKind,
    /// The LV2 URI or the LADSPA library; unused by a builtin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// Which filter inside that plugin. For a VST3 plug-in this is the
    /// class id, which is what its factory is asked for.
    pub label: String,
    #[serde(default)]
    pub controls: Vec<Control>,
}

/// An application playing audio, as offered to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    /// What an assignment matches on, stable across runs of the application:
    /// its binary when the server knows it, its name otherwise.
    pub key: String,
    /// What to show.
    pub name: String,
    /// Icon name the application advertises, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// An audio device the user can attach to a mix or turn into a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// `node.name`, stable across reboots, what we store in the config.
    pub name: String,
    /// `node.description`, what we show.
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_volume_is_cubic_and_clamped() {
        let half = ChainState {
            gain: 0.5,
            muted: false,
        };
        assert!((half.linear_volume() - 0.125).abs() < 1e-6);
        assert_eq!(
            ChainState {
                gain: 3.0,
                muted: false
            }
            .linear_volume(),
            1.0
        );
        assert_eq!(
            ChainState {
                gain: -1.0,
                muted: false
            }
            .linear_volume(),
            0.0
        );
    }

    #[test]
    fn node_names_derive_from_ids() {
        assert_eq!(SourceId(7).sink_node_name(), "pipedeck.src.7");
        assert_eq!(MixId(2).sink_node_name(), "pipedeck.mix.2");
    }

    #[test]
    fn a_stereo_tool_stage_is_hosted_and_carries_its_preset() {
        let mut effect = Effect {
            name: "Stereo Tool".into(),
            kind: EffectKind::StereoTool,
            plugin: None,
            label: "stereotool".into(),
            controls: Vec::new(),
        };
        assert!(effect.is_plugin());
        assert_eq!(effect.preset(), None);
        effect.plugin = Some("/home/someone/fm.sts".into());
        assert_eq!(effect.preset(), Some("/home/someone/fm.sts"));

        let builtin = Effect {
            kind: EffectKind::Builtin,
            plugin: Some("not a preset".into()),
            ..effect
        };
        assert!(!builtin.is_plugin());
        assert_eq!(builtin.preset(), None);
    }

    #[test]
    fn an_output_reads_from_a_name_or_a_table() {
        let from_name: MixOutput = toml::from_str("value = \"alsa.x\"")
            .map(|v: toml::Value| v["value"].clone())
            .and_then(MixOutput::deserialize)
            .unwrap();
        assert_eq!(from_name, MixOutput::new("alsa.x"));

        let mut full = MixOutput::new("alsa.y");
        full.gain = 0.4;
        full.muted = true;
        let text = toml::to_string(&full).unwrap();
        assert_eq!(toml::from_str::<MixOutput>(&text).unwrap(), full);
    }

    #[test]
    fn an_output_written_before_the_switch_is_on() {
        let old: MixOutput =
            toml::from_str("device = \"alsa.x\"\ngain = 0.5\nmuted = false").unwrap();
        assert!(old.enabled);
        let mut off = MixOutput::new("alsa.y");
        off.enabled = false;
        let text = toml::to_string(&off).unwrap();
        assert_eq!(toml::from_str::<MixOutput>(&text).unwrap(), off);
    }

    #[test]
    fn an_input_row_keeps_its_device() {
        let mic = SourceConfig::input(SourceId(2), "Mic", "alsa_input.x");
        assert!(mic.is_input());
        let text = toml::to_string(&mic).unwrap();
        assert_eq!(toml::from_str::<SourceConfig>(&text).unwrap(), mic);
        let virt = SourceConfig::virtual_sink(SourceId(1), "Game");
        assert!(!virt.is_input());
        assert!(!toml::to_string(&virt).unwrap().contains("device"));
    }
}
