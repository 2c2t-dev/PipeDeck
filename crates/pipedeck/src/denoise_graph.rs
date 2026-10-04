//! Noise suppression, drawn as what it does and set by dragging it.
//!
//! The graph is a voice speaking over a room: words, in colour, and between
//! and under them the room's noise, in grey. The noise is drawn as loud as
//! suppression leaves it, so raising the strength clears the gaps between
//! words while the words stay. One handle sets it: the noise's level,
//! dragged down to take more of it out.
//!
//! What RNNoise leaves of a noise depends on the noise; the graph takes it
//! to be [`RESIDUAL_DB`] at full strength, which is about what it does to a
//! fan or a hiss. The strength mixes that with the untouched sound, which
//! is exactly what the effect does.
//!
//! This depends on GTK, libadwaita and the engine's description only, so
//! `examples/effects_render.rs` can draw it to an image.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::dsp::{self, DENOISE_PRESETS};

/// What is left of a steady noise at full strength, about.
const RESIDUAL_DB: f32 = -30.0;
/// The level the noise is drawn at when nothing is taken out, and the
/// lowest the handle goes.
const TOP_DB: f32 = 0.0;
const BOTTOM_DB: f32 = -36.0;
/// How loud the room is drawn next to the voice, untouched.
const NOISE: f64 = 0.3;
const HANDLE: f64 = 8.0;
const REACH: f64 = 20.0;
const MARGIN: f64 = 12.0;

/// The voice's colour; the noise is the foreground's, faded.
const COLOUR: (f64, f64, f64) = (0.384, 0.627, 0.918);

type Changed = Rc<dyn Fn(&[f32])>;

pub struct DenoiseGraph {
    /// What goes in the layout: the presets, the graph, what it does, and
    /// the slider.
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    readout: gtk::Label,
    presets: gtk::MenuButton,
    scale: gtk::Scale,
    shown: gtk::Label,
    strength: Cell<f32>,
    /// Whether the pointer is on the handle, or dragging it.
    hovered: Cell<bool>,
    /// Set while the slider is given its value, so it does not send it
    /// back.
    syncing: Cell<bool>,
    changed: RefCell<Option<Changed>>,
    /// What it is doing right now: how sure it is of a voice, from 0 to 1,
    /// and how far it takes the room down, in decibels; minus infinity
    /// until it has said, falling back gently between readings.
    live: Cell<(f32, f32)>,
}

/// How much of what is drawn live each reading keeps: a meter's fall.
const LIVE_KEEP: f32 = 0.8;
/// How sure of a voice it has to be for the window to say it hears one.
const VOICE: f32 = 0.5;

impl DenoiseGraph {
    /// A graph showing `values`: the strength.
    pub fn new(values: &[f32]) -> Rc<Self> {
        let spec = dsp::spec("denoise").expect("noise suppression is described");
        let param = spec.params[0];
        let strength = values.first().copied().unwrap_or(param.default);

        let area = gtk::DrawingArea::new();
        area.set_content_height(170);
        area.set_hexpand(true);

        let readout = gtk::Label::new(None);
        readout.add_css_class("caption");
        readout.add_css_class("dim-label");
        readout.set_xalign(0.0);
        readout.set_wrap(true);
        readout.set_lines(2);

        let scale = gtk::Scale::with_range(
            gtk::Orientation::Horizontal,
            f64::from(param.min),
            f64::from(param.max),
            1.0,
        );
        scale.set_hexpand(true);

        let shown = gtk::Label::new(None);
        shown.add_css_class("caption");
        shown.add_css_class("numeric");
        shown.set_width_chars(8);
        shown.set_xalign(1.0);

        let this = Rc::new(Self {
            root: gtk::Box::new(gtk::Orientation::Vertical, 8),
            area,
            readout,
            presets: gtk::MenuButton::new(),
            scale,
            shown,
            strength: Cell::new(strength),
            hovered: Cell::new(false),
            syncing: Cell::new(false),
            changed: RefCell::new(None),
            live: Cell::new((f32::NEG_INFINITY, 0.0)),
        });
        this.build(param.label);
        this.wire();
        this.sync();
        this
    }

    /// What noise suppression is doing now: how sure it was of a voice
    /// since the last reading, and how far it took the room down.
    pub fn set_live(&self, voice: f32, removed: f32) {
        let (shown_voice, shown_removed) = self.live.get();
        let voice = voice.max(shown_voice * LIVE_KEEP);
        let removed = removed.max(shown_removed * LIVE_KEEP);
        if (voice, removed) != (shown_voice, shown_removed) {
            self.live.set((voice, removed));
            self.area.queue_draw();
        }
    }

