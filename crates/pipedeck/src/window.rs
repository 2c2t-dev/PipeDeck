//! Main window: a header bar with an "Add source" button and one column per
//! source.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{Command, Event, SourceConfig, SourceId};

use crate::engine_link::EngineLink;
use crate::source_column::SourceColumn;

const PAGE_EMPTY: &str = "empty";
const PAGE_MIXER: &str = "mixer";

pub struct Window {
    pub window: adw::ApplicationWindow,
    engine: EngineLink,
    add_button: gtk::Button,
    stack: gtk::Stack,
    columns_box: gtk::Box,
    toasts: adw::ToastOverlay,
    columns: RefCell<HashMap<SourceId, SourceColumn>>,
}

impl Window {
    pub fn new(app: &adw::Application, engine: EngineLink) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Pipedeck")
            .default_width(720)
            .default_height(480)
            .build();

        let header = adw::HeaderBar::new();
        let add_button = gtk::Button::new();
        add_button.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("Add source")
                .build(),
        ));
        add_button.set_tooltip_text(Some("Create a new virtual source"));
        header.pack_start(&add_button);

        let empty = adw::StatusPage::new();
        empty.set_icon_name(Some("audio-speakers-symbolic"));
        empty.set_title("No source yet");
        empty.set_description(Some(
            "Add a source to get a virtual output you can pick in any application.",
        ));

        let columns_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        columns_box.set_margin_top(12);
        columns_box.set_margin_bottom(12);
        columns_box.set_margin_start(12);
        columns_box.set_margin_end(12);
        columns_box.set_valign(gtk::Align::Fill);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        scroller.set_child(Some(&columns_box));

        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some(PAGE_EMPTY));
        stack.add_named(&scroller, Some(PAGE_MIXER));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        window.set_content(Some(&view));

        let this = Rc::new(Self {
            window,
            engine,
            add_button,
            stack,
            columns_box,
            toasts,
            columns: RefCell::new(HashMap::new()),
        });

        this.add_button.connect_clicked({
            let this = this.clone();
            move |_| this.prompt_add_source()
        });
        this.update_stack();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn handle_event(&self, event: Event) {
        match event {
            Event::Ready { sources } => {
                self.clear_columns();
                for cfg in &sources {
                    self.add_column(cfg);
                }
                self.add_button.set_sensitive(true);
            }
            Event::SourceAdded(cfg) => self.add_column(&cfg),
            Event::SourceRemoved(id) => self.remove_column(id),
            Event::ChainChanged { id, bus, state } => {
                if let Some(column) = self.columns.borrow().get(&id) {
                    column.set_state(bus, &state);
                }
            }
            Event::Error(message) => self.toast(&message),
            Event::Stopped => {
                self.add_button.set_sensitive(false);
                self.toast("Audio engine stopped");
            }
        }
    }

    fn toast(&self, message: &str) {
        log::warn!("{message}");
        self.toasts.add_toast(adw::Toast::new(message));
    }

    fn add_column(&self, cfg: &SourceConfig) {
        let column = SourceColumn::new(cfg, &self.engine);
        self.columns_box.append(&column.root);
        if let Some(old) = self.columns.borrow_mut().insert(cfg.id, column) {
            self.columns_box.remove(&old.root);
        }
        self.update_stack();
    }

    fn remove_column(&self, id: SourceId) {
        if let Some(column) = self.columns.borrow_mut().remove(&id) {
            self.columns_box.remove(&column.root);
        }
        self.update_stack();
    }

    fn clear_columns(&self) {
        for (_, column) in self.columns.borrow_mut().drain() {
            self.columns_box.remove(&column.root);
        }
    }

    fn update_stack(&self) {
        let page = if self.columns.borrow().is_empty() {
            PAGE_EMPTY
        } else {
            PAGE_MIXER
        };
        self.stack.set_visible_child_name(page);
    }

    fn prompt_add_source(&self) {
        let dialog = adw::AlertDialog::new(Some("Add a source"), None);
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("Game, Music, Chat…"));
        entry.set_activates_default(true);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("add", "Add");
        dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("add"));
        dialog.set_close_response("cancel");
        dialog.set_response_enabled("add", false);

        entry.connect_changed({
            let dialog = dialog.clone();
            move |entry| dialog.set_response_enabled("add", !entry.text().trim().is_empty())
        });
        dialog.connect_response(Some("add"), {
            let engine = self.engine.clone();
            move |_, _| {
                let name = entry.text().trim().to_owned();
                if !name.is_empty() {
                    engine.send(Command::AddSource { name });
                }
            }
        });
        dialog.present(Some(&self.window));
    }
}
