//! Putting Pipedeck's plugins into the Stream Deck applications, OpenDeck
//! and StreamController, and keeping OpenDeck's ready-made profiles laid
//! out from the mixer.
//!
//! The plugins are part of this program: StreamController's sources and
//! OpenDeck's manifest, layout and settings page are carried in it, and
//! OpenDeck's program, `pipedeck-opendeck`, is built with it and found
//! beside it. See `integrations/`.
//!
//! OpenDeck reads its plugins and a deck's profiles once, and writes the
//! profiles it holds back over whatever is on disk: putting a plugin in, or
//! laying out a profile, means closing it first and starting it again
//! after, which is done here when it was running.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use adw::gtk::gio;
use libadwaita as adw;
use pipedeck_engine::{Device, StateSnapshot};

/// OpenDeck's plugin, as the repository has it.
const OPENDECK_MANIFEST: &str = include_str!("../../../integrations/opendeck/plugin/manifest.json");
const OPENDECK_LAYOUT: &str =
    include_str!("../../../integrations/opendeck/plugin/layouts/strip.json");
const OPENDECK_PAGE: &str =
    include_str!("../../../integrations/opendeck/plugin/propertyInspector/index.html");
const OPENDECK_PLUGIN: &str = "dev.2c2t.pipedeck.sdPlugin";
const OPENDECK_PROGRAM: &str = "pipedeck-opendeck";

/// StreamController's plugin, as the repository has it.
const STREAMCONTROLLER_FILES: &[(&str, &str)] = &[
    (
        "main.py",
        include_str!("../../../integrations/streamcontroller/dev_2c2t_Pipedeck/main.py"),
    ),
    (
        "actions.py",
        include_str!("../../../integrations/streamcontroller/dev_2c2t_Pipedeck/actions.py"),
    ),
    (
        "client.py",
        include_str!("../../../integrations/streamcontroller/dev_2c2t_Pipedeck/client.py"),
    ),
    (
        "draw.py",
        include_str!("../../../integrations/streamcontroller/dev_2c2t_Pipedeck/draw.py"),
    ),
    (
        "manifest.json",
        include_str!("../../../integrations/streamcontroller/dev_2c2t_Pipedeck/manifest.json"),
    ),
    (
        "locales.csv",
        include_str!("../../../integrations/streamcontroller/dev_2c2t_Pipedeck/locales.csv"),
    ),
];
const STREAMCONTROLLER_PLUGIN: &str = "dev_2c2t_Pipedeck";

/// Where the mixer's icons are, in the application's resources.
const ICONS: &str = "/dev/_2c2t/Pipedeck/icons/scalable/actions";

/// Left in an installed plugin: what was installed, to tell when it is out
/// of date.
const MARKER: &str = ".pipedeck";
/// What the mixer had when the profiles and pages were last laid out, to
/// tell when they need it again: kept in Pipedeck's cache.
fn laid_out_marker() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"))
        .join("pipedeck")
        .join("stream-deck-layout")
}

/// Whether Pipedeck's plugin is in an application.
fn has_plugin(app: App) -> bool {
    matches!(state(app), State::Installed { .. })
}

/// Whether Pipedeck's plugin is in either application, which is when
/// there are profiles or pages to lay out.
pub fn any_installed() -> bool {
    has_plugin(App::OpenDeck) || has_plugin(App::StreamController)
}

/// One of the Stream Deck applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    OpenDeck,
    StreamController,
}

impl App {
    pub fn name(self) -> &'static str {
        match self {
            App::OpenDeck => "OpenDeck",
            App::StreamController => "StreamController",
        }
    }

    fn program(self) -> &'static str {
        match self {
            App::OpenDeck => "opendeck",
            App::StreamController => "streamcontroller",
        }
    }

    /// Where the application keeps its plugins.
    fn plugins_dir(self) -> PathBuf {
        match self {
            App::OpenDeck => opendeck_dir().join("plugins"),
            App::StreamController => home()
                .join(".var/app/com.core447.StreamController/data")
                .join("plugins"),
        }
    }

    fn plugin_dir(self) -> PathBuf {
        self.plugins_dir().join(match self {
            App::OpenDeck => OPENDECK_PLUGIN,
            App::StreamController => STREAMCONTROLLER_PLUGIN,
        })
    }
}

/// Where an application stands with Pipedeck's plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The application is not on this machine.
    Missing,
    /// It is, without the plugin.
    Available,
    /// It has the plugin, as this Pipedeck has it or not.
    Installed { current: bool },
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn opendeck_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("opendeck")
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// OpenDeck's plugin program: beside this one, as a build leaves them,
/// or wherever the system put it.
fn opendeck_program() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(OPENDECK_PROGRAM)))
        .filter(|path| path.is_file())
        .or_else(|| on_path(OPENDECK_PROGRAM))
}

