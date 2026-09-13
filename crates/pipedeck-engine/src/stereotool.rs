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

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
/// The download holds one build per machine and, for each, a plain one and
/// two saying what they do about X11. The plain one is what the vendor
/// documents; the others are there because a machine without X11 libraries
/// cannot load a build that wants them, and then the next one down works.
fn preferred_names() -> &'static [&'static str] {
    match std::env::consts::ARCH {
        "x86_64" => &[
            "libStereoTool_intel64.so",
            "libStereoTool_noX11_intel64.so",
            "libStereoToolX11_intel64.so",
        ],
        "x86" => &[
            "libStereoTool_intel32.so",
            "libStereoTool_noX11_intel32.so",
            "libStereoToolX11_intel32.so",
        ],
        "aarch64" => &[
            "libStereoTool_arm64.so",
            "libStereoTool_noX11_arm64.so",
            "libStereoTool_pi4_64.so",
        ],
        "arm" => &[
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
    delete: unsafe extern "C" fn(*mut c_void),
    process: unsafe extern "C" fn(*mut c_void, *mut f32, c_int, c_int, c_int),
    latency: unsafe extern "C" fn(*mut c_void, c_int, bool) -> c_int,
    load_preset: unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> bool,
    check_license: unsafe extern "C" fn(*mut c_void) -> bool,
    unlicensed: unsafe extern "C" fn(*mut c_void, *mut c_char, c_int) -> bool,
    software_version: unsafe extern "C" fn() -> c_int,
    path: PathBuf,
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
    let library = libloading::Library::new(path).map_err(|e| e.to_string())?;
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
    })
}

/// Why a block did not go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProcessError {
    #[error(
        "the buffers are empty, uneven, or longer than the block the processor was opened for"
    )]
    BlockSize,
}

/// One Stereo Tool processor, ready to take blocks.
///
/// Dropping it releases the processor; the library stays loaded.
pub struct Instance {
    handle: *mut c_void,
    api: &'static Api,
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
        let handle =
            unsafe { (api.create)(key.as_ref().map_or(std::ptr::null(), |key| key.as_ptr())) };
        if handle.is_null() {
            return Err("Stereo Tool refused to start".into());
        }
        let mut instance = Self {
            handle,
            api,
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
        let loaded = unsafe { (self.api.load_preset)(self.handle, path.as_ptr(), LOAD_TOTALINIT) };
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
        unsafe { (self.api.check_license)(self.handle) }
    }

    /// The features running without a licence, as the library names them.
    pub fn unlicensed_features(&self) -> Option<String> {
        let mut buffer = vec![0 as c_char; 1024];
        // SAFETY: the buffer is ours, and its length is what the call is
        // told; the library writes a zero-terminated string into it.
        let written = unsafe {
            (self.api.unlicensed)(self.handle, buffer.as_mut_ptr(), buffer.len() as c_int)
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
        let frames = unsafe { (self.api.latency)(self.handle, SAMPLE_RATE, false) };
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
            (self.api.process)(
                self.handle,
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

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the handle came from `create` and is released once.
        unsafe { (self.api.delete)(self.handle) };
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
