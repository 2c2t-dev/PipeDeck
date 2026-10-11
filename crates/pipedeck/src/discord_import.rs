//! Taking sounds from Discord's soundboards onto Pipedeck's: the servers
//! the user is in, a server's sounds to tick, and the ones ticked fetched.
//!
//! Discord is asked through Vesktop's plugin, by way of the engine and the
//! control socket, and its answers come back as engine events: each page
//! waits for its own, and says so when none comes.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{Command, DiscordAnswer, DiscordGuild, DiscordQuery, DiscordSound};

use crate::engine_link::EngineLink;

/// How long Discord is waited on before the page says it did not answer.
const PATIENCE: Duration = Duration::from_secs(15);

/// What a page is waiting for.
#[derive(Debug, Clone, PartialEq)]
enum Waiting {
    Nothing,
    Guilds,
    Sounds(String),
    Fetch,
}

pub struct DiscordImport {
    engine: EngineLink,
    dialog: adw::Dialog,
    nav: adw::NavigationView,
    guilds: gtk::Box,
    /// The page of the server being looked at, and its sounds as ticked.
    sounds_page: RefCell<Option<SoundsPage>>,
    waiting: RefCell<Waiting>,
    /// Counts the questions, so a timeout only speaks for its own.
    asked: std::cell::Cell<u32>,
    /// Told how many sounds were taken, once they were.
    on_imported: Box<dyn Fn(usize)>,
}

struct SoundsPage {
    body: gtk::Box,
    import: gtk::Button,
    ticked: Rc<RefCell<Vec<DiscordSound>>>,
}

