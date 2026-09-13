//! The window behind a channel card: its name, its trim, and the mixes it
//! feeds, each with the fader that pair has in the grid.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{App, Command, Device, SourceConfig, SourceId};

use crate::engine_link::EngineLink;
use crate::widgets;

const LEFT_PANE_WIDTH: i32 = 240;

pub struct ChannelDialog {
    dialog: adw::Dialog,
    id: SourceId,
    engine: EngineLink,
    name: gtk::Entry,
    volume: gtk::Scale,
    mute: gtk::ToggleButton,
    /// Hidden for an input row, which has no sink to trim.
    trim: gtk::Box,
    /// What an input row captures, named the way the system names it.
    device: gtk::Label,
    apps: gtk::Box,
    add_app: gtk::MenuButton,
    /// Set while engine state is pushed into the widgets.
    syncing: Rc<StdCell<bool>>,
}

impl ChannelDialog {
    pub fn present(
        parent: &impl IsA<gtk::Widget>,
        engine: &EngineLink,
        source: &SourceConfig,
        inputs: &[Device],
        running: &[App],
    ) -> Rc<Self> {
        let dialog = adw::Dialog::new();
        dialog.set_title("Channel");
        dialog.set_content_width(760);
        dialog.set_content_height(480);

        let volume = widgets::fader(source.gain);
        let mute = widgets::mute_button(source.muted, "Mute this channel everywhere");

        let add_app = gtk::MenuButton::new();
        add_app.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("Add app")
                .build(),
        ));
        add_app.set_tooltip_text(Some("Send an application's audio to this channel"));

        let device = gtk::Label::new(None);
        device.add_css_class("caption");
        device.set_wrap(true);
        device.set_max_width_chars(26);
        device.set_justify(gtk::Justification::Center);

        let this = Rc::new(Self {
            dialog: dialog.clone(),
            id: source.id,
            engine: engine.clone(),
            name: widgets::name_entry(&source.name),
            trim: widgets::level_row(&mute, &volume),
            device,
            volume,
            mute,
            apps: gtk::Box::new(gtk::Orientation::Vertical, 8),
            add_app,
            syncing: Rc::new(StdCell::new(false)),
        });

        dialog.set_child(Some(&this.build(source)));
        this.refresh(source, inputs, running);
        this.connect(source);
        dialog.present(Some(parent));
        this
    }

    fn build(self: &Rc<Self>, source: &SourceConfig) -> gtk::Widget {
        let header = adw::HeaderBar::new();

        let panes = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        panes.set_margin_top(18);
        panes.set_margin_bottom(18);
        panes.set_margin_start(18);
        panes.set_margin_end(18);

        // Left: identity and trim.
        let left = gtk::Box::new(gtk::Orientation::Vertical, 12);
        left.set_width_request(LEFT_PANE_WIDTH);
        left.append(&self.name);

        let badge = widgets::big_badge(if source.is_input() {
            "audio-input-microphone-symbolic"
        } else {
            "audio-speakers-symbolic"
        });
        badge.set_margin_top(12);
        left.append(&badge);

        let caption = gtk::Label::new(Some(if source.is_input() {
            "Captured device"
        } else {
            "Channel volume"
        }));
        caption.add_css_class("dim-label");
        caption.set_margin_top(12);
        left.append(&caption);

        // An input row owns no node to carry a trim, so the left pane says
        // what it listens to instead of offering a fader.
        if source.is_input() {
            left.append(&self.device);
        } else {
            left.append(&self.trim);
        }

        let filler = gtk::Box::new(gtk::Orientation::Vertical, 0);
        filler.set_vexpand(true);
        left.append(&filler);

        let delete = gtk::Button::new();
        delete.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("user-trash-symbolic")
                .label("Remove channel")
                .build(),
        ));
        delete.add_css_class("destructive-action");
        delete.set_halign(gtk::Align::Start);
        delete.connect_clicked({
            let this = self.clone();
            move |_| {
                this.engine.send(Command::RemoveSource(this.id));
                this.dialog.close();
            }
        });
        left.append(&delete);
        panes.append(&left);

        // Right: the applications this channel carries. An input row has
        // none: it takes its audio from a device, not from applications.
        if !source.is_input() {
            let right = gtk::Box::new(gtk::Orientation::Vertical, 12);
            right.set_hexpand(true);

            let title = gtk::Label::new(Some("Apps"));
            title.add_css_class("heading");
            title.set_xalign(0.0);
            right.append(&title);

            let hint = gtk::Label::new(Some(
                "These applications play into this channel, now and the next time they start.",
            ));
            hint.add_css_class("caption");
            hint.add_css_class("dim-label");
            hint.set_xalign(0.0);
            hint.set_wrap(true);
            right.append(&hint);

            let scroller = gtk::ScrolledWindow::new();
            scroller.set_vexpand(true);
            scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroller.set_child(Some(&self.apps));
            right.append(&scroller);

            self.add_app.set_halign(gtk::Align::Center);
            right.append(&self.add_app);
            panes.append(&right);
        }

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&panes));
        view.upcast()
    }

    fn connect(self: &Rc<Self>, source: &SourceConfig) {
        let previous = Rc::new(RefCell::new(source.name.clone()));

        // Renaming lands when the field is validated or the window closes,
        // rather than on every keystroke.
        let rename = {
            let this = self.clone();
            let previous = previous.clone();
            move || {
                let chosen = this.name.text().trim().to_owned();
                if !chosen.is_empty() && chosen != *previous.borrow() {
                    *previous.borrow_mut() = chosen.clone();
                    this.engine.send(Command::RenameSource {
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
                this.engine.send(Command::SetSourceGain {
                    id: this.id,
                    gain: (scale.value() / widgets::FADER_MAX) as f32,
                });
            }
        });
        self.mute.connect_toggled({
            let this = self.clone();
            move |button| {
                if this.syncing.get() {
                    return;
                }
                this.engine.send(Command::SetSourceMute {
                    id: this.id,
                    muted: button.is_active(),
                });
            }
        });
    }

    pub fn id(&self) -> SourceId {
        self.id
    }

    pub fn close(&self) {
        self.dialog.close();
    }

    /// Push engine state into the window, rebuilding the app list.
    pub fn refresh(self: &Rc<Self>, source: &SourceConfig, inputs: &[Device], running: &[App]) {
        self.syncing.set(true);
        if self.name.text() != source.name {
            self.name.set_text(&source.name);
        }
        self.volume
            .set_value(f64::from(source.gain) * widgets::FADER_MAX);
        self.mute.set_active(source.muted);
        if let Some(device) = &source.device {
            let described = inputs
                .iter()
                .find(|d| &d.name == device)
                .map(|d| d.description.clone())
                .unwrap_or_else(|| format!("{device} (unavailable)"));
            self.device.set_label(&described);
            self.device.set_tooltip_text(Some(device));
            self.syncing.set(false);
            return;
        }

        while let Some(child) = self.apps.first_child() {
            self.apps.remove(&child);
        }
        if source.apps.is_empty() {
            let empty = gtk::Label::new(Some("No application sends its audio here yet."));
            empty.add_css_class("dim-label");
            empty.set_margin_top(24);
            self.apps.append(&empty);
        }
        for key in &source.apps {
            // An assigned application that is not playing right now is still
            // listed, since the assignment is what outlives the stream.
            let name = running
                .iter()
                .find(|app| &app.key == key)
                .map(|app| app.name.clone());
            let row = self.app_row(key, name);
            self.apps.append(&row);
        }

        let popover = self.app_popover(source, running);
        self.add_app.set_popover(Some(&popover));
        self.syncing.set(false);
    }

    fn app_row(self: &Rc<Self>, key: &str, running_as: Option<String>) -> gtk::Widget {
        let (card, inner) = widgets::list_card();
        let (top, title) = widgets::card_title(running_as.as_deref().unwrap_or(key));
        title.set_tooltip_text(Some(key));

        let state = gtk::Label::new(Some(if running_as.is_some() {
            "Playing"
        } else {
            "Not running"
        }));
        state.add_css_class("caption");
        state.add_css_class("dim-label");
        top.append(&state);

        let remove = gtk::Button::from_icon_name("list-remove-symbolic");
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some("Hand this application back to the system"));
        remove.connect_clicked({
            let this = self.clone();
            let key = key.to_owned();
            move |_| {
                this.engine.send(Command::ReleaseApp {
                    id: this.id,
                    app: key.clone(),
                })
            }
        });
        top.append(&remove);
        inner.append(&top);

        card.upcast()
    }

    /// The applications playing right now that this channel does not hold.
    fn app_popover(self: &Rc<Self>, source: &SourceConfig, running: &[App]) -> gtk::Popover {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&list));

        let mut offered = 0;
        for app in running {
            if source.apps.contains(&app.key) {
                continue;
            }
            offered += 1;
            let button = gtk::Button::new();
            button.add_css_class("flat");
            button.set_child(Some(
                &gtk::Label::builder()
                    .label(&app.name)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .max_width_chars(32)
                    .build(),
            ));
            button.connect_clicked({
                let this = self.clone();
                let popover = popover.clone();
                let key = app.key.clone();
                move |_| {
                    this.engine.send(Command::AssignApp {
                        id: this.id,
                        app: key.clone(),
                    });
                    popover.popdown();
                }
            });
            list.append(&button);
        }
        if offered == 0 {
            let empty = gtk::Label::new(Some("No other application is playing."));
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
