//! The equaliser, drawn as what it does and set by dragging it.
//!
//! The curve is the equaliser's response across the audible range, worked
//! out from the same filters the audio goes through, so what is drawn is
//! what is heard. Each band has a colour of its own, a handle in it, and
//! its own shape drawn faintly under the curve, so it is plain which band
//! does what. Drag a handle sideways for its frequency and up or down for
//! its gain, scroll over a bell to make it wider or narrower, double-click
//! to put it back flat.
//!
//! Behind the curve, the range is cut into the zones a voice is talked
//! about in — rumble, body, mud and so on — and the pointer over one says
//! what it does and what to do about it. Presets give a starting point, and
//! the band picked under the graph can be set to the exact value.
//!
//! This depends on GTK, libadwaita and the engine's equaliser only, so
//! `examples/eq_render.rs` can draw it to an image.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::dsp::eq::{self, BAND_LAYOUT, LOW_CUT_OFF, PARAMS, PRESETS};

const MIN_FREQ: f32 = 20.0;
const MAX_FREQ: f32 = 20_000.0;
/// The height of the graph in decibels, each way from flat. A little over
/// the bands' own range, so a band at full lift is not drawn on the edge.
const RANGE_DB: f32 = 15.0;
const HANDLE: f64 = 7.0;
/// How near a press has to be to a handle to take it.
const REACH: f64 = 18.0;
const MARGIN: f64 = 10.0;
/// Room over the curve for the zones' names.
const ZONE_BAR: f64 = 18.0;

/// Each band's colour, in the order of `BAND_LAYOUT`: from warm at the
/// bottom of the range to cool at the top.
const BAND_COLOURS: [(f64, f64, f64); 5] = [
    (0.753, 0.380, 0.796), // low cut, purple
    (1.000, 0.471, 0.000), // low, orange
    (0.965, 0.827, 0.176), // mid, yellow
    (0.200, 0.820, 0.478), // presence, green
    (0.384, 0.627, 0.918), // air, blue
];

/// A stretch of the range as a voice is talked about, and what to do there.
struct Zone {
    from: f32,
    to: f32,
    name: &'static str,
    advice: &'static str,
}

const ZONES: &[Zone] = &[
    Zone {
        from: MIN_FREQ,
        to: 80.0,
        name: "Rumble",
        advice: "Traffic, knocks on the desk and hum. A voice has nothing here: cut it.",
    },
    Zone {
        from: 80.0,
        to: 250.0,
        name: "Body",
        advice: "The weight of a voice. A little more for warmth, less if it booms.",
    },
    Zone {
        from: 250.0,
        to: 600.0,
        name: "Mud",
        advice: "Where a voice turns boxy or muffled. Cutting here often clears it.",
    },
    Zone {
        from: 600.0,
        to: 2000.0,
        name: "Honk",
        advice: "Nasal, telephone-like when too strong. A gentle cut softens it.",
    },
    Zone {
        from: 2000.0,
        to: 5000.0,
        name: "Presence",
        advice: "Clarity: what makes words easy to follow. Lift it to cut through.",
    },
    Zone {
        from: 5000.0,
        to: 9000.0,
        name: "Sibilance",
        advice: "The s and sh. Too much here is harsh; the de-esser handles it.",
    },
    Zone {
        from: 9000.0,
        to: MAX_FREQ,
        name: "Air",
        advice: "Sparkle and breath. A gentle lift opens a voice up.",
    },
];

type Changed = Rc<dyn Fn(&[f32])>;

pub struct EqGraph {
    /// What goes in the layout: the presets, the graph, the line under it
    /// saying what is being set, and the bands to pick.
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    readout: gtk::Label,
    presets: gtk::MenuButton,
    /// One per band, to pick the one the exact controls set.
    chips: Vec<gtk::ToggleButton>,
    freq: gtk::SpinButton,
    gain: gtk::SpinButton,
    width: gtk::SpinButton,
    /// The gain and width with their names, hidden for a band without.
    gain_field: gtk::Box,
    width_field: gtk::Box,
    values: RefCell<Vec<f32>>,
    /// The band under the pointer or being dragged.
    hovered: Cell<Option<usize>>,
    /// The band the exact controls set: the last one picked or moved.
    selected: Cell<usize>,
    /// The zone under the pointer, when no handle is.
    zone: Cell<Option<usize>>,
    /// Set while the exact controls are given values, so they do not send
    /// them back.
    syncing: Cell<bool>,
    changed: RefCell<Option<Changed>>,
}

