//! The people of a call, each on a sub-track of their own.
//!
//! Vesktop's plugin says who is in the call (see [`crate::control`]); the
//! row Vesktop is assigned to gets a sink for each of them, joined by hand
//! to the row's own sink, so a person is heard through the row's effects
//! and cells with a level, a mute and a meter of their own. Vesktop plays
//! each person into their sink, and the streams it sends there are kept
//! there rather than moved to the row with the rest of Vesktop.

use std::cell::RefCell;
use std::rc::Rc;

use pipewire::link::Link;
use pipewire::node::{Node, NodeChangeMask, NodeListener};
use pipewire::proxy::ProxyListener;
use pipewire::registry::GlobalObject;
use pipewire::spa::utils::dict::DictRef;

use super::meter::Meter;
use super::{apply_props, Graph};
use crate::error::EngineError;
use crate::types::{
    node_prefix, today, voice_app, voice_labels, voice_node_prefix, CallMember, ChainState,
    SourceId, VoiceConfig, FORGET_AFTER_DAYS,
};

/// Streams whose target was just read, and that target. See
/// `Graph::stream_targets`.
pub(super) type StreamTargets = Rc<RefCell<Vec<(u32, Option<String>)>>>;

/// One person of a call, on a sink of their own that plays into the sink
/// of the row carrying the call.
pub(super) struct Voice {
    /// Their level, measured on the sink. It goes before the sink.
    pub(super) meter: Option<Meter>,
    /// The links joining this sink to the row's. They go before the sink.
    pub(super) links: Vec<Link>,
    pub(super) sink: Node,
    pub(super) _bound: ProxyListener,
    /// What the sink is called, which is how the call's client finds it.
    pub(super) label: String,
    /// When the person was last said to have left. The sink is kept a
    /// while: a call that drops and comes back says everyone left and came
    /// back within a second, and a sink made again under a stream leaves
    /// that stream playing nowhere.
    pub(super) gone_since: Option<std::time::Instant>,
}

/// Forget the people not seen in a call for [`FORGET_AFTER_DAYS`], and
/// count the days of anyone known from before they were counted from
/// today; everyone is taken as absent until the call says otherwise. Says
/// whether anything is to be saved.
fn remember(voices: &mut Vec<VoiceConfig>, today: u64) -> bool {
    let before = voices.len();
    voices.retain(|voice| {
        voice
            .seen
            .is_none_or(|seen| today.saturating_sub(seen) <= FORGET_AFTER_DAYS)
    });
    let mut changed = voices.len() != before;
    for voice in voices.iter_mut() {
        voice.present = false;
        if voice.seen.is_none() {
            voice.seen = Some(today);
            changed = true;
        }
    }
    changed
}

/// How long a person's sink outlives their leaving the call.
const VOICE_GRACE: std::time::Duration = std::time::Duration::from_secs(15);

impl Graph {
    /// Bind a stream to read where it was sent, and queue that for the tick.
    pub(super) fn watch_stream_target(
        &self,
        global: &GlobalObject<&DictRef>,
    ) -> Option<(Node, NodeListener)> {
        let node = match self.registry.bind::<Node, _>(global) {
            Ok(node) => node,
            Err(e) => {
                log::warn!("cannot read where stream {} goes: {e}", global.id);
                return None;
            }
        };
        let queue = self.stream_targets.clone();
        let id = global.id;
        let listener = node
            .add_listener_local()
            .info(move |info| {
                // A node says what it is again on every change of state,
                // with its properties only when they changed: an empty set
                // otherwise, which says nothing about where it goes.
                if !info.change_mask().contains(NodeChangeMask::PROPS) {
                    return;
                }
                let Some(props) = info.props() else {
                    return;
                };
                let target = props.get("target.object").map(str::to_owned);
                queue.borrow_mut().push((id, target));
            })
            .register();
        Some((node, listener))
    }

    /// Move the call's application's streams once where they were sent is
    /// known: one sent to a voice sink stays there, the rest go to its row.
    pub(super) fn place_call_streams(&mut self) {
        let read: Vec<(u32, Option<String>)> = self.stream_targets.borrow_mut().drain(..).collect();
        for (id, target) in read {
            let pinned = target
                .as_deref()
                .is_some_and(|target| target.starts_with(&voice_node_prefix()));
            let Some(stream) = self.streams.get_mut(&id) else {
                continue;
            };
            // The node says what it is again on every change of state; only
            // a change of target is news, including from one voice sink to
            // another.
            let voice = target.clone().filter(|_| pinned);
            if stream.placed && stream.pinned == pinned && stream.voice == voice {
                continue;
            }
            stream.placed = true;
            stream.pinned = pinned;
            stream.voice = voice;
            let name = stream.app.name.clone();
            if let Some(voice) = target.filter(|_| pinned) {
                // Said again where it goes, over whatever moved it before
                // this was known: a move outlives the mixer that made it.
                self.pin_stream(id, &name, &voice);
            } else if let Some(source) = self.source_for_app(voice_app()) {
                self.move_stream(id, &name, Some(source));
            }
        }
    }

