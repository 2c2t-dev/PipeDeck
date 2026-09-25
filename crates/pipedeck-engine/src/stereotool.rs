//! Hosting Thimeo's Stereo Tool through `libStereoTool`.
//!
//! Stereo Tool is a broadcast processor, and Thimeo ships it for Linux as a
//! shared library with a plain C entry table: the same one Liquidsoap calls.
//! That is what the mixer uses, rather than the VST3 build, because the
//! library takes a preset file and a licence key through the API while the
//! plug-in would need its own window to be worth anything.
//!
//! Nothing of it is bundled: it is proprietary, so the user installs it and
//! the mixer looks for it where it was put.
//!
//! Every call here is unsafe by nature — it is the library's own code that
//! runs — and the symbols are exactly those the API exposes.

use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

/// How many channels the mixer hands the processor.
pub const CHANNELS: usize = 2;

/// The sample rate the graph runs at, which is what the processor is told.
pub const SAMPLE_RATE: i32 = 48_000;

/// What `stereoTool_LoadPreset` is asked to take out of a preset.
///
/// The API takes a magic number saying which subset of the settings to load.
/// A mixer wants the whole preset, which is what "total init" means.
const LOAD_TOTALINIT: c_int = 10387;

/// Where the mixer keeps the library, which is where its importer puts it.
pub fn library_dir() -> Option<PathBuf> {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        if !data.is_empty() {
            return Some(PathBuf::from(data).join("pipedeck").join("stereotool"));
        }
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/share/pipedeck/stereotool"))
}

/// The names Thimeo gives the library for this machine, best first.
///
/// The download holds one build per machine and, for each, several saying
/// what they do about X11. The X11 one comes first because it is the only
/// one carrying Stereo Tool's own window — the vendor's header says as much
/// — and the others follow for a machine whose X11 libraries it would ask
/// for and not find.
fn preferred_names() -> &'static [&'static str] {
    match std::env::consts::ARCH {
        "x86_64" => &[
            "libStereoToolX11_intel64.so",
            "libStereoTool_intel64.so",
            "libStereoTool_noX11_intel64.so",
        ],
        "x86" => &[
            "libStereoToolX11_intel32.so",
            "libStereoTool_intel32.so",
            "libStereoTool_noX11_intel32.so",
        ],
        "aarch64" => &[
            "libStereoToolX11_arm64.so",
            "libStereoTool_arm64.so",
            "libStereoTool_noX11_arm64.so",
            "libStereoTool_pi4_64.so",
        ],
        "arm" => &[
            "libStereoToolX11_arm32.so",
            "libStereoTool_arm32.so",
            "libStereoTool_noX11_arm32.so",
            "libStereoTool_pi2.so",
        ],
        _ => &[],
    }
}

/// Does this file look like the library, whatever machine it is for?
pub fn is_library(name: &str) -> bool {
    name.starts_with("libStereoTool") && name.ends_with(".so")
}

/// Is this build the one for this machine?
///
/// The archive carries every machine's build side by side, and their names
/// are all that tells them apart. A build for another architecture is no use
/// here and would only be tried and refused.
pub fn is_for_this_machine(name: &str) -> bool {
    let token = match std::env::consts::ARCH {
        "x86_64" => "intel64",
        "x86" => "intel32",
        "aarch64" => "arm64",
        "arm" => "arm32",
        // An architecture Thimeo does not name: take what there is and let
        // the loader say whether it runs.
        _ => return true,
    };
    name.contains(token)
}

/// The library to load, if one is installed.
///
/// `PIPEDECK_STEREOTOOL` names a file outright, which is how a user points at
/// a copy kept elsewhere.
pub fn library_path() -> Option<PathBuf> {
    candidates().into_iter().next()
}

/// Every library worth trying, best first.
fn candidates() -> Vec<PathBuf> {
    if let Some(named) = std::env::var_os("PIPEDECK_STEREOTOOL") {
        let path = PathBuf::from(named);
        return if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    library_dir().map(|dir| choose(&dir)).unwrap_or_default()
}

/// The libraries in one directory, the build for this machine first.
fn choose(directory: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = preferred_names()
        .iter()
        .map(|name| directory.join(name))
        .filter(|path| path.is_file())
        .collect();

    // A name the vendor has not used before, or a single file a user put
    // there by hand: worth trying once the known ones are exhausted.
    let mut others: Vec<PathBuf> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| is_library(name) && is_for_this_machine(name))
        })
        .filter(|path| !found.contains(path))
        .collect();
    others.sort();
    found.append(&mut others);
    found
}

/// What the settings window says about the installed copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub path: PathBuf,
    /// The build number the library reports.
    pub version: i32,
    /// Whether the licence key it was given covers what it runs.
    pub licensed: bool,
    /// The features it is using without a licence for them, as it names
    /// them. Those are what put speech and beeps in the audio.
    pub unlicensed: Option<String>,
    /// Whether this build carries Stereo Tool's own window. The builds made
    /// for machines without X11 do not.
    pub windows: bool,
}