    /// Called with the strength whenever it moves.
    pub fn connect_changed(&self, f: impl Fn(&[f32]) + 'static) {
        *self.changed.borrow_mut() = Some(Rc::new(f));
    }

    fn build(self: &Rc<Self>, label: &str) {
        self.presets.set_popover(Some(&self.preset_menu()));
        self.presets.set_halign(gtk::Align::Start);
        self.presets
            .set_tooltip_text(Some("Start from a ready-made setting"));
        self.root.append(&self.presets);
        self.root.append(&self.area);
        self.root.append(&self.readout);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&dot(COLOUR));
        let name = gtk::Label::new(Some(label));
        name.add_css_class("caption");
        name.set_xalign(0.0);
        name.set_width_chars(10);
        row.append(&name);
        row.append(&self.scale);
        row.append(&self.shown);
        self.scale.connect_value_changed({
            let this = Rc::downgrade(self);
            move |scale| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if !this.syncing.get() {
                    this.set(scale.value() as f32);
                }
            }
        });
        self.root.append(&row);
    }

    /// The list of presets, each with what it is for.
    fn preset_menu(self: &Rc<Self>) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for preset in DENOISE_PRESETS {
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let title = gtk::Label::new(Some(preset.name));
            title.set_xalign(0.0);
            labels.append(&title);
            let detail = gtk::Label::new(Some(preset.description));
            detail.add_css_class("caption");
            detail.add_css_class("dim-label");
            detail.set_xalign(0.0);
            labels.append(&detail);

            let button = gtk::Button::new();
            button.add_css_class("flat");
            button.set_child(Some(&labels));
            button.connect_clicked({
                let this = Rc::downgrade(self);
                let popover = popover.clone();
                move |_| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    popover.popdown();
                    this.set(preset.values[0]);
                }
            });
            list.append(&button);
        }
        popover.set_child(Some(&list));
        popover
    }

    fn set(&self, strength: f32) {
        self.strength.set(strength.clamp(0.0, 100.0));
        self.sync();
        self.area.queue_draw();
        let changed = self.changed.borrow().clone();
        if let Some(changed) = changed {
            changed(&[self.strength.get()]);
        }
    }

    /// Put the strength in the slider and name the preset it is.
    fn sync(&self) {
        let strength = self.strength.get();
        self.syncing.set(true);
        self.scale.set_value(f64::from(strength));
        self.shown.set_text(&format!("{strength:.0} %"));
        self.syncing.set(false);

        let preset = DENOISE_PRESETS
            .iter()
            .find(|preset| (preset.values[0] - strength).abs() < 1e-3)
            .map_or("Custom", |preset| preset.name);
        self.presets.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("view-list-symbolic")
                .label(format!("Preset: {preset}"))
                .build(),
        ));
        self.say();
    }

    fn wire(self: &Rc<Self>) {
        self.area.set_draw_func({
            let this = Rc::downgrade(self);
            move |_, cr, width, height| {
                if let Some(this) = this.upgrade() {
                    this.draw(cr, f64::from(width), f64::from(height));
                }
            }
        });

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let this = Rc::downgrade(self);
            move |_, x, y| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if this.dragging() {
                    return;
                }
                let near = this.near(x, y);
                if near != this.hovered.get() {
                    this.hovered.set(near);
                    this.say();
                    this.area.queue_draw();
                }
            }
        });
        motion.connect_leave({
            let this = Rc::downgrade(self);
            move |_| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if !this.dragging() {
                    this.hovered.set(false);
                    this.say();
                    this.area.queue_draw();
                }
            }
        });
        self.area.add_controller(motion);

        // The handle moves by as much as the pointer does, from where it
        // was.
        let drag = gtk::GestureDrag::new();
        let grabbed = Rc::new(Cell::new(None::<f64>));
        drag.connect_drag_begin({
            let this = Rc::downgrade(self);
            let grabbed = grabbed.clone();
            move |_, x, y| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let near = this.near(x, y);
                grabbed.set(near.then(|| this.handle().1));
                this.hovered.set(near);
                this.say();
                this.area.queue_draw();
            }
        });
        drag.connect_drag_update({
            let this = Rc::downgrade(self);
            let grabbed = grabbed.clone();
            move |_, _, dy| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if let Some(y) = grabbed.get() {
                    let db = y_to_db(y + dy, f64::from(this.area.height()));
                    this.hovered.set(true);
                    this.set(strength_for(db).round());
                }
            }
        });
        drag.connect_drag_end({
            let grabbed = grabbed.clone();
            move |_, _, _| grabbed.set(None)
        });
        self.area.add_controller(drag);

        // A double-click puts it back where it starts.
        let click = gtk::GestureClick::new();
        click.connect_pressed({
            let this = Rc::downgrade(self);
            move |_, presses, x, y| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if presses == 2 && this.near(x, y) {
                    let spec = dsp::spec("denoise").expect("noise suppression is described");
                    this.set(spec.params[0].default);
                }
            }
        });
        self.area.add_controller(click);
    }

    fn dragging(&self) -> bool {
        self.area
            .observe_controllers()
            .into_iter()
            .flatten()
            .filter_map(|c| c.downcast::<gtk::GestureDrag>().ok())
            .any(|g| g.is_active())
    }

    /// Where the handle is: at the right, as high as the noise is left.
    fn handle(&self) -> (f64, f64) {
        let width = f64::from(self.area.width());
        let height = f64::from(self.area.height());
        (
            width - MARGIN - HANDLE - 4.0,
            db_to_y(noise_left(self.strength.get()), height),
        )
    }

    fn near(&self, x: f64, y: f64) -> bool {
        let (hx, hy) = self.handle();
        (hx - x).hypot(hy - y) <= REACH
    }

    fn say(&self) {
        let strength = self.strength.get();
        let left = noise_left(strength);
        let text = if self.hovered.get() {
            format!(
                "Noise left · about {left:.0} dB · drag it down to take more out, up to keep more \
                 of the room"
            )
        } else if strength <= 0.0 {
            "Off: the sound goes through untouched.".to_owned()
        } else {
            format!(
                "The noise between and under the words is turned down about {:.0} dB; the voice \
                 is kept. It holds the sound back 10 ms.",
                -left
            )
        };
        self.readout.set_label(&text);
    }

    fn draw(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let fg = self.area.color();
        let set = |cr: &gtk::cairo::Context, alpha: f64| {
            cr.set_source_rgba(fg.red().into(), fg.green().into(), fg.blue().into(), alpha);
        };
        let colour = |cr: &gtk::cairo::Context, alpha: f64| {
            let (r, g, b) = COLOUR;
            cr.set_source_rgba(r, g, b, alpha);
        };

        rounded(cr, 0.0, 0.0, width, height, 10.0);
        set(cr, 0.04);
        let _ = cr.fill();

        // A voice over a room, drawn as bars mirrored about the middle: the
        // room under everything, as loud as suppression leaves it, and the
        // words over it.
        let middle = height / 2.0;
        let reach = middle - MARGIN;
        let left = 10f64.powf(f64::from(noise_left(self.strength.get())) / 20.0);
        let right_edge = width - MARGIN - 2.0 * HANDLE - 12.0;
        const STEP: f64 = 3.0;
        let bars = ((right_edge - MARGIN) / STEP) as usize;
        for bar in 0..bars {
            let x = MARGIN + bar as f64 * STEP;
            let at = bar as f64 / bars as f64;
            let noise = NOISE * left * (0.55 + 0.45 * jitter(bar, 1));
            let voice = speech(at) * (0.6 + 0.4 * jitter(bar, 2));
            if voice > 0.0 {
                let voice_height = reach * voice;
                cr.rectangle(x, middle - voice_height, STEP - 1.0, 2.0 * voice_height);
                colour(cr, 0.85);
                let _ = cr.fill();
            }
            // Over the words, so it shows in them as it does between them.
            let noise_height = reach * noise;
            cr.rectangle(x, middle - noise_height, STEP - 1.0, 2.0 * noise_height);
            set(cr, 0.45);
            let _ = cr.fill();
        }

        // Where the noise is, as a line out to the handle.
        let (hx, hy) = self.handle();
        cr.set_dash(&[4.0, 4.0], 0.0);
        cr.set_line_width(1.0);
        for y in [hy, 2.0 * middle - hy] {
            cr.move_to(MARGIN, y);
            cr.line_to(hx, y);
        }
        set(cr, 0.4);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);

        cr.set_font_size(10.0);
        set(cr, 0.5);
        cr.move_to(MARGIN + 2.0, MARGIN + 8.0);
        let _ = cr.show_text("Voice");
        cr.move_to(MARGIN + 2.0, height - MARGIN - 2.0);
        let _ = cr.show_text("Room noise");

        // What it hears now: a voice, lit, or the room and how far it goes
        // down, in the corner.
        let (voice, removed) = self.live.get();
        if voice.is_finite() {
            let hearing = voice >= VOICE;
            let text = if hearing {
                "Voice".to_owned()
            } else if removed > 0.5 {
                format!("Room −{removed:.0} dB")
            } else {
                "Room".to_owned()
            };
            if let Ok(extents) = cr.text_extents(&text) {
                let x = width - MARGIN - extents.width() - 2.0;
                if hearing {
                    // A light beside the word, lit while a voice is heard.
                    cr.new_sub_path();
                    cr.arc(x - 8.0, MARGIN + 4.5, 3.5, 0.0, std::f64::consts::TAU);
                    colour(cr, 1.0);
                    let _ = cr.fill();
                } else {
                    set(cr, 0.6);
                }
                cr.move_to(x, MARGIN + 8.0);
                let _ = cr.show_text(&text);
            }
        }

        let active = self.hovered.get();
        // The text leaves the pen where it ended; the handle starts afresh.
        cr.new_sub_path();
        cr.arc(
            hx,
            hy,
            if active { HANDLE + 2.0 } else { HANDLE },
            0.0,
            std::f64::consts::TAU,
        );
        set(cr, 0.8);
        let _ = cr.fill_preserve();
        cr.set_line_width(if active { 2.5 } else { 1.5 });
        colour(cr, if active { 0.95 } else { 0.6 });
        let _ = cr.stroke();
    }
}

