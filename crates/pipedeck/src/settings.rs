//! What the interface remembers about itself.
//!
//! The mixer's own state belongs to the engine, which owns its config file.
//! These are the choices the window makes for itself, kept beside it so the
//! two never write to the same file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How the window follows the desktop's light or dark setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "Same as system",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub theme: Theme,
    /// Whether a desktop entry starts the mixer with the session.
    #[serde(default)]
    pub start_at_login: bool,
}

impl Settings {
    fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("pipedeck").join("interface.toml"))
    }

    /// Read the settings, falling back to the defaults for anything the file
    /// does not say, or does not exist to say.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("cannot read {}: {e}", path.display());
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log::warn!("cannot read {}: {e}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let Ok(text) = toml::to_string_pretty(self) else {
            return;
        };
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                log::warn!("cannot write {}: {e}", path.display());
                return;
            }
        }
        if let Err(e) = std::fs::write(&path, text) {
            log::warn!("cannot write {}: {e}", path.display());
        }
    }
}

/// The desktop entry that starts the mixer with the session.
fn autostart_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("autostart").join("pipedeck.desktop"))
}

/// Is the mixer set to start with the session?
pub fn starts_at_login() -> bool {
    autostart_path().is_some_and(|path| path.exists())
}

/// Write or remove that desktop entry.
///
/// It points at the running binary rather than at an installed name, so it
/// works for a build that was never installed.
pub fn set_start_at_login(enabled: bool) -> std::io::Result<()> {
    let Some(path) = autostart_path() else {
        return Ok(());
    };
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    }

    let binary = std::env::current_exe()?;
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Pipedeck\n\
         Comment=PipeWire mixer\n\
         Exec={}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        binary.display()
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, entry)
}

/// Where a plug-in a user installs for themselves belongs.
pub fn user_plugin_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".vst3"))
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

/// Copy a directory and everything under it.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Put a plug-in where the mixer will find it.
///
/// A VST3 on Linux is a directory, not a file: `Name.vst3/Contents/<arch>/
/// Name.so`. Both forms are accepted here, a bundle as it stands and a bare
/// shared object, which is wrapped in the layout it is missing. Whichever it
/// was, it lands in the user's own plug-in directory.
pub fn import_plugin(source: &Path) -> Result<PathBuf, String> {
    let directory = user_plugin_dir().ok_or("no home directory to install into")?;
    import_plugin_into(source, &directory)
}

/// The same, into a directory of the caller's choosing.
pub fn import_plugin_into(source: &Path, directory: &Path) -> Result<PathBuf, String> {
    let name = source
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("that file has no usable name")?
        .to_owned();

    let bundle = directory.join(format!("{name}.vst3"));
    if bundle.exists() {
        return Err(format!("{name} is already installed"));
    }

    if source.is_dir() {
        if source.extension().and_then(|e| e.to_str()) != Some("vst3") {
            return Err("a bundle directory has to be named something.vst3".into());
        }
        copy_tree(source, &bundle).map_err(|e| format!("cannot copy the bundle: {e}"))?;
    } else {
        if source.extension().and_then(|e| e.to_str()) != Some("so") {
            return Err("that is neither a .vst3 bundle nor a .so".into());
        }
        let inside = bundle.join("Contents").join(architecture());
        std::fs::create_dir_all(&inside).map_err(|e| format!("cannot make the bundle: {e}"))?;
        std::fs::copy(source, inside.join(format!("{name}.so")))
            .map_err(|e| format!("cannot copy the plug-in: {e}"))?;
    }

    // A bundle with no binary for this machine would be found and refused
    // every time the mixer starts, so it is better not to keep it.
    let binary = bundle
        .join("Contents")
        .join(architecture())
        .join(format!("{name}.so"));
    if !binary.exists() {
        let _ = std::fs::remove_dir_all(&bundle);
        return Err(format!("{name} has nothing for {}", std::env::consts::ARCH));
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of our own, so a test installs nothing on the machine.
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pipedeck-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_bare_shared_object_is_wrapped_in_a_bundle() {
        let home = scratch("import-so");
        let loose = home.join("Thing.so");
        std::fs::write(&loose, b"not really a plugin").unwrap();
        let into = home.join("plugins");

        let bundle = import_plugin_into(&loose, &into).expect("it installs");
        assert_eq!(bundle, into.join("Thing.vst3"));
        assert!(bundle
            .join("Contents")
            .join(architecture())
            .join("Thing.so")
            .exists());

        // Twice is refused rather than silently overwritten.
        assert!(import_plugin_into(&loose, &into).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_bundle_is_copied_whole() {
        let home = scratch("import-bundle");
        let source = home.join("Reverb.vst3");
        let inside = source.join("Contents").join(architecture());
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("Reverb.so"), b"not really a plugin").unwrap();
        std::fs::write(source.join("moduleinfo.json"), b"{}").unwrap();
        let into = home.join("plugins");

        let bundle = import_plugin_into(&source, &into).expect("it installs");
        assert!(bundle.join("moduleinfo.json").exists(), "the whole tree");
        assert!(bundle
            .join("Contents")
            .join(architecture())
            .join("Reverb.so")
            .exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn something_else_is_refused_and_leaves_nothing_behind() {
        let home = scratch("import-junk");
        let junk = home.join("notes.txt");
        std::fs::write(&junk, b"hello").unwrap();
        let into = home.join("plugins");

        assert!(import_plugin_into(&junk, &into).is_err());
        assert!(!into.join("notes.vst3").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_bundle_with_nothing_for_this_machine_is_not_kept() {
        let home = scratch("import-foreign");
        let source = home.join("Foreign.vst3");
        std::fs::create_dir_all(source.join("Contents").join("x86_64-win")).unwrap();
        std::fs::write(
            source
                .join("Contents")
                .join("x86_64-win")
                .join("Foreign.vst3"),
            b"windows",
        )
        .unwrap();
        let into = home.join("plugins");

        assert!(import_plugin_into(&source, &into).is_err());
        assert!(!into.join("Foreign.vst3").exists(), "nothing left behind");
        let _ = std::fs::remove_dir_all(&home);
    }
}
