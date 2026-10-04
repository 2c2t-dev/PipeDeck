//! The compressor, drawn as what it does and set by dragging it.
//!
//! The graph is the compressor's curve: how loud a sound comes out for how
//! loud it goes in, worked out from the same sums the audio goes through.
//! Under the threshold a sound is left alone and the curve follows the
//! diagonal; over it, the curve flattens by the ratio. Three handles set
//! it: the threshold, on the bend, dragged sideways; the ratio, at the top
//! of the curve, dragged up or down; the makeup, at its foot, dragged up or
//! down to lift the whole of it. A line under the graph says in words what
//! it does to a shout.
//!
//! This depends on GTK, libadwaita and the engine's compressor only, so
//! `examples/effects_render.rs` can draw it to an image.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::dsp::{self, compressor_output, COMPRESSOR_PRESETS};

/// The quietest input drawn, in decibels.
const FLOOR: f32 = -60.0;
/// The loudest output drawn: over 0 dB is clipping, and makeup can reach
/// into it.
const CEILING: f32 = 12.0;
const HANDLE: f64 = 7.0;
const REACH: f64 = 18.0;
const MARGIN: f64 = 12.0;
/// Room over the curve for the zones' names.
const ZONE_BAR: f64 = 18.0;
/// What a shout is taken to be, for the line saying what the compressor
/// does.
const SHOUT: f32 = -6.0;

const THRESHOLD: usize = 0;
const RATIO: usize = 1;
const MAKEUP: usize = 2;

/// Each handle's colour, in the order of the controls.
const COLOURS: [(f64, f64, f64); 3] = [
    (1.000, 0.471, 0.000), // threshold, orange
    (0.384, 0.627, 0.918), // ratio, blue
    (0.200, 0.820, 0.478), // makeup, green
];

type Changed = Rc<dyn Fn(&[f32])>;

pub struct CompGraph {
    /// What goes in the layout: the presets, the graph, what it does, and
    /// a slider for each control.
    pub root: gtk::Box,
    area: gtk::DrawingArea,
    readout: gtk::Label,
    presets: gtk::MenuButton,
    scales: Vec<gtk::Scale>,
    shown: Vec<gtk::Label>,
    /// Threshold, ratio and makeup.
    values: RefCell<[f32; 3]>,
    /// The handle under the pointer or being dragged.
    hovered: Cell<Option<usize>>,
    /// Set while the sliders are given values, so they do not send them
    /// back.
    syncing: Cell<bool>,
    changed: RefCell<Option<Changed>>,
    /// What the compressor is doing as it runs, as drawn: the level it
    /// hears and how far it turns it down, both in decibels, falling back
    /// gently rather than blinking from one reading to the next.
    live: Cell<(f32, f32)>,
}

/// How far the drawn level falls between two readings, in decibels, and
/// how much of the drawn reduction each keeps: a meter's fall.
const LIVE_FALL_DB: f32 = 1.5;
const LIVE_KEEP: f32 = 0.85;

