//! RAII wrapper around an in-process PipeWire module.
//!
//! pipewire-rs 0.10 exposes no safe wrapper for `pw_context_load_module`, so
//! this is the single place in the engine that goes through the raw FFI.
//!
//! Modules loaded this way live in *our* process and their streams belong to
//! our client connection: if the process dies, the server drops every node
//! they created. That is what guarantees "no orphan node after a crash".

use std::ffi::CString;
use std::ptr::{self, NonNull};

use pipewire::context::ContextRc;
use pipewire::sys as pw_sys;

use crate::error::EngineError;

/// A module loaded into the engine's [`ContextRc`].
///
/// # Invariants
/// - Must be dropped **before** the context it was loaded into (the context
///   destroys its modules on teardown, so a later drop would be a double free).
///   Owners keep the module in a field declared before their `ContextRc`.
/// - Must never be dropped from inside a registry or proxy listener callback:
///   destroying a module while libpipewire is iterating over its objects is a
///   use-after-free. Removals happen from command handlers only.
pub struct LoadedModule {
    ptr: NonNull<pw_sys::pw_impl_module>,
    name: &'static str,
}

impl LoadedModule {
    pub fn load(context: &ContextRc, name: &'static str, args: &str) -> Result<Self, EngineError> {
        let c_name = CString::new(name).expect("module name without NUL");
        let c_args = CString::new(args).map_err(|_| EngineError::ModuleLoad {
            name,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "NUL byte in module args",
            ),
        })?;

        // SAFETY: `context` is a valid, live context (guaranteed by ContextRc);
        // both strings are NUL-terminated and outlive the call. A null
        // `properties` pointer is explicitly allowed by libpipewire.
        let raw = unsafe {
            pw_sys::pw_context_load_module(
                context.as_raw_ptr(),
                c_name.as_ptr(),
                c_args.as_ptr(),
                ptr::null_mut(),
            )
        };

        let ptr = NonNull::new(raw).ok_or_else(|| EngineError::ModuleLoad {
            name,
            source: std::io::Error::last_os_error(),
        })?;
        log::debug!("loaded module {name} with args {args}");
        Ok(Self { ptr, name })
    }
}

impl Drop for LoadedModule {
    fn drop(&mut self) {
        log::debug!("destroying module {}", self.name);
        // SAFETY: the pointer came from a successful `pw_context_load_module`
        // and, per the type invariants, the owning context is still alive and
        // we are not inside a libpipewire callback.
        unsafe { pw_sys::pw_impl_module_destroy(self.ptr.as_ptr()) }
    }
}
