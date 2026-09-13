//! Shared, clonable access to the engine handle from GTK callbacks.
//!
//! The handle is taken out (and the engine shut down) exactly once, on
//! application shutdown; callbacks that fire after that are no-ops.

use std::cell::RefCell;
use std::rc::Rc;

use pipedeck_engine::{Command, EngineHandle};

#[derive(Clone)]
pub struct EngineLink {
    handle: Rc<RefCell<Option<EngineHandle>>>,
}

impl EngineLink {
    pub fn new(handle: EngineHandle) -> Self {
        Self {
            handle: Rc::new(RefCell::new(Some(handle))),
        }
    }

    pub fn send(&self, cmd: Command) {
        match self.handle.borrow().as_ref() {
            Some(handle) => {
                if let Err(e) = handle.send(cmd) {
                    log::error!("{e}");
                }
            }
            None => log::debug!("engine already stopped, dropping {cmd:?}"),
        }
    }

    /// Stop the engine and wait for it. Idempotent.
    pub fn shutdown(&self) {
        if let Some(handle) = self.handle.borrow_mut().take() {
            handle.shutdown();
        }
    }
}
