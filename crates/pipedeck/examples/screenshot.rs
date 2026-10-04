//! Draws the mixer's window and the equaliser's to images, on a mixer made
//! up for them: the pictures in the README.
//!
//! `cargo run -p pipedeck --example screenshot [folder] [--light]` writes
//! `mixer-dark.png` and `equaliser-dark.png` there, or `-light` with
//! `--light`, drawn at twice their size. Run it on GTK's Broadway backend
//! and nothing shows on the desktop:
//!
//! ```sh
//! gtk4-broadwayd :5 &
//! GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 \
//!     cargo run -p pipedeck --example screenshot .github
//! ```
//!
//! The window is told about the mixer the way the engine would tell it, but
//! by this program: an engine runs only because the window needs one to
//! talk to, on a scratch config, and nothing it says is passed on. So no
//! node is made, no application is moved and nothing plays. The window's
//! own settings are read from a scratch folder too, where the Stream Deck
//! profiles are not laid out from what it shows.

#![allow(dead_code, unused_imports)]

#[path = "shared/capture.rs"]
mod capture;
#[path = "../src/cell.rs"]
mod cell;
#[path = "../src/channel_dialog.rs"]
mod channel_dialog;
#[path = "../src/comp_graph.rs"]
mod comp_graph;
#[path = "../src/deesser_graph.rs"]
mod deesser_graph;
#[path = "../src/denoise_graph.rs"]
mod denoise_graph;
#[path = "../src/desktop.rs"]
mod desktop;
#[path = "../src/dialogs.rs"]
mod dialogs;
#[path = "../src/effect_panel.rs"]
mod effect_panel;
#[path = "../src/effects.rs"]
mod effects;
#[path = "../src/engine_link.rs"]
mod engine_link;
#[path = "../src/eq_graph.rs"]
mod eq_graph;
#[path = "../src/kwin.rs"]
mod kwin;
#[path = "../src/launcher.rs"]
mod launcher;
#[path = "../src/listen.rs"]
mod listen;
#[path = "../src/meter_fader.rs"]
mod meter_fader;
#[path = "../src/mix_dialog.rs"]
mod mix_dialog;
#[path = "../src/preferences.rs"]
mod preferences;
#[path = "../src/presets.rs"]
mod presets;
#[path = "../src/settings.rs"]
mod settings;
#[path = "../src/signals.rs"]
mod signals;
#[path = "../src/streamdeck.rs"]
mod streamdeck;
#[path = "../src/tray.rs"]
mod tray;
#[path = "../src/vesktop.rs"]
mod vesktop;
#[path = "../src/widgets.rs"]
mod widgets;
#[path = "../src/window.rs"]
mod window;

use std::time::Duration;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::stereotool::Status;
use pipedeck_engine::{Config, Device, Event, MixId, SourceId, StateSnapshot};

/// The mixer shown: five channels, three mixes, the cells between them.
const MIXER: &str = r#"
latency = "512/48000"
listen_device = "demo.headphones"

[[mix]]
id = 1
name = "Personal Mix"
icon = "headset"
gain = 1.0
muted = false
[[mix.outputs]]
device = "demo.headphones"
gain = 1.0
muted = false
enabled = true

[[mix]]
id = 2
name = "Chat Mix"
icon = "voice"
gain = 1.0
muted = false

[[mix]]
id = 3
name = "Stream Mix"
icon = "stream"
gain = 0.9
muted = false

[[source]]
id = 1
name = "Microphone"
icon = "mic"
device = "demo.microphone"
gain = 1.0
muted = false

[[source]]
id = 2
name = "Music"
icon = "music"
gain = 0.8
muted = false

[[source]]
id = 3
name = "Game"
icon = "game"
gain = 1.0
muted = false

[[source]]
id = 4
name = "Voice chat"
icon = "voice"
gain = 1.0
muted = false

[[source]]
id = 5
name = "Browser"
icon = "browser"
gain = 0.7
muted = true

[[link]]
source = 1
mix = 2
gain = 1.0
muted = false

[[link]]
source = 1
mix = 3
gain = 1.0
muted = false

[[link]]
source = 2
mix = 1
gain = 0.6
muted = false

[[link]]
source = 2
mix = 2
gain = 0.25
muted = false

[[link]]
source = 2
mix = 3
gain = 0.35
muted = false

[[link]]
source = 3
mix = 1
gain = 0.9
muted = false

[[link]]
source = 3
mix = 3
gain = 0.7
muted = false

[[link]]
source = 4
mix = 1
gain = 1.0
muted = false

[[link]]
source = 4
mix = 3
gain = 0.8
muted = true

[[link]]
source = 5
mix = 1
gain = 0.8
muted = false
"#;

/// What the meters show: each channel and mix at a level of its own.
const SOURCE_PEAKS: [(u32, f32); 5] = [(1, 0.55), (2, 0.4), (3, 0.7), (4, 0.3), (5, 0.0)];
const MIX_PEAKS: [(u32, f32); 3] = [(1, 0.62), (2, 0.5), (3, 0.58)];

