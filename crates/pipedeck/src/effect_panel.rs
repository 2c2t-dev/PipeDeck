//! The effects tab, which a channel and a mix share.
//!
//! Both run the same chain in the same order; where it sits differs and that
//! is the engine's business. A channel is treated before any mix hears it, a
//! mix between its cells and the sink a capture client reads — which is the
//! place for a processor that adds delay, since it leaves the monitoring
//! path alone.

use std::cell::{Cell as StdCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::stereotool::Status;
use pipedeck_engine::{vst3::Plugin, Command, Control, Effect, EffectKind, EffectTarget};

use crate::effects;
use crate::engine_link::EngineLink;
use crate::widgets;

/// Which object the chain belongs to. The engine names them the same way.
pub type Target = EffectTarget;

fn command(target: Target, effects: Vec<Effect>) -> Command {
    match target {
        Target::Channel(id) => Command::SetEffects { id, effects },
        Target::Mix(id) => Command::SetMixEffects { id, effects },
    }
}

/// What the tab says about where the chain runs.
fn hint(target: Target) -> &'static str {
    match target {
        Target::Channel(_) => {
            "Every mix hears this channel through these, in order. Changing one reloads the \
             chain, so the audio stops for a moment."
        }
        Target::Mix(_) => {
            "This mix runs these on its way out, in order, and nothing else hears them. \
             Changing one reloads the chain, so the audio stops for a moment."
        }
    }
}

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
    /// The plug-ins the engine found installed.
    plugins: RefCell<Vec<Plugin>>,
    /// Where Stereo Tool stands, which decides whether it is offered at all.
    stereotool: RefCell<Status>,
    /// A control being dragged sends one command when it settles rather than
    /// one per pixel: each change reloads the chain.
    pending: RefCell<Option<gtk::glib::SourceId>>,
    /// Set while engine state is pushed into the widgets.
    syncing: Rc<StdCell<bool>>,
}

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
            plugins: RefCell::new(Vec::new()),
            stereotool: RefCell::new(Status::Absent),
            pending: RefCell::new(None),
            syncing: Rc::new(StdCell::new(false)),
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

        let hint = gtk::Label::new(Some(hint(self.target)));
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
        self.add.set_tooltip_text(Some(match self.target {
            Target::Channel(_) => "Add an effect to this channel",
            Target::Mix(_) => "Add an effect to this mix",
        }));
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

    /// Redraw the chain, unless it is already what is on screen.
    fn show(self: &Rc<Self>, effects: &[Effect]) {
        if self.drawn.get() && self.shown.borrow().as_slice() == effects {
            return;
        }
        self.drawn.set(true);
        *self.shown.borrow_mut() = effects.to_vec();

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        if effects.is_empty() {
            let empty = gtk::Label::new(Some(match self.target {
                Target::Channel(_) => "No effect on this channel.",
                Target::Mix(_) => "No effect on this mix.",
            }));
            empty.add_css_class("dim-label");
            empty.set_margin_top(24);
            self.list.append(&empty);
            return;
        }
        for (position, effect) in effects.iter().enumerate() {
            let row = self.row(position, effect);
            self.list.append(&row);
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
            let open = gtk::Button::with_label("Open");
            open.add_css_class("flat");
            open.set_tooltip_text(Some("Open Stereo Tool's own window"));
            open.connect_clicked({
                let this = self.clone();
                move |_| {
                    this.engine.send(Command::ShowEffectWindow {
                        target: this.target,
                        index: position,
                    })
                }
            });
            top.append(&open);
        }

        let remove = gtk::Button::from_icon_name("list-remove-symbolic");
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some("Take this effect off"));
        remove.connect_clicked({
            let this = self.clone();
            move |_| {
                let mut chain = this.shown.borrow().clone();
                if position < chain.len() {
                    chain.remove(position);
                }
                this.send(chain, false);
            }
        });
        top.append(&remove);
        inner.append(&top);

        if effect.kind == EffectKind::StereoTool {
            self.stereotool_body(position, effect, &inner);
            return card.upcast();
        }

        let spec = effects::spec(effect);
        for (index, control) in effect.controls.iter().enumerate() {
            let known = spec.and_then(|spec| spec.controls.get(index));
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

            let name = gtk::Label::new(Some(known.map_or(control.name.as_str(), |c| c.label)));
            name.add_css_class("caption");
            name.set_xalign(0.0);
            name.set_width_chars(9);
            row.append(&name);

            let (min, max) = known.map_or((0.0, 1.0), |c| (c.min, c.max));
            let scale =
                gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, (max - min) / 100.0);
            scale.set_hexpand(true);
            scale.set_draw_value(true);
            scale.set_value_pos(gtk::PositionType::Right);
            scale.set_digits(if max <= 10.0 { 1 } else { 0 });
            scale.set_value(f64::from(control.value));
            if let Some(known) = known {
                scale.set_tooltip_text(Some(&format!("{}{}", control.value, known.unit)));
            }
            scale.connect_value_changed({
                let this = self.clone();
                let name = control.name.clone();
                move |scale| {
                    if this.syncing.get() {
                        return;
                    }
                    let mut chain = this.shown.borrow().clone();
                    let Some(effect) = chain.get_mut(position) else {
                        return;
                    };
                    if let Some(control) = effect
                        .controls
                        .iter_mut()
                        .find(|control| control.name == name)
                    {
                        control.value = scale.value() as f32;
                    } else {
                        effect.controls.push(Control {
                            name: name.clone(),
                            value: scale.value() as f32,
                        });
                    }
                    this.send(chain, true);
                }
            });
            row.append(&scale);
            inner.append(&row);
        }

        card.upcast()
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
            let this = self.clone();
            move |button| this.pick_preset(position, button.upcast_ref())
        });
        row.append(&choose);

        if preset.is_some() {
            let clear = gtk::Button::from_icon_name("edit-clear-symbolic");
            clear.add_css_class("flat");
            clear.set_tooltip_text(Some("Go back to the settings it starts with"));
            clear.connect_clicked({
                let this = self.clone();
                move |_| this.set_preset(position, None)
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
            let this = self.clone();
            move |answer| {
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
        self.send(chain, false);
    }

    /// Send a chain to the engine, waiting for a dragged control to settle.
    fn send(self: &Rc<Self>, chain: Vec<Effect>, debounce: bool) {
        if let Some(pending) = self.pending.borrow_mut().take() {
            pending.remove();
        }
        if !debounce {
            // Drawn here rather than when the engine echoes it back: the
            // echo is this very chain, and a panel that only redraws on a
            // difference would find none and leave the list as it was.
            self.shown.borrow_mut().clear();
            self.drawn.set(false);
            self.show(&chain);
            self.engine.send(command(self.target, chain));
            return;
        }
        // A control that moved already shows its own value, and redrawing
        // would take the slider out from under the pointer. What is on
        // screen is the truth while the engine catches up, so the next
        // change reads this one rather than the state before it.
        *self.shown.borrow_mut() = chain.clone();
        let this = self.clone();
        let source =
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                this.pending.borrow_mut().take();
                this.engine.send(command(this.target, chain));
            });
        *self.pending.borrow_mut() = Some(source);
    }

    /// What this object could run.
    fn popover(self: &Rc<Self>) -> gtk::Popover {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let popover = gtk::Popover::new();

        for spec in effects::EFFECTS {
            let button = entry(spec.name, spec.description);
            button.connect_clicked({
                let this = self.clone();
                let popover = popover.clone();
                move |_| {
                    let mut chain = this.shown.borrow().clone();
                    chain.push(effects::build(spec));
                    this.send(chain, false);
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
                let this = self.clone();
                let popover = popover.clone();
                move |_| {
                    let mut chain = this.shown.borrow().clone();
                    chain.push(Effect {
                        name: "Stereo Tool".to_owned(),
                        kind: EffectKind::StereoTool,
                        plugin: None,
                        label: "stereotool".to_owned(),
                        controls: Vec::new(),
                    });
                    this.send(chain, false);
                    popover.popdown();
                }
            });
            list.append(&button);
        }

        for plugin in plugins.iter() {
            let button = entry(&plugin.name, &plugin.vendor);
            button.connect_clicked({
                let this = self.clone();
                let popover = popover.clone();
                let plugin = plugin.clone();
                move |_| {
                    let mut chain = this.shown.borrow().clone();
                    chain.push(Effect {
                        name: plugin.name.clone(),
                        kind: EffectKind::Vst3,
                        plugin: None,
                        label: plugin.class_id.clone(),
                        controls: Vec::new(),
                    });
                    this.send(chain, false);
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
