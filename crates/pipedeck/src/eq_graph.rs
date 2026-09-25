//! The equaliser, drawn as what it does and set by dragging it.
//!
//! The curve is the equaliser's response across the audible range, worked
//! out from the same filters the audio goes through, so what is drawn is
//! what is heard. Each band has a handle on it: drag it sideways for its
//! frequency and up or down for its gain, scroll over a bell to make it
//! wider or narrower, double-click to put it back flat.
//!
//! This depends on GTK, libadwaita and the engine's equaliser only, so
//! `examples/eq_render.rs` can draw it to an image.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::dsp::eq::{self, BAND_LAYOUT, LOW_CUT_OFF, PARAMS};

const MIN_FREQ: f32 = 20.0;
const MAX_FREQ: f32 = 20_000.0;
/// The height of the graph in decibels, each way from flat. A little over
/// the bands' own range, so a band at full lift is not drawn on the edge.
const RANGE_DB: f32 = 15.0;
const HANDLE: f64 = 7.0;
/// How near a press has to be to a handle to take it.
const REACH: f64 = 18.0;
const MARGIN: f64 = 10.0;

type Changed = Rc<dyn Fn(&[f32])>;

pub struct EqGraph {
    /// What goes in the layout: the graph and the line under it saying what
    /// is being set.
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    readout: gtk::Label,
    values: Rc<RefCell<Vec<f32>>>,
    /// The band under the pointer or being dragged.
    active: Rc<Cell<Option<usize>>>,
    changed: Rc<RefCell<Option<Changed>>>,
}

impl EqGraph {
    /// A graph showing `values`, in the order of the equaliser's controls.
    pub fn new(values: &[f32]) -> Rc<Self> {
        let area = gtk::DrawingArea::new();
        area.set_content_height(170);
        area.set_hexpand(true);
        area.add_css_class("pd-eq");

        let readout = gtk::Label::new(None);
        readout.add_css_class("caption");
        readout.add_css_class("dim-label");
        readout.set_xalign(0.0);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.append(&area);
        root.append(&readout);

        let mut initial: Vec<f32> = PARAMS.iter().map(|param| param.default).collect();
        for (slot, value) in initial.iter_mut().zip(values) {
            *slot = *value;
        }

        let this = Rc::new(Self {
            root,
            area,
            readout,
            values: Rc::new(RefCell::new(initial)),
            active: Rc::new(Cell::new(None)),
            changed: Rc::new(RefCell::new(None)),
        });
        this.wire();
        this.say(None);
        this
    }

    /// Called with every value, in order, whenever a band moves.
    pub fn connect_changed(&self, f: impl Fn(&[f32]) + 'static) {
        *self.changed.borrow_mut() = Some(Rc::new(f));
    }

