//! The settings window: what the interface remembers, and the one engine
//! setting that is not a fader.

use std::cell::RefCell;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::stereotool::Status;
use pipedeck_engine::Command;

use crate::engine_link::EngineLink;
use crate::settings::{self, Settings, Theme};
use crate::streamdeck::{self, App};
use crate::vesktop;

/// What the Plug-ins page has to show: how many VST3 effects were found,
/// where Stereo Tool stands, and the licence key it was given.
pub struct PluginState<'a> {
    pub installed: usize,
    pub stereotool: &'a Status,
    pub license: Option<&'a str>,
}

/// The quanta offered, as `frames/rate`, with what to call them and what
/// they mean.
///
/// The label is short because a dropdown is as wide as the row that opens
/// it and cuts off anything longer; the sentence goes under the row, where
/// there is room for it.
const LATENCIES: [Latency; 4] = [
    Latency {
        value: "256/48000",
        label: "5 ms",
        detail: "Needs a machine with headroom",
    },
    Latency {
        value: "512/48000",
        label: "11 ms",
        detail: "Default",
    },
    Latency {
        value: "1024/48000",
        label: "21 ms",
        detail: "Safe on most machines",
    },
    Latency {
        value: "2048/48000",
        label: "43 ms",
        detail: "For a machine with xruns at 21 ms",
    },
];

struct Latency {
    value: &'static str,
    label: &'static str,
    detail: &'static str,
}

/// The settings window, once it is up.
///
/// What it says about plug-ins changes under it: importing one is the whole
/// point of the page, and the engine answers a moment later, so the rows it
/// fills are kept to be written again rather than read once.
pub struct Preferences {
    count: adw::ActionRow,
    found: adw::ActionRow,
    key: adw::EntryRow,
    /// Set once the window is gone, so nothing is written into it after.
    closed: Rc<std::cell::Cell<bool>>,
}

impl Preferences {
    pub fn is_open(&self) -> bool {
        !self.closed.get()
    }

    /// How many VST3 effects the engine now knows.
    pub fn set_plugins(&self, installed: usize) {
        if self.is_open() {
            self.count.set_title(&plugin_count(installed));
        }
    }

    /// Where Stereo Tool now stands, and the key it was given.
    pub fn set_stereotool(&self, status: &Status, license: Option<&str>) {
        if !self.is_open() {
            return;
        }
        let (title, subtitle) = stereotool_state(status);
        self.found.set_title(&title);
        self.found.set_subtitle(&subtitle);
        // Not while it is being typed into: the engine echoes back what it
        // was given, and that must not move the cursor under the user.
        let known = license.unwrap_or_default();
        if !crate::widgets::being_edited(&self.key) && self.key.text() != known {
            self.key.set_text(known);
        }
    }
}

/// Show the settings window.
pub fn present(
    parent: &impl IsA<gtk::Widget>,
    engine: &EngineLink,
    latency: &str,
    plugins: &PluginState<'_>,
    on_plugins: bool,
) -> Rc<Preferences> {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    dialog.add(&general_page());
    dialog.add(&audio_page(engine, latency));

    let count = adw::ActionRow::new();
    let found = adw::ActionRow::new();
    let key = adw::EntryRow::new();
    dialog.add(&plugins_page(parent, engine, plugins, &count, &found, &key));

    dialog.add(&about_page());
    // Opened from a notice about the plug-ins, straight on their page.
    if on_plugins {
        dialog.set_visible_page_name("plugins");
    }

    let closed = Rc::new(std::cell::Cell::new(false));
    dialog.connect_closed({
        let closed = closed.clone();
        move |_| closed.set(true)
    });
    dialog.present(Some(parent));
    Rc::new(Preferences {
        count,
        found,
        key,
        closed,
    })
}

