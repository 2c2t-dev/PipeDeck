//! Main window: the mixer matrix, mixes across the top, sources down the
//! left, one fader per cell.

use std::cell::{Cell as StdCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::gtk;
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::stereotool::Status;
use pipedeck_engine::{
    vst3::Plugin, App, Command, Device, Event, MixConfig, MixId, SourceConfig, SourceId,
    StateSnapshot, VoiceConfig, MAX_MIXES,
};

use crate::cell::{link_button, Cell};
use crate::channel_dialog::ChannelDialog;
use crate::dialogs;
use crate::engine_link::EngineLink;
use crate::listen::Listen;
use crate::mix_dialog::MixDialog;
use crate::preferences;
use crate::presets;
use crate::vesktop;
use crate::widgets;

const MIX_COLUMN_WIDTH: i32 = 240;
const SOURCE_COLUMN_WIDTH: i32 = 180;
const ROW_HEIGHT: i32 = 56;
/// A sub-track is a person of a call: a lighter row under its channel.
const VOICE_ROW_HEIGHT: i32 = 44;
const MIX_HEADER_HEIGHT: i32 = 72;
const BADGE_ICON_SIZE: i32 = 16;
const ADD_MIX_WIDTH: i32 = 56;

pub struct Window {
    pub window: adw::ApplicationWindow,
    engine: EngineLink,
    grid: gtk::Grid,
    hint: gtk::Label,
    toasts: adw::ToastOverlay,
    /// Set once the engine is gone, so a rebuild keeps the add buttons off.
    stopped: StdCell<bool>,
    state: RefCell<StateSnapshot>,
    /// The sound card switch in the header bar.
    listen: Rc<Listen>,
    outputs: RefCell<Vec<Device>>,
    inputs: RefCell<Vec<Device>>,
    /// Applications currently playing, for the channel windows.
    apps: RefCell<Vec<App>>,
    /// The plug-ins installed on the machine, as the engine found them.
    plugins: RefCell<Vec<Plugin>>,
    /// Where Stereo Tool stands, so a window knows whether to offer it.
    stereotool: RefCell<Status>,
    /// The settings window while it is up: what it says about plug-ins is
    /// answered by the engine after the window was drawn.
    preferences: RefCell<Option<Rc<preferences::Preferences>>>,
    /// Last peak of every channel, so a cell can draw what it passes on.
    channel_levels: RefCell<HashMap<SourceId, f32>>,
    cells: RefCell<HashMap<(SourceId, MixId), Cell>>,
    /// The object windows, while they are open, so engine changes reach them.
    mix_dialog: RefCell<Option<Rc<MixDialog>>>,
    channel_dialog: RefCell<Option<Rc<ChannelDialog>>>,
    /// The channels unfolded into their sub-tracks.
    expanded: RefCell<HashSet<SourceId>>,
    /// The faders of the sub-tracks drawn, to move their meters.
    voice_faders: RefCell<HashMap<(SourceId, String), widgets::MeterFader>>,
    /// People's pictures, by file, read once rather than at every redraw.
    avatars: RefCell<HashMap<String, gtk::gdk::Texture>>,
}

impl Window {
    pub fn new(app: &adw::Application, engine: EngineLink) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Pipedeck")
            .default_width(1080)
            .default_height(520)
            .build();

        let header = adw::HeaderBar::new();
        let settings = gtk::Button::from_icon_name("preferences-system-symbolic");
        settings.set_tooltip_text(Some("Settings"));
        header.pack_end(&settings);
        // Where you hear the mix you listen to, beside the settings, as the
        // one thing up here you reach for mid-stream.
        let listen = Listen::new(&engine);
        header.pack_end(&listen.widget());

        let grid = gtk::Grid::new();
        // Cards keep their size in a wide window instead of stretching: a
        // fader is easier to aim at when it does not change length with the
        // window, and a row of them stays readable.
        grid.set_halign(gtk::Align::Start);
        grid.set_valign(gtk::Align::Start);
        grid.set_row_spacing(6);
        grid.set_column_spacing(6);
        grid.set_margin_top(12);
        grid.set_margin_bottom(12);
        grid.set_margin_start(12);
        grid.set_margin_end(12);

        let hint = gtk::Label::new(None);
        hint.add_css_class("dim-label");
        hint.set_margin_bottom(24);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&grid);
        content.append(&hint);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroller.set_child(Some(&content));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&scroller));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        window.set_content(Some(&view));

        let this = Rc::new(Self {
            window,
            engine,
            grid,
            hint,
            toasts,
            stopped: StdCell::new(false),
            listen,
            state: RefCell::new(StateSnapshot {
                latency: String::new(),
                stereotool_license: None,
                listen_device: None,
                mixes: Vec::new(),
                sources: Vec::new(),
                links: Vec::new(),
            }),
            outputs: RefCell::new(Vec::new()),
            inputs: RefCell::new(Vec::new()),
            apps: RefCell::new(Vec::new()),
            plugins: RefCell::new(Vec::new()),
            stereotool: RefCell::new(Status::Absent),
            preferences: RefCell::new(None),
            channel_levels: RefCell::new(HashMap::new()),
            cells: RefCell::new(HashMap::new()),
            mix_dialog: RefCell::new(None),
            channel_dialog: RefCell::new(None),
            expanded: RefCell::new(HashSet::new()),
            voice_faders: RefCell::new(HashMap::new()),
            avatars: RefCell::new(HashMap::new()),
        });

        settings.connect_clicked({
            let this = this.clone();
            move |_| this.open_preferences(false)
        });

        this.rebuild();
        this.check_vesktop_plugin();
        this
    }

    /// Open the settings, on the plug-ins' page when asked.
    fn open_preferences(self: &Rc<Self>, on_plugins: bool) {
        let state = self.state.borrow();
        let latency = state.latency.clone();
        let license = state.stereotool_license.clone();
        drop(state);
        let plugins = preferences::PluginState {
            installed: self.plugins.borrow().len(),
            stereotool: &self.stereotool.borrow(),
            license: license.as_deref(),
        };
        let open = preferences::present(&self.window, &self.engine, &latency, &plugins, on_plugins);
        *self.preferences.borrow_mut() = Some(open);
    }

    /// Say so when the plugin built into Vesktop is behind: Vesktop was
    /// updated under it, or this Pipedeck carries a newer one. Checked once,
    /// off the main thread, as the window opens.
    fn check_vesktop_plugin(self: &Rc<Self>) {
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(vesktop::stale());
        });
        gtk::glib::spawn_future_local({
            let this = self.clone();
            async move {
                let Ok(Some(stale)) = rx.recv().await else {
                    return;
                };
                let toast = adw::Toast::new(&format!(
                    "{}. Update Pipedeck's plugin for Vesktop from the settings.",
                    stale.reason()
                ));
                toast.set_timeout(0);
                toast.set_button_label(Some("Settings"));
                toast.connect_button_clicked({
                    let this = this.clone();
                    move |_| this.open_preferences(true)
                });
                this.toasts.add_toast(toast);
            }
        });
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Lay OpenDeck's Pipedeck profiles out again if what they are laid
    /// out from changed, and the user wants that. See [`streamdeck`].
    fn lay_out_stream_decks(&self) {
        crate::streamdeck::mixer_changed(
            &self.state.borrow(),
            &self.outputs.borrow(),
            crate::settings::Settings::load().stream_deck_profiles,
        );
    }

    pub fn handle_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::State(state) => {
                *self.state.borrow_mut() = state;
                self.lay_out_stream_decks();
                self.listen
                    .refresh(&self.state.borrow(), &self.outputs.borrow());
                self.rebuild();
                self.refresh_settings();
                self.refresh_dialogs();
            }
            Event::Plugins { available } => {
                *self.plugins.borrow_mut() = available;
                self.refresh_settings();
                self.refresh_dialogs();
            }
            Event::StereoTool(status) => {
                *self.stereotool.borrow_mut() = status;
                self.refresh_settings();
                self.refresh_dialogs();
            }
            // Only the Stream Deck shows it, through the control socket.
            Event::Focused { .. } => {}
            Event::Apps { running } => {
                *self.apps.borrow_mut() = running;
                self.refresh_dialogs();
            }
            Event::Devices { outputs, inputs } => {
                *self.outputs.borrow_mut() = outputs;
                *self.inputs.borrow_mut() = inputs;
                self.lay_out_stream_decks();
                self.listen
                    .refresh(&self.state.borrow(), &self.outputs.borrow());
                self.refresh_dialogs();
            }
            Event::Levels {
                sources,
                mixes,
                voices,
                effects,
            } => {
                self.draw_levels(&sources, &mixes);
                self.draw_voice_levels(&voices);
                let open = self.channel_dialog.borrow().clone();
                if let Some(dialog) = open.filter(|dialog| dialog.is_open()) {
                    let levels: Vec<(usize, f32, f32)> = effects
                        .iter()
                        .filter(|effect| effect.source == dialog.id())
                        .map(|effect| (effect.index, effect.level, effect.reduction))
                        .collect();
                    dialog.set_effect_levels(&levels);
                }
            }
            Event::LinkChanged { source, mix, state } => {
                if let Some(cell) = self.cells.borrow().get(&(source, mix)) {
                    cell.set_state(state);
                }
                if let Some(link) = self
                    .state
                    .borrow_mut()
                    .links
                    .iter_mut()
                    .find(|l| l.source == source && l.mix == mix)
                {
                    link.set_state(state);
                }
            }
            Event::MixChanged { id, state } => {
                // A level can move outside the mixer: a media key, a volume
                // applet, anything holding the same sink.
                if let Some(mix) = self
                    .state
                    .borrow_mut()
                    .mixes
                    .iter_mut()
                    .find(|m| m.id == id)
                {
                    mix.set_state(state);
                }
                if let Some(dialog) = self.mix_dialog.borrow().as_ref() {
                    if dialog.id() == id {
                        dialog.set_state(state);
                    }
                }
            }
            Event::SourceChanged { id, state } => {
                if let Some(source) = self
                    .state
                    .borrow_mut()
                    .sources
                    .iter_mut()
                    .find(|s| s.id == id)
                {
                    source.set_state(state);
                }
                if let Some(dialog) = self.channel_dialog.borrow().as_ref() {
                    if dialog.id() == id {
                        dialog.set_state(state);
                    }
                }
            }
            Event::OutputChanged { id, index, state } => {
                // The window that moved it already shows the new value, but
                // the windows are drawn again from this copy: left as it was,
                // the next time would put the fader back where it had been.
                if let Some(output) = self
                    .state
                    .borrow_mut()
                    .mixes
                    .iter_mut()
                    .find(|m| m.id == id)
                    .and_then(|mix| mix.outputs.get_mut(index))
                {
                    output.set_state(state);
                }
            }
            Event::SourceEffects { id, effects } => {
                // Nothing on the grid shows a setting, so only the
                // channel's window, if it is up, is told.
                self.channel_dialog.borrow_mut().take_if(|d| !d.is_open());
                let open = self.channel_dialog.borrow().clone();
                if let Some(dialog) = open.filter(|dialog| dialog.id() == id) {
                    dialog.set_effects(&effects);
                }
                if let Some(source) = self
                    .state
                    .borrow_mut()
                    .sources
                    .iter_mut()
                    .find(|source| source.id == id)
                {
                    source.effects = effects;
                }
            }
            Event::Error(message) => self.toast(&message),
            Event::Notice(message) => {
                log::info!("{message}");
                self.toast_quietly(&message);
            }
            Event::Stopped => {
                self.stopped.set(true);
                self.rebuild();
                self.toast("Audio engine stopped");
            }
        }
    }

    fn toast(&self, message: &str) {
        log::warn!("{message}");
        self.toast_quietly(message);
    }

    /// The same, for what is worth saying rather than worth warning about.
    fn toast_quietly(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    /// Rebuild the whole matrix. Structural changes are rare and the grid is
    /// small, so this is simpler and safer than patching it in place.
    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        self.cells.borrow_mut().clear();
        self.voice_faders.borrow_mut().clear();

        let state = self.state.borrow();

        self.grid.attach(&corner(), 0, 0, 1, 1);
        for (column, mix) in state.mixes.iter().enumerate() {
            let header = self.mix_header(mix, column);
            self.grid.attach(&header, column as i32 + 1, 0, 1, 1);
        }
        // The two add buttons continue the grid: a new column on the right of
        // the last mix, a new row under the last source.
        if state.mixes.len() < MAX_MIXES {
            let add = self.add_mix_button();
            self.grid
                .attach(&add, state.mixes.len() as i32 + 1, 0, 1, 1);
        }
        // Rows are counted as they go: a channel unfolded into its people
        // takes one more for each.
        let mut row = 1;
        for (index, source) in state.sources.iter().enumerate() {
            let header = self.source_header(source, index);
            self.grid.attach(&header, 0, row, 1, 1);

            for (column, mix) in state.mixes.iter().enumerate() {
                let linked = state
                    .links
                    .iter()
                    .find(|l| l.source == source.id && l.mix == mix.id);
                let widget: gtk::Widget = match linked {
                    Some(link) => {
                        let cell = Cell::new(source.id, mix.id, link.state(), &self.engine);
                        if let Some(peak) = self.channel_levels.borrow().get(&source.id) {
                            cell.set_level(*peak);
                        }
                        let root = cell.root.clone().upcast();
                        self.cells.borrow_mut().insert((source.id, mix.id), cell);
                        root
                    }
                    None => link_button(source.id, mix.id, &self.engine).upcast(),
                };
                let holder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                holder.add_css_class("card");
                holder.set_height_request(ROW_HEIGHT);
                holder.set_size_request(MIX_COLUMN_WIDTH, ROW_HEIGHT);
                holder.set_hexpand(false);
                holder.append(&widget);
                widget.set_hexpand(true);
                self.grid.attach(&holder, column as i32 + 1, row, 1, 1);
            }
            row += 1;

            // A sub-track has one level, on its way into the channel: it
            // spans the mixes rather than having a cell in each.
            if self.expanded.borrow().contains(&source.id) {
                for voice in source.voices.iter().filter(|voice| voice.present) {
                    self.grid.attach(&self.voice_header(voice), 0, row, 1, 1);
                    let span = state.mixes.len().max(1) as i32;
                    self.grid
                        .attach(&self.voice_level(source.id, voice), 1, row, span, 1);
                    row += 1;
                }
            }
        }
        let add = self.add_source_button();
        self.grid.attach(&add, 0, row, 1, 1);

        self.hint.set_visible(state.sources.is_empty());
        self.hint.set_label(
            "Create a channel to get a virtual output, then press + to send it to a mix.",
        );
    }

    /// Move every meter: the cells of a channel, and the windows that are
    /// open on the objects concerned.
    fn draw_levels(self: &Rc<Self>, sources: &[(SourceId, f32)], mixes: &[(MixId, f32)]) {
        let mut levels = self.channel_levels.borrow_mut();
        for (id, peak) in sources {
            levels.insert(*id, *peak);
        }
        drop(levels);

        let cells = self.cells.borrow();
        for ((source, _), cell) in cells.iter() {
            if let Some((_, peak)) = sources.iter().find(|(id, _)| id == source) {
                cell.set_level(*peak);
            }
        }
        drop(cells);

        if let Some(dialog) = self.channel_dialog.borrow().as_ref() {
            if let Some((_, peak)) = sources.iter().find(|(id, _)| *id == dialog.id()) {
                dialog.set_level(*peak);
            }
        }
        if let Some(dialog) = self.mix_dialog.borrow().as_ref() {
            if let Some((_, peak)) = mixes.iter().find(|(id, _)| *id == dialog.id()) {
                dialog.set_level(*peak);
            }
        }
    }

    /// Open the window of one mix, replacing whichever was open.
    fn open_mix_dialog(self: &Rc<Self>, id: MixId) {
        let state = self.state.borrow();
        let Some(mix) = state.mixes.iter().find(|m| m.id == id) else {
            return;
        };
        if let Some(open) = self.mix_dialog.borrow_mut().take() {
            open.close();
        }
        let dialog = MixDialog::present(&self.window, &self.engine, mix, &self.outputs.borrow());
        *self.mix_dialog.borrow_mut() = Some(dialog);
    }

    /// Open the window of one channel, replacing whichever was open.
    fn open_channel_dialog(self: &Rc<Self>, id: SourceId) {
        let state = self.state.borrow();
        let Some(source) = state.sources.iter().find(|s| s.id == id) else {
            return;
        };
        if let Some(open) = self.channel_dialog.borrow_mut().take() {
            open.close();
        }
        let dialog = ChannelDialog::present(
            &self.window,
            &self.engine,
            source,
            &self.inputs.borrow(),
            &self.apps.borrow(),
            &self.plugins.borrow(),
            &self.stereotool.borrow(),
        );
        drop(state);
        *self.channel_dialog.borrow_mut() = Some(dialog);
    }

    /// Keep the settings window in step with the engine, if it is up.
    fn refresh_settings(self: &Rc<Self>) {
        self.preferences
            .borrow_mut()
            .take_if(|open| !open.is_open());
        let open = self.preferences.borrow().clone();
        let Some(open) = open else {
            return;
        };
        open.set_plugins(self.plugins.borrow().len());
        let license = self.state.borrow().stereotool_license.clone();
        open.set_stereotool(&self.stereotool.borrow(), license.as_deref());
    }

    /// Keep the open windows in step with the engine, and close one whose
    /// object is gone.
    fn refresh_dialogs(self: &Rc<Self>) {
        // A window the user closed is forgotten rather than refreshed.
        self.mix_dialog.borrow_mut().take_if(|d| !d.is_open());
        self.channel_dialog.borrow_mut().take_if(|d| !d.is_open());

        let open_mix = self.mix_dialog.borrow().clone();
        if let Some(dialog) = open_mix {
            let state = self.state.borrow();
            match state.mixes.iter().find(|m| m.id == dialog.id()) {
                Some(mix) => dialog.refresh(mix, &self.outputs.borrow()),
                None => {
                    drop(state);
                    dialog.close_gone();
                    *self.mix_dialog.borrow_mut() = None;
                }
            }
        }
        let open_channel = self.channel_dialog.borrow().clone();
        if let Some(dialog) = open_channel {
            let state = self.state.borrow();
            match state.sources.iter().find(|s| s.id == dialog.id()) {
                Some(source) => dialog.refresh(
                    source,
                    &self.inputs.borrow(),
                    &self.apps.borrow(),
                    &self.plugins.borrow(),
                    &self.stereotool.borrow(),
                ),
                None => {
                    drop(state);
                    dialog.close_gone();
                    *self.channel_dialog.borrow_mut() = None;
                }
            }
        }
    }

    fn add_mix_button(self: &Rc<Self>) -> gtk::Widget {
        // Icon only: this one sits in the header row next to named mixes, so
        // it stays out of the way until you look for it.
        let button = gtk::Button::from_icon_name("list-add-symbolic");
        button.set_tooltip_text(Some("Add a mix"));
        button.add_css_class("flat");
        button.add_css_class("card");
        button.set_width_request(ADD_MIX_WIDTH);
        button.set_margin_bottom(6);
        button.set_sensitive(!self.stopped.get());
        button.connect_clicked({
            let this = self.clone();
            move |_| this.engine.send(Command::AddMix)
        });
        button.upcast()
    }

    fn add_source_button(self: &Rc<Self>) -> gtk::Widget {
        let button = gtk::Button::new();
        button.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name("list-add-symbolic")
                .label("Create channel")
                .build(),
        ));
        button.set_tooltip_text(Some(
            "Create a channel: a virtual output, or a capture device",
        ));
        button.add_css_class("flat");
        button.add_css_class("card");
        button.set_width_request(SOURCE_COLUMN_WIDTH);
        button.set_height_request(ROW_HEIGHT);
        button.set_sensitive(!self.stopped.get());
        button.connect_clicked({
            let this = self.clone();
            move |_| dialogs::add_source(&this.window, &this.engine, &this.inputs.borrow())
        });
        button.upcast()
    }

    fn mix_header(self: &Rc<Self>, mix: &MixConfig, index: usize) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);

        content.append(&badge_slot(&widgets::badge(
            mix.icon.as_deref(),
            false,
            widgets::Tone::White,
            BADGE_ICON_SIZE,
        )));

        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        labels.set_valign(gtk::Align::Center);
        // Room for the ear, which sits over the card rather than in it: a
        // button inside a button would take its clicks.
        labels.set_margin_end(34);
        let title = gtk::Label::new(Some(&mix.name));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&title);
        // The outputs it plays to now, not those switched off in its list.
        let playing = mix.outputs.iter().filter(|output| output.enabled).count();
        let subtitle = gtk::Label::new(Some(&output_label(playing)));
        subtitle.add_css_class("caption");
        subtitle.add_css_class("dim-label");
        subtitle.set_xalign(0.0);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        labels.append(&subtitle);
        content.append(&labels);

        let card = clickable_card(&content, MIX_COLUMN_WIDTH, MIX_HEADER_HEIGHT);
        card.set_tooltip_text(Some("Rename this mix, choose its outputs, or remove it"));
        card.connect_clicked({
            let this = self.clone();
            let id = mix.id;
            move |_| this.open_mix_dialog(id)
        });

        // The ear: whether this mix is heard on the device you listen on,
        // which the switch in the header bar picks. Several can be lit.
        let listen = self.state.borrow().listen_device.clone();
        let listening = mix
            .outputs
            .iter()
            .any(|output| output.enabled && Some(&output.device) == listen.as_ref());
        let ear = gtk::ToggleButton::new();
        ear.set_icon_name("pd-listen-symbolic");
        ear.add_css_class("flat");
        ear.add_css_class("circular");
        ear.set_active(listening);
        ear.set_halign(gtk::Align::End);
        ear.set_valign(gtk::Align::Center);
        ear.set_margin_end(10);
        ear.set_tooltip_text(Some(if listening {
            "Heard in your headphones; click to stop"
        } else {
            "Not heard in your headphones; click to hear it"
        }));
        ear.connect_toggled({
            let engine = self.engine.clone();
            let id = mix.id;
            move |button| {
                engine.send(Command::SetListening {
                    id,
                    listening: button.is_active(),
                });
            }
        });

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&card));
        overlay.add_overlay(&ear);
        overlay.set_margin_bottom(6);
        // Dropped on the overlay rather than the card, or a drop landing on
        // the ear would find nothing to take it.
        reorderable(&card, &overlay, "mix", mix.id.0, index, {
            let engine = self.engine.clone();
            move |dragged, to| {
                engine.send(Command::MoveMix {
                    id: MixId(dragged),
                    to,
                })
            }
        });
        overlay.upcast()
    }

    fn source_header(self: &Rc<Self>, source: &SourceConfig, index: usize) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);

        content.append(&badge_slot(&widgets::badge(
            source.icon.as_deref(),
            source.is_input(),
            widgets::Tone::Colour,
            BADGE_ICON_SIZE,
        )));

        let title = gtk::Label::new(Some(&source.name));
        title.add_css_class("heading");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&title);

        let card = clickable_card(&content, SOURCE_COLUMN_WIDTH, ROW_HEIGHT);
        card.set_tooltip_text(Some("Rename this channel, set its levels, or remove it"));
        card.connect_clicked({
            let this = self.clone();
            let id = source.id;
            move |_| this.open_channel_dialog(id)
        });
        reorderable(&card, &card, "source", source.id.0, index, {
            let engine = self.engine.clone();
            move |dragged, to| {
                engine.send(Command::MoveSource {
                    id: SourceId(dragged),
                    to,
                })
            }
        });

        // A channel carrying a call unfolds into the people in it.
        let people = source.voices.iter().filter(|voice| voice.present).count();
        if people == 0 {
            return card.upcast();
        }
        let open = self.expanded.borrow().contains(&source.id);
        let unfold = gtk::Button::new();
        unfold.set_child(Some(
            &adw::ButtonContent::builder()
                .icon_name(if open {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                })
                .label(people.to_string())
                .build(),
        ));
        unfold.add_css_class("flat");
        unfold.add_css_class("caption");
        unfold.set_halign(gtk::Align::End);
        unfold.set_valign(gtk::Align::Center);
        unfold.set_margin_end(6);
        unfold.set_tooltip_text(Some(if open {
            "Fold the people of the call away"
        } else {
            "Show each person of the call on a sub-track of their own"
        }));
        unfold.connect_clicked({
            let this = self.clone();
            let id = source.id;
            move |_| {
                {
                    let mut expanded = this.expanded.borrow_mut();
                    if !expanded.remove(&id) {
                        expanded.insert(id);
                    }
                }
                this.rebuild();
            }
        });
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&card));
        overlay.add_overlay(&unfold);
        overlay.upcast()
    }

    /// Move the meter of every person of a call drawn.
    fn draw_voice_levels(&self, voices: &[(SourceId, String, f32)]) {
        let faders = self.voice_faders.borrow();
        for (id, user, peak) in voices {
            if let Some(fader) = faders.get(&(*id, user.clone())) {
                fader.set_level(*peak);
            }
        }
    }

    /// The name of a person of a call, set in from its channel's.
    fn voice_header(&self, voice: &VoiceConfig) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.set_margin_start(26);
        content.set_margin_end(10);
        // Their picture, or their initials until the client has fetched it.
        let avatar = adw::Avatar::new(26, Some(&voice.name), true);
        if let Some(texture) = voice.avatar.as_deref().and_then(|path| self.avatar(path)) {
            avatar.set_custom_image(Some(&texture));
        }
        content.append(&avatar);
        let name = gtk::Label::new(Some(&voice.name));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&name);
        content.set_valign(gtk::Align::Center);

        let card = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        card.add_css_class("card");
        card.add_css_class("pd-voice");
        card.set_size_request(SOURCE_COLUMN_WIDTH, VOICE_ROW_HEIGHT);
        card.set_tooltip_text(Some(&voice.name));
        card.append(&content);
        card.upcast()
    }

    /// A person's picture, read from its file the first time it is shown.
    fn avatar(&self, path: &str) -> Option<gtk::gdk::Texture> {
        if let Some(texture) = self.avatars.borrow().get(path) {
            return Some(texture.clone());
        }
        match gtk::gdk::Texture::from_filename(path) {
            Ok(texture) => {
                self.avatars
                    .borrow_mut()
                    .insert(path.to_owned(), texture.clone());
                Some(texture)
            }
            Err(e) => {
                log::debug!("cannot show {path}: {e}");
                None
            }
        }
    }

    /// Change one person as the window last heard of them.
    fn with_voice(&self, id: SourceId, user: &str, f: impl FnOnce(&mut VoiceConfig)) {
        let mut state = self.state.borrow_mut();
        let voice = state
            .sources
            .iter_mut()
            .find(|source| source.id == id)
            .and_then(|source| source.voices.iter_mut().find(|voice| voice.id == user));
        if let Some(voice) = voice {
            f(voice);
        }
    }

    /// One person's level on their way into the channel, and their mute.
    fn voice_level(self: &Rc<Self>, id: SourceId, voice: &VoiceConfig) -> gtk::Widget {
        let fader = widgets::MeterFader::new(voice.gain);
        let mute = widgets::mute_button(voice.muted, "Mute this person");
        // The engine does not send the state back for a level, so the one
        // drawn from is kept in step here, for the next time the grid is.
        fader.scale.connect_value_changed({
            let this = self.clone();
            let user = voice.id.clone();
            move |scale| {
                let gain = (scale.value() / widgets::FADER_MAX) as f32;
                this.with_voice(id, &user, |voice| voice.gain = gain);
                this.engine.send(Command::SetVoiceGain {
                    id,
                    user: user.clone(),
                    gain,
                })
            }
        });
        mute.connect_toggled({
            let this = self.clone();
            let user = voice.id.clone();
            move |button| {
                let muted = button.is_active();
                this.with_voice(id, &user, |voice| voice.muted = muted);
                this.engine.send(Command::SetVoiceMute {
                    id,
                    user: user.clone(),
                    muted,
                })
            }
        });
        let row = widgets::level_row(&mute, &fader.root);
        fader.root.set_hexpand(true);
        self.voice_faders
            .borrow_mut()
            .insert((id, voice.id.clone()), fader);
        row.set_margin_start(10);
        row.set_margin_end(10);
        let holder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        holder.add_css_class("card");
        holder.add_css_class("pd-voice");
        holder.set_height_request(VOICE_ROW_HEIGHT);
        holder.append(&row);
        row.set_hexpand(true);
        row.set_valign(gtk::Align::Center);
        holder.upcast()
    }
}