/// How loud a steady noise is left, in decibels, at a strength: what
/// RNNoise leaves of it, mixed with the untouched sound.
fn noise_left(strength: f32) -> f32 {
    let s = strength.clamp(0.0, 100.0) / 100.0;
    let residual = 10f32.powf(RESIDUAL_DB / 20.0);
    20.0 * ((1.0 - s) + s * residual).max(1e-6).log10()
}

/// The strength that leaves a noise at `db`.
fn strength_for(db: f32) -> f32 {
    let residual = 10f32.powf(RESIDUAL_DB / 20.0);
    let left = 10f32.powf(db.clamp(RESIDUAL_DB, 0.0) / 20.0);
    ((1.0 - left) / (1.0 - residual) * 100.0).clamp(0.0, 100.0)
}

/// How loud the voice is at `at` across the graph: a few words, with
/// silence between them.
fn speech(at: f64) -> f64 {
    const WORDS: &[(f64, f64, f64)] = &[
        (0.06, 0.05, 0.55),
        (0.16, 0.07, 0.85),
        (0.27, 0.04, 0.6),
        (0.44, 0.08, 0.9),
        (0.55, 0.05, 0.7),
        (0.74, 0.06, 0.8),
        (0.85, 0.05, 0.5),
    ];
    let voice = WORDS
        .iter()
        .map(|(centre, spread, loud)| loud * (-((at - centre) / spread).powi(4)).exp())
        .fold(0.0, f64::max);
    if voice < 0.05 {
        0.0
    } else {
        voice
    }
}

