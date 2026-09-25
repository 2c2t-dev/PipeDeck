//! Draws the fader to an image, to see it without starting the mixer.
//!
//! `cargo run -p pipedeck --example fader_render [out.png]`. It builds rows
//! laid out as a cell of the matrix is — the mute, the fader, the unlink —
//! at a few settings and levels, and saves what GTK renders of them. A
//! window of its own opens for a moment, since GTK lays widgets out only in
//! a window on a display; the image is of that window's content alone.

#[path = "../src/meter_fader.rs"]
#[allow(dead_code)]
mod meter_fader;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use meter_fader::{Meter, MeterFader};

fn row(gain: f32, level: f32, width: i32) -> (gtk::Box, MeterFader) {
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.set_margin_start(8);
    controls.set_margin_end(8);
    controls.set_margin_top(10);
    controls.set_margin_bottom(10);
    controls.set_size_request(width, -1);

    let mute = gtk::ToggleButton::new();
    mute.set_icon_name("audio-volume-muted-symbolic");
    mute.add_css_class("flat");
    mute.add_css_class("circular");
    controls.append(&mute);

    let fader = MeterFader::new(gain);
    fader.root.set_margin_end(6);
    controls.append(&fader.root);
    // What a cell shows: its channel's peak as it leaves the fader.
    fader.set_level(level * gain.powi(3));

    let unlink = gtk::Button::from_icon_name("list-remove-symbolic");
    unlink.add_css_class("flat");
    controls.append(&unlink);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.append(&controls);
    (card, fader)
}

fn main() -> gtk::glib::ExitCode {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "fader.png".to_owned());
    let app = adw::Application::builder()
        .application_id("dev._2c2t.PipedeckFaderRender")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        let provider = gtk::CssProvider::new();
        provider.load_from_string(meter_fader::CSS);
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("a display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
        column.set_margin_top(12);
        column.set_margin_bottom(12);
        column.set_margin_start(12);
        column.set_margin_end(12);
        let mut keep = Vec::new();
        // Settings and channel peaks: silent at full, a signal at full, a
        // quiet one halfway, the loudest there is at 0.8, nothing at zero,
        // and a wider one as the object windows have.
        for (gain, level, width) in [
            (1.0, 0.0, 250),
            (1.0, 0.5, 250),
            (0.5, 0.1, 250),
            (0.8, 1.0, 250),
            (0.0, 0.0, 250),
            (0.7, 0.3, 420),
        ] {
            let (card, fader) = row(gain, level, width);
            column.append(&card);
            keep.push(fader);
        }
        let meter = Meter::new();
        meter.root.set_size_request(200, -1);
        meter.set_level(0.4);
        column.append(&meter.root);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .content(&column)
            .build();
        window.present();

        let out = out.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
            let _keep = (&keep, &meter);
            // The window, not the column: the column has no background of
            // its own, and a transparent one reads as white in an image.
            let width = window.width();
            let height = window.height();
            let paintable = gtk::WidgetPaintable::new(Some(&window));
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
            if let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) {
                let texture = renderer.render_texture(node, None);
                match texture.save_to_png(&out) {
                    Ok(()) => println!("saved {out} ({width}x{height})"),
                    Err(e) => println!("cannot save {out}: {e}"),
                }
            } else {
                println!("nothing was drawn");
            }
            window.close();
        });
    });
    app.run_with_args::<&str>(&[])
}
