//! TOML persistence of the mixer state.
//!
//! Location: `$XDG_CONFIG_HOME/pipedeck/config.toml`, falling back to
//! `$HOME/.config/pipedeck/config.toml`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{ChainState, LinkConfig, MixConfig, MixId, SourceConfig, SourceId};

/// Quantum requested on our own nodes, as `frames/rate`.
///
/// Audio crosses two of our nodes on its way from an application to a device
/// (one for the cell, one for the mix output), so the default asks for half
/// of PipeWire's usual 1024 to keep the round trip in the same ballpark as a
/// single default-sized hop. Raise it if the machine reports xruns.
pub const DEFAULT_LATENCY: &str = "512/48000";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cannot serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("neither $XDG_CONFIG_HOME nor $HOME is set")]
    NoConfigDir,
}

/// The whole mixer: columns, rows, and the cells joining them.
///
/// Scalars come first: TOML requires plain values before arrays of tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Quantum requested on our nodes, see [`DEFAULT_LATENCY`].
    #[serde(default = "default_latency")]
    pub latency: String,
    /// Licence key for Stereo Tool, which the mixer only passes on to the
    /// library. Without one it still runs, and puts speech and beeps in the
    /// audio, which is the vendor's doing and not something to work around.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stereotool_license: Option<String>,
    /// The device you listen on — your headphones — which the sound card
    /// switch changes. A mix is heard there when it has an output to it that
    /// is switched on; that is what the ear on its card says and sets. None
    /// until one is picked, and then the first device a mix plays to stands
    /// in for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen_device: Option<String>,
    #[serde(default, rename = "mix")]
    pub mixes: Vec<MixConfig>,
    #[serde(default, rename = "source")]
    pub sources: Vec<SourceConfig>,
    #[serde(default, rename = "link")]
    pub links: Vec<LinkConfig>,
    /// True when this config was just converted from an older layout, so the
    /// engine knows to write it back in the current shape. Never stored.
    #[serde(skip)]
    pub migrated: bool,
}

fn default_latency() -> String {
    DEFAULT_LATENCY.to_owned()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            latency: default_latency(),
            stereotool_license: None,
            listen_device: None,
            mixes: Vec::new(),
            sources: Vec::new(),
            links: Vec::new(),
            migrated: false,
        }
    }
}

/// The pre-matrix layout: sources carrying one fader per fixed bus.
#[derive(Debug, Deserialize)]
struct LegacyConfig {
    #[serde(default, rename = "source")]
    sources: Vec<LegacySource>,
}

#[derive(Debug, Deserialize)]
struct LegacySource {
    id: SourceId,
    name: String,
    stream: Option<ChainState>,
    monitor: Option<ChainState>,
}

impl LegacyConfig {
    /// Is this the layout of before the matrix? Its sources carry the faders
    /// of the two buses it had, which no source has had since. A config of
    /// today with every mix deleted is not one, and is left as it is.
    fn is_legacy(&self) -> bool {
        !self.sources.is_empty()
            && self
                .sources
                .iter()
                .any(|source| source.stream.is_some() || source.monitor.is_some())
    }
}

