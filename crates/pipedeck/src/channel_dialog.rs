//! The window behind a channel card: its name, its trim, and the mixes it
//! feeds, each with the fader that pair has in the grid.

use std::cell::{Cell as StdCell, RefCell};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::stereotool::Status;
use pipedeck_engine::{vst3::Plugin, App, ChainState, Command, Device, SourceConfig, SourceId};

use crate::desktop::{self, DesktopApp};
use crate::effect_panel::EffectPanel;
use crate::engine_link::EngineLink;
use crate::widgets;

const LEFT_PANE_WIDTH: i32 = 240;

pub struct ChannelDialog {
    dialog: adw::Dialog,
    id: SourceId,
    engine: EngineLink,
    name: gtk::Entry,
    badge: gtk::Image,
    look: gtk::Box,
    volume: gtk::Scale,
    mute: gtk::ToggleButton,
    /// Hidden for an input row, which has no sink to trim.
    trim: gtk::Box,
    /// What an input row captures, named the way the system names it.
    device: gtk::Label,
    /// The trim's fader, with the channel's level drawn in its track.
    fader: widgets::MeterFader,
    /// The level alone, for a channel bound to a microphone, which has no
    /// trim to draw it in.
    meter: widgets::Meter,
    apps: gtk::Box,
    /// The effects tab, which a mix window has too.
    effects: Rc<EffectPanel>,
    add_app: gtk::MenuButton,
    /// Installed applications, read once when the window opens.
    installed: Vec<DesktopApp>,
    /// Set while engine state is pushed into the widgets.
    syncing: Rc<StdCell<bool>>,
    /// Set once the window is gone, so it is never closed twice: the second
    /// time, libadwaita has nothing left to close and says so loudly.
    closed: Rc<StdCell<bool>>,
}

