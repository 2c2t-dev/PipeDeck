//! Main window: the mixer matrix, mixes across the top, sources down the
//! left, one fader per cell.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{
    App, Device, Event, MixConfig, MixId, SourceConfig, SourceId, StateSnapshot, MAX_MIXES,
};

use crate::cell::{link_button, Cell};
use crate::channel_dialog::ChannelDialog;
use crate::dialogs;
use crate::engine_link::EngineLink;
use crate::mix_dialog::MixDialog;
use crate::preferences;
use crate::presets;
use crate::widgets;

const MIX_COLUMN_WIDTH: i32 = 240;
const SOURCE_COLUMN_WIDTH: i32 = 180;
const ROW_HEIGHT: i32 = 56;
const MIX_HEADER_HEIGHT: i32 = 72;
const BADGE_ICON_SIZE: i32 = 16;
const ADD_MIX_WIDTH: i32 = 56;

pub struct Window {
    pub window: adw::ApplicationWindow,
    engine: EngineLink,
    grid: gtk::Grid,
    hint: gtk::Label,
    toasts: adw::ToastOverlay,
    /// Set once the engine is gone, so a rebuild keeps the add buttons off.
    stopped: StdCell<bool>,
    state: RefCell<StateSnapshot>,
    outputs: RefCell<Vec<Device>>,
    inputs: RefCell<Vec<Device>>,
    /// Applications currently playing, for the channel windows.
    apps: RefCell<Vec<App>>,
    /// Last peak of every channel, so a cell can draw what it passes on.
    channel_levels: RefCell<HashMap<SourceId, f32>>,
    cells: RefCell<HashMap<(SourceId, MixId), Cell>>,
    /// The object windows, while they are open, so engine changes reach them.
    mix_dialog: RefCell<Option<Rc<MixDialog>>>,
    channel_dialog: RefCell<Option<Rc<ChannelDialog>>>,
}

impl Window {
    pub fn new(app: &adw::Application, engine: EngineLink) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Pipedeck")
            .default_width(1080)
            .default_height(520)
            .build();

        let header = adw::HeaderBar::new();
        let settings = gtk::Button::from_icon_name("preferences-system-symbolic");
        settings.set_tooltip_text(Some("Settings"));
        header.pack_end(&settings);

        let grid = gtk::Grid::new();
        // Cards keep their size in a wide window instead of stretching: a
        // fader is easier to aim at when it does not change length with the
        // window, and a row of them stays readable.
        grid.set_halign(gtk::Align::Start);
        grid.set_valign(gtk::Align::Start);
        grid.set_row_spacing(6);
        grid.set_column_spacing(6);
        grid.set_margin_top(12);
        grid.set_margin_bottom(12);
        grid.set_margin_start(12);
        grid.set_margin_end(12);