/// What the mixer can say about Stereo Tool without being asked twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// No copy installed. The user has one to import.
    Absent,
    /// One is there but will not load or open, with the reason.
    Broken(String),
    Ready(Info),
}

/// The entry points the library exports, as Liquidsoap's binding names them.
struct Api {
    create: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    /// The same, told outright whether the processor is to carry its own
    /// interface. Plain `create` decides for itself, and what it decides is
    /// not documented, so this is what the mixer asks for when the build has
    /// a window to give.
    create2: Option<unsafe extern "C" fn(bool, *const c_char) -> *mut c_void>,
    delete: unsafe extern "C" fn(*mut c_void),
    process: unsafe extern "C" fn(*mut c_void, *mut f32, c_int, c_int, c_int),
    latency: unsafe extern "C" fn(*mut c_void, c_int, bool) -> c_int,
    load_preset: unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> bool,
    check_license: unsafe extern "C" fn(*mut c_void) -> bool,
    unlicensed: unsafe extern "C" fn(*mut c_void, *mut c_char, c_int) -> bool,
    software_version: unsafe extern "C" fn() -> c_int,
    /// Stereo Tool's own window. Only the X11 builds carry it, so this is
    /// what the mixer has rather than a promise it cannot keep.
    gui: Option<Gui>,
    path: PathBuf,
}

/// The window entry points, taken together because a build has all of them
/// or none.
struct Gui {
    create: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    show: unsafe extern "C" fn(*mut c_void, *mut c_void),
    hide: unsafe extern "C" fn(*mut c_void),
    delete: unsafe extern "C" fn(*mut c_void),
}

/// The library, once it has been loaded.
///
/// A process loads one and keeps it: the library is not made to come and go,
/// and unloading one that registered anything with the process would take
/// the mixer down with it. A failure is not remembered, so installing it
/// while the mixer runs works on the next look.
static API: OnceLock<&'static Api> = OnceLock::new();

fn api() -> Result<&'static Api, String> {
    if let Some(api) = API.get() {
        return Ok(api);
    }
    let mut refused = None;
    for path in candidates() {
        // SAFETY: loading is the library's own code, which is the only way
        // to host it; the symbols are read straight after and none is
        // called yet.
        match unsafe { load(&path) } {
            Ok(loaded) => {
                let leaked: &'static Api = Box::leak(Box::new(loaded));
                return Ok(API.get_or_init(|| leaked));
            }
            Err(e) => {
                log::warn!("{}: {e}", path.display());
                refused = Some(e);
            }
        }
    }
    Err(refused.unwrap_or_else(|| "Stereo Tool is not installed".to_owned()))
}

/// Load the library and take its entry points.
///
/// # Safety
///
/// The caller gets function pointers into a library that is deliberately
/// never unloaded, so they stay valid for the life of the process. Each
/// signature is the one the API documents.
unsafe fn load(path: &Path) -> Result<Api, String> {
    // Loading runs whatever the library does at load time, which is already
    // where some of the noise comes from.
    let quiet = Hushed::new();
    let library = libloading::Library::new(path).map_err(|e| e.to_string())?;
    drop(quiet);
    let symbol = |name: &[u8]| -> Result<*mut c_void, String> {
        library
            .get::<*mut c_void>(name)
            .map(|symbol| *symbol)
            .map_err(|_| {
                format!(
                    "{} exports no {}",
                    path.display(),
                    String::from_utf8_lossy(&name[..name.len() - 1])
                )
            })
    };
    let api = Api {
        create: std::mem::transmute::<
            *mut c_void,
            unsafe extern "C" fn(*const c_char) -> *mut c_void,
        >(symbol(b"stereoTool_Create\0")?),
        create2: symbol(b"stereoTool_Create2\0").ok().map(|create2| {
            std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(bool, *const c_char) -> *mut c_void,
            >(create2)
        }),
        delete: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void)>(symbol(
            b"stereoTool_Delete\0",
        )?),
        process: std::mem::transmute::<
            *mut c_void,
            unsafe extern "C" fn(*mut c_void, *mut f32, c_int, c_int, c_int),
        >(symbol(b"stereoTool_Process\0")?),
        latency: std::mem::transmute::<
            *mut c_void,
            unsafe extern "C" fn(*mut c_void, c_int, bool) -> c_int,
        >(symbol(b"stereoTool_GetLatency2\0")?),
        load_preset: std::mem::transmute::<
            *mut c_void,
            unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> bool,
        >(symbol(b"stereoTool_LoadPreset\0")?),
        check_license: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void) -> bool>(
            symbol(b"stereoTool_CheckLicenseValid\0")?,
        ),
        unlicensed: std::mem::transmute::<
            *mut c_void,
            unsafe extern "C" fn(*mut c_void, *mut c_char, c_int) -> bool,
        >(symbol(b"stereoTool_GetUnlicensedUsedFeatures\0")?),
        software_version: std::mem::transmute::<*mut c_void, unsafe extern "C" fn() -> c_int>(
            symbol(b"stereoTool_GetSoftwareVersion\0")?,
        ),
        gui: match (
            symbol(b"stereoTool_GUI_Create\0"),
            symbol(b"stereoTool_GUI_Show\0"),
            symbol(b"stereoTool_GUI_Hide\0"),
            symbol(b"stereoTool_GUI_SetSize\0"),
            symbol(b"stereoTool_GUI_Delete\0"),
        ) {
            // `GUI_SetSize` is asked for and not kept: a build that has it
            // is the one carrying the window, but calling it walks into
            // XResizeWindow with nothing to resize. See `Window::open`.
            (Ok(create), Ok(show), Ok(hide), Ok(_set_size), Ok(delete)) => Some(Gui {
                create: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut c_void) -> *mut c_void,
                >(create),
                show: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut c_void, *mut c_void),
                >(show),
                hide: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void)>(hide),
                delete: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut c_void)>(
                    delete,
                ),
            }),
            // A build without them is the one the vendor ships for machines
            // with no X11 at all. Everything else about it works.
            _ => None,
        },
        path: path.to_owned(),
    };
    // Kept loaded on purpose: see the note on `API`.
    std::mem::forget(library);
    Ok(api)
}

