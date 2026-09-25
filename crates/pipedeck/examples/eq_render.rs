//! Draws the equaliser to an image, to see it without starting the mixer.
//!
//! `cargo run -p pipedeck --example eq_render [out.png]`. A window of its own
//! opens for a moment, since GTK lays widgets out only on a display; the
//! image is of that window alone.

#[path = "../src/eq_graph.rs"]
#[allow(dead_code)]
mod eq_graph;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

fn main() -> gtk::glib::ExitCode {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "eq.png".to_owned());
    let app = adw::Application::builder()
        .application_id("dev._2c2t.PipedeckEqRender")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

        // A voice worked the usual way: rumble out, a little less body, some
        // presence, a touch of air.
        let values = [
            80.0,    // low cut
            150.0,   // low
            -3.0,    // low gain
            800.0,   // mid
            -2.5,    // mid gain
            1.4,     // mid width
            3500.0,  // presence
            4.0,     // presence gain
            1.0,     // presence width
            10000.0, // air
            2.5,     // air gain
        ];
        let graph = eq_graph::EqGraph::new(&values);
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.set_margin_top(14);
        column.set_margin_bottom(14);
        column.set_margin_start(14);
        column.set_margin_end(14);
        column.set_size_request(460, -1);
        column.append(&graph.root);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .content(&column)
            .build();
        window.present();

        let out = out.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
            let _keep = &graph;
            let paintable = gtk::WidgetPaintable::new(Some(&window));
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(
                &snapshot,
                f64::from(window.width()),
                f64::from(window.height()),
            );
            if let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) {
                match renderer.render_texture(node, None).save_to_png(&out) {
                    Ok(()) => println!("saved {out}"),
                    Err(e) => println!("cannot save {out}: {e}"),
                }
            }
            window.close();
        });
    });
    app.run_with_args::<&str>(&[])
}
