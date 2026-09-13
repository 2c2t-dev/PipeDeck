//! The live PipeWire graph behind the mixer matrix.
//!
//! ```text
//!  apps ──▶ [pipedeck.src.N] ──monitor──┐
//!                                       ├─ loopback (cell fader) ──▶ [pipedeck.mix.M] ──┬─ loopback ──▶ device
//!  mic  ────────────────────────────────┘                            (captured by OBS)  └─ loopback ──▶ device
//! ```
//!
//! A cell of the matrix is one loopback. Creating it links a source to a mix,
//! destroying it unlinks them, and its playback node carries the fader. Mix
//! outputs work the same way, which keeps every object in the graph identical
//! in nature and lets a mix feed any number of devices.
//!
//! Everything here belongs to this process' client connection: when the
//! process dies, the server drops all of it. Nothing lingers.

pub mod loopback;
pub mod meter;
pub mod module;
pub mod props;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use libspa::param::ParamType;
use libspa::pod::Pod;
use libspa::utils::dict::DictRef;
use pipewire::context::ContextRc;
use pipewire::core::CoreRc;
use pipewire::metadata::Metadata;
use pipewire::node::{Node, NodeListener};
use pipewire::properties::properties;
use pipewire::proxy::{ProxyListener, ProxyT};
use pipewire::registry::{GlobalObject, RegistryRc};
use pipewire::types::ObjectType;

use crate::config::Config;
use crate::engine::{Event, StateSnapshot};
use crate::error::EngineError;
use crate::types::{
    App, ChainState, Device, LinkConfig, MixConfig, MixId, MixOutput, SourceConfig, SourceId,
};

use loopback::{LoopbackSpec, AUDIO_POSITION, CHANNELS};
use meter::Meter;
use module::LoadedModule;

const NODE_PREFIX: &str = "pipedeck.";
/// Marks the nodes of this mixer. Node names come from ids that are private
/// to each mixer, so two Pipedecks on one graph answer to the same names;
/// this says which ones are ours.
const INSTANCE_KEY: &str = "pipedeck.instance";
const ADAPTER_FACTORY: &str = "adapter";
const NULL_SINK_FACTORY: &str = "support.null-audio-sink";
const LOOPBACK_MODULE: &str = "libpipewire-module-loopback";

/// Where a level read back from the graph belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Source(SourceId),
    Mix(MixId),
}

/// A node proxy bound from the registry.
struct BoundNode {
    global_id: u32,
    proxy: Node,
}

/// This process, as written on the nodes it owns.
pub fn instance() -> String {
    std::process::id().to_string()
}

/// Write a level onto a node we hold a proxy for.
fn apply_props(node: &Node, name: &str, state: &ChainState) {
    let bytes = props::volume_props(state, CHANNELS);
    let pod = Pod::from_bytes(&bytes).expect("volume_props builds a valid pod");
    node.set_param(ParamType::Props, 0, pod);
    log::trace!(
        "{name}: volume {:.3} mute {}",
        state.linear_volume(),
        state.muted
    );
}

/// One loopback with a level on it: a cell of the matrix, or one output of a
/// mix. Both are the same object in the graph, so they are the same here.
struct Stage {
    /// `node.name` of the playback node carrying the level.
    node_name: String,
    // Field order matters: the proxy goes first. Destroying the module takes
    // the node with it, and the server frees our binding along with it, so a
    // proxy dropped afterwards would send a destroy for a resource that is
    // already gone and earn an error back.
    node: Option<BoundNode>,
    /// Kept alive as long as the stage exists. See [`LoadedModule`].
    _module: LoadedModule,
    wanted: ChainState,
}

impl Stage {
    fn apply(&self) {
        if let Some(node) = &self.node {
            apply_props(&node.proxy, &self.node_name, &self.wanted);
        }
    }
}

/// What a Pipedeck playback node belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageRef {
    Cell(SourceId, MixId),
    Output(MixId, usize),
}

/// A column: the sink a capture client reads, plus one loopback per device.
struct Mix {
    // Field order matters: the outputs are destroyed before the sink they
    // capture from.
    outputs: Vec<Stage>,
    _sink_listener: NodeListener,
    _sink_bound: ProxyListener,
    /// Also carries the master level of the mix, which the sink applies to
    /// its monitor ports, so it scales the outputs and a capture client
    /// alike.
    sink: Node,
}

/// A row. Virtual rows own a sink; input rows capture a device directly and
/// own nothing in the graph.
struct Source {
    /// The row's sink, which also carries its trim. An input row has none.
    sink: Option<Node>,
    _sink_listener: Option<NodeListener>,
    _sink_bound: Option<ProxyListener>,
}

