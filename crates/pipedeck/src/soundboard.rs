//! The soundboard, in the header bar and, when asked, in a window of its
//! own: its sounds a click from playing, with what adds, renames and
//! removes them.
//!
//! A sound plays into the soundboard's row of the matrix, so who hears it
//! is the cells' business, as for any channel. Clicked again, it starts
//! over.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{Command, DiscordAnswer, SoundConfig, SoundId};

use crate::discord_import::DiscordImport;
use crate::engine_link::EngineLink;
use crate::shortcuts::Shortcuts;

/// Says something in passing, in the mixer's window.
pub type Notify = Rc<dyn Fn(&str)>;

pub struct Soundboard {
    engine: EngineLink,
    button: gtk::MenuButton,
    popover: gtk::Popover,
    /// The soundboard in a window of its own, once it was asked for: kept
    /// when closed, to open again as it was.
    window: RefCell<Option<adw::Window>>,
    /// The sounds as last drawn, so the same list is not drawn again.
    sounds: RefCell<Vec<SoundConfig>>,
    playing: RefCell<Vec<SoundId>>,
    /// Each sound's buttons, in the menu and in the window, to light while
    /// it plays.
    buttons: RefCell<HashMap<SoundId, Vec<gtk::Button>>>,
    /// The desktop's shortcuts for the sounds, where it has them.
    shortcuts: RefCell<Option<Rc<Shortcuts>>>,
    /// Taking sounds from Discord, while its dialog is open.
    import: RefCell<Option<Rc<DiscordImport>>>,
    /// How the mixer's window says something in passing.
    notify: RefCell<Option<Notify>>,
}

impl Soundboard {
    pub fn new(engine: &EngineLink) -> Rc<Self> {
        let button = gtk::MenuButton::new();
        button.set_icon_name("pd-sfx-symbolic");
        button.set_tooltip_text(Some("Soundboard"));
        let popover = gtk::Popover::new();
        button.set_popover(Some(&popover));
        let this = Rc::new(Self {
            engine: engine.clone(),
            button,
            popover,
            window: RefCell::new(None),
            sounds: RefCell::new(Vec::new()),
            playing: RefCell::new(Vec::new()),
            buttons: RefCell::new(HashMap::new()),
            shortcuts: RefCell::new(None),
            import: RefCell::new(None),
            notify: RefCell::new(None),
        });
        this.draw();
        this
    }

    pub fn widget(&self) -> gtk::Widget {
        self.button.clone().upcast()
    }

    /// The desktop's shortcuts, once reached: told of every sound, and a
    /// way to their keys offered.
    pub fn set_shortcuts(self: &Rc<Self>, shortcuts: Rc<Shortcuts>) {
        shortcuts.bind(&self.sounds.borrow());
        *self.shortcuts.borrow_mut() = Some(shortcuts);
        self.draw();
    }

    /// How to say something in passing, in the mixer's window.
    pub fn set_notify(&self, notify: Notify) {
        *self.notify.borrow_mut() = Some(notify);
    }

    /// What Discord answered, for the import under way.
    pub fn discord_answered(&self, answer: DiscordAnswer) {
        let import = self
            .import
            .borrow()
            .clone()
            .filter(|import| import.is_open());
        match import {
            Some(import) => import.answered(answer),
            None => *self.import.borrow_mut() = None,
        }
    }

    /// Open the dialog that takes sounds from Discord's soundboards.
    fn import_from_discord(self: &Rc<Self>, from: &gtk::Widget) {
        self.popover.popdown();
        let parent = from
            .root()
            .map(|root| root.upcast::<gtk::Widget>())
            .unwrap_or_else(|| from.clone());
        let notify = self.notify.borrow().clone();
        let import = DiscordImport::open(&parent, &self.engine, move |count| {
            let said = match count {
                0 => "No sound could be imported from Discord".to_owned(),
                1 => "Imported a sound from Discord".to_owned(),
                n => format!("Imported {n} sounds from Discord"),
            };
            if let Some(notify) = &notify {
                notify(&said);
            }
        });
        *self.import.borrow_mut() = Some(import);
    }

    /// The sounds, as the engine has them.
    pub fn refresh(self: &Rc<Self>, sounds: &[SoundConfig]) {
        if *self.sounds.borrow() == sounds {
            return;
        }
        *self.sounds.borrow_mut() = sounds.to_vec();
        if let Some(shortcuts) = self.shortcuts.borrow().as_ref() {
            shortcuts.bind(sounds);
        }
        self.draw();
    }

    /// Light the sounds playing.
    pub fn set_playing(&self, playing: &[SoundId]) {
        *self.playing.borrow_mut() = playing.to_vec();
        for (id, buttons) in self.buttons.borrow().iter() {
            for button in buttons {
                if playing.contains(id) {
                    button.add_css_class("suggested-action");
                } else {
                    button.remove_css_class("suggested-action");
                }
            }
        }
    }