impl EqGraph {
    /// A graph showing `values`, in the order of the equaliser's controls.
    pub fn new(values: &[f32]) -> Rc<Self> {
        let area = gtk::DrawingArea::new();
        area.set_content_height(220);
        area.set_hexpand(true);
        area.add_css_class("pd-eq");

        let readout = gtk::Label::new(None);
        readout.add_css_class("caption");
        readout.add_css_class("dim-label");
        readout.set_xalign(0.0);
        readout.set_wrap(true);
        readout.set_lines(2);

        let mut initial: Vec<f32> = PARAMS.iter().map(|param| param.default).collect();
        for (slot, value) in initial.iter_mut().zip(values) {
            *slot = *value;
        }

        let spin = |min: f64, max: f64, step: f64, digits: u32| {
            let spin = gtk::SpinButton::with_range(min, max, step);
            spin.set_digits(digits);
            spin.set_numeric(true);
            spin
        };

        let this = Rc::new(Self {
            root: gtk::Box::new(gtk::Orientation::Vertical, 8),
            area,
            readout,
            presets: gtk::MenuButton::new(),
            chips: BAND_LAYOUT
                .iter()
                .map(|band| gtk::ToggleButton::with_label(band.label))
                .collect(),
            freq: spin(20.0, 20000.0, 1.0, 0),
            gain: spin(-12.0, 12.0, 0.5, 1),
            width: spin(0.3, 6.0, 0.1, 1),
            gain_field: gtk::Box::new(gtk::Orientation::Horizontal, 6),
            width_field: gtk::Box::new(gtk::Orientation::Horizontal, 6),
            values: RefCell::new(initial),
            hovered: Cell::new(None),
            selected: Cell::new(2),
            zone: Cell::new(None),
            syncing: Cell::new(false),
            changed: RefCell::new(None),
        });
        this.build();
        this.wire();
        this.select(2);
        this.say();
        this
    }

    /// Called with every value, in order, whenever a band moves.
    pub fn connect_changed(&self, f: impl Fn(&[f32]) + 'static) {
        *self.changed.borrow_mut() = Some(Rc::new(f));
    }

    fn build(self: &Rc<Self>) {
        // The presets, named by the one the values are, if any.
        self.presets.set_popover(Some(&self.preset_menu()));
        self.presets.set_halign(gtk::Align::Start);
        self.presets
            .set_tooltip_text(Some("Start from a ready-made curve"));
        self.show_preset();
        self.root.append(&self.presets);

        self.root.append(&self.area);
        self.root.append(&self.readout);

        // The bands, each in its colour; the one picked is set exactly
        // below.
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chips.set_homogeneous(true);
        for (band, chip) in self.chips.iter().enumerate() {
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            content.set_halign(gtk::Align::Center);
            content.append(&dot(BAND_COLOURS[band]));
            content.append(&gtk::Label::new(Some(BAND_LAYOUT[band].label)));
            chip.set_child(Some(&content));
            chip.add_css_class("flat");
            if band > 0 {
                chip.set_group(Some(&self.chips[0]));
            }
            chip.connect_toggled({
                let this = Rc::downgrade(self);
                move |chip| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    if chip.is_active() && !this.syncing.get() {
                        this.select(band);
                        this.area.queue_draw();
                    }
                }
            });
            chips.append(chip);
        }
        self.root.append(&chips);

        let fields = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        fields.set_halign(gtk::Align::Center);
        let field = |name: &str, spin: &gtk::SpinButton, unit: &str, into: &gtk::Box| {
            let label = gtk::Label::new(Some(name));
            label.add_css_class("caption");
            into.append(&label);
            into.append(spin);
            if !unit.is_empty() {
                let unit = gtk::Label::new(Some(unit));
                unit.add_css_class("caption");
                unit.add_css_class("dim-label");
                into.append(&unit);
            }
        };
        let freq_field = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        field("Frequency", &self.freq, "Hz", &freq_field);
        field("Gain", &self.gain, "dB", &self.gain_field);
        field("Width", &self.width, "", &self.width_field);
        fields.append(&freq_field);
        fields.append(&self.gain_field);
        fields.append(&self.width_field);
        self.root.append(&fields);

