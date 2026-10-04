//! The keys and dials OpenDeck has put the plugin's actions on: what each
//! sends Pipedeck when used, and what it shows.
//!
//! Every action aims at one thing in the mixer, picked in its settings from
//! what Pipedeck has now, and kept by id: a channel renamed is still the
//! same key. It shows that thing as Pipedeck says it is, whoever changed it.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::draw::{self, State};
use crate::mixer::{order, Kind, Mixer, Target, View};

/// The plugin's identifier, which its actions' start with.
pub const PLUGIN: &str = "com.fabienmillet.pipedeck";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Mutes or unmutes.
    Mute,
    /// A key moves the level by its step; a dial moves it either way.
    Volume,
    /// Which mixes are heard in the headphones.
    Hear,
    /// The device the mixes are heard on.
    Output,
}

impl Action {
    fn from_uuid(uuid: &str) -> Option<Self> {
        match uuid.strip_prefix(PLUGIN)?.strip_prefix('.')? {
            "mute" => Some(Action::Mute),
            "volume" => Some(Action::Volume),
            "hear" => Some(Action::Hear),
            "output" => Some(Action::Output),
            _ => None,
        }
    }

    fn kinds(self) -> &'static [Kind] {
        match self {
            Action::Mute | Action::Volume => &[Kind::Channel, Kind::Mix, Kind::Cell, Kind::Voice],
            Action::Hear => &[Kind::Mix],
            Action::Output => &[Kind::Output],
        }
    }
}

/// One action where OpenDeck put it.
struct Instance {
    action: Action,
    /// On a dial, drawn on its part of the touch strip.
    dial: bool,
    settings: Value,
    /// What was last sent, so a change elsewhere does not redraw every key.
    shown: Option<Value>,
}

impl Instance {
    fn target(&self) -> Option<Target> {
        serde_json::from_value(self.settings.get("target")?.clone()).ok()
    }

    fn step(&self) -> f32 {
        self.settings
            .get("step")
            .and_then(Value::as_f64)
            .unwrap_or(5.0) as f32
            / 100.0
    }

    fn only(&self) -> bool {
        self.settings
            .get("only")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }
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
/// tell each what to do.
pub struct Deck {
    instances: HashMap<String, Instance>,
    view: Option<View>,
    mixer: Mixer,
}

impl Deck {
    pub fn new(mixer: Mixer) -> Self {
        Deck {
            instances: HashMap::new(),
            view: None,
            mixer,
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
            "propertyInspectorDidAppear" => self.choices(&context),
            "sendToPlugin" if payload["request"] == "targets" => self.choices(&context),
            _ => Vec::new(),
        }
    }

    /// Take in the mixer's new state, and redraw what it changed.
    pub fn mixer_changed(&mut self, view: Option<View>) -> Vec<Value> {
        if self.view == view {
            return Vec::new();
        }
        self.view = view;
        let contexts: Vec<String> = self.instances.keys().cloned().collect();
        contexts
            .iter()
            .flat_map(|context| self.draw(context))
            .collect()
    }

