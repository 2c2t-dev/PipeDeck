//! The keys and dials OpenDeck has put the plugin's actions on: what each
//! sends Pipedeck when used, and what it shows.
//!
//! The actions are Wave Link's, as its Stream Deck plugin has them:
//!
//! - **Channel Level**: a channel's level, its own or in one mix, or a
//!   person's of a call. A key mutes it, sets it, or moves it by a step; a
//!   dial moves it and mutes it when pressed.
//! - **Mix Level**: the same for a mix.
//! - **Monitor Mix**: the mix heard in the headphones, one, or the other of
//!   two.
//! - **Main Output Device**: the device it is heard on, one, or the other
//!   of two.
//!
//! And two of Pipedeck's own, for the people of a Discord call:
//!
//! - **Call Voice**: whoever is at a place in the call, the first, the
//!   second, and on: their level as a channel's, following the call as
//!   people come and go.
//! - **Call**: to the profile laid out for the call, saying how many are in
//!   it, and back to the mixer's; and, when asked, the decks go there by
//!   themselves as a call starts, and back as it ends.
//!
//! And two more of Wave Link's:
//!
//! - **Channel Effect**: one effect of a channel, switched off or on.
//! - **Add to Channel**: an application put on a channel, or taken off it.
//!
//! Everything is kept by id, so renaming a channel does not lose its key,
//! and shown as Pipedeck says it is, whoever changed it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::avatar::Avatars;
use crate::draw::{self, Picture, State};
use crate::mixer::{order, Mixer, Peaks, Target, View};

/// The plugin's identifier, which its actions' start with.
pub const PLUGIN: &str = "dev.2c2t.pipedeck";

/// How often a fade moves the fader.
const FADE_STEP: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    ChannelLevel,
    MixLevel,
    MonitorMix,
    MainOutput,
    CallVoice,
    CallPage,
    ChannelEffect,
    AddApp,
}

/// The profiles the Call action goes between. See `profiles.rs`.
pub const MIXER_PROFILE: &str = "Pipedeck";
pub const CALL_PROFILE: &str = "Pipedeck Call";

/// What Add to Channel is set to for the application in front, whichever
/// it is when the key is pressed.
pub const FRONT: &str = "@front";

impl Action {
    fn from_uuid(uuid: &str) -> Option<Self> {
        match uuid.strip_prefix(PLUGIN)?.strip_prefix('.')? {
            "channel" => Some(Action::ChannelLevel),
            "mix" => Some(Action::MixLevel),
            "monitor" => Some(Action::MonitorMix),
            "output" => Some(Action::MainOutput),
            "voice" => Some(Action::CallVoice),
            "call" => Some(Action::CallPage),
            "effect" => Some(Action::ChannelEffect),
            "app" => Some(Action::AddApp),
            _ => None,
        }
    }
}

/// What a key does when pressed, for the level actions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[default]
    Mute,
    Set,
    Adjust,
    /// For the monitor mix and the output: one, or the other of two.
    Select,
    Toggle,
}

/// An action's settings, as its settings page writes them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
struct Settings {
    channel: Option<u32>,
    /// A person of the call the channel carries.
    user: Option<String>,
    /// The mix a channel's level is taken in, or the mix itself.
    mix: Option<u32>,
    /// The other of two mixes, to toggle between.
    mix2: Option<u32>,
    device: Option<String>,
    device2: Option<String>,
    mode: Mode,
    /// Where a key sets the level, in percent.
    volume: Option<f32>,
    /// How long it takes to get there, in milliseconds.
    fade: Option<u64>,
    /// How far a key, or a notch of a dial, moves the level, in percent.
    step: Option<f32>,
    /// "volume" for the level alone; the meter is shown with it otherwise.
    display: Option<String>,
    /// Which person of the call, from 1.
    slot: Option<usize>,
    /// Where the Call action goes: "call", or back to the "mixer".
    page: Option<String>,
    /// An effect of the channel, by its place and its name: found by its
    /// name when the chain moved, by its place when it was renamed.
    index: Option<usize>,
    effect: Option<String>,
    /// An application, by its key, or [`FRONT`] for the one in front.
    app: Option<String>,
    /// What it was called, shown while Pipedeck is away.
    label: String,
}

impl Settings {
    /// The channel and the place of the effect an effect action works on.
    fn effect_in(&self, view: &View) -> Option<(u32, usize)> {
        let channel = view.channels.iter().find(|c| Some(c.id) == self.channel)?;
        let by_name = self
            .effect
            .as_ref()
            .and_then(|name| channel.effects.iter().position(|e| e.name == *name));
        let index = by_name.or(self.index.filter(|i| *i < channel.effects.len()))?;
        Some((channel.id, index))
    }

    /// The level a level action works on. A person of the call is found
    /// by their place in it, so the mixer is needed for that.
    fn target(&self, action: Action, view: Option<&View>) -> Option<Target> {
        match action {
            Action::CallVoice => {
                let (channel, user) = view?.in_call(self.slot.unwrap_or(1).max(1) - 1)?;
                Some(Target::Voice { channel, user })
            }
            Action::ChannelLevel => {
                let channel = self.channel?;
                Some(match (&self.user, self.mix) {
                    (Some(user), _) => Target::Voice {
                        channel,
                        user: user.clone(),
                    },
                    (None, Some(mix)) => Target::Cell { channel, mix },
                    (None, None) => Target::Channel { id: channel },
                })
            }
            Action::MixLevel => Some(Target::Mix { id: self.mix? }),
            Action::MonitorMix
            | Action::MainOutput
            | Action::CallPage
            | Action::ChannelEffect
            | Action::AddApp => None,
        }
    }

    fn step(&self) -> f32 {
        self.step.unwrap_or(5.0) / 100.0
    }
}

/// One action where OpenDeck put it.
struct Instance {
    action: Action,
    /// The deck it is on, which a profile is switched on.
    device: String,
    /// On a dial, drawn on its part of the touch strip.
    dial: bool,
    settings: Settings,
    /// What was last sent, so a change elsewhere does not redraw every key.
    shown: Option<Value>,
}