        for (spin, which) in [(&self.freq, 0), (&self.gain, 1), (&self.width, 2)] {
            spin.connect_value_changed({
                let this = Rc::downgrade(self);
                move |spin| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    if this.syncing.get() {
                        return;
                    }
                    let band = this.selected.get();
                    let layout = BAND_LAYOUT[band];
                    let index = match which {
                        0 => Some(layout.freq),
                        1 => layout.gain,
                        _ => layout.q,
                    };
                    let Some(index) = index else {
                        return;
                    };
                    let spec = &PARAMS[index];
                    this.values.borrow_mut()[index] =
                        (spin.value() as f32).clamp(spec.min, spec.max);
                    this.after_change(band);
                }
            });
        }
    }

    /// The list of presets, each with what it is for.
    fn preset_menu(self: &Rc<Self>) -> gtk::Popover {
        let popover = gtk::Popover::new();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for preset in PRESETS {
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
                    *this.values.borrow_mut() = preset.values.to_vec();
                    popover.popdown();
                    this.after_change(this.selected.get());
                }
            });
            list.append(&button);
        }
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_max_content_height(420);
        scroller.set_propagate_natural_height(true);
        scroller.set_child(Some(&list));
        popover.set_child(Some(&scroller));
        popover
    }

    /// Name the preset on its button: the one the curve is, or none.
    fn show_preset(&self) {
        let name = eq::preset_of(&self.values.borrow()).map_or("Custom", |preset| preset.name);
        self.presets.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("view-list-symbolic")
                .label(format!("Preset: {name}"))
                .build(),
        ));
    }

    /// Pick the band the exact controls set, and show its values in them.
    fn select(&self, band: usize) {
        self.selected.set(band);
        self.syncing.set(true);
        self.chips[band].set_active(true);
        let layout = BAND_LAYOUT[band];
        let values = self.values.borrow();

        let spec = &PARAMS[layout.freq];
        self.freq
            .set_range(f64::from(spec.min), f64::from(spec.max));
        self.freq.set_value(f64::from(values[layout.freq]));
        self.gain_field.set_visible(layout.gain.is_some());
        if let Some(gain) = layout.gain {
            self.gain.set_value(f64::from(values[gain]));
        }
        self.width_field.set_visible(layout.q.is_some());
        if let Some(q) = layout.q {
            self.width.set_value(f64::from(values[q]));
        }
        drop(values);
        self.syncing.set(false);
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
                if let Some(this) = this.upgrade() {
                    this.hover(x, y);
                }
            }
        });
        motion.connect_leave({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.leave();
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
            let this = Rc::downgrade(self);
            let grabbed = grabbed.clone();
            move |_, x, y| {
                if let Some(this) = this.upgrade() {
                    grabbed.set(this.grab(x, y));
                }
            }
        });
        drag.connect_drag_update({
            let this = Rc::downgrade(self);
            let grabbed = grabbed.clone();
            move |_, dx, dy| {
                if let (Some(this), Some((band, x, y))) = (this.upgrade(), grabbed.get()) {
                    this.move_band(band, x + dx, y + dy);
                }
            }
        });
        drag.connect_drag_end({
            let grabbed = grabbed.clone();
            move |_, _, _| grabbed.set(None)
        });
        self.area.add_controller(drag);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll({
            let this = Rc::downgrade(self);
            move |_, _, dy| {
                this.upgrade()
                    .map_or(gtk::glib::Propagation::Proceed, |this| this.widen(dy))
            }
        });
        self.area.add_controller(scroll);

        let click = gtk::GestureClick::new();
        click.connect_pressed({
            let this = Rc::downgrade(self);
            move |_, presses, x, y| {
                if let Some(this) = this.upgrade() {
                    if presses == 2 {
                        this.flatten_at(x, y);
                    }
                }
            }
        });
        self.area.add_controller(click);
    }

    /// Hovering says which band, or which zone, is under the pointer.
    fn hover(&self, x: f64, y: f64) {
        if self.dragging() {
            return;
        }
        let near = self.nearest(x, y);
        let zone = if near.is_some() {
            None
        } else {
            let freq = x_to_freq(x, f64::from(self.area.width()));
            ZONES.iter().position(|z| freq >= z.from && freq < z.to)
        };
        if near != self.hovered.get() || zone != self.zone.get() {
            self.hovered.set(near);
            self.zone.set(zone);
            self.say();
            self.area.queue_draw();
        }
    }

    fn leave(&self) {
        if !self.dragging() {
            self.hovered.set(None);
            self.zone.set(None);
            self.say();
            self.area.queue_draw();
        }
    }

    /// Take the band under the pointer, if any: which, and where its
    /// handle was.
    fn grab(&self, x: f64, y: f64) -> Option<(usize, f64, f64)> {
        let near = self.nearest(x, y);
        let grabbed = near.map(|band| {
            let (hx, hy) = self.handle(band);
            (band, hx, hy)
        });
        if let Some(band) = near {
            self.select(band);
        }
        self.hovered.set(near);
        self.zone.set(None);
        self.say();
        self.area.queue_draw();
        grabbed
    }

    /// Scrolling over a bell makes it wider or narrower.
    fn widen(&self, dy: f64) -> gtk::glib::Propagation {
        let Some(band) = self.hovered.get() else {
            return gtk::glib::Propagation::Proceed;
        };
        let Some(q) = BAND_LAYOUT[band].q else {
            return gtk::glib::Propagation::Proceed;
        };
        let factor = if dy > 0.0 { 1.0 / 1.15 } else { 1.15 };
        let spec = &PARAMS[q];
        {
            let mut values = self.values.borrow_mut();
            values[q] = (values[q] * factor).clamp(spec.min, spec.max);
        }
        self.after_change(band);
        gtk::glib::Propagation::Stop
    }

    /// A double-click puts a band back flat.
    fn flatten_at(&self, x: f64, y: f64) {
        let Some(band) = self.nearest(x, y) else {
            return;
        };
        let layout = BAND_LAYOUT[band];
        {
            let mut values = self.values.borrow_mut();
            match layout.gain {
                Some(gain) => values[gain] = 0.0,
                // The low cut has no gain: flat is off.
                None => values[layout.freq] = LOW_CUT_OFF,
            }
            if let Some(q) = layout.q {
                values[q] = PARAMS[q].default;
            }
        }
        self.after_change(band);
    }

    fn dragging(&self) -> bool {
        // A band stays the hovered one while it is dragged, whatever the
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
        let (width, height) = self.size();
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
        self.hovered.set(Some(band));
        self.select(band);
        self.say();
        self.show_preset();
        self.area.queue_draw();
        let values = self.values.borrow().clone();
        let changed = self.changed.borrow().clone();
        if let Some(changed) = changed {
            changed(&values);
        }
    }

    fn size(&self) -> (f64, f64) {
        (f64::from(self.area.width()), f64::from(self.area.height()))
    }

    /// Where a band's handle is drawn.
    fn handle(&self, band: usize) -> (f64, f64) {
        let (width, height) = self.size();
        handle(&self.values.borrow(), band, width, height)
    }

    /// The band whose handle is nearest the pointer, if one is in reach.
    fn nearest(&self, x: f64, y: f64) -> Option<usize> {
        (0..BAND_LAYOUT.len())
            .map(|band| {
                let (hx, hy) = self.handle(band);
                (band, (hx - x).hypot(hy - y))
            })
            .filter(|(_, distance)| *distance <= REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(band, _)| band)
    }

    /// Say what the band under the pointer is set to, or what the zone
    /// under it is, or how to set one.
    fn say(&self) {
        if let Some(band) = self.hovered.get() {
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
            if let Some(zone) = ZONES.iter().find(|z| freq >= z.from && freq < z.to) {
                parts.push(format!("in {}", zone.name));
            }
            self.readout.set_label(&parts.join("  ·  "));
        } else if let Some(zone) = self.zone.get() {
            let zone = &ZONES[zone];
            self.readout.set_label(&format!(
                "{} ({} – {}): {}",
                zone.name,
                format_freq(zone.from),
                format_freq(zone.to),
                zone.advice
            ));
        } else {
            self.readout.set_label(
                "Drag a point to move a band, scroll over a bell to widen it, \
                 double-click to put it back. Point at a zone to see what it does.",
            );
        }
    }

    fn draw(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let values = self.values.borrow();
        let fg = self.area.color();

        // The ground.
        rounded(cr, 0.0, 0.0, width, height, 10.0);
        paint(cr, &fg, 0.04);
        let _ = cr.fill();

        self.draw_zones(cr, &fg, width, height);
        let flat = draw_grid(cr, &fg, width, height);
        const POINTS: usize = 240;
        let xs: Vec<f64> = (0..=POINTS)
            .map(|i| MARGIN + (width - 2.0 * MARGIN) * i as f64 / POINTS as f64)
            .collect();
        self.draw_bands(cr, &values, &xs, flat, width, height);
        draw_curve(cr, &fg, &values, &xs, width, height);
        self.draw_handles(cr, &fg, &values, width, height);
    }

    /// The zones: every other one shaded, the one pointed at more, each
    /// named along the top.
    fn draw_zones(&self, cr: &gtk::cairo::Context, fg: &gtk::gdk::RGBA, width: f64, height: f64) {
        let top = ZONE_BAR;
        cr.set_font_size(10.0);
        for (index, zone) in ZONES.iter().enumerate() {
            let from = freq_to_x(zone.from, width);
            let to = freq_to_x(zone.to, width);
            let pointed = self.zone.get() == Some(index);
            let alpha = if pointed {
                0.07
            } else if index % 2 == 1 {
                0.025
            } else {
                0.0
            };
            if alpha > 0.0 {
                cr.rectangle(from, top, to - from, height - top - MARGIN);
                paint(cr, fg, alpha);
                let _ = cr.fill();
            }
            let Ok(extents) = cr.text_extents(zone.name) else {
                continue;
            };
            if extents.width() < to - from - 4.0 {
                cr.move_to((from + to) / 2.0 - extents.width() / 2.0, top - 5.0);
                paint(cr, fg, if pointed { 0.85 } else { 0.4 });
                let _ = cr.show_text(zone.name);
            }
        }
    }

    /// Each band's own shape, in its colour, the picked one plainer.
    fn draw_bands(
        &self,
        cr: &gtk::cairo::Context,
        values: &[f32],
        xs: &[f64],
        flat: f64,
        width: f64,
        height: f64,
    ) {
        let filters = eq::design(values);
        for (band, filter) in filters.iter().enumerate() {
            if !band_on(values, band) {
                continue;
            }
            let picked = self.selected.get() == band || self.hovered.get() == Some(band);
            cr.move_to(xs[0], flat);
            for x in xs {
                let db = filter.response_db(x_to_freq(*x, width));
                cr.line_to(*x, db_to_y(db, height));
            }
            cr.line_to(xs[xs.len() - 1], flat);
            cr.close_path();
            tint(cr, BAND_COLOURS[band], if picked { 0.22 } else { 0.1 });
            let _ = cr.fill_preserve();
            cr.set_line_width(1.0);
            tint(cr, BAND_COLOURS[band], if picked { 0.7 } else { 0.35 });
            let _ = cr.stroke();
        }
    }

    /// The handles, each in its colour, the one pointed at larger.
    fn draw_handles(
        &self,
        cr: &gtk::cairo::Context,
        fg: &gtk::gdk::RGBA,
        values: &[f32],
        width: f64,
        height: f64,
    ) {
        for band in 0..BAND_LAYOUT.len() {
            // A band that is off, and has no gain to show it by, is grey.
            let colour = (band_on(values, band) || BAND_LAYOUT[band].gain.is_some())
                .then_some(BAND_COLOURS[band]);
            draw_handle(
                cr,
                fg,
                handle(values, band, width, height),
                colour,
                self.hovered.get() == Some(band),
                self.selected.get() == band,
            );
        }
    }
}

