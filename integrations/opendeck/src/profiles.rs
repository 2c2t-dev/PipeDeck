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
//! Each has a Call key, to a second profile, "Pipedeck Call": the people
//! of a Discord call by their place in it, on the dials first and then the
//! keys, so it follows the call as people come and go; and a key back.
//! OpenDeck lets no plugin but its own Starter Pack switch a deck's
//! profile, so those two keys are the Starter Pack's Switch Profile,
//! drawn as Pipedeck's keys are.
//!
//! StreamController gets the same, as pages: it keeps pages apart from
//! decks, so there is a pair for each kind of deck, named after it, and a
//! deck is given its own in StreamController.
//!
//! A deck is told apart by its keys and dials, from the profiles OpenDeck
//! has already written for it; one OpenDeck has not seen yet gets its
//! profile the next time this runs.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::deck::{CALL_PROFILE, MIXER_PROFILE, PLUGIN};
use crate::draw::{self, Picture, State};
use crate::mixer::View;

/// OpenDeck's Starter Pack, which comes with it, and its action that
/// switches a deck's profile.
const STARTER_PACK: &str = "com.amansprojects.starterpack";
const SWITCH_PROFILE: &str = "com.amansprojects.starterpack.switchprofile";

/// The manifests of the plugins a profile's actions come from. Without the
/// Starter Pack, the Call keys are left empty.
struct Manifests {
    pipedeck: Value,
    starter_pack: Option<Value>,
}

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

/// A key for each effect of a microphone, to switch it off and on: noise
/// suppression, most often.
fn microphone_effects(view: &View) -> Vec<Slot> {
    view.channels
        .iter()
        .filter(|channel| channel.input)
        .flat_map(|channel| {
            channel.effects.iter().enumerate().map(|(index, effect)| {
                Some((
                    "effect",
                    json!({
                        "channel": channel.id,
                        "index": index,
                        "effect": effect.name,
                        "label": effect.name,
                    }),
                ))
            })
        })
        .collect()
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

/// The key between the mixer's profile and the call's.
fn call_page(to: &str) -> Slot {
    Some(("call", json!({ "page": to })))
}

/// The person at a place in the call, from 1.
fn voice(place: usize) -> Slot {
    Some((
        "voice",
        json!({ "slot": place, "label": format!("Person {place}") }),
    ))
}

/// The call's profile: a person on every dial and key, the last key back.
fn call_layout(model: Model) -> Layout {
    let mut layout = Layout::new(model);
    let mut place = 1;
    for dial in layout.dials.iter_mut() {
        *dial = voice(place);
        place += 1;
    }
    let last = layout.keys.len() - 1;
    for key in layout.keys[..last].iter_mut() {
        *key = voice(place);
        place += 1;
    }
    layout.keys[last] = call_page("mixer");
    layout
}

fn layout(model: Model, view: &View) -> Layout {
    let mut layout = Layout::new(model);
    match model {
        Model::StreamDeck => stream_deck_layout(&mut layout, view),
        Model::Plus => plus_layout(&mut layout, view),
        Model::Xl => xl_layout(&mut layout, view),
    }
    // The microphone's effects on the keys left: switched off and on, they
    // are worth a key, not a dial.
    let mut effects = microphone_effects(view).into_iter();
    let free = layout.keys.iter_mut().filter(|slot| slot.is_none());
    for (slot, effect) in free.zip(&mut effects) {
        *slot = effect;
    }
    layout
}

/// Mutes along the top, the mixes heard and their levels under them.
fn stream_deck_layout(layout: &mut Layout, view: &View) {
    let model = Model::StreamDeck;
    for i in 0..5 {
        layout.put(model, 0, i, channel(view, i));
    }
    for i in 0..4 {
        layout.put(model, 1, i, monitor(view, i));
        layout.put(model, 2, i, mix_level(view, i));
    }
    layout.put(model, 1, 4, output(view));
    layout.put(model, 2, 4, call_page("call"));
}

/// The channels on the dials, the mixes on the keys.
fn plus_layout(layout: &mut Layout, view: &View) {
    let model = Model::Plus;
    for i in 0..4 {
        layout.dials[i] = channel(view, i);
    }
    for i in 0..3 {
        layout.put(model, 0, i, mix_level(view, i));
        layout.put(model, 1, i, monitor(view, i));
    }
    layout.put(model, 0, 3, call_page("call"));
    layout.put(model, 1, 3, output(view));
}

/// The grid: a channel's own level, then its level in each mix.
fn xl_layout(layout: &mut Layout, view: &View) {
    let model = Model::Xl;
    for c in 0..6 {
        layout.put(model, 0, c, channel(view, c));
        for m in 0..3 {
            layout.put(model, m + 1, c, cell(view, c, m));
        }
    }
    for m in 0..3 {
        layout.put(model, m, 6, mix_level(view, m));
    }
    layout.put(model, 3, 6, call_page("call"));
    for m in 0..3 {
        layout.put(model, m, 7, monitor(view, m));
    }
    layout.put(model, 3, 7, output(view));
}

/// An action where OpenDeck keeps it in a profile: its manifest entry,
/// where it is, and its settings.
fn instance(manifests: &Manifests, controller: &str, position: usize, slot: &Slot) -> Value {
    let Some((name, settings)) = slot else {
        return Value::Null;
    };
    let (plugin, manifest, uuid, settings, image) = match (*name, &manifests.starter_pack) {
        ("call", Some(starter_pack)) => {
            let back = settings["page"] == "mixer";
            let profile = if back { MIXER_PROFILE } else { CALL_PROFILE };
            (
                STARTER_PACK,
                starter_pack,
                SWITCH_PROFILE.to_owned(),
                json!({ "profile": profile }),
                draw::data_url(&page_key(back)),
            )
        }
        ("call", None) => return Value::Null,
        _ => (
            PLUGIN,
            &manifests.pipedeck,
            format!("{PLUGIN}.{name}"),
            settings.clone(),
            format!("plugins/{PLUGIN}.sdPlugin/icons/{name}.svg"),
        ),
    };
    let Some(action) = manifest["Actions"]
        .as_array()
        .and_then(|actions| actions.iter().find(|a| a["UUID"] == uuid.as_str()))
    else {
        return Value::Null;
    };
    let folder = format!("plugins/{plugin}.sdPlugin");
    // What a manifest leaves out is left out here too: OpenDeck takes a
    // null for a wrong type and throws the whole profile away.
    let action = without_nulls(json!({
        "name": action["Name"],
        "uuid": uuid,
        "plugin": format!("{plugin}.sdPlugin"),
        "tooltip": action["Tooltip"],
        "icon": image,
        "disable_automatic_states": false,
        "visible_in_action_list": true,
        "supported_in_multi_actions": true,
        "property_inspector": format!("{folder}/{}", action["PropertyInspectorPath"].as_str().unwrap_or_default()),
        "controllers": action["Controllers"],
        "encoder": {
            "icon": if plugin == PLUGIN { format!("icons/{name}") } else { image.clone() },
            "stack_color": "",
            "trigger_description": action["Encoder"]["TriggerDescription"],
            "background": "",
            "layout": action["Encoder"]["layout"],
        },
        "states": [{ "image": image }],
    }));
    json!({
        "action": action,
        "context": format!("{controller}.{position}.0"),
        "states": [{ "image": image }],
        "current_state": 0,
        "settings": settings,
        "children": null,
    })
}

/// A value with its null fields taken out, all the way down.
fn without_nulls(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, without_nulls(v)))
                .collect(),
        ),
        other => other,
    }
}