/// A device the user can attach to a mix or turn into a row.
struct DeviceEntry {
    device: Device,
    is_output: bool,
}

/// A playback stream belonging to some application.
struct AppStream {
    app: App,
}

/// The key an assignment matches on: the binary when the server knows it,
/// the application name otherwise, and the node name as a last resort.
fn app_key(props: &DictRef) -> Option<String> {
    props
        .get("application.process.binary")
        .or_else(|| props.get("application.name"))
        .or_else(|| props.get("node.name"))
        .map(str::to_owned)
}

fn app_name(props: &DictRef) -> String {
    props
        .get("application.name")
        .or_else(|| props.get("application.process.binary"))
        .or_else(|| props.get("node.name"))
        .unwrap_or("Unknown application")
        .to_owned()
}

pub struct Graph {
    // Drop order: cells first (their modules capture the sinks below), then
    // the mixes and sources, while context, core and registry are still
    // alive further down.
    links: HashMap<(SourceId, MixId), Stage>,
    mixes: HashMap<MixId, Mix>,
    sources: HashMap<SourceId, Source>,
    /// Playback node name -> what it belongs to. Filled before the module is
    /// loaded, so a registry announcement always finds its owner.
    stage_index: HashMap<String, StageRef>,
    /// Sinks the server has just named, whose meter is waiting for that id.
    bound_sinks: Rc<RefCell<Vec<(Owner, u32)>>>,
    /// Levels the graph reported on our own sinks, waiting to be taken in.
    ///
    /// A sink's volume is the system's volume: the user can move it from
    /// pavucontrol or a media key, and the mixer has to agree rather than
    /// hold a number of its own. The listener only queues, because it fires
    /// while the graph is borrowed.
    incoming: Rc<RefCell<Vec<(Owner, ChainState)>>>,
    /// Proxies of nodes we are about to destroy.
    ///
    /// Destroying a loopback module takes its nodes with it, and the server
    /// frees the bindings we hold on them at the same moment. Dropping such a
    /// proxy right away races the notification telling us it is gone, and the
    /// server answers our destroy with an error. Holding them until the next
    /// turn of the loop lets that notification land first, after which
    /// dropping them says nothing on the wire.
    retired: Vec<Node>,
    devices: HashMap<u32, DeviceEntry>,
    devices_dirty: bool,
    /// One measurement per row and per column. A cell needs none: it carries
    /// its channel's signal scaled by its own fader.
    source_meters: HashMap<SourceId, Meter>,
    mix_meters: HashMap<MixId, Meter>,
    /// Application playback streams currently on the graph.
    streams: HashMap<u32, AppStream>,
    streams_dirty: bool,
    /// The server's `default` metadata, which is how a stream is moved from
    /// one sink to another. Bound when the registry announces it.
    metadata: Option<Metadata>,
    registry: RegistryRc,
    core: CoreRc,
    context: ContextRc,
    config: Config,
    config_path: PathBuf,
    dirty: bool,
    events: Rc<dyn Fn(Event)>,
}

impl Graph {
    pub fn new(
        context: ContextRc,
        core: CoreRc,
        registry: RegistryRc,
        config: Config,
        config_path: PathBuf,
        events: Rc<dyn Fn(Event)>,
    ) -> Self {
        Self {
            // A converted config is written back on the first tick.
            dirty: config.migrated,
            links: HashMap::new(),
            mixes: HashMap::new(),
            sources: HashMap::new(),
            stage_index: HashMap::new(),
            bound_sinks: Rc::new(RefCell::new(Vec::new())),
            incoming: Rc::new(RefCell::new(Vec::new())),
            retired: Vec::new(),
            devices: HashMap::new(),
            devices_dirty: false,
            source_meters: HashMap::new(),
            mix_meters: HashMap::new(),
            streams: HashMap::new(),
            streams_dirty: false,
            metadata: None,
            registry,
            core,
            context,
            config,
            config_path,
            events,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn emit(&self, event: Event) {
        (self.events)(event);
    }

    /// The current matrix, as the clients see it.
    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            latency: self.config.latency.clone(),
            mixes: self.config.mixes.clone(),
            sources: self.config.sources.clone(),
            links: self.config.links.clone(),
        }
    }

    pub fn emit_state(&self) {
        self.emit(Event::State(self.snapshot()));
    }

    // --- building blocks ----------------------------------------------------