impl Config {
    /// Default config file path following the XDG base directory spec.
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or(ConfigError::NoConfigDir)?;
        Ok(base.join("pipedeck").join("config.toml"))
    }

    /// Load the config, returning a fresh one when the file does not exist
    /// and converting a pre-matrix file when it finds one.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::fresh()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                })
            }
        };
        let config: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })?;
        if config.mixes.is_empty() && config.links.is_empty() {
            if let Ok(legacy) = toml::from_str::<LegacyConfig>(&text) {
                if legacy.is_legacy() {
                    log::info!("converting the pre-matrix config at {}", path.display());
                    return Ok(Self::from_legacy(legacy, config.latency));
                }
            }
        }
        Ok(config)
    }

    /// A brand new config: one empty mix, no source. The mix has no output
    /// until the user attaches a device to it, and is named the way any
    /// first mix is.
    pub fn fresh() -> Self {
        let (name, icon) = crate::types::new_mix(0);
        let mut first = MixConfig::new(MixId(1), name);
        first.icon = icon;
        Self {
            mixes: vec![first],
            ..Self::default()
        }
    }

    /// Turn the two fixed buses of the previous layout into two mixes, with
    /// one link per source per bus carrying the fader it had.
    fn from_legacy(legacy: LegacyConfig, latency: String) -> Self {
        let stream = MixId(1);
        let monitor = MixId(2);
        let mut links = Vec::with_capacity(legacy.sources.len() * 2);
        let mut sources = Vec::with_capacity(legacy.sources.len());
        for source in legacy.sources {
            for (mix, state) in [
                (stream, source.stream.unwrap_or_default()),
                (monitor, source.monitor.unwrap_or_default()),
            ] {
                let mut link = LinkConfig::new(source.id, mix);
                link.set_state(state);
                links.push(link);
            }
            sources.push(SourceConfig::virtual_sink(source.id, source.name));
        }
        Self {
            stereotool_license: None,
            listen_device: None,
            latency,
            mixes: vec![
                MixConfig::new(stream, "Stream Mix"),
                MixConfig::new(monitor, "Monitor"),
            ],
            sources,
            links,
            migrated: true,
        }
    }

    /// Atomically write the config (temp file + rename), creating the parent
    /// directory when needed.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self)?;
        let write_err = |source| ConfigError::Write {
            path: path.to_owned(),
            source,
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(write_err)?;
        }
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(write_err)?;
        fs::rename(&tmp, path).map_err(write_err)
    }

    /// One past the highest id in use. Ids are never reused: they name graph
    /// objects and may still be referenced by a session manager's state.
    pub fn next_source_id(&self) -> SourceId {
        SourceId(self.sources.iter().map(|s| s.id.0 + 1).max().unwrap_or(1))
    }

    pub fn next_mix_id(&self) -> MixId {
        MixId(self.mixes.iter().map(|m| m.id.0 + 1).max().unwrap_or(1))
    }

    pub fn source(&self, id: SourceId) -> Option<&SourceConfig> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn source_mut(&mut self, id: SourceId) -> Option<&mut SourceConfig> {
        self.sources.iter_mut().find(|s| s.id == id)
    }

    pub fn mix(&self, id: MixId) -> Option<&MixConfig> {
        self.mixes.iter().find(|m| m.id == id)
    }

    pub fn mix_mut(&mut self, id: MixId) -> Option<&mut MixConfig> {
        self.mixes.iter_mut().find(|m| m.id == id)
    }

    pub fn link(&self, source: SourceId, mix: MixId) -> Option<&LinkConfig> {
        self.links
            .iter()
            .find(|l| l.source == source && l.mix == mix)
    }

    pub fn link_mut(&mut self, source: SourceId, mix: MixId) -> Option<&mut LinkConfig> {
        self.links
            .iter_mut()
            .find(|l| l.source == source && l.mix == mix)
    }

    /// Drop every link that no longer has both ends.
    /// Put a mix at another place among the columns. The order is only how
    /// the matrix is drawn: nothing on the graph moves.
    pub fn move_mix(&mut self, id: MixId, to: usize) -> bool {
        let Some(from) = self.mixes.iter().position(|mix| mix.id == id) else {
            return false;
        };
        move_within(&mut self.mixes, from, to);
        true
    }

    /// Put a row at another place among the rows. Same as for a mix.
    pub fn move_source(&mut self, id: SourceId, to: usize) -> bool {
        let Some(from) = self.sources.iter().position(|source| source.id == id) else {
            return false;
        };
        move_within(&mut self.sources, from, to);
        true
    }

    pub fn prune_links(&mut self) {
        let sources: Vec<SourceId> = self.sources.iter().map(|s| s.id).collect();
        let mixes: Vec<MixId> = self.mixes.iter().map(|m| m.id).collect();
        self.links
            .retain(|l| sources.contains(&l.source) && mixes.contains(&l.mix));
    }
}

