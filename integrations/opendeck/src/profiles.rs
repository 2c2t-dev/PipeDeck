//! Ready-made OpenDeck profiles, laid out from the mixer as it is: one for
//! each Stream Deck OpenDeck knows, called "Pipedeck", next to the user's
//! own.
//!
//! - **Stream Deck** (5 × 3): the channels' mutes on top, the mixes to hear
//!   in the headphones under them, the mixes' mutes at the bottom, and the
//!   devices to listen on in the corner.
//! - **Stream Deck +** (4 × 2, 4 dials): a channel on each dial, the mixes'
//!   mutes and the mixes to hear on the keys.
//! - **Stream Deck XL** (8 × 4): the mixer's grid, a column for each
//!   channel with its own level on top and its level in each mix under it,
//!   then a column of mixes and one of mixes to hear.
//!
//! A deck is told apart by its keys and dials, from the profiles OpenDeck
//! has already written for it; one OpenDeck has not seen yet gets its
//! profile the next time this runs.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::deck::PLUGIN;
use crate::mixer::View;

/// What a key or a dial is given: one of the plugin's actions, and its
/// settings.
type Slot = Option<(&'static str, Value)>;

/// The decks laid out, by their keys and dials.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Model {
    StreamDeck,
    Plus,
    Xl,
}

impl Model {
    fn of(keys: usize, dials: usize) -> Option<Self> {
        match (keys, dials) {
            (15, 0) => Some(Model::StreamDeck),
            (8, 4) => Some(Model::Plus),
            (32, 0) => Some(Model::Xl),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Model::StreamDeck => "Stream Deck",
            Model::Plus => "Stream Deck +",
            Model::Xl => "Stream Deck XL",
        }
    }

    fn columns(self) -> usize {
        match self {
            Model::StreamDeck => 5,
            Model::Plus => 4,
            Model::Xl => 8,
        }
    }

    fn rows(self) -> usize {
        match self {
            Model::StreamDeck => 3,
            Model::Plus => 2,
            Model::Xl => 4,
        }
    }
}

/// The keys and dials of a deck, laid out from the mixer.
struct Layout {
    keys: Vec<Slot>,
    dials: Vec<Slot>,
}

impl Layout {
    fn new(model: Model) -> Self {
        Layout {
            keys: vec![None; model.columns() * model.rows()],
            dials: vec![None; if model == Model::Plus { 4 } else { 0 }],
        }
    }

    fn put(&mut self, model: Model, row: usize, column: usize, slot: Slot) {
        if row < model.rows() && column < model.columns() {
            self.keys[row * model.columns() + column] = slot;
        }
    }
}

fn channel(view: &View, index: usize) -> Slot {
    let channel = view.channels.get(index)?;
    Some((
        "channel",
        json!({ "channel": channel.id, "label": channel.name }),
    ))
}

fn cell(view: &View, channel: usize, mix: usize) -> Slot {
    let (channel, mix) = (view.channels.get(channel)?, view.mixes.get(mix)?);
    view.cells
        .iter()
        .any(|c| c.channel == channel.id && c.mix == mix.id)
        .then(|| {
            (
                "channel",
                json!({ "channel": channel.id, "mix": mix.id, "label": channel.name }),
            )
        })
}

fn mix_level(view: &View, index: usize) -> Slot {
    let mix = view.mixes.get(index)?;
    Some(("mix", json!({ "mix": mix.id, "label": mix.name })))
}

fn monitor(view: &View, index: usize) -> Slot {
    let mix = view.mixes.get(index)?;
    Some((
        "monitor",
        json!({ "mode": "select", "mix": mix.id, "label": mix.name }),
    ))
}

/// The devices to listen on: the other of two, or the one there is.
fn output(view: &View) -> Slot {
    let first = view.outputs.first()?;
    Some(match view.outputs.get(1) {
        Some(second) => (
            "output",
            json!({ "mode": "toggle", "device": first.name, "device2": second.name, "label": first.description }),
        ),
        None => (
            "output",
            json!({ "mode": "select", "device": first.name, "label": first.description }),
        ),
    })
}