/// Where plug-ins come from, and how to add one.
fn plugins_page(
    parent: &impl IsA<gtk::Widget>,
    engine: &EngineLink,
    state: &PluginState<'_>,
    count: &adw::ActionRow,
    found: &adw::ActionRow,
    key: &adw::EntryRow,
) -> adw::PreferencesPage {
    let installed = state.installed;
    let page = adw::PreferencesPage::new();
    page.set_title("Plug-ins");
    page.set_name(Some("plugins"));
    page.set_icon_name(Some("pd-sfx-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title("Installed");

    count.set_title(&plugin_count(installed));
    count.set_subtitle(&settings::user_plugin_dir().map_or_else(
        || "and the system directories".to_owned(),
        |path| format!("{} and the system directories", path.display()),
    ));
    let rescan = gtk::Button::with_label("Look again");
    rescan.set_valign(gtk::Align::Center);
    rescan.connect_clicked({
        let engine = engine.clone();
        move |_| engine.send(Command::RescanPlugins)
    });
    count.add_suffix(&rescan);
    group.add(count);
    page.add(&group);

    let install = adw::PreferencesGroup::new();
    install.set_title("Add one");

    let status = status_label();

    let bundle_row = adw::ActionRow::new();
    bundle_row.set_title("Import a bundle");
    bundle_row.set_subtitle("A .vst3 directory");
    let choose_bundle = gtk::Button::with_label("Choose…");
    choose_bundle.set_valign(gtk::Align::Center);
    choose_bundle.connect_clicked({
        let parent = parent.as_ref().clone();
        let engine = engine.clone();
        let status = status.clone();
        move |_| pick_bundle(&parent, &engine, &status)
    });
    bundle_row.add_suffix(&choose_bundle);
    install.add(&bundle_row);

    let binary_row = adw::ActionRow::new();
    binary_row.set_title("Import a shared object");
    binary_row.set_subtitle("A .so on its own");
    let choose_binary = gtk::Button::with_label("Choose…");
    choose_binary.set_valign(gtk::Align::Center);
    choose_binary.connect_clicked({
        let parent = parent.as_ref().clone();
        let engine = engine.clone();
        let status = status.clone();
        move |_| pick_binary(&parent, &engine, &status)
    });
    binary_row.add_suffix(&choose_binary);
    install.add(&binary_row);

    install.add(&status);
    page.add(&install);

    page.add(&stereotool_group(parent, engine, state, found, key));
    if crate::launcher::sandboxed() {
        page.add(&outside_group());
    } else if vesktop::state() != vesktop::State::Missing {
        page.add(&vesktop_group());
    }
    if [App::OpenDeck, App::StreamController]
        .iter()
        .any(|app| streamdeck::state(*app) != streamdeck::State::Missing)
    {
        page.add(&stream_deck_group());
    }
    page
}

/// In a Flatpak, the plugins Pipedeck cannot put in from its sandbox, and
/// the way to put them in from outside.
fn outside_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("From outside the Flatpak");
    group.set_description(Some(
        "From its sandbox, Pipedeck reaches neither StreamController's folders nor the \
         git and Node.js that build Vesktop's plugin. Install them from a copy of \
         Pipedeck's sources instead.",
    ));
    for (title, subtitle, page) in [
        (
            "StreamController",
            "Run integrations/streamcontroller/install.sh",
            "https://github.com/2c2t-dev/PipeDeck/tree/main/integrations/streamcontroller#install",
        ),
        (
            "Discord voices",
            "Run integrations/vencord/build.sh, then point Vesktop at what it built",
            "https://github.com/2c2t-dev/PipeDeck/tree/main/integrations/vencord#install",
        ),
    ] {
        let row = adw::ActionRow::new();
        row.set_title(title);
        row.set_subtitle(subtitle);
        let how = gtk::LinkButton::with_label(page, "How");
        how.set_valign(gtk::Align::Center);
        row.add_suffix(&how);
        group.add(&row);
    }
    group
}

/// Run something that takes a while on a thread of its own, and tell the
/// window how it ended.
fn in_background(
    work: impl FnOnce() -> Result<String, String> + Send + 'static,
    done: impl Fn(Result<String, String>) + 'static,
) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    gtk::glib::spawn_future_local(async move {
        if let Ok(result) = rx.recv().await {
            done(result);
        }
    });
}

/// Something to run on a thread of its own, saying how it went.
type Work = Box<dyn FnOnce() -> Result<String, String> + Send>;
/// Start some work, with every button waiting until it ends, say how it
/// went, then do what follows.
type Start = Rc<dyn Fn(Work, Rc<dyn Fn()>)>;

/// A line under a group, saying how what was started went.
fn status_label() -> gtk::Label {
    let status = gtk::Label::new(None);
    status.add_css_class("caption");
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.set_visible(false);
    status
}

