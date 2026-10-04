//! What a key, or a dial's part of the touch strip, shows: the badge the
//! mixer draws for the object, its level around it or beside it, whether it
//! is muted or heard, and its name. Drawn as SVG, which OpenDeck renders
//! itself: a key through its window, the strip through its layout.

use std::f32::consts::PI;
use std::fmt::Write as _;

use crate::presets;

/// The mixer's icons, as `crates/pipedeck/icons` has them.
macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/pipedeck/icons/",
            $name,
            ".svg"
        )))),*]
    };
}

const ICONS: &[(&str, &str)] = icons!(
    "pd-1-symbolic",
    "pd-2-symbolic",
    "pd-3-symbolic",
    "pd-4-symbolic",
    "pd-5-symbolic",
    "pd-aux-symbolic",
    "pd-browser-symbolic",
    "pd-fun-symbolic",
    "pd-game-symbolic",
    "pd-headset-symbolic",
    "pd-listen-symbolic",
    "pd-mic-symbolic",
    "pd-music-symbolic",
    "pd-people-symbolic",
    "pd-record-symbolic",
    "pd-sfx-symbolic",
    "pd-speaker-symbolic",
    "pd-star-symbolic",
    "pd-stream-symbolic",
    "pd-system-symbolic",
    "pd-video-symbolic",
    "pd-voice-symbolic",
);

/// A mix's badge, white, as the mixer draws its columns.
pub const WHITE: &str = "#f2f2f2";
/// The glyph on a badge, and a muted badge.
const GLYPH: &str = "#12141a";
const GREY: &str = "#5e5c64";
pub const RED: &str = "#ed333b";
const TEXT: &str = "#ffffff";
const FAINT: &str = "#a0a0a0";
const TRACK: &str = "rgba(255,255,255,0.2)";
const LEVEL: &str = "rgba(255,255,255,0.9)";

/// The outline of an icon: the path its file draws.
fn glyph(icon: &str) -> &'static str {
    let file = ICONS
        .iter()
        .find(|(name, _)| *name == icon)
        .or_else(|| {
            ICONS
                .iter()
                .find(|(name, _)| *name == "pd-speaker-symbolic")
        })
        .map_or("", |(_, file)| file);
    file.split_once(" d=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map_or("", |(d, _)| d)
}

/// The icon and colour an object wears, falling back as the mixer does.
pub fn look(key: Option<&str>, is_input: bool, mix: bool) -> (&'static str, &'static str) {
    let preset = presets::look(key, is_input);
    (preset.icon, if mix { WHITE } else { preset.color })
}

/// What a badge says beyond its look.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct State {
    pub muted: bool,
    /// Drawn faint when off: a mix not heard, a device not listened on.
    pub dim: bool,
}

/// A rounded square of a colour with the icon on it, at `x, y`.
fn badge(svg: &mut String, icon: &str, color: &str, x: f32, y: f32, side: f32, state: State) {
    let fill = if state.muted { GREY } else { color };
    let opacity = if state.dim { 0.35 } else { 1.0 };
    let inner = side * 0.62;
    let pad = (side - inner) / 2.0;
    let _ = write!(
        svg,
        r#"<g opacity="{opacity}"><rect x="{x}" y="{y}" width="{side}" height="{side}" rx="{rx}" fill="{fill}"/><svg x="{gx}" y="{gy}" width="{inner}" height="{inner}" viewBox="0 -960 960 960"><path d="{d}" fill="{GLYPH}"/></svg>"#,
        rx = side * 0.22,
        gx = x + pad,
        gy = y + pad,
        d = glyph(icon),
    );
    if state.muted {
        let (x0, y0, x1, y1) = (
            x + side * 0.12,
            y + side * 0.88,
            x + side * 0.88,
            y + side * 0.12,
        );
        let width = side * 0.09;
        let _ = write!(
            svg,
            r#"<line x1="{x0}" y1="{y0}" x2="{x1}" y2="{y1}" stroke="{GLYPH}" stroke-width="{outer}" stroke-linecap="round"/><line x1="{x0}" y1="{y0}" x2="{x1}" y2="{y1}" stroke="{RED}" stroke-width="{width}" stroke-linecap="round"/>"#,
            outer = width * 2.2,
        );
    }
    svg.push_str("</g>");
}

