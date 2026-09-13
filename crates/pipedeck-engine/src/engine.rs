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
use crate::types::{
    self, App, ChainState, Device, Effect, EffectTarget, LinkConfig, MixConfig, MixId,
    SourceConfig, SourceId, MAX_MIXES,
};

/// Requests from a client to the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Add a column, named and dressed by rank. Refused past
    /// [`MAX_MIXES`](crate::types::MAX_MIXES).
    AddMix,
    RemoveMix(MixId),
    /// Rename a column. The graph keeps the description its nodes were born
    /// with: `node.name` derives from the id and never moves, and rebuilding
    /// the sink just to refresh a label would cut the audio and make a
    /// capture client reselect it.
    RenameMix {
        id: MixId,
        name: String,
    },
    /// Replace the devices a mix plays to, in one go. A device that stays
    /// attached keeps the level it had.
    SetMixOutputs {
        id: MixId,
        devices: Vec<String>,
    },
    /// Master level of a mix, applied to its sink, so it scales the outputs
    /// and a capture client alike.
    SetMixGain {
        id: MixId,
        gain: f32,
    },
    SetMixMute {
        id: MixId,
        muted: bool,
    },
    /// Level of one output of a mix.
    SetOutputGain {
        id: MixId,
        index: usize,
        gain: f32,
    },
    SetOutputMute {
        id: MixId,
        index: usize,
        muted: bool,
    },
    /// Add a row: a virtual sink, or a capture device when `device` is set.
    /// `icon` is carried through to the clients as the row's look.
    AddSource {
        name: String,
        device: Option<String>,
        icon: Option<String>,
    },
    RemoveSource(SourceId),
    /// Rename a row. Same trade-off as [`Command::RenameMix`].
    RenameSource {
        id: SourceId,
        name: String,
    },
    /// The look the interface gives a row or a column. The engine stores it
    /// and passes it on; what it means is the interface's business.
    SetSourceIcon {
        id: SourceId,
        icon: Option<String>,
    },
    SetMixIcon {
        id: MixId,
        icon: Option<String>,
    },
    /// Trim of a row, applied to its sink ahead of every cell.
    SetSourceGain {
        id: SourceId,
        gain: f32,
    },
    SetSourceMute {
        id: SourceId,
        muted: bool,
    },
    /// Replace the effects a row runs, in order. Reloads its chain and the
    /// cells reading it.
    SetEffects {
        id: SourceId,
        effects: Vec<Effect>,
    },
    /// Replace the effects a column runs, in order. A mix is treated between
    /// its cells and its sink, so this reloads the cells feeding it while
    /// the sink a capture client reads stays put.
    SetMixEffects {
        id: MixId,
        effects: Vec<Effect>,
    },
    /// Put the interface of one hosted plug-in on the screen, or take it
    /// away, by its place in the chain. Stereo Tool has one; a VST3 does
    /// not, here.
    ///
    /// Closing is the host's job: the window carries WM_DELETE_WINDOW like
    /// any other, and the library does nothing with it, so the button that
    /// opened it is what closes it.
    SetEffectWindow {
        target: EffectTarget,
        index: usize,
        open: bool,
    },
    /// The Stereo Tool licence key, which the mixer only hands to the
    /// library. Whatever runs on it is opened again, because a processor is
    /// told its key when it is created.
    SetStereoToolLicense {
        key: Option<String>,
    },
    /// Send an application's audio to a row, taking it from whichever row
    /// held it. Its running streams move at once, and so do the ones it
    /// opens later.
    AssignApp {
        id: SourceId,
        app: String,
    },
    /// Hand an application back to the session manager's own policy.
    ReleaseApp {
        id: SourceId,
        app: String,
    },
    /// Create or destroy one cell of the matrix.
    SetLink {
        source: SourceId,
        mix: MixId,
        linked: bool,
    },
    SetLinkGain {
        source: SourceId,
        mix: MixId,
        gain: f32,
    },
    SetLinkMute {
        source: SourceId,
        mix: MixId,
        muted: bool,
    },
    /// Read the installed plug-ins again, after one was added or removed.
    RescanPlugins,
    /// Quantum asked of our nodes, as `frames/rate`. Every loopback is
    /// reloaded, which cuts the audio for as long as that takes.
    SetLatency {
        latency: String,
    },
    /// Tear the graph down and stop the thread.
    Shutdown,
}