/// Take the item at `from` out and put it back at `to`, so that it ends up
/// there whichever side of it `to` was: dropping a card on another puts it
/// where that one stood.
fn move_within<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from >= items.len() {
        return;
    }
    let item = items.remove(from);
    items.insert(to.min(items.len()), item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MixOutput;

    #[test]
    fn roundtrip_toml() {
        let mut cfg = Config::fresh();
        cfg.sources
            .push(SourceConfig::virtual_sink(SourceId(1), "Game"));
        cfg.sources
            .push(SourceConfig::input(SourceId(2), "Mic", "alsa_input.x"));
        cfg.mixes[0].outputs.push(MixOutput::new("alsa_output.y"));
        cfg.mixes[0].gain = 0.7;
        let mut link = LinkConfig::new(SourceId(1), MixId(1));
        link.gain = 0.5;
        link.muted = true;
        cfg.links.push(link);
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("[[mix]]"), "{text}");
        assert!(text.contains("[[link]]"), "{text}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
    }

    #[test]
    fn a_config_with_every_mix_deleted_is_not_taken_for_an_old_one() {
        let mut cfg = Config::default();
        cfg.sources
            .push(SourceConfig::input(SourceId(2), "Mic", "alsa_input.x"));
        cfg.stereotool_license = Some("KEY".into());
        let dir = std::env::temp_dir().join(format!("pipedeck-config-{}", std::process::id()));
        let path = dir.join("config.toml");
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!loaded.migrated);
        assert!(loaded.mixes.is_empty());
        assert_eq!(loaded.sources, cfg.sources);
        assert_eq!(loaded.stereotool_license.as_deref(), Some("KEY"));
    }

    #[test]
    fn ids_are_never_reused() {
        let mut cfg = Config::default();
        assert_eq!(cfg.next_source_id(), SourceId(1));
        cfg.sources
            .push(SourceConfig::virtual_sink(SourceId(4), "a"));
        cfg.sources
            .push(SourceConfig::virtual_sink(SourceId(2), "b"));
        assert_eq!(cfg.next_source_id(), SourceId(5));
        assert_eq!(cfg.next_mix_id(), MixId(1));
    }

    #[test]
    fn an_output_written_as_a_bare_name_still_loads() {
        let text = r#"
[[mix]]
id = 1
name = "Monitor"
outputs = ["alsa_output.legacy"]
"#;
        let cfg: Config = toml::from_str(text).unwrap();
        assert_eq!(
            cfg.mixes[0].outputs,
            vec![MixOutput::new("alsa_output.legacy")]
        );
        assert_eq!(cfg.mixes[0].gain, 1.0);
    }

    #[test]
    fn a_pre_matrix_file_becomes_two_mixes() {
        let legacy = r#"
[[source]]
id = 1
name = "Game"

[source.stream]
gain = 0.8
muted = false

[source.monitor]
gain = 0.6
muted = true
"#;
        let dir = std::env::temp_dir().join(format!("pipedeck-migrate-{}", std::process::id()));
        let path = dir.join("config.toml");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, legacy).unwrap();

        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.mixes.len(), 2);
        assert_eq!(cfg.mixes[0].name, "Stream Mix");
        assert!(cfg.mixes[0].outputs.is_empty());
        assert_eq!(cfg.sources.len(), 1);
        assert!(!cfg.sources[0].is_input());
        assert!(cfg.migrated, "a converted config must be written back");
        let stream = cfg.link(SourceId(1), MixId(1)).unwrap();
        assert_eq!(stream.gain, 0.8);
        let monitor = cfg.link(SourceId(1), MixId(2)).unwrap();
        assert!(monitor.muted);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_drops_orphan_links() {
        let mut cfg = Config::fresh();
        cfg.sources
            .push(SourceConfig::virtual_sink(SourceId(1), "Game"));
        cfg.links.push(LinkConfig::new(SourceId(1), MixId(1)));
        cfg.links.push(LinkConfig::new(SourceId(9), MixId(1)));
        cfg.links.push(LinkConfig::new(SourceId(1), MixId(9)));
        cfg.prune_links();
        assert_eq!(cfg.links.len(), 1);
    }

    #[test]
    fn a_channel_reads_back_its_effects() {
        let text = r#"
[[source]]
id = 1
name = "Voice"

[[source.effects]]
name = "Low cut"
kind = "builtin"
label = "bq_highpass"

[[source.effects.controls]]
name = "Freq"
value = 90.0
"#;
        let cfg: Config = toml::from_str(text).expect("the file parses");
        let effects = &cfg.sources[0].effects;
        assert_eq!(effects.len(), 1, "{cfg:?}");
        assert_eq!(effects[0].label, "bq_highpass");
        assert_eq!(effects[0].controls[0].value, 90.0);
    }

    #[test]
    fn the_first_mix_is_named_like_any_first_mix() {
        let cfg = Config::fresh();
        assert_eq!(cfg.mixes[0].name, crate::types::NEW_MIXES[0].0);
        assert_eq!(
            cfg.mixes[0].icon.as_deref(),
            Some(crate::types::NEW_MIXES[0].1)
        );
    }

    #[test]
    fn load_missing_file_is_fresh() {
        let dir = std::env::temp_dir().join(format!("pipedeck-fresh-{}", std::process::id()));
        let path = dir.join("nope").join("config.toml");
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.mixes.len(), 1);
        assert!(cfg.sources.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_column_or_a_row_lands_where_it_was_dropped() {
        let mut cfg = Config::fresh();
        cfg.mixes = (1..=4)
            .map(|id| MixConfig::new(MixId(id), format!("m{id}")))
            .collect();
        let order = |cfg: &Config| cfg.mixes.iter().map(|m| m.id.0).collect::<Vec<_>>();

        // Rightwards, onto the third: it takes the third place.
        assert!(cfg.move_mix(MixId(1), 2));
        assert_eq!(order(&cfg), [2, 3, 1, 4]);
        // Leftwards, onto the first.
        assert!(cfg.move_mix(MixId(4), 0));
        assert_eq!(order(&cfg), [4, 2, 3, 1]);
        // Past the end is the end.
        assert!(cfg.move_mix(MixId(4), 99));
        assert_eq!(order(&cfg), [2, 3, 1, 4]);
        // A mix that is not there moves nothing.
        assert!(!cfg.move_mix(MixId(9), 0));

        cfg.sources = (1..=3)
            .map(|id| SourceConfig::virtual_sink(SourceId(id), format!("s{id}")))
            .collect();
        assert!(cfg.move_source(SourceId(3), 0));
        assert_eq!(
            cfg.sources.iter().map(|s| s.id.0).collect::<Vec<_>>(),
            [3, 1, 2]
        );
    }
}