/// Where the plugin program goes in OpenDeck's plugin, by what the machine
/// is, as the manifest names it.
fn opendeck_target() -> String {
    format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
}

/// FNV-1a: small, and the same from one build of Pipedeck to the next.
fn fingerprint<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        // Parts told apart, so moving a byte from one to the next shows.
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// What this Pipedeck would install, as a fingerprint.
fn wanted(app: App) -> Option<String> {
    match app {
        App::OpenDeck => {
            let program = std::fs::read(opendeck_program()?).ok()?;
            Some(fingerprint([
                OPENDECK_MANIFEST.as_bytes(),
                OPENDECK_LAYOUT.as_bytes(),
                OPENDECK_PAGE.as_bytes(),
                program.as_slice(),
            ]))
        }
        App::StreamController => Some(fingerprint(
            STREAMCONTROLLER_FILES
                .iter()
                .map(|(_, text)| text.as_bytes()),
        )),
    }
}

/// Can this Pipedeck install the plugin? OpenDeck's needs its program,
/// which is built beside Pipedeck.
pub fn can_install(app: App) -> Result<(), String> {
    match app {
        App::OpenDeck if opendeck_program().is_none() => Err(format!(
            "{OPENDECK_PROGRAM} was not built with this Pipedeck: \
             cargo build --release builds it beside it."
        )),
        _ => Ok(()),
    }
}

/// Where an application stands now.
pub fn state(app: App) -> State {
    let present =
        on_path(app.program()).is_some() || app.plugins_dir().parent().is_some_and(Path::is_dir);
    if !present {
        return State::Missing;
    }
    let dir = app.plugin_dir();
    if !dir.is_dir() {
        return State::Available;
    }
    let installed = std::fs::read_to_string(dir.join(MARKER)).ok();
    State::Installed {
        current: installed.is_some() && installed == wanted(app),
    }
}

/// Is a program of this name running, and which?
fn running(name: &str) -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<i32>().ok()?;
            let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
            (comm.trim() == name).then_some(pid)
        })
        .collect()
}