impl DiscordImport {
    /// Open the dialog over `parent`, and ask for the servers.
    pub fn open(
        parent: &impl IsA<gtk::Widget>,
        engine: &EngineLink,
        on_imported: impl Fn(usize) + 'static,
    ) -> Rc<Self> {
        let dialog = adw::Dialog::new();
        dialog.set_title("Import from Discord");
        dialog.set_content_width(420);
        dialog.set_content_height(560);
        let nav = adw::NavigationView::new();

        let guilds = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = adw::HeaderBar::new();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&scrolled(&guilds)));
        nav.add(&adw::NavigationPage::new(&view, "Import from Discord"));
        dialog.set_child(Some(&nav));

        let this = Rc::new(Self {
            engine: engine.clone(),
            dialog,
            nav,
            guilds,
            sounds_page: RefCell::new(None),
            waiting: RefCell::new(Waiting::Nothing),
            asked: std::cell::Cell::new(0),
            on_imported: Box::new(on_imported),
        });
        this.dialog.present(Some(parent));
        this.ask(DiscordQuery::Guilds, Waiting::Guilds);
        this
    }

    pub fn is_open(&self) -> bool {
        self.dialog.parent().is_some()
    }

    /// Ask Discord, show that it is asked, and give up on it after a while.
    fn ask(self: &Rc<Self>, query: DiscordQuery, waiting: Waiting) {
        let shown = match &waiting {
            Waiting::Guilds => Some(self.guilds.clone()),
            Waiting::Sounds(_) | Waiting::Fetch => self
                .sounds_page
                .borrow()
                .as_ref()
                .map(|page| page.body.clone()),
            Waiting::Nothing => None,
        };
        if let Some(body) = &shown {
            let message = if waiting == Waiting::Fetch {
                "Fetching the sounds…"
            } else {
                "Asking Discord…"
            };
            fill(body, &status(message, true));
        }
        *self.waiting.borrow_mut() = waiting;
        let asked = self.asked.get() + 1;
        self.asked.set(asked);
        self.engine.send(Command::AskDiscord(query));
        let this = Rc::downgrade(self);
        gtk::glib::timeout_add_local_once(PATIENCE, move || {
            let Some(this) = this.upgrade() else {
                return;
            };
            if this.asked.get() == asked && *this.waiting.borrow() != Waiting::Nothing {
                this.answered(DiscordAnswer::Error(
                    "Discord did not answer. Is Vesktop running, with Pipedeck's plugin on?".into(),
                ));
            }
        });
    }

    /// Take Discord's answer to whatever is waited for.
    pub fn answered(self: &Rc<Self>, answer: DiscordAnswer) {
        let waiting = self.waiting.borrow().clone();
        match (answer, waiting) {
            (DiscordAnswer::Guilds(guilds), Waiting::Guilds) => {
                *self.waiting.borrow_mut() = Waiting::Nothing;
                self.show_guilds(&guilds);
            }
            (DiscordAnswer::Sounds { guild, sounds }, Waiting::Sounds(asked)) if guild == asked => {
                *self.waiting.borrow_mut() = Waiting::Nothing;
                self.show_sounds(&sounds);
            }
            (DiscordAnswer::Imported(count), Waiting::Fetch) => {
                *self.waiting.borrow_mut() = Waiting::Nothing;
                (self.on_imported)(count);
                self.dialog.close();
            }
            (DiscordAnswer::Error(error), waiting) if waiting != Waiting::Nothing => {
                *self.waiting.borrow_mut() = Waiting::Nothing;
                let body = match waiting {
                    Waiting::Guilds => Some(self.guilds.clone()),
                    _ => self
                        .sounds_page
                        .borrow()
                        .as_ref()
                        .map(|page| page.body.clone()),
                };
                if let Some(body) = body {
                    fill(&body, &status(&error, false));
                }
            }
            _ => {}
        }
    }

    fn show_guilds(self: &Rc<Self>, guilds: &[DiscordGuild]) {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        for guild in guilds {
            let row = adw::ActionRow::new();
            row.set_title(&gtk::glib::markup_escape_text(&guild.name));
            if guild.id == pipedeck_engine::DISCORD_SOUNDS {
                row.set_subtitle("Discord's own sounds");
            }
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            row.set_activatable(true);
            row.connect_activated({
                let this = self.clone();
                let guild = guild.clone();
                move |_| this.open_guild(&guild)
            });
            list.append(&row);
        }
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let hint = gtk::Label::new(Some("Pick a server whose soundboard to take sounds from."));
        hint.add_css_class("dim-label");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        page.append(&hint);
        page.append(&list);
        fill(&self.guilds, &page);
    }

    fn open_guild(self: &Rc<Self>, guild: &DiscordGuild) {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let import = gtk::Button::with_label("Import");
        import.add_css_class("suggested-action");
        import.set_sensitive(false);
        let ticked: Rc<RefCell<Vec<DiscordSound>>> = Rc::default();
        import.connect_clicked({
            let this = self.clone();
            let ticked = ticked.clone();
            move |button| {
                button.set_sensitive(false);
                let sounds = ticked.borrow().clone();
                this.ask(DiscordQuery::Fetch { sounds }, Waiting::Fetch);
            }
        });
        let header = adw::HeaderBar::new();
        header.pack_end(&import);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&scrolled(&body)));
        let page = adw::NavigationPage::new(&view, &guild.name);
        *self.sounds_page.borrow_mut() = Some(SoundsPage {
            body,
            import,
            ticked,
        });
        self.nav.push(&page);
        self.ask(
            DiscordQuery::Sounds {
                guild: guild.id.clone(),
            },
            Waiting::Sounds(guild.id.clone()),
        );
    }

    fn show_sounds(self: &Rc<Self>, sounds: &[DiscordSound]) {
        let page = self.sounds_page.borrow();
        let Some(page) = page.as_ref() else {
            return;
        };
        if sounds.is_empty() {
            fill(
                &page.body,
                &status("This server has no sounds on its soundboard.", false),
            );
            return;
        }
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        for sound in sounds {
            let row = adw::ActionRow::new();
            let title = match &sound.emoji {
                Some(emoji) => format!("{emoji}  {}", sound.name),
                None => sound.name.clone(),
            };
            row.set_title(&gtk::glib::markup_escape_text(&title));
            let tick = gtk::CheckButton::new();
            row.add_prefix(&tick);
            row.set_activatable_widget(Some(&tick));
            tick.connect_toggled({
                let ticked = page.ticked.clone();
                let import = page.import.clone();
                let sound = sound.clone();
                move |tick| {
                    let mut ticked = ticked.borrow_mut();
                    ticked.retain(|kept| kept.id != sound.id);
                    if tick.is_active() {
                        ticked.push(sound.clone());
                    }
                    import.set_sensitive(!ticked.is_empty());
                }
            });
            list.append(&row);
        }
        let hint = gtk::Label::new(Some("Tick the sounds to add to Pipedeck's soundboard."));
        hint.add_css_class("dim-label");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.append(&hint);
        content.append(&list);
        fill(&page.body, &content);
    }
}

/// A page's content, with room around it, scrolling when long.
fn scrolled(body: &gtk::Box) -> gtk::ScrolledWindow {
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);
    scroller.set_child(Some(body));
    scroller
}

/// Put `content` in place of what a page showed.
fn fill(body: &gtk::Box, content: &impl IsA<gtk::Widget>) {
    while let Some(child) = body.first_child() {
        body.remove(&child);
    }
    body.append(content);
}

/// A line saying where things stand, with a spinner while waiting.
fn status(message: &str, waiting: bool) -> gtk::Widget {
    let line = gtk::Box::new(gtk::Orientation::Vertical, 12);
    line.set_valign(gtk::Align::Center);
    line.set_vexpand(true);
    if waiting {
        let spinner = adw::Spinner::new();
        spinner.set_size_request(32, 32);
        line.append(&spinner);
    }
    let label = gtk::Label::new(Some(message));
    label.set_wrap(true);
    label.set_justify(gtk::Justification::Center);
    if !waiting {
        label.add_css_class("dim-label");
    }
    line.append(&label);
    line.upcast()
}