        let hint = gtk::Label::new(None);
        hint.add_css_class("dim-label");
        hint.set_margin_bottom(24);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&grid);
        content.append(&hint);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&content));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&scroller));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        window.set_content(Some(&view));

        let this = Rc::new(Self {
            window,
            engine,
            grid,
            hint,
            toasts,
            stopped: StdCell::new(false),
            state: RefCell::new(StateSnapshot {
                latency: String::new(),
                mixes: Vec::new(),
                sources: Vec::new(),
                links: Vec::new(),
            }),
            outputs: RefCell::new(Vec::new()),
            inputs: RefCell::new(Vec::new()),
            apps: RefCell::new(Vec::new()),
            channel_levels: RefCell::new(HashMap::new()),
            cells: RefCell::new(HashMap::new()),
            mix_dialog: RefCell::new(None),
            channel_dialog: RefCell::new(None),
        });

        settings.connect_clicked({
            let this = this.clone();
            move |_| {
                let latency = this.state.borrow().latency.clone();
                preferences::present(&this.window, &this.engine, &latency)
            }
        });

        this.rebuild();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn handle_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::State(state) => {
                *self.state.borrow_mut() = state;
                self.rebuild();
                self.refresh_dialogs();
            }
            Event::Apps { running } => {
                *self.apps.borrow_mut() = running;
                self.refresh_dialogs();
            }
            Event::Devices { outputs, inputs } => {
                *self.outputs.borrow_mut() = outputs;
                *self.inputs.borrow_mut() = inputs;
                self.refresh_dialogs();
            }
            Event::Levels { sources, mixes } => self.draw_levels(&sources, &mixes),
            Event::LinkChanged { source, mix, state } => {
                if let Some(cell) = self.cells.borrow().get(&(source, mix)) {
                    cell.set_state(state);
                }
                if let Some(link) = self
                    .state
                    .borrow_mut()
                    .links
                    .iter_mut()
                    .find(|l| l.source == source && l.mix == mix)
                {
                    link.set_state(state);
                }
            }
            Event::MixChanged { id, state } => {
                // A level can move outside the mixer: a media key, a volume
                // applet, anything holding the same sink.
                if let Some(mix) = self
                    .state
                    .borrow_mut()
                    .mixes
                    .iter_mut()
                    .find(|m| m.id == id)
                {
                    mix.set_state(state);
                }
                if let Some(dialog) = self.mix_dialog.borrow().as_ref() {
                    if dialog.id() == id {
                        dialog.set_state(state);
                    }
                }
            }
            Event::SourceChanged { id, state } => {
                if let Some(source) = self
                    .state
                    .borrow_mut()
                    .sources
                    .iter_mut()
                    .find(|s| s.id == id)
                {
                    source.set_state(state);
                }
                if let Some(dialog) = self.channel_dialog.borrow().as_ref() {
                    if dialog.id() == id {
                        dialog.set_state(state);
                    }
                }
            }
            Event::OutputChanged { .. } => {
                // The window that moved it already shows the new value.
            }
            Event::Error(message) => self.toast(&message),
            Event::Stopped => {
                self.stopped.set(true);
                self.rebuild();
                self.toast("Audio engine stopped");
            }
        }
    }

    fn toast(&self, message: &str) {
        log::warn!("{message}");
        self.toasts.add_toast(adw::Toast::new(message));
    }

    /// Rebuild the whole matrix. Structural changes are rare and the grid is
    /// small, so this is simpler and safer than patching it in place.
    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        self.cells.borrow_mut().clear();

        let state = self.state.borrow();

        self.grid.attach(&corner(), 0, 0, 1, 1);
        for (column, mix) in state.mixes.iter().enumerate() {
            let header = self.mix_header(mix);
            self.grid.attach(&header, column as i32 + 1, 0, 1, 1);
        }
        // The two add buttons continue the grid: a new column on the right of
        // the last mix, a new row under the last source.
        if state.mixes.len() < MAX_MIXES {
            let add = self.add_mix_button();
            self.grid
                .attach(&add, state.mixes.len() as i32 + 1, 0, 1, 1);
        }
        let add = self.add_source_button();
        self.grid
            .attach(&add, 0, state.sources.len() as i32 + 1, 1, 1);

        for (row, source) in state.sources.iter().enumerate() {
            let header = self.source_header(source);
            self.grid.attach(&header, 0, row as i32 + 1, 1, 1);

            for (column, mix) in state.mixes.iter().enumerate() {
                let linked = state
                    .links
                    .iter()
                    .find(|l| l.source == source.id && l.mix == mix.id);
                let widget: gtk::Widget = match linked {
                    Some(link) => {
                        let cell = Cell::new(source.id, mix.id, link.state(), &self.engine);
                        if let Some(peak) = self.channel_levels.borrow().get(&source.id) {
                            cell.set_level(*peak);
                        }
                        let root = cell.root.clone().upcast();
                        self.cells.borrow_mut().insert((source.id, mix.id), cell);
                        root
                    }
                    None => link_button(source.id, mix.id, &self.engine).upcast(),
                };
                let holder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                holder.add_css_class("card");
                holder.set_height_request(ROW_HEIGHT);
                holder.set_size_request(MIX_COLUMN_WIDTH, ROW_HEIGHT);
                holder.set_hexpand(false);
                holder.append(&widget);
                widget.set_hexpand(true);
                self.grid
                    .attach(&holder, column as i32 + 1, row as i32 + 1, 1, 1);
            }
        }

        self.hint.set_visible(state.sources.is_empty());
        self.hint
            .set_label("Add a source to get a virtual output, then press + to send it to a mix.");
    }

    /// Move every meter: the cells of a channel, and the windows that are
    /// open on the objects concerned.
    fn draw_levels(self: &Rc<Self>, sources: &[(SourceId, f32)], mixes: &[(MixId, f32)]) {
        let mut levels = self.channel_levels.borrow_mut();
        for (id, peak) in sources {
            levels.insert(*id, *peak);
        }
        drop(levels);

        let cells = self.cells.borrow();
        for ((source, _), cell) in cells.iter() {
            if let Some((_, peak)) = sources.iter().find(|(id, _)| id == source) {
                cell.set_level(*peak);
            }
        }
        drop(cells);

        if let Some(dialog) = self.channel_dialog.borrow().as_ref() {
            if let Some((_, peak)) = sources.iter().find(|(id, _)| *id == dialog.id()) {
                dialog.set_level(*peak);
            }
        }
        if let Some(dialog) = self.mix_dialog.borrow().as_ref() {
            if let Some((_, peak)) = mixes.iter().find(|(id, _)| *id == dialog.id()) {
                dialog.set_level(*peak);
            }
        }
    }

    /// Open the window of one mix, replacing whichever was open.
    fn open_mix_dialog(self: &Rc<Self>, id: MixId) {
        let state = self.state.borrow();
        let Some(mix) = state.mixes.iter().find(|m| m.id == id) else {
            return;
        };
        if let Some(open) = self.mix_dialog.borrow_mut().take() {
            open.close();
        }
        let dialog = MixDialog::present(&self.window, &self.engine, mix, &self.outputs.borrow());
        *self.mix_dialog.borrow_mut() = Some(dialog);
    }

    /// Open the window of one channel, replacing whichever was open.
    fn open_channel_dialog(self: &Rc<Self>, id: SourceId) {
        let state = self.state.borrow();
        let Some(source) = state.sources.iter().find(|s| s.id == id) else {
            return;
        };
        if let Some(open) = self.channel_dialog.borrow_mut().take() {
            open.close();
        }
        let dialog = ChannelDialog::present(
            &self.window,
            &self.engine,
            source,
            &self.inputs.borrow(),
            &self.apps.borrow(),
        );
        drop(state);
        *self.channel_dialog.borrow_mut() = Some(dialog);
    }

    /// Keep the open windows in step with the engine, and close one whose
    /// object is gone.
    fn refresh_dialogs(self: &Rc<Self>) {
        // A window the user closed is forgotten rather than refreshed.
        self.mix_dialog.borrow_mut().take_if(|d| !d.is_open());
        self.channel_dialog.borrow_mut().take_if(|d| !d.is_open());

        let open_mix = self.mix_dialog.borrow().clone();
        if let Some(dialog) = open_mix {
            let state = self.state.borrow();
            match state.mixes.iter().find(|m| m.id == dialog.id()) {
                Some(mix) => dialog.refresh(mix, &self.outputs.borrow()),
                None => {
                    drop(state);
                    dialog.close();
                    *self.mix_dialog.borrow_mut() = None;
                }
            }
        }
        let open_channel = self.channel_dialog.borrow().clone();
        if let Some(dialog) = open_channel {
            let state = self.state.borrow();
            match state.sources.iter().find(|s| s.id == dialog.id()) {
                Some(source) => dialog.refresh(source, &self.inputs.borrow(), &self.apps.borrow()),
                None => {
                    drop(state);
                    dialog.close();
                    *self.channel_dialog.borrow_mut() = None;
                }
            }
        }
    }

    fn add_mix_button(self: &Rc<Self>) -> gtk::Widget {
        // Icon only: this one sits in the header row next to named mixes, so
        // it stays out of the way until you look for it.
        let button = gtk::Button::from_icon_name("list-add-symbolic");
        button.set_tooltip_text(Some("Add a mix"));
        button.add_css_class("flat");
        button.add_css_class("card");
        button.set_width_request(ADD_MIX_WIDTH);
        button.set_margin_bottom(6);
        button.set_sensitive(!self.stopped.get());
        button.connect_clicked({
            let this = self.clone();
            move |_| dialogs::add_mix(&this.window, &this.engine)
        });
        button.upcast()
    }

    fn add_source_button(self: &Rc<Self>) -> gtk::Widget {
        let button = gtk::Button::new();
        button.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("Create channel")
                .build(),
        ));
        button.set_tooltip_text(Some(
            "Create a channel: a virtual output, or a capture device",
        ));
        button.add_css_class("flat");
        button.add_css_class("card");
        button.set_width_request(SOURCE_COLUMN_WIDTH);
        button.set_height_request(ROW_HEIGHT);
        button.set_sensitive(!self.stopped.get());
        button.connect_clicked({
            let this = self.clone();
            move |_| dialogs::add_source(&this.window, &this.engine, &this.inputs.borrow())
        });
        button.upcast()
    }

    fn mix_header(self: &Rc<Self>, mix: &MixConfig) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);

        content.append(&widgets::badge(
            mix.icon.as_deref(),
            false,
            widgets::Tone::White,
            BADGE_ICON_SIZE,
        ));

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(Some(&mix.name));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&title);
        let subtitle = gtk::Label::new(Some(&output_label(mix.outputs.len())));
        subtitle.add_css_class("caption");
        subtitle.add_css_class("dim-label");
        subtitle.set_xalign(0.0);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&subtitle);
        content.append(&labels);

        let card = clickable_card(&content, MIX_COLUMN_WIDTH, MIX_HEADER_HEIGHT);
        card.set_tooltip_text(Some("Rename this mix, choose its outputs, or remove it"));
        card.connect_clicked({
            let this = self.clone();
            let id = mix.id;
            move |_| this.open_mix_dialog(id)
        });
        card.set_margin_bottom(6);
        card.upcast()
    }

    fn source_header(self: &Rc<Self>, source: &SourceConfig) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);

        content.append(&widgets::badge(
            source.icon.as_deref(),
            source.is_input(),
            widgets::Tone::Colour,
            BADGE_ICON_SIZE,
        ));

        let title = gtk::Label::new(Some(&source.name));
        title.add_css_class("heading");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&title);

        let card = clickable_card(&content, SOURCE_COLUMN_WIDTH, ROW_HEIGHT);
        card.set_tooltip_text(Some("Rename this channel, set its levels, or remove it"));
        card.connect_clicked({
            let this = self.clone();
            let id = source.id;
            move |_| this.open_channel_dialog(id)
        });
        card.upcast()
    }
}