fn layout(model: Model, view: &View) -> Layout {
    let mut layout = Layout::new(model);
    match model {
        Model::StreamDeck => {
            for i in 0..5 {
                layout.put(model, 0, i, channel(view, i));
                layout.put(model, 2, i, mix_level(view, i));
            }
            for i in 0..4 {
                layout.put(model, 1, i, monitor(view, i));
            }
            layout.put(model, 1, 4, output(view));
        }
        Model::Plus => {
            for i in 0..4 {
                layout.dials[i] = channel(view, i);
                layout.put(model, 0, i, mix_level(view, i));
            }
            for i in 0..3 {
                layout.put(model, 1, i, monitor(view, i));
            }
            layout.put(model, 1, 3, output(view));
        }
        Model::Xl => {
            // The grid: a channel's own level, then its level in each mix.
            for c in 0..6 {
                layout.put(model, 0, c, channel(view, c));
                for m in 0..3 {
                    layout.put(model, m + 1, c, cell(view, c, m));
                }
            }
            for m in 0..4 {
                layout.put(model, m, 6, mix_level(view, m));
            }
            for m in 0..3 {
                layout.put(model, m, 7, monitor(view, m));
            }
            layout.put(model, 3, 7, output(view));
        }
    }
    layout
}

/// An action where OpenDeck keeps it in a profile: its manifest entry,
/// where it is, and its settings.
fn instance(manifest: &Value, controller: &str, position: usize, slot: &Slot) -> Value {
    let Some((name, settings)) = slot else {
        return Value::Null;
    };
    let uuid = format!("{PLUGIN}.{name}");
    let Some(action) = manifest["Actions"]
        .as_array()
        .and_then(|actions| actions.iter().find(|a| a["UUID"] == uuid.as_str()))
    else {
        return Value::Null;
    };
    let folder = format!("plugins/{PLUGIN}.sdPlugin");
    let image = format!("{folder}/icons/{name}.svg");
    json!({
        "action": {
            "name": action["Name"],
            "uuid": uuid,
            "plugin": format!("{PLUGIN}.sdPlugin"),
            "tooltip": action["Tooltip"],
            "icon": image,
            "disable_automatic_states": false,
            "visible_in_action_list": true,
            "supported_in_multi_actions": true,
            "property_inspector": format!("{folder}/{}", action["PropertyInspectorPath"].as_str().unwrap_or_default()),
            "controllers": action["Controllers"],
            "encoder": {
                "icon": format!("icons/{name}"),
                "stack_color": "",
                "trigger_description": action["Encoder"]["TriggerDescription"],
                "background": "",
                "layout": action["Encoder"]["layout"],
            },
            "states": [{ "image": image }],
        },
        "context": format!("{controller}.{position}.0"),
        "states": [{ "image": image }],
        "current_state": 0,
        "settings": settings,
        "children": null,
    })
}

fn profile(model: Model, view: &View, manifest: &Value) -> Value {
    let layout = layout(model, view);
    json!({
        "keys": layout.keys.iter().enumerate().map(|(i, slot)| instance(manifest, "Keypad", i, slot)).collect::<Vec<_>>(),
        "sliders": layout.dials.iter().enumerate().map(|(i, slot)| instance(manifest, "Encoder", i, slot)).collect::<Vec<_>>(),
        "infobars": [],
    })
}

/// Where OpenDeck keeps its settings.
fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("OPENDECK_CONFIG") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_default();
    base.join("opendeck")
}

/// The keys and dials of the deck a folder of profiles is for.
fn model_of(device: &Path) -> Option<Model> {
    let profile = std::fs::read_dir(device)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.path().extension().is_some_and(|e| e == "json"))?;
    let profile: Value =
        serde_json::from_str(&std::fs::read_to_string(profile.path()).ok()?).ok()?;
    Model::of(
        profile["keys"].as_array()?.len(),
        profile["sliders"].as_array().map_or(0, Vec::len),
    )
}