/// Pipedeck's plugins for the Stream Deck applications that are here,
/// installed, updated and removed from here, and OpenDeck's profiles laid
/// out from the mixer.
fn stream_deck_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Stream Deck");

    let status = status_label();
    let spinner = adw::Spinner::new();
    spinner.set_visible(false);
    // Every button here waits while one of them runs.
    let buttons: Rc<RefCell<Vec<gtk::Button>>> = Rc::default();
    let saved = Rc::new(RefCell::new(Settings::load()));

    let (profiles, lay_out) = profiles_row(&saved);
    buttons.borrow_mut().push(lay_out.clone());
    let start = starter(&status, &spinner, &buttons);

    let mut shows: Vec<Rc<dyn Fn()>> = Vec::new();
    for app in [App::OpenDeck, App::StreamController] {
        if streamdeck::state(app) == streamdeck::State::Missing {
            continue;
        }
        let (row, show) = app_row(app, &profiles, &buttons, &start, &saved);
        group.add(&row);
        shows.push(show);
    }
    group.add(&profiles);
    // On KDE, the application in front, for Add to Channel: a KWin script
    // says which window has the focus.
    if crate::kwin::available() {
        group.add(&front_row(&status, &spinner));
    }
    let show_all: Rc<dyn Fn()> = Rc::new(move || shows.iter().for_each(|show| show()));
    lay_out.connect_clicked(move |_| {
        start(Box::new(streamdeck::lay_out_profiles), show_all.clone());
    });

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.append(&spinner);
    footer.append(&status);
    group.add(&footer);
    group
}

/// Whether OpenDeck's profiles follow the mixer, and a button to lay them
/// out now.
fn profiles_row(saved: &Rc<RefCell<Settings>>) -> (adw::ActionRow, gtk::Button) {
    let lay_out = gtk::Button::with_label("Lay out now");
    lay_out.set_valign(gtk::Align::Center);
    // A Flatpak cannot close OpenDeck, which writes its profiles back over
    // new ones when it closes: the user closes it, and nothing is laid out
    // behind their back.
    if crate::launcher::sandboxed() {
        let profiles = adw::ActionRow::new();
        profiles.set_title("Pipedeck profiles");
        profiles.set_subtitle(
            "Quit OpenDeck first, then lay them out and start it again: \
             from its Flatpak, Pipedeck cannot do it for you",
        );
        profiles.add_suffix(&lay_out);
        return (profiles, lay_out);
    }
    let profiles = adw::SwitchRow::new();
    profiles.set_title("Pipedeck profiles follow the mixer");
    profiles
        .set_subtitle("Redone when a channel, a mix or a device comes or goes; restarts OpenDeck");
    profiles.set_active(saved.borrow().stream_deck_profiles);
    profiles.connect_active_notify({
        let saved = saved.clone();
        move |row| {
            saved.borrow_mut().stream_deck_profiles = row.is_active();
            saved.borrow().save();
        }
    });
    profiles.add_suffix(&lay_out);
    (profiles.upcast(), lay_out)
}

fn starter(
    status: &gtk::Label,
    spinner: &adw::Spinner,
    buttons: &Rc<RefCell<Vec<gtk::Button>>>,
) -> Start {
    let (status, spinner, buttons) = (status.clone(), spinner.clone(), buttons.clone());
    Rc::new(move |work: Work, then: Rc<dyn Fn()>| {
        for button in buttons.borrow().iter() {
            button.set_sensitive(false);
        }
        spinner.set_visible(true);
        status.set_visible(false);
        let (status, spinner, buttons) = (status.clone(), spinner.clone(), buttons.clone());
        in_background(work, move |result| {
            for button in buttons.borrow().iter() {
                button.set_sensitive(true);
            }
            spinner.set_visible(false);
            status.remove_css_class("error");
            match result {
                Ok(said) => status.set_label(&said),
                Err(e) => {
                    status.add_css_class("error");
                    status.set_label(&format!("It did not work: {e}"));
                }
            }
            status.set_visible(true);
            then();
        });
    })
}

/// A Stream Deck application's row, with its plugin's buttons, and what
/// shows where its plugin stands again.
fn app_row(
    app: App,
    profiles: &adw::ActionRow,
    buttons: &Rc<RefCell<Vec<gtk::Button>>>,
    start: &Start,
    saved: &Rc<RefCell<Settings>>,
) -> (adw::ActionRow, Rc<dyn Fn()>) {
    let row = adw::ActionRow::new();
    row.set_title(app.name());
    let install = gtk::Button::new();
    install.set_valign(gtk::Align::Center);
    install.add_css_class("suggested-action");
    row.add_suffix(&install);
    let remove = gtk::Button::with_label("Remove");
    remove.set_valign(gtk::Align::Center);
    row.add_suffix(&remove);
    buttons.borrow_mut().push(install.clone());
    buttons.borrow_mut().push(remove.clone());

    let show: Rc<dyn Fn()> = Rc::new({
        let (row, install, remove, profiles) = (
            row.clone(),
            install.clone(),
            remove.clone(),
            profiles.clone(),
        );
        move || show_plugin(app, &row, &install, &remove, &profiles)
    });
    show();

    install.connect_clicked({
        let (start, saved, show) = (start.clone(), saved.clone(), show.clone());
        move |_| {
            let lay_out = saved.borrow().stream_deck_profiles && !crate::launcher::sandboxed();
            start(
                Box::new(move || streamdeck::install(app, lay_out)),
                show.clone(),
            );
        }
    });
    remove.connect_clicked({
        let (start, show) = (start.clone(), show.clone());
        move |_| start(Box::new(move || streamdeck::remove(app)), show.clone())
    });
    (row, show)
}

