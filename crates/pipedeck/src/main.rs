//! Pipedeck: a PipeWire mixer built as a matrix of channels and mixes.

mod cell;
mod channel_dialog;
mod desktop;
mod dialogs;
mod engine_link;
mod mix_dialog;
mod signals;
mod widgets;
mod window;

use adw::gtk::glib;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{Config, Event};

use engine_link::EngineLink;
use window::Window;

/// Reverse-DNS id of 2c2t.dev. GApplication (like D-Bus) forbids an element
/// starting with a digit, so the `2c2t` component takes the conventional
/// leading underscore. Changing this later also renames the .desktop file,
/// the GSettings path and the Flatpak sandbox, so it is meant to be stable.
const APP_ID: &str = "dev._2c2t.Pipedeck";

enum Msg {
    Engine(Event),
    Quit,
}

fn main() -> glib::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // Before any thread exists, so every thread inherits the mask.
    signals::block_termination_signals();

    ignore_prefer_dark_theme();

    let app_id = std::env::var("PIPEDECK_APP_ID").unwrap_or_else(|_| APP_ID.to_owned());
    let app = adw::Application::builder().application_id(app_id).build();
    app.connect_activate(activate);
    app.run()
}

/// Neutralize `GtkSettings:gtk-application-prefer-dark-theme` for this
/// process.
///
/// Desktops that also configure GTK3 (KDE writes the same key into both
/// `gtk-3.0/settings.ini` and `gtk-4.0/settings.ini`) leave that key set to
/// true. libadwaita manages light/dark itself from the desktop's
/// `color-scheme` and warns on every startup when it sees the key, so we
/// clear it here rather than asking users to edit a file GTK3 still needs.
/// This is process-local and changes nothing for other applications.
///
/// It has to happen before libadwaita builds its style manager, which the
/// application does during `startup`; the `startup` class handler runs ahead
/// of any handler we could connect, hence the explicit `gtk::init` here.
fn ignore_prefer_dark_theme() {
    if let Err(e) = adw::gtk::init() {
        log::debug!("cannot initialize GTK early: {e}");
        return;
    }
    if let Some(settings) = adw::gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(false);
    }
}

fn activate(app: &adw::Application) {
    if let Some(existing) = app.active_window() {
        existing.present();
        return;
    }

    let config_path = match Config::default_path() {
        Ok(path) => path,
        Err(e) => {
            log::error!("{e}");
            std::path::PathBuf::from("pipedeck.toml")
        }
    };
    log::info!("config: {}", config_path.display());

    let (tx, rx) = async_channel::unbounded::<Msg>();
    let engine = EngineLink::new(pipedeck_engine::spawn(config_path, {
        let tx = tx.clone();
        move |event| {
            let _ = tx.send_blocking(Msg::Engine(event));
        }
    }));
    signals::spawn_watcher(move || {
        let _ = tx.send_blocking(Msg::Quit);
    });

    window::load_css();
    let window = Window::new(app, engine.clone());

    // Stop the engine (and wait for the graph teardown) when the app exits.
    app.connect_shutdown(move |_| engine.shutdown());

    glib::spawn_future_local({
        let window = window.clone();
        let app = app.clone();
        async move {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    Msg::Engine(event) => window.handle_event(event),
                    Msg::Quit => app.quit(),
                }
            }
        }
    });

    window.present();
}
