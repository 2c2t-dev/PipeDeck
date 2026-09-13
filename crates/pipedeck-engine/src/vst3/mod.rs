//! Hosting VST3 plug-ins.
//!
//! [`scan`] answers what is installed, [`host`] runs one. Everything here
//! speaks the plug-in's own ABI, which is a C++ COM interface: the calls are
//! unsafe by nature and each one says what it relies on.

pub mod host;
pub mod scan;

pub use host::Instance;
pub use scan::{bundles, installed, search_paths, Plugin};