/// Where an application's plugin stands, and which of its buttons make
/// sense.
fn show_plugin(
    app: App,
    row: &adw::ActionRow,
    install: &gtk::Button,
    remove: &gtk::Button,
    profiles: &adw::ActionRow,
) {
    let state = streamdeck::state(app);
    let installed = matches!(state, streamdeck::State::Installed { .. });
    match (&state, streamdeck::can_install(app)) {
        (streamdeck::State::Installed { current: true }, _) => {
            row.set_subtitle("Up to date");
        }
        (streamdeck::State::Installed { .. }, Err(e)) => {
            row.set_subtitle(&format!("Installed. {e}"))
        }
        (streamdeck::State::Installed { .. }, Ok(())) => {
            row.set_subtitle("Update available");
        }
        (_, Err(e)) => row.set_subtitle(&e),
        _ => row.set_subtitle("Not installed"),
    }
    // Up to date, it can only be put back as it is.
    let current = state == streamdeck::State::Installed { current: true };
    install.set_label(match (installed, current) {
        (false, _) => "Install",
        (true, false) => "Update",
        (true, true) => "Reinstall",
    });
    if current {
        install.remove_css_class("suggested-action");
    } else {
        install.add_css_class("suggested-action");
    }
    install.set_visible(streamdeck::can_install(app).is_ok());
    remove.set_visible(installed);
    profiles.set_visible(streamdeck::any_installed());
}

/// Whether the KWin script that says which window has the focus is
/// installed, and installing or removing it.
fn front_row(status: &gtk::Label, spinner: &adw::Spinner) -> adw::SwitchRow {
    let front = adw::SwitchRow::new();
    front.set_title("Application in front");
    front.set_subtitle("A KWin script tells Pipedeck which window has the focus");
    front.set_active(crate::kwin::installed());
    front.connect_active_notify({
        let (status, spinner) = (status.clone(), spinner.clone());
        move |row| {
            if row.is_active() == crate::kwin::installed() {
                return;
            }
            let on = row.is_active();
            let result = if on {
                crate::kwin::install()
            } else {
                crate::kwin::remove()
            };
            spinner.set_visible(false);
            if let Err(e) = result {
                status.add_css_class("error");
                status.set_label(&format!("It did not work: {e}"));
                status.set_visible(true);
                row.set_active(crate::kwin::installed());
            }
        }
    });
    front
}

/// What the plugin for Vesktop is doing, from the thread doing it.
enum Progress {
    Says(String),
    Done(Result<(), String>),
}

/// The Vesktop row's parts, which the work started from it changes.
#[derive(Clone)]
struct VesktopRow {
    row: adw::ActionRow,
    install: gtk::Button,
    remove: gtk::Button,
    spinner: adw::Spinner,
    status: gtk::Label,
}

impl VesktopRow {
    /// Where the plugin stands.
    fn show(&self) {
        let installed = vesktop::state() == vesktop::State::Installed;
        self.row.set_title("Vesktop");
        if installed {
            self.row.set_subtitle(&match vesktop::stale() {
                Some(stale) => stale.reason().to_owned(),
                None => "Up to date".to_owned(),
            });
            self.install.set_label("Update");
        } else {
            self.row.set_subtitle("Not installed");
            self.install.set_label("Install");
        }
        self.remove.set_visible(installed);
    }

    /// Wait for the work running, or stop waiting for it.
    fn wait(&self, waiting: bool) {
        self.install.set_sensitive(!waiting);
        self.remove.set_sensitive(!waiting);
        self.spinner.set_visible(waiting);
    }

