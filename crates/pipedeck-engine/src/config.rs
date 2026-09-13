//! TOML persistence of the mixer state.
//!
//! Location: `$XDG_CONFIG_HOME/pipedeck/config.toml`, falling back to
//! `$HOME/.config/pipedeck/config.toml`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{SourceConfig, SourceId};

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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, rename = "source")]
    pub sources: Vec<SourceConfig>,
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

    /// Load the config, returning the default (empty) config when the file
    /// does not exist yet.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                })
            }
        };
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })
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
    /// objects and may still be referenced by a session manager's saved state.
    pub fn next_id(&self) -> SourceId {
        SourceId(self.sources.iter().map(|s| s.id.0 + 1).max().unwrap_or(1))
    }

    pub fn source(&self, id: SourceId) -> Option<&SourceConfig> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn source_mut(&mut self, id: SourceId) -> Option<&mut SourceConfig> {
        self.sources.iter_mut().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MixBus;

    #[test]
    fn roundtrip_toml() {
        let mut cfg = Config::default();
        let mut src = SourceConfig::new(SourceId(1), "Game");
        src.chain_mut(MixBus::Monitor).gain = 0.5;
        src.chain_mut(MixBus::Stream).muted = true;
        cfg.sources.push(src);
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("[[source]]"));
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn next_id_skips_used_ids() {
        let mut cfg = Config::default();
        assert_eq!(cfg.next_id(), SourceId(1));
        cfg.sources.push(SourceConfig::new(SourceId(4), "a"));
        cfg.sources.push(SourceConfig::new(SourceId(2), "b"));
        assert_eq!(cfg.next_id(), SourceId(5));
    }

    #[test]
    fn load_missing_file_is_default() {
        let dir = std::env::temp_dir().join(format!("pipedeck-test-{}", std::process::id()));
        let path = dir.join("nope").join("config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        let mut cfg = Config::default();
        cfg.sources.push(SourceConfig::new(SourceId(1), "x"));
        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
        let _ = fs::remove_dir_all(&dir);
    }
}