/// A fader on its way somewhere.
struct Fade {
    target: Target,
    from: f32,
    to: f32,
    start: Instant,
    length: Duration,
    last: Instant,
}

/// A use of a key or a dial.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Press {
    Key,
    Dial,
    Touch,
    Turn(i64),
}

/// Every action OpenDeck has shown, the mixer as last told, and the way to
/// tell it what to do.
pub struct Deck {
    instances: HashMap<String, Instance>,
    view: Option<View>,
    /// What the meters last read.
    levels: Peaks,
    mixer: Mixer,
    fades: Vec<Fade>,
    /// The action whose settings page is open, which is told when what
    /// there is to pick from changes.
    open_page: Option<String>,
    /// The people's pictures, made round.
    avatars: Avatars,
    /// Whether the decks go to the call's profile as a call starts, and
    /// back as it ends: the plugin's own setting, the same for every key.
    follow_call: bool,
}

impl Deck {
    pub fn new(mixer: Mixer) -> Self {
        Deck {
            instances: HashMap::new(),
            view: None,
            levels: Peaks::default(),
            mixer,
            fades: Vec::new(),
            open_page: None,
            avatars: Avatars::default(),
            follow_call: false,
        }
    }

    /// Take in a message from OpenDeck, and say what to send it back.
    pub fn hear(&mut self, message: &Value) -> Vec<Value> {
        let event = message["event"].as_str().unwrap_or_default();
        if event == "didReceiveGlobalSettings" {
            self.follow_call = message["payload"]["settings"]["follow_call"] == true;
            return Vec::new();
        }
        let Some(context) = message["context"].as_str().map(str::to_owned) else {
            return Vec::new();
        };
        let payload = &message["payload"];
        match event {
            "willAppear" => {
                let Some(action) = message["action"].as_str().and_then(Action::from_uuid) else {
                    return Vec::new();
                };
                self.instances.insert(
                    context.clone(),
                    Instance {
                        action,
                        device: message["device"].as_str().unwrap_or_default().to_owned(),
                        dial: payload["controller"] == "Encoder",
                        settings: settings_of(payload),
                        shown: None,
                    },
                );
                self.draw(&context)
            }
            "willDisappear" => {
                self.instances.remove(&context);
                Vec::new()
            }
            "didReceiveSettings" => {
                if let Some(instance) = self.instances.get_mut(&context) {
                    instance.settings = settings_of(payload);
                }
                self.draw(&context)
            }
            "keyDown" => self.press(&context, Press::Key),
            "dialDown" | "dialPress" => self.press(&context, Press::Dial),
            "touchTap" => self.press(&context, Press::Touch),
            "dialRotate" => {
                let ticks = payload["ticks"].as_i64().unwrap_or_default();
                self.press(&context, Press::Turn(ticks))
            }
            "propertyInspectorDidAppear" => {
                self.open_page = Some(context.clone());
                self.tell_settings_page(&context)
            }
            "propertyInspectorDidDisappear" => {
                if self.open_page.as_ref() == Some(&context) {
                    self.open_page = None;
                }
                Vec::new()
            }
            "sendToPlugin" if payload["request"] == "mixer" => self.tell_settings_page(&context),
            _ => Vec::new(),
        }
    }

    /// Take in the mixer's new state, and redraw what it changed.
    pub fn mixer_changed(&mut self, view: Option<View>) -> Vec<Value> {
        if self.view == view {
            return Vec::new();
        }
        let listed = catalogue(self.view.as_ref()) != catalogue(view.as_ref());
        let before = self.view.as_ref().map_or(0, View::people_in_call);
        let after = view.as_ref().map_or(0, View::people_in_call);
        self.view = view;
        let mut followed = Vec::new();
        if self.follow_call && (before == 0) != (after == 0) && self.view.is_some() {
            followed = self.follow(after > 0);
        }
        let contexts: Vec<String> = self.instances.keys().cloned().collect();
        let mut messages: Vec<Value> = contexts.iter().flat_map(|c| self.draw(c)).collect();
        messages.extend(followed);
        if let Some(open) = self.open_page.clone().filter(|_| listed) {
            messages.extend(self.tell_settings_page(&open));
        }
        messages
    }

    /// Take every deck showing one Pipedeck profile to the other: to the
    /// call's as a call starts, back to the mixer's as it ends. A deck on a
    /// profile of the user's own is left there.
    fn follow(&self, started: bool) -> Vec<Value> {
        let (from, to) = if started {
            (MIXER_PROFILE, CALL_PROFILE)
        } else {
            (CALL_PROFILE, MIXER_PROFILE)
        };
        let mut devices: Vec<&str> = self
            .instances
            .iter()
            .filter(|(context, instance)| profile_of(context, &instance.device) == Some(from))
            .map(|(_, instance)| instance.device.as_str())
            .collect();
        devices.sort_unstable();
        devices.dedup();
        devices
            .into_iter()
            .map(|device| json!({ "event": "switchProfile", "device": device, "profile": to }))
            .collect()
    }

    /// Take in what the meters read, and redraw the meters it moved.
    pub fn levels_changed(&mut self, levels: Peaks) -> Vec<Value> {
        self.levels = levels;
        let metered: Vec<String> = self
            .instances
            .iter()
            .filter(|(_, i)| {
                matches!(
                    i.action,
                    Action::ChannelLevel | Action::MixLevel | Action::CallVoice
                )
            })
            .filter(|(_, i)| i.settings.display.as_deref() != Some("volume"))
            .map(|(context, _)| context.clone())
            .collect();
        metered
            .iter()
            .flat_map(|context| self.draw(context))
            .collect()
    }

