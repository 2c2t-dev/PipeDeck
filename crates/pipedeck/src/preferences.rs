//! The settings window: what the interface remembers, and the one engine
//! setting that is not a fader.

use std::cell::RefCell;
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::Command;

use crate::engine_link::EngineLink;
use crate::settings::{self, Settings, Theme};

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

/// Show the settings window.
pub fn present(parent: &impl IsA<gtk::Widget>, engine: &EngineLink, latency: &str, plugins: usize) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    dialog.add(&general_page());
    dialog.add(&audio_page(engine, latency));
    dialog.add(&plugins_page(parent, engine, plugins));
    dialog.add(&about_page());
    dialog.present(Some(parent));
}

/// Where plug-ins come from, and how to add one.
fn plugins_page(
    parent: &impl IsA<gtk::Widget>,
    engine: &EngineLink,
    installed: usize,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Plug-ins");
    page.set_icon_name(Some("pd-sfx-symbolic"));

    let group = adw::PreferencesGroup::new();
    group.set_title("Installed");
    group.set_description(Some(
        "Effects a channel can run, read once when Pipedeck starts because \
         opening one runs its own code.",
    ));

    let count = adw::ActionRow::new();
    count.set_title(&match installed {
        0 => "No VST3 effect found".to_owned(),
        1 => "1 VST3 effect".to_owned(),
        many => format!("{many} VST3 effects"),
    });
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
    group.add(&count);
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

    page
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