/// An arc around `cx, cy`, from the bottom left round to `part` of the way
/// to the bottom right, like a knob's travel.
fn arc(cx: f32, cy: f32, r: f32, part: f32) -> String {
    let start = 135.0_f32.to_radians();
    let sweep = 270.0 * part.clamp(0.0, 1.0);
    let end = start + sweep.to_radians();
    let point = |a: f32| (cx + r * a.cos(), cy + r * a.sin());
    let (x0, y0) = point(start);
    let (x1, y1) = point(end.min(start + 2.0 * PI - 0.001));
    let large = u8::from(sweep > 180.0);
    format!("M {x0:.2} {y0:.2} A {r} {r} 0 {large} 1 {x1:.2} {y1:.2}")
}

fn text(svg: &mut String, x: f32, y: f32, size: f32, anchor: &str, color: &str, words: &str) {
    let _ = write!(
        svg,
        r#"<text x="{x}" y="{y}" font-family="sans-serif" font-size="{size}" font-weight="600" fill="{color}" text-anchor="{anchor}">{}</text>"#,
        escape(words),
    );
}

/// A key: the name above, the badge in the middle, ringed by the level
/// when there is one, and what it is doing below.
pub fn key(
    name: &str,
    (icon, color): (&str, &str),
    level: Option<f32>,
    state: State,
    below: (&str, &str),
) -> String {
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144"><rect width="144" height="144" fill="#000000"/>"##,
    );
    text(
        &mut svg,
        72.0,
        25.0,
        18.0,
        "middle",
        TEXT,
        &shorten(name, 13),
    );
    let side = if level.is_some() { 50.0 } else { 62.0 };
    let (cx, cy) = (72.0, 76.0);
    if let Some(level) = level {
        let r = side * 0.72 + 6.0;
        let _ = write!(
            svg,
            r#"<path d="{}" fill="none" stroke="{TRACK}" stroke-width="7" stroke-linecap="round"/>"#,
            arc(cx, cy, r, 1.0)
        );
        if level > 0.0 {
            let fill = if state.muted { RED } else { LEVEL };
            let _ = write!(
                svg,
                r#"<path d="{}" fill="none" stroke="{fill}" stroke-width="7" stroke-linecap="round"/>"#,
                arc(cx, cy, r, level)
            );
        }
    }
    badge(
        &mut svg,
        icon,
        color,
        cx - side / 2.0,
        cy - side / 2.0,
        side,
        state,
    );
    text(&mut svg, 72.0, 136.0, 18.0, "middle", below.1, below.0);
    svg.push_str("</svg>");
    svg
}

/// A dial's part of the touch strip: the badge on the left, the name and
/// the level as a bar beside it, and what it is doing under them.
pub fn strip(
    name: &str,
    (icon, color): (&str, &str),
    level: Option<f32>,
    state: State,
    below: (&str, &str),
) -> String {
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100"><rect width="200" height="100" fill="#000000"/>"##,
    );
    badge(&mut svg, icon, color, 12.0, 27.0, 46.0, state);
    let left = 70.0;
    match level {
        Some(level) => {
            text(
                &mut svg,
                left,
                38.0,
                16.0,
                "start",
                TEXT,
                &shorten(name, 14),
            );
            let _ = write!(
                svg,
                r#"<rect x="{left}" y="48" width="118" height="8" rx="4" fill="{TRACK}"/>"#
            );
            let filled = 118.0 * level.clamp(0.0, 1.0);
            if filled > 0.0 {
                let fill = if state.muted { RED } else { LEVEL };
                let _ = write!(
                    svg,
                    r#"<rect x="{left}" y="48" width="{filled:.1}" height="8" rx="4" fill="{fill}"/>"#
                );
            }
            text(&mut svg, left, 78.0, 15.0, "start", below.1, below.0);
        }
        None => {
            text(
                &mut svg,
                left,
                46.0,
                16.0,
                "start",
                TEXT,
                &shorten(name, 14),
            );
            text(&mut svg, left, 70.0, 15.0, "start", below.1, below.0);
        }
    }
    svg.push_str("</svg>");
    svg
}

