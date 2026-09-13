//! One mixer column: source name, a fader + mute per bus, a remove button.

use std::cell::Cell;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{ChainState, Command, MixBus, SourceConfig, SourceId};

use crate::engine_link::EngineLink;

/// Fader travel in UI units; mapped to `gain = value / 100`.
const FADER_MAX: f64 = 100.0;
const FADER_HEIGHT: i32 = 240;

struct Fader {
    scale: gtk::Scale,
    mute: gtk::ToggleButton,
    /// Set while we push engine state into the widgets, so the handlers do
    /// not echo it back as a command.
    syncing: Rc<Cell<bool>>,
}

impl Fader {
    fn new(id: SourceId, bus: MixBus, state: &ChainState, engine: &EngineLink) -> (gtk::Box, Self) {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
        column.set_hexpand(true);

        let label = gtk::Label::new(Some(bus.label()));
        label.add_css_class("caption-heading");
        column.append(&label);

        let scale = gtk::Scale::with_range(gtk::Orientation::Vertical, 0.0, FADER_MAX, 1.0);
        scale.set_inverted(true);
        scale.set_draw_value(true);
        scale.set_value_pos(gtk::PositionType::Bottom);
        scale.set_digits(0);
        scale.set_height_request(FADER_HEIGHT);
        scale.set_vexpand(true);
        scale.set_halign(gtk::Align::Center);
        scale.add_mark(FADER_MAX, gtk::PositionType::Left, None);
        scale.add_mark(FADER_MAX / 2.0, gtk::PositionType::Left, None);
        scale.add_mark(0.0, gtk::PositionType::Left, None);
        scale.set_value(f64::from(state.gain) * FADER_MAX);
        column.append(&scale);

        let mute = gtk::ToggleButton::new();
        mute.set_icon_name("audio-volume-muted-symbolic");
        mute.set_tooltip_text(Some("Mute"));
        mute.set_halign(gtk::Align::Center);
        mute.set_active(state.muted);
        mute.add_css_class("circular");
        column.append(&mute);

        let syncing = Rc::new(Cell::new(false));

        scale.connect_value_changed({
            let engine = engine.clone();
            let syncing = syncing.clone();
            move |scale| {
                if syncing.get() {
                    return;
                }
                let gain = (scale.value() / FADER_MAX) as f32;
                engine.send(Command::SetGain { id, bus, gain });
            }
        });
        mute.connect_toggled({
            let engine = engine.clone();
            let syncing = syncing.clone();
            move |button| {
                if syncing.get() {
                    return;
                }
                engine.send(Command::SetMute {
                    id,
                    bus,
                    muted: button.is_active(),
                });
            }
        });

        (
            column,
            Self {
                scale,
                mute,
                syncing,
            },
        )
    }

    fn set_state(&self, state: &ChainState) {
        self.syncing.set(true);
        self.scale.set_value(f64::from(state.gain) * FADER_MAX);
        self.mute.set_active(state.muted);
        self.syncing.set(false);
    }
}

pub struct SourceColumn {
    pub root: gtk::Box,
    stream: Fader,
    monitor: Fader,
}

impl SourceColumn {
    pub fn new(cfg: &SourceConfig, engine: &EngineLink) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("card");
        root.set_margin_top(6);
        root.set_margin_bottom(6);
        root.set_width_request(200);

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 12);
        inner.set_margin_top(12);
        inner.set_margin_bottom(12);
        inner.set_margin_start(12);
        inner.set_margin_end(12);
        root.append(&inner);

        let name = gtk::Label::new(Some(&cfg.name));
        name.add_css_class("title-3");
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(16);
        name.set_tooltip_text(Some(&cfg.name));
        inner.append(&name);

        let faders = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        faders.set_vexpand(true);
        let (stream_box, stream) = Fader::new(cfg.id, MixBus::Stream, &cfg.stream, engine);
        let (monitor_box, monitor) = Fader::new(cfg.id, MixBus::Monitor, &cfg.monitor, engine);
        faders.append(&stream_box);
        faders.append(&monitor_box);
        inner.append(&faders);

        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.set_tooltip_text(Some("Remove source"));
        remove.add_css_class("flat");
        remove.set_halign(gtk::Align::Center);
        remove.connect_clicked({
            let engine = engine.clone();
            let id = cfg.id;
            move |_| engine.send(Command::RemoveSource(id))
        });
        inner.append(&remove);

        Self {
            root,
            stream,
            monitor,
        }
    }

    pub fn set_state(&self, bus: MixBus, state: &ChainState) {
        match bus {
            MixBus::Stream => self.stream.set_state(state),
            MixBus::Monitor => self.monitor.set_state(state),
        }
    }
}