impl CompGraph {
    /// A graph showing `values`: threshold, ratio and makeup.
    pub fn new(values: &[f32]) -> Rc<Self> {
        let spec = dsp::spec("compressor").expect("the compressor is described");
        let mut initial = [0.0; 3];
        for (slot, param) in initial.iter_mut().zip(spec.params) {
            *slot = param.default;
        }
        for (slot, value) in initial.iter_mut().zip(values) {
            *slot = *value;
        }

        let area = gtk::DrawingArea::new();
        area.set_content_height(230);
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
            hovered: Cell::new(None),
            syncing: Cell::new(false),
            changed: RefCell::new(None),
            live: Cell::new((f32::NEG_INFINITY, 0.0)),
        });
        this.build();
        this.wire();
        this.sync();
        this
    }

    /// What the compressor is doing now: the loudest level it heard since
    /// the last reading and the most it turned down, in decibels.
    pub fn set_live(&self, level: f32, reduction: f32) {
        let (shown_level, shown_reduction) = self.live.get();
        let level = level.max(shown_level - LIVE_FALL_DB);
        let reduction = reduction.max(shown_reduction * LIVE_KEEP);
        if (level, reduction) != (shown_level, shown_reduction) {
            self.live.set((level, reduction));
            self.area.queue_draw();
        }
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

        let spec = dsp::spec("compressor").expect("the compressor is described");
        for (index, param) in spec.params.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&dot(COLOURS[index]));
            let name = gtk::Label::new(Some(param.label));
            name.add_css_class("caption");
            name.set_xalign(0.0);
            name.set_width_chars(10);
            row.append(&name);
            row.append(&self.scales[index]);
            row.append(&self.shown[index]);
            self.scales[index].connect_value_changed({
                let this = Rc::downgrade(self);
                move |scale| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
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
        for preset in COMPRESSOR_PRESETS {
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

        let preset = COMPRESSOR_PRESETS
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
                let near = this.nearest(x, y);
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
                    this.hovered.set(None);
                    this.say();
                    this.area.queue_draw();
                }
            }
        });
        self.area.add_controller(motion);

        // A handle moves by as much as the pointer does, from where it was.
        let drag = gtk::GestureDrag::new();
        let grabbed = Rc::new(Cell::new(None::<(usize, f64, f64)>));
        drag.connect_drag_begin({
            let this = Rc::downgrade(self);
            let grabbed = grabbed.clone();
            move |_, x, y| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let near = this.nearest(x, y);
                grabbed.set(near.map(|which| {
                    let (hx, hy) = this.handle(which);
                    (which, hx, hy)
                }));
                this.hovered.set(near);
                this.say();
                this.area.queue_draw();
            }
        });
        drag.connect_drag_update({
            let this = Rc::downgrade(self);
            let grabbed = grabbed.clone();
            move |_, dx, dy| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if let Some((which, x, y)) = grabbed.get() {
                    this.move_handle(which, x + dx, y + dy);
                }
            }
        });
        drag.connect_drag_end({
            let grabbed = grabbed.clone();
            move |_, _, _| grabbed.set(None)
        });
        self.area.add_controller(drag);

        // A double-click puts a handle back where it starts.
        let click = gtk::GestureClick::new();
        click.connect_pressed({
            let this = Rc::downgrade(self);
            move |_, presses, x, y| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                if presses != 2 {
                    return;
                }
                let Some(which) = this.nearest(x, y) else {
                    return;
                };
                let spec = dsp::spec("compressor").expect("the compressor is described");
                this.values.borrow_mut()[which] = spec.params[which].default;
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

    /// Where a handle is drawn: the threshold on the bend, the ratio at the
    /// loudest input, the makeup at the quietest.
    fn handle(&self, which: usize) -> (f64, f64) {
        let (width, height) = self.size();
        let [threshold, ratio, makeup] = *self.values.borrow();
        let input = match which {
            THRESHOLD => threshold,
            RATIO => 0.0,
            _ => FLOOR,
        };
        let output = compressor_output(input, threshold, ratio, makeup);
        (in_to_x(input, width), out_to_y(output, height))
    }

    fn nearest(&self, x: f64, y: f64) -> Option<usize> {
        (0..3)
            .map(|which| {
                let (hx, hy) = self.handle(which);
                (which, (hx - x).hypot(hy - y))
            })
            .filter(|(_, distance)| *distance <= REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(which, _)| which)
    }

    /// Set a control from where its handle is dragged to.
    fn move_handle(&self, which: usize, x: f64, y: f64) {
        let (width, height) = self.size();
        let spec = dsp::spec("compressor").expect("the compressor is described");
        {
            let mut values = self.values.borrow_mut();
            let [threshold, _, makeup] = *values;
            let value = match which {
                THRESHOLD => x_to_in(x, width),
                // The loudest input comes out at threshold + (0 - threshold)
                // / ratio + makeup; the ratio is what puts it at the pointer.
                RATIO => {
                    let over = y_to_out(y, height) - makeup - threshold;
                    if over <= 0.0 {
                        spec.params[RATIO].max
                    } else {
                        -threshold / over
                    }
                }
                _ => y_to_out(y, height) - FLOOR,
            };
            let param = &spec.params[which];
            // Tenths are all the sliders show.
            values[which] = ((value * 10.0).round() / 10.0).clamp(param.min, param.max);
        }
        self.hovered.set(Some(which));
        self.after_change();
    }

    /// Say what the handle under the pointer sets, or what the compressor
    /// does to a shout.
    fn say(&self) {
        let [threshold, ratio, makeup] = *self.values.borrow();
        let text = match self.hovered.get() {
            Some(THRESHOLD) => format!(
                "Threshold · {} · louder than this is turned down; drag it sideways",
                format(THRESHOLD, threshold)
            ),
            Some(RATIO) => format!(
                "Ratio · {} · how hard it turns down; drag it down for more",
                format(RATIO, ratio)
            ),
            Some(_) => format!(
                "Makeup · {} · turns everything back up after; drag it up or down",
                format(MAKEUP, makeup)
            ),
            None if ratio <= 1.0 => {
                format!("It turns nothing down. Everything comes out {makeup:+.1} dB louder.")
            }
            None => {
                let out = compressor_output(SHOUT, threshold, ratio, makeup);
                let down = SHOUT + makeup - out;
                format!(
                    "Over {threshold:.0} dB, every {ratio:.1} dB louder comes out 1 dB louder. \
                     A shout at {SHOUT:.0} dB is turned down {down:.1} dB and comes out at \
                     {out:.1} dB."
                )
            }
        };
        self.readout.set_label(&text);
    }

    fn draw(&self, cr: &gtk::cairo::Context, width: f64, height: f64) {
        let [threshold, ratio, makeup] = *self.values.borrow();
        let fg = self.area.color();
        let set = |cr: &gtk::cairo::Context, alpha: f64| {
            cr.set_source_rgba(fg.red().into(), fg.green().into(), fg.blue().into(), alpha);
        };
        let colour = |cr: &gtk::cairo::Context, (r, g, b): (f64, f64, f64), alpha: f64| {
            cr.set_source_rgba(r, g, b, alpha);
        };
        let (left, right) = (in_to_x(FLOOR, width), in_to_x(0.0, width));
        let (top, bottom) = (out_to_y(CEILING, height), out_to_y(FLOOR, height));

        rounded(cr, 0.0, 0.0, width, height, 10.0);
        set(cr, 0.04);
        let _ = cr.fill();

        // Left alone under the threshold, turned down over it.
        let bend = in_to_x(threshold, width);
        cr.rectangle(bend, top, right - bend, bottom - top);
        colour(cr, COLOURS[THRESHOLD], 0.07);
        let _ = cr.fill();
        // Over 0 dB out, the sound clips.
        let clip = out_to_y(0.0, height);
        cr.rectangle(left, top, right - left, clip - top);
        cr.set_source_rgba(0.878, 0.106, 0.141, 0.08);
        let _ = cr.fill();

        cr.set_font_size(10.0);
        let label = |cr: &gtk::cairo::Context, text: &str, from: f64, to: f64, alpha: f64| {
            if let Ok(extents) = cr.text_extents(text) {
                if extents.width() < to - from - 4.0 {
                    cr.move_to((from + to) / 2.0 - extents.width() / 2.0, ZONE_BAR - 5.0);
                    set(cr, alpha);
                    let _ = cr.show_text(text);
                }
            }
        };
        label(cr, "Left alone", left, bend, 0.45);
        label(cr, "Turned down", bend, right, 0.6);
        cr.move_to(left + 4.0, clip - 4.0);
        cr.set_source_rgba(0.878, 0.106, 0.141, 0.7);
        let _ = cr.show_text("Clips");

        // A grid every 12 dB, and the diagonal: what goes in comes out.
        cr.set_line_width(1.0);
        let mut db = FLOOR;
        while db <= 0.0 {
            let x = in_to_x(db, width).round() + 0.5;
            cr.move_to(x, top);
            cr.line_to(x, bottom);
            let y = out_to_y(db, height).round() + 0.5;
            cr.move_to(left, y);
            cr.line_to(right, y);
            db += 12.0;
        }
        set(cr, 0.06);
        let _ = cr.stroke();
        cr.set_dash(&[4.0, 4.0], 0.0);
        cr.move_to(left, out_to_y(FLOOR, height));
        cr.line_to(right, out_to_y(0.0, height));
        set(cr, 0.25);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);

        set(cr, 0.45);
        for db in [-48.0, -24.0, 0.0] {
            cr.move_to(in_to_x(db, width) + 3.0, bottom - 3.0);
            let _ = cr.show_text(&format!("{db:.0}"));
        }
        cr.move_to(left + 3.0, out_to_y(-24.0, height) - 3.0);
        let _ = cr.show_text("-24");

        // The curve, and under it how far it turns down, shaded.
        const POINTS: usize = 200;
        let curve: Vec<(f64, f64)> = (0..=POINTS)
            .map(|i| {
                let input = FLOOR + (0.0 - FLOOR) * i as f32 / POINTS as f32;
                let output =
                    compressor_output(input, threshold, ratio, makeup).clamp(FLOOR, CEILING);
                (in_to_x(input, width), out_to_y(output, height))
            })
            .collect();
        cr.move_to(curve[0].0, curve[0].1);
        for (x, y) in &curve[1..] {
            cr.line_to(*x, *y);
        }
        for i in (0..=POINTS).rev() {
            let input = FLOOR + (0.0 - FLOOR) * i as f32 / POINTS as f32;
            let untouched = (input + makeup).clamp(FLOOR, CEILING);
            cr.line_to(in_to_x(input, width), out_to_y(untouched, height));
        }
        cr.close_path();
        colour(cr, COLOURS[RATIO], 0.18);
        let _ = cr.fill();

        cr.move_to(curve[0].0, curve[0].1);
        for (x, y) in &curve[1..] {
            cr.line_to(*x, *y);
        }
        cr.set_line_width(2.5);
        set(cr, 0.92);
        let _ = cr.stroke();

        // The threshold as a line down to where it is read.
        cr.move_to(bend.round() + 0.5, top);
        cr.line_to(bend.round() + 0.5, bottom);
        colour(cr, COLOURS[THRESHOLD], 0.5);
        cr.set_line_width(1.0);
        let _ = cr.stroke();

        // What it is doing now: a dot where the voice is on the curve, and
        // a line down from where it would be untouched to where it comes
        // out, as long as it is turned down.
        let (level, turned) = self.live.get();
        if level > FLOOR {
            let level = level.min(0.0);
            let x = in_to_x(level, width);
            let out = compressor_output(level, threshold, ratio, makeup);
            let y = out_to_y(out, height);
            if turned > 0.1 {
                cr.move_to(x, out_to_y(level + makeup, height));
                cr.line_to(x, y);
                colour(cr, COLOURS[RATIO], 0.9);
                cr.set_line_width(3.0);
                let _ = cr.stroke();
                let text = format!("−{turned:.1} dB");
                if let Ok(extents) = cr.text_extents(&text) {
                    cr.move_to(right - extents.width() - 4.0, ZONE_BAR + 12.0);
                    colour(cr, COLOURS[RATIO], 1.0);
                    let _ = cr.show_text(&text);
                }
            }
            cr.new_sub_path();
            cr.arc(x, y, 5.0, 0.0, std::f64::consts::TAU);
            set(cr, 0.95);
            let _ = cr.fill();
        }

        for (which, handle_colour) in COLOURS.iter().enumerate() {
            let (x, y) = self.handle(which);
            let active = self.hovered.get() == Some(which);
            cr.arc(
                x,
                y.clamp(top, bottom),
                if active { HANDLE + 2.0 } else { HANDLE },
                0.0,
                std::f64::consts::TAU,
            );
            colour(cr, *handle_colour, 1.0);
            let _ = cr.fill_preserve();
            cr.set_line_width(if active { 2.5 } else { 1.5 });
            set(cr, if active { 0.95 } else { 0.5 });
            let _ = cr.stroke();
        }
    }
}

