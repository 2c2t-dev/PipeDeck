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

pub mod args;
pub mod filter_chain;
mod focus;
pub mod loopback;
pub mod meter;
pub mod module;
pub mod plugin_chain;
pub mod props;
mod voices;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use libspa::param::ParamType;
use libspa::pod::Pod;
use libspa::utils::dict::DictRef;
use pipewire::context::ContextRc;
use pipewire::core::CoreRc;
use pipewire::link::Link;
use pipewire::metadata::Metadata;
use pipewire::node::{Node, NodeListener};
use pipewire::properties::properties;
use pipewire::proxy::{ProxyListener, ProxyT};
use pipewire::registry::{GlobalObject, RegistryRc};
use pipewire::types::ObjectType;

use crate::config::Config;
use crate::engine::{Event, StateSnapshot};
use crate::error::EngineError;
use crate::types::{node_prefix, voice_app};
use crate::types::{
    App, CallMember, ChainState, Device, Effect, EffectKind, LinkConfig, MixConfig, MixId,
    MixOutput, SourceConfig, SourceId,
};
use crate::vst3::Plugin;

use filter_chain::ChainSpec;
use loopback::{LoopbackSpec, AUDIO_POSITION, CHANNELS};
use meter::Meter;
use module::LoadedModule;
use plugin_chain::{PluginChain, Request};
use voices::{StreamTargets, Voice};

/// What the nodes of every Pipedeck start with, whatever prefix each was
/// given: a mixer's nodes are never an application's, this one's or
/// another's.
const NODE_PREFIX: &str = "pipedeck";
/// Marks the nodes of this mixer. Node names come from ids that are private
/// to each mixer, so two Pipedecks on one graph answer to the same names;
/// this says which ones are ours.
const INSTANCE_KEY: &str = "pipedeck.instance";
const ADAPTER_FACTORY: &str = "adapter";
const NULL_SINK_FACTORY: &str = "support.null-audio-sink";
const LOOPBACK_MODULE: &str = "libpipewire-module-loopback";
const FILTER_CHAIN_MODULE: &str = "libpipewire-module-filter-chain";

/// Proxies of objects about to go, held a while before they are let go.
///
/// Destroying a loopback module takes its nodes with it, and the links to
/// them, and the server frees the bindings we hold on them at the same
/// moment — on the module's own connection, which is not ordered with ours.
/// Dropping such a proxy right away races the notification telling us it is
/// gone, and the server answers our destroy with "unknown resource". Once
/// the notification has landed, dropping it says nothing on the wire. One
/// turn of the loop was usually enough and sometimes not, under load; two
/// seconds always is.
struct Quarantine<T> {
    held: Vec<(Instant, T)>,
}

impl<T> Quarantine<T> {
    const HOLD: Duration = Duration::from_secs(2);

    fn new() -> Self {
        Self { held: Vec::new() }
    }

    fn hold(&mut self, item: T) {
        self.held.push((Instant::now(), item));
    }

    /// Let go of what has been held long enough.
    fn release(&mut self) {
        self.held.retain(|(since, _)| since.elapsed() < Self::HOLD);
    }
}

/// Where a level read back from the graph belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Owner {
    Source(SourceId),
    Mix(MixId),
}

/// A node proxy bound from the registry.
struct BoundNode {
    global_id: u32,
    proxy: Node,
}

/// The node a channel ends on, and whether it is a sink, whose monitor is
/// what a reader captures, rather than a source to read straight.
///
/// Its plug-ins if it has any, else what PipeWire runs for it, else the
/// channel itself — which for a row bound to a capture device is that
/// device, and for any other row is its own sink.
pub fn channel_output(source: &SourceConfig) -> (String, bool) {
    if source.effects.iter().any(|effect| effect.is_plugin()) {
        (source.id.plugins_node_name(), true)
    } else if !source.effects.is_empty() {
        (source.id.effects_node_name(), true)
    } else {
        channel_input(source)
    }
}

