//! The three dialogs: add a mix, add a source, choose a mix's devices.

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{Command, Device};

use crate::engine_link::EngineLink;

/// Ask for a name and create a mix.
pub fn add_mix(parent: &impl IsA<gtk::Widget>, engine: &EngineLink) {
    let dialog = adw::AlertDialog::new(Some("New mix"), None);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Stream, Monitor, Record…"));
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("add", "Create");
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("add", false);

    entry.connect_changed({
        let dialog = dialog.clone();
        move |entry| dialog.set_response_enabled("add", !entry.text().trim().is_empty())
    });
    dialog.connect_response(Some("add"), {
        let engine = engine.clone();
        move |_, _| {
            let name = entry.text().trim().to_owned();
            if !name.is_empty() {
                engine.send(Command::AddMix { name });
            }
        }
    });
    dialog.present(Some(parent));
}

/// Ask for a name and a kind, then create a channel.
///
/// A channel is either empty, which shows up as a virtual output device
/// applications can select, or bound to a capture device such as a
/// microphone. The engine calls it a source.
pub fn add_source(parent: &impl IsA<gtk::Widget>, engine: &EngineLink, inputs: &[Device]) {
    let dialog = adw::AlertDialog::new(Some("New channel"), None);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Game, Music, Chat…"));
    entry.set_activates_default(true);
    content.append(&entry);

    let mut labels: Vec<String> = vec!["Empty channel (virtual output)".to_owned()];
    labels.extend(inputs.iter().map(|d| d.description.clone()));
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let kinds = gtk::DropDown::from_strings(&refs);
    kinds.set_tooltip_text(Some("What this source captures"));
    content.append(&kinds);
    dialog.set_extra_child(Some(&content));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("add", "Create");
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("add"));
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("add", false);

    // Picking a device prefills the name, so the common case is two clicks.
    kinds.connect_selected_notify({
        let entry = entry.clone();
        let inputs = inputs.to_vec();
        move |kinds| {
            let index = kinds.selected() as usize;
            if index > 0 && entry.text().trim().is_empty() {
                if let Some(device) = inputs.get(index - 1) {
                    entry.set_text(&device.description);
                }
            }
        }
    });
    entry.connect_changed({
        let dialog = dialog.clone();
        move |entry| dialog.set_response_enabled("add", !entry.text().trim().is_empty())
    });
    dialog.connect_response(Some("add"), {
        let engine = engine.clone();
        let inputs = inputs.to_vec();
        move |_, _| {
            let name = entry.text().trim().to_owned();
            if name.is_empty() {
                return;
            }
            let index = kinds.selected() as usize;
            let device = index
                .checked_sub(1)
                .and_then(|i| inputs.get(i))
                .map(|d| d.name.clone());
            engine.send(Command::AddSource { name, device });
        }
    });
    dialog.present(Some(parent));
}
