//! Finding the VST3 plug-ins installed on the machine.
//!
//! A VST3 plug-in is a bundle: a directory named `Something.vst3` holding a
//! shared object under `Contents/<architecture>/`. Opening it means loading
//! that object, calling its module entry point, asking it for its factory,
//! and reading the classes the factory advertises. Only the audio effects
//! interest a mixer.
//!
//! This module answers "what is installed"; running one is another matter.

use std::ffi::{c_void, CStr, OsStr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vst3::Steinberg::{IPluginFactory, IPluginFactoryTrait, PClassInfo, PFactoryInfo};
use vst3::{ComPtr, Steinberg::kResultOk};

/// A plug-in a channel can be asked to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plugin {
    /// The bundle it lives in, which is what identifies it on disk.
    pub bundle: PathBuf,
    /// The class inside that bundle, by the id the factory gives it.
    pub class_id: String,
    pub name: String,
    pub vendor: String,
    /// What the plug-in calls itself: "Fx", "Fx|Reverb", "Instrument"...
    pub category: String,
}

impl Plugin {
    /// Does it process audio rather than generate it?
    ///
    /// A mixer has nothing to do with instruments: they make sound from
    /// notes, and a channel has none to give them.
    pub fn is_effect(&self) -> bool {
        self.category.starts_with("Audio Module Class")
            || self.category.contains("Fx")
            || self.category == "Audio Effect"
    }
}

/// Where VST3 bundles live, most specific first.
///
/// These are the paths the specification names, plus `VST3_PATH`, which is
/// how a user points at bundles kept somewhere else.
pub fn search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(extra) = std::env::var_os("VST3_PATH") {
        paths.extend(std::env::split_paths(&extra));
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(&home).join(".vst3"));
    }
    paths.push(PathBuf::from("/usr/local/lib/vst3"));
    paths.push(PathBuf::from("/usr/lib/vst3"));
    paths
}

/// The architecture directory inside a bundle, as the specification spells
/// it for Linux.
fn architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64-linux",
        "aarch64" => "aarch64-linux",
        "x86" => "i386-linux",
        other => other,
    }
}

/// The shared object inside a bundle, if it has one for this machine.
fn binary(bundle: &Path) -> Option<PathBuf> {
    let name = bundle.file_stem()?.to_str()?;
    let candidate = bundle
        .join("Contents")
        .join(architecture())
        .join(format!("{name}.so"));
    candidate.exists().then_some(candidate)
}

/// Every bundle under the search paths.
pub fn bundles() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for directory in search_paths() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() == Some(OsStr::new("vst3")) && binary(&path).is_some() {
                found.push(path);
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Load a bundle and hand back its factory.
///
/// # Safety notes
///
/// Loading a plug-in runs its code: its module entry point, its factory, and
/// whatever it does at load time. The library is leaked rather than closed,
/// because a plug-in that registered anything with the process, static
/// objects included, would take it down with it on unload; hosts keep them
/// loaded for the life of the process for that reason.
pub(crate) fn module_factory(bundle: &Path) -> Result<ComPtr<IPluginFactory>, String> {
    let binary = binary(bundle).ok_or("no binary for this architecture")?;
    // SAFETY: loading is the plug-in's own code; there is no way to host one
    // without running it.
    let library = unsafe { libloading::Library::new(&binary) }.map_err(|e| e.to_string())?;
    unsafe {
        let entry: libloading::Symbol<unsafe extern "C" fn(*mut c_void) -> bool> = library
            .get(b"ModuleEntry\0")
            .map_err(|_| "no ModuleEntry".to_string())?;
        if !entry(std::ptr::null_mut()) {
            return Err("the module refused to start".into());
        }
        let get_factory: libloading::Symbol<unsafe extern "C" fn() -> *mut IPluginFactory> =
            library
                .get(b"GetPluginFactory\0")
                .map_err(|_| "no GetPluginFactory".to_string())?;
        let factory = ComPtr::from_raw(get_factory()).ok_or("the factory is null")?;
        // Kept loaded on purpose: see the note above.
        std::mem::forget(library);
        Ok(factory)
    }
}

/// The classes one bundle offers.
fn read_bundle(bundle: &Path) -> Result<Vec<Plugin>, String> {
    let factory = module_factory(bundle)?;

    // SAFETY: the factory is alive, and every out parameter is a struct the
    // API fills in full.
    unsafe {
        let mut info = std::mem::zeroed::<PFactoryInfo>();
        let vendor = if factory.getFactoryInfo(&mut info) == kResultOk {
            read_string(&info.vendor)
        } else {
            String::new()
        };

        let mut plugins = Vec::new();
        for index in 0..factory.countClasses() {
            let mut class = std::mem::zeroed::<PClassInfo>();
            if factory.getClassInfo(index, &mut class) != kResultOk {
                continue;
            }
            plugins.push(Plugin {
                bundle: bundle.to_owned(),
                class_id: class
                    .cid
                    .iter()
                    .map(|byte| format!("{:02x}", *byte as u8))
                    .collect::<String>(),
                name: read_string(&class.name),
                vendor: vendor.clone(),
                category: read_string(&class.category),
            });
        }
        Ok(plugins)
    }
}

/// Read one of the fixed-size, NUL-padded strings the API is made of.
fn read_string(raw: &[std::os::raw::c_char]) -> String {
    // SAFETY: the API guarantees a NUL within the array.
    let bytes = unsafe { CStr::from_ptr(raw.as_ptr()) };
    bytes.to_string_lossy().trim().to_owned()
}

/// Every audio effect installed on this machine.
///
/// A bundle that refuses to load is reported and skipped: one broken plug-in
/// must not cost the user the others.
pub fn installed() -> Vec<Plugin> {
    let mut plugins = Vec::new();
    for bundle in bundles() {
        match read_bundle(&bundle) {
            Ok(found) => plugins.extend(found.into_iter().filter(Plugin::is_effect)),
            Err(e) => log::warn!("cannot read {}: {e}", bundle.display()),
        }
    }
    plugins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_is_found_by_its_binary() {
        let dir = std::env::temp_dir().join(format!("pipedeck-vst3-{}", std::process::id()));
        let bundle = dir.join("Thing.vst3");
        let binary_path = bundle.join("Contents").join(architecture());
        std::fs::create_dir_all(&binary_path).unwrap();
        assert!(binary(&bundle).is_none(), "no binary yet");
        std::fs::write(binary_path.join("Thing.so"), b"not really a plugin").unwrap();
        assert_eq!(binary(&bundle), Some(binary_path.join("Thing.so")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn instruments_are_not_offered_to_a_mixer() {
        let effect = Plugin {
            bundle: PathBuf::new(),
            class_id: String::new(),
            name: "Reverb".into(),
            vendor: String::new(),
            category: "Audio Module Class|Fx|Reverb".into(),
        };
        assert!(effect.is_effect());
        let instrument = Plugin {
            category: "Instrument|Synth".into(),
            ..effect.clone()
        };
        assert!(!instrument.is_effect());
    }
}