/// The matrix as the engine holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct StateSnapshot {
    /// Quantum asked of our nodes, so the settings window can show it.
    pub latency: String,
    /// The Stereo Tool licence key the settings hold, so the window can show
    /// what is in use.
    pub stereotool_license: Option<String>,
    pub mixes: Vec<MixConfig>,
    pub sources: Vec<SourceConfig>,
    pub links: Vec<LinkConfig>,
}

/// Notifications from the engine to its clients.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The whole matrix, sent once the graph is up and after every
    /// structural change. Clients rebuild their grid from it.
    State(StateSnapshot),
    /// The plug-ins installed on the machine, read once at startup.
    Plugins { available: Vec<crate::vst3::Plugin> },
    /// Where Stereo Tool stands: installed or not, licensed or not.
    StereoTool(crate::stereotool::Status),
    /// The applications currently playing audio, whatever they play into.
    Apps { running: Vec<App> },
    /// The audio devices currently on the system.
    Devices {
        outputs: Vec<Device>,
        inputs: Vec<Device>,
    },
    /// A mix's master level moved.
    MixChanged { id: MixId, state: ChainState },
    /// A row's trim moved.
    SourceChanged { id: SourceId, state: ChainState },
    /// The level of one output of a mix moved.
    OutputChanged {
        id: MixId,
        index: usize,
        state: ChainState,
    },
    /// One cell's fader moved. Sent on every change so that every client
    /// (there will be several once this is a daemon) stays in sync without
    /// rebuilding its grid.
    LinkChanged {
        source: SourceId,
        mix: MixId,
        state: ChainState,
    },
    /// Peaks measured since the last one of these, for the meters. Sent
    /// several times a second while the engine runs.
    Levels {
        sources: Vec<(SourceId, f32)>,
        mixes: Vec<(MixId, f32)>,
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
        let snapshot = g.snapshot();
        for mix in &snapshot.mixes {
            if let Err(e) = g.create_mix(mix) {
                log::error!("cannot create mix {}: {e}", mix.name);
                events(Event::Error(format!("cannot create mix {}: {e}", mix.name)));
            }
        }
        for source in &snapshot.sources {
            if let Err(e) = g.create_source(source) {
                log::error!("cannot create source {}: {e}", source.name);
                events(Event::Error(format!(
                    "cannot create source {}: {e}",
                    source.name
                )));
            }
        }
        for link in &snapshot.links {
            if let Err(e) = g.create_link(link) {
                log::error!(
                    "cannot link source {} to mix {}: {e}",
                    link.source,
                    link.mix
                );
                events(Event::Error(e.to_string()));
            }
        }
        g.emit_state();
        g.emit_devices();
        g.emit_apps();
        g.emit_plugins();
        g.refresh_stereotool();
        g.emit_stereotool();
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

    // Meters run far faster than housekeeping: a level that updates once a
    // quarter of a second reads as a stutter rather than as a signal.
    let meter_timer = {
        let graph = graph.clone();
        mainloop
            .loop_()
            .add_timer(move |_| graph.borrow().emit_levels())
    };
    meter_timer.update_timer(Some(METER_TICK), Some(METER_TICK));

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
const METER_TICK: Duration = Duration::from_millis(50);

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
    let mut structural = true;
    let result = match cmd {
        Command::AddMix => add_mix(&mut g),
        Command::RemoveMix(id) => g.remove_mix(id).map(|()| {
            let cfg = g.config_mut();
            cfg.mixes.retain(|m| m.id != id);
            cfg.prune_links();
        }),
        Command::RenameMix { id, name } => rename_mix(&mut g, id, name),
        Command::SetMixIcon { id, icon } => g
            .config_mut()
            .mix_mut(id)
            .map(|mix| mix.icon = icon)
            .ok_or(EngineError::UnknownMix(id)),
        Command::SetSourceIcon { id, icon } => g
            .config_mut()
            .source_mut(id)
            .map(|source| source.icon = icon)
            .ok_or(EngineError::UnknownSource(id)),
        Command::SetMixOutputs { id, devices } => g.set_mix_outputs(id, devices),
        Command::SetMixGain { id, gain } => {
            structural = false;
            g.update_mix(id, |c| c.gain = gain.clamp(0.0, 1.0))
        }
        Command::SetMixMute { id, muted } => {
            structural = false;
            g.update_mix(id, |c| c.muted = muted)
        }
        Command::SetOutputGain { id, index, gain } => {
            structural = false;
            g.update_output(id, index, |c| c.gain = gain.clamp(0.0, 1.0))
        }
        Command::SetOutputMute { id, index, muted } => {
            structural = false;
            g.update_output(id, index, |c| c.muted = muted)
        }
        Command::AddSource { name, device, icon } => add_source(&mut g, name, device, icon),
        Command::RemoveSource(id) => g.remove_source(id).map(|()| {
            let cfg = g.config_mut();
            cfg.sources.retain(|s| s.id != id);
            cfg.prune_links();
        }),
        Command::RenameSource { id, name } => rename_source(&mut g, id, name),
        Command::SetSourceGain { id, gain } => {
            structural = false;
            g.update_source(id, |c| c.gain = gain.clamp(0.0, 1.0))
        }
        Command::SetSourceMute { id, muted } => {
            structural = false;
            g.update_source(id, |c| c.muted = muted)
        }
        Command::SetEffects { id, effects } => g.set_effects(id, effects),
        Command::SetMixEffects { id, effects } => g.set_mix_effects(id, effects),
        Command::SetEffectWindow {
            target,
            index,
            open,
        } => {
            structural = false;
            g.set_effect_window(target, index, open)
        }
        Command::SetStereoToolLicense { key } => {
            structural = false;
            let result = g.set_stereotool_license(key);
            g.emit_stereotool();
            result
        }
        Command::AssignApp { id, app } => g.assign_app(id, app),
        Command::ReleaseApp { id, app } => g.release_app(id, &app),
        Command::SetLink {
            source,
            mix,
            linked,
        } => set_link(&mut g, source, mix, linked),
        Command::SetLinkGain { source, mix, gain } => {
            structural = false;
            g.update_link(source, mix, |c| c.gain = gain.clamp(0.0, 1.0))
        }
        Command::SetLinkMute { source, mix, muted } => {
            structural = false;
            g.update_link(source, mix, |c| c.muted = muted)
        }
        Command::RescanPlugins => {
            structural = false;
            g.rescan_plugins();
            Ok(())
        }
        Command::SetLatency { latency } => g.set_latency(latency),
        Command::Shutdown => {
            structural = false;
            mainloop.quit();
            Ok(())
        }
    };
    if let Err(e) = result {
        log::error!("{e}");
        events(Event::Error(e.to_string()));
    }
    if structural {
        g.emit_state();
    }
}