    /// One started from a window closed since is still running: this one
    /// says so rather than offering to start another.
    fn follow_earlier_work(&self) {
        self.wait(true);
        self.status.set_label(
            &vesktop::last_said()
                .unwrap_or_else(|| "Already at it, from when the settings were last open.".into()),
        );
        self.status.set_visible(true);
        let this = self.clone();
        // What the work says reaches the window that started it; this one
        // reads it from where the work keeps it, and how it ended.
        gtk::glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            if let Some(said) = vesktop::last_said() {
                this.status.set_label(&said);
            }
            if vesktop::busy() {
                return gtk::glib::ControlFlow::Continue;
            }
            this.wait(false);
            this.show();
            gtk::glib::ControlFlow::Break
        });
    }

    /// Install or remove the plugin. The work runs on a thread of its own,
    /// since building takes a while and waiting for Vesktop to close takes
    /// as long as the user does.
    fn start(&self, removing: bool) {
        self.wait(true);
        self.status.set_visible(false);
        let (tx, rx) = async_channel::unbounded::<Progress>();
        std::thread::spawn(move || {
            let say = |said: &str| {
                let _ = tx.send_blocking(Progress::Says(said.to_owned()));
            };
            let result = if removing {
                vesktop::remove(say)
            } else {
                vesktop::install(say)
            };
            let _ = tx.send_blocking(Progress::Done(result));
        });
        let this = self.clone();
        gtk::glib::spawn_future_local(async move {
            while let Ok(progress) = rx.recv().await {
                this.status.set_visible(true);
                match progress {
                    Progress::Says(said) => this.status.set_label(&said),
                    Progress::Done(result) => {
                        this.status.set_label(&vesktop::last_said().unwrap_or_else(
                            || match result {
                                Ok(()) => "Done.".to_owned(),
                                Err(e) => format!("It did not work: {e}"),
                            },
                        ));
                        this.wait(false);
                        this.show();
                    }
                }
            }
        });
    }
}

/// Pipedeck's plugin for Vesktop: installed, updated and removed from
/// here, so Vesktop's settings are never touched by hand.
fn vesktop_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Discord voices");
    group.set_description(Some(
        "Each person of a Vesktop call gets a level, a mute and a meter. Needs git and Node.js.",
    ));

    let row = adw::ActionRow::new();
    let spinner = adw::Spinner::new();
    spinner.set_visible(false);
    row.add_suffix(&spinner);
    let install = gtk::Button::new();
    install.set_valign(gtk::Align::Center);
    install.add_css_class("suggested-action");
    row.add_suffix(&install);
    let remove = gtk::Button::with_label("Remove");
    remove.set_valign(gtk::Align::Center);
    row.add_suffix(&remove);
    group.add(&row);

    let status = status_label();
    group.add(&status);

    let vesktop_row = VesktopRow {
        row,
        install: install.clone(),
        remove: remove.clone(),
        spinner,
        status,
    };
    vesktop_row.show();
    if vesktop::busy() {
        vesktop_row.follow_earlier_work();
    }
    install.connect_clicked({
        let vesktop_row = vesktop_row.clone();
        move |_| vesktop_row.start(false)
    });
    remove.connect_clicked(move |_| vesktop_row.start(true));
    group
}

/// What the count row says.
fn plugin_count(installed: usize) -> String {
    match installed {
        0 => "No VST3 effect found".to_owned(),
        1 => "1 VST3 effect".to_owned(),
        many => format!("{many} VST3 effects"),
    }
}

/// What the Stereo Tool row says: what was found, and what it is worth
/// without a licence.
fn stereotool_state(status: &Status) -> (String, String) {
    match status {
        Status::Absent => (
            "Not installed".to_owned(),
            pipedeck_engine::stereotool::library_dir().map_or_else(
                || "Nowhere to install it: no home directory".to_owned(),
                |path| format!("It goes in {}", path.display()),
            ),
        ),
        Status::Broken(reason) => ("Installed, but it will not run".to_owned(), reason.clone()),
        Status::Ready(info) => (
            format!("Stereo Tool {}", info.version),
            match (&info.licensed, &info.unlicensed) {
                // Not "licensed": the library answers for what it is running,
                // and an unregistered copy running nothing that needs a key
                // answers yes. Saying so plainly beats a word the window's
                // own title bar would contradict.
                (true, _) => "Nothing it runs needs a key".to_owned(),
                (false, Some(features)) => {
                    format!("No licence for: {features}. It adds speech and beeps to the audio.")
                }
                (false, None) => {
                    "No licence key. It adds speech and beeps to the audio.".to_owned()
                }
            },
        ),
    }
}