/// A number between 0 and 1 that looks random, but is the same every time
/// the graph is drawn.
fn jitter(bar: usize, seed: u64) -> f64 {
    let mut x = (bar as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed.wrapping_mul(0xBF58_476D);
    x ^= x >> 31;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 29;
    (x % 1000) as f64 / 1000.0
}

/// A small disc of the voice's colour, beside the slider.
fn dot(colour: (f64, f64, f64)) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(10);
    area.set_content_height(10);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |_, cr, width, height| {
        let (r, g, b) = colour;
        cr.set_source_rgb(r, g, b);
        cr.arc(
            f64::from(width) / 2.0,
            f64::from(height) / 2.0,
            f64::from(width.min(height)) / 2.0,
            0.0,
            std::f64::consts::TAU,
        );
        let _ = cr.fill();
    });
    area
}

// --- geometry -------------------------------------------------------------

/// Where the noise's level is drawn: its bars' top, which halves with every
/// 6 dB, so the line and the bars meet.
fn db_to_y(db: f32, height: f64) -> f64 {
    let middle = height / 2.0;
    let level = 10f64.powf(f64::from(db.clamp(BOTTOM_DB, TOP_DB)) / 20.0);
    middle - (middle - MARGIN) * NOISE * level
}

fn y_to_db(y: f64, height: f64) -> f32 {
    let middle = height / 2.0;
    let level = ((middle - y) / ((middle - MARGIN) * NOISE)).max(1e-6);
    (20.0 * level.log10()) as f32
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strength_comes_back_from_the_noise_it_leaves() {
        for strength in [0.0, 25.0, 50.0, 80.0, 100.0] {
            let back = strength_for(noise_left(strength));
            assert!(
                (back - strength).abs() < 0.01,
                "{strength} came back {back}"
            );
        }
        assert_eq!(noise_left(0.0), 0.0);
        assert!((noise_left(100.0) - RESIDUAL_DB).abs() < 0.01);
        for db in [0.0, -6.0, -24.0] {
            assert!(
                (y_to_db(db_to_y(db, 170.0), 170.0) - db).abs() < 1e-3,
                "{db}"
            );
        }
    }
}
