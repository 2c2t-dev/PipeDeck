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
        detail: "The shortest. Only for a machine with headroom to spare.",
    },
    Latency {
        value: "512/48000",
        label: "11 ms",
        detail: "The default. A round trip close to one PipeWire hop.",
    },
    Latency {
        value: "1024/48000",
        label: "21 ms",
        detail: "PipeWire's own quantum. Safe on most machines.",
    },
    Latency {
        value: "2048/48000",
        label: "43 ms",
        detail: "The longest. For a machine that reports xruns at 21 ms.",
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
        if !self.key.has_focus() && self.key.text() != known {
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
    page.set_icon_name(Some("pd-sfx-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title("Installed");
    group.set_description(Some(
        "Effects a channel can run, read once when Pipedeck starts because \
         opening one runs its own code.",
    ));

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
    install.set_description(Some(
        "A VST3 on Linux is a directory called something.vst3, not a file. Pick \
         the directory, or the shared object inside it and Pipedeck will build \
         the directory around it.",
    ));

    let status = gtk::Label::new(None);
    status.add_css_class("caption");
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.set_visible(false);

    let bundle_row = adw::ActionRow::new();
    bundle_row.set_title("Import a bundle");
    bundle_row.set_subtitle("A something.vst3 directory");
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
    binary_row.set_subtitle("A .so, which is wrapped in the directory it is missing");
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
    if vesktop::state() != vesktop::State::Missing {
        page.add(&vesktop_group());
    }
    page
}

/// What the plugin for Vesktop is doing, from the thread doing it.
enum Work {
    Says(String),
    Done(Result<(), String>),
}

/// Pipedeck's plugin for Vesktop: installed, updated and removed from
/// here, so Vesktop's settings are never touched by hand.
fn vesktop_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title("Discord voices");
    group.set_description(Some(
        "With Pipedeck's plugin in Vesktop, the channel Vesktop plays into unfolds into \
         the people of your call, each with a level, a mute and a meter. Vencord only \
         runs the plugins built into it, so installing builds Vencord with this one in \
         and points Vesktop at it. It needs git and Node.js, and takes a minute or two \
         the first time.",
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

    let status = gtk::Label::new(None);
    status.add_css_class("caption");
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.set_visible(false);
    group.add(&status);

    let show = {
        let (row, install, remove) = (row.clone(), install.clone(), remove.clone());
        move || {
            let installed = vesktop::state() == vesktop::State::Installed;
            if installed {
                row.set_title("Installed in Vesktop");
                row.set_subtitle("Update it when Vesktop or Vencord has been updated");
                install.set_label("Update");
            } else {
                row.set_title("Not installed");
                row.set_subtitle("Vesktop runs its own Vencord, without the plugin");
                install.set_label("Install");
            }
            remove.set_visible(installed);
        }
    };
    show();

    // The work runs on a thread of its own, since building takes a while
    // and waiting for Vesktop to close takes as long as the user does.
    let start = {
        let (install, remove, spinner, status) = (
            install.clone(),
            remove.clone(),
            spinner.clone(),
            status.clone(),
        );
        let show = show.clone();
        move |removing: bool| {
            install.set_sensitive(false);
            remove.set_sensitive(false);
            spinner.set_visible(true);
            status.set_visible(false);
            let (tx, rx) = async_channel::unbounded::<Work>();
            std::thread::spawn(move || {
                let say = |said: &str| {
                    let _ = tx.send_blocking(Work::Says(said.to_owned()));
                };
                let result = if removing {
                    vesktop::remove(say)
                } else {
                    vesktop::install(say)
                };
                let _ = tx.send_blocking(Work::Done(result));
            });
            gtk::glib::spawn_future_local({
                let (install, remove, spinner, status) = (
                    install.clone(),
                    remove.clone(),
                    spinner.clone(),
                    status.clone(),
                );
                let show = show.clone();
                async move {
                    while let Ok(work) = rx.recv().await {
                        status.set_visible(true);
                        match work {
                            Work::Says(said) => status.set_label(&said),
                            Work::Done(result) => {
                                status.set_label(&match (result, removing) {
                                    (Ok(()), false) => {
                                        "Done. Start Vesktop: the plugin is on.".to_owned()
                                    }
                                    (Ok(()), true) => {
                                        "Removed. Vesktop runs its own Vencord again.".to_owned()
                                    }
                                    (Err(e), _) => format!("It did not work: {e}"),
                                });
                                spinner.set_visible(false);
                                install.set_sensitive(true);
                                remove.set_sensitive(true);
                                show();
                            }
                        }
                    }
                }
            });
        }
    };
    install.connect_clicked({
        let start = start.clone();
        move |_| start(false)
    });
    remove.connect_clicked(move |_| start(true));
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
                (true, _) => format!("Nothing it runs needs a key. {}", info.path.display()),
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
        "Thimeo's broadcast processor. It is not ours to ship, so download it          from thimeo.com and import the archive here; Pipedeck runs the          library it holds, on a preset you export from Stereo Tool itself.",
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

    let status = gtk::Label::new(None);
    status.add_css_class("caption");
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.set_visible(false);

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
    autostart.set_subtitle("Writes a desktop entry pointing at this build");
    // The file on disk is the truth, not what was saved last time: it can be
    // removed from outside.
    autostart.set_active(settings::starts_at_login());
    autostart.connect_active_notify({
        let saved = saved.clone();
        move |row| {
            if let Err(e) = settings::set_start_at_login(row.is_active()) {
                log::error!("cannot change the autostart entry: {e}");
                row.set_active(settings::starts_at_login());
                return;
            }
            saved.borrow_mut().start_at_login = row.is_active();
            saved.borrow().save();
        }
    });
    session.add(&autostart);
    page.add(&session);

    page
}

fn audio_page(engine: &EngineLink, latency: &str) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Audio");
    page.set_icon_name(Some("audio-card-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title("Latency");
    group.set_description(Some(
        "Audio crosses two Pipedeck nodes on its way to a device, and each one \
         costs a quantum. A shorter one means less delay and more work for the \
         machine.",
    ));

    let labels: Vec<&str> = LATENCIES.iter().map(|entry| entry.label).collect();
    let row = adw::ComboRow::new();
    row.set_title("Quantum");
    row.set_model(Some(&gtk::StringList::new(&labels)));
    let chosen = LATENCIES
        .iter()
        .position(|entry| entry.value == latency)
        .unwrap_or(1);
    row.set_selected(chosen as u32);
    row.set_subtitle(LATENCIES[chosen].detail);
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
    warning.set_title("Changing it reloads every route");
    warning.set_subtitle("The audio stops for a moment");
    warning.add_prefix(&gtk::Image::from_icon_name("dialog-information-symbolic"));
    group.add(&warning);
    page.add(&group);

    page
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