/// Stereo Tool: what the mixer found, the licence key it passes on, and the
/// way to install it.
fn stereotool_group(
    parent: &impl IsA<gtk::Widget>,
    engine: &EngineLink,
    state: &PluginState<'_>,
    found: &adw::ActionRow,
    key: &adw::EntryRow,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Stereo Tool");
    group.set_description(Some(
        "Download it from thimeo.com and import the archive here.",
    ));

    let (title, subtitle) = stereotool_state(state.stereotool);
    found.set_title(&title);
    found.set_subtitle(&subtitle);
    group.add(found);

    key.set_title("Licence key");
    key.set_show_apply_button(true);
    key.set_text(state.license.unwrap_or_default());
    key.connect_apply({
        let engine = engine.clone();
        move |row| {
            let typed = row.text().trim().to_owned();
            engine.send(Command::SetStereoToolLicense {
                key: (!typed.is_empty()).then_some(typed),
            });
        }
    });
    group.add(key);

    let status = status_label();

    let import = adw::ActionRow::new();
    import.set_title("Import Stereo Tool");
    import.set_subtitle("The .zip as downloaded, or a libStereoTool .so out of it");
    let choose = gtk::Button::with_label("Choose…");
    choose.set_valign(gtk::Align::Center);
    choose.connect_clicked({
        let parent = parent.as_ref().clone();
        let engine = engine.clone();
        let status = status.clone();
        move |_| pick_stereotool(&parent, &engine, &status)
    });
    import.add_suffix(&choose);
    group.add(&import);
    group.add(&status);

    group
}

/// Ask for the archive and install what comes back.
fn pick_stereotool(parent: &gtk::Widget, engine: &EngineLink, status: &gtk::Label) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Stereo Tool"));
    filter.add_pattern("*.zip");
    filter.add_pattern("libStereoTool*.so");
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);

    let dialog = gtk::FileDialog::new();
    dialog.set_title("Choose the Stereo Tool download");
    dialog.set_filters(Some(&filters));
    let window = parent.root().and_downcast::<gtk::Window>();
    dialog.open(window.as_ref(), None::<&gtk::gio::Cancellable>, {
        let engine = engine.clone();
        let status = status.clone();
        move |answer| {
            let Ok(file) = answer else {
                return;
            };
            let Some(path) = file.path() else {
                return;
            };
            status.set_visible(true);
            status.remove_css_class("error");
            status.remove_css_class("success");
            match settings::import_stereotool(&path) {
                Ok(installed) => {
                    status.add_css_class("success");
                    status.set_label(&format!(
                        "Installed {}. It is in the effects list now.",
                        installed
                            .iter()
                            .filter_map(|path| path.file_name())
                            .map(|name| name.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    // The engine looks again, which is also what tells it to
                    // load the library and report on the licence.
                    engine.send(Command::RescanPlugins);
                }
                Err(e) => {
                    status.add_css_class("error");
                    status.set_label(&e);
                }
            }
        }
    });
}

/// Ask for a bundle directory and install what comes back.
fn pick_bundle(parent: &gtk::Widget, engine: &EngineLink, status: &gtk::Label) {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Choose a .vst3 directory");
    let window = parent.root().and_downcast::<gtk::Window>();
    dialog.select_folder(window.as_ref(), None::<&gtk::gio::Cancellable>, {
        let engine = engine.clone();
        let status = status.clone();
        move |answer| {
            let Ok(file) = answer else {
                return;
            };
            let Some(path) = file.path() else {
                return;
            };
            install(&path, &engine, &status);
        }
    });
}

/// Ask for a shared object and install what comes back.
fn pick_binary(parent: &gtk::Widget, engine: &EngineLink, status: &gtk::Label) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Shared objects"));
    filter.add_pattern("*.so");
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);

    let dialog = gtk::FileDialog::new();
    dialog.set_title("Choose a plug-in");
    dialog.set_filters(Some(&filters));
    let window = parent.root().and_downcast::<gtk::Window>();
    dialog.open(window.as_ref(), None::<&gtk::gio::Cancellable>, {
        let engine = engine.clone();
        let status = status.clone();
        move |answer| {
            let Ok(file) = answer else {
                return;
            };
            let Some(path) = file.path() else {
                return;
            };
            install(&path, &engine, &status);
        }
    });
}

