//! What the interface remembers about itself.
//!
//! The mixer's own state belongs to the engine, which owns its config file.
//! These are the choices the window makes for itself, kept beside it so the
//! two never write to the same file.

use std::path::PathBuf;

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
