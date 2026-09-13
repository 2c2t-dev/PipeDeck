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
pub mod module;
pub mod props;

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use libspa::param::ParamType;
use libspa::pod::Pod;
use libspa::utils::dict::DictRef;
use pipewire::context::ContextRc;
use pipewire::core::CoreRc;
use pipewire::node::Node;
use pipewire::properties::properties;
use pipewire::registry::{GlobalObject, RegistryRc};
use pipewire::types::ObjectType;

use crate::config::Config;
use crate::engine::{Event, StateSnapshot};
use crate::error::EngineError;
use crate::types::{ChainState, Device, LinkConfig, MixConfig, MixId, SourceConfig, SourceId};

use loopback::{LoopbackSpec, AUDIO_POSITION, CHANNELS};
use module::LoadedModule;

const NODE_PREFIX: &str = "pipedeck.";
const ADAPTER_FACTORY: &str = "adapter";
const NULL_SINK_FACTORY: &str = "support.null-audio-sink";
const LOOPBACK_MODULE: &str = "libpipewire-module-loopback";

/// A node proxy bound from the registry.
struct BoundNode {
    global_id: u32,
    proxy: Node,
}

/// One cell of the matrix.
struct Link {
    /// `node.name` of the playback node carrying the fader.
    node_name: String,
    /// Kept alive as long as the cell exists. See [`LoadedModule`].
    _module: LoadedModule,
    node: Option<BoundNode>,
    wanted: ChainState,
}

impl Link {
    fn apply(&self) {
        let Some(node) = &self.node else {
            return;
        };
        let bytes = props::volume_props(&self.wanted, CHANNELS);
        let pod = Pod::from_bytes(&bytes).expect("volume_props builds a valid pod");
        node.proxy.set_param(ParamType::Props, 0, pod);
        log::trace!(
            "{}: volume {:.3} mute {}",
            self.node_name,
            self.wanted.linear_volume(),
            self.wanted.muted
        );
    }
}

/// A column: the sink a capture client reads, plus one loopback per device.
struct Mix {
    // Field order matters: the outputs are destroyed before the sink they
    // capture from.
    outputs: Vec<LoadedModule>,
    #[allow(dead_code)] // held only to keep the remote object alive
    sink: Node,
}

/// A row. Virtual rows own a sink; input rows capture a device directly and
/// own nothing in the graph.
struct Source {
    #[allow(dead_code)] // held only to keep the remote object alive
    sink: Option<Node>,
}

/// A device the user can attach to a mix or turn into a row.
struct DeviceEntry {
    device: Device,
    is_output: bool,
}

