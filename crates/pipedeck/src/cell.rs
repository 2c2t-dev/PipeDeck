//! One cell of the matrix: the fader a source has on a given mix.

use std::cell::Cell as StdCell;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{ChainState, Command, MixId, SourceId};

use crate::engine_link::EngineLink;

/// Fader travel in UI units; the gain is `value / FADER_MAX`.
const FADER_MAX: f64 = 100.0;

pub struct Cell {
    pub root: gtk::Box,
    /// The fader, with the level it passes drawn in its track.
    fader: crate::widgets::MeterFader,
    mute: gtk::ToggleButton,
    /// Set while we push engine state into the widgets, so the handlers do
    /// not echo it back to the engine as a command.
    syncing: Rc<StdCell<bool>>,
}

impl Cell {
    pub fn new(source: SourceId, mix: MixId, state: ChainState, engine: &EngineLink) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.set_margin_start(8);
        root.set_margin_end(8);
        root.set_valign(gtk::Align::Center);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        root.append(&controls);

        let mute = gtk::ToggleButton::new();
        mute.set_icon_name("audio-volume-muted-symbolic");
        mute.set_tooltip_text(Some("Mute"));
        mute.add_css_class("flat");
        mute.add_css_class("circular");
        mute.set_active(state.muted);
        controls.append(&mute);

        let fader = crate::widgets::MeterFader::new(state.gain);
        let scale = fader.scale.clone();
        // The value is drawn at the right edge of the scale, so keep it off
        // the unlink button.
        fader.root.set_margin_end(6);
        controls.append(&fader.root);

        let unlink = gtk::Button::from_icon_name("list-remove-symbolic");
        unlink.set_tooltip_text(Some("Unlink this channel from this mix"));
        unlink.add_css_class("flat");
        controls.append(&unlink);

        let syncing = Rc::new(StdCell::new(false));

        scale.connect_value_changed({
            let engine = engine.clone();
            let syncing = syncing.clone();
            move |scale| {
                if syncing.get() {
                    return;
                }
                engine.send(Command::SetLinkGain {
                    source,
                    mix,
                    gain: (scale.value() / FADER_MAX) as f32,
                });
            }
        });
        mute.connect_toggled({
            let engine = engine.clone();
            let syncing = syncing.clone();
            move |button| {
                if syncing.get() {
                    return;
                }
                engine.send(Command::SetLinkMute {
                    source,
                    mix,
                    muted: button.is_active(),
                });
            }
        });
        unlink.connect_clicked({
            let engine = engine.clone();
            move |_| {
                engine.send(Command::SetLink {
                    source,
                    mix,
                    linked: false,
                })
            }
        });

        Self {
            root,
            fader,
            mute,
            syncing,
        }
    }

    /// Draw the peak this cell passes on: what its channel hears, scaled by
    /// the fader in front of it.
    pub fn set_level(&self, channel_peak: f32) {
        let state = ChainState {
            gain: (self.fader.scale.value() / FADER_MAX) as f32,
            muted: self.mute.is_active(),
        };
        let passed = if state.muted {
            0.0
        } else {
            channel_peak * state.linear_volume()
        };
        self.fader.set_level(passed);
    }

    /// Push engine state into the widgets without sending it back.
    pub fn set_state(&self, state: ChainState) {
        self.syncing.set(true);
        self.fader
            .scale
            .set_value(f64::from(state.gain) * FADER_MAX);
        self.mute.set_active(state.muted);
        self.syncing.set(false);
    }
}

/// The empty cell: a button that links the source to the mix.
pub fn link_button(source: SourceId, mix: MixId, engine: &EngineLink) -> gtk::Button {
    let button = gtk::Button::from_icon_name("list-add-symbolic");
    button.set_tooltip_text(Some("Send this channel to this mix"));
    button.add_css_class("flat");
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
    button.connect_clicked({
        let engine = engine.clone();
        move |_| {
            engine.send(Command::SetLink {
                source,
                mix,
                linked: true,
            })
        }
    });
    button
}
