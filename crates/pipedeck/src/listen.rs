//! The sound card switch in the header bar.
//!
//! You listen on one device — your headphones — and the ear on each mix card
//! says whether that mix is heard there. This is where you say which device
//! it is and how loud: picking another moves every mix you hear onto it,
//! which is what changing sound cards means when you are wearing one of
//! them.

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

    /// Say what is being listened on, and offer what it could be.
    pub fn refresh(self: &Rc<Self>, state: &StateSnapshot, devices: &[Device]) {
        let listen = state.listen_device.clone();
        let describe = |name: &str| {
            devices
                .iter()
                .find(|device| device.name == name)
                .map_or_else(|| name.to_owned(), |device| device.description.clone())
        };
        self.label.set_label(&match &listen {
            Some(name) => describe(name),
            None => "No output".to_owned(),
        });
        self.button
            .set_tooltip_text(Some("The device you listen on, and how loud"));

        // The menu is made again on the next word from the engine once it
        // is shut: made again while open, it would close under the pointer,
        // a volume being dragged with it.
        if self
            .button
            .popover()
            .is_some_and(|popover| popover.is_visible())
        {
            return;
        }

        // Every output heard on that device: one per mix whose ear is lit.
        // They share the one volume up here.
        let heard: Vec<(pipedeck_engine::MixId, usize, f32)> = state
            .mixes
            .iter()
            .flat_map(|mix| {
                mix.outputs
                    .iter()
                    .enumerate()
                    .filter(|(_, output)| output.enabled && Some(&output.device) == listen.as_ref())
                    .map(move |(index, output)| (mix.id, index, output.gain))
            })
            .collect();

        let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        page.set_margin_top(10);
        page.set_margin_bottom(10);
        page.set_margin_start(10);
        page.set_margin_end(10);
        page.set_width_request(300);

        let names: Vec<&str> = state
            .mixes
            .iter()
            .filter(|mix| heard.iter().any(|(id, _, _)| *id == mix.id))
            .map(|mix| mix.name.as_str())
            .collect();
        let heading = gtk::Label::new(Some(&if names.is_empty() {
            "No mix is heard here; light the ear on one".to_owned()
        } else {
            format!("Hearing {}", names.join(", "))
        }));
        heading.add_css_class("caption");
        heading.add_css_class("dim-label");
        heading.set_xalign(0.0);
        heading.set_wrap(true);
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
        match heard.first() {
            Some((_, _, gain)) => {
                volume.set_value(f64::from(*gain) * FADER_MAX);
                volume.connect_value_changed({
                    let engine = self.engine.clone();
                    let heard = heard.clone();
                    move |scale| {
                        let gain = (scale.value() / FADER_MAX) as f32;
                        for (id, index, _) in &heard {
                            engine.send(Command::SetOutputGain {
                                id: *id,
                                index: *index,
                                gain,
                            });
                        }
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
            let is_active = listen.as_deref() == Some(device.name.as_str());
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
                let device = device.name.clone();
                move |_| {
                    engine.send(Command::SetListenDevice(device.clone()));
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