    /// Keep a stream on the voice sink it was sent to, by saying so where
    /// a move would be said.
    pub(super) fn pin_stream(&self, stream: u32, name: &str, voice: &str) {
        let Some(metadata) = &self.metadata else {
            // Said again when the metadata is bound. See `reassign_apps`.
            return;
        };
        metadata.set_property(stream, "target.object", Some("Spa:String"), Some(voice));
        log::info!("{name} plays a person of the call on {voice}");
    }

    /// Take who is in the call now, and give each a sub-track on the row
    /// carrying Vesktop.
    ///
    /// Says whether anything changed: the plugin says the call again
    /// whenever it reconnects, and the same call again is no news.
    pub fn set_call(&mut self, members: Vec<CallMember>) -> bool {
        let changed = members != self.call;
        if changed {
            log::info!("{} in the call", members.len());
        }
        self.call = members;
        // Even the same call again: a sink that could not be made last time
        // is tried again.
        self.sync_voices();
        changed
    }

    /// The row the call is heard through: the one Vesktop is assigned to,
    /// when it has a sink for the voices to play into.
    fn call_row(&self) -> Option<SourceId> {
        let id = self.source_for_app(voice_app())?;
        self.config
            .source(id)
            .is_some_and(|source| !source.is_input())
            .then_some(id)
    }

    /// Make the graph and the config agree with the call: a sink for each
    /// person in it on the call's row, none for anyone else.
    pub(super) fn sync_voices(&mut self) {
        let row = self.call_row();
        let labels = voice_labels(&self.call);

        // Who is present, in the config: the level a person had last time
        // is theirs again. Someone not seen for long is forgotten, and
        // someone known from before days were counted is counted from now.
        let today = today();
        for source in &mut self.config.sources {
            if remember(&mut source.voices, today) {
                self.dirty = true;
            }
        }
        if let Some(cfg) = row.and_then(|id| self.config.source_mut(id)) {
            for member in &self.call {
                match cfg.voices.iter_mut().find(|voice| voice.id == member.id) {
                    Some(voice) => {
                        voice.present = true;
                        if voice.seen != Some(today) {
                            voice.seen = Some(today);
                            self.dirty = true;
                        }
                        if voice.name != member.name {
                            voice.name = member.name.clone();
                            self.dirty = true;
                        }
                        if member.avatar.is_some() && voice.avatar != member.avatar {
                            voice.avatar = member.avatar.clone();
                            self.dirty = true;
                        }
                    }
                    None => {
                        cfg.voices.push(VoiceConfig {
                            id: member.id.clone(),
                            name: member.name.clone(),
                            avatar: member.avatar.clone(),
                            gain: 1.0,
                            muted: false,
                            seen: Some(today),
                            present: true,
                        });
                        self.dirty = true;
                    }
                }
            }
        }

        // On the graph: what is wanted, labelled as the client was told.
        let wanted: Vec<((SourceId, String), String)> = match row {
            Some(row) => self
                .call
                .iter()
                .zip(labels)
                .map(|(member, label)| ((row, member.id.clone()), label))
                .collect(),
            None => Vec::new(),
        };
        // Someone gone from the call keeps their sink a while; one whose
        // name changed gets a new one at once, since the client looks
        // their output up by it.
        // A sink kept for someone who left goes at once when someone in the
        // call needs its name: two outputs called the same, and the client
        // could pick the one nobody listens to.
        let now = std::time::Instant::now();
        let mut gone: Vec<(SourceId, String)> = Vec::new();
        for (key, voice) in &mut self.voices {
            match wanted.iter().find(|(wanted, _)| wanted == key) {
                Some((_, label)) if *label == voice.label => voice.gone_since = None,
                Some(_) => gone.push(key.clone()),
                None => {
                    let since = *voice.gone_since.get_or_insert(now);
                    let needed = wanted.iter().any(|(_, label)| *label == voice.label);
                    if needed || now.duration_since(since) >= VOICE_GRACE {
                        gone.push(key.clone());
                    }
                }
            }
        }
        for key in gone {
            if let Some(label) = self.drop_voice(&key) {
                log::info!("{label} left the call");
            }
        }
        for ((row, user), label) in wanted {
            if self.voices.contains_key(&(row, user.clone())) {
                continue;
            }
            let name = row.voice_node_name(&user);
            let sink = match self.create_sink(name.clone(), label.clone()) {
                Ok(sink) => sink,
                Err(e) => {
                    log::error!("cannot make a sink for {label}: {e}");
                    continue;
                }
            };
            let state = self
                .config
                .source(row)
                .and_then(|cfg| cfg.voices.iter().find(|voice| voice.id == user))
                .map(VoiceConfig::state)
                .unwrap_or_default();
            apply_props(&sink, &name, &state);
            let bound = self.watch_sink_id(&sink, name);
            log::info!("{label} joined the call");
            self.voices.insert(
                (row, user),
                Voice {
                    meter: None,
                    links: Vec::new(),
                    sink,
                    _bound: bound,
                    label,
                    gone_since: None,
                },
            );
        }
    }

