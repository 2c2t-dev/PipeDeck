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
    "pd-back-symbolic",
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
pub const FAINT: &str = "#a0a0a0";
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
        slash(svg, x, y, side);
    }
    svg.push_str("</g>");
}

/// The red line across a muted badge.
fn slash(svg: &mut String, x: f32, y: f32, side: f32) {
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

/// What goes over a picture, a person's, in place of a badge: grey when
/// muted, faint when off, and the red line.
fn over_picture(svg: &mut String, x: f32, y: f32, side: f32, state: State) {
    if state.muted || state.dim {
        let shade = if state.dim { 0.65 } else { 0.5 };
        let _ = write!(
            svg,
            r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="rgba(0,0,0,{shade})"/>"#,
            cx = x + side / 2.0,
            cy = y + side / 2.0,
            r = side / 2.0,
        );
    }
    if state.muted {
        slash(svg, x, y, side);
    }
}

/// An arc around `cx, cy`, from the bottom left round to `part` of the way
/// to the bottom right, like a knob's travel.
fn arc(cx: f32, cy: f32, r: f32, part: f32) -> String {
    arc_between(cx, cy, r, 0.0, part)
}

/// The part of that travel from `from` to `to`.
fn arc_between(cx: f32, cy: f32, r: f32, from: f32, to: f32) -> String {
    let at = |part: f32| (135.0 + 270.0 * part.clamp(0.0, 1.0)).to_radians();
    let (start, end) = (at(from), at(to));
    let point = |a: f32| (cx + r * a.cos(), cy + r * a.sin());
    let (x0, y0) = point(start);
    let (x1, y1) = point(end.min(start + 2.0 * PI - 0.001));
    let large = u8::from(end - start > PI);
    format!("M {x0:.2} {y0:.2} A {r} {r} 0 {large} 1 {x1:.2} {y1:.2}")
}

/// A meter's colours, by where it reads: green, then yellow from 6 dB
/// under full scale, red from 1 dB under it.
const METER: [(f32, f32, &str); 3] = [
    (0.0, 0.794, "#57e389"),
    (0.794, 0.962, "#f6d32d"),
    (0.962, 1.0, "#ed333b"),
];

/// Where a peak sits on a meter, as the mixer's own meters put it: the
/// cube root spreads it as the faders are spread.
pub fn meter_position(peak: f32) -> f32 {
    peak.clamp(0.0, 1.0).cbrt()
}

fn text(svg: &mut String, x: f32, y: f32, size: f32, anchor: &str, color: &str, words: &str) {
    let _ = write!(
        svg,
        r#"<text x="{x}" y="{y}" font-family="sans-serif" font-size="{size}" font-weight="600" fill="{color}" text-anchor="{anchor}">{}</text>"#,
        escape(words),
    );
}

/// What a key or a dial shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Picture<'a> {
    pub name: &'a str,
    /// The badge's icon and colour.
    pub look: (&'a str, &'a str),
    /// The icon of the mix a channel's level is taken in, worn in the
    /// badge's corner, as Wave Link marks a level by its mix.
    pub corner: Option<&'a str>,
    pub level: Option<f32>,
    /// What the meter reads, as a position from 0 to 1, when it is shown.
    pub meter: Option<f32>,
    pub state: State,
    /// What it is doing, and in what colour.
    pub below: (&'a str, &'a str),
    /// A person's picture, round, as a PNG data URL, worn in place of the
    /// badge.
    pub avatar: Option<&'a str>,
}

/// The badge, and the mix's in its corner when there is one; or the
/// person's picture.
fn badges(svg: &mut String, picture: &Picture, x: f32, y: f32, side: f32) {
    if let Some(avatar) = picture.avatar {
        let _ = write!(
            svg,
            r#"<image x="{x}" y="{y}" width="{side}" height="{side}" href="{avatar}"/>"#
        );
        over_picture(svg, x, y, side, picture.state);
        return;
    }
    badge(
        svg,
        picture.look.0,
        picture.look.1,
        x,
        y,
        side,
        picture.state,
    );
    if let Some(icon) = picture.corner {
        let small = side * 0.5;
        let (cx, cy) = (x + side - small * 0.6, y + side - small * 0.6);
        let _ = write!(
            svg,
            r##"<rect x="{bx}" y="{by}" width="{b}" height="{b}" rx="{r}" fill="#000000"/>"##,
            bx = cx - 2.5,
            by = cy - 2.5,
            b = small + 5.0,
            r = (small + 5.0) * 0.22,
        );
        let state = State {
            muted: false,
            dim: picture.state.dim,
        };
        badge(svg, icon, WHITE, cx, cy, small, state);
    }
}

/// A key: the name above, the badge in the middle, ringed by the level
/// when there is one, and what it is doing below.
pub fn key(picture: &Picture) -> String {
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
        &shorten(picture.name, 13),
    );
    let side = if picture.level.is_some() { 50.0 } else { 62.0 };
    let (cx, cy) = (72.0, 76.0);
    if let Some(level) = picture.level {
        let r = side * 0.72 + 6.0;
        let _ = write!(
            svg,
            r#"<path d="{}" fill="none" stroke="{TRACK}" stroke-width="7" stroke-linecap="round"/>"#,
            arc(cx, cy, r, 1.0)
        );
        if level > 0.0 {
            let fill = if picture.state.muted { RED } else { LEVEL };
            let _ = write!(
                svg,
                r#"<path d="{}" fill="none" stroke="{fill}" stroke-width="7" stroke-linecap="round"/>"#,
                arc(cx, cy, r, level)
            );
        }
        if let Some(meter) = picture.meter {
            for (from, to, color) in METER {
                if meter > from {
                    let _ = write!(
                        svg,
                        r#"<path d="{}" fill="none" stroke="{color}" stroke-width="3"/>"#,
                        arc_between(cx, cy, r - 7.0, from, meter.min(to))
                    );
                }
            }
        }
    }
    badges(&mut svg, picture, cx - side / 2.0, cy - side / 2.0, side);
    let (words, color) = picture.below;
    text(&mut svg, 72.0, 136.0, 18.0, "middle", color, words);
    svg.push_str("</svg>");
    svg
}