/// Is a copy installed at all? Answers without loading anything.
pub fn installed() -> bool {
    !candidates().is_empty()
}

/// What to tell the user about the installed copy, asking the library
/// itself when there is one to ask.
pub fn status(license: Option<&str>) -> Status {
    if !installed() {
        return Status::Absent;
    }
    match probe(license) {
        Ok(info) => Status::Ready(info),
        Err(e) => Status::Broken(e),
    }
}

/// Load the library, open one processor and report what it says about
/// itself. This runs the library's code, so it happens on demand.
pub fn probe(license: Option<&str>) -> Result<Info, String> {
    let api = api()?;
    let instance = Instance::open(None, license)?;
    Ok(Info {
        path: api.path.clone(),
        // SAFETY: the library is loaded and the call takes nothing.
        version: unsafe { (api.software_version)() },
        licensed: instance.licensed(),
        unlicensed: instance.unlicensed_features(),
        windows: api.gui.is_some(),
    })
}

/// Keeps the library's chatter off the terminal.
///
/// Stereo Tool walks every ALSA device and looks for a JACK server each time
/// a processor is created, and says so on the standard error — a few hundred
/// lines, written in C straight to the descriptor, which no Rust logger can
/// filter. The descriptor is pointed at `/dev/null` for exactly as long as
/// the call takes and put back after.
///
/// The descriptor belongs to the whole process, so a line the mixer itself
/// logs during that moment is lost with it. That is the trade: a handful of
/// milliseconds against a screenful on every start. `PIPEDECK_STEREOTOOL_NOISE`
/// turns it off when the library's own words are what is wanted.
struct Hushed(Option<c_int>);

impl Hushed {
    fn new() -> Self {
        if std::env::var_os("PIPEDECK_STEREOTOOL_NOISE").is_some() {
            return Self(None);
        }
        // SAFETY: plain descriptor calls, each checked; nothing is kept on
        // failure, and the drop below only acts on what was taken.
        unsafe {
            let saved = libc::dup(libc::STDERR_FILENO);
            if saved < 0 {
                return Self(None);
            }
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            if null < 0 {
                libc::close(saved);
                return Self(None);
            }
            libc::dup2(null, libc::STDERR_FILENO);
            libc::close(null);
            Self(Some(saved))
        }
    }
}

impl Drop for Hushed {
    fn drop(&mut self) {
        if let Some(saved) = self.0 {
            // SAFETY: `saved` is ours, taken in `new` and released once.
            unsafe {
                libc::dup2(saved, libc::STDERR_FILENO);
                libc::close(saved);
            }
        }
    }
}

/// Why a block did not go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProcessError {
    #[error(
        "the buffers are empty, uneven, or longer than the block the processor was opened for"
    )]
    BlockSize,
}

/// A processor the library made, released when the last holder lets go.
///
/// The audio thread runs it block by block while the control thread opens
/// and closes its window, which is how every plug-in host is built and what
/// the vendor's own VST does. The pointer is therefore shared rather than
/// owned by either side.
pub struct Handle {
    ptr: *mut c_void,
    api: &'static Api,
}

// SAFETY: the library is made to be driven this way — one thread handing it
// blocks, another showing its window — and nothing here reads or writes the
// pointer itself, only passes it back.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // Closing is as talkative as opening.
        let _quiet = Hushed::new();
        // SAFETY: the pointer came from `create` and is released once, when
        // the last holder of this handle lets go.
        unsafe { (self.api.delete)(self.ptr) };
    }
}