/// A key that has nothing to show yet, and why.
pub fn waiting(name: &str, why: &str, strip_sized: bool) -> String {
    let look = ("pd-speaker-symbolic", GREY);
    let state = State {
        muted: false,
        dim: true,
    };
    if strip_sized {
        strip(name, look, None, state, (why, FAINT))
    } else {
        key(name, look, None, state, (why, FAINT))
    }
}

/// The level as the mixer writes it.
pub fn percent(level: f32) -> String {
    format!("{}%", (level * 100.0).round())
}

/// A data URL a key's image can be set to. In base64: OpenDeck writes a
/// key's image to a file when it saves the profile, and only decodes it
/// first when it is.
pub fn data_url(svg: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut url = String::from("data:image/svg+xml;base64,");
    for chunk in svg.as_bytes().chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        for i in 0..4 {
            if i <= chunk.len() {
                url.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                url.push('=');
            }
        }
    }
    url
}

/// The icons OpenDeck lists the plugin and its actions with.
pub fn catalogue_icons() -> Vec<(&'static str, String)> {
    let badge_only = |icon: &str, color: &str, state: State| {
        let mut svg = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">"#,
        );
        badge(&mut svg, icon, color, 16.0, 16.0, 112.0, state);
        svg.push_str("</svg>");
        svg
    };
    let plain = State::default();
    vec![
        ("plugin", badge_only("pd-listen-symbolic", WHITE, plain)),
        ("category", badge_only("pd-listen-symbolic", WHITE, plain)),
        (
            "mute",
            badge_only(
                "pd-speaker-symbolic",
                "#4cc26a",
                State {
                    muted: true,
                    dim: false,
                },
            ),
        ),
        (
            "volume",
            badge_only("pd-speaker-symbolic", "#4cc26a", plain),
        ),
        ("hear", badge_only("pd-listen-symbolic", WHITE, plain)),
        (
            "output",
            badge_only("pd-headset-symbolic", "#3ba7c9", plain),
        ),
    ]
}

fn shorten(words: &str, length: usize) -> String {
    if words.chars().count() <= length {
        return words.to_owned();
    }
    let mut short: String = words.chars().take(length - 1).collect();
    short.push('…');
    short
}

fn escape(words: &str) -> String {
    let mut escaped = String::with_capacity(words.len());
    for c in words.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_look_has_its_icon() {
        for preset in presets::PRESETS {
            assert!(
                ICONS.iter().any(|(name, _)| *name == preset.icon),
                "{} is not drawn",
                preset.icon
            );
            assert!(glyph(preset.icon).starts_with('M'), "{}", preset.icon);
        }
    }

    #[test]
    fn a_name_cannot_break_the_picture() {
        let svg = key(
            "<a & \"b\">",
            ("pd-music-symbolic", "#e35db5"),
            Some(0.5),
            State::default(),
            ("50%", TEXT),
        );
        assert!(svg.contains("&lt;a &amp; &quot;b&quot;&gt;"));
        assert!(!data_url(&svg).contains(['<', '#', '"']));
    }

    #[test]
    fn a_picture_is_carried_in_base64() {
        let url = |svg: &str| data_url(svg).replace("data:image/svg+xml;base64,", "");
        assert_eq!(url("<svg/>"), "PHN2Zy8+");
        assert_eq!(url("<svg />"), "PHN2ZyAvPg==");
        assert_eq!(url("<svg  />"), "PHN2ZyAgLz4=");
    }

    #[test]
    fn a_long_name_is_cut_short() {
        assert_eq!(shorten("Blue Microphones Stéréo", 6), "Blue …");
        assert_eq!(shorten("Music", 6), "Music");
    }
}
