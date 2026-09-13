//! The live PipeWire graph: one null sink per source, two loopbacks per
//! source, one null sink for the Stream Mix.
//!
//! ```text
//! apps ──▶ [pipedeck.N] null sink
//!             │
//!             ├─ monitor ──▶ loopback ──▶ [pipedeck.N.stream]  ──▶ [pipedeck.stream_mix]
//!             └─ monitor ──▶ loopback ──▶ [pipedeck.N.monitor] ──▶ default output device
//! ```
//!
//! Both chains are identical and capture the same monitor ports, so they see
//! the same signal and behave the same way with respect to the sink volume
//! (which is applied to the monitor ports via `monitor.channel-volumes`).
//! The fader of each chain is the `Props` volume of its playback node.
//!
//! Every object here belongs to this process' client connection(s): when the
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
use crate::engine::Event;
use crate::error::EngineError;
use crate::types::{ChainState, MixBus, SourceConfig, SourceId};

use loopback::{LoopbackSpec, AUDIO_POSITION, CHANNELS};
use module::LoadedModule;

/// `node.name` of the sink OBS captures.
pub const STREAM_MIX_NODE: &str = "pipedeck.stream_mix";
const STREAM_MIX_DESCRIPTION: &str = "Pipedeck Stream Mix";
const ADAPTER_FACTORY: &str = "adapter";
const NULL_SINK_FACTORY: &str = "support.null-audio-sink";
const LOOPBACK_MODULE: &str = "libpipewire-module-loopback";

/// A node proxy bound from the registry.
struct BoundNode {
    global_id: u32,
    proxy: Node,
}

/// One gain chain: a loopback plus the fader state of its playback node.
struct Chain {
    /// `node.name` of the playback stream carrying the fader volume.
    node_name: String,
    /// Kept alive as long as the chain exists. See [`LoadedModule`] invariants.
    _module: LoadedModule,
    /// Bound once the registry announces the playback node.
    node: Option<BoundNode>,
    wanted: ChainState,
}

impl Chain {
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

/// A source that exists in the graph.
struct LiveSource {
    // Field order matters: chains (modules) are destroyed before the sink
    // proxy so the loopbacks never outlive the sink they capture.
    stream: Chain,
    monitor: Chain,
    #[allow(dead_code)] // held only to keep the remote object alive
    sink: Node,
}

impl LiveSource {
    fn chain_mut(&mut self, bus: MixBus) -> &mut Chain {
        match bus {
            MixBus::Stream => &mut self.stream,
            MixBus::Monitor => &mut self.monitor,
        }
    }

    fn chains_mut(&mut self) -> [&mut Chain; 2] {
        [&mut self.stream, &mut self.monitor]
    }
}

pub struct Graph {
    // Drop order: sources (modules + proxies) and the stream mix go first,
    // while `context`/`core`/`registry` below are still alive.
    sources: HashMap<SourceId, LiveSource>,
    stream_mix: Option<Node>,
    /// Playback node name -> owning chain. Filled before the loopbacks are
    /// loaded, so a registry announcement always finds its chain.
    chain_index: HashMap<String, (SourceId, MixBus)>,
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
            sources: HashMap::new(),
            stream_mix: None,
            chain_index: HashMap::new(),
            registry,
            core,
            context,
            config,
            config_path,
            dirty: false,
            events,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    fn emit(&self, event: Event) {
        (self.events)(event);
    }

    /// Create the Stream Mix null sink. No loopback involved, so no extra
    /// latency: OBS captures its monitor ports directly.
    pub fn create_stream_mix(&mut self) -> Result<(), EngineError> {
        let node = self
            .core
            .create_object::<Node>(
                ADAPTER_FACTORY,
                &properties! {
                    "factory.name" => NULL_SINK_FACTORY,
                    "node.name" => STREAM_MIX_NODE,
                    "node.description" => STREAM_MIX_DESCRIPTION,
                    "media.class" => "Audio/Sink",
                    "audio.position" => AUDIO_POSITION,
                },
            )
            .map_err(|source| EngineError::CreateObject {
                what: "stream mix sink",
                source,
            })?;
        self.stream_mix = Some(node);
        Ok(())
    }

