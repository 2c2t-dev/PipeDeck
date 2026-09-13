//! The window behind a mix card: its name, its master level, and the devices
//! it plays to, each with a level of its own.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{ChainState, Command, Device, MixConfig, MixId};

use crate::engine_link::EngineLink;
use crate::widgets;

use widgets::FADER_MAX;

const LEFT_PANE_WIDTH: i32 = 240;

pub struct MixDialog {
    dialog: adw::Dialog,
    id: MixId,
    engine: EngineLink,
    name: gtk::Entry,
    volume: gtk::Scale,
    mute: gtk::ToggleButton,
    outputs: gtk::Box,
    add_output: gtk::MenuButton,
    /// Devices currently attached, in engine order, so a row knows its index.
    attached: RefCell<Vec<String>>,
    /// Set while engine state is pushed into the widgets, so the handlers do
    /// not echo it back as a command.
    syncing: Rc<StdCell<bool>>,
}

impl MixDialog {
    /// Build and show the window for `mix`.
    pub fn present(
        parent: &impl IsA<gtk::Widget>,
        engine: &EngineLink,
        mix: &MixConfig,
        devices: &[Device],
    ) -> Rc<Self> {
        let dialog = adw::Dialog::new();
        dialog.set_title("Mix");
        dialog.set_content_width(760);
        dialog.set_content_height(480);

        let name = gtk::Entry::new();
        name.add_css_class("title-4");
        name.set_hexpand(true);

        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, FADER_MAX, 1.0);
        volume.set_hexpand(true);
        volume.set_draw_value(true);
        volume.set_value_pos(gtk::PositionType::Right);
        volume.set_digits(0);

        let mute = gtk::ToggleButton::new();
        mute.set_icon_name("audio-volume-muted-symbolic");
        mute.set_tooltip_text(Some("Mute the whole mix"));
        mute.add_css_class("flat");
        mute.add_css_class("circular");

