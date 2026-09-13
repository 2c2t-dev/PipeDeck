//! Main window: the mixer matrix, mixes across the top, sources down the
//! left, one fader per cell.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{
    Command, Device, Event, MixConfig, MixId, SourceConfig, SourceId, StateSnapshot, MAX_MIXES,
};

use crate::cell::{link_button, Cell};
use crate::dialogs;
use crate::engine_link::EngineLink;

const MIX_COLUMN_WIDTH: i32 = 240;
const SOURCE_COLUMN_WIDTH: i32 = 180;
const ROW_HEIGHT: i32 = 56;
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
                .label("Source")
                .build(),
        ));
        button.set_tooltip_text(Some("Add a source"));
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
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("card");
        root.set_width_request(MIX_COLUMN_WIDTH);
        root.set_margin_bottom(6);

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 4);
        inner.set_margin_top(10);
        inner.set_margin_bottom(10);
        inner.set_margin_start(10);
        inner.set_margin_end(10);
        root.append(&inner);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::new(Some(&mix.name));
        title.add_css_class("heading");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        top.append(&title);

        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some("Remove this mix"));
        remove.connect_clicked({
            let this = self.clone();
            let id = mix.id;
            let name = mix.name.clone();
            move |_| {
                dialogs::confirm_remove(
                    &this.window,
                    &this.engine,
                    &format!("Remove {name}?"),
                    "Its outputs and every fader on this mix are removed.",
                    Command::RemoveMix(id),
                )
            }
        });
        top.append(&remove);
        inner.append(&top);

        let devices = gtk::Button::new();
        devices.add_css_class("flat");
        devices.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("audio-speakers-symbolic")
                .label(output_label(mix.outputs.len()))
                .build(),
        ));
        devices.set_tooltip_text(Some("Choose the devices this mix plays to"));
        devices.connect_clicked({
            let this = self.clone();
            let mix = mix.clone();
            move |_| dialogs::mix_outputs(&this.window, &this.engine, &mix, &this.outputs.borrow())
        });
        inner.append(&devices);

        root.upcast()
    }

    fn source_header(self: &Rc<Self>, source: &SourceConfig) -> gtk::Widget {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        root.add_css_class("card");
        root.set_width_request(SOURCE_COLUMN_WIDTH);
        root.set_height_request(ROW_HEIGHT);

        let icon = gtk::Image::from_icon_name(if source.is_input() {
            "audio-input-microphone-symbolic"
        } else {
            "audio-speakers-symbolic"
        });
        icon.set_margin_start(10);
        root.append(&icon);

        let title = gtk::Label::new(Some(&source.name));
        title.add_css_class("heading");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_tooltip_text(Some(&source.name));
        root.append(&title);

        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("flat");
        remove.set_margin_end(6);
        remove.set_tooltip_text(Some("Remove this source"));
        remove.connect_clicked({
            let this = self.clone();
            let id = source.id;
            let name = source.name.clone();
            move |_| {
                dialogs::confirm_remove(
                    &this.window,
                    &this.engine,
                    &format!("Remove {name}?"),
                    "Applications sending audio to it lose their output.",
                    Command::RemoveSource(id),
                )
            }
        });
        root.append(&remove);

        root.upcast()
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