    /// Move every fade on.
    pub fn tick(&mut self) {
        let now = Instant::now();
        let mixer = self.mixer.clone();
        self.fades.retain_mut(|fade| {
            if now.duration_since(fade.last) < FADE_STEP && now < fade.start + fade.length {
                return true;
            }
            fade.last = now;
            let part = if fade.length.is_zero() {
                1.0
            } else {
                (now.duration_since(fade.start).as_secs_f32() / fade.length.as_secs_f32()).min(1.0)
            };
            let volume = fade.from + (fade.to - fade.from) * part;
            mixer.send(order(&fade.target, json!({ "volume": volume })));
            part < 1.0
        });
    }

    /// Do what a press, a touch or a turn means for an action.
    fn press(&mut self, context: &str, press: Press) -> Vec<Value> {
        let Some(instance) = self.instances.get(context) else {
            return Vec::new();
        };
        let Some(view) = &self.view else {
            return vec![alert(context)];
        };
        let settings = &instance.settings;
        let levels = matches!(
            instance.action,
            Action::ChannelLevel | Action::MixLevel | Action::CallVoice
        );
        if matches!(press, Press::Turn(_)) && !levels {
            return Vec::new();
        }
        let reply = match instance.action {
            Action::ChannelEffect => settings.effect_in(view).map(|(channel, index)| {
                Reply::Order(json!({ "what": "effect", "channel": channel, "index": index, "bypass": "toggle" }))
            }),
            Action::AddApp => app_order(settings, view).map(Reply::Order),
            Action::CallPage => Some(Reply::Tell(profile_switch(settings, &instance.device))),
            Action::ChannelLevel | Action::MixLevel | Action::CallVoice => {
                level_reply(instance.action, settings, view, press)
            }
            Action::MonitorMix => mix_order(settings, view).map(Reply::Order),
            Action::MainOutput => output_order(settings, view).map(Reply::Order),
        };
        match reply {
            None => vec![alert(context)],
            Some(Reply::Tell(message)) => vec![message],
            Some(Reply::Order(change)) => {
                if self.mixer.send(change) {
                    Vec::new()
                } else {
                    vec![alert(context)]
                }
            }
            Some(Reply::Fade(fade)) => {
                self.fades.retain(|other| other.target != fade.target);
                self.fades.push(fade);
                self.tick();
                Vec::new()
            }
        }
    }

    /// What an action shows now, as messages for OpenDeck, unless it shows
    /// it already.
    fn draw(&mut self, context: &str) -> Vec<Value> {
        let Some(instance) = self.instances.get_mut(context) else {
            return Vec::new();
        };
        let drawn = picture(
            instance.action,
            &instance.settings,
            self.view.as_ref(),
            &self.levels,
        );
        // The strip in three layers, since OpenDeck draws no picture inside
        // an SVG: the drawing, a person's picture, and what goes over it.
        let (key, strip, avatar, over) = match drawn {
            Ok(mut drawn) => {
                let avatar = drawn
                    .avatar_file
                    .take()
                    .and_then(|path| self.avatars.get(&path).map(str::to_owned));
                drawn.avatar = avatar.clone();
                let picture = drawn.as_picture();
                let over = avatar.as_ref().map(|_| draw::strip_over(&picture));
                (draw::key(&picture), draw::strip(&picture), avatar, over)
            }
            Err(why) => {
                let label = &instance.settings.label;
                (
                    draw::waiting(label, why, false),
                    draw::waiting(label, why, true),
                    None,
                    None,
                )
            }
        };
        let shown = json!([key, strip, over]);
        if instance.shown.as_ref() == Some(&shown) {
            return Vec::new();
        }
        instance.shown = Some(shown);
        // A dial's key picture is what OpenDeck's window shows for it.
        let mut messages = vec![json!({
            "event": "setImage",
            "context": context,
            "payload": { "image": draw::data_url(&key), "target": 0 },
        })];
        if instance.dial {
            messages.push(json!({
                "event": "setFeedback",
                "context": context,
                // A layer left empty is drawn as a checkerboard: one with
                // nothing to show is given a picture of nothing.
                "payload": {
                    "canvas": strip,
                    "avatar": avatar.unwrap_or_else(|| draw::NOTHING.to_owned()),
                    "over": over.unwrap_or_else(|| draw::NOTHING.to_owned()),
                },
            }));
        }
        messages
    }

    /// Tell an action's settings page what the mixer has to pick from.
    fn tell_settings_page(&self, context: &str) -> Vec<Value> {
        if !self.instances.contains_key(context) {
            return Vec::new();
        }
        vec![json!({
            "event": "sendToPropertyInspector",
            "context": context,
            "payload": { "mixer": catalogue(self.view.as_ref()) },
        })]
    }
}

/// What a press comes to, once it is known to make sense.
enum Reply {
    /// An order for the mixer.
    Order(Value),
    /// A message for OpenDeck.
    Tell(Value),
    /// A level to move there step by step.
    Fade(Fade),
}

/// Put an application on its channel, or take it off when it is on it
/// already.
fn app_order(settings: &Settings, view: &View) -> Option<Value> {
    let channel = settings.channel?;
    let mut app = settings.app.clone()?;
    if app == FRONT {
        app = view.focused.as_ref()?.key.clone();
    }
    let here = view
        .channels
        .iter()
        .any(|c| c.id == channel && c.apps.contains(&app));
    Some(json!({ "what": "app", "app": app, "channel": channel, "release": here }))
}

/// Go to the call's page or back to the mixer's. OpenDeck's own event, not
/// the Stream Deck SDK's.
fn profile_switch(settings: &Settings, device: &str) -> Value {
    let profile = if settings.page.as_deref() == Some("mixer") {
        MIXER_PROFILE
    } else {
        CALL_PROFILE
    };
    json!({
        "event": "switchProfile",
        "device": device,
        "profile": profile,
    })
}

