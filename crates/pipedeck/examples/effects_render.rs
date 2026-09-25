//! Draws a channel's effects tab to an image, with every effect the mixer
//! runs itself, and each effect's settings window to one more, to see them
//! without opening a channel.
//!
//! `cargo run -p pipedeck --example effects_render [out.png]` writes the tab
//! to `out.png` and the windows to `out-1.png` and on. The tab needs
//! an engine to talk to, so one is started on a scratch config and stopped
//! once the image is taken; nothing is sent to it.

#[path = "../src/comp_graph.rs"]
mod comp_graph;
#[path = "../src/deesser_graph.rs"]
mod deesser_graph;
#[path = "../src/effect_panel.rs"]
#[allow(dead_code)]
mod effect_panel;
#[path = "../src/effects.rs"]
mod effects;
#[path = "../src/engine_link.rs"]
mod engine_link;
#[path = "../src/eq_graph.rs"]
mod eq_graph;
#[path = "../src/meter_fader.rs"]
#[allow(dead_code)]
mod meter_fader;
#[path = "../src/presets.rs"]
#[allow(dead_code)]
mod presets;
#[path = "../src/widgets.rs"]
#[allow(dead_code, unused_imports)]
mod widgets;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::stereotool::Status;
use pipedeck_engine::SourceId;

fn main() -> gtk::glib::ExitCode {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "effects.png".to_owned());
    let config = std::env::temp_dir().join(format!("pipedeck-render-{}.json", std::process::id()));
    let link = engine_link::EngineLink::new(pipedeck_engine::spawn(config.clone(), |_| {}));

    let app = adw::Application::builder()
        .application_id("dev._2c2t.PipedeckEffectsRender")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate({
        let link = link.clone();
        move |app| {
            adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

            let panel = effect_panel::EffectPanel::new(&link, SourceId(1));
            let chain: Vec<_> = effects::catalogue().iter().map(effects::build).collect();
            panel.refresh(&chain, &[], &Status::Absent);

            let page = panel.widget();
            page.set_margin_start(14);
            page.set_margin_end(14);
            page.set_margin_bottom(14);
            page.set_size_request(480, 420);

            let window = adw::ApplicationWindow::builder()
                .application(app)
                .content(&page)
                .build();
            window.present();
            for position in 0..chain.len() {
                panel.open_settings(position);
            }

            let out = out.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
                let stem = out.strip_suffix(".png").unwrap_or(&out).to_owned();
                save(window.upcast_ref(), &out);
                let settings = gtk::Window::list_toplevels()
                    .into_iter()
                    .filter_map(|w| w.downcast::<gtk::Window>().ok())
                    .filter(|w| w.transient_for().as_ref() == Some(window.upcast_ref()));
                for (n, settings) in settings.enumerate() {
                    save(&settings, &format!("{stem}-{}.png", n + 1));
                }
                panel.close_windows();
                window.close();
            });
        }
    });
    let code = app.run_with_args::<&str>(&[]);
    link.shutdown();
    let _ = std::fs::remove_file(&config);
    code
}

/// Write what a window shows to a PNG.
fn save(window: &gtk::Window, out: &str) {
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(
        &snapshot,
        f64::from(window.width()),
        f64::from(window.height()),
    );
    if let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) {
        match renderer.render_texture(node, None).save_to_png(out) {
            Ok(()) => println!("saved {out}"),
            Err(e) => println!("cannot save {out}: {e}"),
        }
    }
}