pub struct Graph {
    // Drop order: cells first (their modules capture the sinks below), then
    // the mixes and sources, while context, core and registry are still
    // alive further down.
    links: HashMap<(SourceId, MixId), Link>,
    mixes: HashMap<MixId, Mix>,
    sources: HashMap<SourceId, Source>,
    /// Playback node name -> owning cell. Filled before the module is loaded,
    /// so a registry announcement always finds its cell.
    link_index: HashMap<String, (SourceId, MixId)>,
    devices: HashMap<u32, DeviceEntry>,
    devices_dirty: bool,
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
            link_index: HashMap::new(),
            devices: HashMap::new(),
            devices_dirty: false,
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
            mixes: self.config.mixes.clone(),
            sources: self.config.sources.clone(),
            links: self.config.links.clone(),
        }
    }

    pub fn emit_state(&self) {
        self.emit(Event::State(self.snapshot()));
    }

    // --- building blocks ----------------------------------------------------

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
    fn load_outputs(&self, cfg: &MixConfig) -> Vec<LoadedModule> {
        let mut modules = Vec::with_capacity(cfg.outputs.len());
        for (index, device) in cfg.outputs.iter().enumerate() {
            let spec = LoopbackSpec::for_output(cfg, index, device, &self.config.latency);
            match LoadedModule::load(&self.context, LOOPBACK_MODULE, &spec.to_args()) {
                Ok(module) => modules.push(module),
                Err(e) => {
                    log::error!("{e}");
                    self.emit(Event::Error(format!(
                        "cannot send {} to {device}: {e}",
                        cfg.name
                    )));
                }
            }
        }
        modules
    }

    // --- mixes --------------------------------------------------------------

    pub fn create_mix(&mut self, cfg: &MixConfig) -> Result<(), EngineError> {
        if self.mixes.contains_key(&cfg.id) {
            return Ok(());
        }
        let sink = self.create_sink(cfg.id.sink_node_name(), format!("Pipedeck {}", cfg.name))?;
        let outputs = self.load_outputs(cfg);
        self.mixes.insert(cfg.id, Mix { outputs, sink });
        log::info!("mix {} ({}) created", cfg.id, cfg.name);
        Ok(())
    }

    /// Tear a mix down, cells first. Only ever called from the command
    /// handler, never from a listener (see [`LoadedModule`]).
    pub fn remove_mix(&mut self, id: MixId) -> Result<(), EngineError> {
        let mix = self.mixes.remove(&id).ok_or(EngineError::UnknownMix(id))?;
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
        cfg.outputs = devices;
        let cfg = cfg.clone();
        let mix = self.mixes.get_mut(&id).ok_or(EngineError::UnknownMix(id))?;
        // Drop the old loopbacks before loading the new ones, so a device
        // that stays attached is not captured twice for an instant.
        mix.outputs.clear();
        let outputs = self.load_outputs(&cfg);
        self.mixes
            .get_mut(&id)
            .ok_or(EngineError::UnknownMix(id))?
            .outputs = outputs;
        self.dirty = true;
        log::info!("mix {id} now feeds {} device(s)", cfg.outputs.len());
        Ok(())
    }

    // --- sources ------------------------------------------------------------

    pub fn create_source(&mut self, cfg: &SourceConfig) -> Result<(), EngineError> {
        if self.sources.contains_key(&cfg.id) {
            return Ok(());
        }
        let sink = match cfg.device {
            // An input row captures its device straight from every cell, so
            // it owns no node of its own.
            Some(_) => None,
            None => {
                Some(self.create_sink(cfg.id.sink_node_name(), format!("Pipedeck: {}", cfg.name))?)
            }
        };
        self.sources.insert(cfg.id, Source { sink });
        log::info!("source {} ({}) created", cfg.id, cfg.name);
        Ok(())
    }

    pub fn remove_source(&mut self, id: SourceId) -> Result<(), EngineError> {
        let source = self
            .sources
            .remove(&id)
            .ok_or(EngineError::UnknownSource(id))?;
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
        self.link_index.insert(node_name.clone(), cell);

        let spec = LoopbackSpec::for_link(&source, &mix, &self.config.latency);
        let module = match LoadedModule::load(&self.context, LOOPBACK_MODULE, &spec.to_args()) {
            Ok(module) => module,
            Err(e) => {
                self.link_index.remove(&node_name);
                return Err(e);
            }
        };

        self.links.insert(
            cell,
            Link {
                node_name,
                _module: module,
                node: None,
                wanted: link.state(),
            },
        );
        log::info!("{} feeds {}", source.name, mix.name);
        Ok(())
    }

    fn drop_link(&mut self, cell: (SourceId, MixId)) {
        if let Some(link) = self.links.remove(&cell) {
            self.link_index.remove(&link.node_name);
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
        if global.type_ != ObjectType::Node {
            return;
        }
        let Some(props) = global.props else {
            return;
        };
        let Some(name) = props.get("node.name") else {
            return;
        };

        if let Some(&cell) = self.link_index.get(name) {
            self.bind_link(cell, global);
            return;
        }
        if name.starts_with(NODE_PREFIX) {
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

    fn bind_link(&mut self, cell: (SourceId, MixId), global: &GlobalObject<&DictRef>) {
        let Some(link) = self.links.get_mut(&cell) else {
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
        for link in self.links.values_mut() {
            if link.node.as_ref().is_some_and(|n| n.global_id == global_id) {
                log::debug!("{} gone from the graph", link.node_name);
                link.node = None;
            }
        }
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

    /// Periodic housekeeping from the engine timer: debounced config saves
    /// and device list updates.
    pub fn tick(&mut self) {
        if self.devices_dirty {
            self.emit_devices();
        }
        self.flush_config();
    }
}