    /// Draw the menu again, and the window when there is one.
    fn draw(self: &Rc<Self>) {
        let mut buttons: HashMap<SoundId, Vec<gtk::Button>> = HashMap::new();

        let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
        page.set_margin_top(10);
        page.set_margin_bottom(10);
        page.set_margin_start(10);
        page.set_margin_end(10);
        page.set_width_request(480);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let title = gtk::Label::new(Some("Soundboard"));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        top.append(&title);
        for control in self.controls() {
            top.append(&control);
        }
        let detach = gtk::Button::from_icon_name("window-new-symbolic");
        detach.add_css_class("flat");
        detach.set_tooltip_text(Some("Open in a window"));
        detach.connect_clicked({
            let this = self.clone();
            move |_| {
                this.popover.popdown();
                this.open_window();
            }
        });
        top.append(&detach);
        page.append(&top);
        page.append(&self.sounds_area(3, Some(420), &mut buttons));
        if let Some(keys) = self.shortcuts_button() {
            page.append(&keys);
        }
        self.popover.set_child(Some(&page));

        if let Some(window) = self.window.borrow().as_ref() {
            window.set_content(Some(&self.window_content(&mut buttons)));
        }

        *self.buttons.borrow_mut() = buttons;
        let playing = self.playing.borrow().clone();
        self.set_playing(&playing);
    }

    /// What stops the sounds and adds others, in the menu's top line and
    /// the window's header bar.
    fn controls(self: &Rc<Self>) -> [gtk::Widget; 2] {
        let stop = gtk::Button::from_icon_name("media-playback-stop-symbolic");
        stop.add_css_class("flat");
        stop.set_tooltip_text(Some("Stop every sound"));
        stop.connect_clicked({
            let engine = self.engine.clone();
            move |_| engine.send(Command::StopSounds)
        });
        [stop.upcast(), self.add_button().upcast()]
    }

    /// What adds sounds: files, or Discord's.
    fn add_button(self: &Rc<Self>) -> gtk::MenuButton {
        let add = gtk::MenuButton::new();
        add.set_icon_name("list-add-symbolic");
        add.add_css_class("flat");
        add.set_tooltip_text(Some("Add sounds"));
        let choices = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let menu = gtk::Popover::new();
        menu.add_css_class("menu");
        for (label, discord) in [("Sound Files…", false), ("From Discord…", true)] {
            let choice = gtk::Button::with_label(label);
            choice.add_css_class("flat");
            if let Some(text) = choice.child().and_downcast::<gtk::Label>() {
                text.set_xalign(0.0);
            }
            choice.connect_clicked({
                let this = self.clone();
                let menu = menu.clone();
                let add = add.clone();
                move |_| {
                    menu.popdown();
                    if discord {
                        this.import_from_discord(add.upcast_ref());
                    } else {
                        this.pick_sounds(add.upcast_ref());
                    }
                }
            });
            choices.append(&choice);
        }
        menu.set_child(Some(&choices));
        add.set_popover(Some(&menu));
        add
    }

    /// The sounds, as many to a line as `columns` at most, or what to do
    /// without any. A menu stops growing at `height`; a window scrolls.
    fn sounds_area(
        self: &Rc<Self>,
        columns: u32,
        height: Option<i32>,
        buttons: &mut HashMap<SoundId, Vec<gtk::Button>>,
    ) -> gtk::Widget {
        let sounds = self.sounds.borrow().clone();
        if sounds.is_empty() {
            let empty = gtk::Label::new(Some(
                "Add sounds to play them into the mixes: from here, a Stream Deck or a keyboard shortcut.",
            ));
            empty.add_css_class("dim-label");
            empty.set_wrap(true);
            empty.set_xalign(0.0);
            empty.set_max_width_chars(40);
            return empty.upcast();
        }
        let grid = gtk::FlowBox::new();
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_homogeneous(true);
        grid.set_valign(gtk::Align::Start);
        grid.set_min_children_per_line(columns.min(3));
        grid.set_max_children_per_line(columns);
        grid.set_column_spacing(6);
        grid.set_row_spacing(6);
        for sound in &sounds {
            let (tile, play) = self.tile(sound);
            buttons.entry(sound.id).or_default().push(play);
            grid.append(&tile);
        }
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        match height {
            Some(height) => {
                scroller.set_propagate_natural_height(true);
                scroller.set_max_content_height(height);
            }
            None => scroller.set_vexpand(true),
        }
        scroller.set_child(Some(&grid));
        scroller.upcast()
    }

    /// The way to the desktop's settings for the sounds' keys, when it
    /// offers one and there are sounds to give keys.
    fn shortcuts_button(&self) -> Option<gtk::Button> {
        let shortcuts =
            self.shortcuts.borrow().clone().filter(|shortcuts| {
                shortcuts.can_configure() && !self.sounds.borrow().is_empty()
            })?;
        let keys = gtk::Button::with_label("Keyboard Shortcuts…");
        keys.add_css_class("flat");
        keys.set_tooltip_text(Some(
            "Choose keys that play each sound, even with the window in the background",
        ));
        keys.connect_clicked({
            let popover = self.popover.clone();
            move |_| {
                popover.popdown();
                shortcuts.configure();
            }
        });
        Some(keys)
    }