    /// Create the sink and both loopbacks of a source. Volumes are applied as
    /// soon as the registry announces the playback nodes.
    pub fn add_source(&mut self, cfg: &SourceConfig) -> Result<(), EngineError> {
        if self.sources.contains_key(&cfg.id) {
            return Ok(());
        }
        let sink = self
            .core
            .create_object::<Node>(
                ADAPTER_FACTORY,
                &properties! {
                    "factory.name" => NULL_SINK_FACTORY,
                    "node.name" => cfg.id.sink_node_name(),
                    "node.description" => format!("Pipedeck: {}", cfg.name),
                    "media.class" => "Audio/Sink",
                    "audio.position" => AUDIO_POSITION,
                    // Apply the sink volume (what pavucontrol shows) to the
                    // monitor ports, so it acts as a pre-fader trim on both
                    // chains instead of on neither.
                    "monitor.channel-volumes" => "true",
                },
            )
            .map_err(|source| EngineError::CreateObject {
                what: "source sink",
                source,
            })?;

        // Index first: the registry announces the playback nodes only once the
        // loop iterates again, but keeping the index ahead of the graph means
        // `on_global` never has to cope with an unknown Pipedeck node.
        for bus in MixBus::ALL {
            self.chain_index
                .insert(loopback::playback_node_name(cfg.id, bus), (cfg.id, bus));
        }
        let load = |bus: MixBus| -> Result<Chain, EngineError> {
            let spec = LoopbackSpec::for_chain(cfg.id, &cfg.name, bus, STREAM_MIX_NODE);
            let module = LoadedModule::load(&self.context, LOOPBACK_MODULE, &spec.to_args())?;
            Ok(Chain {
                node_name: loopback::playback_node_name(cfg.id, bus),
                _module: module,
                node: None,
                wanted: *cfg.chain(bus),
            })
        };
        let chains = load(MixBus::Stream).and_then(|stream| Ok((stream, load(MixBus::Monitor)?)));
        let (stream, monitor) = match chains {
            Ok(chains) => chains,
            Err(e) => {
                for bus in MixBus::ALL {
                    self.chain_index
                        .remove(&loopback::playback_node_name(cfg.id, bus));
                }
                return Err(e);
            }
        };

        self.sources.insert(
            cfg.id,
            LiveSource {
                stream,
                monitor,
                sink,
            },
        );
        log::info!("source {} ({}) created", cfg.id, cfg.name);
        Ok(())
    }

    /// Tear down a source. Only ever called from the command handler, never
    /// from a registry or proxy listener (see [`LoadedModule`]).
    pub fn remove_source(&mut self, id: SourceId) -> Result<(), EngineError> {
        let live = self
            .sources
            .remove(&id)
            .ok_or(EngineError::UnknownSource(id))?;
        self.chain_index.remove(&live.stream.node_name);
        self.chain_index.remove(&live.monitor.node_name);
        drop(live);
        log::info!("source {id} removed");
        Ok(())
    }

    pub fn update_chain(
        &mut self,
        id: SourceId,
        bus: MixBus,
        f: impl FnOnce(&mut ChainState),
    ) -> Result<(), EngineError> {
        let live = self
            .sources
            .get_mut(&id)
            .ok_or(EngineError::UnknownSource(id))?;
        let chain = live.chain_mut(bus);
        f(&mut chain.wanted);
        chain.apply();
        let state = chain.wanted;
        if let Some(cfg) = self.config.source_mut(id) {
            *cfg.chain_mut(bus) = state;
        }
        self.dirty = true;
        self.emit(Event::ChainChanged { id, bus, state });
        Ok(())
    }

    // --- config -----------------------------------------------------------

    pub fn config_add(&mut self, cfg: SourceConfig) {
        self.config.sources.push(cfg);
        self.dirty = true;
    }

    pub fn config_remove(&mut self, id: SourceId) {
        self.config.sources.retain(|s| s.id != id);
        self.dirty = true;
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

    /// Registry `global` event: bind the playback node of a chain as soon as
    /// it is announced and push the wanted volume. Must not destroy anything.
    pub fn on_global(&mut self, global: &GlobalObject<&DictRef>) {
        if global.type_ != ObjectType::Node {
            return;
        }
        let Some(name) = global.props.and_then(|p| p.get("node.name")) else {
            return;
        };
        let Some(&(id, bus)) = self.chain_index.get(name) else {
            return;
        };
        let Some(live) = self.sources.get_mut(&id) else {
            return;
        };
        let chain = live.chain_mut(bus);
        let failure = match self.registry.bind::<Node, _>(global) {
            Ok(proxy) => {
                chain.node = Some(BoundNode {
                    global_id: global.id,
                    proxy,
                });
                chain.apply();
                log::debug!("{} bound to global {}", chain.node_name, global.id);
                None
            }
            Err(e) => Some(format!("cannot bind {}: {e}", chain.node_name)),
        };
        if let Some(message) = failure {
            log::error!("{message}");
            self.emit(Event::Error(message));
        }
    }

    /// Registry `global_remove` event. Dropping a proxy here is fine (this is
    /// what pw-dump does); dropping a module is not.
    pub fn on_global_remove(&mut self, global_id: u32) {
        for live in self.sources.values_mut() {
            for chain in live.chains_mut() {
                if chain
                    .node
                    .as_ref()
                    .is_some_and(|n| n.global_id == global_id)
                {
                    log::debug!("{} gone from the graph", chain.node_name);
                    chain.node = None;
                }
            }
        }
    }

    /// Periodic housekeeping from the engine timer: debounced config save.
    pub fn tick(&mut self) {
        self.flush_config();
    }
}