        let outputs = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let add_output = gtk::MenuButton::new();
        add_output.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("Add output")
                .build(),
        ));
        add_output.set_tooltip_text(Some("Send this mix to one more device"));

        let this = Rc::new(Self {
            dialog: dialog.clone(),
            id: mix.id,
            engine: engine.clone(),
            name,
            volume,
            mute,
            outputs,
            add_output,
            attached: RefCell::new(Vec::new()),
            syncing: Rc::new(StdCell::new(false)),
        });

        dialog.set_child(Some(&this.build()));
        this.refresh(mix, devices);
        this.connect(mix);
        dialog.present(Some(parent));
        this
    }

    fn build(self: &Rc<Self>) -> gtk::Widget {
        let header = adw::HeaderBar::new();

        let panes = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        panes.set_margin_top(18);
        panes.set_margin_bottom(18);
        panes.set_margin_start(18);
        panes.set_margin_end(18);

        // Left: identity and master level.
        let left = gtk::Box::new(gtk::Orientation::Vertical, 12);
        left.set_width_request(LEFT_PANE_WIDTH);
        left.append(&self.name);

        let icon = widgets::big_badge("audio-speakers-symbolic", None);
        icon.set_margin_top(12);
        left.append(&icon);

        let caption = gtk::Label::new(Some("Mix volume"));
        caption.add_css_class("dim-label");
        caption.set_margin_top(12);
        left.append(&caption);

        let level = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        level.append(&self.mute);
        level.append(&self.volume);
        left.append(&level);

        let filler = gtk::Box::new(gtk::Orientation::Vertical, 0);
        filler.set_vexpand(true);
        left.append(&filler);

        let delete = gtk::Button::new();
        delete.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("user-trash-symbolic")
                .label("Delete mix")
                .build(),
        ));
        delete.add_css_class("destructive-action");
        delete.set_halign(gtk::Align::Start);
        delete.connect_clicked({
            let this = self.clone();
            move |_| {
                this.engine.send(Command::RemoveMix(this.id));
                this.dialog.close();
            }
        });
        left.append(&delete);
        panes.append(&left);

        // Right: where the mix goes.
        let right = gtk::Box::new(gtk::Orientation::Vertical, 12);
        right.set_hexpand(true);

        let title = gtk::Label::new(Some("Audio output"));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        right.append(&title);

        let hint = gtk::Label::new(Some(
            "A mix is capturable by OBS whether or not it plays to a device.",
        ));
        hint.add_css_class("caption");
        hint.add_css_class("dim-label");
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        right.append(&hint);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&self.outputs));
        right.append(&scroller);

        self.add_output.set_halign(gtk::Align::Center);
        right.append(&self.add_output);
        panes.append(&right);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&panes));
        view.upcast()
    }

    fn connect(self: &Rc<Self>, mix: &MixConfig) {
        let previous = Rc::new(RefCell::new(mix.name.clone()));

        // Renaming lands when the field is validated or the window closes,
        // rather than on every keystroke.
        let rename = {
            let this = self.clone();
            let previous = previous.clone();
            move || {
                let chosen = this.name.text().trim().to_owned();
                if !chosen.is_empty() && chosen != *previous.borrow() {
                    *previous.borrow_mut() = chosen.clone();
                    this.engine.send(Command::RenameMix {
                        id: this.id,
                        name: chosen,
                    });
                }
            }
        };
        self.name.connect_activate({
            let rename = rename.clone();
            move |_| rename()
        });
        self.dialog.connect_closed(move |_| rename());

        self.volume.connect_value_changed({
            let this = self.clone();
            move |scale| {
                if this.syncing.get() {
                    return;
                }
                this.engine.send(Command::SetMixGain {
                    id: this.id,
                    gain: (scale.value() / FADER_MAX) as f32,
                });
            }
        });
        self.mute.connect_toggled({
            let this = self.clone();
            move |button| {
                if this.syncing.get() {
                    return;
                }
                this.engine.send(Command::SetMixMute {
                    id: this.id,
                    muted: button.is_active(),
                });
            }
        });
    }

    pub fn id(&self) -> MixId {
        self.id
    }

    pub fn close(&self) {
        self.dialog.close();
    }

    /// Push engine state into the window, rebuilding the output list.
    pub fn refresh(self: &Rc<Self>, mix: &MixConfig, devices: &[Device]) {
        self.syncing.set(true);
        if self.name.text() != mix.name {
            self.name.set_text(&mix.name);
        }
        self.volume.set_value(f64::from(mix.gain) * FADER_MAX);
        self.mute.set_active(mix.muted);

        *self.attached.borrow_mut() = mix.outputs.iter().map(|o| o.device.clone()).collect();

        while let Some(child) = self.outputs.first_child() {
            self.outputs.remove(&child);
        }
        if mix.outputs.is_empty() {
            let empty = gtk::Label::new(Some("No output. This mix is capture only."));
            empty.add_css_class("dim-label");
            empty.set_margin_top(24);
            self.outputs.append(&empty);
        }
        for (index, output) in mix.outputs.iter().enumerate() {
            let label = devices
                .iter()
                .find(|d| d.name == output.device)
                .map(|d| d.description.clone())
                .unwrap_or_else(|| format!("{} (unavailable)", output.device));
            let row = self.output_row(index, &label, output.state());
            self.outputs.append(&row);
        }

        let popover = self.device_popover(devices);
        self.add_output.set_popover(Some(&popover));
        self.syncing.set(false);
    }

    fn output_row(self: &Rc<Self>, index: usize, label: &str, state: ChainState) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        row.add_css_class("card");

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 4);
        inner.set_margin_top(10);
        inner.set_margin_bottom(10);
        inner.set_margin_start(10);
        inner.set_margin_end(10);
        row.append(&inner);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::new(Some(label));
        title.set_hexpand(true);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        top.append(&title);

        let remove = gtk::Button::from_icon_name("list-remove-symbolic");
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some("Stop sending this mix to this device"));
        remove.connect_clicked({
            let this = self.clone();
            move |_| {
                let devices = this
                    .attached
                    .borrow()
                    .iter()
                    .enumerate()
                    .filter(|(position, _)| *position != index)
                    .map(|(_, device)| device.clone())
                    .collect();
                this.engine.send(Command::SetMixOutputs {
                    id: this.id,
                    devices,
                });
            }
        });
        top.append(&remove);
        inner.append(&top);

        let level = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let mute = gtk::ToggleButton::new();
        mute.set_icon_name("audio-volume-muted-symbolic");
        mute.add_css_class("flat");
        mute.add_css_class("circular");
        mute.set_active(state.muted);
        mute.connect_toggled({
            let this = self.clone();
            move |button| {
                if this.syncing.get() {
                    return;
                }
                this.engine.send(Command::SetOutputMute {
                    id: this.id,
                    index,
                    muted: button.is_active(),
                });
            }
        });
        level.append(&mute);

        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, FADER_MAX, 1.0);
        scale.set_hexpand(true);
        scale.set_draw_value(true);
        scale.set_value_pos(gtk::PositionType::Right);
        scale.set_digits(0);
        scale.set_value(f64::from(state.gain) * FADER_MAX);
        scale.connect_value_changed({
            let this = self.clone();
            move |scale| {
                if this.syncing.get() {
                    return;
                }
                this.engine.send(Command::SetOutputGain {
                    id: this.id,
                    index,
                    gain: (scale.value() / FADER_MAX) as f32,
                });
            }
        });
        level.append(&scale);
        inner.append(&level);

        row.upcast()
    }

    /// The devices this mix does not play to yet.
    fn device_popover(self: &Rc<Self>, devices: &[Device]) -> gtk::Popover {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&list));

        let attached = self.attached.borrow().clone();
        let mut offered = 0;
        for device in devices {
            if attached.contains(&device.name) {
                continue;
            }
            offered += 1;
            let button = gtk::Button::new();
            button.add_css_class("flat");
            button.set_child(Some(
                &gtk::Label::builder()
                    .label(&device.description)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .max_width_chars(32)
                    .build(),
            ));
            button.connect_clicked({
                let this = self.clone();
                let popover = popover.clone();
                let name = device.name.clone();
                move |_| {
                    let mut devices = this.attached.borrow().clone();
                    devices.push(name.clone());
                    this.engine.send(Command::SetMixOutputs {
                        id: this.id,
                        devices,
                    });
                    popover.popdown();
                }
            });
            list.append(&button);
        }
        if offered == 0 {
            let empty = gtk::Label::new(Some("Every device is already attached."));
            empty.add_css_class("dim-label");
            empty.set_margin_top(6);
            empty.set_margin_bottom(6);
            empty.set_margin_start(6);
            empty.set_margin_end(6);
            list.append(&empty);
        }
        popover
    }
}