/// One band's handle: in its colour, or grey, larger when pointed at, and
/// ringed more firmly when pointed at or picked.
fn draw_handle(
    cr: &gtk::cairo::Context,
    fg: &gtk::gdk::RGBA,
    (x, y): (f64, f64),
    colour: Option<(f64, f64, f64)>,
    active: bool,
    picked: bool,
) {
    let radius = if active { HANDLE + 2.0 } else { HANDLE };
    cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
    match colour {
        Some(colour) => tint(cr, colour, 1.0),
        None => paint(cr, fg, 0.35),
    }
    let _ = cr.fill_preserve();
    let firm = active || picked;
    cr.set_line_width(if firm { 2.5 } else { 1.5 });
    paint(cr, fg, if firm { 0.95 } else { 0.5 });
    let _ = cr.stroke();
}

/// Paint with the foreground colour, at some opacity.
fn paint(cr: &gtk::cairo::Context, color: &gtk::gdk::RGBA, alpha: f64) {
    cr.set_source_rgba(
        color.red().into(),
        color.green().into(),
        color.blue().into(),
        alpha,
    );
}

/// Paint with one of the bands' colours, at some opacity.
fn tint(cr: &gtk::cairo::Context, (r, g, b): (f64, f64, f64), alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}

/// Decades and the usual landmarks across, decibels along, and their
/// figures. Says where the flat line is.
fn draw_grid(cr: &gtk::cairo::Context, fg: &gtk::gdk::RGBA, width: f64, height: f64) -> f64 {
    let top = ZONE_BAR;
    cr.set_line_width(1.0);
    for freq in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
        let x = freq_to_x(freq, width).round() + 0.5;
        cr.move_to(x, top);
        cr.line_to(x, height - MARGIN);
    }
    paint(cr, fg, 0.06);
    let _ = cr.stroke();
    for db in [-12.0, -6.0, 6.0, 12.0] {
        let y = db_to_y(db, height).round() + 0.5;
        cr.move_to(MARGIN, y);
        cr.line_to(width - MARGIN, y);
    }
    paint(cr, fg, 0.05);
    let _ = cr.stroke();
    let flat = db_to_y(0.0, height).round() + 0.5;
    cr.move_to(MARGIN, flat);
    cr.line_to(width - MARGIN, flat);
    paint(cr, fg, 0.18);
    let _ = cr.stroke();

    paint(cr, fg, 0.45);
    for (freq, text) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        let x = freq_to_x(freq, width);
        cr.move_to(x + 3.0, height - MARGIN - 3.0);
        let _ = cr.show_text(text);
    }
    for (db, text) in [(12.0, "+12"), (-12.0, "-12")] {
        cr.move_to(MARGIN + 3.0, db_to_y(db, height) - 3.0);
        let _ = cr.show_text(text);
    }
    flat
}

