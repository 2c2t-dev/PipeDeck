//! The de-esser, drawn as what it does and set by dragging it.
//!
//! The graph is what the de-esser does across the top of the range when an
//! s comes: under its frequency nothing moves; over it, the sound is turned
//! down, by as much as its strength says. It is worked out from the same
//! filters and sums the audio goes through. One handle sets both: sideways
//! for where the s start, down to take them further. The zones a voice is
//! talked about in are named along the top, so the s are easy to find.
//!
//! This depends on GTK, libadwaita and the engine's de-esser only, so
//! `examples/effects_render.rs` can draw it to an image.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::dsp::biquad::Coeffs;
use pipedeck_engine::dsp::{self, deesser_gain, DEESSER_PRESETS};

const MIN_FREQ: f32 = 1000.0;
const MAX_FREQ: f32 = 20_000.0;
/// The top and bottom of the graph, in decibels.
const TOP_DB: f32 = 3.0;
const BOTTOM_DB: f32 = -36.0;
/// How loud an s is taken to be, in the band the de-esser listens to, for
/// what the graph draws.
const LOUD_S: f32 = -12.0;
const HANDLE: f64 = 8.0;
const REACH: f64 = 20.0;
const MARGIN: f64 = 12.0;
/// Room over the curve for the zones' names.
const ZONE_BAR: f64 = 18.0;

const FREQ: usize = 0;
const STRENGTH: usize = 1;

/// The handle's colour, and the part it turns down.
const COLOUR: (f64, f64, f64) = (0.200, 0.820, 0.478);

/// The stretches of the top of the range, as a voice is talked about.
const ZONES: &[(f32, f32, &str)] = &[
    (MIN_FREQ, 2000.0, "Honk"),
    (2000.0, 5000.0, "Presence"),
    (5000.0, 9000.0, "Sibilance"),
    (9000.0, MAX_FREQ, "Air"),
];

type Changed = Rc<dyn Fn(&[f32])>;

pub struct DeEsserGraph {
    /// What goes in the layout: the presets, the graph, what it does, and
    /// a slider for each control.
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    readout: gtk::Label,
    presets: gtk::MenuButton,
    scales: Vec<gtk::Scale>,
    shown: Vec<gtk::Label>,
    /// Frequency and strength.
    values: RefCell<[f32; 2]>,
    /// Whether the pointer is on the handle, or dragging it.
    hovered: Cell<bool>,
    /// Set while the sliders are given values, so they do not send them
    /// back.
    syncing: Cell<bool>,
    changed: RefCell<Option<Changed>>,
}

impl DeEsserGraph {
    /// A graph showing `values`: frequency and strength.
    pub fn new(values: &[f32]) -> Rc<Self> {
        let spec = dsp::spec("deesser").expect("the de-esser is described");
        let mut initial = [0.0; 2];
        for (slot, param) in initial.iter_mut().zip(spec.params) {
            *slot = param.default;
        }
        for (slot, value) in initial.iter_mut().zip(values) {
            *slot = *value;
        }

        let area = gtk::DrawingArea::new();
        area.set_content_height(200);
        area.set_hexpand(true);

        let readout = gtk::Label::new(None);
        readout.add_css_class("caption");
        readout.add_css_class("dim-label");
        readout.set_xalign(0.0);
        readout.set_wrap(true);
        readout.set_lines(2);

        let this = Rc::new(Self {
            root: gtk::Box::new(gtk::Orientation::Vertical, 8),
            area,
            readout,
            presets: gtk::MenuButton::new(),
            scales: spec
                .params
                .iter()
                .map(|param| {
                    let (min, max) = (f64::from(param.min), f64::from(param.max));
                    let scale = gtk::Scale::with_range(
                        gtk::Orientation::Horizontal,
                        min,
                        max,
                        (max - min) / 200.0,
                    );
                    scale.set_hexpand(true);
                    scale
                })
                .collect(),
            shown: spec
                .params
                .iter()
                .map(|_| {
                    let label = gtk::Label::new(None);
                    label.add_css_class("caption");
                    label.add_css_class("numeric");
                    label.set_width_chars(8);
                    label.set_xalign(1.0);
                    label
                })
                .collect(),
            values: RefCell::new(initial),
            hovered: Cell::new(false),
            syncing: Cell::new(false),
            changed: RefCell::new(None),
        });
        this.build();
        this.wire();
        this.sync();
        this
    }

    /// Called with every value, in order, whenever one moves.
    pub fn connect_changed(&self, f: impl Fn(&[f32]) + 'static) {
        *self.changed.borrow_mut() = Some(Rc::new(f));
    }