/// Put a plug-in in place and ask the engine to look again.
fn install(path: &std::path::Path, engine: &EngineLink, status: &gtk::Label) {
    status.set_visible(true);
    status.remove_css_class("error");
    status.remove_css_class("success");
    match settings::import_plugin(path) {
        Ok(bundle) => {
            status.add_css_class("success");
            status.set_label(&format!(
                "Installed in {}. It is in the effects list now.",
                bundle.display()
            ));
            engine.send(Command::RescanPlugins);
        }
        Err(e) => {
            status.add_css_class("error");
            status.set_label(&e);
        }
    }
}

fn general_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("General");
    page.set_icon_name(Some("preferences-system-symbolic"));

    let saved = Rc::new(RefCell::new(Settings::load()));

    let look = adw::PreferencesGroup::new();
    look.set_title("Personalization");

    let themes: Vec<&str> = Theme::ALL.iter().map(|theme| theme.label()).collect();
    let theme = adw::ComboRow::new();
    theme.set_title("Colour theme");
    theme.set_model(Some(&gtk::StringList::new(&themes)));
    theme.set_selected(
        Theme::ALL
            .iter()
            .position(|candidate| *candidate == saved.borrow().theme)
            .unwrap_or(0) as u32,
    );
    theme.connect_selected_notify({
        let saved = saved.clone();
        move |row| {
            let chosen = Theme::ALL
                .get(row.selected() as usize)
                .copied()
                .unwrap_or_default();
            apply_theme(chosen);
            saved.borrow_mut().theme = chosen;
            saved.borrow().save();
        }
    });
    look.add(&theme);
    page.add(&look);

    let session = adw::PreferencesGroup::new();
    session.set_title("Session");

    let autostart = adw::SwitchRow::new();
    autostart.set_title("Start Pipedeck at login");
    // The file on disk is the truth, not what was saved last time: it can be
    // removed from outside.
    autostart.set_active(settings::starts_at_login());
    autostart.connect_active_notify({
        let saved = saved.clone();
        move |row| {
            if let Err(e) =
                settings::set_start_at_login(row.is_active(), saved.borrow().keep_running)
            {
                log::error!("cannot change the autostart entry: {e}");
                row.set_active(settings::starts_at_login());
                return;
            }
            saved.borrow_mut().start_at_login = row.is_active();
            saved.borrow().save();
        }
    });
    session.add(&autostart);

    let keep = adw::SwitchRow::new();
    keep.set_title("Keep running when the window is closed");
    keep.set_subtitle("The mixer and the Stream Decks carry on, with an icon to open or quit");
    keep.set_active(saved.borrow().keep_running);
    keep.connect_active_notify({
        let saved = saved.clone();
        move |row| {
            let on = row.is_active();
            saved.borrow_mut().keep_running = on;
            saved.borrow().save();
            // The window open now, and the session's next start.
            if let Some(app) = gtk::gio::Application::default().and_downcast::<adw::Application>() {
                for window in app.windows() {
                    window.set_hide_on_close(on);
                }
                crate::tray::show(&app, on);
            }
            if settings::starts_at_login() {
                if let Err(e) = settings::set_start_at_login(true, on) {
                    log::error!("cannot change the autostart entry: {e}");
                }
            }
        }
    });
    session.add(&keep);

    let software = adw::SwitchRow::new();
    software.set_title("Draw without the graphics card");
    software.set_subtitle("If letters look broken. Applies on restart");
    software.set_active(saved.borrow().software_rendering);
    software.connect_active_notify({
        let saved = saved.clone();
        move |row| {
            saved.borrow_mut().software_rendering = row.is_active();
            saved.borrow().save();
        }
    });
    session.add(&software);
    page.add(&session);

    page
}