/// The node a channel starts on: the device it captures, or its own sink.
pub fn channel_input(source: &SourceConfig) -> (String, bool) {
    match &source.device {
        Some(device) => (device.clone(), false),
        None => (source.id.sink_node_name(), true),
    }
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

/// One port of a node, kept so that two nodes can be joined by hand.
struct Port {
    /// What the server calls it, which is what a link is asked for.
    global_id: u32,
    /// Which of FL, FR... it carries, so the two sides are joined in order.
    channel: String,
    input: bool,
}

/// One loopback with a level on it: a cell of the matrix, or one output of a
/// mix. Both are the same object in the graph, so they are the same here.
struct Stage {
    /// `node.name` of the playback node carrying the level.
    node_name: String,
    /// The links joining this stage to what it feeds, when the mixer made
    /// them itself. They go before the node, which goes before the module.
    links: Vec<Link>,
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

/// A column: the sink a capture client reads, plus one loopback per device,
/// and whatever the mix runs between its cells and that sink.
struct Mix {
    // Field order matters: the outputs are destroyed before the node they
    // read.
    /// One slot per output in the config, at the same index, empty for an
    /// output switched off or one that failed to load.
    outputs: Vec<Option<Stage>>,
    _sink_listener: NodeListener,
    _sink_bound: ProxyListener,
    /// The mix itself: an input device to the rest of the system, and the
    /// node the cells are linked into. It carries the master level, which it
    /// applies to what it hands on, so that scales the outputs and a capture
    /// client alike.
    sink: Node,
}

/// A row. Virtual rows own a sink; input rows capture a device directly and
/// own nothing in the graph.
/// A row's effects: the chain that runs them, and the sink it plays into.
struct Effects {
    // Field order matters: the chain goes before the sink it feeds.
    module: Option<LoadedModule>,
    _bound: ProxyListener,
    #[allow(dead_code)] // held only to keep the remote object alive
    sink: Node,
}

struct Source {
    /// The plug-ins the mixer hosts itself, after the filter chain.
    plugins: Option<PluginChain>,
    /// The sink the plug-ins play into, kept while they run.
    plugins_sink: Option<Node>,
    _plugins_bound: Option<ProxyListener>,
    /// The effects chain, when the row has one. It is dropped before the
    /// sink it captures.
    effects: Option<Effects>,
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
    /// The process playing it, as the application says.
    pid: Option<u32>,
    /// Sent by the application to one of our voice sinks: it is already
    /// where it should be, and moving it to the row's sink with the rest of
    /// the application would put that person back in with everyone.
    pinned: bool,
    /// That sink's name, for a pinned stream.
    voice: Option<String>,
    /// Whether it has been put where it goes, for a stream that waits to
    /// say where it was sent.
    placed: bool,
    /// For a stream of the call's application, the node bound to read what
    /// it was sent to, which the registry does not say.
    _watch: Option<(Node, NodeListener)>,
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

/// Which of a row's hosted plug-ins the effect at `index` is: the chain
/// counts only the effects the mixer hosts, the user sees all of them.
fn hosted_index(effects: &[Effect], index: usize) -> usize {
    effects
        .iter()
        .take(index)
        .filter(|effect| effect.is_plugin())
        .count()
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
    /// The people of a call, each on a sink playing into their row's: they
    /// go before the rows.
    voices: HashMap<(SourceId, String), Voice>,
    /// Who is in the call Vesktop is in, as its plugin last said.
    call: Vec<CallMember>,
    mixes: HashMap<MixId, Mix>,
    sources: HashMap<SourceId, Source>,
    /// Playback node name -> what it belongs to. Filled before the module is
    /// loaded, so a registry announcement always finds its owner.
    stage_index: HashMap<String, StageRef>,
    /// Sinks the server has just named: their `node.name` and the id it gave
    /// them, which is how a meter points at our node rather than at a name
    /// another mixer answers to.
    bound_sinks: Rc<RefCell<Vec<(String, u32)>>>,
    sink_ids: HashMap<String, u32>,
    /// The ports of every node, so that two of them can be joined by hand:
    /// nothing routes into a mix, which is a source.
    ports: HashMap<u32, Vec<Port>>,
    /// Which node a port belongs to, for when the server takes one away.
    port_owner: HashMap<u32, u32>,
    /// What each row's meter is currently listening to: the node's name, and
    /// the id the server gave it. The id matters because reloading a chain
    /// puts a new node behind the same name, and a meter left on the old one
    /// hears nothing while looking right.
    meter_targets: HashMap<SourceId, (String, Option<u32>)>,
    /// Levels the graph reported on our own sinks, waiting to be taken in.
    ///
    /// A sink's volume is the system's volume: the user can move it from
    /// pavucontrol or a media key, and the mixer has to agree rather than
    /// hold a number of its own. The listener only queues, because it fires
    /// while the graph is borrowed.
    incoming: Rc<RefCell<Vec<(Owner, ChainState)>>>,
    /// When the mixer last set each sink's level itself. The graph says the
    /// level back a moment later, and while a fader is dragged those words
    /// trail behind it: taken for a change made elsewhere, they would put
    /// the fader back where it was, and be saved.
    written: HashMap<Owner, std::time::Instant>,
    /// Proxies of nodes we are about to destroy. See [`Quarantine`].
    retired: Quarantine<Node>,
    devices: HashMap<u32, DeviceEntry>,
    devices_dirty: bool,
    /// One measurement per row and per column. A cell needs none: it carries
    /// its channel's signal scaled by its own fader.
    source_meters: HashMap<SourceId, Meter>,
    mix_meters: HashMap<MixId, Meter>,
    /// The plug-ins installed on the machine, read once: opening a bundle
    /// runs its code, and doing that on every tick would be both slow and
    /// rude.
    plugins: Vec<Plugin>,
    /// Where Stereo Tool stands, as it last said. Kept because asking means
    /// opening a processor, which is neither quick nor quiet.
    stereotool: crate::stereotool::Status,
    /// Set when the answer above can have changed although the library is
    /// already loaded, which only a new licence key does.
    stereotool_stale: bool,
    /// Application playback streams currently on the graph.
    streams: HashMap<u32, AppStream>,
    /// Streams of the call's application whose target has just been read:
    /// their id, and the sink they were sent to, if any. The listener only
    /// queues, for the same reason as `incoming`.
    stream_targets: StreamTargets,
    streams_dirty: bool,
    /// The window with the focus, as the desktop last said, and the
    /// application playing in it, as last told.
    focus: Option<focus::Focus>,
    focused: Option<App>,
    /// The server's `default` metadata, which is how a stream is moved from
    /// one sink to another. Bound when the registry announces it.
    metadata: Option<Metadata>,
    /// The id the server gave it. WirePlumber makes it again whenever it
    /// restarts, empty, and the one we hold is then dead: this is how its
    /// going is noticed.
    metadata_id: Option<u32>,
    /// Proxies of objects the server has just taken away, held until the next
    /// tick for the same reason as `retired`.
    retired_metadata: Quarantine<Metadata>,
    retired_links: Quarantine<Link>,
    /// The links we made, by the id the server gave each, and the cell it
    /// joins to its mix. A link can go without us — WirePlumber restarting
    /// renegotiates the streams at either end, and their ports go with it —
    /// and a cell whose link has gone is a cell nobody hears.
    link_owner: HashMap<u32, (SourceId, MixId)>,
    /// The same for the links joining a person's sink to their row's.
    voice_link_owner: HashMap<u32, (SourceId, String)>,
    /// The clients this process holds on the server. A node's `client.id`
    /// says whether it is ours, which its name cannot: another mixer's
    /// nodes answer to the same names.
    own_clients: std::collections::HashSet<u32>,
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
            voices: HashMap::new(),
            call: Vec::new(),
            stream_targets: Rc::new(RefCell::new(Vec::new())),
            mixes: HashMap::new(),
            sources: HashMap::new(),
            stage_index: HashMap::new(),
            bound_sinks: Rc::new(RefCell::new(Vec::new())),
            sink_ids: HashMap::new(),
            ports: HashMap::new(),
            port_owner: HashMap::new(),
            meter_targets: HashMap::new(),
            incoming: Rc::new(RefCell::new(Vec::new())),
            written: HashMap::new(),
            retired: Quarantine::new(),
            devices: HashMap::new(),
            devices_dirty: false,
            plugins: crate::vst3::installed(),
            stereotool: crate::stereotool::Status::Absent,
            stereotool_stale: false,
            source_meters: HashMap::new(),
            mix_meters: HashMap::new(),
            streams: HashMap::new(),
            streams_dirty: false,
            focus: None,
            focused: None,
            metadata: None,
            metadata_id: None,
            retired_metadata: Quarantine::new(),
            retired_links: Quarantine::new(),
            link_owner: HashMap::new(),
            voice_link_owner: HashMap::new(),
            own_clients: std::collections::HashSet::new(),
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
            stereotool_license: self.config.stereotool_license.clone(),
            listen_device: self.listen_device(),
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
    fn watch_sink_id(&self, sink: &Node, node_name: String) -> ProxyListener {
        let queue = self.bound_sinks.clone();
        sink.upcast_ref()
            .add_listener_local()
            .bound(move |global_id| queue.borrow_mut().push((node_name.clone(), global_id)))
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
        self.create_node(node_name, description, "Audio/Sink")
    }

    /// A sink only the mixer plays into and reads: what a channel's effects
    /// or plug-ins hand on. Its class keeps it out of the system's list of
    /// outputs, where nobody has any reason to pick it, as a meter's keeps
    /// it out of the recording applications. It is played into and captured
    /// like any other.
    fn create_internal_sink(
        &self,
        node_name: String,
        description: String,
    ) -> Result<Node, EngineError> {
        self.create_node(node_name, description, "Audio/Sink/Internal")
    }

    /// The node a mix collects into.
    ///
    /// A mix is something you record, so it is a source: the system lists it
    /// among the microphones and nowhere else, which is what a mix is to
    /// anyone using it. Underneath it is the same null sink as any other
    /// node here — it has input ports, and what plays into them comes out of
    /// its capture ports — but nothing routes into a source on its own, so
    /// whatever feeds it is linked by hand. See [`Graph::hook_up_links`].
    fn create_mix_node(&self, node_name: String, description: String) -> Result<Node, EngineError> {
        self.create_node(node_name, description, "Audio/Source/Virtual")
    }

    fn create_node(
        &self,
        node_name: String,
        description: String,
        class: &str,
    ) -> Result<Node, EngineError> {
        self.core
            .create_object::<Node>(
                ADAPTER_FACTORY,
                &properties! {
                    "factory.name" => NULL_SINK_FACTORY,
                    "node.name" => node_name,
                    "node.description" => description,
                    "media.class" => class,
                    "audio.position" => AUDIO_POSITION,
                    INSTANCE_KEY => instance(),
                    // The mixer owns these levels. Without this the session
                    // manager restores whatever it saved last time, over the
                    // value the config just asked for.
                    "state.restore-props" => "false",
                    // Apply the node's volume (what a mixer applet shows) to
                    // the ports it hands on, so a row's is a pre-fader trim
                    // on every cell of it and a column's is its master. A
                    // source made this way carries its volume the same way.
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
    fn load_outputs(&mut self, cfg: &MixConfig) -> Vec<Option<Stage>> {
        (0..cfg.outputs.len())
            .map(|index| self.load_output(cfg, index))
            .collect()
    }

    /// The loopback sending a mix to one of its devices, unless that output
    /// is switched off, in which case the device is left alone entirely.
    fn load_output(&mut self, cfg: &MixConfig, index: usize) -> Option<Stage> {
        let output = cfg.outputs.get(index)?;
        if !output.enabled {
            return None;
        }
        let node_name = loopback::output_node_name(cfg.id, index);
        self.stage_index
            .insert(node_name.clone(), StageRef::Output(cfg.id, index));
        let spec = LoopbackSpec::for_output(cfg, index, &output.device, &self.config.latency);
        match LoadedModule::load(&self.context, LOOPBACK_MODULE, &spec.to_args()) {
            Ok(module) => Some(Stage {
                node_name,
                links: Vec::new(),
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
                None
            }
        }
    }

    /// Switch one output of a mix on or off, leaving the others playing.
    pub fn set_output_enabled(
        &mut self,
        id: MixId,
        index: usize,
        enabled: bool,
    ) -> Result<(), EngineError> {
        let cfg = self.config.mix_mut(id).ok_or(EngineError::UnknownMix(id))?;
        let output = cfg
            .outputs
            .get_mut(index)
            .ok_or(EngineError::UnknownOutput(id, index))?;
        if output.enabled == enabled {
            return Ok(());
        }
        output.enabled = enabled;
        let cfg = cfg.clone();
        self.dirty = true;

        let previous = self
            .mixes
            .get_mut(&id)
            .and_then(|mix| mix.outputs.get_mut(index))
            .and_then(Option::take);
        if let Some(mut stage) = previous {
            self.stage_index.remove(&stage.node_name);
            self.retire(&mut stage);
        }
        let loaded = self.load_output(&cfg, index);
        if let Some(slot) = self
            .mixes
            .get_mut(&id)
            .and_then(|mix| mix.outputs.get_mut(index))
        {
            *slot = loaded;
        }
        log::info!(
            "{} {} {}",
            cfg.name,
            if enabled {
                "plays to"
            } else {
                "no longer plays to"
            },
            cfg.outputs[index].device
        );
        Ok(())
    }

    /// Hear a mix in your headphones, or stop.
    ///
    /// That is its output to the device you listen on, switched on or off;
    /// one is added if the mix has none there yet. Its other outputs are
    /// left as they are.
    pub fn set_listening(&mut self, id: MixId, listening: bool) -> Result<(), EngineError> {
        let device = self
            .listen_device()
            .or_else(|| self.device_lists().0.first().map(|d| d.name.clone()))
            .ok_or(EngineError::NoListenDevice)?;
        // Heard somewhere means the somewhere is now the one picked.
        self.config.listen_device = Some(device.clone());
        self.dirty = true;

        let cfg = self.config.mix(id).ok_or(EngineError::UnknownMix(id))?;
        match cfg
            .outputs
            .iter()
            .position(|output| output.device == device)
        {
            Some(index) => self.set_output_enabled(id, index, listening),
            None if listening => {
                let mut devices: Vec<String> =
                    cfg.outputs.iter().map(|o| o.device.clone()).collect();
                devices.push(device);
                self.set_mix_outputs(id, devices)
            }
            None => Ok(()),
        }
    }

    /// Listen on another device: every mix heard on the old one is moved,
    /// its output there switched off and one to the new device switched on.
    pub fn set_listen_device(&mut self, device: String) -> Result<(), EngineError> {
        let old = self.listen_device();
        self.config.listen_device = Some(device.clone());
        self.dirty = true;
        if old.as_deref() == Some(device.as_str()) {
            return Ok(());
        }
        let heard: Vec<MixId> = self
            .config
            .mixes
            .iter()
            .filter(|mix| {
                mix.outputs
                    .iter()
                    .any(|o| o.enabled && Some(&o.device) == old.as_ref())
            })
            .map(|mix| mix.id)
            .collect();
        for id in heard {
            let Some(cfg) = self.config.mix_mut(id) else {
                continue;
            };
            if cfg.output(&device).is_none() {
                cfg.outputs.push(MixOutput::new(device.clone()));
            }
            for output in &mut cfg.outputs {
                if Some(&output.device) == old.as_ref() {
                    output.enabled = false;
                }
                if output.device == device {
                    output.enabled = true;
                }
            }
            let devices: Vec<String> = cfg.outputs.iter().map(|o| o.device.clone()).collect();
            // The mix's outputs are loaded again: switching cards is a moment
            // the audio moves anyway, and this keeps one path for it.
            self.set_mix_outputs(id, devices)?;
        }
        log::info!("listening on {device} now");
        Ok(())
    }

    /// Take a stage's proxy out before the stage, and its module, go.
    fn retire(&mut self, stage: &mut Stage) {
        // The links go into quarantine with the node, not straight away. The
        // node belongs to the loopback module, which talks to the server on a
        // connection of its own; tearing the module down removes the node,
        // and the links with it, on that connection, while a destroy for the
        // links would go on ours. The two are not ordered, so the links were
        // often gone by the time ours arrived, and the server answered
        // "unknown resource". Held until the next tick, they learn they are
        // gone first, and letting them go then says nothing on the wire.
        for link in stage.links.drain(..) {
            self.retired_links.hold(link);
        }
        if let Some(bound) = stage.node.take() {
            self.retired.hold(bound.proxy);
        }
    }

    /// What a row's meter should be listening to.
    ///
    /// A row with effects is measured at the end of its chain, so what the
    /// meter shows is what every mix hears, effects and all. An input row is
    /// measured at its device, which is the only thing it has.
    fn meter_target(&self, cfg: &SourceConfig) -> (String, bool) {
        channel_output(cfg)
    }

    /// Point every meter at what it should be listening to.
    ///
    /// Sinks are named by the server only once the loop runs again, so this
    /// is a convergence rather than a one-off: a meter appears as soon as
    /// its target has an id, and moves when the row changes under it.
    fn hook_up_meters(&mut self) {
        for (name, global_id) in self.bound_sinks.borrow_mut().drain(..) {
            self.sink_ids.insert(name, global_id);
        }

        let mixes: Vec<MixId> = self.config.mixes.iter().map(|mix| mix.id).collect();
        for id in mixes {
            if self.mix_meters.contains_key(&id) {
                continue;
            }
            let name = id.sink_node_name();
            let Some(global_id) = self.sink_ids.get(&name).copied() else {
                continue;
            };
            // A mix is a source, so its meter reads it straight rather than
            // through the monitor of a sink.
            if let Some(meter) = self.watch_level(
                &format!("{}.meter.mix.{id}", node_prefix()),
                &name,
                Some(global_id),
                false,
            ) {
                self.mix_meters.insert(id, meter);
            }
        }

        let sources: Vec<SourceConfig> = self.config.sources.clone();
        for cfg in sources {
            let (target, from_sink) = self.meter_target(&cfg);
            // A device has one name on the graph, so it needs no id; one of
            // our sinks does, and it may not have been named yet.
            let global_id = if from_sink {
                match self.sink_ids.get(&target).copied() {
                    Some(id) => Some(id),
                    None => continue,
                }
            } else {
                None
            };
            if self.meter_targets.get(&cfg.id) == Some(&(target.clone(), global_id)) {
                continue;
            }
            self.source_meters.remove(&cfg.id);
            if let Some(meter) = self.watch_level(
                &format!("{}.meter.src.{}", node_prefix(), cfg.id),
                &target,
                global_id,
                from_sink,
            ) {
                self.source_meters.insert(cfg.id, meter);
                self.meter_targets.insert(cfg.id, (target, global_id));
            }
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
        if self.source_meters.is_empty() && self.mix_meters.is_empty() && self.voices.is_empty() {
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
            voices: self
                .voices
                .iter()
                .filter_map(|((row, user), voice)| {
                    Some((*row, user.clone(), voice.meter.as_ref()?.take()))
                })
                .collect(),
            effects: self.effect_levels(),
        });
    }

    /// What every running compressor, de-esser and noise suppression is
    /// doing, by its row and its place in the row's chain.
    fn effect_levels(&self) -> Vec<crate::engine::EffectLevel> {
        let mut levels = Vec::new();
        for cfg in &self.config.sources {
            let Some(chain) = self.sources.get(&cfg.id).and_then(|s| s.plugins.as_ref()) else {
                continue;
            };
            for (index, effect) in cfg.effects.iter().enumerate() {
                let says = matches!(effect.label.as_str(), "compressor" | "deesser" | "denoise");
                if effect.kind != EffectKind::Native || !says {
                    continue;
                }
                let Some(params) = chain.params(hosted_index(&cfg.effects, index)) else {
                    continue;
                };
                let (level, reduction) = params.live.take();
                levels.push(crate::engine::EffectLevel {
                    source: cfg.id,
                    index,
                    level,
                    reduction,
                });
            }
        }
        levels
    }

    fn forget_outputs(&mut self, id: MixId) {
        self.stage_index
            .retain(|_, owner| !matches!(owner, StageRef::Output(mix, _) if *mix == id));
    }

    // --- mixes --------------------------------------------------------------

    /// The device you listen on: the one picked, else the first a mix
    /// plays to, so that someone who never touched the switch hears what
    /// they already heard.
    pub fn listen_device(&self) -> Option<String> {
        self.config.listen_device.clone().or_else(|| {
            self.config
                .mixes
                .iter()
                .flat_map(|mix| mix.outputs.iter())
                .find(|output| output.enabled)
                .map(|output| output.device.clone())
        })
    }

    pub fn create_mix(&mut self, cfg: &MixConfig) -> Result<(), EngineError> {
        if self.mixes.contains_key(&cfg.id) {
            return Ok(());
        }
        let sink =
            self.create_mix_node(cfg.id.sink_node_name(), format!("Pipedeck {}", cfg.name))?;
        apply_props(&sink, &cfg.id.sink_node_name(), &cfg.state());
        let listener = self.watch_sink(&sink, Owner::Mix(cfg.id));
        // The sink has no global id yet: the meter is hooked up when the
        // registry announces it, which is also how we tell our sink from the
        // one another mixer gave the same name.
        let bound = self.watch_sink_id(&sink, cfg.id.sink_node_name());
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

    /// Make a mix again, under the name it now has.
    ///
    /// A mix *is* the input device someone picks in a list, and a node
    /// carries the description it was born with, so a rename means a new
    /// node — and with it the cells that feed it and the outputs that read
    /// it. A recorder has to pick it again; that is the price of the name
    /// being true.
    pub fn rebuild_mix(&mut self, id: MixId) -> Result<(), EngineError> {
        let Some(cfg) = self.config.mix(id).cloned() else {
            return Ok(());
        };
        let cells: Vec<LinkConfig> = self
            .config
            .links
            .iter()
            .filter(|link| link.mix == id)
            .copied()
            .collect();
        self.remove_mix(id)?;
        self.create_mix(&cfg)?;
        for link in &cells {
            if let Err(e) = self.create_link(link) {
                log::error!("{e}");
                self.emit(Event::Error(e.to_string()));
            }
        }
        Ok(())
    }

    /// Make a row again under its new name, as a mix is: its sink, and the
    /// effects and plug-ins after it, carry the description they were born
    /// with, which is what the system lists them by. With them go the cells
    /// that read it, made again, its applications, sent back to it, and the
    /// people of a call it carries, given sinks again.
    pub fn rebuild_source(&mut self, id: SourceId) -> Result<(), EngineError> {
        let Some(cfg) = self.config.source(id).cloned() else {
            return Ok(());
        };
        let cells: Vec<LinkConfig> = self
            .config
            .links
            .iter()
            .filter(|link| link.source == id)
            .copied()
            .collect();
        self.remove_source(id)?;
        self.create_source(&cfg)?;
        for link in &cells {
            if let Err(e) = self.create_link(link) {
                log::error!("{e}");
                self.emit(Event::Error(e.to_string()));
            }
        }
        for key in &cfg.apps {
            self.move_app(key, Some(id));
        }
        self.sync_voices();
        Ok(())
    }

    /// Tear a mix down, cells first. Only ever called from the command
    /// handler, never from a listener (see [`LoadedModule`]).
    pub fn remove_mix(&mut self, id: MixId) -> Result<(), EngineError> {
        let mut mix = self.mixes.remove(&id).ok_or(EngineError::UnknownMix(id))?;
        self.mix_meters.remove(&id);
        self.sink_ids.remove(&id.sink_node_name());
        for stage in mix.outputs.iter_mut().flatten() {
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
        for stage in previous.iter_mut().flatten() {
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
        self.written
            .insert(Owner::Mix(id), std::time::Instant::now());
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

        // An output switched off has no loopback to carry the level; it is
        // kept for when it comes back on.
        if let Some(stage) = self
            .mixes
            .get_mut(&id)
            .and_then(|mix| mix.outputs.get_mut(index))
            .and_then(Option::as_mut)
        {
            stage.wanted = state;
            stage.apply();
        }
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
                let bound = self.watch_sink_id(&sink, cfg.id.sink_node_name());
                (Some(sink), Some((listener, bound)))
            }
        };
        // The effects chain reads the row's sink and offers a sink of its
        // own, which is what the cells capture from then on. The plug-ins
        // the mixer hosts come after it, on a sink of their own again.
        let effects = self.load_effects(&ChainSpec::for_source(cfg));
        let (plugins_sink, plugins_bound) = self.load_plugins_sink(
            cfg.effects.iter().any(|effect| effect.is_plugin()),
            cfg.id.plugins_node_name(),
            format!("Pipedeck: {} plug-ins", cfg.name),
            &cfg.name,
        );

        // A virtual row is measured on its sink's monitor, an input row on
        // the device it captures, which is the same signal every cell gets.
        let (sink_listener, sink_bound) = match listeners {
            Some((listener, bound)) => (Some(listener), Some(bound)),
            None => (None, None),
        };
        self.sources.insert(
            cfg.id,
            Source {
                plugins: None,
                plugins_sink,
                _plugins_bound: plugins_bound,
                effects,
                sink,
                _sink_listener: sink_listener,
                _sink_bound: sink_bound,
            },
        );
        log::info!("source {} ({}) created", cfg.id, cfg.name);
        Ok(())
    }

    pub fn remove_source(&mut self, id: SourceId) -> Result<(), EngineError> {
        // Its applications are handed back first: left aimed at a sink that
        // is about to go, they would stay aimed at its name, which another
        // mixer, or the next row given this id, answers to.
        let apps: Vec<String> = self
            .config
            .source(id)
            .map(|cfg| cfg.apps.clone())
            .unwrap_or_default();
        for key in &apps {
            self.move_app(key, None);
        }
        // The voices play into the row's sink, so they go before it.
        let voices: Vec<(SourceId, String)> = self
            .voices
            .keys()
            .filter(|(row, _)| *row == id)
            .cloned()
            .collect();
        for key in voices {
            self.drop_voice(&key);
        }
        let source = self
            .sources
            .remove(&id)
            .ok_or(EngineError::UnknownSource(id))?;
        self.source_meters.remove(&id);
        self.meter_targets.remove(&id);
        self.sink_ids.remove(&id.sink_node_name());
        self.sink_ids.remove(&id.effects_node_name());
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

    /// Load the effects PipeWire runs for a channel or a mix, if there are
    /// any.
    ///
    /// The chain plays into a sink of ours rather than being one itself:
    /// asking a filter chain to be a sink crashes PipeWire 1.6, and whoever
    /// reads it wants a monitor to capture in any case.
    fn load_effects(&self, spec: &ChainSpec<'_>) -> Option<Effects> {
        // Plug-ins are hosted by the mixer, so an object that has only those
        // needs nothing from PipeWire and no sink to put it in.
        if !spec.effects.iter().any(|effect| !effect.is_plugin()) {
            return None;
        }
        let sink = match self.create_internal_sink(
            spec.node.clone(),
            format!("Pipedeck: {} effects", spec.owner),
        ) {
            Ok(sink) => sink,
            Err(e) => {
                log::error!("{e}");
                self.emit(Event::Error(format!(
                    "cannot run the effects of {}: {e}",
                    spec.owner
                )));
                return None;
            }
        };
        let bound = self.watch_sink_id(&sink, spec.node.clone());
        Some(Effects {
            module: self.load_chain(spec),
            _bound: bound,
            sink,
        })
    }

    /// Make the sink the hosted plug-ins hang on: the one a channel's play
    /// into, the one a mix's read.
    ///
    /// The chain itself waits: it has to be told the ids of the nodes it
    /// sits between, and the server names them only once the loop runs
    /// again. [`Graph::hook_up_plugins`] finishes the job.
    fn load_plugins_sink(
        &self,
        wanted: bool,
        node_name: String,
        description: String,
        owner: &str,
    ) -> (Option<Node>, Option<ProxyListener>) {
        if !wanted {
            return (None, None);
        }
        match self.create_internal_sink(node_name.clone(), description) {
            Ok(sink) => {
                let bound = self.watch_sink_id(&sink, node_name);
                (Some(sink), Some(bound))
            }
            Err(e) => {
                log::error!("{e}");
                self.emit(Event::Error(format!(
                    "cannot run the plug-ins of {owner}: {e}"
                )));
                (None, None)
            }
        }
    }

    /// The id the server gave a device, by the name the config keeps.
    fn device_id(&self, name: &str) -> Option<u32> {
        self.devices
            .iter()
            .find(|(_, entry)| entry.device.name == name)
            .map(|(id, _)| *id)
    }

    /// The plug-ins an object asks for, resolved against what is installed.
    ///
    /// One slot per hosted effect, empty for one that cannot be had — a
    /// plug-in uninstalled since — so that the effects after it keep their
    /// place: the chain is told "the third" by counting the hosted effects,
    /// and an effect left out would hand its settings to the next one.
    fn wanted_plugins(&self, effects: &[Effect]) -> Vec<Option<Request>> {
        let license = self.config.stereotool_license.as_deref();
        effects
            .iter()
            .filter(|effect| effect.is_plugin())
            .map(|effect| Request::resolve(effect, &self.plugins, license))
            .collect()
    }

    /// Open the chains whose two ends the server has now named.
    ///
    /// A channel's plug-ins come last, so they read what the channel ends on
    /// and play into a sink of their own; a mix's come just before its sink,
    /// so they read a sink of their own and play into the mix. Both are the
    /// same pair of streams between two node ids.
    fn hook_up_plugins(&mut self) {
        let sources: Vec<SourceConfig> = self.config.sources.clone();
        for cfg in sources {
            let wants = cfg.effects.iter().any(|effect| effect.is_plugin());
            let running = self
                .sources
                .get(&cfg.id)
                .is_some_and(|source| source.plugins.is_some());
            if !wants || running {
                continue;
            }

            // What the plug-ins read: whatever PipeWire runs for the row,
            // or the row itself — which for a row bound to a microphone is
            // that microphone, and is read straight rather than through a
            // monitor.
            let (from_name, from_sink) = if cfg.effects.iter().any(|effect| !effect.is_plugin()) {
                (cfg.id.effects_node_name(), true)
            } else {
                channel_input(&cfg)
            };
            let from = if from_sink {
                self.sink_ids.get(&from_name).copied()
            } else {
                self.device_id(&from_name)
            };
            let (Some(from), Some(into)) = (
                from,
                self.sink_ids.get(&cfg.id.plugins_node_name()).copied(),
            ) else {
                continue;
            };

            let plugins = self.wanted_plugins(&cfg.effects);
            if plugins.iter().all(Option::is_none) {
                continue;
            }
            let bypassed: Vec<bool> = cfg
                .effects
                .iter()
                .filter(|effect| effect.is_plugin())
                .map(|effect| effect.bypassed)
                .collect();
            let chain = PluginChain::new(
                &self.core,
                &cfg.id.plugins_node_name(),
                &cfg.name,
                &plugins,
                &bypassed,
                from,
                from_sink,
                into,
                &self.config.latency,
            );
            match chain {
                Ok(chain) => {
                    if let Some(source) = self.sources.get_mut(&cfg.id) {
                        source.plugins = Some(chain);
                    }
                }
                Err(e) => {
                    log::error!("{e}");
                    self.emit(Event::Error(format!(
                        "cannot run the plug-ins of {}: {e}",
                        cfg.name
                    )));
                    // Asking again every tick would spin; the row keeps its
                    // sink and stays silent until something changes.
                    if let Some(source) = self.sources.get_mut(&cfg.id) {
                        source.plugins_sink = None;
                    }
                    self.sink_ids.remove(&cfg.id.plugins_node_name());
                }
            }
        }
    }

    /// Load the chain itself, against a sink that already exists.
    fn load_chain(&self, spec: &ChainSpec<'_>) -> Option<LoadedModule> {
        let args = filter_chain::args(spec, &self.config.latency)?;
        match LoadedModule::load(&self.context, FILTER_CHAIN_MODULE, &args) {
            Ok(module) => {
                log::info!("{} runs {} effect(s)", spec.owner, spec.effects.len());
                Some(module)
            }
            Err(e) => {
                log::error!("{e}");
                self.emit(Event::Error(format!(
                    "cannot run the effects of {}: {e}",
                    spec.owner
                )));
                None
            }
        }
    }

    /// Replace a row's effects, and point its cells at whatever now ends its
    /// chain.
    ///
    /// The chain is a module, fixed when it is loaded, so a change reloads
    /// it and the cells that read it. The audio stops for as long as that
    /// takes, which is why the interface waits for a slider to settle before
    /// asking.
    pub fn set_effects(&mut self, id: SourceId, effects: Vec<Effect>) -> Result<(), EngineError> {
        let cfg = self
            .config
            .source_mut(id)
            .ok_or(EngineError::UnknownSource(id))?;
        let before = std::mem::replace(&mut cfg.effects, effects);
        let cfg = cfg.clone();
        self.dirty = true;

        if self.retune_effects(id, &before, &cfg) {
            return Ok(());
        }

        let (had_chain, had_plugins) = self.sources.get(&id).map_or((false, false), |source| {
            (source.effects.is_some(), source.plugins.is_some())
        });
        let has_chain = !cfg.effects.is_empty();
        let has_plugins = cfg.effects.iter().any(|effect| effect.is_plugin());
        if had_chain && has_chain && !had_plugins && !has_plugins {
            self.reload_chain(id, &cfg);
        } else {
            self.rebuild_effects(id, &cfg);
        }
        Ok(())
    }

    /// Give a row's running effects their new settings where they run, when
    /// the effects are the same in the same order. Says whether that was
    /// all there was to do: only PipeWire's own filters, fixed when their
    /// module is loaded, need it made again, and only when one of theirs
    /// moved.
    fn retune_effects(&self, id: SourceId, before: &[Effect], cfg: &SourceConfig) -> bool {
        let same = before.len() == cfg.effects.len()
            && before
                .iter()
                .zip(&cfg.effects)
                .all(|(old, new)| old.same_effect(new));
        let running = self
            .sources
            .get(&id)
            .is_some_and(|source| source.plugins.is_some() || source.effects.is_some());
        if !same || !running {
            return false;
        }
        let hosted = cfg.effects.iter().filter(|effect| effect.is_plugin());
        if let Some(chain) = self.sources.get(&id).and_then(|s| s.plugins.as_ref()) {
            for (index, effect) in hosted.enumerate() {
                chain.set_params(index, &effect.controls);
                chain.set_bypass(index, effect.bypassed);
            }
        }
        let filters_moved = before.iter().zip(&cfg.effects).any(|(old, new)| {
            !new.is_plugin() && (old.controls != new.controls || old.bypassed != new.bypassed)
        });
        !filters_moved
    }

    /// Load a row's chain again behind the sink it has. Changing what a
    /// chain runs keeps its sink: recreating that sink would put a second
    /// node behind the same name for a moment, and the new chain would as
    /// likely feed the old one as the new.
    fn reload_chain(&mut self, id: SourceId, cfg: &SourceConfig) {
        if let Some(fx) = self
            .sources
            .get_mut(&id)
            .and_then(|source| source.effects.as_mut())
        {
            fx.module = None;
        }
        let module = self.load_chain(&ChainSpec::for_source(cfg));
        if let Some(fx) = self
            .sources
            .get_mut(&id)
            .and_then(|source| source.effects.as_mut())
        {
            fx.module = module;
        }
    }

    /// Make a row's effects again from nothing. The chain appears or goes,
    /// so the cells change what they read, and are made again too.
    fn rebuild_effects(&mut self, id: SourceId, cfg: &SourceConfig) {
        let cells: Vec<LinkConfig> = self
            .config
            .links
            .iter()
            .filter(|link| link.source == id)
            .copied()
            .collect();
        for link in &cells {
            self.drop_link((link.source, link.mix));
        }
        if let Some(source) = self.sources.get_mut(&id) {
            source.plugins = None;
            source.plugins_sink = None;
            source.effects = None;
        }
        self.sink_ids.remove(&id.effects_node_name());
        self.sink_ids.remove(&id.plugins_node_name());
        let loaded = self.load_effects(&ChainSpec::for_source(cfg));
        let (plugins_sink, plugins_bound) = self.load_plugins_sink(
            cfg.effects.iter().any(|effect| effect.is_plugin()),
            cfg.id.plugins_node_name(),
            format!("Pipedeck: {} plug-ins", cfg.name),
            &cfg.name,
        );
        if let Some(source) = self.sources.get_mut(&id) {
            source.effects = loaded;
            source.plugins = None;
            source.plugins_sink = plugins_sink;
            source._plugins_bound = plugins_bound;
        }
        for link in &cells {
            if let Err(e) = self.create_link(link) {
                log::error!("{e}");
                self.emit(Event::Error(e.to_string()));
            }
        }
    }

    /// Take the Stereo Tool licence key, and open again whatever runs on it:
    /// a processor is told its key when it is created and not after.
    pub fn set_stereotool_license(&mut self, key: Option<String>) -> Result<(), EngineError> {
        let key = key
            .map(|key| key.trim().to_owned())
            .filter(|k| !k.is_empty());
        if self.config.stereotool_license == key {
            return Ok(());
        }
        self.config.stereotool_license = key;
        self.dirty = true;
        // A processor is told its key when it is created, so what the
        // library says about the licence is worth asking again.
        self.stereotool_stale = true;
        self.refresh_stereotool();
        self.reopen_stereotool();
        Ok(())
    }

    /// Close the plug-in windows whose close button has been pressed.
    fn poll_windows(&self) {
        for chain in self.sources.values().filter_map(|s| s.plugins.as_ref()) {
            chain.poll_windows();
        }
    }

    /// Put the interface of one hosted plug-in on the screen, or take it
    /// away.
    ///
    /// `index` is the effect's place in the chain the user is looking at;
    /// which plug-in that is depends on how many of the effects before it
    /// the mixer hosts rather than PipeWire, which only this side knows.
    pub fn set_effect_window(
        &self,
        id: SourceId,
        index: usize,
        open: bool,
    ) -> Result<(), EngineError> {
        let effects = self
            .config
            .source(id)
            .map(|cfg| cfg.effects.clone())
            .ok_or(EngineError::UnknownSource(id))?;
        let among_plugins = hosted_index(&effects, index);
        let Some(chain) = self
            .sources
            .get(&id)
            .and_then(|source| source.plugins.as_ref())
        else {
            if open {
                self.emit(Event::Error(
                    "that effect is not running yet; give it a moment".into(),
                ));
            }
            return Ok(());
        };
        if let Err(e) = chain.set_window(among_plugins, open) {
            log::error!("{e}");
            self.emit(Event::Error(e));
        }
        Ok(())
    }

    /// Drop the chains holding a Stereo Tool stage; the next tick opens them
    /// again, with whatever the settings now hold.
    fn reopen_stereotool(&mut self) {
        let uses = |effects: &[Effect]| {
            effects
                .iter()
                .any(|effect| effect.kind == EffectKind::StereoTool)
        };
        let sources: Vec<SourceId> = self
            .config
            .sources
            .iter()
            .filter(|cfg| uses(&cfg.effects))
            .map(|cfg| cfg.id)
            .collect();
        for id in sources {
            if let Some(source) = self.sources.get_mut(&id) {
                source.plugins = None;
            }
        }
    }

    /// New settings for one effect of a row, as a control moves.
    ///
    /// The chain is the same, so `set_effects` takes the quick way: the
    /// mixer's own effects are handed the settings where they run. Only the
    /// row's effects are sent back, not the matrix, which a setting changes
    /// nothing of.
    pub fn set_effect_params(
        &mut self,
        id: SourceId,
        index: usize,
        controls: Vec<crate::types::Control>,
    ) -> Result<(), EngineError> {
        let mut effects = self
            .config
            .source(id)
            .ok_or(EngineError::UnknownSource(id))?
            .effects
            .clone();
        let Some(effect) = effects.get_mut(index) else {
            return Ok(());
        };
        effect.controls = controls;
        self.set_effects(id, effects.clone())?;
        self.emit(Event::SourceEffects { id, effects });
        Ok(())
    }

    /// Switch the effect at `index` of a row off or on. One the mixer runs
    /// is skipped where it runs, with no gap; one PipeWire runs is left out
    /// of its chain, which is reloaded as when one of its settings moves.
    pub fn set_effect_bypass(
        &mut self,
        id: SourceId,
        index: usize,
        bypassed: bool,
    ) -> Result<(), EngineError> {
        let mut effects = self
            .config
            .source(id)
            .ok_or(EngineError::UnknownSource(id))?
            .effects
            .clone();
        let Some(effect) = effects.get_mut(index) else {
            return Ok(());
        };
        if effect.bypassed == bypassed {
            return Ok(());
        }
        effect.bypassed = bypassed;
        self.set_effects(id, effects.clone())?;
        self.emit(Event::SourceEffects { id, effects });
        Ok(())
    }

    /// Have the effect at `index` of a row listen, then set it from what it
    /// heard: the compressor to the level of a voice, the de-esser to its
    /// s.
    ///
    /// The listening happens where it runs, on the audio thread, which
    /// counts how loud the sound going into it is; the settings are worked
    /// out from those counts when it is told to finish, and sent back as a
    /// control moved by hand would be.
    pub fn learn_effect(
        &mut self,
        id: SourceId,
        index: usize,
        step: crate::engine::Learning,
    ) -> Result<(), EngineError> {
        use crate::engine::Learning;

        let effects = self
            .config
            .source(id)
            .map(|cfg| cfg.effects.clone())
            .ok_or(EngineError::UnknownSource(id))?;
        let Some(learner) = effects
            .get(index)
            .filter(|effect| effect.kind == EffectKind::Native && crate::dsp::learns(&effect.label))
            .map(|effect| effect.label.clone())
        else {
            return Ok(());
        };
        let among_plugins = hosted_index(&effects, index);
        let Some(params) = self
            .sources
            .get(&id)
            .and_then(|source| source.plugins.as_ref())
            .and_then(|chain| chain.params(among_plugins))
            .cloned()
        else {
            if step == Learning::Start {
                let e = "that effect is not running yet; give it a moment";
                log::error!("{e}");
                self.emit(Event::Error(e.into()));
            }
            return Ok(());
        };
        match step {
            Learning::Start => params.heard.listen(true),
            Learning::Cancel => params.heard.listen(false),
            Learning::Finish => {
                params.heard.listen(false);
                match crate::dsp::learn(&learner, &params.heard.counts()) {
                    Ok(controls) => self.set_effect_params(id, index, controls)?,
                    Err(e) => {
                        log::error!("{e}");
                        self.emit(Event::Error(e));
                    }
                }
            }
        }
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
            self.written
                .insert(Owner::Source(id), std::time::Instant::now());
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
                links: Vec::new(),
                node: None,
                _module: module,
                wanted: link.state(),
            },
        );
        log::info!("{} feeds {}", source.name, mix.name);
        Ok(())
    }

    fn drop_link(&mut self, cell: (SourceId, MixId)) {
        // Links we take down ourselves are not losses to mend.
        self.link_owner.retain(|_, owner| *owner != cell);
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
        // The rows' effects and plug-ins ask for a latency too, and the graph
        // runs at the smallest any node asks for: left as they were, they
        // would hold it where it was. Their chains are made again, the
        // plug-ins on the next tick.
        let sources: Vec<SourceConfig> = self.config.sources.clone();
        for cfg in &sources {
            let chained = self
                .sources
                .get(&cfg.id)
                .is_some_and(|source| source.effects.is_some());
            if chained {
                if let Some(fx) = self
                    .sources
                    .get_mut(&cfg.id)
                    .and_then(|source| source.effects.as_mut())
                {
                    fx.module = None;
                }
                let module = self.load_chain(&ChainSpec::for_source(cfg));
                if let Some(fx) = self
                    .sources
                    .get_mut(&cfg.id)
                    .and_then(|source| source.effects.as_mut())
                {
                    fx.module = module;
                }
            }
            if let Some(source) = self.sources.get_mut(&cfg.id) {
                source.plugins = None;
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
        match global.type_ {
            ObjectType::Metadata => self.remember_metadata(global),
            ObjectType::Link => self.remember_link(global),
            ObjectType::Client => self.remember_client(global),
            ObjectType::Port => self.remember_port(global),
            ObjectType::Node => self.remember_node(global),
            _ => {}
        }
    }

    /// Bind the default metadata, which says where each stream plays.
    fn remember_metadata(&mut self, global: &GlobalObject<&DictRef>) {
        let is_default = global
            .props
            .and_then(|p| p.get("metadata.name"))
            .is_some_and(|name| name == "default");
        if !is_default || self.metadata.is_some() {
            return;
        }
        match self.registry.bind::<Metadata, _>(global) {
            Ok(metadata) => {
                log::debug!("bound the default metadata");
                self.metadata = Some(metadata);
                self.metadata_id = Some(global.id);
                // A metadata made again is made empty: every application
                // sent to a channel has to be sent there again, or it plays
                // wherever it likes.
                self.reassign_apps();
            }
            Err(e) => log::error!("cannot bind the default metadata: {e}"),
        }
    }

    /// Keep the clients of this process, which made the nodes that are ours.
    fn remember_client(&mut self, global: &GlobalObject<&DictRef>) {
        let Some(props) = global.props else {
            return;
        };
        // The server fills these in from the socket and the sandbox, so
        // they cannot be claimed by anyone else. In a Flatpak, the process
        // has a number of the sandbox's own, which the server does not
        // know it by: there it goes by the sandbox's instance.
        let ours = match flatpak_instance() {
            Some(instance) => {
                props.get("pipewire.access.portal.instance_id") == Some(instance.as_str())
            }
            None => props.get("pipewire.sec.pid") == Some(std::process::id().to_string().as_str()),
        };
        if ours {
            self.own_clients.insert(global.id);
        }
    }

    /// A node: one of our stages, an application playing, or a device.
    fn remember_node(&mut self, global: &GlobalObject<&DictRef>) {
        let Some(props) = global.props else {
            return;
        };
        let Some(name) = props.get("node.name") else {
            return;
        };

        if let Some(&owner) = self.stage_index.get(name) {
            // Another mixer on the same graph answers to the same names, and a
            // stage bound to its node would set its levels — and link it —
            // from here. A node says which client made it, and the server
            // says which process each client is, so ours are the ones made
            // by a client of this process.
            let ours = props
                .get("client.id")
                .and_then(|id| id.parse::<u32>().ok())
                .is_some_and(|client| self.own_clients.contains(&client));
            if ours {
                self.bind_stage(owner, global);
            }
            return;
        }
        if name.starts_with(NODE_PREFIX) || name.starts_with(node_prefix()) {
            return;
        }

        if props
            .get("media.class")
            .is_some_and(|class| class.starts_with("Stream/Output/Audio"))
        {
            self.remember_stream(global, props);
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

    /// An application playing: sent to its channel when it has one.
    fn remember_stream(&mut self, global: &GlobalObject<&DictRef>, props: &DictRef) {
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
        // The call's application sends each person to a voice sink of
        // theirs, and its own mix wherever the user assigned it. Which is
        // which is in the stream's own properties, which the registry leaves
        // out: the node is bound to read them before it is moved.
        let watch = if app.key == voice_app() {
            self.watch_stream_target(global)
        } else {
            // An application the user has assigned lands on its row's sink
            // as soon as it starts playing.
            if let Some(source) = self.source_for_app(&app.key) {
                self.move_stream(global.id, &app.name, Some(source));
            }
            None
        };
        let pid = props
            .get("application.process.id")
            .and_then(|pid| pid.parse().ok());
        self.streams.insert(
            global.id,
            AppStream {
                app,
                pid,
                pinned: false,
                voice: None,
                placed: false,
                _watch: watch,
            },
        );
        self.streams_dirty = true;
    }

    fn bind_stage(&mut self, owner: StageRef, global: &GlobalObject<&DictRef>) {
        let stage = match owner {
            StageRef::Cell(source, mix) => self.links.get_mut(&(source, mix)),
            StageRef::Output(mix, index) => self
                .mixes
                .get_mut(&mix)
                .and_then(|mix| mix.outputs.get_mut(index))
                .and_then(Option::as_mut),
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
    /// Keep a port, so that a node of ours can be joined to another by
    /// hand. Only the ports of nodes we may have to link are worth keeping.
    fn remember_port(&mut self, global: &GlobalObject<&DictRef>) {
        let Some(props) = global.props else {
            return;
        };
        let Some(node) = props.get("node.id").and_then(|id| id.parse::<u32>().ok()) else {
            return;
        };
        let input = props.get("port.direction") == Some("in");
        let channel = props.get("audio.channel").unwrap_or("MONO").to_owned();
        self.port_owner.insert(global.id, node);
        self.ports.entry(node).or_default().push(Port {
            global_id: global.id,
            channel,
            input,
        });
    }

    /// Send every running application back to the channel it belongs to.
    fn reassign_apps(&self) {
        for (id, stream) in &self.streams {
            if let Some(voice) = &stream.voice {
                self.pin_stream(*id, &stream.app.name, voice);
            } else if stream.app.key == voice_app() && !stream.placed {
                // Where it was sent is not known yet; it is placed then.
            } else if let Some(source) = self.source_for_app(&stream.app.key) {
                self.move_stream(*id, &stream.app.name, Some(source));
            }
        }
    }

    /// Take back every move the mixer made, as it closes: each application
    /// it put on a row, and each person of a call it kept on their sink.
    /// The session manager places them again as it would have, on the
    /// default output. Says whether there was any to take back.
    pub fn release_streams(&self) -> bool {
        let Some(metadata) = &self.metadata else {
            return false;
        };
        let mut any = false;
        for (id, stream) in &self.streams {
            if stream.voice.is_some() || self.source_for_app(&stream.app.key).is_some() {
                metadata.set_property(*id, "target.object", None, None);
                any = true;
            }
        }
        if any {
            log::info!("applications handed back to the session manager");
        }
        any
    }

    /// Note a link that joins one of our cells to its mix, so that its going
    /// can be noticed and the cell joined again.
    fn remember_link(&mut self, global: &GlobalObject<&DictRef>) {
        let Some(props) = global.props else {
            return;
        };
        let Some(from) = props
            .get("link.output.node")
            .and_then(|id| id.parse::<u32>().ok())
        else {
            return;
        };
        let owner = self
            .links
            .iter()
            .find(|(_, stage)| {
                stage
                    .node
                    .as_ref()
                    .is_some_and(|node| node.global_id == from)
            })
            .map(|(cell, _)| *cell);
        if let Some(cell) = owner {
            self.link_owner.insert(global.id, cell);
            return;
        }
        // A person's join to their row: from their sink into the row's. The
        // sink's monitor feeds others too — its meter, a level applet — and
        // their links coming and going is none of the join's business. One
        // announced while the person has no join is a partial one already
        // let go.
        let into = props
            .get("link.input.node")
            .and_then(|id| id.parse::<u32>().ok());
        let voice = self
            .voices
            .iter()
            .find(|((row, user), voice)| {
                !voice.links.is_empty()
                    && self.sink_ids.get(&row.voice_node_name(user)) == Some(&from)
                    && into.is_some()
                    && self.sink_ids.get(&row.sink_node_name()).copied() == into
            })
            .map(|(key, _)| key.clone());
        if let Some(key) = voice {
            self.voice_link_owner.insert(global.id, key);
        }
    }

    /// Join two nodes, channel to channel.
    ///
    /// This is what a session manager would do, and will not: it routes into
    /// sinks, and a mix is a source. The links are handed back to be held for
    /// as long as they should last.
    fn link_ports(&self, from: u32, into: u32) -> Vec<Link> {
        let (Some(outputs), Some(inputs)) = (self.ports.get(&from), self.ports.get(&into)) else {
            return Vec::new();
        };
        // Every channel or none: the ports of a new node are announced one
        // by one, and a join made with the first alone would leave the other
        // side silent for good, since a joined node is not joined again.
        let pairs: Vec<(&Port, &Port)> = outputs
            .iter()
            .filter(|port| !port.input)
            .filter_map(|output| {
                inputs
                    .iter()
                    .find(|port| port.input && port.channel == output.channel)
                    .map(|input| (output, input))
            })
            .collect();
        let outputs = outputs.iter().filter(|port| !port.input).count();
        if outputs < CHANNELS || pairs.len() < outputs {
            return Vec::new();
        }
        let mut made = Vec::new();
        for (output, input) in pairs {
            let link = self.core.create_object::<Link>(
                "link-factory",
                &properties! {
                    "link.output.node" => from.to_string(),
                    "link.output.port" => output.global_id.to_string(),
                    "link.input.node" => into.to_string(),
                    "link.input.port" => input.global_id.to_string(),
                    "object.linger" => "false",
                },
            );
            match link {
                Ok(link) => made.push(link),
                Err(e) => log::error!("cannot link {from} to {into}: {e}"),
            }
        }
        made
    }

    /// Join every cell to the mix it feeds, once both ends have their ports.
    ///
    /// A mix is a source and nothing routes into a source, so this is the
    /// only thing joining the two. Both ends are named by the ids the server
    /// gave them, so another mixer answering to the same names changes
    /// nothing here.
    fn hook_up_links(&mut self) {
        let wanted: Vec<((SourceId, MixId), u32, u32)> = self
            .links
            .iter()
            .filter(|(_, stage)| stage.links.is_empty())
            .filter_map(|(cell, stage)| {
                let from = stage.node.as_ref()?.global_id;
                let into = self.sink_ids.get(&cell.1.sink_node_name()).copied()?;
                Some((*cell, from, into))
            })
            .collect();
        for (cell, from, into) in wanted {
            let made = self.link_ports(from, into);
            if made.is_empty() {
                continue;
            }
            log::debug!("cell {}.{} linked into its mix", cell.0, cell.1);
            if let Some(stage) = self.links.get_mut(&cell) {
                stage.links = made;
            }
        }
    }

    pub fn on_global_remove(&mut self, global_id: u32) {
        self.own_clients.remove(&global_id);
        if self.metadata_id == Some(global_id) {
            // WirePlumber has gone, or is making it again. The proxy is dead
            // either way; the next one announced is bound in its place.
            log::info!("the default metadata went away; waiting for the next one");
            self.metadata_id = None;
            if let Some(dead) = self.metadata.take() {
                self.retired_metadata.hold(dead);
            }
        }
        if let Some(key) = self.voice_link_owner.remove(&global_id) {
            // A person's join went without being asked to: made again, both
            // channels, on the next tick.
            self.voice_link_owner.retain(|_, owner| *owner != key);
            if let Some(voice) = self.voices.get_mut(&key) {
                log::info!(
                    "{} came unjoined from its row; joining it again",
                    voice.label
                );
                for link in voice.links.drain(..) {
                    self.retired_links.hold(link);
                }
            }
        }
        if let Some(cell) = self.link_owner.remove(&global_id) {
            // A link of ours went without being asked to. The cell is heard
            // by nobody until it is joined again, which the next tick does.
            // Its other channel's link goes too, so the two are made afresh.
            self.link_owner.retain(|_, owner| *owner != cell);
            if let Some(stage) = self.links.get_mut(&cell) {
                log::info!("cell {}.{} came unlinked; joining it again", cell.0, cell.1);
                for link in stage.links.drain(..) {
                    self.retired_links.hold(link);
                }
            }
        }
        if let Some(node) = self.port_owner.remove(&global_id) {
            if let Some(ports) = self.ports.get_mut(&node) {
                ports.retain(|port| port.global_id != global_id);
            }
        }
        self.ports.remove(&global_id);
        if self.devices.remove(&global_id).is_some() {
            self.devices_dirty = true;
        }
        if self.streams.remove(&global_id).is_some() {
            self.streams_dirty = true;
        }
        let outputs = self
            .mixes
            .values_mut()
            .flat_map(|mix| mix.outputs.iter_mut().flatten());
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
            // Not a failure: at startup the registry announces streams before
            // the metadata, and binding the metadata sends every assigned
            // application where it belongs. See `reassign_apps`.
            log::debug!("{name} waits for the default metadata to be moved");
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
        for (id, stream) in self.streams.iter().filter(|(_, s)| !s.pinned) {
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
        self.sync_voices();
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
        self.sync_voices();
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

    /// Look for plug-ins again, after one was installed or removed.
    ///
    /// Opening a bundle runs its code, so this happens when asked rather
    /// than on a timer.
    pub fn rescan_plugins(&mut self) {
        self.plugins = crate::vst3::installed();
        log::info!("{} plug-in(s) installed", self.plugins.len());
        self.emit_plugins();
        // A copy may have just been imported, which is the other half of
        // what this button is for.
        self.refresh_stereotool();
        self.emit_stereotool();
    }

    /// What the interface can offer to put on a channel.
    pub fn emit_plugins(&self) {
        self.emit(Event::Plugins {
            available: self.plugins.clone(),
        });
    }

    /// Say where Stereo Tool stands, from what was found last time.
    pub fn emit_stereotool(&self) {
        self.emit(Event::StereoTool(self.stereotool.clone()));
    }

    /// Ask the library itself where it stands.
    ///
    /// This loads it and opens one processor, which takes a moment and walks
    /// every audio device on the machine, so it is done when the answer can
    /// have changed and not otherwise: at startup, when a licence key is
    /// given, and when a copy may have just been installed. A library
    /// already loaded cannot be swapped inside one run, so a second look at
    /// one that answered gives the same answer.
    pub fn refresh_stereotool(&mut self) {
        if matches!(self.stereotool, crate::stereotool::Status::Ready(_)) && !self.stereotool_stale
        {
            return;
        }
        self.stereotool_stale = false;
        self.stereotool = crate::stereotool::status(self.config.stereotool_license.as_deref());
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
        /// How long the graph's word on a level the mixer just set is taken
        /// for an echo of it.
        const ECHO: std::time::Duration = std::time::Duration::from_secs(1);
        let incoming: Vec<(Owner, ChainState)> = self.incoming.borrow_mut().drain(..).collect();
        for (owner, state) in incoming {
            if self
                .written
                .get(&owner)
                .is_some_and(|at| at.elapsed() < ECHO)
            {
                continue;
            }
            let known = match owner {
                Owner::Source(id) => self.config.source(id).map(|s| s.state()),
                Owner::Mix(id) => self.config.mix(id).map(|m| m.state()),
            };
            let Some(known) = known else {
                continue;
            };
            // Compared as faders: a step low on one is a tiny amplitude, and
            // compared as amplitudes it would pass for no change at all.
            if (known.gain - state.gain).abs() < 1e-3 && known.muted == state.muted {
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
        self.hook_up_links();
        self.hook_up_plugins();
        self.hook_up_meters();
        self.hook_up_voices();
        self.place_call_streams();
        self.expire_voices();
        self.absorb_levels();
        self.poll_windows();
        // What the server has had time to tell us is gone leaves without a
        // word.
        self.retired.release();
        self.retired_metadata.release();
        self.retired_links.release();
        if self.devices_dirty {
            self.emit_devices();
        }
        if self.streams_dirty {
            self.emit_apps();
            self.emit_focus();
        }
        self.flush_config();
    }
}

/// The Flatpak instance this runs in, if it runs in one, as the sandbox's
/// own description has it.
fn flatpak_instance() -> Option<&'static String> {
    static INSTANCE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            std::fs::read_to_string("/.flatpak-info")
                .ok()
                .and_then(|info| instance_in(&info))
        })
        .as_ref()
}

/// The `instance-id` of a `.flatpak-info`'s `[Instance]` group.
fn instance_in(info: &str) -> Option<String> {
    let mut in_instance = false;
    for line in info.lines().map(str::trim) {
        if line.starts_with('[') {
            in_instance = line == "[Instance]";
        } else if in_instance {
            if let Some(id) = line.strip_prefix("instance-id=") {
                return Some(id.to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flatpak_instance_is_read_from_its_group() {
        let info = "[Application]\nname=dev._2c2t.Pipedeck\n\n[Instance]\n\
                    instance-id=1832809038\nbranch=master\n";
        assert_eq!(instance_in(info).as_deref(), Some("1832809038"));
        assert_eq!(instance_in("[Application]\ninstance-id=7\n"), None);
        assert_eq!(instance_in(""), None);
    }
}