    /// Learn the global id the server gives one of our sinks, which is what
    /// a meter needs to point at that exact node.
    fn watch_sink_id(&self, sink: &Node, owner: Owner) -> ProxyListener {
        let queue = self.bound_sinks.clone();
        sink.upcast_ref()
            .add_listener_local()
            .bound(move |global_id| queue.borrow_mut().push((owner, global_id)))
            .register()
    }

    /// Follow a sink's own level, so a change made anywhere lands here too.
    fn watch_sink(&self, sink: &Node, owner: Owner) -> NodeListener {
        sink.subscribe_params(&[ParamType::Props]);
        let incoming = self.incoming.clone();
        sink.add_listener_local()
            .param(move |_, id, _, _, param| {
                if id != ParamType::Props {
                    return;
                }
                let Some(state) = param.and_then(props::parse_volume) else {
                    return;
                };
                incoming.borrow_mut().push((owner, state));
            })
            .register()
    }

    fn create_sink(&self, node_name: String, description: String) -> Result<Node, EngineError> {
        self.core
            .create_object::<Node>(
                ADAPTER_FACTORY,
                &properties! {
                    "factory.name" => NULL_SINK_FACTORY,
                    "node.name" => node_name,
                    "node.description" => description,
                    "media.class" => "Audio/Sink",
                    "audio.position" => AUDIO_POSITION,
                    INSTANCE_KEY => instance(),
                    // The mixer owns these levels. Without this the session
                    // manager restores whatever it saved last time, over the
                    // value the config just asked for.
                    "state.restore-props" => "false",
                    // Apply the sink volume (what pavucontrol shows) to the
                    // monitor ports, so it acts as a pre-fader trim on every
                    // cell of the row instead of on none of them.
                    "monitor.channel-volumes" => "true",
                },
            )
            .map_err(|source| EngineError::CreateObject {
                what: "sink",
                source,
            })
    }

    /// Load the loopbacks feeding a mix's devices. Any output that fails is
    /// reported and skipped: one dead device must not take the mix down.
    fn load_outputs(&mut self, cfg: &MixConfig) -> Vec<Stage> {
        let mut stages = Vec::with_capacity(cfg.outputs.len());
        for (index, output) in cfg.outputs.iter().enumerate() {
            let node_name = loopback::output_node_name(cfg.id, index);
            self.stage_index
                .insert(node_name.clone(), StageRef::Output(cfg.id, index));
            let spec = LoopbackSpec::for_output(cfg, index, &output.device, &self.config.latency);
            match LoadedModule::load(&self.context, LOOPBACK_MODULE, &spec.to_args()) {
                Ok(module) => stages.push(Stage {
                    node_name,
                    node: None,
                    _module: module,
                    wanted: output.state(),
                }),
                Err(e) => {
                    log::error!("{e}");
                    self.stage_index.remove(&node_name);
                    self.emit(Event::Error(format!(
                        "cannot send {} to {}: {e}",
                        cfg.name, output.device
                    )));
                }
            }
        }
        stages
    }

    /// Take a stage's proxy out before the stage, and its module, go.
    fn retire(&mut self, stage: &mut Stage) {
        if let Some(bound) = stage.node.take() {
            self.retired.push(bound.proxy);
        }
    }

    /// Attach the meters whose sink has just been given a global id.
    ///
    /// Waiting for that id is what makes a meter measure *our* sink: node
    /// names come from ids private to each mixer, so a second Pipedeck on the
    /// same graph carries the same names, and a meter pointed by name could
    /// land on its audio instead.
    fn hook_up_meters(&mut self) {
        let bound: Vec<(Owner, u32)> = self.bound_sinks.borrow_mut().drain(..).collect();
        for (owner, global_id) in bound {
            let (meter_name, target) = match owner {
                Owner::Source(id) => (format!("pipedeck.meter.src.{id}"), id.sink_node_name()),
                Owner::Mix(id) => (format!("pipedeck.meter.mix.{id}"), id.sink_node_name()),
            };
            let Some(meter) = self.watch_level(&meter_name, &target, Some(global_id), true) else {
                continue;
            };
            match owner {
                Owner::Source(id) => self.source_meters.insert(id, meter),
                Owner::Mix(id) => self.mix_meters.insert(id, meter),
            };
        }
    }

    /// Start measuring one node. A meter that cannot be created costs the
    /// user a moving bar, not their audio, so it is reported and dropped.
    fn watch_level(
        &self,
        name: &str,
        target: &str,
        target_id: Option<u32>,
        from_sink: bool,
    ) -> Option<Meter> {
        match Meter::new(&self.core, name, target, target_id, from_sink) {
            Ok(meter) => Some(meter),
            Err(e) => {
                log::error!("cannot measure {target}: {e}");
                None
            }
        }
    }

