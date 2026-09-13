//! Pipedeck: a PipeWire mixer with a Stream mix and a Monitor mix per source.

mod engine_link;
mod signals;
mod source_column;
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

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(activate);
    app.run()
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
