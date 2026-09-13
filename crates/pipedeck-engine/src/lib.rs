//! Pipedeck engine: PipeWire graph management for a streamer-oriented mixer.
//!
//! The mixer is a matrix. Sources are rows, mixes are columns, and each cell
//! is an independent gain and mute stage. A mix collects into a sink a
//! capture client such as OBS can read, and plays to any number of devices.
//!
//! This crate has no UI dependency. It exposes a thread-backed engine driven
//! by [`Command`]s and reporting [`Event`]s, plus the plain data types and
//! the TOML config. It is meant to become a standalone daemon later.

pub mod config;
pub mod engine;
pub mod error;
mod pw;
pub mod stereotool;
pub mod types;
pub mod vst3;

pub use config::Config;
pub use engine::{spawn, Command, EngineHandle, Event, StateSnapshot};
pub use error::EngineError;
pub use types::{
    new_mix, App, ChainState, Control, Device, Effect, EffectKind, LinkConfig, MixConfig, MixId,
    MixOutput, SourceConfig, SourceId, MAX_MIXES, NEW_MIXES,
};