    /// Peaks since the last read, one per row and per column.
    pub fn emit_levels(&self) {
        if self.source_meters.is_empty() && self.mix_meters.is_empty() {
            return;
        }
        self.emit(Event::Levels {
            sources: self
                .source_meters
                .iter()
                .map(|(id, meter)| (*id, meter.take()))
                .collect(),
            mixes: self
                .mix_meters
                .iter()
                .map(|(id, meter)| (*id, meter.take()))
                .collect(),
        });
    }

    fn forget_outputs(&mut self, id: MixId) {
        self.stage_index
            .retain(|_, owner| !matches!(owner, StageRef::Output(mix, _) if *mix == id));
    }

    // --- mixes --------------------------------------------------------------

    pub fn create_mix(&mut self, cfg: &MixConfig) -> Result<(), EngineError> {
        if self.mixes.contains_key(&cfg.id) {
            return Ok(());
        }
        let sink = self.create_sink(cfg.id.sink_node_name(), format!("Pipedeck {}", cfg.name))?;
        apply_props(&sink, &cfg.id.sink_node_name(), &cfg.state());
        let listener = self.watch_sink(&sink, Owner::Mix(cfg.id));
        // The sink has no global id yet: the meter is hooked up when the
        // registry announces it, which is also how we tell our sink from the
        // one another mixer gave the same name.
        let bound = self.watch_sink_id(&sink, Owner::Mix(cfg.id));
        let outputs = self.load_outputs(cfg);
        self.mixes.insert(
            cfg.id,
            Mix {
                outputs,
                _sink_listener: listener,
                _sink_bound: bound,
                sink,
            },
        );
        log::info!("mix {} ({}) created", cfg.id, cfg.name);
        Ok(())
    }

    /// Tear a mix down, cells first. Only ever called from the command
    /// handler, never from a listener (see [`LoadedModule`]).
    pub fn remove_mix(&mut self, id: MixId) -> Result<(), EngineError> {
        let mut mix = self.mixes.remove(&id).ok_or(EngineError::UnknownMix(id))?;
        self.mix_meters.remove(&id);
        self.bound_sinks
            .borrow_mut()
            .retain(|(owner, _)| *owner != Owner::Mix(id));
        for stage in &mut mix.outputs {
            self.retire(stage);
        }
        self.forget_outputs(id);
        let cells: Vec<(SourceId, MixId)> = self
            .links
            .keys()
            .copied()
            .filter(|(_, mix_id)| *mix_id == id)
            .collect();
        for cell in cells {
            self.drop_link(cell);
        }
        drop(mix);
        log::info!("mix {id} removed");
        Ok(())
    }

    /// Replace the devices a mix plays to. The mix sink stays in place, so a
    /// capture client such as OBS keeps its connection across the change.
    pub fn set_mix_outputs(&mut self, id: MixId, devices: Vec<String>) -> Result<(), EngineError> {
        let cfg = self.config.mix_mut(id).ok_or(EngineError::UnknownMix(id))?;
        // A device that stays attached keeps the level it had.
        cfg.outputs = devices
            .into_iter()
            .map(|device| match cfg.output(&device) {
                Some(existing) => existing.clone(),
                None => MixOutput::new(device),
            })
            .collect();
        let cfg = cfg.clone();

        let mut previous = std::mem::take(
            &mut self
                .mixes
                .get_mut(&id)
                .ok_or(EngineError::UnknownMix(id))?
                .outputs,
        );
        for stage in &mut previous {
            self.retire(stage);
        }
        // Drop the old loopbacks before loading the new ones, so a device
        // that stays attached is not captured twice for an instant.
        drop(previous);
        self.forget_outputs(id);
        let outputs = self.load_outputs(&cfg);
        self.mixes
            .get_mut(&id)
            .ok_or(EngineError::UnknownMix(id))?
            .outputs = outputs;
        self.dirty = true;
        log::info!("mix {id} now feeds {} device(s)", cfg.outputs.len());
        Ok(())
    }