impl ChannelDialog {
    pub fn present(
        parent: &impl IsA<gtk::Widget>,
        engine: &EngineLink,
        source: &SourceConfig,
        inputs: &[Device],
        running: &[App],
        plugins: &[Plugin],
        stereotool: &Status,
    ) -> Rc<Self> {
        let dialog = adw::Dialog::new();
        dialog.set_title("Channel");
        dialog.set_content_width(760);
        dialog.set_content_height(480);

        let fader = widgets::MeterFader::new(source.gain);
        let volume = fader.scale.clone();
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
            badge: widgets::big_badge(
                source.icon.as_deref(),
                source.is_input(),
                widgets::Tone::Colour,
            ),
            look: gtk::Box::new(gtk::Orientation::Vertical, 6),
            trim: widgets::level_row(&mute, &fader.root),
            device,
            volume,
            mute,
            fader,
            meter: widgets::Meter::new(),
            apps: gtk::Box::new(gtk::Orientation::Vertical, 8),
            effects: EffectPanel::new(engine, source.id),
            add_app,
            installed: if source.is_input() {
                Vec::new()
            } else {
                desktop::installed()
            },
            syncing: Rc::new(StdCell::new(false)),
            closed: Rc::new(StdCell::new(false)),
        });

        dialog.set_child(Some(&this.build(source)));
        this.refresh(source, inputs, running, plugins, stereotool);
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

        let menu = widgets::channel_look_menu(source.icon.as_deref(), source.is_input(), {
            let this = self.clone();
            move |icon| {
                this.engine
                    .send(Command::SetSourceIcon { id: this.id, icon })
            }
        });
        self.look.append(&menu);
        self.badge.set_margin_top(6);
        self.look.append(&self.badge);
        left.append(&self.look);

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
            self.meter.root.set_margin_top(6);
            left.append(&self.meter.root);
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

        // Right: what passes through this channel. A row carrying
        // applications has both those and the effects they are heard
        // through, which is why they are tabs rather than one list: the
        // second will not be a list. A row bound to a microphone carries no
        // applications, so its effects stand alone.
        let right = gtk::Box::new(gtk::Orientation::Vertical, 12);
        right.set_hexpand(true);
        if source.is_input() {
            let title = gtk::Label::new(Some("Audio effects"));
            title.add_css_class("heading");
            title.set_xalign(0.0);
            right.append(&title);
            right.append(&self.effects.widget());
        } else {
            // A plain stack switcher rather than libadwaita's: that one
            // pairs every tab with an icon, and these two are named things,
            // not pictures.
            let stack = gtk::Stack::new();
            stack.set_vexpand(true);
            stack.add_titled(&self.apps_page(), Some("apps"), "Apps");
            stack.add_titled(&self.effects.widget(), Some("effects"), "Audio effects");

            let tabs = gtk::StackSwitcher::new();
            tabs.set_stack(Some(&stack));
            tabs.set_halign(gtk::Align::Start);
            right.append(&tabs);
            right.append(&stack);
        }
        panes.append(&right);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&panes));
        view.upcast()
    }

    /// The applications this channel carries, and the way to add one.
    fn apps_page(self: &Rc<Self>) -> gtk::Widget {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.set_margin_top(12);

        let hint = gtk::Label::new(Some(
            "These applications play into this channel, now and the next time they start.",
        ));
        hint.add_css_class("caption");
        hint.add_css_class("dim-label");
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        page.append(&hint);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&self.apps));
        page.append(&scroller);

        self.add_app.set_halign(gtk::Align::Center);
        page.append(&self.add_app);
        page.upcast()
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
        self.dialog.connect_closed({
            let closed = self.closed.clone();
            let effects = self.effects.clone();
            move |_| {
                closed.set(true);
                // Effect windows belong to this one: left open, they would
                // be set by a tab no longer told when the chain changes.
                effects.close_windows();
                rename();
            }
        });

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

    /// Show a level that was set elsewhere, without sending it back.
    pub fn set_state(&self, state: ChainState) {
        self.syncing.set(true);
        self.volume
            .set_value(f64::from(state.gain) * widgets::FADER_MAX);
        self.mute.set_active(state.muted);
        self.syncing.set(false);
    }

    /// Move the meter of the object this window is about.
    pub fn set_level(&self, peak: f32) {
        self.fader.set_level(peak);
        self.meter.set_level(peak);
    }

    pub fn id(&self) -> SourceId {
        self.id
    }

    /// True until the window goes away, whoever closed it.
    pub fn is_open(&self) -> bool {
        !self.closed.get()
    }

    pub fn close(&self) {
        if self.is_open() {
            self.dialog.close();
        }
    }

    /// Push engine state into the window, rebuilding the app list.
    pub fn refresh(
        self: &Rc<Self>,
        source: &SourceConfig,
        inputs: &[Device],
        running: &[App],
        plugins: &[Plugin],
        stereotool: &Status,
    ) {
        self.syncing.set(true);
        self.effects.refresh(&source.effects, plugins, stereotool);
        if self.name.text() != source.name {
            self.name.set_text(&source.name);
        }
        self.volume
            .set_value(f64::from(source.gain) * widgets::FADER_MAX);
        self.mute.set_active(source.muted);
        widgets::set_badge_look(
            &self.badge,
            source.icon.as_deref(),
            source.is_input(),
            widgets::Tone::Colour,
        );
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
            let row = self.app_row(key, running.iter().find(|app| &app.key == key));
            self.apps.append(&row);
        }

        let popover = self.app_popover(source, running);
        self.add_app.set_popover(Some(&popover));
        self.syncing.set(false);
    }

    fn app_row(self: &Rc<Self>, key: &str, running: Option<&App>) -> gtk::Widget {
        let installed = self.installed.iter().find(|app| app.key == key);
        let label = running
            .map(|app| app.name.clone())
            .or_else(|| installed.map(|app| app.name.clone()))
            .unwrap_or_else(|| key.to_owned());
        let icon = running
            .and_then(|app| app.icon.clone())
            .or_else(|| installed.and_then(|app| app.icon.clone()));

        let (card, inner) = widgets::list_card();
        let (top, title) = widgets::card_title(&label);
        title.set_tooltip_text(Some(key));
        top.insert_child_after(
            &widgets::app_icon(icon.as_deref(), 24),
            None::<&gtk::Widget>,
        );

        let state = gtk::Label::new(Some(if running.is_some() {
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

    /// What the picker offers: the applications playing right now, then the
    /// ones the system knows how to launch.
    ///
    /// An application that is not playing cannot be matched against the
    /// graph, so its key comes from its desktop entry and only proves itself
    /// the first time it opens a stream.
    fn app_popover(self: &Rc<Self>, source: &SourceConfig, running: &[App]) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.set_width_request(280);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search"));
        content.append(&search);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_max_content_height(320);
        scroller.set_propagate_natural_height(true);
        scroller.set_child(Some(&list));
        content.append(&scroller);
        popover.set_child(Some(&content));

        // Rows are built once and filtered on search, so typing never
        // rebuilds the list.
        let mut rows: Vec<(String, gtk::Widget)> = Vec::new();

        let held = |key: &str| source.apps.iter().any(|app| app == key);
        let playing: Vec<&App> = running.iter().filter(|app| !held(&app.key)).collect();
        if !playing.is_empty() {
            rows.push((String::new(), section("Playing now")));
            for app in playing {
                rows.push((
                    app.name.to_lowercase(),
                    self.app_choice(&popover, &app.key, &app.name, app.icon.as_deref()),
                ));
            }
        }

        let installed: Vec<&DesktopApp> = self
            .installed
            .iter()
            .filter(|app| !held(&app.key) && !running.iter().any(|r| r.key == app.key))
            .collect();
        if !installed.is_empty() {
            rows.push((String::new(), section("Installed")));
            for app in installed {
                rows.push((
                    app.name.to_lowercase(),
                    self.app_choice(&popover, &app.key, &app.name, app.icon.as_deref()),
                ));
            }
        }

        if rows.is_empty() {
            let empty = gtk::Label::new(Some("Nothing left to add."));
            empty.add_css_class("dim-label");
            empty.set_margin_top(12);
            empty.set_margin_bottom(12);
            list.append(&empty);
        }
        for (_, row) in &rows {
            list.append(row);
        }

        search.connect_search_changed(move |entry| {
            let needle = entry.text().to_lowercase();
            for (haystack, row) in &rows {
                // A section heading has no name of its own; it follows the
                // rows under it, so it hides as soon as a search starts.
                row.set_visible(if haystack.is_empty() {
                    needle.is_empty()
                } else {
                    haystack.contains(&needle)
                });
            }
        });

        popover
    }

    /// One line of the picker.
    fn app_choice(
        self: &Rc<Self>,
        popover: &gtk::Popover,
        key: &str,
        name: &str,
        icon: Option<&str>,
    ) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.append(&widgets::app_icon(icon, 20));
        content.append(
            &gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(28)
                .build(),
        );

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_child(Some(&content));
        button.set_tooltip_text(Some(key));
        button.connect_clicked({
            let this = self.clone();
            let popover = popover.clone();
            let key = key.to_owned();
            move |_| {
                this.engine.send(Command::AssignApp {
                    id: this.id,
                    app: key.clone(),
                });
                popover.popdown();
            }
        });
        button.upcast()
    }
}

/// A heading between two groups of the picker.
fn section(label: &str) -> gtk::Widget {
    let heading = gtk::Label::new(Some(label));
    heading.add_css_class("caption-heading");
    heading.add_css_class("dim-label");
    heading.set_xalign(0.0);
    heading.set_margin_top(6);
    heading.set_margin_start(6);
    heading.upcast()
}