/// The X11 calls it takes to give Stereo Tool a window the window manager
/// can close.
struct X11 {
    init_threads: unsafe extern "C" fn() -> c_int,
    open_display: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    close_display: unsafe extern "C" fn(*mut c_void) -> c_int,
    default_root: unsafe extern "C" fn(*mut c_void) -> c_ulong,
    create_window: unsafe extern "C" fn(
        *mut c_void,
        c_ulong,
        c_int,
        c_int,
        c_uint,
        c_uint,
        c_uint,
        c_ulong,
        c_ulong,
    ) -> c_ulong,
    destroy_window: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int,
    map_window: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int,
    raise_window: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int,
    move_window: unsafe extern "C" fn(*mut c_void, c_ulong, c_int, c_int) -> c_int,
    reparent_window: unsafe extern "C" fn(*mut c_void, c_ulong, c_ulong, c_int, c_int) -> c_int,
    resize_window: unsafe extern "C" fn(*mut c_void, c_ulong, c_uint, c_uint) -> c_int,
    store_name: unsafe extern "C" fn(*mut c_void, c_ulong, *const c_char) -> c_int,
    intern_atom: unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong,
    set_wm_protocols: unsafe extern "C" fn(*mut c_void, c_ulong, *mut c_ulong, c_int) -> c_int,
    set_wm_normal_hints: unsafe extern "C" fn(*mut c_void, c_ulong, *mut XSizeHints),
    select_input: unsafe extern "C" fn(*mut c_void, c_ulong, c_long) -> c_int,
    query_tree: unsafe extern "C" fn(
        *mut c_void,
        c_ulong,
        *mut c_ulong,
        *mut c_ulong,
        *mut *mut c_ulong,
        *mut c_uint,
    ) -> c_int,
    fetch_name: unsafe extern "C" fn(*mut c_void, c_ulong, *mut *mut c_char) -> c_int,
    free: unsafe extern "C" fn(*mut c_void) -> c_int,
    get_window_attributes:
        unsafe extern "C" fn(*mut c_void, c_ulong, *mut XWindowAttributes) -> c_int,
    pending: unsafe extern "C" fn(*mut c_void) -> c_int,
    next_event: unsafe extern "C" fn(*mut c_void, *mut XEvent) -> c_int,
    flush: unsafe extern "C" fn(*mut c_void) -> c_int,
}

/// Xlib's `XSizeHints`, as far as the window manager reads it.
#[repr(C)]
struct XSizeHints {
    flags: c_long,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    min_width: c_int,
    min_height: c_int,
    max_width: c_int,
    max_height: c_int,
    width_inc: c_int,
    height_inc: c_int,
    min_aspect_x: c_int,
    min_aspect_y: c_int,
    max_aspect_x: c_int,
    max_aspect_y: c_int,
    base_width: c_int,
    base_height: c_int,
    win_gravity: c_int,
}

const P_MIN_SIZE: c_long = 1 << 4;
const P_MAX_SIZE: c_long = 1 << 5;

/// Xlib's `XWindowAttributes`, whole, since Xlib writes all of it.
#[repr(C)]
struct XWindowAttributes {
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    border_width: c_int,
    depth: c_int,
    visual: *mut c_void,
    root: c_ulong,
    class: c_int,
    bit_gravity: c_int,
    win_gravity: c_int,
    backing_store: c_int,
    backing_planes: c_ulong,
    backing_pixel: c_ulong,
    save_under: c_int,
    colormap: c_ulong,
    map_installed: c_int,
    map_state: c_int,
    all_event_masks: c_long,
    your_event_mask: c_long,
    do_not_propagate_mask: c_long,
    override_redirect: c_int,
    screen: *mut c_void,
}

/// Xlib's event union, big enough for any of them. Only the type is read,
/// the one message the window manager sends, and where a window went.
#[repr(C)]
struct XEvent {
    words: [c_long; 24],
}

#[repr(C)]
struct XClientMessageEvent {
    type_: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut c_void,
    window: c_ulong,
    message_type: c_ulong,
    format: c_int,
    data: [c_long; 5],
}

#[repr(C)]
struct XConfigureEvent {
    type_: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut c_void,
    event: c_ulong,
    window: c_ulong,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    border_width: c_int,
    above: c_ulong,
    override_redirect: c_int,
}

const CONFIGURE_NOTIFY: c_int = 22;
const CLIENT_MESSAGE: c_int = 33;
const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;
const SUBSTRUCTURE_NOTIFY_MASK: c_long = 1 << 19;

/// X11, if this machine has it. Loaded once, for the same reasons as the
/// library itself.
static X11: OnceLock<Option<&'static X11>> = OnceLock::new();

