//! The engine thread: owns the PipeWire main loop and the graph, receives
//! commands over a [`pipewire::channel`] and reports back through an event
//! callback.
//!
//! Threading model:
//! - Every PipeWire object lives on the engine thread. Nothing is shared.
//! - Clients talk to the engine with [`EngineHandle::send`], which is a
//!   message on an eventfd-backed channel: no mutex is ever held across the
//!   UI/engine boundary.
//! - The real-time data thread belongs to libpipewire (the loopback modules
//!   do the mixing). The engine thread only does control-plane work.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::context::ContextRc;
use pw::main_loop::MainLoopRc;

use crate::config::Config;
use crate::error::EngineError;
use crate::pw::Graph;
use crate::types::{ChainState, MixBus, SourceConfig, SourceId};

/// Requests from a client to the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    AddSource {
        name: String,
    },
    RemoveSource(SourceId),
    SetGain {
        id: SourceId,
        bus: MixBus,
        gain: f32,
    },
    SetMute {
        id: SourceId,
        bus: MixBus,
        muted: bool,
    },
    /// Tear the graph down and stop the thread.
    Shutdown,
}

/// Notifications from the engine to its client.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Connected to PipeWire, initial graph built from the config.
    Ready {
        sources: Vec<SourceConfig>,
    },
    SourceAdded(SourceConfig),
    SourceRemoved(SourceId),
    /// A chain state as the engine sees it, emitted after every change so
    /// every client (there will be several once this is a daemon) resyncs.
    ChainChanged {
        id: SourceId,
        bus: MixBus,
        state: ChainState,
    },
    /// Non-fatal problem worth showing to the user.
    Error(String),
    /// The engine thread is exiting; no further events follow.
    Stopped,
}

/// Client-side handle. Dropping it shuts the engine down and waits for the
/// thread, so the graph is always cleaned up on a normal exit.
pub struct EngineHandle {
    tx: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, cmd: Command) -> Result<(), EngineError> {
        self.tx.send(cmd).map_err(|_| EngineError::Stopped)
    }

    /// Ask the engine to stop and block until the thread has exited.
    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::error!("engine thread panicked");
            }
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

/// Start the engine on its own thread.
///
/// `on_event` is called from the engine thread; it should hand the event to
/// the client's own loop (e.g. push it into a channel) and return quickly.
pub fn spawn<F>(config_path: PathBuf, on_event: F) -> EngineHandle
where
    F: Fn(Event) + Send + 'static,
{
    let (tx, rx) = pw::channel::channel::<Command>();
    let thread = std::thread::Builder::new()
        .name("pipedeck-engine".into())
        .spawn(move || {
            let events: Rc<dyn Fn(Event)> = Rc::new(on_event);
            if let Err(e) = run(config_path, rx, events.clone()) {
                log::error!("engine stopped: {e}");
                events(Event::Error(e.to_string()));
            }
            events(Event::Stopped);
        })
        .expect("cannot spawn the engine thread");
    EngineHandle {
        tx,
        thread: Some(thread),
    }
}

/// Load the config, moving a corrupt file out of the way rather than
/// silently overwriting it later.
fn load_config(path: &PathBuf, events: &dyn Fn(Event)) -> Config {
    match Config::load(path) {
        Ok(cfg) => cfg,
        Err(e) => {
            log::error!("{e}");
            let backup = path.with_extension(format!(
                "toml.broken-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            ));
            let moved = std::fs::rename(path, &backup).is_ok();
            events(Event::Error(if moved {
                format!("{e}. The file was moved to {}.", backup.display())
            } else {
                e.to_string()
            }));
            Config::default()
        }
    }
}

fn run(
    config_path: PathBuf,
    rx: pw::channel::Receiver<Command>,
    events: Rc<dyn Fn(Event)>,
) -> Result<(), EngineError> {
    let config = load_config(&config_path, &*events);

    pw::init();
    let mainloop = MainLoopRc::new(None)?;
    let context = ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;

    let graph = Rc::new(RefCell::new(Graph::new(
        context.clone(),
        core.clone(),
        registry.clone(),
        config,
        config_path,
        events.clone(),
    )));

    // Lose the server: stop the loop, the client will see Error + Stopped.
    let _core_listener = {
        let mainloop = mainloop.clone();
        let events = events.clone();
        core.add_listener_local()
            .error(move |id, seq, res, message| {
                log::warn!("core error: id {id} seq {seq} res {res}: {message}");
                if id == pw::core::PW_ID_CORE && res == -libc_epipe() {
                    events(Event::Error("connection to PipeWire lost".into()));
                    mainloop.quit();
                }
            })
            .register()
    };

    let _registry_listener = {
        let weak = Rc::downgrade(&graph);
        let weak_remove = weak.clone();
        registry
            .add_listener_local()
            .global(move |global| {
                if let Some(graph) = weak.upgrade() {
                    graph.borrow_mut().on_global(global);
                }
            })
            .global_remove(move |id| {
                if let Some(graph) = weak_remove.upgrade() {
                    graph.borrow_mut().on_global_remove(id);
                }
            })
            .register()
    };

    // Build the initial graph from the config.
    {
        let mut g = graph.borrow_mut();
        g.create_stream_mix()?;
        let sources = g.config().sources.clone();
        for src in &sources {
            if let Err(e) = g.add_source(src) {
                log::error!("cannot create source {}: {e}", src.name);
                events(Event::Error(format!(
                    "cannot create source {}: {e}",
                    src.name
                )));
            }
        }
        events(Event::Ready { sources });
    }

    let _receiver = {
        let graph = graph.clone();
        let loop_owner = mainloop.clone();
        let events = events.clone();
        rx.attach(mainloop.loop_(), move |cmd| {
            handle_command(&graph, &loop_owner, &events, cmd)
        })
    };

    // Housekeeping tick: debounced config saves.
    let timer = {
        let graph = graph.clone();
        mainloop
            .loop_()
            .add_timer(move |_| graph.borrow_mut().tick())
    };
    timer.update_timer(Some(TICK), Some(TICK));

    log::info!("engine running");
    mainloop.run();
    log::info!("engine shutting down");

    graph.borrow_mut().flush_config();
    Ok(())
    // Locals drop in reverse order: timer and receiver first, then the
    // listeners, then `graph` (modules, proxies) while context/core are
    // still alive, then core disconnects.
}

const TICK: Duration = Duration::from_millis(250);

fn libc_epipe() -> i32 {
    32 // EPIPE on Linux
}

fn handle_command(
    graph: &Rc<RefCell<Graph>>,
    mainloop: &MainLoopRc,
    events: &Rc<dyn Fn(Event)>,
    cmd: Command,
) {
    log::debug!("command {cmd:?}");
    let mut g = graph.borrow_mut();
    let result = match cmd {
        Command::AddSource { name } => {
            let cfg = SourceConfig::new(g.config().next_id(), name.trim());
            g.add_source(&cfg).map(|()| {
                g.config_add(cfg.clone());
                events(Event::SourceAdded(cfg));
            })
        }
        Command::RemoveSource(id) => g.remove_source(id).map(|()| {
            g.config_remove(id);
            events(Event::SourceRemoved(id));
        }),
        Command::SetGain { id, bus, gain } => {
            g.update_chain(id, bus, |c| c.gain = gain.clamp(0.0, 1.0))
        }
        Command::SetMute { id, bus, muted } => g.update_chain(id, bus, |c| c.muted = muted),
        Command::Shutdown => {
            mainloop.quit();
            Ok(())
        }
    };
    if let Err(e) = result {
        log::error!("{e}");
        events(Event::Error(e.to_string()));
    }
}
