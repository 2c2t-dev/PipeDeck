//! The sound card switch in the header bar.
//!
//! You listen to one mix — the one with the ear lit on its card — and this is
//! where you say on what: the device it plays to, and how loud. Picking
//! another device switches the mix over to it and off every other, which is
//! what changing sound cards means when you are wearing one of them.

use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{Command, Device, StateSnapshot};

use crate::engine_link::EngineLink;
use crate::widgets::FADER_MAX;

pub struct Listen {
    engine: EngineLink,
    button: gtk::MenuButton,
    label: gtk::Label,
}

impl Listen {
    pub fn new(engine: &EngineLink) -> Rc<Self> {
        let label = gtk::Label::new(Some("No output"));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(28);

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.append(&gtk::Image::from_icon_name("pd-listen-symbolic"));
        content.append(&label);

        let button = gtk::MenuButton::new();
        button.set_child(Some(&content));
        button.set_always_show_arrow(true);

        Rc::new(Self {
            engine: engine.clone(),
            button,
            label,
        })
    }

    pub fn widget(&self) -> gtk::Widget {
        self.button.clone().upcast()
    }

    /// Say what is being listened to, and offer what it could be.
    pub fn refresh(self: &Rc<Self>, state: &StateSnapshot, devices: &[Device]) {
        let mix = state
            .monitored_mix
            .and_then(|id| state.mixes.iter().find(|mix| mix.id == id));
        let Some(mix) = mix else {
            self.label.set_label("No mix");
            self.button
                .set_tooltip_text(Some("Add a mix to have something to listen to"));
            self.button.set_popover(None::<&gtk::Popover>);
            return;
        };

        // The device it plays to, if it plays anywhere. When several are on,
        // the first is the one this speaks of, as a switch leaves one only.
        let active = mix
            .outputs
            .iter()
            .enumerate()
            .find(|(_, output)| output.enabled);
        let describe = |name: &str| {
            devices
                .iter()
                .find(|device| device.name == name)
                .map_or_else(|| name.to_owned(), |device| device.description.clone())
        };
        self.label.set_label(&match active {
            Some((_, output)) => describe(&output.device),
            None => "No output".to_owned(),
        });
        self.button
            .set_tooltip_text(Some(&format!("Where you hear {}", mix.name)));

        let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page.set_margin_top(10);
        page.set_margin_bottom(10);
        page.set_margin_start(10);
        page.set_margin_end(10);
        page.set_width_request(300);

        let heading = gtk::Label::new(Some(&format!("Listening to {}", mix.name)));
        heading.add_css_class("caption");
        heading.add_css_class("dim-label");
        heading.set_xalign(0.0);
        page.append(&heading);

        let volume_title = gtk::Label::new(Some("Volume"));
        volume_title.add_css_class("heading");
        volume_title.set_xalign(0.0);
        volume_title.set_margin_top(6);
        page.append(&volume_title);

        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, FADER_MAX, 1.0);
        volume.set_draw_value(true);
        volume.set_value_pos(gtk::PositionType::Right);
        volume.set_digits(0);
        match active {
            Some((index, output)) => {
                volume.set_value(f64::from(output.gain) * FADER_MAX);
                volume.connect_value_changed({
                    let engine = self.engine.clone();
                    let id = mix.id;
                    move |scale| {
                        engine.send(Command::SetOutputGain {
                            id,
                            index,
                            gain: (scale.value() / FADER_MAX) as f32,
                        })
                    }
                });
            }
            None => volume.set_sensitive(false),
        }
        page.append(&volume);

        let output_title = gtk::Label::new(Some("Output"));
        output_title.add_css_class("heading");
        output_title.set_xalign(0.0);
        output_title.set_margin_top(10);
        page.append(&output_title);

        let popover = gtk::Popover::new();
        for device in devices {
            let is_active = active.is_some_and(|(_, output)| output.device == device.name);
            let name = gtk::Label::new(Some(&device.description));
            name.set_xalign(0.0);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            if is_active {
                // The accent colour, as the one in use; a check mark would
                // depend on an icon the desktop's theme may not have.
                name.add_css_class("accent");
                name.add_css_class("heading");
            }
            let button = gtk::Button::new();
            button.add_css_class("flat");
            button.set_child(Some(&name));
            button.connect_clicked({
                let engine = self.engine.clone();
                let popover = popover.clone();
                let id = mix.id;
                let device = device.name.clone();
                move |_| {
                    engine.send(Command::SwitchOutput {
                        id,
                        device: device.clone(),
                    });
                    popover.popdown();
                }
            });
            page.append(&button);
        }
        if devices.is_empty() {
            let none = gtk::Label::new(Some("No output device found."));
            none.add_css_class("dim-label");
            none.set_xalign(0.0);
            page.append(&none);
        }

        popover.set_child(Some(&page));
        self.button.set_popover(Some(&popover));
    }
}
