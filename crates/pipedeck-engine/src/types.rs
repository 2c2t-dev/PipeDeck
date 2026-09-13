//! Plain data types shared between the engine and its clients.
//!
//! Nothing in here touches PipeWire or GTK: these types travel through the
//! command/event channels and are persisted verbatim in the TOML config.

use serde::{Deserialize, Serialize};

/// Stable identifier of a source. Persisted in the config, never reused
/// during the lifetime of a config file, and used to derive node names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub u32);

impl SourceId {
    /// `node.name` of the virtual sink backing this source (e.g. `pipedeck.3`).
    ///
    /// Node names derive from the id, not from the user-facing name, so that
    /// renaming a source or creating two sources with the same name never
    /// changes or collides the graph objects.
    pub fn sink_node_name(self) -> String {
        format!("pipedeck.{}", self.0)
    }
}

impl std::fmt::Display for SourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One of the two output mixes every source feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixBus {
    /// Mixed into the "Pipedeck Stream Mix" sink, meant to be captured by OBS.
    Stream,
    /// Played on the default physical output, what the user hears.
    Monitor,
}

impl MixBus {
    pub const ALL: [MixBus; 2] = [MixBus::Stream, MixBus::Monitor];

    /// Short lowercase suffix used in node names.
    pub fn suffix(self) -> &'static str {
        match self {
            MixBus::Stream => "stream",
            MixBus::Monitor => "monitor",
        }
    }

    /// Human readable label.
    pub fn label(self) -> &'static str {
        match self {
            MixBus::Stream => "Stream",
            MixBus::Monitor => "Monitor",
        }
    }
}

/// Gain chain state for one source on one bus.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChainState {
    /// Fader position in `0.0..=1.0`. This is the "cubic" scale users expect
    /// from a mixer (like PulseAudio percentages), converted to a linear
    /// amplitude with [`ChainState::linear_volume`] before reaching PipeWire.
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

/// Everything the engine needs to (re)create one source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceConfig {
    pub id: SourceId,
    pub name: String,
    #[serde(default)]
    pub stream: ChainState,
    #[serde(default)]
    pub monitor: ChainState,
}

impl SourceConfig {
    pub fn new(id: SourceId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            stream: ChainState::default(),
            monitor: ChainState::default(),
        }
    }

    pub fn chain(&self, bus: MixBus) -> &ChainState {
        match bus {
            MixBus::Stream => &self.stream,
            MixBus::Monitor => &self.monitor,
        }
    }

    pub fn chain_mut(&mut self, bus: MixBus) -> &mut ChainState {
        match bus {
            MixBus::Stream => &mut self.stream,
            MixBus::Monitor => &mut self.monitor,
        }
    }
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
        let over = ChainState {
            gain: 3.0,
            muted: false,
        };
        assert_eq!(over.linear_volume(), 1.0);
        let under = ChainState {
            gain: -1.0,
            muted: false,
        };
        assert_eq!(under.linear_volume(), 0.0);
    }

    #[test]
    fn node_name_derives_from_id() {
        assert_eq!(SourceId(7).sink_node_name(), "pipedeck.7");
    }
}
