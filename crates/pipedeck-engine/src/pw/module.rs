//! RAII wrapper around an in-process PipeWire module.
//!
//! pipewire-rs 0.10 exposes no safe wrapper for `pw_context_load_module`, so
//! this is the single place in the engine that goes through the raw FFI.
//!
//! Modules loaded this way live in *our* process and their streams belong to
//! our client connection: if the process dies, the server drops every node
//! they created. That is what guarantees "no orphan node after a crash".

use std::cell::{Cell, UnsafeCell};
use std::ffi::{c_void, CString};
use std::ptr::{self, NonNull};

use pipewire::context::ContextRc;
use pipewire::sys as pw_sys;

use crate::error::EngineError;

/// Told when the module is destroyed, whoever destroyed it. A loopback
/// destroys itself when one of its streams is cut off for good, its device
/// unplugged, and a module destroyed again is memory freed twice.
struct Watch {
    /// Written by libpipewire, which links it into the module's listeners.
    hook: UnsafeCell<libspa::sys::spa_hook>,
    gone: Cell<bool>,
}

static EVENTS: pw_sys::pw_impl_module_events = pw_sys::pw_impl_module_events {
    version: pw_sys::PW_VERSION_IMPL_MODULE_EVENTS,
    destroy: Some(destroyed),
    free: None,
    initialized: None,
    registered: None,
};

/// # Safety
/// Called by libpipewire with the [`Watch`] the listener was added with,
/// which outlives it: the module removes its listeners as it is destroyed,
/// and the watch is dropped only after.
unsafe extern "C" fn destroyed(data: *mut c_void) {
    let watch = unsafe { &*(data as *const Watch) };
    watch.gone.set(true);
}

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
    /// Boxed, so the hook libpipewire holds stays where it is.
    watch: Box<Watch>,
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
        let watch = Box::new(Watch {
            // SAFETY: a hook is plain data, all zeroes before it is added.
            hook: UnsafeCell::new(unsafe { std::mem::zeroed() }),
            gone: Cell::new(false),
        });
        // SAFETY: the module is live; the hook and the watch it points back
        // to are boxed, so they stay put, and they outlive the listener (see
        // `destroyed`).
        unsafe {
            pw_sys::pw_impl_module_add_listener(
                ptr.as_ptr(),
                watch.hook.get(),
                &EVENTS,
                &*watch as *const Watch as *mut c_void,
            );
        }
        log::debug!("loaded module {name} with args {args}");
        Ok(Self { ptr, name, watch })
    }

    /// Whether the module destroyed itself: what it made went with it, and
    /// the loopback has to be made again.
    pub fn is_gone(&self) -> bool {
        self.watch.gone.get()
    }
}

impl Drop for LoadedModule {
    fn drop(&mut self) {
        if self.is_gone() {
            log::debug!("module {} went by itself", self.name);
            return;
        }
        log::debug!("destroying module {}", self.name);
        // SAFETY: the pointer came from a successful `pw_context_load_module`
        // and the module has not been destroyed since, as the watch says;
        // per the type invariants, the owning context is still alive and we
        // are not inside a libpipewire callback.
        unsafe { pw_sys::pw_impl_module_destroy(self.ptr.as_ptr()) }
    }
}