fn x11() -> Option<&'static X11> {
    *X11.get_or_init(|| {
        // SAFETY: the library is the system's own, kept for the life of the
        // process, and each field's type is the signature Xlib publishes,
        // which is what the symbol is read as.
        let loaded = unsafe {
            let library = libloading::Library::new("libX11.so.6").ok()?;
            macro_rules! symbol {
                ($name:literal) => {
                    *library.get($name).ok()?
                };
            }
            let x11 = X11 {
                init_threads: symbol!(b"XInitThreads\0"),
                open_display: symbol!(b"XOpenDisplay\0"),
                close_display: symbol!(b"XCloseDisplay\0"),
                default_root: symbol!(b"XDefaultRootWindow\0"),
                create_window: symbol!(b"XCreateSimpleWindow\0"),
                destroy_window: symbol!(b"XDestroyWindow\0"),
                map_window: symbol!(b"XMapWindow\0"),
                raise_window: symbol!(b"XRaiseWindow\0"),
                move_window: symbol!(b"XMoveWindow\0"),
                reparent_window: symbol!(b"XReparentWindow\0"),
                resize_window: symbol!(b"XResizeWindow\0"),
                store_name: symbol!(b"XStoreName\0"),
                intern_atom: symbol!(b"XInternAtom\0"),
                set_wm_protocols: symbol!(b"XSetWMProtocols\0"),
                set_wm_normal_hints: symbol!(b"XSetWMNormalHints\0"),
                select_input: symbol!(b"XSelectInput\0"),
                query_tree: symbol!(b"XQueryTree\0"),
                fetch_name: symbol!(b"XFetchName\0"),
                free: symbol!(b"XFree\0"),
                get_window_attributes: symbol!(b"XGetWindowAttributes\0"),
                pending: symbol!(b"XPending\0"),
                next_event: symbol!(b"XNextEvent\0"),
                flush: symbol!(b"XFlush\0"),
            };
            // Stereo Tool draws from threads of its own. Xlib wants to be
            // told before that happens.
            (x11.init_threads)();
            std::mem::forget(library);
            Some(x11)
        }?;
        Some(Box::leak(Box::new(loaded)))
    })
}

/// What the library calls its window, which is how it is told apart.
const WINDOW_TITLE: &str = "Thimeo Stereo Tool";

/// A window of ours for Stereo Tool's to live in.
///
/// Stereo Tool's Linux interface does not open on its own: `GUI_Show` wants
/// the X11 id of a host window — the way a plug-in is handed the window its
/// host drew for it — and does nothing whatever when given none. Told about
/// one, it opens a toplevel of its own beside it, which declares the usual
/// close request and then ignores it, the way a plug-in leaves its editor
/// to whoever opened it: a window with a close button that does nothing.
///
/// So its window is adopted into ours. Reparented under it, the window
/// manager decorates ours instead, and the close button lands with a client
/// that listens. The window is the library's to draw and ours to close.
struct Host {
    x11: &'static X11,
    display: *mut c_void,
    window: c_ulong,
    /// Stereo Tool's own, once adopted.
    child: Option<c_ulong>,
    wm_delete: c_ulong,
}

impl Host {
    fn new() -> Result<Self, String> {
        let x11 = x11().ok_or("no X11 on this machine, and its window needs it")?;
        // SAFETY: the display is ours until `close_display`, and the window
        // is made on its own root with sizes the API accepts; the protocol
        // list outlives the call that copies it.
        unsafe {
            let display = (x11.open_display)(std::ptr::null());
            if display.is_null() {
                return Err("no display to open its window on".into());
            }
            let root = (x11.default_root)(display);
            let window = (x11.create_window)(display, root, 0, 0, 1, 1, 0, 0, 0);
            let mut wm_delete = (x11.intern_atom)(display, c"WM_DELETE_WINDOW".as_ptr(), 0);
            (x11.set_wm_protocols)(display, window, &mut wm_delete, 1);
            // Its own events for the close request, its children's to see
            // where Stereo Tool puts its window once it is in ours.
            (x11.select_input)(
                display,
                window,
                STRUCTURE_NOTIFY_MASK | SUBSTRUCTURE_NOTIFY_MASK,
            );
            (x11.flush)(display);
            Ok(Self {
                x11,
                display,
                window,
                child: None,
                wm_delete,
            })
        }
    }

    /// Every toplevel there is right now.
    fn toplevels(&self) -> Vec<c_ulong> {
        let mut root = 0;
        let mut parent = 0;
        let mut children: *mut c_ulong = std::ptr::null_mut();
        let mut count: c_uint = 0;
        // SAFETY: the out-pointers are ours, and what Xlib hands back is
        // freed with its own `free` once copied.
        unsafe {
            let screen_root = (self.x11.default_root)(self.display);
            if (self.x11.query_tree)(
                self.display,
                screen_root,
                &mut root,
                &mut parent,
                &mut children,
                &mut count,
            ) == 0
                || children.is_null()
            {
                return Vec::new();
            }
            let found = std::slice::from_raw_parts(children, count as usize).to_vec();
            (self.x11.free)(children.cast());
            found
        }
    }

    /// What a window calls itself, if it says.
    fn name(&self, window: c_ulong) -> Option<String> {
        let mut name: *mut c_char = std::ptr::null_mut();
        // SAFETY: the out-pointer is ours; a non-null name is Xlib's, read
        // and then freed with its own `free`.
        unsafe {
            if (self.x11.fetch_name)(self.display, window, &mut name) == 0 || name.is_null() {
                return None;
            }
            let text = CStr::from_ptr(name).to_string_lossy().into_owned();
            (self.x11.free)(name.cast());
            Some(text)
        }
    }