    /// Do what a press, a touch or a turn means for an action.
    fn press(&mut self, context: &str, press: Press) -> Vec<Value> {
        let Some(instance) = self.instances.get(context) else {
            return Vec::new();
        };
        let Some(target) = instance.target() else {
            return vec![alert(context)];
        };
        let change = match (instance.action, press) {
            (Action::Mute, Press::Turn(_)) => return Vec::new(),
            (Action::Mute, _) => order(&target, json!({ "mute": "toggle" })),
            (Action::Volume, Press::Key) => order(&target, json!({ "nudge": instance.step() })),
            (Action::Volume, Press::Turn(ticks)) => order(
                &target,
                json!({ "nudge": instance.step().abs() * ticks as f32 }),
            ),
            (Action::Volume, _) => order(&target, json!({ "mute": "toggle" })),
            (_, Press::Turn(_)) => return Vec::new(),
            (Action::Hear, _) => {
                let Target::Mix { id } = target else {
                    return vec![alert(context)];
                };
                if instance.only() {
                    json!({ "what": "hear", "mix": id, "only": true })
                } else {
                    json!({ "what": "hear", "mix": id, "listening": "toggle" })
                }
            }
            (Action::Output, _) => {
                let Target::Output { device } = target else {
                    return vec![alert(context)];
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
        let label = instance.settings["label"].as_str().unwrap_or_default();
        let found = instance
            .target()
            .and_then(|target| self.view.as_ref()?.find(&target));
        let (key, strip) = match found {
            None => {
                let why = if self.view.is_none() {
                    "Offline"
                } else if instance.target().is_some() {
                    "Gone"
                } else {
                    "Pick one"
                };
                (
                    draw::waiting(label, why, false),
                    draw::waiting(label, why, true),
                )
            }
            Some(found) => {
                let mut look = draw::look(found.icon.as_deref(), found.input, found.mix);
                let muted = if found.muted { "Muted" } else { "" };
                let (level, state, below) = match instance.action {
                    Action::Mute => (
                        None,
                        State {
                            muted: found.muted,
                            dim: false,
                        },
                        (muted.to_owned(), draw::RED),
                    ),
                    Action::Volume => (
                        Some(found.volume),
                        State {
                            muted: found.muted,
                            dim: false,
                        },
                        if found.muted {
                            ("Muted".to_owned(), draw::RED)
                        } else {
                            (draw::percent(found.volume), "#ffffff")
                        },
                    ),
                    Action::Hear => (
                        None,
                        State {
                            muted: false,
                            dim: !found.listening,
                        },
                        (
                            if found.listening { "Heard" } else { "" }.to_owned(),
                            "#ffffff",
                        ),
                    ),
                    Action::Output => {
                        look = draw::look(Some("headset"), false, false);
                        (
                            None,
                            State {
                                muted: false,
                                dim: !found.listening,
                            },
                            (
                                if found.listening { "Listening" } else { "" }.to_owned(),
                                "#ffffff",
                            ),
                        )
                    }
                };
                let below = (below.0.as_str(), below.1);
                (
                    draw::key(&found.name, look, level, state, below),
                    draw::strip(&found.name, look, level, state, below),
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

    /// The list a key's settings pick its target from.
    fn choices(&self, context: &str) -> Vec<Value> {
        let Some(instance) = self.instances.get(context) else {
            return Vec::new();
        };
        let targets: Vec<Value> = self
            .view
            .as_ref()
            .map(|view| view.targets(instance.action.kinds()))
            .unwrap_or_default()
            .into_iter()
            .map(|(target, label)| json!({ "target": target, "label": label }))
            .collect();
        vec![json!({
            "event": "sendToPropertyInspector",
            "context": context,
            "payload": {
                "targets": targets,
                "running": self.view.is_some(),
            },
        })]
    }
}

fn settings_of(payload: &Value) -> Value {
    match &payload["settings"] {
        Value::Object(_) => payload["settings"].clone(),
        _ => json!({}),
    }
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
            "mixes": [{"id": 1, "name": "Stream", "icon": null, "volume": 1.0,
                       "muted": false, "listening": true}],
            "cells": [{"channel": 2, "mix": 1, "volume": 1.0, "muted": false}],
            "listen": "phones",
            "outputs": [{"name": "phones", "description": "Headphones"}],
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

    #[test]
    fn a_key_shows_its_target_and_draws_again_only_on_change() {
        let mut deck = Deck::new(Mixer::default());
        let shown = appear(
            &mut deck,
            "volume",
            "Encoder",
            json!({"target": {"what": "channel", "id": 2}}),
        );
        // Offline: nothing told yet.
        assert_eq!(shown.len(), 2);
        assert!(shown[1]["payload"]["canvas"]
            .as_str()
            .unwrap()
            .contains("Offline"));

        let shown = deck.mixer_changed(Some(view()));
        assert!(shown[1]["payload"]["canvas"]
            .as_str()
            .unwrap()
            .contains("50%"));
        assert!(deck.mixer_changed(Some(view())).is_empty());

        let mut louder = view();
        louder.mixes[0].volume = 0.2;
        assert!(
            deck.mixer_changed(Some(louder)).is_empty(),
            "the mix is not on this key"
        );
    }

    #[test]
    fn the_settings_list_what_an_action_can_aim_at() {
        let mut deck = Deck::new(Mixer::default());
        deck.mixer_changed(Some(view()));
        appear(&mut deck, "hear", "Keypad", json!({}));
        let told = deck.hear(&json!({"event": "propertyInspectorDidAppear", "context": "here"}));
        assert_eq!(
            told[0]["payload"]["targets"],
            json!([{"target": {"what": "mix", "id": 1}, "label": "Mix · Stream"}])
        );
    }

    #[test]
    fn a_key_without_pipedeck_says_so() {
        let mut deck = Deck::new(Mixer::default());
        appear(
            &mut deck,
            "mute",
            "Keypad",
            json!({"target": {"what": "mix", "id": 1}}),
        );
        let told = deck.hear(&json!({"event": "keyDown", "context": "here", "payload": {}}));
        assert_eq!(told, vec![alert("here")]);
    }
}
