//! The effects tab of a channel.
//!
//! A channel is treated before any mix hears it. Adding or taking off an
//! effect makes the chain again; turning one of its controls does not, and
//! is sent the moment it moves.
//!
//! The tab only lists the chain. An effect's controls are in a window of
//! their own, opened from its card, so the ones being worked on can sit
//! beside the mixer rather than inside the channel's window.

use std::cell::{Cell as StdCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::stereotool::Status;
use pipedeck_engine::{vst3::Plugin, Command, Effect, EffectKind, Learning, SourceId};

use crate::comp_graph::CompGraph;
use crate::deesser_graph::DeEsserGraph;
use crate::denoise_graph::DenoiseGraph;
use crate::effects;
use crate::engine_link::EngineLink;
use crate::eq_graph::EqGraph;
use crate::widgets;

/// The channel whose chain this is. Only a channel runs effects: a mix is
/// what comes out of them.
pub type Target = SourceId;

/// What the tab says about where the chain runs.
const HINT: &str = "Every mix hears this channel through these, in order. Adding or taking \
                    one off stops the audio for a moment; turning a control does not.";

/// How long the compressor listens to a voice before setting itself.
const LEARN_SECONDS: u32 = 5;

/// How long after a control was last moved here the engine's answers are
/// taken to be about that move.
const SETTLE: Duration = Duration::from_secs(1);

pub struct EffectPanel {
    engine: EngineLink,
    target: Target,
    page: gtk::Box,
    /// The chain as it is drawn.
    list: gtk::Box,
    add: gtk::MenuButton,
    /// What was last drawn, so an echo of our own change does not redraw it
    /// under the pointer.
    shown: RefCell<Vec<Effect>>,
    /// Whether anything has been drawn at all. An empty chain is what a
    /// panel starts with, so without this the first state to arrive would
    /// match and the list would be left blank rather than told it is empty.
    drawn: StdCell<bool>,
    /// When a control was last moved here. The engine answers every move
    /// with the chain as it then stood, and while a control is dragged
    /// those answers trail behind it: taken as news, they would put the
    /// control back where it was a moment ago, under the pointer.
    touched: StdCell<Option<Instant>>,
    /// The compressor that has just been told to set itself from what it
    /// heard, until its window is drawn again with the result.
    learnt: StdCell<Option<usize>>,
    /// How many times each effect has been told to listen, by its place,
    /// so a count left behind by a window drawn again knows it is not that
    /// effect's latest.
    sessions: RefCell<std::collections::HashMap<usize, u64>>,
    /// The plug-ins the engine found installed.
    plugins: RefCell<Vec<Plugin>>,
    /// Where Stereo Tool stands, which decides whether it is offered at all.
    stereotool: RefCell<Status>,
    /// The settings windows open, one per effect at most.
    windows: RefCell<Vec<SettingsWindow>>,
    /// The graphs drawn of the effects that say what they do as they run,
    /// by their place.
    live: RefCell<std::collections::HashMap<usize, LiveGraph>>,
}

/// The window holding one effect's controls.
/// A graph that draws what its effect is doing as it runs.
enum LiveGraph {
    Compressor(std::rc::Weak<CompGraph>),
    DeEsser(std::rc::Weak<DeEsserGraph>),
    Denoise(std::rc::Weak<DenoiseGraph>),
}

impl LiveGraph {
    fn show(&self, level: f32, reduction: f32) {
        match self {
            LiveGraph::Compressor(graph) => {
                if let Some(graph) = graph.upgrade().filter(|g| g.root.is_mapped()) {
                    graph.set_live(level, reduction);
                }
            }
            LiveGraph::DeEsser(graph) => {
                if let Some(graph) = graph.upgrade().filter(|g| g.root.is_mapped()) {
                    graph.set_live(level, reduction);
                }
            }
            LiveGraph::Denoise(graph) => {
                if let Some(graph) = graph.upgrade().filter(|g| g.root.is_mapped()) {
                    graph.set_live(level, reduction);
                }
            }
        }
    }
}

struct SettingsWindow {
    /// Where the effect is in the chain.
    position: usize,
    /// What the effect is, so a window is closed when another takes its
    /// place.
    label: String,
    window: adw::Window,
    /// What the controls are drawn in, drawn again when the chain changes
    /// under them.
    body: gtk::Box,
    /// The graph drawn in it, held here and nowhere else: its handlers
    /// know it only weakly, so it goes with the window, or when the window
    /// is drawn again.
    kept: RefCell<Option<Kept>>,
}

/// A graph a settings window holds, of whichever kind.
type Kept = Rc<dyn std::any::Any>;

impl EffectPanel {
    pub fn new(engine: &EngineLink, target: Target) -> Rc<Self> {
        let this = Rc::new(Self {
            engine: engine.clone(),
            target,
            page: gtk::Box::new(gtk::Orientation::Vertical, 12),
            list: gtk::Box::new(gtk::Orientation::Vertical, 8),
            add: gtk::MenuButton::new(),
            shown: RefCell::new(Vec::new()),
            drawn: StdCell::new(false),
            touched: StdCell::new(None),
            learnt: StdCell::new(None),
            sessions: RefCell::new(std::collections::HashMap::new()),
            plugins: RefCell::new(Vec::new()),
            stereotool: RefCell::new(Status::Absent),
            windows: RefCell::new(Vec::new()),
            live: RefCell::new(std::collections::HashMap::new()),
        });
        this.build();
        this
    }

    /// The tab itself, to put in a stack.
    pub fn widget(&self) -> gtk::Widget {
        self.page.clone().upcast()
    }

    fn build(self: &Rc<Self>) {
        self.page.set_margin_top(12);

        let hint = gtk::Label::new(Some(HINT));
        hint.add_css_class("caption");
        hint.add_css_class("dim-label");
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        self.page.append(&hint);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&self.list));
        self.page.append(&scroller);

        self.add.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("Add effect")
                .build(),
        ));
        self.add
            .set_tooltip_text(Some("Add an effect to this channel"));
        self.add.set_halign(gtk::Align::Center);
        self.add.set_popover(Some(&self.popover()));
        self.page.append(&self.add);
    }

    /// Push engine state into the tab.
    pub fn refresh(self: &Rc<Self>, effects: &[Effect], plugins: &[Plugin], stereotool: &Status) {
        let offered_changed =
            self.plugins.borrow().as_slice() != plugins || *self.stereotool.borrow() != *stereotool;
        if offered_changed {
            *self.plugins.borrow_mut() = plugins.to_vec();
            *self.stereotool.borrow_mut() = stereotool.clone();
            self.add.set_popover(Some(&self.popover()));
        }
        self.show(effects);
    }

    /// Show the chain as the engine now has it, a control's setting
    /// included.
    pub fn set_effects(self: &Rc<Self>, effects: &[Effect]) {
        self.show(effects);
    }

    /// Redraw the chain, unless it is already what is on screen.
    fn show(self: &Rc<Self>, effects: &[Effect]) {
        if self.drawn.get() {
            // An effect told to set itself from a voice answers with its
            // settings, and that answer is drawn whatever was touched just
            // before, and even when it is what was there already: the
            // window drawn again is how it says it learnt.
            let learning = self.learnt.get().is_some();
            let shown = self.shown.borrow();
            if shown.as_slice() == effects {
                drop(shown);
                if learning {
                    self.redraw_windows();
                }
                return;
            }
            // The same chain set otherwise: the cards show no control, so
            // only the windows have anything to draw again, and not while a
            // control is being moved here.
            if same_chain(&shown, effects) {
                drop(shown);
                if !learning && self.touched.get().is_some_and(|at| at.elapsed() < SETTLE) {
                    return;
                }
                *self.shown.borrow_mut() = effects.to_vec();
                self.redraw_windows();
                return;
            }
        }
        self.drawn.set(true);
        *self.shown.borrow_mut() = effects.to_vec();

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        if effects.is_empty() {
            let empty = gtk::Label::new(Some("No effect on this channel."));
            empty.add_css_class("dim-label");
            empty.set_margin_top(24);
            self.list.append(&empty);
        }
        for (position, effect) in effects.iter().enumerate() {
            let row = self.row(position, effect);
            self.list.append(&row);
        }
        self.redraw_windows();
    }

    /// Show what the running effects are doing: their place in the chain,
    /// what each hears and how far it turns down.
    pub fn set_live(&self, levels: &[(usize, f32, f32)]) {
        let live = self.live.borrow();
        for (position, level, reduction) in levels {
            if let Some(graph) = live.get(position) {
                graph.show(*level, *reduction);
            }
        }
    }

    /// Open the controls of the effect at `position` in a window of their
    /// own, or bring that window forward if it is already open.
    pub fn open_settings(self: &Rc<Self>, position: usize) {
        let open = self
            .windows
            .borrow()
            .iter()
            .find(|open| open.position == position)
            .map(|open| open.window.clone());
        if let Some(window) = open {
            window.present();
            return;
        }
        let Some(effect) = self.shown.borrow().get(position).cloned() else {
            return;
        };
        let Some(spec) = effects::spec(&effect) else {
            return;
        };

        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.set_margin_top(12);
        body.set_margin_bottom(18);
        body.set_margin_start(18);
        body.set_margin_end(18);
        let kept = self.settings_body(position, &effect, spec, &body);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&body));

        // Kept above the mixer, and gone with it, but not modal: the point
        // of a window apart is to keep working beside it.
        let window = adw::Window::builder()
            .title(spec.name)
            .default_width(match spec.id {
                "eq" => 720,
                "compressor" | "deesser" | "denoise" => 540,
                _ => 420,
            })
            // As tall as its controls: without a height asked for, a
            // libadwaita window is never under 200 pixels, and a single
            // slider sits above a gap.
            .height_request(0)
            .content(&view)
            .destroy_with_parent(true)
            .build();
        if let Some(parent) = self.page.root().and_downcast::<gtk::Window>() {
            window.set_transient_for(Some(&parent));
            window.set_application(parent.application().as_ref());
        }
        window.connect_close_request({
            let this = Rc::downgrade(self);
            move |window| {
                if let Some(this) = this.upgrade() {
                    this.windows
                        .borrow_mut()
                        .retain(|open| open.window != *window);
                }
                gtk::glib::Propagation::Proceed
            }
        });
        self.windows.borrow_mut().push(SettingsWindow {
            position,
            label: effect.label.clone(),
            window: window.clone(),
            body,
            kept: RefCell::new(kept),
        });
        window.present();
    }

    /// Close every settings window, as the channel's own window goes.
    pub fn close_windows(&self) {
        let open = self.windows.take();
        for open in open {
            open.window.close();
        }
    }

    /// Follow a chain that changed: a window whose effect is still where it
    /// was is drawn again with what it is now set to, and one whose effect
    /// went away is closed.
    fn redraw_windows(self: &Rc<Self>) {
        let shown = self.shown.borrow().clone();
        let (kept, gone): (Vec<_>, Vec<_>) = self.windows.take().into_iter().partition(|open| {
            shown
                .get(open.position)
                .is_some_and(|effect| effects::spec(effect).is_some() && effect.label == open.label)
        });
        *self.windows.borrow_mut() = kept;
        for open in gone {
            open.window.close();
        }
        let windows = self.windows.borrow();
        for open in windows.iter() {
            let effect = &shown[open.position];
            let Some(spec) = effects::spec(effect) else {
                continue;
            };
            while let Some(child) = open.body.first_child() {
                open.body.remove(&child);
            }
            *open.kept.borrow_mut() = self.settings_body(open.position, effect, spec, &open.body);
        }
    }

    /// The controls of one of the mixer's own effects.
    fn settings_body(
        self: &Rc<Self>,
        position: usize,
        effect: &Effect,
        spec: &'static pipedeck_engine::dsp::EffectSpec,
        body: &gtk::Box,
    ) -> Option<Kept> {
        if spec.id == "eq" {
            Some(self.equaliser_body(position, effect, body))
        } else if matches!(spec.id, "compressor" | "deesser" | "denoise") {
            Some(self.graph_body(position, effect, spec, body))
        } else {
            self.controls_body(position, effect, spec, body);
            None
        }
    }

    fn row(self: &Rc<Self>, position: usize, effect: &Effect) -> gtk::Widget {
        let (card, inner) = widgets::list_card();
        let (top, title) = widgets::card_title(&effect.name);
        if let Some(spec) = effects::spec(effect) {
            title.set_tooltip_text(Some(spec.description));
        }

        // Stereo Tool's own interface: every band and every curve it has,
        // and the only place its settings can be made, since they live in
        // Stereo Tool and not here. The builds for a machine without X11
        // carry no window, and then there is none to offer.
        if effect.kind == EffectKind::StereoTool
            && matches!(&*self.stereotool.borrow(), Status::Ready(info) if info.windows)
        {
            // Opening is all this does. Closing is the window's own
            // business: it lives in one of the mixer's, which the window
            // manager can close like any other.
            let open = gtk::Button::with_label("Window");
            open.add_css_class("flat");
            open.set_tooltip_text(Some("Open Stereo Tool's own window"));
            open.connect_clicked({
                let this = Rc::downgrade(self);
                move |_| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    this.engine.send(Command::SetEffectWindow {
                        id: this.target,
                        index: position,
                        open: true,
                    })
                }
            });
            top.append(&open);
        }

        // On or bypassed: the sound goes past it, its settings kept.
        let on = gtk::Switch::new();
        on.set_active(!effect.bypassed);
        on.set_valign(gtk::Align::Center);
        on.set_tooltip_text(Some("Switch this effect off or on"));
        if effect.bypassed {
            title.add_css_class("dim-label");
        }
        on.connect_active_notify({
            let this = Rc::downgrade(self);
            move |on| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                this.engine.send(Command::SetEffectBypass {
                    id: this.target,
                    index: position,
                    bypassed: !on.is_active(),
                });
            }
        });
        top.append(&on);

        if effects::spec(effect).is_some() {
            let settings = gtk::Button::from_icon_name("emblem-system-symbolic");
            settings.add_css_class("flat");
            settings.set_tooltip_text(Some("Open its settings"));
            settings.connect_clicked({
                let this = Rc::downgrade(self);
                move |_| {
                    if let Some(this) = this.upgrade() {
                        this.open_settings(position);
                    }
                }
            });
            top.append(&settings);
        }

        let remove = gtk::Button::from_icon_name("list-remove-symbolic");
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some("Take this effect off"));
        remove.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let mut chain = this.shown.borrow().clone();
                if position < chain.len() {
                    chain.remove(position);
                }
                this.send(chain);
            }
        });
        top.append(&remove);
        inner.append(&top);

        // Stereo Tool keeps its preset on the card. The mixer's own effects
        // keep their controls in their window, and an effect from an older
        // chain, of a kind no longer offered, still runs but has nothing to
        // set.
        if effect.kind == EffectKind::StereoTool {
            self.stereotool_body(position, effect, &inner);
        }
        card.upcast()
    }

    /// Send what one effect of the chain is now set to. The engine hands it
    /// to the effect where it runs: nothing is reloaded, so there is no need
    /// to wait for a control to settle.
    fn send_params(&self, position: usize, effect: &Effect) {
        self.touched.set(Some(Instant::now()));
        self.engine.send(Command::SetEffectParams {
            id: self.target,
            index: position,
            controls: effect.controls.clone(),
        });
    }

    /// Write one value into the chain as drawn and send it.
    fn set_param(&self, position: usize, name: &str, value: f32) {
        let mut shown = self.shown.borrow_mut();
        let Some(effect) = shown.get_mut(position) else {
            return;
        };
        effects::set_value(effect, name, value);
        let effect = effect.clone();
        drop(shown);
        self.send_params(position, &effect);
    }

    /// A slider for each control, with its unit.
    fn controls_body(
        self: &Rc<Self>,
        position: usize,
        effect: &Effect,
        spec: &'static pipedeck_engine::dsp::EffectSpec,
        inner: &gtk::Box,
    ) {
        for param in spec.params {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

            let name = gtk::Label::new(Some(param.label));
            name.add_css_class("caption");
            name.set_xalign(0.0);
            name.set_width_chars(10);
            row.append(&name);

            let (min, max) = (f64::from(param.min), f64::from(param.max));
            let scale =
                gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, (max - min) / 200.0);
            scale.set_hexpand(true);
            scale.set_value(f64::from(effects::value_of(effect, param)));

            // The value in a label of its own, as wide as the widest one, so
            // every slider of the window is the same length.
            let value = gtk::Label::new(Some(&effects::format(param, scale.value() as f32)));
            value.add_css_class("caption");
            value.add_css_class("numeric");
            value.set_width_chars(8);
            value.set_xalign(1.0);

            scale.connect_value_changed({
                let this = Rc::downgrade(self);
                let value = value.clone();
                move |scale| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    let now = scale.value() as f32;
                    value.set_text(&effects::format(param, now));
                    this.set_param(position, param.name, now);
                }
            });
            row.append(&scale);
            row.append(&value);
            inner.append(&row);
        }
    }

    /// A button that has an effect listen to a voice for a few seconds and
    /// set itself from it, which the sliders under it can then adjust.
    fn learn_row(self: &Rc<Self>, position: usize, id: &str) -> gtk::Widget {
        // The de-esser needs to hear s to find them.
        let ask = if id == "deesser" {
            "Say a few sentences with s and sh in them while it listens."
        } else {
            "Speak as you do on air while it listens."
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_margin_bottom(6);

        let button = gtk::Button::with_label("Learn from my voice");
        row.append(&button);

        // Drawn again with what it learnt: the one sign that it did.
        let said = gtk::Label::new(Some(if self.learnt.get() == Some(position) {
            self.learnt.set(None);
            "Set from your voice. The sliders can take it from there."
        } else {
            ask
        }));
        said.add_css_class("caption");
        said.add_css_class("dim-label");
        said.set_xalign(0.0);
        said.set_wrap(true);
        said.set_hexpand(true);
        row.append(&said);

        button.connect_clicked({
            let this = Rc::downgrade(self);
            let said = said.clone();
            move |button| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let learn = |step| {
                    this.engine.send(Command::LearnEffect {
                        id: this.target,
                        index: position,
                        step,
                    })
                };
                learn(Learning::Start);
                // Which listening this is: a newer one, started from the
                // button of a window drawn again since, is not this one's
                // to cancel.
                let session = {
                    let mut sessions = this.sessions.borrow_mut();
                    let session = sessions.entry(position).or_default();
                    *session += 1;
                    *session
                };
                button.set_sensitive(false);
                let left = StdCell::new(LEARN_SECONDS);
                said.set_text(&format!("Listening… {}", left.get()));
                gtk::glib::timeout_add_seconds_local(1, {
                    let this = this.clone();
                    let button = button.clone();
                    let said = said.clone();
                    move || {
                        let learn = |step| {
                            this.engine.send(Command::LearnEffect {
                                id: this.target,
                                index: position,
                                step,
                            })
                        };
                        // The window closed, or was drawn again for a chain
                        // that changed: what was being heard is dropped,
                        // unless listening has started again since.
                        if button.root().is_none() {
                            if this.sessions.borrow().get(&position) == Some(&session) {
                                learn(Learning::Cancel);
                            }
                            return gtk::glib::ControlFlow::Break;
                        }
                        left.set(left.get() - 1);
                        if left.get() > 0 {
                            said.set_text(&format!("Listening… {}", left.get()));
                            return gtk::glib::ControlFlow::Continue;
                        }
                        learn(Learning::Finish);
                        this.learnt.set(Some(position));
                        said.set_text("Working it out…");
                        // The settings come back within a moment, and the
                        // window is drawn again with them. Still here after
                        // that, it learnt nothing.
                        gtk::glib::timeout_add_local_once(Duration::from_millis(1500), {
                            let this = this.clone();
                            let button = button.clone();
                            let said = said.clone();
                            move || {
                                // The answer, or none: either way this
                                // effect is no longer waiting, even with its
                                // window closed meanwhile.
                                let waiting = this.learnt.get() == Some(position);
                                if waiting {
                                    this.learnt.set(None);
                                }
                                if button.root().is_none() || !waiting {
                                    return;
                                }
                                button.set_sensitive(true);
                                said.set_text(
                                    "Too little was heard. Try again, speaking through the \
                                     whole count.",
                                );
                            }
                        });
                        gtk::glib::ControlFlow::Break
                    }
                });
            }
        });
        row.upcast()
    }

    /// The compressor, the de-esser or noise suppression, as what it does
    /// with a handle to drag, under the button that sets it from a voice
    /// for the ones that learn.
    fn graph_body(
        self: &Rc<Self>,
        position: usize,
        effect: &Effect,
        spec: &'static pipedeck_engine::dsp::EffectSpec,
        inner: &gtk::Box,
    ) -> Kept {
        if pipedeck_engine::dsp::learns(spec.id) {
            inner.append(&self.learn_row(position, spec.id));
        }
        let values: Vec<f32> = spec
            .params
            .iter()
            .map(|param| effects::value_of(effect, param))
            .collect();
        let changed = {
            let this = Rc::downgrade(self);
            move |values: &[f32]| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let mut shown = this.shown.borrow_mut();
                let Some(effect) = shown.get_mut(position) else {
                    return;
                };
                for (param, value) in spec.params.iter().zip(values) {
                    effects::set_value(effect, param.name, *value);
                }
                let effect = effect.clone();
                drop(shown);
                this.send_params(position, &effect);
            }
        };
        match spec.id {
            "compressor" => {
                let graph = CompGraph::new(&values);
                graph.connect_changed(changed);
                inner.append(&graph.root);
                self.live
                    .borrow_mut()
                    .insert(position, LiveGraph::Compressor(Rc::downgrade(&graph)));
                graph
            }
            "deesser" => {
                let graph = DeEsserGraph::new(&values);
                graph.connect_changed(changed);
                inner.append(&graph.root);
                self.live
                    .borrow_mut()
                    .insert(position, LiveGraph::DeEsser(Rc::downgrade(&graph)));
                graph
            }
            _ => {
                let graph = DenoiseGraph::new(&values);
                graph.connect_changed(changed);
                inner.append(&graph.root);
                self.live
                    .borrow_mut()
                    .insert(position, LiveGraph::Denoise(Rc::downgrade(&graph)));
                graph
            }
        }
    }

    /// The equaliser, as a curve with a handle on each band.
    fn equaliser_body(self: &Rc<Self>, position: usize, effect: &Effect, inner: &gtk::Box) -> Kept {
        use pipedeck_engine::dsp::eq::PARAMS;
        let values: Vec<f32> = PARAMS
            .iter()
            .map(|param| effects::value_of(effect, param))
            .collect();
        let graph = EqGraph::new(&values);
        graph.connect_changed({
            let this = Rc::downgrade(self);
            move |values| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let mut shown = this.shown.borrow_mut();
                let Some(effect) = shown.get_mut(position) else {
                    return;
                };
                for (param, value) in PARAMS.iter().zip(values) {
                    effects::set_value(effect, param.name, *value);
                }
                let effect = effect.clone();
                drop(shown);
                this.send_params(position, &effect);
            }
        });
        graph.root.set_margin_top(4);
        inner.append(&graph.root);
        graph
    }

    /// Stereo Tool has no controls of ours: it is configured by the preset
    /// exported from the application itself, which is what this picks.
    fn stereotool_body(self: &Rc<Self>, position: usize, effect: &Effect, inner: &gtk::Box) {
        let preset = effect.preset().map(PathBuf::from);

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some("Preset"));
        name.add_css_class("caption");
        name.set_xalign(0.0);
        name.set_width_chars(9);
        row.append(&name);

        let chosen = gtk::Label::new(Some(&match &preset {
            Some(path) => path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            None => "Default settings".to_owned(),
        }));
        chosen.add_css_class("caption");
        chosen.add_css_class("dim-label");
        chosen.set_hexpand(true);
        chosen.set_xalign(0.0);
        chosen.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        if let Some(path) = &preset {
            chosen.set_tooltip_text(Some(&path.display().to_string()));
        }
        row.append(&chosen);

        let choose = gtk::Button::with_label("Choose…");
        choose.add_css_class("flat");
        choose.connect_clicked({
            let this = Rc::downgrade(self);
            move |button| {
                if let Some(this) = this.upgrade() {
                    this.pick_preset(position, button.upcast_ref());
                }
            }
        });
        row.append(&choose);

        if preset.is_some() {
            let clear = gtk::Button::from_icon_name("edit-clear-symbolic");
            clear.add_css_class("flat");
            clear.set_tooltip_text(Some("Go back to the settings it starts with"));
            clear.connect_clicked({
                let this = Rc::downgrade(self);
                move |_| {
                    if let Some(this) = this.upgrade() {
                        this.set_preset(position, None);
                    }
                }
            });
            row.append(&clear);
        }
        inner.append(&row);

        // An unlicensed copy still runs, and puts speech and beeps in the
        // audio. Better said here than discovered on air.
        if let Status::Ready(info) = &*self.stereotool.borrow() {
            if !info.licensed {
                let said = match &info.unlicensed {
                    Some(features) => format!(
                        "No licence for: {features}. Stereo Tool adds speech and beeps to the \
                         audio until it has one."
                    ),
                    None => {
                        "No licence key. Stereo Tool adds speech and beeps to the audio.".to_owned()
                    }
                };
                let warning = gtk::Label::new(Some(&said));
                warning.add_css_class("caption");
                warning.add_css_class("warning");
                warning.set_xalign(0.0);
                warning.set_wrap(true);
                inner.append(&warning);
            }
        }
    }

    /// Ask for a preset file and put it on the stage at `position`.
    fn pick_preset(self: &Rc<Self>, position: usize, near: &gtk::Widget) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Stereo Tool presets"));
        filter.add_pattern("*.sts");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let any = gtk::FileFilter::new();
        any.set_name(Some("All files"));
        any.add_pattern("*");
        filters.append(&any);

        let dialog = gtk::FileDialog::new();
        dialog.set_title("Choose a Stereo Tool preset");
        dialog.set_filters(Some(&filters));
        let window = near.root().and_downcast::<gtk::Window>();
        dialog.open(window.as_ref(), None::<&gtk::gio::Cancellable>, {
            let this = Rc::downgrade(self);
            move |answer| {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let Ok(file) = answer else {
                    return;
                };
                let Some(path) = file.path() else {
                    return;
                };
                this.set_preset(position, Some(&path));
            }
        });
    }

    fn set_preset(self: &Rc<Self>, position: usize, preset: Option<&Path>) {
        let mut chain = self.shown.borrow().clone();
        let Some(effect) = chain.get_mut(position) else {
            return;
        };
        effect.plugin = preset.map(|path| path.display().to_string());
        self.send(chain);
    }

    /// Send a chain to the engine, which makes it again.
    fn send(self: &Rc<Self>, chain: Vec<Effect>) {
        // Drawn here rather than when the engine echoes it back: the echo is
        // this very chain, and a panel that only redraws on a difference
        // would find none and leave the list as it was.
        self.shown.borrow_mut().clear();
        self.drawn.set(false);
        self.show(&chain);
        self.engine.send(Command::SetEffects {
            id: self.target,
            effects: chain,
        });
    }

    /// What this object could run.
    fn popover(self: &Rc<Self>) -> gtk::Popover {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let popover = gtk::Popover::new();

        for spec in effects::catalogue() {
            let button = entry(spec.name, spec.description);
            button.connect_clicked({
                let this = Rc::downgrade(self);
                let popover = popover.clone();
                move |_| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    let mut chain = this.shown.borrow().clone();
                    chain.push(effects::build(spec));
                    this.send(chain);
                    popover.popdown();
                }
            });
            list.append(&button);
        }

        let plugins = self.plugins.borrow();
        let stereotool = matches!(&*self.stereotool.borrow(), Status::Ready(_));
        if !plugins.is_empty() || stereotool {
            let heading = gtk::Label::new(Some("Plug-ins"));
            heading.add_css_class("caption-heading");
            heading.add_css_class("dim-label");
            heading.set_xalign(0.0);
            heading.set_margin_top(8);
            heading.set_margin_start(6);
            list.append(&heading);
        }

        if stereotool {
            let button = entry("Stereo Tool", "Thimeo's broadcast processor, on a preset");
            button.connect_clicked({
                let this = Rc::downgrade(self);
                let popover = popover.clone();
                move |_| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    let mut chain = this.shown.borrow().clone();
                    chain.push(Effect {
                        name: "Stereo Tool".to_owned(),
                        kind: EffectKind::StereoTool,
                        plugin: None,
                        label: "stereotool".to_owned(),
                        controls: Vec::new(),
                        bypassed: false,
                    });
                    this.send(chain);
                    popover.popdown();
                }
            });
            list.append(&button);
        }

        for plugin in plugins.iter() {
            let button = entry(&plugin.name, &plugin.vendor);
            button.connect_clicked({
                let this = Rc::downgrade(self);
                let popover = popover.clone();
                let plugin = plugin.clone();
                move |_| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    let mut chain = this.shown.borrow().clone();
                    chain.push(Effect {
                        name: plugin.name.clone(),
                        kind: EffectKind::Vst3,
                        plugin: None,
                        label: plugin.class_id.clone(),
                        controls: Vec::new(),
                        bypassed: false,
                    });
                    this.send(chain);
                    popover.popdown();
                }
            });
            list.append(&button);
        }
        drop(plugins);

        popover.set_child(Some(&list));
        popover
    }
}

/// Whether two chains hold the same effects in the same order, each on or
/// off alike, whatever their controls are set to: the cards show which are
/// on, not the controls.
fn same_chain(a: &[Effect], b: &[Effect]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.name == b.name
                && a.kind == b.kind
                && a.plugin == b.plugin
                && a.label == b.label
                && a.bypassed == b.bypassed
        })
}

/// One line of the menu: what it is, and what it does.
fn entry(name: &str, description: &str) -> gtk::Button {
    let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Label::new(Some(name));
    title.set_xalign(0.0);
    labels.append(&title);
    let detail = gtk::Label::new(Some(description));
    detail.add_css_class("caption");
    detail.add_css_class("dim-label");
    detail.set_xalign(0.0);
    labels.append(&detail);

    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.set_child(Some(&labels));
    button
}
