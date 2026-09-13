//! The three dialogs: add a mix, add a source, choose a mix's devices.

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{Command, Device};

use crate::presets;
use crate::widgets;

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

/// Offer what a new channel can be: a capture device, or one of the ready
/// made empty channels.
///
/// Wave Link asks for the kind first and the name never, which is the right
/// order: a channel called Game with a red pad is recognised before it is
/// read. The name stays editable in the channel's own window.
pub fn add_source(parent: &impl IsA<gtk::Widget>, engine: &EngineLink, inputs: &[Device]) {
    let dialog = adw::Dialog::new();
    dialog.set_title("New channel");
    dialog.set_content_width(420);
    dialog.set_content_height(560);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    if !inputs.is_empty() {
        list.append(&section("Capture devices"));
        for device in inputs {
            let button = choice(
                &device.description,
                Some(presets::default_key(true)),
                true,
                &dialog,
                engine,
                Command::AddSource {
                    name: device.description.clone(),
                    device: Some(device.name.clone()),
                    icon: Some(presets::default_key(true).to_owned()),
                },
            );
            button.set_tooltip_text(Some(&device.name));
            list.append(&button);
        }
    }

    list.append(&section("Empty channels"));
    for preset in presets::kinds() {
        list.append(&choice(
            preset.label,
            Some(preset.key),
            false,
            &dialog,
            engine,
            Command::AddSource {
                name: preset.label.to_owned(),
                device: None,
                icon: Some(preset.key.to_owned()),
            },
        ));
    }

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_vexpand(true);
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_child(Some(&list));
    content.append(&scroller);

    let header = adw::HeaderBar::new();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&content));
    dialog.set_child(Some(&view));
    dialog.present(Some(parent));
}

/// One line of the new-channel list: a coloured badge and a name.
fn choice(
    label: &str,
    look: Option<&str>,
    is_input: bool,
    dialog: &adw::Dialog,
    engine: &EngineLink,
    command: Command,
) -> gtk::Button {
    let badge = widgets::badge(look, is_input, widgets::Tone::Colour, 16);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.append(&badge);
    row.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );

    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.set_child(Some(&row));
    button.connect_clicked({
        let engine = engine.clone();
        let dialog = dialog.clone();
        move |_| {
            engine.send(command.clone());
            dialog.close();
        }
    });
    button
}

fn section(label: &str) -> gtk::Widget {
    let heading = gtk::Label::new(Some(label));
    heading.add_css_class("caption-heading");
    heading.add_css_class("dim-label");
    heading.set_xalign(0.0);
    heading.set_margin_top(8);
    heading.set_margin_start(6);
    heading.upcast()
}