    fn wire(self: &Rc<Self>) {
        self.area.set_draw_func({
            let values = self.values.clone();
            let active = self.active.clone();
            move |area, cr, width, height| {
                draw(
                    area,
                    cr,
                    f64::from(width),
                    f64::from(height),
                    &values.borrow(),
                    active.get(),
                )
            }
        });

        // Hovering says which band is under the pointer.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let this = self.clone();
            move |_, x, y| {
                if this.dragging() {
                    return;
                }
                let near = this.nearest(x, y);
                if near != this.active.get() {
                    this.active.set(near);
                    this.say(near);
                    this.area.queue_draw();
                }
            }
        });
        motion.connect_leave({
            let this = self.clone();
            move |_| {
                if !this.dragging() {
                    this.active.set(None);
                    this.say(None);
                    this.area.queue_draw();
                }
            }
        });
        self.area.add_controller(motion);

        // Dragging a handle moves its band. It moves by as much as the
        // pointer does, from where the handle was: taking a handle by its
        // edge does not make it jump to put its middle under the pointer.
        let drag = gtk::GestureDrag::new();
        let grabbed = Rc::new(Cell::new(None::<(usize, f64, f64)>));
        drag.connect_drag_begin({
            let this = self.clone();
            let grabbed = grabbed.clone();
            move |_, x, y| {
                let near = this.nearest(x, y);
                grabbed.set(near.map(|band| {
                    let width = f64::from(this.area.width());
                    let height = f64::from(this.area.height());
                    let (hx, hy) = handle(&this.values.borrow(), band, width, height);
                    (band, hx, hy)
                }));
                this.active.set(near);
                this.say(near);
                this.area.queue_draw();
            }
        });
        drag.connect_drag_update({
            let this = self.clone();
            let grabbed = grabbed.clone();
            move |_, dx, dy| {
                let Some((band, x, y)) = grabbed.get() else {
                    return;
                };
                this.move_band(band, x + dx, y + dy);
            }
        });
        drag.connect_drag_end({
            let grabbed = grabbed.clone();
            move |_, _, _| grabbed.set(None)
        });
        self.area.add_controller(drag);

        // Scrolling over a bell makes it wider or narrower.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll({
            let this = self.clone();
            move |_, _, dy| {
                let Some(band) = this.active.get() else {
                    return gtk::glib::Propagation::Proceed;
                };
                let Some(q) = BAND_LAYOUT[band].q else {
                    return gtk::glib::Propagation::Proceed;
                };
                let factor = if dy > 0.0 { 1.0 / 1.15 } else { 1.15 };
                let spec = &PARAMS[q];
                {
                    let mut values = this.values.borrow_mut();
                    values[q] = (values[q] * factor).clamp(spec.min, spec.max);
                }
                this.after_change(band);
                gtk::glib::Propagation::Stop
            }
        });
        self.area.add_controller(scroll);

        // A double-click puts a band back flat.
        let click = gtk::GestureClick::new();
        click.connect_pressed({
            let this = self.clone();
            move |_, presses, x, y| {
                if presses != 2 {
                    return;
                }
                let Some(band) = this.nearest(x, y) else {
                    return;
                };
                let layout = BAND_LAYOUT[band];
                {
                    let mut values = this.values.borrow_mut();
                    match layout.gain {
                        Some(gain) => values[gain] = 0.0,
                        // The low cut has no gain: flat is off.
                        None => values[layout.freq] = LOW_CUT_OFF,
                    }
                    if let Some(q) = layout.q {
                        values[q] = PARAMS[q].default;
                    }
                }
                this.after_change(band);
            }
        });
        self.area.add_controller(click);
    }

    fn dragging(&self) -> bool {
        // A band stays the active one while it is dragged, whatever the
        // pointer passes over; the drag gesture is what says it is.
        self.area
            .observe_controllers()
            .into_iter()
            .flatten()
            .filter_map(|c| c.downcast::<gtk::GestureDrag>().ok())
            .any(|g| g.is_active())
    }

    /// Set a band from where the pointer is.
    fn move_band(&self, band: usize, x: f64, y: f64) {
        let width = f64::from(self.area.width());
        let height = f64::from(self.area.height());
        let layout = BAND_LAYOUT[band];
        {
            let mut values = self.values.borrow_mut();
            let freq_spec = &PARAMS[layout.freq];
            values[layout.freq] = x_to_freq(x, width).clamp(freq_spec.min, freq_spec.max);
            if let Some(gain) = layout.gain {
                let spec = &PARAMS[gain];
                // Whole tenths of a decibel: the readout shows no more.
                let db = (y_to_db(y, height) * 10.0).round() / 10.0;
                values[gain] = db.clamp(spec.min, spec.max);
            }
        }
        self.after_change(band);
    }

    fn after_change(&self, band: usize) {
        self.active.set(Some(band));
        self.say(Some(band));
        self.area.queue_draw();
        let values = self.values.borrow().clone();
        let changed = self.changed.borrow().clone();
        if let Some(changed) = changed {
            changed(&values);
        }
    }

    /// The band whose handle is nearest the pointer, if one is in reach.
    fn nearest(&self, x: f64, y: f64) -> Option<usize> {
        let width = f64::from(self.area.width());
        let height = f64::from(self.area.height());
        let values = self.values.borrow();
        (0..BAND_LAYOUT.len())
            .map(|band| {
                let (hx, hy) = handle(&values, band, width, height);
                (band, (hx - x).hypot(hy - y))
            })
            .filter(|(_, distance)| *distance <= REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(band, _)| band)
    }

    /// Say what a band is set to, or how to set one.
    fn say(&self, band: Option<usize>) {
        let Some(band) = band else {
            self.readout.set_label(
                "Drag a point to move a band, scroll over a bell to widen it, \
                 double-click to put it back",
            );
            return;
        };
        let values = self.values.borrow();
        let layout = BAND_LAYOUT[band];
        let freq = values[layout.freq];
        let mut parts = vec![layout.label.to_owned()];
        if layout.gain.is_none() && freq <= LOW_CUT_OFF {
            parts.push("off".to_owned());
        } else {
            parts.push(format_freq(freq));
        }
        if let Some(gain) = layout.gain {
            parts.push(format!("{:+.1} dB", values[gain]));
        }
        if let Some(q) = layout.q {
            parts.push(format!("width {:.1}", values[q]));
        }
        self.readout.set_label(&parts.join("  ·  "));
    }
}

fn format_freq(freq: f32) -> String {
    if freq >= 1000.0 {
        format!("{:.1} kHz", freq / 1000.0)
    } else {
        format!("{freq:.0} Hz")
    }
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
    let at = f64::from((db / RANGE_DB).clamp(-1.0, 1.0));
    height / 2.0 - at * (height / 2.0 - MARGIN)
}

fn y_to_db(y: f64, height: f64) -> f32 {
    ((height / 2.0 - y) / (height / 2.0 - MARGIN)) as f32 * RANGE_DB
}

