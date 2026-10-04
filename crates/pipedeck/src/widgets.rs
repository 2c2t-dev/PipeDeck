//! Small widgets the two object windows share.

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

/// Fader travel in UI units; a gain is `value / FADER_MAX`.
pub const FADER_MAX: f64 = 100.0;

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
pub use crate::meter_fader::{Meter, MeterFader};

/// The mute and the fader side by side.
pub fn level_row(mute: &gtk::ToggleButton, fader: &impl IsA<gtk::Widget>) -> gtk::Box {
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

/// Whether a badge wears the colour of its look.
///
/// Channels are colour, so a row is picked out at a glance. Mixes are white:
/// there are at most five of them, they sit in their own row, and colouring
/// them too would leave nothing plain for the eye to rest on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Colour,
    White,
}

/// An icon in a rounded badge.
///
/// The look is resolved here, so an object that never chose one still gets a
/// badge from the catalogue rather than an icon of its own.
pub fn badge(key: Option<&str>, is_input: bool, tone: Tone, size: i32) -> gtk::Image {
    let look = crate::presets::look(key, is_input);
    let icon = gtk::Image::from_icon_name(known_icon(look.icon, "pd-speaker-symbolic"));
    icon.set_pixel_size(size);
    icon.add_css_class("pd-badge");
    if tone == Tone::Colour {
        if let Some(class) = crate::presets::badge_class(Some(look.key)) {
            icon.add_css_class(&class);
        }
    }
    // Without this the image stretches to the height of its row and the
    // badge stops being a square.
    icon.set_valign(gtk::Align::Center);
    icon.set_halign(gtk::Align::Center);
    icon
}

/// The large icon that stands for the object a window is about.
pub fn big_badge(key: Option<&str>, is_input: bool, tone: Tone) -> gtk::Image {
    let look = crate::presets::look(key, is_input);
    let icon = gtk::Image::from_icon_name(known_icon(look.icon, "pd-speaker-symbolic"));
    icon.set_pixel_size(72);
    icon.add_css_class("pd-badge-large");
    if tone == Tone::Colour {
        if let Some(class) = crate::presets::badge_class(Some(look.key)) {
            icon.add_css_class(&class);
        }
    }
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon
}

/// Point a badge at another look, dropping the colour it had.
pub fn set_badge_look(badge: &gtk::Image, key: Option<&str>, is_input: bool, tone: Tone) {
    for look in crate::presets::PRESETS {
        badge.remove_css_class(&format!("pd-badge-{}", look.key));
    }
    let look = crate::presets::look(key, is_input);
    badge.set_icon_name(Some(known_icon(look.icon, "pd-speaker-symbolic")));
    if tone == Tone::Colour {
        if let Some(class) = crate::presets::badge_class(Some(look.key)) {
            badge.add_css_class(&class);
        }
    }
}

/// The "..." button that opens the icon picker of an object window.
///
/// Picking a look is not renaming or re-routing anything, so it sits out of
/// the way behind a menu rather than taking room next to the level.
///
/// A mix gets a grid: its looks are mostly numbers and it wears no colour,
/// so the glyphs are the whole story. A channel gets a named list: its looks
/// stand for what plays through it, and "Voice chat" says that where a
/// speech bubble only hints at it.
pub fn mix_look_menu(
    current: Option<&str>,
    on_pick: impl Fn(Option<String>) + 'static,
) -> gtk::MenuButton {
    const COLUMNS: i32 = 5;

    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(6);

    let popover = gtk::Popover::new();
    let on_pick = std::rc::Rc::new(on_pick);
    for (position, look) in crate::presets::PRESETS.iter().enumerate() {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_tooltip_text(Some(look.label));
        button.set_child(Some(&badge(Some(look.key), false, Tone::White, 16)));
        mark_current(&button, current == Some(look.key));
        button.connect_clicked(pick(&on_pick, &popover, look.key));
        let position = position as i32;
        grid.attach(&button, position % COLUMNS, position / COLUMNS, 1, 1);
    }

    popover.set_child(Some(&framed("Select an icon", &grid, false)));
    menu_button(&popover)
}

pub fn channel_look_menu(
    current: Option<&str>,
    is_input: bool,
    on_pick: impl Fn(Option<String>) + 'static,
) -> gtk::MenuButton {
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);

    let popover = gtk::Popover::new();
    let on_pick = std::rc::Rc::new(on_pick);
    let current = current.unwrap_or(crate::presets::default_key(is_input));
    for look in crate::presets::channel_looks() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&badge(Some(look.key), false, Tone::Colour, 16));
        row.append(
            &gtk::Label::builder()
                .label(look.label)
                .xalign(0.0)
                .hexpand(true)
                .build(),
        );

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_child(Some(&row));
        mark_current(&button, current == look.key);
        button.connect_clicked(pick(&on_pick, &popover, look.key));
        list.append(&button);
    }

    popover.set_child(Some(&framed("Select an icon", &list, true)));
    menu_button(&popover)
}

/// Heading and margins around the contents of a picker, scrolling when the
/// list is longer than a menu should be.
fn framed(heading: &str, content: &impl IsA<gtk::Widget>, scroll: bool) -> gtk::Box {
    let title = gtk::Label::new(Some(heading));
    title.add_css_class("caption-heading");
    title.set_margin_bottom(6);

    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.set_margin_top(6);
    frame.set_margin_bottom(6);
    frame.set_margin_start(6);
    frame.set_margin_end(6);
    frame.append(&title);
    if scroll {
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_max_content_height(360);
        scroller.set_propagate_natural_height(true);
        scroller.set_child(Some(content));
        frame.append(&scroller);
    } else {
        frame.append(content);
    }
    frame
}

fn mark_current(button: &gtk::Button, current: bool) {
    if current {
        button.add_css_class("suggested-action");
    }
}

fn pick(
    on_pick: &std::rc::Rc<impl Fn(Option<String>) + 'static>,
    popover: &gtk::Popover,
    key: &'static str,
) -> impl Fn(&gtk::Button) + 'static {
    let on_pick = on_pick.clone();
    let popover = popover.clone();
    move |_: &gtk::Button| {
        on_pick(Some(key.to_owned()));
        popover.popdown();
    }
}

fn menu_button(popover: &gtk::Popover) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_icon_name(known_icon(
        "view-more-horizontal-symbolic",
        "view-more-symbolic",
    ));
    button.add_css_class("flat");
    button.set_tooltip_text(Some("Choose an icon"));
    button.set_halign(gtk::Align::Center);
    button.set_popover(Some(popover));
    button
}

/// Is the user typing into this field? Its focus is on the text inside it,
/// not on the field itself, so the field asks after what it holds.
pub fn being_edited(widget: &impl IsA<gtk::Widget>) -> bool {
    widget
        .as_ref()
        .state_flags()
        .contains(gtk::StateFlags::FOCUS_WITHIN)
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
