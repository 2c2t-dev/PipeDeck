//! Main window: the mixer matrix, mixes across the top, sources down the
//! left, one fader per cell.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{
    Device, Event, MixConfig, MixId, SourceConfig, SourceId, StateSnapshot, MAX_MIXES,
};

use crate::cell::{link_button, Cell};
use crate::dialogs;
use crate::engine_link::EngineLink;

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
    cells: RefCell<HashMap<(SourceId, MixId), Cell>>,
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

        let grid = gtk::Grid::new();
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
                mixes: Vec::new(),
                sources: Vec::new(),
                links: Vec::new(),
            }),
            outputs: RefCell::new(Vec::new()),
            inputs: RefCell::new(Vec::new()),
            cells: RefCell::new(HashMap::new()),
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
            }
            Event::Devices { outputs, inputs } => {
                *self.outputs.borrow_mut() = outputs;
                *self.inputs.borrow_mut() = inputs;
            }
            Event::LinkChanged { source, mix, state } => {
                if let Some(cell) = self.cells.borrow().get(&(source, mix)) {
                    cell.set_state(state);
                }
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
                        let root = cell.root.clone().upcast();
                        self.cells.borrow_mut().insert((source.id, mix.id), cell);
                        root
                    }
                    None => link_button(source.id, mix.id, &self.engine).upcast(),
                };
                let holder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                holder.add_css_class("card");
                holder.set_height_request(ROW_HEIGHT);
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

        content.append(&icon_badge("audio-speakers-symbolic"));

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
            let mix = mix.clone();
            move |_| dialogs::edit_mix(&this.window, &this.engine, &mix, &this.outputs.borrow())
        });
        card.set_margin_bottom(6);
        card.upcast()
    }

    fn source_header(self: &Rc<Self>, source: &SourceConfig) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);

        content.append(&icon_badge(if source.is_input() {
            "audio-input-microphone-symbolic"
        } else {
            "audio-speakers-symbolic"
        }));

        let title = gtk::Label::new(Some(&source.name));
        title.add_css_class("heading");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&title);

        let card = clickable_card(&content, SOURCE_COLUMN_WIDTH, ROW_HEIGHT);
        card.set_tooltip_text(Some("Rename or remove this channel"));
        card.connect_clicked({
            let this = self.clone();
            let source = source.clone();
            move |_| dialogs::edit_channel(&this.window, &this.engine, &source)
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

/// An icon in a rounded badge, so a card reads as an object at a glance.
///
/// The badge inverts the window colours, which keeps it readable in both
/// themes without hardcoding a palette.
fn icon_badge(icon_name: &str) -> gtk::Image {
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(BADGE_ICON_SIZE);
    icon.add_css_class("pd-badge");
    // Without this the image stretches to the height of the card and the
    // badge stops being a square.
    icon.set_valign(gtk::Align::Center);
    icon.set_halign(gtk::Align::Center);
    icon
}

/// Install the stylesheet. Call once, after GTK is initialised.
pub fn load_css() {
    const CSS: &str = "
        .pd-card .pd-pencil { opacity: 0; transition: opacity 120ms ease-out; }
        .pd-card:hover .pd-pencil,
        .pd-card:focus-visible .pd-pencil { opacity: 1; }
        .pd-badge {
            background-color: @window_fg_color;
            color: @window_bg_color;
            border-radius: 10px;
            min-width: 32px;
            min-height: 32px;
        }
    ";
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
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
