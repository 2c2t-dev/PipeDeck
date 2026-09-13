//! Pipedeck engine: PipeWire graph management for a streamer-oriented mixer.
//!
//! This crate has no UI dependency. It exposes a thread-backed engine driven
//! by [`Command`]s and reporting [`Event`]s, plus the plain data types and
//! the TOML config. It is meant to become a standalone daemon later.

pub mod config;
pub mod engine;
pub mod error;
mod pw;
pub mod types;

pub use config::Config;
pub use engine::{spawn, Command, EngineHandle, Event};
pub use error::EngineError;
pub use pw::STREAM_MIX_NODE;
pub use types::{ChainState, MixBus, SourceConfig, SourceId};