fn add_mix(g: &mut Graph) -> Result<(), EngineError> {
    let existing = g.config().mixes.len();
    if existing >= MAX_MIXES {
        return Err(EngineError::TooManyMixes(MAX_MIXES));
    }
    let (name, icon) = types::new_mix(existing);
    let mut cfg = MixConfig::new(g.config().next_mix_id(), name);
    cfg.icon = icon;
    g.create_mix(&cfg)?;
    g.config_mut().mixes.push(cfg);
    Ok(())
}

fn add_source(
    g: &mut Graph,
    name: String,
    device: Option<String>,
    icon: Option<String>,
) -> Result<(), EngineError> {
    let id = g.config().next_source_id();
    let cfg = match device {
        Some(device) => SourceConfig::input(id, name.trim(), device),
        None => SourceConfig::virtual_sink(id, name.trim()),
    }
    .with_icon(icon);
    g.create_source(&cfg)?;
    g.config_mut().sources.push(cfg);
    Ok(())
}

fn rename_mix(g: &mut Graph, id: MixId, name: String) -> Result<(), EngineError> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Ok(());
    }
    g.config_mut()
        .mix_mut(id)
        .ok_or(EngineError::UnknownMix(id))?
        .name = name;
    Ok(())
}

fn rename_source(g: &mut Graph, id: SourceId, name: String) -> Result<(), EngineError> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Ok(());
    }
    g.config_mut()
        .source_mut(id)
        .ok_or(EngineError::UnknownSource(id))?
        .name = name;
    Ok(())
}

fn set_link(g: &mut Graph, source: SourceId, mix: MixId, linked: bool) -> Result<(), EngineError> {
    if linked {
        let cfg = g
            .config()
            .link(source, mix)
            .copied()
            .unwrap_or_else(|| LinkConfig::new(source, mix));
        g.create_link(&cfg)?;
        let config = g.config_mut();
        if config.link(source, mix).is_none() {
            config.links.push(cfg);
        }
    } else {
        g.remove_link(source, mix)?;
        g.config_mut()
            .links
            .retain(|l| !(l.source == source && l.mix == mix));
    }
    Ok(())
}