/// Write a "Pipedeck" profile for every deck OpenDeck knows, and say what
/// was done.
pub fn write_all() -> Result<(), Box<dyn std::error::Error>> {
    let view = crate::mixer::fetch()
        .map_err(|e| format!("cannot reach Pipedeck, is it running? ({e})"))?
        .ok_or("Pipedeck is not ready yet")?;
    let config = config_dir();
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(
        config
            .join("plugins")
            .join(format!("{PLUGIN}.sdPlugin"))
            .join("manifest.json"),
    )?)?;
    let mut written = 0;
    for device in std::fs::read_dir(config.join("profiles"))? {
        let device = device?.path();
        if !device.is_dir() {
            continue;
        }
        let id = device.file_name().unwrap_or_default().to_string_lossy();
        let Some(model) = model_of(&device) else {
            println!("{id}: not a deck these profiles are for, left alone");
            continue;
        };
        let path = device.join("Pipedeck.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&profile(model, &view, &manifest))?,
        )?;
        println!(
            "{id}: {} profile written to {}",
            model.name(),
            path.display()
        );
        written += 1;
    }
    if written == 0 {
        return Err(
            "OpenDeck knows no Stream Deck yet: start it once with the decks plugged in".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        let cells: Vec<Value> = [[1, 1], [3, 1], [2, 2], [1, 3]]
            .iter()
            .map(|[c, m]| json!({"channel": c, "mix": m, "volume": 1.0, "muted": false}))
            .collect();
        serde_json::from_value(json!({
            "channels": [
                {"id": 1, "name": "Music", "icon": "music", "volume": 1.0, "muted": false, "input": false, "voices": []},
                {"id": 3, "name": "Voice chat", "icon": "voice", "volume": 1.0, "muted": false, "input": false, "voices": []},
                {"id": 2, "name": "Mic", "icon": "mic", "volume": 1.0, "muted": false, "input": true, "voices": []},
            ],
            "mixes": [
                {"id": 1, "name": "Personal", "icon": null, "volume": 1.0, "muted": false, "listening": true},
                {"id": 2, "name": "Chat", "icon": null, "volume": 1.0, "muted": false, "listening": false},
                {"id": 3, "name": "Stream", "icon": null, "volume": 1.0, "muted": false, "listening": false},
            ],
            "cells": cells,
            "listen": "phones",
            "outputs": [{"name": "phones", "description": "Headphones"}, {"name": "speakers", "description": "Speakers"}],
        }))
        .unwrap()
    }

    fn names(slots: &[Slot]) -> Vec<&str> {
        slots
            .iter()
            .map(|s| s.as_ref().map_or("-", |(n, _)| *n))
            .collect()
    }

    #[test]
    fn a_stream_deck_has_mutes_mixes_and_the_headphones() {
        let layout = layout(Model::StreamDeck, &view());
        assert_eq!(
            names(&layout.keys),
            [
                "channel", "channel", "channel", "-", "-", //
                "monitor", "monitor", "monitor", "-", "output", //
                "mix", "mix", "mix", "-", "-",
            ]
        );
        assert_eq!(layout.keys[9].as_ref().unwrap().1["mode"], "toggle");
    }

    #[test]
    fn a_stream_deck_plus_turns_the_channels() {
        let layout = layout(Model::Plus, &view());
        assert_eq!(names(&layout.dials), ["channel", "channel", "channel", "-"]);
        assert_eq!(
            names(&layout.keys),
            ["mix", "mix", "mix", "-", "monitor", "monitor", "monitor", "output"]
        );
    }

    #[test]
    fn a_stream_deck_xl_is_the_grid() {
        let layout = layout(Model::Xl, &view());
        // Music feeds Personal and Stream, Voice chat Personal, Mic Chat.
        let column =
            |c: usize| -> Vec<&str> { (0..4).map(|r| names(&layout.keys)[r * 8 + c]).collect() };
        assert_eq!(column(0), ["channel", "channel", "-", "channel"]);
        assert_eq!(column(2), ["channel", "-", "channel", "-"]);
        assert_eq!(column(6), ["mix", "mix", "mix", "-"]);
        assert_eq!(column(7), ["monitor", "monitor", "monitor", "output"]);
        assert_eq!(layout.keys[8].as_ref().unwrap().1["mix"], 1);
    }

    #[test]
    fn a_profile_is_as_opendeck_keeps_it() {
        let manifest: Value =
            serde_json::from_str(include_str!("../plugin/manifest.json")).unwrap();
        let profile = profile(Model::Plus, &view(), &manifest);
        assert_eq!(profile["keys"].as_array().unwrap().len(), 8);
        assert_eq!(profile["sliders"][0]["context"], "Encoder.0.0");
        assert_eq!(
            profile["sliders"][0]["action"]["uuid"],
            format!("{PLUGIN}.channel")
        );
        assert_eq!(profile["sliders"][3], Value::Null);
        assert_eq!(profile["keys"][7]["settings"]["device2"], "speakers");
    }
}