/// Move a level, set it, or mute it. A dial moves the level and mutes it,
/// whatever a key does.
fn level_reply(action: Action, settings: &Settings, view: &View, press: Press) -> Option<Reply> {
    let target = settings.target(action, Some(view))?;
    let mode = match press {
        Press::Key => settings.mode,
        Press::Turn(_) => Mode::Adjust,
        Press::Dial | Press::Touch => Mode::Mute,
    };
    let change = match (mode, press) {
        (Mode::Adjust, Press::Turn(ticks)) => {
            json!({ "nudge": settings.step().abs() * ticks as f32 })
        }
        (Mode::Adjust, _) => json!({ "nudge": settings.step() }),
        (Mode::Set, _) => {
            let from = view.find(&target)?.volume;
            let now = Instant::now();
            return Some(Reply::Fade(Fade {
                target,
                from,
                to: settings.volume.unwrap_or(100.0).clamp(0.0, 100.0) / 100.0,
                start: now,
                length: Duration::from_millis(settings.fade.unwrap_or(0)),
                last: now - FADE_STEP,
            }));
        }
        _ => json!({ "mute": "toggle" }),
    };
    Some(Reply::Order(order(&target, change)))
}

/// Hear a mix alone, or the other of two when it is heard already.
fn mix_order(settings: &Settings, view: &View) -> Option<Value> {
    let mix = settings.mix?;
    let heard = |id: u32| view.mixes.iter().any(|m| m.id == id && m.listening);
    let mix = match (settings.mode, settings.mix2) {
        (Mode::Toggle, Some(other)) if heard(mix) => other,
        _ => mix,
    };
    Some(json!({ "what": "hear", "mix": mix, "only": true }))
}

/// Listen on a device, or on the other of two when it is listened on
/// already.
fn output_order(settings: &Settings, view: &View) -> Option<Value> {
    let device = settings.device.clone()?;
    let device = match (settings.mode, &settings.device2) {
        (Mode::Toggle, Some(other)) if view.listen.as_ref() == Some(&device) => other.clone(),
        _ => device,
    };
    Some(json!({ "what": "listen", "device": device }))
}

/// What a settings page picks from: the mixer's channels, the people of
/// its call, its mixes and cells and the devices it can be heard on, by id
/// and name. Null while Pipedeck is not running.
fn catalogue(view: Option<&View>) -> Value {
    let Some(view) = view else {
        return Value::Null;
    };
    json!({
        "channels": view.channels.iter().map(|c| json!({
            "id": c.id,
            "name": c.name,
            "voices": c.voices.iter().map(|v| json!({ "user": v.user, "name": v.name })).collect::<Vec<_>>(),
            "effects": c.effects.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
            "apps": c.apps,
        })).collect::<Vec<_>>(),
        "apps": view.apps.iter().map(|a| json!({ "key": a.key, "name": a.name })).collect::<Vec<_>>(),
        "mixes": view.mixes.iter().map(|m| json!({ "id": m.id, "name": m.name })).collect::<Vec<_>>(),
        "cells": view.cells.iter().map(|c| json!([c.channel, c.mix])).collect::<Vec<_>>(),
        "outputs": view.outputs.iter().map(|o| json!({ "name": o.name, "description": o.description })).collect::<Vec<_>>(),
    })
}

/// A picture's parts, owned.
struct Drawn {
    name: String,
    look: (&'static str, &'static str),
    corner: Option<&'static str>,
    level: Option<f32>,
    meter: Option<f32>,
    state: State,
    below: (String, &'static str),
    /// A person's picture: the file, then what it is drawn from.
    avatar_file: Option<String>,
    avatar: Option<String>,
}

impl Drawn {
    fn new(name: impl Into<String>, look: (&'static str, &'static str)) -> Self {
        Drawn {
            name: name.into(),
            look,
            corner: None,
            level: None,
            meter: None,
            state: State::default(),
            below: (String::new(), "#ffffff"),
            avatar_file: None,
            avatar: None,
        }
    }

    fn as_picture(&self) -> Picture<'_> {
        Picture {
            name: &self.name,
            look: self.look,
            corner: self.corner,
            level: self.level,
            meter: self.meter,
            state: self.state,
            below: (&self.below.0, self.below.1),
            avatar: self.avatar.as_deref(),
        }
    }
}

/// What an action shows, or why it shows nothing.
fn picture(
    action: Action,
    settings: &Settings,
    view: Option<&View>,
    levels: &Peaks,
) -> Result<Drawn, &'static str> {
    let view = view.ok_or("Offline")?;
    match action {
        Action::ChannelEffect => effect_picture(settings, view),
        Action::AddApp => app_picture(settings, view),
        Action::CallPage => page_picture(settings, view),
        // Nobody there: the place shows it is free.
        Action::CallVoice if settings.target(action, Some(view)).is_none() => {
            Ok(free_place(settings))
        }
        Action::ChannelLevel | Action::MixLevel | Action::CallVoice => {
            level_picture(action, settings, view, levels)
        }
        Action::MonitorMix => mix_picture(settings, view),
        Action::MainOutput => output_picture(settings, view),
    }
}

/// Why an action not set up yet shows nothing.
const UNSET: &str = "Pick one";

/// An effect of a channel, on or bypassed.
fn effect_picture(settings: &Settings, view: &View) -> Result<Drawn, &'static str> {
    let channel = settings.channel.ok_or(UNSET)?;
    let (_, index) = settings.effect_in(view).ok_or("Gone")?;
    let channel = view
        .channels
        .iter()
        .find(|c| c.id == channel)
        .ok_or("Gone")?;
    let effect = &channel.effects[index];
    let mut drawn = Drawn::new(
        effect.name.clone(),
        draw::look(channel.icon.as_deref(), channel.input, false),
    );
    drawn.state.dim = effect.bypassed;
    drawn.below = if effect.bypassed {
        ("Off".to_owned(), draw::FAINT)
    } else {
        ("On".to_owned(), "#ffffff")
    };
    Ok(drawn)
}