/// A card whose whole surface acts as a button, showing a pencil on hover to
/// say so without spending room on a permanent button.
///
/// The reveal is left to the stylesheet in [`load_css`], so it also covers
/// keyboard focus and needs no event plumbing.
fn clickable_card(content: &gtk::Box, width: i32, height: i32) -> gtk::Button {
    let pencil = gtk::Image::from_icon_name("document-edit-symbolic");
    pencil.add_css_class("pd-pencil");
    // Always in the layout, so revealing it never shifts the text.
    content.append(&pencil);

    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(10);
    content.set_margin_end(10);

    let card = gtk::Button::new();
    card.add_css_class("card");
    card.add_css_class("flat");
    card.add_css_class("pd-card");
    card.set_child(Some(content));
    card.set_width_request(width);
    card.set_height_request(height);
    card
}

/// Install the stylesheet. Call once, after GTK is initialised.
pub fn load_css() {
    const CSS: &str = "
        .pd-card .pd-pencil { opacity: 0; transition: opacity 120ms ease-out; }
        .pd-card:hover .pd-pencil,
        .pd-card:focus-visible .pd-pencil { opacity: 1; }
        .pd-badge-large {
            background-color: @window_fg_color;
            color: @window_bg_color;
            border-radius: 28px;
            min-width: 148px;
            min-height: 148px;
        }
        .pd-badge {
            background-color: @window_fg_color;
            color: @window_bg_color;
            border-radius: 10px;
            min-width: 32px;
            min-height: 32px;
        }
    ";
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!("{CSS}{}", presets::css()));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn corner() -> gtk::Widget {
    let corner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    corner.set_width_request(SOURCE_COLUMN_WIDTH);
    corner.upcast()
}

fn output_label(count: usize) -> String {
    match count {
        0 => "No output".to_owned(),
        1 => "1 output".to_owned(),
        n => format!("{n} outputs"),
    }
}