/// Where a band's handle sits: at its frequency, and at its gain, or on the
/// flat line for the low cut, which has none.
///
/// Not on the curve: the curve is every band at once, and a handle riding
/// it would move whenever a band beside it did.
fn handle(values: &[f32], band: usize, width: f64, height: f64) -> (f64, f64) {
    let layout = BAND_LAYOUT[band];
    let freq = values[layout.freq];
    let db = layout.gain.map_or(0.0, |gain| values[gain]);
    (freq_to_x(freq, width), db_to_y(db, height))
}

// --- drawing --------------------------------------------------------------

fn draw(
    area: &gtk::DrawingArea,
    cr: &gtk::cairo::Context,
    width: f64,
    height: f64,
    values: &[f32],
    active: Option<usize>,
) {
    let fg = area.color();
    let accent = adw::StyleManager::default().accent_color_rgba();
    let set = |cr: &gtk::cairo::Context, color: &gtk::gdk::RGBA, alpha: f64| {
        cr.set_source_rgba(
            color.red().into(),
            color.green().into(),
            color.blue().into(),
            alpha,
        );
    };

    // The ground.
    rounded(cr, 0.0, 0.0, width, height, 10.0);
    set(cr, &fg, 0.04);
    let _ = cr.fill();

    // Decades and the usual landmarks across, decibels along.
    cr.set_line_width(1.0);
    for freq in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
        let x = freq_to_x(freq, width).round() + 0.5;
        cr.move_to(x, MARGIN);
        cr.line_to(x, height - MARGIN);
    }
    set(cr, &fg, 0.07);
    let _ = cr.stroke();
    for db in [-12.0, -6.0, 6.0, 12.0] {
        let y = db_to_y(db, height).round() + 0.5;
        cr.move_to(MARGIN, y);
        cr.line_to(width - MARGIN, y);
    }
    set(cr, &fg, 0.05);
    let _ = cr.stroke();
    let flat = db_to_y(0.0, height).round() + 0.5;
    cr.move_to(MARGIN, flat);
    cr.line_to(width - MARGIN, flat);
    set(cr, &fg, 0.18);
    let _ = cr.stroke();

    cr.set_font_size(10.0);
    set(cr, &fg, 0.45);
    for (freq, text) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        let x = freq_to_x(freq, width);
        cr.move_to(x + 3.0, height - MARGIN - 3.0);
        let _ = cr.show_text(text);
    }
    for (db, text) in [(12.0, "+12"), (-12.0, "-12")] {
        cr.move_to(MARGIN + 3.0, db_to_y(db, height) - 3.0);
        let _ = cr.show_text(text);
    }

    // The curve, and what it takes away from or adds to flat, shaded.
    const POINTS: usize = 240;
    let curve: Vec<(f64, f64)> = (0..=POINTS)
        .map(|i| {
            let x = MARGIN + (width - 2.0 * MARGIN) * i as f64 / POINTS as f64;
            let db = eq::response(values, x_to_freq(x, width));
            (x, db_to_y(db, height))
        })
        .collect();
    cr.move_to(curve[0].0, flat);
    for (x, y) in &curve {
        cr.line_to(*x, *y);
    }
    cr.line_to(curve[POINTS].0, flat);
    cr.close_path();
    set(cr, &accent, 0.16);
    let _ = cr.fill();
    cr.move_to(curve[0].0, curve[0].1);
    for (x, y) in &curve[1..] {
        cr.line_to(*x, *y);
    }
    cr.set_line_width(2.0);
    set(cr, &accent, 1.0);
    let _ = cr.stroke();

    // The handles, the active one larger and ringed.
    for band in 0..BAND_LAYOUT.len() {
        let (x, y) = handle(values, band, width, height);
        let off = BAND_LAYOUT[band].gain.is_none() && values[BAND_LAYOUT[band].freq] <= LOW_CUT_OFF;
        let radius = if active == Some(band) {
            HANDLE + 2.0
        } else {
            HANDLE
        };
        cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
        if off {
            set(cr, &fg, 0.35);
        } else {
            set(cr, &accent, 1.0);
        }
        let _ = cr.fill_preserve();
        cr.set_line_width(if active == Some(band) { 2.5 } else { 1.5 });
        set(cr, &fg, if active == Some(band) { 0.95 } else { 0.55 });
        let _ = cr.stroke();
    }
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
    fn a_frequency_and_a_level_come_back_from_where_they_are_drawn() {
        for freq in [20.0, 100.0, 1000.0, 6500.0, 20000.0] {
            let x = freq_to_x(freq, 400.0);
            assert!((x_to_freq(x, 400.0) - freq).abs() / freq < 1e-3, "{freq}");
        }
        for db in [-12.0, -3.5, 0.0, 6.0, 12.0] {
            let y = db_to_y(db, 170.0);
            assert!((y_to_db(y, 170.0) - db).abs() < 1e-3, "{db}");
        }
    }
}