/// The Call key's picture, or the key back's, drawn as Pipedeck's keys
/// are.
fn page_key(back: bool) -> String {
    let (name, look, below) = if back {
        ("Mixer", ("pd-listen-symbolic", draw::WHITE), "Back")
    } else {
        ("Call", draw::look(Some("people"), false, false), "People")
    };
    draw::key(&Picture {
        name,
        look,
        corner: None,
        level: None,
        meter: None,
        state: State::default(),
        below: (below, "#ffffff"),
        avatar: None,
    })
}

fn profile(layout: &Layout, manifests: &Manifests) -> Value {
    json!({
        "keys": layout.keys.iter().enumerate().map(|(i, slot)| instance(manifests, "Keypad", i, slot)).collect::<Vec<_>>(),
        "sliders": layout.dials.iter().enumerate().map(|(i, slot)| instance(manifests, "Encoder", i, slot)).collect::<Vec<_>>(),
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
    let manifest = |plugin: &str| -> Result<Value, Box<dyn std::error::Error>> {
        let path = config
            .join("plugins")
            .join(format!("{plugin}.sdPlugin"))
            .join("manifest.json");
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    };
    let manifests = Manifests {
        pipedeck: manifest(PLUGIN)?,
        starter_pack: manifest(STARTER_PACK).ok(),
    };
    if manifests.starter_pack.is_none() {
        println!("OpenDeck's Starter Pack is not installed: the Call keys are left out");
    }
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
        for (name, layout) in [
            (MIXER_PROFILE, layout(model, &view)),
            (CALL_PROFILE, call_layout(model)),
        ] {
            let path = device.join(format!("{name}.json"));
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&profile(&layout, &manifests))?,
            )?;
            println!(
                "{id}: {} profile written to {}",
                model.name(),
                path.display()
            );
        }
        written += 1;
    }
    if written == 0 {
        return Err(
            "OpenDeck knows no Stream Deck yet: start it once with the decks plugged in".into(),
        );
    }
    Ok(())
}