    /// Take in the window Stereo Tool has just opened.
    ///
    /// `before` is what was on the screen before it was asked to: another
    /// instance's window carries the same title, and must be left alone.
    /// The library opens its window a moment after `GUI_Show` returns, so
    /// this looks for it a little while.
    fn adopt(&mut self, before: &[c_ulong]) -> Result<(), String> {
        let mut found = None;
        for _ in 0..100 {
            found = self
                .toplevels()
                .into_iter()
                .filter(|window| *window != self.window && !before.contains(window))
                .find(|window| {
                    self.name(*window)
                        .is_some_and(|name| name.contains(WINDOW_TITLE))
                });
            if found.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let child = found.ok_or("Stereo Tool opened no window to take in")?;

        // The window keeps the size the library gave it, and so does ours:
        // whether the library lays itself out again at another size is not
        // something to find out on the user.
        let mut attributes = std::mem::MaybeUninit::<XWindowAttributes>::uninit();
        // SAFETY: the out-pointer is ours and the call fills the whole
        // struct on success, which is the only case it is read in.
        let (width, height) = unsafe {
            if (self.x11.get_window_attributes)(self.display, child, attributes.as_mut_ptr()) == 0 {
                return Err("Stereo Tool's window would not say its size".into());
            }
            let attributes = attributes.assume_init();
            (attributes.width, attributes.height)
        };
        let title = self
            .name(child)
            .and_then(|name| CString::new(name).ok())
            .unwrap_or_else(|| c"Stereo Tool".to_owned());
        let mut hints = XSizeHints {
            flags: P_MIN_SIZE | P_MAX_SIZE,
            x: 0,
            y: 0,
            width,
            height,
            min_width: width,
            min_height: height,
            max_width: width,
            max_height: height,
            width_inc: 0,
            height_inc: 0,
            min_aspect_x: 0,
            min_aspect_y: 0,
            max_aspect_x: 0,
            max_aspect_y: 0,
            base_width: 0,
            base_height: 0,
            win_gravity: 0,
        };
        // SAFETY: both windows exist, ours until dropped and theirs until
        // the library deletes it, which happens before we do; the title and
        // the hints outlive the calls that copy them.
        unsafe {
            (self.x11.resize_window)(self.display, self.window, width as c_uint, height as c_uint);
            (self.x11.set_wm_normal_hints)(self.display, self.window, &mut hints);
            (self.x11.store_name)(self.display, self.window, title.as_ptr());
            (self.x11.reparent_window)(self.display, child, self.window, 0, 0);
            (self.x11.map_window)(self.display, self.window);
            (self.x11.map_window)(self.display, child);
            (self.x11.flush)(self.display);
        }
        self.child = Some(child);
        self.settle();
        Ok(())
    }

    /// Put Stereo Tool's window back in the corner of ours.
    ///
    /// The library keeps its window where it was on the screen: taken in,
    /// it moves it back to those coordinates, which are now inside ours and
    /// far past its edge, and all that shows is our own black. So it is put
    /// back, now and whenever it moves again.
    fn settle(&self) {
        let Some(child) = self.child else {
            return;
        };
        // SAFETY: both windows exist, as for `adopt`.
        unsafe {
            (self.x11.move_window)(self.display, child, 0, 0);
            (self.x11.flush)(self.display);
        }
    }

    /// Bring it to the front, having been buried or hidden.
    fn raise(&self) {
        // SAFETY: the window is ours and alive.
        unsafe {
            (self.x11.map_window)(self.display, self.window);
            (self.x11.raise_window)(self.display, self.window);
            (self.x11.flush)(self.display);
        }
    }

    /// Has the window manager asked for it to close since last time?
    ///
    /// The events wait on our own connection until someone reads them, so
    /// this is called from time to time rather than from a thread of its
    /// own; a close button that answers within a tick is answered. Stereo
    /// Tool's window having moved inside ours is among them, and it is put
    /// back on the way.
    fn close_requested(&self) -> bool {
        let mut asked = false;
        let mut moved = false;
        // SAFETY: the display is ours; `pending` says whether `next_event`
        // would block, and the event buffer is as large as Xlib's union.
        unsafe {
            while (self.x11.pending)(self.display) > 0 {
                let mut event = XEvent { words: [0; 24] };
                (self.x11.next_event)(self.display, &mut event);
                let type_ = *(&event as *const XEvent).cast::<c_int>();
                if type_ == CONFIGURE_NOTIFY {
                    let configured = &*(&event as *const XEvent).cast::<XConfigureEvent>();
                    if Some(configured.window) == self.child
                        && (configured.x, configured.y) != (0, 0)
                    {
                        moved = true;
                    }
                    continue;
                }
                if type_ != CLIENT_MESSAGE {
                    continue;
                }
                let message = &*(&event as *const XEvent).cast::<XClientMessageEvent>();
                if message.window == self.window
                    && message.format == 32
                    && message.data[0] as c_ulong == self.wm_delete
                {
                    asked = true;
                }
            }
        }
        if moved {
            self.settle();
        }
        asked
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // SAFETY: both came from the calls above and are released once. The
        // child, if any, is the library's and already gone by now.
        unsafe {
            (self.x11.destroy_window)(self.display, self.window);
            (self.x11.close_display)(self.display);
        }
    }
}

/// Stereo Tool's own window, on the processor it belongs to.
///
/// It is the whole application's interface — every band, every curve — and
/// it is the only way to set up what the mixer runs, since the settings live
/// in Stereo Tool and not here. Only the X11 builds carry it.
///
/// Dropping it takes the window away; the processor keeps running.
pub struct Window {
    // Field order matters: the interface goes before the window it lives in.
    ptr: *mut c_void,
    host: Host,
    /// Held so the processor outlives the window that draws it.
    handle: Arc<Handle>,
}

impl Window {
    /// Make the window for a processor, and put it on the screen.
    pub fn open(handle: Arc<Handle>) -> Result<Self, String> {
        let gui = handle
            .api
            .gui
            .as_ref()
            .ok_or("this build of Stereo Tool has no window; install the X11 one")?;
        let mut host = Host::new()?;
        let before = host.toplevels();
        let _quiet = Hushed::new();
        // SAFETY: the processor is alive, this handle holds it.
        let ptr = unsafe { (gui.create)(handle.ptr) };
        if ptr.is_null() {
            return Err("Stereo Tool would not make its window".into());
        }
        // SAFETY: the interface is ours, and the host window outlives it.
        unsafe { (gui.show)(ptr, host.window as *mut c_void) };
        if let Err(e) = host.adopt(&before) {
            // It is up, on its own, with a close button that does nothing;
            // the button in the mixer still closes it.
            log::warn!("{e}; the window is on its own");
        }
        Ok(Self { ptr, host, handle })
    }