/// The curve all the bands make together.
fn draw_curve(
    cr: &gtk::cairo::Context,
    fg: &gtk::gdk::RGBA,
    values: &[f32],
    xs: &[f64],
    width: f64,
    height: f64,
) {
    for (i, x) in xs.iter().enumerate() {
        let y = db_to_y(eq::response(values, x_to_freq(*x, width)), height);
        if i == 0 {
            cr.move_to(*x, y);
        } else {
            cr.line_to(*x, y);
        }
    }
    cr.set_line_width(2.5);
    paint(cr, fg, 0.92);
    let _ = cr.stroke();
}

/// A small disc of a band's colour, for its chip.
fn dot(colour: (f64, f64, f64)) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(10);
    area.set_content_height(10);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |_, cr, width, height| {
        let (r, g, b) = colour;
        cr.set_source_rgb(r, g, b);
        let radius = f64::from(width.min(height)) / 2.0;
        cr.arc(
            f64::from(width) / 2.0,
            f64::from(height) / 2.0,
            radius,
            0.0,
            std::f64::consts::TAU,
        );
        let _ = cr.fill();
    });
    area
}

/// Whether a band does anything: the low cut is off at its lowest, and a
/// band at no gain is flat.
fn band_on(values: &[f32], band: usize) -> bool {
    let layout = BAND_LAYOUT[band];
    match layout.gain {
        Some(gain) => values[gain].abs() >= 0.05,
        None => values[layout.freq] > LOW_CUT_OFF,
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

/// Where flat is, halfway down what is left under the zones' names.
fn middle(height: f64) -> f64 {
    ZONE_BAR + (height - ZONE_BAR - MARGIN) / 2.0
}

/// How far from flat full scale is.
fn half(height: f64) -> f64 {
    (height - ZONE_BAR - MARGIN) / 2.0 - 4.0
}

fn db_to_y(db: f32, height: f64) -> f64 {
    let at = f64::from((db / RANGE_DB).clamp(-1.0, 1.0));
    middle(height) - at * half(height)
}

fn y_to_db(y: f64, height: f64) -> f32 {
    ((middle(height) - y) / half(height)) as f32 * RANGE_DB
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
            let y = db_to_y(db, 220.0);
            assert!((y_to_db(y, 220.0) - db).abs() < 1e-3, "{db}");
        }
    }

    #[test]
    fn the_zones_cover_the_range_end_to_end() {
        assert_eq!(ZONES[0].from, MIN_FREQ);
        assert_eq!(ZONES[ZONES.len() - 1].to, MAX_FREQ);
        for pair in ZONES.windows(2) {
            assert_eq!(pair[0].to, pair[1].from);
        }
        assert_eq!(BAND_COLOURS.len(), BAND_LAYOUT.len());
    }
}
