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

/// The quanta offered, as `frames/rate`, from the shortest that most
/// machines survive to PipeWire's own default.
const LATENCIES: [(&str, &str); 4] = [
    ("256/48000", "5 ms, for a machine that can take it"),
    ("512/48000", "11 ms, the default"),
    ("1024/48000", "21 ms, PipeWire's own"),
    ("2048/48000", "43 ms, when nothing else holds"),
];

/// Show the settings window.
pub fn present(parent: &impl IsA<gtk::Widget>, engine: &EngineLink, latency: &str) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    dialog.add(&general_page());
    dialog.add(&audio_page(engine, latency));
    dialog.add(&about_page());
    dialog.present(Some(parent));
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
         costs a quantum. A shorter quantum means less delay and more work for \
         the machine; if it reports xruns, take the next one up.",
    ));

    let labels: Vec<&str> = LATENCIES.iter().map(|(_, label)| *label).collect();
    let row = adw::ComboRow::new();
    row.set_title("Quantum");
    row.set_model(Some(&gtk::StringList::new(&labels)));
    row.set_selected(
        LATENCIES
            .iter()
            .position(|(value, _)| *value == latency)
            .unwrap_or(1) as u32,
    );
    row.connect_selected_notify({
        let engine = engine.clone();
        move |row| {
            let Some((value, _)) = LATENCIES.get(row.selected() as usize) else {
                return;
            };
            engine.send(Command::SetLatency {
                latency: (*value).to_owned(),
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
