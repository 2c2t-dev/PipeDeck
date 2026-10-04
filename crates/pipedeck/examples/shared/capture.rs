//! Drawing a window to an image, for the examples that make pictures.

use adw::gtk::{self, gsk};
use adw::prelude::*;
use libadwaita as adw;

/// Write what a window shows to a PNG, `scale` times its size. Drawn by
/// Cairo, which every backend has, Broadway's included.
pub fn save(window: &gtk::Window, path: &std::path::Path, scale: f32) {
    let out = path.display();
    let (width, height) = (window.width() as f32, window.height() as f32);
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    snapshot.scale(scale, scale);
    paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
    let Some(node) = snapshot.to_node() else {
        println!("nothing drawn for {out}");
        return;
    };
    let renderer = gsk::CairoRenderer::new();
    if let Err(e) = renderer.realize_for_display(&WidgetExt::display(window)) {
        println!("cannot draw {out}: {e}");
        return;
    }
    let area = gtk::graphene::Rect::new(0.0, 0.0, width * scale, height * scale);
    match renderer.render_texture(node, Some(&area)).save_to_png(path) {
        Ok(()) => println!("saved {out}"),
        Err(e) => println!("cannot save {out}: {e}"),
    }
    renderer.unrealize();
}