    /// Bring it to the front, having been buried.
    pub fn raise(&self) {
        self.host.raise();
    }

    /// Has the user asked the window manager to close it?
    pub fn close_requested(&self) -> bool {
        self.host.close_requested()
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if let Some(gui) = self.handle.api.gui.as_ref() {
            let _quiet = Hushed::new();
            // SAFETY: the window came from `create` and is deleted once,
            // before the processor it belongs to, which this holds, and
            // before the host window it lives in, which drops after.
            unsafe {
                (gui.hide)(self.ptr);
                (gui.delete)(self.ptr);
            }
        }
    }
}

/// One Stereo Tool processor, ready to take blocks.
///
/// Dropping it releases the processor unless its window still holds it; the
/// library stays loaded either way.
pub struct Instance {
    handle: Arc<Handle>,
    /// One block, interleaved, allocated once: the real-time thread must not
    /// ask for memory.
    scratch: Vec<f32>,
    max_block: usize,
    preset: Option<PathBuf>,
}

impl Instance {
    /// Open a processor, optionally on a preset and a licence key.
    ///
    /// Without a key the processor still runs, and says so through
    /// [`Instance::licensed`]: what it puts in the audio then is speech and
    /// beeps, which is the vendor's business and not something to hide.
    pub fn open(preset: Option<&Path>, license: Option<&str>) -> Result<Self, String> {
        Self::with_block(preset, license, 2048)
    }

    pub fn with_block(
        preset: Option<&Path>,
        license: Option<&str>,
        max_block: usize,
    ) -> Result<Self, String> {
        let api = api()?;
        let key = license
            .map(|key| {
                CString::new(key).map_err(|_| "the licence key holds a zero byte".to_owned())
            })
            .transpose()?;
        // SAFETY: the key outlives the call, and a null pointer is what the
        // API takes for "no licence".
        let handle = {
            let _quiet = Hushed::new();
            let key = key.as_ref().map_or(std::ptr::null(), |key| key.as_ptr());
            match (api.create2, api.gui.is_some()) {
                // Ask for the interface outright when the build has one: a
                // processor made without it has no window to show later.
                (Some(create2), wanted) => unsafe { create2(wanted, key) },
                (None, _) => unsafe { (api.create)(key) },
            }
        };
        if handle.is_null() {
            return Err("Stereo Tool refused to start".into());
        }
        let mut instance = Self {
            handle: Arc::new(Handle { ptr: handle, api }),
            scratch: vec![0.0; max_block * CHANNELS],
            max_block,
            preset: None,
        };
        if let Some(preset) = preset {
            instance.load_preset(preset)?;
        }
        Ok(instance)
    }

    /// Read a preset file into the processor. This is how it is configured:
    /// the settings live in the file the user exported from Stereo Tool
    /// itself.
    pub fn load_preset(&mut self, preset: &Path) -> Result<(), String> {
        let path = CString::new(preset.as_os_str().as_encoded_bytes())
            .map_err(|_| format!("{} is not a usable path", preset.display()))?;
        // SAFETY: the path outlives the call and the handle is ours.
        let loaded = unsafe {
            (self.handle.api.load_preset)(self.handle.ptr, path.as_ptr(), LOAD_TOTALINIT)
        };
        if !loaded {
            return Err(format!("Stereo Tool refused {}", preset.display()));
        }
        self.preset = Some(preset.to_owned());
        Ok(())
    }

    pub fn preset(&self) -> Option<&Path> {
        self.preset.as_deref()
    }

    /// Does the licence cover what it is running?
    pub fn licensed(&self) -> bool {
        // SAFETY: the handle is ours and alive.
        unsafe { (self.handle.api.check_license)(self.handle.ptr) }
    }