    /// Master level of a mix, applied to its sink.
    pub fn update_mix(
        &mut self,
        id: MixId,
        f: impl FnOnce(&mut ChainState),
    ) -> Result<(), EngineError> {
        let cfg = self.config.mix_mut(id).ok_or(EngineError::UnknownMix(id))?;
        let mut state = cfg.state();
        f(&mut state);
        cfg.set_state(state);
        let mix = self.mixes.get(&id).ok_or(EngineError::UnknownMix(id))?;
        apply_props(&mix.sink, &id.sink_node_name(), &state);
        self.dirty = true;
        self.emit(Event::MixChanged { id, state });
        Ok(())
    }

    /// Level of one output of a mix.
    pub fn update_output(
        &mut self,
        id: MixId,
        index: usize,
        f: impl FnOnce(&mut ChainState),
    ) -> Result<(), EngineError> {
        let cfg = self.config.mix_mut(id).ok_or(EngineError::UnknownMix(id))?;
        let output = cfg
            .outputs
            .get_mut(index)
            .ok_or(EngineError::UnknownOutput(id, index))?;
        let mut state = output.state();
        f(&mut state);
        output.set_state(state);

        let stage = self
            .mixes
            .get_mut(&id)
            .and_then(|mix| mix.outputs.get_mut(index))
            .ok_or(EngineError::UnknownOutput(id, index))?;
        stage.wanted = state;
        stage.apply();
        self.dirty = true;
        self.emit(Event::OutputChanged { id, index, state });
        Ok(())
    }

    // --- sources ------------------------------------------------------------

    pub fn create_source(&mut self, cfg: &SourceConfig) -> Result<(), EngineError> {
        if self.sources.contains_key(&cfg.id) {
            return Ok(());
        }
        // An input row captures its device straight from every cell, so it
        // owns no node of its own, and has no level to follow either.
        let (sink, listeners) = match cfg.device {
            Some(_) => (None, None),
            None => {
                let sink =
                    self.create_sink(cfg.id.sink_node_name(), format!("Pipedeck: {}", cfg.name))?;
                apply_props(&sink, &cfg.id.sink_node_name(), &cfg.state());
                let listener = self.watch_sink(&sink, Owner::Source(cfg.id));
                let bound = self.watch_sink_id(&sink, Owner::Source(cfg.id));
                (Some(sink), Some((listener, bound)))
            }
        };
        // A virtual row is measured on its sink's monitor, an input row on
        // the device it captures, which is the same signal every cell gets.
        // A device is named once on the graph, so its meter needs no waiting.
        // A sink of ours waits for the id the server gives it, in
        // `hook_up_meters`.
        if let Some(device) = &cfg.device {
            if let Some(meter) = self.watch_level(
                &format!("pipedeck.meter.src.{}", cfg.id),
                device,
                None,
                false,
            ) {
                self.source_meters.insert(cfg.id, meter);
            }
        }

        let (sink_listener, sink_bound) = match listeners {
            Some((listener, bound)) => (Some(listener), Some(bound)),
            None => (None, None),
        };
        self.sources.insert(
            cfg.id,
            Source {
                sink,
                _sink_listener: sink_listener,
                _sink_bound: sink_bound,
            },
        );
        log::info!("source {} ({}) created", cfg.id, cfg.name);
        Ok(())
    }

    pub fn remove_source(&mut self, id: SourceId) -> Result<(), EngineError> {
        let source = self
            .sources
            .remove(&id)
            .ok_or(EngineError::UnknownSource(id))?;
        self.source_meters.remove(&id);
        self.bound_sinks
            .borrow_mut()
            .retain(|(owner, _)| *owner != Owner::Source(id));
        let cells: Vec<(SourceId, MixId)> = self
            .links
            .keys()
            .copied()
            .filter(|(source_id, _)| *source_id == id)
            .collect();
        for cell in cells {
            self.drop_link(cell);
        }
        drop(source);
        log::info!("source {id} removed");
        Ok(())
    }

    /// Trim of a row, applied to its sink. An input row has no sink, so it
    /// keeps the value for the interface and changes nothing in the graph.
    pub fn update_source(
        &mut self,
        id: SourceId,
        f: impl FnOnce(&mut ChainState),
    ) -> Result<(), EngineError> {
        let cfg = self
            .config
            .source_mut(id)
            .ok_or(EngineError::UnknownSource(id))?;
        let mut state = cfg.state();
        f(&mut state);
        cfg.set_state(state);
        if let Some(sink) = self.sources.get(&id).and_then(|s| s.sink.as_ref()) {
            apply_props(sink, &id.sink_node_name(), &state);
        }
        self.dirty = true;
        self.emit(Event::SourceChanged { id, state });
        Ok(())
    }

    // --- cells --------------------------------------------------------------