/// An application, on its channel or to be put on it.
fn app_picture(settings: &Settings, view: &View) -> Result<Drawn, &'static str> {
    let channel = settings.channel.ok_or(UNSET)?;
    let mut app = settings.app.clone().ok_or(UNSET)?;
    let channel = view
        .channels
        .iter()
        .find(|c| c.id == channel)
        .ok_or("Gone")?;
    if app == FRONT {
        let Some(front) = &view.focused else {
            let mut drawn = Drawn::new(
                "In front",
                draw::look(channel.icon.as_deref(), channel.input, false),
            );
            drawn.state.dim = true;
            drawn.below = ("Nothing playing".to_owned(), draw::FAINT);
            return Ok(drawn);
        };
        app = front.key.clone();
    }
    let app = &app;
    let name = view
        .apps
        .iter()
        .find(|a| a.key == *app)
        .map_or(settings.label.clone(), |a| a.name.clone());
    let here = channel.apps.contains(app);
    let mut drawn = Drawn::new(
        name,
        draw::look(channel.icon.as_deref(), channel.input, false),
    );
    drawn.state.dim = !here;
    drawn.below = if here {
        (format!("On {}", channel.name), "#ffffff")
    } else {
        (format!("To {}", channel.name), draw::FAINT)
    };
    Ok(drawn)
}

/// The way to the call's page, with how many are in it, or back.
fn page_picture(settings: &Settings, view: &View) -> Result<Drawn, &'static str> {
    if settings.page.as_deref() == Some("mixer") {
        return Ok(Drawn {
            name: "Mixer".to_owned(),
            look: ("pd-listen-symbolic", draw::WHITE),
            corner: None,
            level: None,
            meter: None,
            state: State::default(),
            below: ("Back".to_owned(), "#ffffff"),
            avatar_file: None,
            avatar: None,
        });
    }
    let people = view.people_in_call();
    Ok(Drawn {
        name: "Call".to_owned(),
        look: draw::look(Some("people"), false, false),
        corner: None,
        level: None,
        meter: None,
        state: State {
            muted: false,
            dim: people == 0,
        },
        below: match people {
            0 => ("No call".to_owned(), draw::FAINT),
            1 => ("1 person".to_owned(), "#ffffff"),
            n => (format!("{n} people"), "#ffffff"),
        },
        avatar_file: None,
        avatar: None,
    })
}

/// A place in the call nobody is in.
fn free_place(settings: &Settings) -> Drawn {
    Drawn {
        name: format!("Person {}", settings.slot.unwrap_or(1).max(1)),
        look: draw::look(Some("people"), false, false),
        corner: None,
        level: None,
        meter: None,
        state: State {
            muted: false,
            dim: true,
        },
        below: ("Empty".to_owned(), draw::FAINT),
        avatar_file: None,
        avatar: None,
    }
}

/// A level, with its meter unless only the level is asked for.
fn level_picture(
    action: Action,
    settings: &Settings,
    view: &View,
    levels: &Peaks,
) -> Result<Drawn, &'static str> {
    let target = settings.target(action, Some(view)).ok_or(UNSET)?;
    let found = view.find(&target).ok_or("Gone")?;
    let below = if found.muted {
        ("Muted".to_owned(), draw::RED)
    } else {
        (draw::percent(found.volume), "#ffffff")
    };
    Ok(Drawn {
        name: found.name.clone(),
        look: draw::look(found.icon.as_deref(), found.input, found.mix),
        corner: found
            .within
            .as_ref()
            .map(|mix| draw::look(mix.icon.as_deref(), false, true).0),
        level: Some(found.volume),
        // In steps a key can show, so a meter that barely moved is
        // not drawn again.
        meter: (settings.display.as_deref() != Some("volume")).then(|| {
            let at = draw::meter_position(levels.of(&target).unwrap_or(0.0));
            (at * 40.0).round() / 40.0
        }),
        state: State {
            muted: found.muted,
            dim: false,
        },
        below,
        avatar_file: found.avatar.clone(),
        avatar: None,
    })
}

/// A mix to hear, or the one of two that is heard.
fn mix_picture(settings: &Settings, view: &View) -> Result<Drawn, &'static str> {
    let first = settings.mix.ok_or(UNSET)?;
    // Of two, the one heard, or the first when neither is.
    let shown = match (settings.mode, settings.mix2) {
        (Mode::Toggle, Some(other))
            if view.mixes.iter().any(|m| m.id == other && m.listening)
                && !view.mixes.iter().any(|m| m.id == first && m.listening) =>
        {
            other
        }
        _ => first,
    };
    let mix = view.mixes.iter().find(|m| m.id == shown).ok_or("Gone")?;
    Ok(Drawn {
        name: mix.name.clone(),
        look: draw::look(mix.icon.as_deref(), false, true),
        corner: None,
        level: None,
        meter: None,
        state: State {
            muted: false,
            dim: !mix.listening,
        },
        below: if mix.listening {
            ("Heard".to_owned(), "#ffffff")
        } else {
            ("Hear".to_owned(), draw::FAINT)
        },
        avatar_file: None,
        avatar: None,
    })
}

/// A device to listen on, or the one of two that is listened on.
fn output_picture(settings: &Settings, view: &View) -> Result<Drawn, &'static str> {
    let first = settings.device.as_ref().ok_or(UNSET)?;
    let shown = match (settings.mode, &settings.device2) {
        (Mode::Toggle, Some(other)) if view.listen.as_ref() == Some(other) => other,
        _ => first,
    };
    let found = view
        .find(&Target::Output {
            device: shown.clone(),
        })
        .ok_or("Gone")?;
    Ok(Drawn {
        name: found.name,
        look: draw::look(Some("headset"), false, false),
        corner: None,
        level: None,
        meter: None,
        state: State {
            muted: false,
            dim: !found.listening,
        },
        below: if found.listening {
            ("Listening".to_owned(), "#ffffff")
        } else {
            ("Listen".to_owned(), draw::FAINT)
        },
        avatar_file: None,
        avatar: None,
    })
}