fn main() -> gtk::glib::ExitCode {
    let mut args = std::env::args().skip(1);
    let light = std::env::args().any(|arg| arg == "--light");
    let folder = std::path::PathBuf::from(
        args.find(|arg| !arg.starts_with("--"))
            .unwrap_or_else(|| ".".to_owned()),
    );
    let theme = if light { "light" } else { "dark" };
    let mixer_png = folder.join(format!("mixer-{theme}.png"));
    let equaliser_png = folder.join(format!("equaliser-{theme}.png"));

    // The window's settings, kept apart from the user's: no Stream Deck
    // profiles laid out from a made-up mixer. Before any thread exists, so
    // nothing else reads the environment.
    let scratch = std::env::temp_dir().join(format!("pipedeck-screenshot-{}", std::process::id()));
    let interface = scratch.join("pipedeck/interface.toml");
    std::fs::create_dir_all(interface.parent().expect("a folder")).expect("a scratch folder");
    std::fs::write(&interface, "stream_deck_profiles = false\n").expect("a scratch file");
    std::env::set_var("XDG_CONFIG_HOME", &scratch);
    std::env::set_var("PIPEDECK_NODE_PREFIX", "pipedeck-screenshot");

    let mut config: Config = toml::from_str(MIXER).expect("the mixer shown is valid");
    let catalogue = effects::catalogue();
    let effect = |id: &str| {
        let spec = catalogue
            .iter()
            .find(|spec| spec.id == id)
            .expect("one of ours");
        effects::build(spec)
    };
    config.sources[0].effects = ["denoise", "eq", "deesser", "compressor"]
        .into_iter()
        .map(effect)
        .collect();
    config.sources[1].effects = vec![effect("eq")];
    // The microphone's equaliser, set for a voice: the rumble cut, some
    // body, less mud, more presence and air.
    let mut equaliser = config.sources[0].effects[1].clone();
    for (control, value) in [
        ("low_cut", 80.0),
        ("low_gain", 3.0),
        ("mid_freq", 350.0),
        ("mid_gain", -4.0),
        ("pres_gain", 4.0),
        ("high_gain", 3.0),
    ] {
        effects::set_value(&mut equaliser, control, value);
    }
    config.sources[0].effects[1] = equaliser;
    let state = StateSnapshot {
        latency: config.latency.clone(),
        stereotool_license: None,
        listen_device: config.listen_device.clone(),
        mixes: config.mixes.clone(),
        sources: config.sources.clone(),
        links: config.links.clone(),
    };

    let engine = pipedeck_engine::spawn(scratch.join("pipedeck/config.toml"), |_| {});
    let link = engine_link::EngineLink::new(engine);

    gtk::gio::resources_register_include!("pipedeck.gresource")
        .expect("the icons are compiled into the binary");
    let app = adw::Application::builder()
        .application_id("dev._2c2t.PipedeckScreenshot")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate({
        let link = link.clone();
        move |app| {
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::IconTheme::for_display(&display)
                    .add_resource_path("/dev/_2c2t/Pipedeck/icons");
            }
            adw::StyleManager::default().set_color_scheme(if light {
                adw::ColorScheme::ForceLight
            } else {
                adw::ColorScheme::ForceDark
            });
            window::load_css();

            let window = window::Window::new(app, link.clone());
            window.handle_event(Event::Devices {
                outputs: vec![
                    device("demo.headphones", "Headphones"),
                    device("demo.speakers", "Speakers"),
                ],
                inputs: vec![device("demo.microphone", "USB Microphone")],
            });
            window.handle_event(Event::State(state.clone()));
            window.handle_event(Event::Plugins { available: vec![] });
            window.handle_event(Event::StereoTool(Status::Absent));
            window.present();

            // The meters fall back between peaks, so they are fed until the
            // picture is taken.
            gtk::glib::timeout_add_local(Duration::from_millis(50), {
                let window = window.clone();
                move || {
                    window.handle_event(Event::Levels {
                        sources: SOURCE_PEAKS
                            .iter()
                            .map(|&(id, peak)| (SourceId(id), peak))
                            .collect(),
                        mixes: MIX_PEAKS
                            .iter()
                            .map(|&(id, peak)| (MixId(id), peak))
                            .collect(),
                        voices: vec![],
                        effects: vec![],
                    });
                    gtk::glib::ControlFlow::Continue
                }
            });
            gtk::glib::timeout_add_local_once(Duration::from_millis(1500), {
                let app = app.clone();
                let link = link.clone();
                let mixer_png = mixer_png.clone();
                let equaliser_png = equaliser_png.clone();
                let chain = state.sources[0].effects.clone();
                move || {
                    if let Some(shown) = app.active_window() {
                        capture::save(&shown, &mixer_png, 2.0);
                    }
                    show_equaliser(&app, &link, &chain, equaliser_png.clone());
                }
            });
        }
    });
    let code = app.run_with_args::<&str>(&[]);
    link.shutdown();
    let _ = std::fs::remove_dir_all(&scratch);
    code
}

/// Open the microphone's equaliser in a window of its own, the way its gear
/// does, then draw it and quit.
fn show_equaliser(
    app: &adw::Application,
    link: &engine_link::EngineLink,
    chain: &[pipedeck_engine::Effect],
    out: std::path::PathBuf,
) {
    let panel = effect_panel::EffectPanel::new(link, SourceId(1));
    panel.refresh(chain, &[], &Status::Absent);
    let holder = adw::ApplicationWindow::builder()
        .application(app)
        .content(&panel.widget())
        .build();
    holder.present();
    panel.open_settings(1);
    gtk::glib::timeout_add_local_once(Duration::from_millis(600), {
        let app = app.clone();
        move || {
            let settings = gtk::Window::list_toplevels()
                .into_iter()
                .filter_map(|w| w.downcast::<gtk::Window>().ok())
                .find(|w| w.transient_for().as_ref() == Some(holder.upcast_ref()));
            match settings {
                Some(settings) => capture::save(&settings, &out, 2.0),
                None => println!("the equaliser did not open"),
            }
            panel.close_windows();
            app.quit();
        }
    });
}

fn device(name: &str, description: &str) -> Device {
    Device {
        name: name.to_owned(),
        description: description.to_owned(),
    }
}