    pub fn create_link(&mut self, link: &LinkConfig) -> Result<(), EngineError> {
        let cell = (link.source, link.mix);
        if self.links.contains_key(&cell) {
            return Ok(());
        }
        let source = self
            .config
            .source(link.source)
            .ok_or(EngineError::UnknownSource(link.source))?
            .clone();
        let mix = self
            .config
            .mix(link.mix)
            .ok_or(EngineError::UnknownMix(link.mix))?
            .clone();

        let node_name = loopback::link_node_name(link.source, link.mix);
        // Index before loading: the registry announces the playback node only
        // once the loop iterates again, and this way `on_global` never has to
        // cope with an unknown Pipedeck node.
        self.stage_index
            .insert(node_name.clone(), StageRef::Cell(link.source, link.mix));

        let spec = LoopbackSpec::for_link(&source, &mix, &self.config.latency);
        let module = match LoadedModule::load(&self.context, LOOPBACK_MODULE, &spec.to_args()) {
            Ok(module) => module,
            Err(e) => {
                self.stage_index.remove(&node_name);
                return Err(e);
            }
        };

        self.links.insert(
            cell,
            Stage {
                node_name,
                node: None,
                _module: module,
                wanted: link.state(),
            },
        );
        log::info!("{} feeds {}", source.name, mix.name);
        Ok(())
    }

    fn drop_link(&mut self, cell: (SourceId, MixId)) {
        if let Some(mut link) = self.links.remove(&cell) {
            self.stage_index.remove(&link.node_name);
            self.retire(&mut link);
        }
    }

    pub fn remove_link(&mut self, source: SourceId, mix: MixId) -> Result<(), EngineError> {
        if !self.links.contains_key(&(source, mix)) {
            return Err(EngineError::UnknownLink(source, mix));
        }
        self.drop_link((source, mix));
        log::info!("source {source} no longer feeds mix {mix}");
        Ok(())
    }

    pub fn update_link(
        &mut self,
        source: SourceId,
        mix: MixId,
        f: impl FnOnce(&mut ChainState),
    ) -> Result<(), EngineError> {
        let link = self
            .links
            .get_mut(&(source, mix))
            .ok_or(EngineError::UnknownLink(source, mix))?;
        f(&mut link.wanted);
        link.apply();
        let state = link.wanted;
        if let Some(cfg) = self.config.link_mut(source, mix) {
            cfg.set_state(state);
        }
        self.dirty = true;
        self.emit(Event::LinkChanged { source, mix, state });
        Ok(())
    }

    /// Ask the server for a different quantum on our nodes.
    ///
    /// It is a property of each loopback, fixed when the module is loaded, so
    /// every one of them is reloaded. The audio stops for as long as that
    /// takes, which is why this lives in a settings window and not on a
    /// fader.
    pub fn set_latency(&mut self, latency: String) -> Result<(), EngineError> {
        if self.config.latency == latency {
            return Ok(());
        }
        self.config.latency = latency;
        self.dirty = true;

        let cells: Vec<LinkConfig> = self.config.links.clone();
        for link in &cells {
            self.drop_link((link.source, link.mix));
        }
        for link in &cells {
            if let Err(e) = self.create_link(link) {
                log::error!("{e}");
                self.emit(Event::Error(e.to_string()));
            }
        }

        let mixes: Vec<MixId> = self.config.mixes.iter().map(|mix| mix.id).collect();
        for id in mixes {
            let devices = self
                .config
                .mix(id)
                .map(|mix| mix.outputs.iter().map(|o| o.device.clone()).collect())
                .unwrap_or_default();
            if let Err(e) = self.set_mix_outputs(id, devices) {
                log::error!("{e}");
                self.emit(Event::Error(e.to_string()));
            }
        }
        log::info!("nodes reloaded at {}", self.config.latency);
        Ok(())
    }

    // --- config -------------------------------------------------------------

    pub fn config_mut(&mut self) -> &mut Config {
        self.dirty = true;
        &mut self.config
    }

    /// Write the config if anything changed since the last save.
    pub fn flush_config(&mut self) {
        if !self.dirty {
            return;
        }
        match self.config.save(&self.config_path) {
            Ok(()) => {
                self.dirty = false;
                log::debug!("config saved to {}", self.config_path.display());
            }
            Err(e) => {
                log::error!("{e}");
                self.emit(Event::Error(e.to_string()));
                // Do not retry on every tick.
                self.dirty = false;
            }
        }
    }

    // --- registry -----------------------------------------------------------

