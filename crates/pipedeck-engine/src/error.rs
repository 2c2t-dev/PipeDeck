use crate::config::ConfigError;
use crate::types::SourceId;

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
    #[error("the engine thread is no longer running")]
    Stopped,
}
