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

/// A meter bar, thin enough to sit under a fader without crowding it.
pub fn meter() -> gtk::LevelBar {
    let bar = gtk::LevelBar::new();
    bar.set_mode(gtk::LevelBarMode::Continuous);
    bar.set_min_value(0.0);
    bar.set_max_value(1.0);
    bar.set_value(0.0);
    bar.set_height_request(4);
    bar.add_css_class("pd-meter");
    bar
}

/// Where a peak sits on a meter.
///
/// Peaks are linear amplitudes, and a bar drawn straight from one spends
/// most of its length on sounds nobody calls loud. The cube root spreads it
/// the way the faders are spread, so a bar at half length means a fader at
/// half travel.
pub fn meter_position(peak: f32) -> f64 {
    f64::from(peak.clamp(0.0, 1.0)).cbrt()
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
        Some(icon) if !icon.is_empty() => {
            gtk::Image::from_icon_name(known_icon(icon, "application-x-executable-symbolic"))
        }
        _ => gtk::Image::from_icon_name("application-x-executable-symbolic"),
    };
    image.set_pixel_size(size);
    image
}

/// An icon name the running theme can actually draw.
///
/// Icon sets disagree on what they carry: Breeze has no `web-browser`, and
/// a name it lacks is drawn as a broken image rather than ignored.
pub fn known_icon<'a>(name: &'a str, fallback: &'a str) -> &'a str {
    let Some(display) = gtk::gdk::Display::default() else {
        return name;
    };
    if gtk::IconTheme::for_display(&display).has_icon(name) {
        name
    } else {
        log::debug!("the icon theme has no {name}, falling back to {fallback}");
        fallback
    }
}

/// An icon in a rounded badge, coloured by the preset it belongs to.
pub fn badge(icon_name: &str, preset: Option<&str>, size: i32) -> gtk::Image {
    let icon = gtk::Image::from_icon_name(known_icon(icon_name, "audio-speakers-symbolic"));
    icon.set_pixel_size(size);
    icon.add_css_class("pd-badge");
    if let Some(class) = crate::presets::badge_class(preset) {
        icon.add_css_class(&class);
    }
    // Without this the image stretches to the height of its row and the
    // badge stops being a square.
    icon.set_valign(gtk::Align::Center);
    icon.set_halign(gtk::Align::Center);
    icon
}

/// The large icon that stands for the object a window is about.
pub fn big_badge(icon_name: &str, preset: Option<&str>) -> gtk::Image {
    let icon = gtk::Image::from_icon_name(known_icon(icon_name, "audio-speakers-symbolic"));
    icon.set_pixel_size(72);
    icon.add_css_class("pd-badge-large");
    if let Some(class) = crate::presets::badge_class(preset) {
        icon.add_css_class(&class);
    }
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon
}

/// Point a badge at another look, dropping the colour it had.
pub fn set_badge_look(badge: &gtk::Image, icon_name: &str, preset: Option<&str>, fallback: &str) {
    for look in crate::presets::PRESETS {
        badge.remove_css_class(&format!("pd-badge-{}", look.key));
    }
    badge.set_icon_name(Some(known_icon(icon_name, fallback)));
    if let Some(class) = crate::presets::badge_class(preset) {
        badge.add_css_class(&class);
    }
}

/// The "..." button that opens the icon picker of an object window.
///
/// Picking a look is not renaming or re-routing anything, so it sits out of
/// the way behind a menu rather than taking room next to the level.
pub fn look_menu(
    current: Option<&str>,
    on_pick: impl Fn(Option<String>) + 'static,
) -> gtk::MenuButton {
    const COLUMNS: i32 = 5;

    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(6);
    grid.set_margin_top(6);
    grid.set_margin_bottom(6);
    grid.set_margin_start(6);
    grid.set_margin_end(6);

    let heading = gtk::Label::new(Some("Select an icon"));
    heading.add_css_class("caption-heading");
    heading.set_margin_bottom(6);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&heading);
    content.append(&grid);

    let popover = gtk::Popover::new();
    popover.set_child(Some(&content));

    let on_pick = std::rc::Rc::new(on_pick);
    for (position, look) in crate::presets::PRESETS.iter().enumerate() {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_tooltip_text(Some(look.label));
        button.set_child(Some(&badge(look.icon, Some(look.key), 16)));
        if current == Some(look.key) {
            button.add_css_class("suggested-action");
        }
        button.connect_clicked({
            let on_pick = on_pick.clone();
            let popover = popover.clone();
            let key = look.key;
            move |_| {
                on_pick(Some(key.to_owned()));
                popover.popdown();
            }
        });
        let position = position as i32;
        grid.attach(&button, position % COLUMNS, position / COLUMNS, 1, 1);
    }

    let button = gtk::MenuButton::new();
    button.set_icon_name(known_icon(
        "view-more-horizontal-symbolic",
        "view-more-symbolic",
    ));
    button.add_css_class("flat");
    button.set_tooltip_text(Some("Choose an icon"));
    button.set_halign(gtk::Align::Center);
    button.set_popover(Some(&popover));
    button
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