    /// Registry `global` event: bind the playback node of a cell as soon as
    /// it is announced, and keep the device list up to date. Must not destroy
    /// anything.
    pub fn on_global(&mut self, global: &GlobalObject<&DictRef>) {
        if global.type_ == ObjectType::Metadata {
            let is_default = global
                .props
                .and_then(|p| p.get("metadata.name"))
                .is_some_and(|name| name == "default");
            if is_default && self.metadata.is_none() {
                match self.registry.bind::<Metadata, _>(global) {
                    Ok(metadata) => {
                        log::debug!("bound the default metadata");
                        self.metadata = Some(metadata);
                    }
                    Err(e) => log::error!("cannot bind the default metadata: {e}"),
                }
            }
            return;
        }
        if global.type_ != ObjectType::Node {
            return;
        }
        let Some(props) = global.props else {
            return;
        };
        let Some(name) = props.get("node.name") else {
            return;
        };

        if let Some(&owner) = self.stage_index.get(name) {
            self.bind_stage(owner, global);
            return;
        }
        if name.starts_with(NODE_PREFIX) {
            return;
        }

        if props
            .get("media.class")
            .is_some_and(|class| class.starts_with("Stream/Output/Audio"))
        {
            let Some(key) = app_key(props) else {
                return;
            };
            let app = App {
                key,
                name: app_name(props),
                icon: props
                    .get("application.icon-name")
                    .or_else(|| props.get("application.id"))
                    .map(str::to_owned),
            };
            // An application the user has assigned lands on its row's sink as
            // soon as it starts playing.
            if let Some(source) = self.source_for_app(&app.key) {
                self.move_stream(global.id, &app.name, Some(source));
            }
            self.streams.insert(global.id, AppStream { app });
            self.streams_dirty = true;
            return;
        }

        let is_output = match props.get("media.class") {
            Some("Audio/Sink") => true,
            Some("Audio/Source") => false,
            _ => return,
        };
        let description = props.get("node.description").unwrap_or(name).to_owned();
        self.devices.insert(
            global.id,
            DeviceEntry {
                device: Device {
                    name: name.to_owned(),
                    description,
                },
                is_output,
            },
        );
        self.devices_dirty = true;
    }

    fn bind_stage(&mut self, owner: StageRef, global: &GlobalObject<&DictRef>) {
        let stage = match owner {
            StageRef::Cell(source, mix) => self.links.get_mut(&(source, mix)),
            StageRef::Output(mix, index) => self
                .mixes
                .get_mut(&mix)
                .and_then(|mix| mix.outputs.get_mut(index)),
        };
        let Some(link) = stage else {
            return;
        };
        let failure = match self.registry.bind::<Node, _>(global) {
            Ok(proxy) => {
                link.node = Some(BoundNode {
                    global_id: global.id,
                    proxy,
                });
                link.apply();
                log::debug!("{} bound to global {}", link.node_name, global.id);
                None
            }
            Err(e) => Some(format!("cannot bind {}: {e}", link.node_name)),
        };
        if let Some(message) = failure {
            log::error!("{message}");
            self.emit(Event::Error(message));
        }
    }

    /// Registry `global_remove` event. Dropping a proxy here is fine, but a
    /// module must never be destroyed from a listener.
    pub fn on_global_remove(&mut self, global_id: u32) {
        if self.devices.remove(&global_id).is_some() {
            self.devices_dirty = true;
        }
        if self.streams.remove(&global_id).is_some() {
            self.streams_dirty = true;
        }
        let outputs = self
            .mixes
            .values_mut()
            .flat_map(|mix| mix.outputs.iter_mut());
        for stage in self.links.values_mut().chain(outputs) {
            if stage
                .node
                .as_ref()
                .is_some_and(|n| n.global_id == global_id)
            {
                log::debug!("{} gone from the graph", stage.node_name);
                stage.node = None;
            }
        }
    }

    /// The row an application is assigned to, if any.
    fn source_for_app(&self, key: &str) -> Option<SourceId> {
        self.config
            .sources
            .iter()
            .find(|source| source.apps.iter().any(|app| app == key))
            .map(|source| source.id)
    }

    /// Point a stream at a row's sink, the way a session manager does it.
    ///
    /// Passing no value clears the property instead, which hands the stream
    /// back to the session manager's own policy.
    fn move_stream(&self, stream: u32, name: &str, source: Option<SourceId>) {
        let Some(metadata) = &self.metadata else {
            log::warn!("cannot move {name}: the default metadata is not bound yet");
            return;
        };
        let target = source.map(|id| id.sink_node_name());
        metadata.set_property(
            stream,
            "target.object",
            target.as_deref().map(|_| "Spa:String"),
            target.as_deref(),
        );
        match &target {
            Some(sink) => log::info!("{name} now plays into {sink}"),
            None => log::info!("{name} handed back to the session manager"),
        }
    }