/// StreamController's pages for a kind of deck: the mixer's, and the
/// call's.
fn streamcontroller_names(model: Model) -> (String, String) {
    let mixer = match model {
        Model::StreamDeck => MIXER_PROFILE.to_owned(),
        Model::Plus => format!("{MIXER_PROFILE} +"),
        Model::Xl => format!("{MIXER_PROFILE} XL"),
    };
    let call = format!("{mixer} Call");
    (mixer, call)
}

/// StreamController's name for one of the plugin's actions.
fn streamcontroller_action(name: &str) -> Option<&'static str> {
    Some(match name {
        "channel" => "ChannelLevel",
        "mix" => "MixLevel",
        "monitor" => "MonitorMix",
        "output" => "MainOutput",
        "voice" => "CallVoice",
        "call" => "CallPage",
        "effect" => "ChannelEffect",
        "app" => "AddApp",
        _ => return None,
    })
}

/// A layout as one of StreamController's pages: keys by their column and
/// row, dials by their place. A Call key is told the page it goes to by
/// name, since StreamController's pages are not a deck's own.
fn streamcontroller_page(model: Model, layout: &Layout) -> Value {
    let (mixer, call) = streamcontroller_names(model);
    let slot = |slot: &Slot| -> Option<Value> {
        let (name, settings) = slot.as_ref()?;
        let action = streamcontroller_action(name)?;
        let mut settings = settings.clone();
        if *name == "call" {
            let to = if settings["page"] == "mixer" {
                &mixer
            } else {
                &call
            };
            settings["to"] = Value::from(to.clone());
        }
        Some(json!({
            "states": { "0": {
                "actions": [{ "id": format!("dev_2c2t_Pipedeck::{action}"), "settings": settings }],
                "image-control-action": 0,
                "label-control-actions": [0, 0, 0],
                "background-control-action": 0,
            }},
        }))
    };
    let mut keys = serde_json::Map::new();
    for (index, key) in layout.keys.iter().enumerate() {
        if let Some(key) = slot(key) {
            let (column, row) = (index % model.columns(), index / model.columns());
            keys.insert(format!("{column}x{row}"), key);
        }
    }
    let mut dials = serde_json::Map::new();
    for (index, dial) in layout.dials.iter().enumerate() {
        if let Some(dial) = slot(dial) {
            dials.insert(index.to_string(), dial);
        }
    }
    json!({ "keys": keys, "dials": dials })
}