    fn build(self: &Rc<Self>) {
        self.presets.set_popover(Some(&self.preset_menu()));
        self.presets.set_halign(gtk::Align::Start);
        self.presets
            .set_tooltip_text(Some("Start from a ready-made setting"));
        self.root.append(&self.presets);
        self.root.append(&self.area);
        self.root.append(&self.readout);

        let spec = dsp::spec("deesser").expect("the de-esser is described");
        for (index, param) in spec.params.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&dot(COLOUR));
            let name = gtk::Label::new(Some(param.label));
            name.add_css_class("caption");
            name.set_xalign(0.0);
            name.set_width_chars(10);
            row.append(&name);
            row.append(&self.scales[index]);
            row.append(&self.shown[index]);
            self.scales[index].connect_value_changed({
                let this = self.clone();
                move |scale| {
                    if this.syncing.get() {
                        return;
                    }
                    this.values.borrow_mut()[index] = scale.value() as f32;
                    this.after_change();
                }
            });
            self.root.append(&row);
        }
    }

    /// The list of presets, each with what it is for.
    fn preset_menu(self: &Rc<Self>) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for preset in DEESSER_PRESETS {
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
                let this = self.clone();
                let popover = popover.clone();
                move |_| {
                    *this.values.borrow_mut() = preset.values;
                    popover.popdown();
                    this.after_change();
                }
            });
            list.append(&button);
        }
        popover.set_child(Some(&list));
        popover
    }

    /// Put the values in the sliders and name the preset they are.
    fn sync(&self) {
        let values = *self.values.borrow();
        self.syncing.set(true);
        for (index, value) in values.iter().enumerate() {
            self.scales[index].set_value(f64::from(*value));
            self.shown[index].set_text(&format(index, *value));
        }
        self.syncing.set(false);

        let preset = DEESSER_PRESETS
            .iter()
            .find(|preset| {
                preset
                    .values
                    .iter()
                    .zip(values)
                    .all(|(a, b)| (a - b).abs() < 1e-3)
            })
            .map_or("Custom", |preset| preset.name);
        self.presets.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("view-list-symbolic")
                .label(format!("Preset: {preset}"))
                .build(),
        ));
        self.say();
    }

    fn after_change(&self) {
        self.sync();
        self.area.queue_draw();
        let values = self.values.borrow().to_vec();
        let changed = self.changed.borrow().clone();
        if let Some(changed) = changed {
            changed(&values);
        }
    }

    fn wire(self: &Rc<Self>) {
        self.area.set_draw_func({
            let this = self.clone();
            move |_, cr, width, height| this.draw(cr, f64::from(width), f64::from(height))
        });

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let this = self.clone();
            move |_, x, y| {
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
            let this = self.clone();
            move |_| {
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
        let grabbed = Rc::new(Cell::new(None::<(f64, f64)>));
        drag.connect_drag_begin({
            let this = self.clone();
            let grabbed = grabbed.clone();
            move |_, x, y| {
                let near = this.near(x, y);
                grabbed.set(near.then(|| this.handle()));
                this.hovered.set(near);
                this.say();
                this.area.queue_draw();
            }
        });
        drag.connect_drag_update({
            let this = self.clone();
            let grabbed = grabbed.clone();
            move |_, dx, dy| {
                if let Some((x, y)) = grabbed.get() {
                    this.move_handle(x + dx, y + dy);
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
            let this = self.clone();
            move |_, presses, x, y| {
                if presses != 2 || !this.near(x, y) {
                    return;
                }
                let spec = dsp::spec("deesser").expect("the de-esser is described");
                *this.values.borrow_mut() =
                    [spec.params[FREQ].default, spec.params[STRENGTH].default];
                this.after_change();
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

    fn size(&self) -> (f64, f64) {
        (f64::from(self.area.width()), f64::from(self.area.height()))
    }

    /// Where the handle is drawn: at the frequency, as deep as a loud s is
    /// turned down.
    fn handle(&self) -> (f64, f64) {
        let (width, height) = self.size();
        let [freq, strength] = *self.values.borrow();
        (
            freq_to_x(freq, width),
            db_to_y(deesser_gain(strength, LOUD_S), height),
        )
    }

    fn near(&self, x: f64, y: f64) -> bool {
        let (hx, hy) = self.handle();
        (hx - x).hypot(hy - y) <= REACH
    }

    /// Set the frequency from across, and the strength from how deep.
    fn move_handle(&self, x: f64, y: f64) {
        let (width, height) = self.size();
        let spec = dsp::spec("deesser").expect("the de-esser is described");
        let freq_spec = &spec.params[FREQ];
        let freq = (x_to_freq(x, width) / 10.0).round() * 10.0;
        let depth = y_to_db(y, height).min(0.0);
        // The gain falls as the strength rises; the strength that gives the
        // depth pointed at is found by halving.
        let (mut low, mut high) = (0.0f32, 100.0f32);
        for _ in 0..30 {
            let mid = (low + high) / 2.0;
            if deesser_gain(mid, LOUD_S) > depth {
                low = mid;
            } else {
                high = mid;
            }
        }
        *self.values.borrow_mut() = [
            freq.clamp(freq_spec.min, freq_spec.max),
            ((low + high) / 2.0).round(),
        ];
        self.hovered.set(true);
        self.after_change();
    }

    /// Say what the handle sets, or what the de-esser does to an s.
    fn say(&self) {
        let [freq, strength] = *self.values.borrow();
        let text = if self.hovered.get() {
            format!(
                "{}  ·  strength {strength:.0} %  ·  sideways for where the s start, down to \
                 take them further",
                format_freq(freq)
            )
        } else {
            format!(
                "Over {}, a loud s is turned down {:.1} dB. The voice under it is left alone, \
                 and so is the top when no s is there.",
                format_freq(freq),
                -deesser_gain(strength, LOUD_S)
            )
        };
        self.readout.set_label(&text);
    }

    fn draw(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let [freq, strength] = *self.values.borrow();
        let fg = self.area.color();
        let set = |cr: &gtk::cairo::Context, alpha: f64| {
            cr.set_source_rgba(fg.red().into(), fg.green().into(), fg.blue().into(), alpha);
        };
        let colour = |cr: &gtk::cairo::Context, alpha: f64| {
            let (r, g, b) = COLOUR;
            cr.set_source_rgba(r, g, b, alpha);
        };
        let (left, right) = (freq_to_x(MIN_FREQ, width), freq_to_x(MAX_FREQ, width));
        let (top, bottom) = (db_to_y(TOP_DB, height), db_to_y(BOTTOM_DB, height));

        rounded(cr, 0.0, 0.0, width, height, 10.0);
        set(cr, 0.04);
        let _ = cr.fill();

        // The zones, every other one shaded, named along the top.
        cr.set_font_size(10.0);
        for (index, (from, to, name)) in ZONES.iter().enumerate() {
            let (from, to) = (freq_to_x(*from, width), freq_to_x(*to, width));
            if index % 2 == 1 {
                cr.rectangle(from, top, to - from, bottom - top);
                set(cr, 0.025);
                let _ = cr.fill();
            }
            if let Ok(extents) = cr.text_extents(name) {
                if extents.width() < to - from - 4.0 {
                    cr.move_to((from + to) / 2.0 - extents.width() / 2.0, ZONE_BAR - 5.0);
                    set(cr, if *name == "Sibilance" { 0.75 } else { 0.4 });
                    let _ = cr.show_text(name);
                }
            }
        }

        // What it turns down, shaded from the split up.
        let split = freq_to_x(freq, width);
        cr.rectangle(split, top, right - split, bottom - top);
        colour(cr, 0.06);
        let _ = cr.fill();

        // The grid: landmarks across, every 6 dB along.
        cr.set_line_width(1.0);
        for mark in [2000.0, 3000.0, 5000.0, 10000.0] {
            let x = freq_to_x(mark, width).round() + 0.5;
            cr.move_to(x, top);
            cr.line_to(x, bottom);
        }
        let mut db = 0.0;
        while db >= BOTTOM_DB {
            let y = db_to_y(db, height).round() + 0.5;
            cr.move_to(left, y);
            cr.line_to(right, y);
            db -= 6.0;
        }
        set(cr, 0.06);
        let _ = cr.stroke();
        let flat = db_to_y(0.0, height).round() + 0.5;
        cr.move_to(left, flat);
        cr.line_to(right, flat);
        set(cr, 0.18);
        let _ = cr.stroke();

        set(cr, 0.45);
        for (mark, text) in [(2000.0, "2k"), (5000.0, "5k"), (10000.0, "10k")] {
            cr.move_to(freq_to_x(mark, width) + 3.0, bottom - 3.0);
            let _ = cr.show_text(text);
        }
        for db in [-12.0, -24.0] {
            cr.move_to(left + 3.0, db_to_y(db, height) - 3.0);
            let _ = cr.show_text(&format!("{db:.0}"));
        }

        // The curve: the two sides of the split put back together, the
        // upper one turned down as a loud s would have it. The sides stay
        // in phase, so their sizes add.
        let low = Coeffs::lowpass(freq);
        let high = Coeffs::highpass(freq);
        let gain = 10f32.powf(deesser_gain(strength, LOUD_S) / 20.0);
        const POINTS: usize = 200;
        let curve: Vec<(f64, f64)> = (0..=POINTS)
            .map(|i| {
                let x = left + (right - left) * i as f64 / POINTS as f64;
                let f = x_to_freq(x, width);
                let under = 10f32.powf(low.response_db(f) * 2.0 / 20.0);
                let over = 10f32.powf(high.response_db(f) * 2.0 / 20.0);
                let db = 20.0 * (under + gain * over).max(1e-6).log10();
                (x, db_to_y(db, height))
            })
            .collect();
        cr.move_to(curve[0].0, flat);
        for (x, y) in &curve {
            cr.line_to(*x, *y);
        }
        cr.line_to(curve[POINTS].0, flat);
        cr.close_path();
        colour(cr, 0.22);
        let _ = cr.fill();
        cr.move_to(curve[0].0, curve[0].1);
        for (x, y) in &curve[1..] {
            cr.line_to(*x, *y);
        }
        cr.set_line_width(2.5);
        set(cr, 0.92);
        let _ = cr.stroke();

        // The split, and the handle on it.
        cr.move_to(split.round() + 0.5, top);
        cr.line_to(split.round() + 0.5, bottom);
        colour(cr, 0.5);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        let (x, y) = self.handle();
        // How deep a loud s goes, from the handle out to the top of the
        // range, which is where the curve ends up.
        cr.set_dash(&[4.0, 4.0], 0.0);
        cr.move_to(x, y.clamp(top, bottom));
        cr.line_to(right, y.clamp(top, bottom));
        colour(cr, 0.6);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
        let active = self.hovered.get();
        cr.arc(
            x,
            y.clamp(top, bottom),
            if active { HANDLE + 2.0 } else { HANDLE },
            0.0,
            std::f64::consts::TAU,
        );
        colour(cr, 1.0);
        let _ = cr.fill_preserve();
        cr.set_line_width(if active { 2.5 } else { 1.5 });
        set(cr, if active { 0.95 } else { 0.5 });
        let _ = cr.stroke();
    }
}

/// A control's value as the window writes it, with its unit.
fn format(which: usize, value: f32) -> String {
    match which {
        FREQ => format_freq(value),
        _ => format!("{value:.0} %"),
    }
}

fn format_freq(freq: f32) -> String {
    if freq >= 1000.0 {
        format!("{:.1} kHz", freq / 1000.0)
    } else {
        format!("{freq:.0} Hz")
    }
}

/// A small disc of the handle's colour, beside a slider.
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

fn freq_to_x(freq: f32, width: f64) -> f64 {
    let span = (MAX_FREQ / MIN_FREQ).log10();
    let at = (freq.clamp(MIN_FREQ, MAX_FREQ) / MIN_FREQ).log10() / span;
    MARGIN + f64::from(at) * (width - 2.0 * MARGIN)
}

fn x_to_freq(x: f64, width: f64) -> f32 {
    let at = ((x - MARGIN) / (width - 2.0 * MARGIN)).clamp(0.0, 1.0) as f32;
    MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf(at)
}

fn db_to_y(db: f32, height: f64) -> f64 {
    let at = f64::from((TOP_DB - db.clamp(BOTTOM_DB, TOP_DB)) / (TOP_DB - BOTTOM_DB));
    ZONE_BAR + at * (height - ZONE_BAR - MARGIN)
}

fn y_to_db(y: f64, height: f64) -> f32 {
    let at = ((y - ZONE_BAR) / (height - ZONE_BAR - MARGIN)) as f32;
    TOP_DB - at * (TOP_DB - BOTTOM_DB)
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
    fn a_place_comes_back_from_where_it_is_drawn() {
        for freq in [1000.0, 6500.0, 20000.0] {
            let x = freq_to_x(freq, 400.0);
            assert!((x_to_freq(x, 400.0) - freq).abs() / freq < 1e-3, "{freq}");
        }
        for db in [3.0, 0.0, -12.0, -36.0] {
            assert!(
                (y_to_db(db_to_y(db, 200.0), 200.0) - db).abs() < 1e-3,
                "{db}"
            );
        }
    }
}