/// A card whose whole surface acts as a button, showing a pencil on hover to
/// say so without spending room on a permanent button.
///
/// The reveal is left to the stylesheet in [`load_css`], so it also covers
/// keyboard focus and needs no event plumbing.
/// What a dragged card carries: which kind of thing it is, and which one.
/// A mix is only dropped among mixes and a channel among channels, so a
/// column never ends up a row.
fn drag_payload(kind: &str, id: u32) -> String {
    format!("{kind}:{id}")
}

fn read_payload(text: &str, kind: &str) -> Option<u32> {
    let (dragged, id) = text.split_once(':')?;
    (dragged == kind).then(|| id.parse().ok())?
}

/// Let a card be dragged, and let one of its own kind be dropped on `onto`,
/// which then takes this card's place: a column along the columns, a row
/// along the rows. `moved` says where to.
fn reorderable(
    card: &gtk::Button,
    onto: &impl IsA<gtk::Widget>,
    kind: &'static str,
    id: u32,
    index: usize,
    moved: impl Fn(u32, usize) + 'static,
) {
    // Where on the card it was taken, so the picture of it stays under the
    // pointer at that same spot rather than hanging off its corner.
    let grabbed = std::rc::Rc::new(StdCell::new((0, 0)));

    let drag = gtk::DragSource::new();
    drag.set_actions(gtk::gdk::DragAction::MOVE);
    drag.connect_prepare({
        let grabbed = grabbed.clone();
        move |_, x, y| {
            grabbed.set((x as i32, y as i32));
            Some(gtk::gdk::ContentProvider::for_value(
                &drag_payload(kind, id).to_value(),
            ))
        }
    });
    // The card is known weakly: the drag is the card's own, and held
    // strongly the two would keep each other once the grid is drawn again.
    drag.connect_drag_begin({
        let card = card.downgrade();
        let grabbed = grabbed.clone();
        move |source, _| {
            let Some(card) = card.upgrade() else {
                return;
            };
            // A still picture of the card, taken now. A live one redraws the
            // card all through the drag, pressed and hovered as it is, and
            // outlives it: dropping rebuilds the grid, and the icon went on
            // drawing a card that no longer existed.
            let picture = gtk::WidgetPaintable::new(Some(&card)).current_image();
            let (x, y) = grabbed.get();
            source.set_icon(Some(&picture), x, y);
            // The card left in place fades, so what moves is plain.
            card.add_css_class("pd-dragging");
        }
    });
    drag.connect_drag_end({
        let card = card.downgrade();
        move |_, _, _| {
            if let Some(card) = card.upgrade() {
                card.remove_css_class("pd-dragging");
            }
        }
    });
    card.add_controller(drag);

    let drop = gtk::DropTarget::new(gtk::glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    drop.connect_drop(move |_, value, _, _| {
        let Ok(text) = value.get::<String>() else {
            return false;
        };
        let Some(dragged) = read_payload(&text, kind) else {
            return false;
        };
        if dragged != id {
            moved(dragged, index);
        }
        true
    });
    onto.add_css_class("pd-drop");
    onto.add_controller(drop);
}

/// A card's icon, with the grip and the pencil laid over it.
///
/// On hover the icon fades and the two show in its place: the card can be
/// dragged, and clicked to edit. They take none of the card's width of their
/// own, so nothing sits empty beside the icon when they are not shown and
/// nothing moves when they are.
fn badge_slot(badge: &gtk::Image) -> gtk::Overlay {
    badge.add_css_class("pd-badge-face");

    let grip = gtk::Image::from_icon_name("pd-drag-symbolic");
    grip.set_tooltip_text(Some("Drag to reorder"));
    let pencil = gtk::Image::from_icon_name("document-edit-symbolic");

    let tools = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    tools.add_css_class("pd-hover-tools");
    tools.set_halign(gtk::Align::Center);
    tools.set_valign(gtk::Align::Center);
    tools.append(&grip);
    tools.append(&pencil);

    let slot = gtk::Overlay::new();
    slot.set_child(Some(badge));
    slot.add_overlay(&tools);
    slot
}

fn clickable_card(content: &gtk::Box, width: i32, height: i32) -> gtk::Button {
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(10);
    content.set_margin_end(10);

    let card = gtk::Button::new();
    card.add_css_class("card");
    card.add_css_class("flat");
    card.add_css_class("pd-card");
    card.set_child(Some(content));
    card.set_width_request(width);
    card.set_height_request(height);
    card
}

/// Install the stylesheet. Call once, after GTK is initialised.
pub fn load_css() {
    const CSS: &str = "
        .pd-card .pd-hover-tools { opacity: 0; transition: opacity 120ms ease-out; }
        .pd-card .pd-badge-face { transition: opacity 120ms ease-out; }
        .pd-card:hover .pd-badge-face,
        .pd-card:focus-visible .pd-badge-face { opacity: 0; }
        .pd-card.pd-dragging { opacity: 0.35; }
        .card.pd-voice { background-color: alpha(@card_bg_color, 0.55); }
        .pd-drop:drop(active) {
            box-shadow: inset 0 0 0 2px @accent_color;
            border-radius: 12px;
        }
        .pd-card:hover .pd-hover-tools,
        .pd-card:focus-visible .pd-hover-tools { opacity: 1; }
        .pd-badge-large {
            background-color: @window_fg_color;
            color: @window_bg_color;
            border-radius: 28px;
            min-width: 148px;
            min-height: 148px;
        }
        .pd-badge {
            background-color: @window_fg_color;
            color: @window_bg_color;
            border-radius: 10px;
            min-width: 32px;
            min-height: 32px;
        }
    ";
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!(
        "{CSS}{}{}",
        crate::meter_fader::CSS,
        presets::css()
    ));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn corner() -> gtk::Widget {
    let corner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    corner.set_width_request(SOURCE_COLUMN_WIDTH);
    corner.upcast()
}

fn output_label(count: usize) -> String {
    match count {
        0 => "No output".to_owned(),
        1 => "1 output".to_owned(),
        n => format!("{n} outputs"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_column_is_never_dropped_among_rows() {
        let mix = super::drag_payload("mix", 3);
        assert_eq!(super::read_payload(&mix, "mix"), Some(3));
        assert_eq!(super::read_payload(&mix, "source"), None);
        assert_eq!(super::read_payload("rubbish", "mix"), None);
        assert_eq!(super::read_payload("mix:x", "mix"), None);
    }
}
