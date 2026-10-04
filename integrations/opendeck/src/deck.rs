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
//! Everything is kept by id, so renaming a channel does not lose its key,
//! and shown as Pipedeck says it is, whoever changed it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::draw::{self, Picture, State};
use crate::mixer::{order, Mixer, Peaks, Target, View};

/// The plugin's identifier, which its actions' start with.
pub const PLUGIN: &str = "com.fabienmillet.pipedeck";

/// How often a fade moves the fader.
const FADE_STEP: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    ChannelLevel,
    MixLevel,
    MonitorMix,
    MainOutput,
}

impl Action {
    fn from_uuid(uuid: &str) -> Option<Self> {
        match uuid.strip_prefix(PLUGIN)?.strip_prefix('.')? {
            "channel" => Some(Action::ChannelLevel),
            "mix" => Some(Action::MixLevel),
            "monitor" => Some(Action::MonitorMix),
            "output" => Some(Action::MainOutput),
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
    /// What it was called, shown while Pipedeck is away.
    label: String,
}

impl Settings {
    /// The level a level action works on.
    fn target(&self, action: Action) -> Option<Target> {
        match action {
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
            Action::MonitorMix | Action::MainOutput => None,
        }
    }

    fn step(&self) -> f32 {
        self.step.unwrap_or(5.0) / 100.0
    }
}

/// One action where OpenDeck put it.
struct Instance {
    action: Action,
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
        }
    }

    /// Take in a message from OpenDeck, and say what to send it back.
    pub fn hear(&mut self, message: &Value) -> Vec<Value> {
        let event = message["event"].as_str().unwrap_or_default();
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
        self.view = view;
        let contexts: Vec<String> = self.instances.keys().cloned().collect();
        let mut messages: Vec<Value> = contexts.iter().flat_map(|c| self.draw(c)).collect();
        if let Some(open) = self.open_page.clone().filter(|_| listed) {
            messages.extend(self.tell_settings_page(&open));
        }
        messages
    }

    /// Take in what the meters read, and redraw the meters it moved.
    pub fn levels_changed(&mut self, levels: Peaks) -> Vec<Value> {
        self.levels = levels;
        let metered: Vec<String> = self
            .instances
            .iter()
            .filter(|(_, i)| matches!(i.action, Action::ChannelLevel | Action::MixLevel))
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
        let settings = &instance.settings;
        let Some(view) = &self.view else {
            return vec![alert(context)];
        };
        let change = match instance.action {
            Action::ChannelLevel | Action::MixLevel => {
                let Some(target) = settings.target(instance.action) else {
                    return vec![alert(context)];
                };
                // A dial moves the level and mutes it, whatever a key does.
                let mode = match press {
                    Press::Key => settings.mode,
                    Press::Turn(_) => Mode::Adjust,
                    Press::Dial | Press::Touch => Mode::Mute,
                };
                match (mode, press) {
                    (Mode::Adjust, Press::Turn(ticks)) => order(
                        &target,
                        json!({ "nudge": settings.step().abs() * ticks as f32 }),
                    ),
                    (Mode::Adjust, _) => order(&target, json!({ "nudge": settings.step() })),
                    (Mode::Set, _) => {
                        let Some(from) = view.find(&target).map(|found| found.volume) else {
                            return vec![alert(context)];
                        };
                        let to = settings.volume.unwrap_or(100.0).clamp(0.0, 100.0) / 100.0;
                        let length = Duration::from_millis(settings.fade.unwrap_or(0));
                        let now = Instant::now();
                        self.fades.retain(|fade| fade.target != target);
                        self.fades.push(Fade {
                            target,
                            from,
                            to,
                            start: now,
                            length,
                            last: now - FADE_STEP,
                        });
                        self.tick();
                        return Vec::new();
                    }
                    _ => order(&target, json!({ "mute": "toggle" })),
                }
            }
            Action::MonitorMix => {
                if matches!(press, Press::Turn(_)) {
                    return Vec::new();
                }
                let Some(mix) = settings.mix else {
                    return vec![alert(context)];
                };
                let heard = |id: u32| view.mixes.iter().any(|m| m.id == id && m.listening);
                let mix = match (settings.mode, settings.mix2) {
                    (Mode::Toggle, Some(other)) if heard(mix) => other,
                    _ => mix,
                };
                json!({ "what": "hear", "mix": mix, "only": true })
            }
            Action::MainOutput => {
                if matches!(press, Press::Turn(_)) {
                    return Vec::new();
                }
                let Some(device) = settings.device.clone() else {
                    return vec![alert(context)];
                };
                let device = match (settings.mode, &settings.device2) {
                    (Mode::Toggle, Some(other)) if view.listen.as_ref() == Some(&device) => {
                        other.clone()
                    }
                    _ => device,
                };
                json!({ "what": "listen", "device": device })
            }
        };
        if self.mixer.send(change) {
            Vec::new()
        } else {
            vec![alert(context)]
        }
    }

    /// What an action shows now, as messages for OpenDeck, unless it shows
    /// it already.
    fn draw(&mut self, context: &str) -> Vec<Value> {
        let Some(instance) = self.instances.get_mut(context) else {
            return Vec::new();
        };
        let (key, strip) = match picture(
            instance.action,
            &instance.settings,
            self.view.as_ref(),
            &self.levels,
        ) {
            Ok(picture) => (
                draw::key(&picture.as_picture()),
                draw::strip(&picture.as_picture()),
            ),
            Err(why) => {
                let label = &instance.settings.label;
                (
                    draw::waiting(label, why, false),
                    draw::waiting(label, why, true),
                )
            }
        };
        let shown = json!([key, strip]);
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
                "payload": { "canvas": strip },
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
        })).collect::<Vec<_>>(),
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
}

impl Drawn {
    fn as_picture(&self) -> Picture<'_> {
        Picture {
            name: &self.name,
            look: self.look,
            corner: self.corner,
            level: self.level,
            meter: self.meter,
            state: self.state,
            below: (&self.below.0, self.below.1),
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
    let unset = "Pick one";
    match action {
        Action::ChannelLevel | Action::MixLevel => {
            let target = settings.target(action).ok_or(unset)?;
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
            })
        }
        Action::MonitorMix => {
            let first = settings.mix.ok_or(unset)?;
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
            })
        }
        Action::MainOutput => {
            let first = settings.device.as_ref().ok_or(unset)?;
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
            })
        }
    }
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