fn audio_page(engine: &EngineLink, latency: &str) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Audio");
    page.set_icon_name(Some("audio-card-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title("Latency");

    let labels: Vec<&str> = LATENCIES.iter().map(|entry| entry.label).collect();
    let row = adw::ComboRow::new();
    row.set_title("Quantum");
    row.set_model(Some(&gtk::StringList::new(&labels)));
    // A quantum set some other way than here is said as it is, with none of
    // the four chosen, so that any of them can be picked from it.
    match LATENCIES.iter().position(|entry| entry.value == latency) {
        Some(chosen) => {
            row.set_selected(chosen as u32);
            row.set_subtitle(LATENCIES[chosen].detail);
        }
        None => {
            row.set_selected(gtk::INVALID_LIST_POSITION);
            row.set_subtitle(&format!("Set to {latency}, which is none of these."));
        }
    }
    row.connect_selected_notify({
        let engine = engine.clone();
        move |row| {
            let Some(entry) = LATENCIES.get(row.selected() as usize) else {
                return;
            };
            row.set_subtitle(entry.detail);
            engine.send(Command::SetLatency {
                latency: entry.value.to_owned(),
            });
        }
    });
    group.add(&row);

    let warning = adw::ActionRow::new();
    warning.set_title("Changing it stops the audio for a moment");
    warning.add_prefix(&gtk::Image::from_icon_name("dialog-information-symbolic"));
    group.add(&warning);
    page.add(&group);
    page.add(&mixer_group(engine));

    page
}

/// The mixer in a file and back: to keep it, to bring it to another
/// computer, or to give it. The engine writes and reads the file, and says
/// how it went.
fn mixer_group(engine: &EngineLink) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Mixer");
    group.set_description(Some(
        "Its channels, mixes, cells and effects, in a file. The Stereo Tool key stays out \
         of it, and devices another computer lacks are picked again there.",
    ));

    let export = gtk::Button::with_label("Export…");
    export.set_valign(gtk::Align::Center);
    export.connect_clicked({
        let engine = engine.clone();
        move |button| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("Export the mixer");
            dialog.set_initial_name(Some("pipedeck-mixer.toml"));
            dialog.set_filters(Some(&mixer_files()));
            let window = button.root().and_downcast::<gtk::Window>();
            let engine = engine.clone();
            dialog.save(
                window.as_ref(),
                None::<&gtk::gio::Cancellable>,
                move |answer| {
                    if let Some(path) = answer.ok().and_then(|file| file.path()) {
                        engine.send(Command::ExportConfig(path));
                    }
                },
            );
        }
    });
    let row = adw::ActionRow::new();
    row.set_title("Export the mixer");
    row.set_subtitle("To keep it, or to bring it to another computer");
    row.add_suffix(&export);
    group.add(&row);

    let import = gtk::Button::with_label("Import…");
    import.set_valign(gtk::Align::Center);
    import.connect_clicked({
        let engine = engine.clone();
        move |button| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("Import a mixer");
            dialog.set_filters(Some(&mixer_files()));
            let window = button.root().and_downcast::<gtk::Window>();
            let (engine, button) = (engine.clone(), button.clone());
            dialog.open(
                window.as_ref(),
                None::<&gtk::gio::Cancellable>,
                move |answer| {
                    if let Some(path) = answer.ok().and_then(|file| file.path()) {
                        confirm_import(&button, &engine, path);
                    }
                },
            );
        }
    });
    let row = adw::ActionRow::new();
    row.set_title("Import a mixer");
    row.set_subtitle("In place of this one");
    row.add_suffix(&import);
    group.add(&row);
    group
}

/// What the mixer's file dialogs show.
fn mixer_files() -> gtk::gio::ListStore {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Pipedeck mixers"));
    filter.add_pattern("*.toml");
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    filters
}

/// Ask before the mixer is replaced, since nothing of it is kept.
fn confirm_import(parent: &gtk::Button, engine: &EngineLink, path: std::path::PathBuf) {
    let dialog = adw::AlertDialog::new(
        Some("Replace the mixer?"),
        Some(
            "Its channels, mixes, cells and effects give way to the file's. The audio stops \
             for a moment while the new mixer is put in place.",
        ),
    );
    dialog.add_responses(&[("cancel", "Cancel"), ("replace", "Replace")]);
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let engine = engine.clone();
    dialog.choose(
        Some(parent),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response == "replace" {
                engine.send(Command::ImportConfig(path));
            }
        },
    );
}

fn about_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("About");
    page.set_icon_name(Some("help-about-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title("Pipedeck");

    let version = adw::ActionRow::new();
    version.set_title("Version");
    version.set_subtitle(env!("CARGO_PKG_VERSION"));
    group.add(&version);

    let icons = adw::ActionRow::new();
    icons.set_title("Icons");
    icons.set_subtitle("Material Symbols by Google, Apache License 2.0");
    group.add(&icons);

    let repository = adw::ActionRow::new();
    repository.set_title("Source");
    repository.set_subtitle(env!("CARGO_PKG_REPOSITORY"));
    group.add(&repository);
    page.add(&group);

    page
}

/// Follow the desktop, or override it.
pub fn apply_theme(theme: Theme) {
    adw::StyleManager::default().set_color_scheme(match theme {
        Theme::System => adw::ColorScheme::Default,
        Theme::Light => adw::ColorScheme::ForceLight,
        Theme::Dark => adw::ColorScheme::ForceDark,
    });
}