    /// The soundboard in a window of its own, beside the mixer or on
    /// another screen.
    fn open_window(self: &Rc<Self>) {
        if let Some(window) = self.window.borrow().as_ref() {
            window.present();
            return;
        }
        // Not one of the application's windows: the mixer's is the one a
        // second start of Pipedeck, or its icon, brings up.
        let window = adw::Window::builder()
            .title("Soundboard")
            .default_width(560)
            .default_height(380)
            .hide_on_close(true)
            .build();
        *self.window.borrow_mut() = Some(window.clone());
        self.draw();
        window.present();
    }

    fn window_content(
        self: &Rc<Self>,
        buttons: &mut HashMap<SoundId, Vec<gtk::Button>>,
    ) -> gtk::Widget {
        let header = adw::HeaderBar::new();
        for control in self.controls() {
            header.pack_start(&control);
        }
        let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.append(&self.sounds_area(8, None, buttons));
        if let Some(keys) = self.shortcuts_button() {
            keys.set_halign(gtk::Align::Start);
            content.append(&keys);
        }
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&content));
        view.upcast()
    }

    /// One sound: a button that plays it, and a menu to rename or remove it.
    fn tile(self: &Rc<Self>, sound: &SoundConfig) -> (gtk::Widget, gtk::Button) {
        let name = gtk::Label::new(Some(&sound.name));
        // Two lines before a long name is cut, as on a Stream Deck key.
        name.set_wrap(true);
        name.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        name.set_lines(2);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_justify(gtk::Justification::Center);
        name.set_max_width_chars(12);
        let play = gtk::Button::new();
        play.set_child(Some(&name));
        play.set_hexpand(true);
        play.set_tooltip_text(Some(&sound.name));
        play.connect_clicked({
            let engine = self.engine.clone();
            let id = sound.id;
            move |_| engine.send(Command::PlaySound(id))
        });

        let entry = gtk::Entry::new();
        entry.set_text(&sound.name);
        entry.connect_activate({
            let engine = self.engine.clone();
            let id = sound.id;
            move |entry| {
                engine.send(Command::RenameSound {
                    id,
                    name: entry.text().to_string(),
                });
            }
        });
        let remove = gtk::Button::with_label("Remove");
        remove.add_css_class("destructive-action");
        remove.connect_clicked({
            let engine = self.engine.clone();
            let id = sound.id;
            move |_| engine.send(Command::RemoveSound(id))
        });
        let menu_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        menu_page.set_margin_top(6);
        menu_page.set_margin_bottom(6);
        menu_page.set_margin_start(6);
        menu_page.set_margin_end(6);
        let caption = gtk::Label::new(Some("Name"));
        caption.add_css_class("caption-heading");
        caption.set_xalign(0.0);
        menu_page.append(&caption);
        menu_page.append(&entry);
        menu_page.append(&remove);
        let menu_popover = gtk::Popover::new();
        menu_popover.set_child(Some(&menu_page));
        // A name typed and left, rather than confirmed, is taken as well.
        menu_popover.connect_closed({
            let engine = self.engine.clone();
            let id = sound.id;
            let entry = entry.clone();
            let was = sound.name.clone();
            move |_| {
                let now = entry.text().to_string();
                if now.trim() != was && !now.trim().is_empty() {
                    engine.send(Command::RenameSound { id, name: now });
                }
            }
        });
        let menu = gtk::MenuButton::new();
        menu.set_icon_name("view-more-symbolic");
        menu.set_tooltip_text(Some("Rename or remove"));
        menu.set_popover(Some(&menu_popover));

        let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tile.add_css_class("linked");
        tile.append(&play);
        tile.append(&menu);
        (tile.upcast(), play)
    }

    /// Ask for sound files and add each, over the window asked from.
    fn pick_sounds(self: &Rc<Self>, from: &gtk::Widget) {
        self.popover.popdown();
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Sounds"));
        for mime in [
            "audio/mpeg",
            "audio/ogg",
            "audio/x-vorbis+ogg",
            "audio/flac",
            "audio/x-flac",
            "audio/wav",
            "audio/x-wav",
            "audio/x-opus+ogg",
            "audio/opus",
        ] {
            filter.add_mime_type(mime);
        }
        for pattern in ["*.mp3", "*.ogg", "*.oga", "*.opus", "*.flac", "*.wav"] {
            filter.add_pattern(pattern);
        }
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Add Sounds");
        dialog.set_filters(Some(&filters));
        let window = from.root().and_downcast::<gtk::Window>();
        dialog.open_multiple(window.as_ref(), None::<&gtk::gio::Cancellable>, {
            let engine = self.engine.clone();
            move |answer| {
                let Ok(files) = answer else {
                    return;
                };
                for file in files.iter::<gtk::gio::File>().flatten() {
                    if let Some(path) = file.path() {
                        engine.send(Command::AddSound {
                            path,
                            name: None,
                            taken: false,
                        });
                    }
                }
            }
        });
    }
}