/// Close OpenDeck, if it runs, and say whether it did: it writes what it
/// holds when it closes, so it is waited for.
fn close_opendeck() -> bool {
    let pids = running("opendeck");
    if pids.is_empty() {
        return false;
    }
    for pid in &pids {
        // SAFETY: a signal to a process of the user's, found by its name.
        unsafe {
            libc::kill(*pid, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !running("opendeck").is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    // One that would not hear it: started with SIGTERM blocked, as a
    // Pipedeck before this one started it.
    for pid in running("opendeck") {
        log::warn!("OpenDeck did not close when asked; killing it");
        // SAFETY: as above.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while !running("opendeck").is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Start OpenDeck again, in the background as it starts with the session,
/// and not as a child of the mixer, which may close first.
fn start_opendeck() {
    let mut command = match on_path("setsid") {
        Some(setsid) => {
            let mut command = Command::new(setsid);
            command.args(["-f", "opendeck", "--hide"]);
            command
        }
        None => {
            let mut command = Command::new("opendeck");
            command.arg("--hide");
            command
        }
    };
    let started = crate::signals::unblocked_in(&mut command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(e) = started {
        log::warn!("cannot start OpenDeck again: {e}");
    }
}

/// Install the plugin into an application, or put this Pipedeck's over the
/// one there, and say what to do next.
pub fn install(app: App, lay_out: bool) -> Result<String, String> {
    can_install(app)?;
    let _busy = Busy::take()?;
    match app {
        App::OpenDeck => {
            let was_running = close_opendeck();
            let result = install_opendeck().and_then(|()| {
                if lay_out {
                    opendeck_profiles_now()
                } else {
                    Ok(())
                }
            });
            if was_running {
                start_opendeck();
            }
            result.map(|()| {
                if lay_out {
                    "Installed, with a Pipedeck profile for each deck: pick it in OpenDeck."
                        .to_owned()
                } else {
                    "Installed: the actions are under Pipedeck in OpenDeck.".to_owned()
                }
            })
        }
        App::StreamController => install_streamcontroller()
            .and_then(|()| {
                if lay_out {
                    streamcontroller_pages_now()
                } else {
                    Ok(())
                }
            })
            .map(|()| "Installed: restart StreamController to load it.".to_owned()),
    }
}

fn write(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

fn install_opendeck() -> Result<(), String> {
    let program = opendeck_program().ok_or("the plugin program is missing")?;
    let dir = App::OpenDeck.plugin_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    write(&dir.join("manifest.json"), OPENDECK_MANIFEST.as_bytes())?;
    write(&dir.join("layouts/strip.json"), OPENDECK_LAYOUT.as_bytes())?;
    write(
        &dir.join("propertyInspector/index.html"),
        OPENDECK_PAGE.as_bytes(),
    )?;
    let installed = dir
        .join(opendeck_target())
        .join("bin")
        .join(OPENDECK_PROGRAM);
    if let Some(bin) = installed.parent() {
        std::fs::create_dir_all(bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    }
    std::fs::copy(&program, &installed).map_err(|e| format!("{}: {e}", installed.display()))?;
    let icons = dir.join("icons");
    std::fs::create_dir_all(&icons).map_err(|e| format!("{}: {e}", icons.display()))?;
    let drawn = Command::new(&installed)
        .arg("--icons")
        .arg(&icons)
        .status()
        .map_err(|e| format!("{}: {e}", installed.display()))?;
    if !drawn.success() {
        return Err("the plugin could not draw its icons".into());
    }
    write(
        &dir.join(MARKER),
        wanted(App::OpenDeck).unwrap_or_default().as_bytes(),
    )
}

fn install_streamcontroller() -> Result<(), String> {
    let dir = App::StreamController.plugin_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    for (name, text) in STREAMCONTROLLER_FILES {
        write(&dir.join(name), text.as_bytes())?;
    }
    // The mixer's icons, which the plugin draws its badges with.
    let icons = gio::resources_enumerate_children(ICONS, gio::ResourceLookupFlags::NONE)
        .map_err(|e| format!("the icons: {e}"))?;
    for name in icons {
        let data =
            gio::resources_lookup_data(&format!("{ICONS}/{name}"), gio::ResourceLookupFlags::NONE)
                .map_err(|e| format!("{name}: {e}"))?;
        write(&dir.join("assets/icons").join(name.as_str()), &data)?;
    }
    write(
        &dir.join(MARKER),
        wanted(App::StreamController).unwrap_or_default().as_bytes(),
    )
}

/// Take the plugin out of an application.
pub fn remove(app: App) -> Result<String, String> {
    let _busy = Busy::take()?;
    let was_running = app == App::OpenDeck && close_opendeck();
    let dir = app.plugin_dir();
    let result = std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()));
    if was_running {
        start_opendeck();
    }
    result.map(|()| match app {
        App::OpenDeck => {
            "Removed. The Pipedeck profiles are left, with nothing on them.".to_owned()
        }
        App::StreamController => "Removed: restart StreamController to let go of it.".to_owned(),
    })
}

/// Lay out the Pipedeck profiles of OpenDeck and the Pipedeck pages of
/// StreamController, whichever have the plugin, from the mixer as it is:
/// OpenDeck is closed meanwhile.
pub fn lay_out_profiles() -> Result<String, String> {
    let _busy = Busy::take()?;
    let mut said = Vec::new();
    if has_plugin(App::OpenDeck) {
        let was_running = close_opendeck();
        let result = opendeck_profiles_now();
        if was_running {
            start_opendeck();
        }
        result?;
        said.push("pick the Pipedeck profile in OpenDeck");
    }
    if has_plugin(App::StreamController) {
        streamcontroller_pages_now()?;
        said.push("the Pipedeck page of your deck in StreamController");
    }
    Ok(format!("Laid out: {}.", said.join(", and ")))
}

/// Run OpenDeck's plugin program on something to lay out, and note what
/// it was laid out from, so it is not laid out again for it.
fn run_layout(program: &Path, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(said
            .trim()
            .trim_start_matches(&format!("{OPENDECK_PROGRAM}: "))
            .to_owned());
    }
    if let Some(layout) = LATEST.lock().ok().and_then(|latest| latest.clone()) {
        let marker = laid_out_marker();
        let written = marker
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&marker, layout));
        if let Err(e) = written {
            log::warn!("{}: {e}", marker.display());
        }
    }
    Ok(())
}

fn opendeck_profiles_now() -> Result<(), String> {
    let program = App::OpenDeck
        .plugin_dir()
        .join(opendeck_target())
        .join("bin")
        .join(OPENDECK_PROGRAM);
    run_layout(&program, &["--profiles".as_ref()])
}

/// StreamController's pages, laid out by the same program, wherever it
/// is: beside Pipedeck, or in OpenDeck's plugin.
fn streamcontroller_pages_now() -> Result<(), String> {
    let program = opendeck_program()
        .or_else(|| {
            let installed = App::OpenDeck
                .plugin_dir()
                .join(opendeck_target())
                .join("bin")
                .join(OPENDECK_PROGRAM);
            installed.is_file().then_some(installed)
        })
        .ok_or_else(|| format!("{OPENDECK_PROGRAM}, which lays the pages out, is missing"))?;
    let pages = App::StreamController
        .plugins_dir()
        .parent()
        .map(|data| data.join("pages"))
        .ok_or("StreamController keeps its pages nowhere")?;
    run_layout(
        &program,
        &["--streamcontroller-pages".as_ref(), pages.as_os_str()],
    )
}

/// Set while an install, a removal or a layout runs: two at once would
/// close and start OpenDeck over each other.
static BUSY: AtomicBool = AtomicBool::new(false);

struct Busy;

impl Busy {
    fn take() -> Result<Self, String> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Busy)
            .map_err(|_| "Pipedeck is already at it.".to_owned())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// What a deck's profile is laid out from: the channels, mixes, cells and
/// devices, by id, and a microphone's effects. A name is only what a key says while the mixer is
/// away, and a level is the mixer's to say: neither calls for laying out
/// again.
fn layout_of(state: &StateSnapshot, outputs: &[Device]) -> String {
    let mut parts = Vec::new();
    for source in &state.sources {
        parts.push(format!("c{}", source.id.0));
        // A microphone's effects have keys of their own.
        if source.is_input() {
            for effect in &source.effects {
                parts.push(format!("e{}:{}", source.id.0, effect.name));
            }
        }
    }
    for mix in &state.mixes {
        parts.push(format!("m{}", mix.id.0));
    }
    for link in &state.links {
        parts.push(format!("l{}.{}", link.source.0, link.mix.0));
    }
    for output in outputs {
        parts.push(format!("o{}", output.name));
    }
    parts.join(" ")
}

/// Which wait for the mixer to settle is the latest: an earlier one finds
/// it is not, and leaves it to this one.
static WAITING: AtomicU64 = AtomicU64::new(0);
/// What the mixer last had, as [`layout_of`] puts it.
static LATEST: Mutex<Option<String>> = Mutex::new(None);

/// Take in what the mixer has, and lay the profiles out again, a moment
/// after it stops changing, when what they are laid out from changed:
/// a channel, a mix, a cell or a device came or went.
pub fn mixer_changed(mixer: &StateSnapshot, outputs: &[Device], enabled: bool) {
    // Devices are told apart from the matrix: until both are, a layout
    // would be taken from half of it.
    if !enabled || outputs.is_empty() || mixer.mixes.is_empty() {
        return;
    }
    let layout = layout_of(mixer, outputs);
    if let Ok(mut latest) = LATEST.lock() {
        if latest.as_deref() == Some(layout.as_str()) {
            return;
        }
        *latest = Some(layout);
    }
    let this = WAITING.fetch_add(1, Ordering::AcqRel) + 1;
    adw::glib::timeout_add_local_once(Duration::from_secs(3), move || {
        if WAITING.load(Ordering::Acquire) != this {
            return;
        }
        let Some(layout) = LATEST.lock().ok().and_then(|latest| latest.clone()) else {
            return;
        };
        if !any_installed() {
            return;
        }
        let marker = laid_out_marker();
        if std::fs::read_to_string(&marker).ok().as_deref() == Some(layout.as_str()) {
            return;
        }
        std::thread::spawn(move || match lay_out_profiles() {
            Ok(_) => log::info!("the Stream Deck profiles were laid out again"),
            Err(e) => log::warn!("cannot lay out the Stream Deck profiles: {e}"),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_tells_its_parts_apart() {
        assert_ne!(
            fingerprint([&b"ab"[..], b"c"]),
            fingerprint([&b"a"[..], b"bc"])
        );
        assert_eq!(fingerprint([&b"ab"[..]]), fingerprint([&b"ab"[..]]));
    }

    #[test]
    fn the_plugins_are_carried_whole() {
        assert!(OPENDECK_MANIFEST.contains("dev.2c2t.pipedeck.channel"));
        assert!(OPENDECK_PAGE.contains("connectElgatoStreamDeckSocket"));
        let names: Vec<&str> = STREAMCONTROLLER_FILES.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"main.py") && names.contains(&"manifest.json"));
    }

    #[test]
    fn a_name_or_a_level_does_not_call_for_laying_out_again() {
        let mut state = StateSnapshot {
            latency: String::new(),
            stereotool_license: None,
            listen_device: None,
            mixes: vec![pipedeck_engine::MixConfig::new(
                pipedeck_engine::MixId(1),
                "Stream",
            )],
            sources: vec![pipedeck_engine::SourceConfig::virtual_sink(
                pipedeck_engine::SourceId(2),
                "Music",
            )],
            links: Vec::new(),
        };
        let before = layout_of(&state, &[]);
        state.sources[0].name = "Tunes".into();
        state.sources[0].gain = 0.3;
        assert_eq!(layout_of(&state, &[]), before);
        state.mixes.push(pipedeck_engine::MixConfig::new(
            pipedeck_engine::MixId(2),
            "Chat",
        ));
        assert_ne!(layout_of(&state, &[]), before);
    }
}