/// A dial's part of the touch strip: the badge on the left, the name and
/// the level as a bar beside it, and what it is doing under them.
pub fn strip(picture: &Picture) -> String {
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100"><rect width="200" height="100" fill="#000000"/>"##,
    );
    // OpenDeck draws the strip without pictures inside it: a person's is a
    // layer of its own, and what goes over it another. See `strip_layers`.
    if picture.avatar.is_none() {
        badges(&mut svg, picture, 12.0, 27.0, 46.0);
    }
    let left = 70.0;
    let name = shorten(picture.name, 14);
    let (words, color) = picture.below;
    match picture.level {
        Some(level) => {
            text(&mut svg, left, 38.0, 16.0, "start", TEXT, &name);
            let _ = write!(
                svg,
                r#"<rect x="{left}" y="48" width="118" height="8" rx="4" fill="{TRACK}"/>"#
            );
            let filled = 118.0 * level.clamp(0.0, 1.0);
            match picture.meter {
                // The meter in the track, and the level as a handle on it.
                Some(meter) => {
                    for (from, to, color) in METER {
                        if meter > from {
                            let _ = write!(
                                svg,
                                r#"<rect x="{x:.1}" y="48" width="{w:.1}" height="8" fill="{color}"/>"#,
                                x = left + 118.0 * from,
                                w = 118.0 * (meter.min(to) - from),
                            );
                        }
                    }
                    let fill = if picture.state.muted { RED } else { TEXT };
                    let _ = write!(
                        svg,
                        r##"<rect x="{x:.1}" y="43" width="5" height="18" rx="2.5" fill="{fill}" stroke="#000000" stroke-width="1.5"/>"##,
                        x = left + filled - 2.5,
                    );
                }
                None if filled > 0.0 => {
                    let fill = if picture.state.muted { RED } else { LEVEL };
                    let _ = write!(
                        svg,
                        r#"<rect x="{left}" y="48" width="{filled:.1}" height="8" rx="4" fill="{fill}"/>"#
                    );
                }
                None => {}
            }
            text(&mut svg, left, 78.0, 15.0, "start", color, words);
        }
        None => {
            text(&mut svg, left, 46.0, 16.0, "start", TEXT, &name);
            text(&mut svg, left, 70.0, 15.0, "start", color, words);
        }
    }
    svg.push_str("</svg>");
    svg
}

/// A picture of nothing, for a layer of the strip with nothing to show.
pub const NOTHING: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="46" height="46" viewBox="0 0 46 46"><rect width="46" height="46" fill="none"/></svg>"#;

/// Where a person's picture sits on the strip, as `plugin/layouts/strip.json`
/// puts its layers.
pub const STRIP_PICTURE: (f32, f32, f32) = (12.0, 27.0, 46.0);

/// What goes over a person's picture on the strip: an SVG the size of the
/// picture, empty but for the grey and the red line.
pub fn strip_over(picture: &Picture) -> String {
    let side = STRIP_PICTURE.2;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}" viewBox="0 0 {side} {side}"><rect width="{side}" height="{side}" fill="none"/>"#
    );
    over_picture(&mut svg, 0.0, 0.0, side, picture.state);
    svg.push_str("</svg>");
    svg
}

/// A key that has nothing to show yet, and why.
pub fn waiting(name: &str, why: &str, strip_sized: bool) -> String {
    let picture = Picture {
        name,
        look: ("pd-speaker-symbolic", GREY),
        corner: None,
        level: None,
        meter: None,
        state: State {
            muted: false,
            dim: true,
        },
        below: (why, FAINT),
        avatar: None,
    };
    if strip_sized {
        strip(&picture)
    } else {
        key(&picture)
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
    format!("data:image/svg+xml;base64,{}", base64(svg.as_bytes()))
}

/// Bytes in base64, as a data URL carries them.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut url = String::new();
    for chunk in bytes.chunks(3) {
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
        ("channel", badge_only("pd-music-symbolic", "#e35db5", plain)),
        ("mix", badge_only("pd-speaker-symbolic", WHITE, plain)),
        ("voice", badge_only("pd-people-symbolic", "#5b8cf5", plain)),
        ("effect", badge_only("pd-sfx-symbolic", "#f08a24", plain)),
        ("app", badge_only("pd-browser-symbolic", "#9b6ef3", plain)),
        ("monitor", badge_only("pd-listen-symbolic", WHITE, plain)),
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
        let svg = key(&Picture {
            name: "<a & \"b\">",
            look: ("pd-music-symbolic", "#e35db5"),
            corner: Some("pd-stream-symbolic"),
            level: Some(0.5),
            meter: Some(0.9),
            state: State::default(),
            below: ("50%", TEXT),
            avatar: None,
        });
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