    /// Move every running stream of an application, or release them.
    fn move_app(&self, key: &str, source: Option<SourceId>) {
        for (id, stream) in &self.streams {
            if stream.app.key == key {
                self.move_stream(*id, &stream.app.name, source);
            }
        }
    }

    /// Assign an application to a row, taking it from whichever row had it.
    pub fn assign_app(&mut self, id: SourceId, key: String) -> Result<(), EngineError> {
        if self.config.source(id).is_none() {
            return Err(EngineError::UnknownSource(id));
        }
        for source in &mut self.config.sources {
            source.apps.retain(|app| app != &key);
        }
        let source = self
            .config
            .source_mut(id)
            .ok_or(EngineError::UnknownSource(id))?;
        source.apps.push(key.clone());
        self.dirty = true;
        self.move_app(&key, Some(id));
        Ok(())
    }

    pub fn release_app(&mut self, id: SourceId, key: &str) -> Result<(), EngineError> {
        let source = self
            .config
            .source_mut(id)
            .ok_or(EngineError::UnknownSource(id))?;
        source.apps.retain(|app| app != key);
        self.dirty = true;
        self.move_app(key, None);
        Ok(())
    }

    /// Applications currently playing, deduplicated by key.
    fn app_list(&self) -> Vec<App> {
        let mut apps: Vec<App> = Vec::new();
        for stream in self.streams.values() {
            if !apps.iter().any(|app| app.key == stream.app.key) {
                apps.push(stream.app.clone());
            }
        }
        apps.sort_by(|a, b| a.name.cmp(&b.name));
        apps
    }

    pub fn emit_apps(&mut self) {
        self.streams_dirty = false;
        self.emit(Event::Apps {
            running: self.app_list(),
        });
    }

    fn device_lists(&self) -> (Vec<Device>, Vec<Device>) {
        let mut outputs: Vec<Device> = Vec::new();
        let mut inputs: Vec<Device> = Vec::new();
        for entry in self.devices.values() {
            if entry.is_output {
                outputs.push(entry.device.clone());
            } else {
                inputs.push(entry.device.clone());
            }
        }
        outputs.sort_by(|a, b| a.description.cmp(&b.description));
        inputs.sort_by(|a, b| a.description.cmp(&b.description));
        (outputs, inputs)
    }

    pub fn emit_devices(&mut self) {
        self.devices_dirty = false;
        let (outputs, inputs) = self.device_lists();
        self.emit(Event::Devices { outputs, inputs });
    }

    /// Take in the levels the graph reported on our sinks.
    ///
    /// Only a real difference is kept: every level we write comes back
    /// through the same listener, and taking those in again would be a loop
    /// with the interface.
    fn absorb_levels(&mut self) {
        let incoming: Vec<(Owner, ChainState)> = self.incoming.borrow_mut().drain(..).collect();
        for (owner, state) in incoming {
            let known = match owner {
                Owner::Source(id) => self.config.source(id).map(|s| s.state()),
                Owner::Mix(id) => self.config.mix(id).map(|m| m.state()),
            };
            let Some(known) = known else {
                continue;
            };
            if (known.linear_volume() - state.linear_volume()).abs() < 1e-3
                && known.muted == state.muted
            {
                continue;
            }
            log::debug!("{owner:?} was set to {state:?} outside the mixer");
            match owner {
                Owner::Source(id) => {
                    if let Some(cfg) = self.config.source_mut(id) {
                        cfg.set_state(state);
                    }
                    self.emit(Event::SourceChanged { id, state });
                }
                Owner::Mix(id) => {
                    if let Some(cfg) = self.config.mix_mut(id) {
                        cfg.set_state(state);
                    }
                    self.emit(Event::MixChanged { id, state });
                }
            }
            self.dirty = true;
        }
    }

    /// Periodic housekeeping from the engine timer: debounced config saves
    /// and device list updates.
    pub fn tick(&mut self) {
        self.hook_up_meters();
        self.absorb_levels();
        // The server has told us by now that these nodes are gone, so their
        // proxies leave without a word.
        self.retired.clear();
        if self.devices_dirty {
            self.emit_devices();
        }
        if self.streams_dirty {
            self.emit_apps();
        }
        self.flush_config();
    }
}
