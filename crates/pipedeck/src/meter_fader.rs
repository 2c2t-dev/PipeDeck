//! A fader whose track shows the level going through it, and a level alone.
//!
//! This depends on GTK and libadwaita only, so a program of its own can draw
//! it: `examples/fader_render.rs` renders it to an image, which is how its
//! look is checked without starting the mixer.

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

/// Fader travel in UI units; a gain is `value / FADER_MAX`.
pub const FADER_MAX: f64 = 100.0;

/// The rules the track needs from the stylesheet: the scale's own trough is
/// made transparent, since the track is drawn under it.
pub const CSS: &str = "
    .pd-meter-fader trough,
    .pd-meter-fader trough highlight {
        background: none;
        box-shadow: none;
    }
";

/// The scale the fader is made of, as every fader here is.
fn scale(gain: f32) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, FADER_MAX, 1.0);
    scale.set_hexpand(true);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_digits(0);
    scale.set_value(f64::from(gain) * FADER_MAX);
    scale
}

/// A fader whose track shows the level going through it, as a mixer's does.
///
/// The fader is a plain `gtk::Scale` — dragging, the keyboard, scrolling and
/// what a screen reader says all stay GTK's — with its trough made
/// transparent. Underneath it the track is drawn here: faint all along, the
/// accent up to the knob so the setting reads in silence, and the accent at
/// full strength as far as the level reaches. The knob stays on top.
pub struct MeterFader {
    /// What goes in the layout.
    pub root: gtk::Overlay,
    /// The control itself, for its value and its signals.
    pub scale: gtk::Scale,
    area: gtk::DrawingArea,
    /// Where the level sits along the track, from 0 to 1.
    level: std::rc::Rc<std::cell::Cell<f64>>,
}

impl MeterFader {
    pub fn new(gain: f32) -> Self {
        let scale = scale(gain);
        scale.add_css_class("pd-meter-fader");

        let level = std::rc::Rc::new(std::cell::Cell::new(0.0));
        let area = gtk::DrawingArea::new();
        area.set_hexpand(true);
        area.set_draw_func({
            let scale = scale.clone();
            let level = level.clone();
            move |area, cr, _, _| draw_track(area, &scale, level.get(), cr)
        });
        scale.connect_value_changed({
            let area = area.clone();
            move |_| area.queue_draw()
        });

        let root = gtk::Overlay::new();
        root.set_hexpand(true);
        root.set_child(Some(&area));
        root.add_overlay(&scale);
        // The overlay is as big as the scale, so the area under it is too,
        // and the two share their coordinates.
        root.set_measure_overlay(&scale, true);

        Self {
            root,
            scale,
            area,
            level,
        }
    }

    /// Show a peak, as a linear amplitude.
    pub fn set_level(&self, peak: f32) {
        let position = meter_position(peak);
        if (position - self.level.get()).abs() > 1e-3 {
            self.level.set(position);
            self.area.queue_draw();
        }
    }
}

/// A level alone, drawn as a fader's track is, for what has no fader: a
/// channel bound to a microphone has nothing to trim.
pub struct Meter {
    pub root: gtk::DrawingArea,
    level: std::rc::Rc<std::cell::Cell<f64>>,
}

impl Meter {
    pub fn new() -> Self {
        let level = std::rc::Rc::new(std::cell::Cell::new(0.0));
        let root = gtk::DrawingArea::new();
        root.set_hexpand(true);
        root.set_content_height(THICKNESS as i32);
        root.set_draw_func({
            let level = level.clone();
            move |area, cr, width, height| {
                let y = f64::from(height) / 2.0 - THICKNESS / 2.0;
                draw_pills(area, cr, 0.0, y, f64::from(width), None, level.get());
            }
        });
        Self { root, level }
    }

    /// Show a peak, as a linear amplitude.
    pub fn set_level(&self, peak: f32) {
        let position = meter_position(peak);
        if (position - self.level.get()).abs() > 1e-3 {
            self.level.set(position);
            self.root.queue_draw();
        }
    }
}

/// The scale's trough, which is where the track is drawn: found among its
/// children by the name its style gives it, and measured in the coordinates
/// of the area the track is drawn on. Measured in the scale's own, it came
/// out off by the padding the theme puts around a scale — a track drawn a
/// dozen pixels up and to the left of its knob.
fn trough_bounds(scale: &gtk::Scale, area: &gtk::DrawingArea) -> Option<gtk::graphene::Rect> {
    let mut child = scale.first_child();
    while let Some(widget) = child {
        if widget.css_name() == "trough" {
            return widget.compute_bounds(area);
        }
        child = widget.next_sibling();
    }
    None
}

/// How thick the track is drawn: thicker than a bare trough, so the level
/// reads at a glance.
const THICKNESS: f64 = 6.0;

fn draw_track(area: &gtk::DrawingArea, scale: &gtk::Scale, level: f64, cr: &gtk::cairo::Context) {
    let (x, y, width) = match trough_bounds(scale, area) {
        Some(trough) => (
            f64::from(trough.x()),
            f64::from(trough.y()) + f64::from(trough.height()) / 2.0 - THICKNESS / 2.0,
            f64::from(trough.width()),
        ),
        // A GTK that lays its scale out otherwise: a track where one
        // usually is, rather than a knob floating over nothing, since the
        // trough itself is made transparent.
        None => {
            const KNOB: f64 = 10.0;
            const VALUE: f64 = 40.0;
            (
                KNOB,
                f64::from(area.height()) / 2.0 - THICKNESS / 2.0,
                (f64::from(area.width()) - 2.0 * KNOB - VALUE).max(0.0),
            )
        }
    };
    let fraction = (scale.value() - scale.adjustment().lower())
        / (scale.adjustment().upper() - scale.adjustment().lower()).max(f64::EPSILON);
    draw_pills(area, cr, x, y, width, Some(fraction), level);
}

/// The track itself: faint all along, the accent softly up to `setting`
/// when there is one, and at full strength as far as `level`.
fn draw_pills(
    area: &gtk::DrawingArea,
    cr: &gtk::cairo::Context,
    x: f64,
    y: f64,
    width: f64,
    setting: Option<f64>,
    level: f64,
) {
    let fg = area.color();
    let accent = adw::StyleManager::default().accent_color_rgba();
    let pill = |cr: &gtk::cairo::Context, length: f64| {
        let length = length.clamp(0.0, width);
        if length <= 0.0 {
            return;
        }
        let r = (THICKNESS / 2.0).min(length / 2.0);
        cr.new_sub_path();
        cr.arc(
            x + length - r,
            y + r,
            r,
            -std::f64::consts::FRAC_PI_2,
            std::f64::consts::FRAC_PI_2,
        );
        cr.arc(
            x + r,
            y + r,
            r,
            std::f64::consts::FRAC_PI_2,
            3.0 * std::f64::consts::FRAC_PI_2,
        );
        cr.close_path();
        let _ = cr.fill();
    };

    // The whole travel, faint.
    cr.set_source_rgba(fg.red().into(), fg.green().into(), fg.blue().into(), 0.15);
    pill(cr, width);
    // As far as the fader is set, so the setting shows in silence.
    if let Some(setting) = setting {
        cr.set_source_rgba(
            accent.red().into(),
            accent.green().into(),
            accent.blue().into(),
            0.35,
        );
        pill(cr, width * setting);
    }
    // As far as the level reaches.
    cr.set_source_rgba(
        accent.red().into(),
        accent.green().into(),
        accent.blue().into(),
        1.0,
    );
    pill(cr, width * level);
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