/// The profile an action is on, from its context, which OpenDeck makes of
/// the deck, the profile, the controller and the place.
fn profile_of<'a>(context: &'a str, device: &str) -> Option<&'a str> {
    let rest = context.strip_prefix(device)?.strip_prefix('.')?;
    let mut parts = rest.rsplitn(4, '.');
    let (_index, _position, _controller) = (parts.next()?, parts.next()?, parts.next()?);
    parts.next()
}

fn settings_of(payload: &Value) -> Settings {
    serde_json::from_value(payload["settings"].clone()).unwrap_or_default()
}

fn alert(context: &str) -> Value {
    json!({ "event": "showAlert", "context": context })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        serde_json::from_value(json!({
            "channels": [{"id": 2, "name": "Music", "icon": "music", "volume": 0.5,
                          "muted": false, "input": false, "voices": []}],
            "mixes": [{"id": 1, "name": "Personal", "icon": null, "volume": 1.0,
                       "muted": false, "listening": true},
                      {"id": 3, "name": "Stream", "icon": "stream", "volume": 1.0,
                       "muted": false, "listening": false}],
            "cells": [{"channel": 2, "mix": 3, "volume": 0.25, "muted": true}],
            "listen": "phones",
            "outputs": [{"name": "phones", "description": "Headphones"},
                        {"name": "speakers", "description": "Speakers"}],
        }))
        .unwrap()
    }

    fn appear(deck: &mut Deck, action: &str, controller: &str, settings: Value) -> Vec<Value> {
        deck.hear(&json!({
            "event": "willAppear",
            "action": format!("{PLUGIN}.{action}"),
            "context": "here",
            "payload": {"controller": controller, "settings": settings},
        }))
    }

    fn canvas(messages: &[Value]) -> String {
        messages
            .iter()
            .find(|m| m["event"] == "setFeedback")
            .and_then(|m| m["payload"]["canvas"].as_str())
            .unwrap_or_default()
            .to_owned()
    }

    #[test]
    fn a_dial_shows_its_level_and_draws_again_only_on_change() {
        let mut deck = Deck::new(Mixer::default());
        let shown = appear(&mut deck, "channel", "Encoder", json!({"channel": 2}));
        assert!(canvas(&shown).contains("Offline"));

        let shown = deck.mixer_changed(Some(view()));
        assert!(canvas(&shown).contains("50%"));
        assert!(deck.mixer_changed(Some(view())).is_empty());

        let mut other = view();
        other.mixes[0].volume = 0.2;
        assert!(
            deck.mixer_changed(Some(other)).is_empty(),
            "the mix is not on this dial"
        );
    }

    #[test]
    fn a_meter_is_drawn_again_only_when_it_moves_enough_to_show() {
        let mut deck = Deck::new(Mixer::default());
        deck.mixer_changed(Some(view()));
        appear(&mut deck, "channel", "Encoder", json!({"channel": 2}));
        let peaks = |p: f32| Peaks {
            channels: vec![(2, p)],
            ..Peaks::default()
        };
        assert!(!deck.levels_changed(peaks(0.5)).is_empty());
        assert!(deck.levels_changed(peaks(0.5001)).is_empty());
        assert!(!deck.levels_changed(peaks(0.0)).is_empty());

        let mut plain = Deck::new(Mixer::default());
        plain.mixer_changed(Some(view()));
        appear(
            &mut plain,
            "channel",
            "Encoder",
            json!({"channel": 2, "display": "volume"}),
        );
        assert!(plain.levels_changed(peaks(0.5)).is_empty());
    }

    fn in_call(names: &[&str]) -> View {
        let mut view = view();
        view.channels[0].voices = names
            .iter()
            .map(|name| crate::mixer::Voice {
                user: format!("id-{name}"),
                name: (*name).to_owned(),
                volume: 1.0,
                muted: false,
                avatar: None,
            })
            .collect();
        view
    }

    #[test]
    fn a_place_in_the_call_follows_who_is_there() {
        let place = |slot| Settings {
            slot: Some(slot),
            ..Settings::default()
        };
        let call = in_call(&["Alice", "Bob"]);
        let drawn = picture(Action::CallVoice, &place(2), Some(&call), &Peaks::default()).unwrap();
        assert_eq!((drawn.name.as_str(), drawn.state.dim), ("Bob", false));
        // Alice leaves: Bob moves up, and the second place is free.
        let call = in_call(&["Bob"]);
        let drawn = picture(Action::CallVoice, &place(1), Some(&call), &Peaks::default()).unwrap();
        assert_eq!(drawn.name, "Bob");
        let drawn = picture(Action::CallVoice, &place(2), Some(&call), &Peaks::default()).unwrap();
        assert_eq!(
            (drawn.name.as_str(), drawn.below.0.as_str()),
            ("Person 2", "Empty")
        );
    }

    #[test]
    fn the_call_key_counts_the_people_and_switches_the_profile() {
        let mut deck = Deck::new(Mixer::default());
        deck.mixer_changed(Some(in_call(&["Alice", "Bob", "Carol"])));
        let shown = deck.hear(&json!({
            "event": "willAppear",
            "action": format!("{PLUGIN}.call"),
            "context": "here",
            "device": "sd-1",
            "payload": {"controller": "Keypad", "settings": {}},
        }));
        let image = shown[0]["payload"]["image"].as_str().unwrap().to_owned();
        assert!(image.starts_with("data:image/svg+xml;base64,"));
        let told = deck.hear(&json!({"event": "keyDown", "context": "here", "payload": {}}));
        assert_eq!(
            told,
            vec![json!({"event": "switchProfile", "device": "sd-1", "profile": CALL_PROFILE})]
        );
    }

    #[test]
    fn a_profile_is_read_from_where_opendeck_says_an_action_is() {
        assert_eq!(
            profile_of("sd-1.Pipedeck Call.Keypad.3.0", "sd-1"),
            Some(CALL_PROFILE)
        );
        assert_eq!(
            profile_of("sd-1.Pipedeck.Encoder.0.0", "sd-1"),
            Some(MIXER_PROFILE)
        );
        assert_eq!(profile_of("sd-2.Pipedeck.Keypad.0.0", "sd-1"), None);
    }

    #[test]
    fn the_decks_follow_a_call_when_asked() {
        let mut deck = Deck::new(Mixer::default());
        deck.mixer_changed(Some(view()));
        deck.hear(&json!({
            "event": "willAppear",
            "action": format!("{PLUGIN}.call"),
            "context": "sd-1.Pipedeck.Keypad.0.0",
            "device": "sd-1",
            "payload": {"controller": "Keypad", "settings": {}},
        }));
        let switches = |told: &[Value]| -> Vec<Value> {
            told.iter()
                .filter(|m| m["event"] == "switchProfile")
                .cloned()
                .collect()
        };
        // Not asked: nothing moves.
        assert!(switches(&deck.mixer_changed(Some(in_call(&["Alice"])))).is_empty());
        deck.mixer_changed(Some(view()));
        deck.hear(&json!({"event": "didReceiveGlobalSettings", "payload": {"settings": {"follow_call": true}}}));
        assert_eq!(
            switches(&deck.mixer_changed(Some(in_call(&["Alice"])))),
            vec![json!({"event": "switchProfile", "device": "sd-1", "profile": CALL_PROFILE})]
        );
        // Someone else joining is not a call starting.
        assert!(switches(&deck.mixer_changed(Some(in_call(&["Alice", "Bob"])))).is_empty());
    }

    #[test]
    fn an_effect_and_an_app_are_found_and_shown() {
        let mut view = view();
        view.channels[0].effects = vec![
            crate::mixer::EffectState {
                name: "EQ".into(),
                bypassed: false,
            },
            crate::mixer::EffectState {
                name: "Noise suppression".into(),
                bypassed: true,
            },
        ];
        view.channels[0].apps = vec!["spotify".into()];
        view.apps = vec![crate::mixer::App {
            key: "spotify".into(),
            name: "Spotify".into(),
        }];
        // Found by its name, though it moved.
        let effect = Settings {
            channel: Some(2),
            index: Some(0),
            effect: Some("Noise suppression".into()),
            ..Settings::default()
        };
        assert_eq!(effect.effect_in(&view), Some((2, 1)));
        let drawn = picture(
            Action::ChannelEffect,
            &effect,
            Some(&view),
            &Peaks::default(),
        )
        .unwrap();
        assert_eq!(
            (drawn.name.as_str(), drawn.below.0.as_str()),
            ("Noise suppression", "Off")
        );
        let app = Settings {
            channel: Some(2),
            app: Some("spotify".into()),
            ..Settings::default()
        };
        let drawn = picture(Action::AddApp, &app, Some(&view), &Peaks::default()).unwrap();
        assert_eq!(
            (drawn.name.as_str(), drawn.below.0.as_str()),
            ("Spotify", "On Music")
        );
    }

    #[test]
    fn the_application_in_front_is_the_one_moved() {
        let mut view = view();
        let front = |view: &View| {
            picture(
                Action::AddApp,
                &Settings {
                    channel: Some(2),
                    app: Some(FRONT.into()),
                    ..Settings::default()
                },
                Some(view),
                &Peaks::default(),
            )
            .unwrap()
        };
        assert_eq!(front(&view).below.0, "Nothing playing");
        view.focused = Some(crate::mixer::App {
            key: "spotify".into(),
            name: "Spotify".into(),
        });
        view.apps = vec![view.focused.clone().unwrap()];
        let drawn = front(&view);
        assert_eq!(
            (drawn.name.as_str(), drawn.below.0.as_str()),
            ("Spotify", "To Music")
        );
    }

    /// The touch strip as OpenDeck draws it, from what a dial was sent:
    /// its layout, with OpenDeck's own renderer, as RGBA rows.
    fn on_the_strip(feedback: &Value, layout: &str) -> Vec<u8> {
        let mut renderer =
            streamdeck_strip_render::get_incremental_renderer(layout.to_owned(), None).unwrap();
        renderer.set_feedback(feedback.clone()).unwrap();
        renderer.get_image().into_raw()
    }

    const LAYOUT: &str = include_str!("../plugin/layouts/strip.json");
    /// The strip with nothing but the drawing, to compare with.
    const CANVAS_ONLY: &str = r#"{"id": "canvas", "items": [{"key": "canvas", "type": "pixmap", "rect": [0, 0, 200, 100]}]}"#;

    fn pixel(image: &[u8], x: usize, y: usize) -> [u8; 4] {
        let at = (y * 200 + x) * 4;
        [image[at], image[at + 1], image[at + 2], image[at + 3]]
    }

    fn feedback_of(deck: &mut Deck, action: &str, settings: Value) -> Value {
        let told = deck.hear(&json!({
            "event": "willAppear",
            "action": format!("{PLUGIN}.{action}"),
            "context": format!("{action}-dial"),
            "device": "d",
            "payload": {"controller": "Encoder", "settings": settings},
        }));
        told.into_iter()
            .find(|m| m["event"] == "setFeedback")
            .expect("a dial is sent its strip")["payload"]
            .clone()
    }

    #[test]
    fn a_dial_without_a_picture_is_its_drawing_and_nothing_over_it() {
        let mut deck = Deck::new(Mixer::default());
        deck.mixer_changed(Some(view()));
        let feedback = feedback_of(&mut deck, "channel", json!({"channel": 2}));
        let drawn = on_the_strip(&feedback, LAYOUT);
        let alone = on_the_strip(&json!({"canvas": feedback["canvas"]}), CANVAS_ONLY);
        assert!(drawn == alone, "the layers over the badge are not empty");
    }

    #[test]
    fn a_person_wears_their_picture_round_on_the_strip() {
        let path = std::env::temp_dir().join(format!("pipedeck-strip-{}.png", std::process::id()));
        {
            let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), 64, 64);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let red: Vec<u8> = [230u8, 20, 20].repeat(64 * 64);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&red)
                .unwrap();
        }
        let mut view = in_call(&["Alice"]);
        view.channels[0].voices[0].avatar = Some(path.to_string_lossy().into_owned());
        let mut deck = Deck::new(Mixer::default());
        deck.mixer_changed(Some(view));
        let drawn = on_the_strip(&feedback_of(&mut deck, "voice", json!({"slot": 1})), LAYOUT);
        let _ = std::fs::remove_file(&path);
        // The picture's middle, at 12 + 23 by 27 + 23: red.
        let [r, g, b, _] = pixel(&drawn, 35, 50);
        assert!(r > 200 && g < 60 && b < 60, "the middle is {r} {g} {b}");
        // Its corner, outside the circle: the strip's black, no checkerboard.
        assert_eq!(pixel(&drawn, 13, 28)[..3], [0, 0, 0]);
    }

    #[test]
    fn a_channel_in_a_mix_is_its_cell() {
        let settings = Settings {
            channel: Some(2),
            mix: Some(3),
            ..Settings::default()
        };
        let drawn = picture(
            Action::ChannelLevel,
            &settings,
            Some(&view()),
            &Peaks::default(),
        )
        .unwrap();
        assert_eq!(drawn.name, "Music");
        assert_eq!(drawn.corner, Some("pd-stream-symbolic"));
        assert_eq!(drawn.below.0, "Muted");
    }

    #[test]
    fn two_mixes_take_turns() {
        let settings = Settings {
            mix: Some(1),
            mix2: Some(3),
            mode: Mode::Toggle,
            ..Settings::default()
        };
        let drawn = picture(
            Action::MonitorMix,
            &settings,
            Some(&view()),
            &Peaks::default(),
        )
        .unwrap();
        assert_eq!((drawn.name.as_str(), drawn.state.dim), ("Personal", false));

        let mut stream = view();
        stream.mixes[0].listening = false;
        stream.mixes[1].listening = true;
        let drawn = picture(
            Action::MonitorMix,
            &settings,
            Some(&stream),
            &Peaks::default(),
        )
        .unwrap();
        assert_eq!(drawn.name, "Stream");
    }

    #[test]
    fn each_press_comes_to_its_order() {
        let mut view = view();
        let channel = Target::Channel { id: 2 };
        let level = |settings: &Settings, view: &View, press| match level_reply(
            Action::ChannelLevel,
            settings,
            view,
            press,
        ) {
            Some(Reply::Order(change)) => change,
            _ => panic!("a level is moved by an order"),
        };
        let on_music = Settings {
            channel: Some(2),
            ..Settings::default()
        };
        assert_eq!(
            level(&on_music, &view, Press::Key),
            order(&channel, json!({"mute": "toggle"}))
        );
        let adjust = Settings {
            mode: Mode::Adjust,
            step: Some(10.0),
            ..on_music.clone()
        };
        assert_eq!(
            level(&adjust, &view, Press::Key),
            order(&channel, json!({"nudge": 0.1_f32}))
        );
        assert_eq!(
            level(&adjust, &view, Press::Turn(-2)),
            order(&channel, json!({"nudge": -0.2_f32}))
        );
        assert_eq!(
            level(&adjust, &view, Press::Dial),
            order(&channel, json!({"mute": "toggle"}))
        );

        let set = Settings {
            mode: Mode::Set,
            volume: Some(150.0),
            fade: Some(300),
            ..on_music.clone()
        };
        let Some(Reply::Fade(fade)) = level_reply(Action::ChannelLevel, &set, &view, Press::Key)
        else {
            panic!("a level set is faded to");
        };
        assert_eq!((fade.from, fade.to), (0.5, 1.0));
        assert_eq!(fade.length, Duration::from_millis(300));
        let gone = Settings {
            channel: Some(9),
            ..set
        };
        assert!(level_reply(Action::ChannelLevel, &gone, &view, Press::Key).is_none());

        let toggle = Settings {
            mix: Some(1),
            mix2: Some(3),
            mode: Mode::Toggle,
            ..Settings::default()
        };
        assert_eq!(mix_order(&toggle, &view).unwrap()["mix"], 3);
        view.mixes[0].listening = false;
        assert_eq!(mix_order(&toggle, &view).unwrap()["mix"], 1);

        let toggle = Settings {
            device: Some("phones".into()),
            device2: Some("speakers".into()),
            mode: Mode::Toggle,
            ..Settings::default()
        };
        assert_eq!(output_order(&toggle, &view).unwrap()["device"], "speakers");
        view.listen = Some("speakers".into());
        assert_eq!(output_order(&toggle, &view).unwrap()["device"], "phones");
        assert!(output_order(&Settings::default(), &view).is_none());

        let front = Settings {
            channel: Some(2),
            app: Some(FRONT.into()),
            ..Settings::default()
        };
        assert!(app_order(&front, &view).is_none(), "nothing in front");
        view.focused = Some(crate::mixer::App {
            key: "spotify".into(),
            name: "Spotify".into(),
        });
        assert_eq!(
            app_order(&front, &view).unwrap(),
            json!({"what": "app", "app": "spotify", "channel": 2, "release": false})
        );
        view.channels[0].apps = vec!["spotify".into()];
        assert_eq!(app_order(&front, &view).unwrap()["release"], true);
    }

    #[test]
    fn a_key_without_pipedeck_says_so() {
        let mut deck = Deck::new(Mixer::default());
        appear(&mut deck, "mix", "Keypad", json!({"mix": 1}));
        let told = deck.hear(&json!({"event": "keyDown", "context": "here", "payload": {}}));
        assert_eq!(told, vec![alert("here")]);
    }

    #[test]
    fn the_settings_page_is_told_what_the_mixer_has() {
        let mut deck = Deck::new(Mixer::default());
        deck.mixer_changed(Some(view()));
        appear(&mut deck, "monitor", "Keypad", json!({}));
        let told = deck.hear(&json!({"event": "propertyInspectorDidAppear", "context": "here"}));
        assert_eq!(told[0]["payload"]["mixer"]["mixes"][1]["name"], "Stream");
    }
}