/// A control's value as the window writes it, with its unit.
fn format(which: usize, value: f32) -> String {
    match which {
        THRESHOLD => format!("{value:+.1} dB"),
        RATIO => format!("{value:.1}:1"),
        _ => format!("{value:+.1} dB"),
    }
}

/// A small disc of a handle's colour, beside its slider.
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

fn in_to_x(db: f32, width: f64) -> f64 {
    let at = f64::from((db.clamp(FLOOR, 0.0) - FLOOR) / (0.0 - FLOOR));
    MARGIN + at * (width - 2.0 * MARGIN)
}

fn x_to_in(x: f64, width: f64) -> f32 {
    let at = ((x - MARGIN) / (width - 2.0 * MARGIN)).clamp(0.0, 1.0) as f32;
    FLOOR + at * (0.0 - FLOOR)
}

fn out_to_y(db: f32, height: f64) -> f64 {
    let at = f64::from((db.clamp(FLOOR, CEILING) - FLOOR) / (CEILING - FLOOR));
    height - MARGIN - at * (height - MARGIN - ZONE_BAR)
}

fn y_to_out(y: f64, height: f64) -> f32 {
    let at = ((height - MARGIN - y) / (height - MARGIN - ZONE_BAR)) as f32;
    FLOOR + at * (CEILING - FLOOR)
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
    fn a_level_comes_back_from_where_it_is_drawn() {
        for db in [-60.0, -20.0, 0.0] {
            assert!((x_to_in(in_to_x(db, 400.0), 400.0) - db).abs() < 1e-3);
        }
        for db in [-60.0, -6.0, 0.0, 12.0] {
            assert!((y_to_out(out_to_y(db, 230.0), 230.0) - db).abs() < 1e-3);
        }
    }
}