    /// Take one person's sink off the graph, and what joins and measures
    /// it. Says what it was called, if there was one.
    pub(super) fn drop_voice(&mut self, key: &(SourceId, String)) -> Option<String> {
        let voice = self.voices.remove(key)?;
        self.sink_ids.remove(&key.0.voice_node_name(&key.1));
        self.voice_link_owner.retain(|_, owner| owner != key);
        for link in voice.links {
            self.retired_links.hold(link);
        }
        self.retired.hold(voice.sink);
        Some(voice.label)
    }

    /// Join every voice's sink to its row's, once both have their ports,
    /// and measure it once the server has named it.
    pub(super) fn hook_up_voices(&mut self) {
        let unmeasured: Vec<((SourceId, String), u32)> = self
            .voices
            .iter()
            .filter(|(_, voice)| voice.meter.is_none())
            .filter_map(|((row, user), _)| {
                let id = self.sink_ids.get(&row.voice_node_name(user)).copied()?;
                Some(((*row, user.clone()), id))
            })
            .collect();
        for ((row, user), id) in unmeasured {
            let name = row.voice_node_name(&user);
            let meter = self.watch_level(
                &format!("{}.meter.voice.{}.{user}", node_prefix(), row.0),
                &name,
                Some(id),
                true,
            );
            if let Some(voice) = self.voices.get_mut(&(row, user)) {
                voice.meter = meter;
            }
        }

        let wanted: Vec<((SourceId, String), u32, u32)> = self
            .voices
            .iter()
            .filter(|(_, voice)| voice.links.is_empty())
            .filter_map(|((row, user), _)| {
                let from = self.sink_ids.get(&row.voice_node_name(user)).copied()?;
                let into = self.sink_ids.get(&row.sink_node_name()).copied()?;
                Some(((*row, user.clone()), from, into))
            })
            .collect();
        for (key, from, into) in wanted {
            // Joined with every channel or not yet: see `link_ports`.
            let made = self.link_ports(from, into);
            if made.is_empty() {
                continue;
            }
            if let Some(voice) = self.voices.get_mut(&key) {
                log::debug!("{} joined to its row", voice.label);
                voice.links = made;
            }
        }
    }

    /// Set one person's level on a row.
    pub fn update_voice(
        &mut self,
        id: SourceId,
        user: &str,
        f: impl FnOnce(&mut ChainState),
    ) -> Result<(), EngineError> {
        let cfg = self
            .config
            .source_mut(id)
            .ok_or(EngineError::UnknownSource(id))?;
        let Some(voice) = cfg.voices.iter_mut().find(|voice| voice.id == user) else {
            return Ok(());
        };
        let mut state = voice.state();
        f(&mut state);
        voice.gain = state.gain;
        voice.muted = state.muted;
        self.dirty = true;
        if let Some(live) = self.voices.get(&(id, user.to_owned())) {
            apply_props(&live.sink, &id.voice_node_name(user), &state);
        }
        Ok(())
    }

    /// Forget someone of the calls a row carried: their level and their
    /// picture. Not while they are in the call, where they would be met
    /// again at once.
    pub fn forget_voice(&mut self, id: SourceId, user: &str) -> Result<(), EngineError> {
        let cfg = self
            .config
            .source_mut(id)
            .ok_or(EngineError::UnknownSource(id))?;
        let before = cfg.voices.len();
        cfg.voices.retain(|voice| voice.present || voice.id != user);
        if cfg.voices.len() != before {
            self.dirty = true;
        }
        Ok(())
    }

    /// Take the sinks of the people who left a while ago off the graph.
    pub(super) fn expire_voices(&mut self) {
        if self.voices.values().any(|voice| {
            voice
                .gone_since
                .is_some_and(|since| since.elapsed() >= VOICE_GRACE)
        }) {
            self.sync_voices();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(id: &str, seen: Option<u64>) -> VoiceConfig {
        VoiceConfig {
            id: id.into(),
            name: id.into(),
            avatar: None,
            gain: 0.5,
            muted: false,
            seen,
            present: true,
        }
    }

    #[test]
    fn someone_not_seen_for_months_is_forgotten() {
        let today = 20_000;
        let mut voices = vec![
            voice("recent", Some(today - 10)),
            voice("gone", Some(today - FORGET_AFTER_DAYS - 1)),
            voice("before", None),
        ];
        assert!(remember(&mut voices, today));
        let kept: Vec<_> = voices
            .iter()
            .map(|v| (v.id.as_str(), v.seen, v.present))
            .collect();
        assert_eq!(
            kept,
            [
                ("recent", Some(today - 10), false),
                ("before", Some(today), false)
            ]
        );
        // Nothing more to save the second time.
        assert!(!remember(&mut voices, today));
    }
}