    /// The features running without a licence, as the library names them.
    pub fn unlicensed_features(&self) -> Option<String> {
        let mut buffer = vec![0 as c_char; 1024];
        // SAFETY: the buffer is ours, and its length is what the call is
        // told; the library writes a zero-terminated string into it.
        let written = unsafe {
            (self.handle.api.unlicensed)(
                self.handle.ptr,
                buffer.as_mut_ptr(),
                buffer.len() as c_int,
            )
        };
        if !written {
            return None;
        }
        // SAFETY: the buffer was zeroed, so there is a terminator whatever
        // the library wrote.
        let text = unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        (!text.is_empty()).then_some(text)
    }

    /// The delay the processor adds, in frames.
    pub fn latency(&self) -> usize {
        // SAFETY: the handle is ours; `false` says the mixer does not feed
        // it silence to flush the delay.
        let frames = unsafe { (self.handle.api.latency)(self.handle.ptr, SAMPLE_RATE, false) };
        frames.max(0) as usize
    }

    pub fn name(&self) -> &str {
        "Stereo Tool"
    }

    /// Run one block through the processor, in place.
    ///
    /// The library takes interleaved frames, so the planar buffers the graph
    /// hands over are woven into the scratch block and back out again.
    pub fn process(&mut self, channels: &mut [&mut [f32]]) -> Result<(), ProcessError> {
        let frames = channels.first().map_or(0, |channel| channel.len());
        if frames == 0 || frames > self.max_block || channels.len() < CHANNELS {
            return Err(ProcessError::BlockSize);
        }
        if channels.iter().any(|channel| channel.len() != frames) {
            return Err(ProcessError::BlockSize);
        }

        for frame in 0..frames {
            for (channel, buffer) in channels.iter().enumerate().take(CHANNELS) {
                self.scratch[frame * CHANNELS + channel] = buffer[frame];
            }
        }
        // SAFETY: the block is ours and holds exactly `frames * CHANNELS`
        // floats, which is what the counts say.
        unsafe {
            (self.handle.api.process)(
                self.handle.ptr,
                self.scratch.as_mut_ptr(),
                frames as c_int,
                CHANNELS as c_int,
                SAMPLE_RATE,
            );
        }
        for frame in 0..frames {
            for (channel, buffer) in channels.iter_mut().enumerate().take(CHANNELS) {
                buffer[frame] = self.scratch[frame * CHANNELS + channel];
            }
        }
        Ok(())
    }
}

impl Instance {
    /// The processor itself, to hold on to for as long as its window is up.
    pub fn handle(&self) -> Arc<Handle> {
        self.handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_library_is_taken_for_it() {
        assert!(is_library("libStereoTool_intel64.so"));
        assert!(is_library("libStereoTool_pi2.so"));
        assert!(!is_library("libStereoTool_intel64.so.bak"));
        assert!(!is_library("StereoTool.vst3"));
        assert!(!is_library("libsomethingelse.so"));
        // The download carries the Kantar build alongside, under the same
        // name in a directory of its own.
        assert!(!is_library("libKantarPlugin300.so"));
    }

    #[test]
    fn a_build_for_another_machine_is_not_offered() {
        let mine = match std::env::consts::ARCH {
            "x86_64" => "libStereoTool_intel64.so",
            "x86" => "libStereoTool_intel32.so",
            "aarch64" => "libStereoTool_arm64.so",
            "arm" => "libStereoTool_arm32.so",
            // Nothing is ruled out on an architecture the vendor does not
            // name, so there is nothing to check.
            _ => return,
        };
        assert!(is_for_this_machine(mine));
        for other in [
            "libStereoTool_intel64.so",
            "libStereoTool_intel32.so",
            "libStereoTool_arm64.so",
            "libStereoTool_arm32.so",
        ] {
            if other != mine {
                assert!(!is_for_this_machine(other), "{other} is not for us");
            }
        }
    }

    #[test]
    fn the_library_is_looked_for_where_it_was_put() {
        let directory = std::env::temp_dir().join(format!("pipedeck-st-{}", std::process::id()));
        std::fs::remove_dir_all(&directory).ok();
        std::fs::create_dir_all(&directory).expect("a temp directory");
        assert!(choose(&directory).is_empty());

        let Some(preferred) = preferred_names().first() else {
            std::fs::remove_dir_all(&directory).ok();
            return;
        };
        // A name the vendor has not used, but for this machine, is still
        // worth trying.
        let odd = directory.join(format!(
            "libStereoTool_whatever_{}.so",
            preferred
                .trim_start_matches("libStereoTool_")
                .replace(".so", "")
        ));
        std::fs::write(&odd, b"not really a library").expect("a file");
        assert_eq!(choose(&directory), vec![odd.clone()]);

        // With both, the one the vendor documents comes first.
        let mine = directory.join(preferred);
        std::fs::write(&mine, b"not really a library either").expect("a file");
        assert_eq!(choose(&directory), vec![mine, odd]);

        std::fs::remove_dir_all(&directory).ok();
    }
}