/// Write StreamController's Pipedeck pages, for every kind of deck, into
/// its pages folder, and say what was done.
pub fn write_streamcontroller(pages: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let view = crate::mixer::fetch()
        .map_err(|e| format!("cannot reach Pipedeck, is it running? ({e})"))?
        .ok_or("Pipedeck is not ready yet")?;
    std::fs::create_dir_all(pages)?;
    for model in [Model::StreamDeck, Model::Plus, Model::Xl] {
        let (mixer, call) = streamcontroller_names(model);
        for (name, layout) in [(mixer, layout(model, &view)), (call, call_layout(model))] {
            let path = pages.join(format!("{name}.json"));
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&streamcontroller_page(model, &layout))?,
            )?;
            println!("{} page written to {}", model.name(), path.display());
        }
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
                "mix", "mix", "mix", "-", "call",
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
            ["mix", "mix", "mix", "call", "monitor", "monitor", "monitor", "output"]
        );
    }

    #[test]
    fn a_microphone_s_effects_take_what_is_left() {
        let mut view = view();
        view.channels[2].effects = vec![crate::mixer::EffectState {
            name: "Noise suppression".into(),
            bypassed: false,
        }];
        let deck = layout(Model::StreamDeck, &view);
        assert_eq!(names(&deck.keys)[3], "effect");
        assert_eq!(
            deck.keys[3].as_ref().unwrap().1["effect"],
            "Noise suppression"
        );
        // A Stream Deck + has no key left, and a dial is no place for it.
        let plus = layout(Model::Plus, &view);
        assert_eq!(names(&plus.dials), ["channel", "channel", "channel", "-"]);
        assert!(!names(&plus.keys).contains(&"effect"));
    }

    #[test]
    fn a_stream_deck_xl_is_the_grid() {
        let layout = layout(Model::Xl, &view());
        // Music feeds Personal and Stream, Voice chat Personal, Mic Chat.
        let column =
            |c: usize| -> Vec<&str> { (0..4).map(|r| names(&layout.keys)[r * 8 + c]).collect() };
        assert_eq!(column(0), ["channel", "channel", "-", "channel"]);
        assert_eq!(column(2), ["channel", "-", "channel", "-"]);
        assert_eq!(column(6), ["mix", "mix", "mix", "call"]);
        assert_eq!(column(7), ["monitor", "monitor", "monitor", "output"]);
        assert_eq!(layout.keys[8].as_ref().unwrap().1["mix"], 1);
    }

    #[test]
    fn the_call_is_people_by_their_place_and_a_way_back() {
        let layout = call_layout(Model::Plus);
        let places: Vec<_> = layout
            .dials
            .iter()
            .chain(&layout.keys)
            .filter_map(|slot| slot.as_ref()?.1["slot"].as_u64())
            .collect();
        assert_eq!(places, (1..=11).collect::<Vec<_>>());
        assert_eq!(layout.keys[7].as_ref().unwrap().1["page"], "mixer");
        assert_eq!(call_layout(Model::Xl).keys.iter().flatten().count(), 32);
    }

    #[test]
    fn a_streamcontroller_page_is_keyed_by_column_and_row() {
        let page = streamcontroller_page(Model::Plus, &layout(Model::Plus, &view()));
        assert_eq!(
            page["keys"]["3x0"]["states"]["0"]["actions"][0]["id"],
            "dev_2c2t_Pipedeck::CallPage"
        );
        assert_eq!(
            page["keys"]["3x0"]["states"]["0"]["actions"][0]["settings"]["to"],
            "Pipedeck + Call"
        );
        assert_eq!(
            page["dials"]["0"]["states"]["0"]["actions"][0]["id"],
            "dev_2c2t_Pipedeck::ChannelLevel"
        );
        let call = streamcontroller_page(Model::Plus, &call_layout(Model::Plus));
        assert_eq!(
            call["keys"]["3x1"]["states"]["0"]["actions"][0]["settings"]["to"],
            "Pipedeck +"
        );
    }

    #[test]
    fn a_profile_is_as_opendeck_keeps_it() {
        let manifests = Manifests {
            pipedeck: serde_json::from_str(include_str!("../plugin/manifest.json")).unwrap(),
            starter_pack: Some(json!({ "Actions": [{
                "Name": "Switch Profile",
                "UUID": SWITCH_PROFILE,
                "Controllers": ["Keypad", "Encoder"],
                "PropertyInspectorPath": "propertyInspector/switchProfile.html",
            }]})),
        };
        let profile = profile(&layout(Model::Plus, &view()), &manifests);
        assert_eq!(profile["keys"].as_array().unwrap().len(), 8);
        assert_eq!(profile["sliders"][0]["context"], "Encoder.0.0");
        assert_eq!(
            profile["sliders"][0]["action"]["uuid"],
            format!("{PLUGIN}.channel")
        );
        assert_eq!(profile["sliders"][3], Value::Null);
        assert_eq!(profile["keys"][7]["settings"]["device2"], "speakers");
    }

    #[test]
    fn the_call_key_and_the_key_back_are_the_starter_packs() {
        let starter_pack =
            json!({ "Actions": [{ "Name": "Switch Profile", "UUID": SWITCH_PROFILE }] });
        let manifests = Manifests {
            pipedeck: serde_json::from_str(include_str!("../plugin/manifest.json")).unwrap(),
            starter_pack: Some(starter_pack),
        };
        let mixer = profile(&layout(Model::Plus, &view()), &manifests);
        let call = &mixer["keys"][3];
        assert_eq!(call["action"]["uuid"], SWITCH_PROFILE);
        assert_eq!(
            call["action"]["plugin"],
            "com.amansprojects.starterpack.sdPlugin"
        );
        assert_eq!(call["settings"], json!({ "profile": CALL_PROFILE }));
        assert!(call["states"][0]["image"]
            .as_str()
            .unwrap()
            .starts_with("data:image/svg+xml;base64,"));
        // A manifest's action with no dial says nothing of one, which
        // OpenDeck must not be given as null.
        fn nulls(value: &Value) -> bool {
            match value {
                Value::Null => true,
                Value::Object(fields) => fields.values().any(nulls),
                Value::Array(items) => items.iter().any(nulls),
                _ => false,
            }
        }
        assert!(!nulls(&call["action"]), "{}", call["action"]);
        let back = profile(&call_layout(Model::Plus), &manifests);
        assert_eq!(
            back["keys"][7]["settings"],
            json!({ "profile": MIXER_PROFILE })
        );

        // Without it, there is no key that can go there.
        let alone = Manifests {
            starter_pack: None,
            ..manifests
        };
        let mixer = profile(&layout(Model::Plus, &view()), &alone);
        assert_eq!(mixer["keys"][3], Value::Null);
    }
}
