//! Small widgets the two object windows share.

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

/// Fader travel in UI units; a gain is `value / FADER_MAX`.
pub const FADER_MAX: f64 = 100.0;

pub fn fader(gain: f32) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, FADER_MAX, 1.0);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_digits(0);
    scale.set_value(f64::from(gain) * FADER_MAX);
    scale
}

pub fn mute_button(muted: bool, tooltip: &str) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::new();
    button.set_icon_name("audio-volume-muted-symbolic");
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_active(muted);
    button
}

/// The mute and the fader side by side.
pub fn level_row(mute: &gtk::ToggleButton, fader: &gtk::Scale) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(mute);
    row.append(fader);
    row
}

/// An application icon, from the icon theme or from a path when a desktop
/// entry gives one. Falls back to a neutral glyph.
pub fn app_icon(icon: Option<&str>, size: i32) -> gtk::Image {
    let image = match icon {
        Some(icon) if icon.starts_with('/') && std::path::Path::new(icon).exists() => {
            gtk::Image::from_file(icon)
        }
        Some(icon) if !icon.is_empty() => gtk::Image::from_icon_name(icon),
        _ => gtk::Image::from_icon_name("application-x-executable-symbolic"),
    };
    image.set_pixel_size(size);
    image
}

/// The large icon that stands for the object a window is about.
pub fn big_badge(icon_name: &str) -> gtk::Image {
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(72);
    icon.add_css_class("pd-badge-large");
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon
}

/// The name field at the top of an object window.
pub fn name_entry(name: &str) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.add_css_class("title-4");
    entry.set_hexpand(true);
    entry.set_text(name);
    entry
}

/// A card holding one row of a window's right-hand list.
pub fn list_card() -> (gtk::Box, gtk::Box) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("card");
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 4);
    inner.set_margin_top(10);
    inner.set_margin_bottom(10);
    inner.set_margin_start(10);
    inner.set_margin_end(10);
    card.append(&inner);
    (card, inner)
}

/// Title line of a list card: a label, then trailing widgets.
pub fn card_title(label: &str) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let title = gtk::Label::new(Some(label));
    title.set_hexpand(true);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&title);
    (row, title)
}
