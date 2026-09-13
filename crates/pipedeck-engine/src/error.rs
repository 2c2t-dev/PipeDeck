use crate::config::ConfigError;
use crate::types::{MixId, SourceId};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("PipeWire error: {0}")]
    PipeWire(#[from] pipewire::Error),
    #[error("cannot load module {name}: {source}")]
    ModuleLoad {
        name: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot create {what} on the PipeWire server: {source}")]
    CreateObject {
        what: &'static str,
        #[source]
        source: pipewire::Error,
    },
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("unknown source {0}")]
    UnknownSource(SourceId),
    #[error("unknown mix {0}")]
    UnknownMix(MixId),
    #[error("source {0} does not feed mix {1}")]
    UnknownLink(SourceId, MixId),
    #[error("a mixer holds at most {0} mixes")]
    TooManyMixes(usize),
    #[error("the engine thread is no longer running")]
    Stopped,
}
