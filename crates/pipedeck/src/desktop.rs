//! Reading the desktop entries of installed applications.
//!
//! The picker offers applications that are not playing yet, so it cannot get
//! them from the graph. It reads the XDG desktop entries instead, which is
//! where a system keeps the name, the icon and the command of everything it
//! can launch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One installed application, as the picker shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopApp {
    /// The key an assignment matches on, which must line up with what
    /// PipeWire reports for the streams this application opens: the basename
    /// of its executable.
    pub key: String,
    pub name: String,
    /// Icon name, or an absolute path when the entry gives one.
    pub icon: Option<String>,
}

/// Directories holding desktop entries, following the XDG base directory
/// spec, most specific first so a user override wins.
fn entry_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    {
        dirs.push(home.join("applications"));
    }
    let system =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    for dir in system.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(dir).join("applications"));
    }
    dirs
}

/// Turn an `Exec=` line into the key PipeWire will report.
///
/// Field codes such as `%U` are dropped, as is a leading `env VAR=value`.
/// A Flatpak launcher runs the application from its own binary, so the
/// application id is the closer match.
fn key_from_exec(exec: &str) -> Option<String> {
    let mut words = exec
        .split_whitespace()
        .filter(|word| !word.starts_with('%'))
        .peekable();
    let mut first = words.next()?;
    if first.ends_with("env") {
        while words.peek().is_some_and(|word| word.contains('=')) {
            words.next();
        }
        first = words.next()?;
    }
    let binary = Path::new(first).file_name()?.to_str()?;
    if binary == "flatpak" {
        if let Some(id) = words.find(|word| word.contains('.') && !word.starts_with('-')) {
            return Some(id.to_owned());
        }
    }
    Some(binary.to_owned())
}

/// Parse one desktop entry, keeping only applications meant to be shown.
fn parse(path: &Path) -> Option<DesktopApp> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Only the first group describes the application itself; the
            // rest are its actions.
            in_entry = line == "[Desktop Entry]";
            if !in_entry && !fields.is_empty() {
                break;
            }
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            // Localised keys look like Name[fr]; the plain one is enough.
            fields.entry(key.trim()).or_insert_with(|| value.trim());
        }
    }

    if fields.get("Type") != Some(&"Application") {
        return None;
    }
    if fields.get("NoDisplay") == Some(&"true") || fields.get("Hidden") == Some(&"true") {
        return None;
    }
    let name = fields.get("Name")?.to_string();
    let key = key_from_exec(fields.get("Exec")?)?;
    Some(DesktopApp {
        key,
        name,
        icon: fields.get("Icon").map(|icon| icon.to_string()),
    })
}

/// How well an entry represents its key.
///
/// Several entries can share one: every Steam game launches `steam`, and
/// only steam.desktop is the application behind that key. The file whose
/// name matches the key wins, which is the convention desktop entries follow.
fn fitness(path: &Path, key: &str) -> u8 {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if stem.eq_ignore_ascii_case(key) {
        2
    } else if stem.to_lowercase().starts_with(&key.to_lowercase()) {
        1
    } else {
        0
    }
}

/// Every installed application, sorted by name, one entry per key.
pub fn installed() -> Vec<DesktopApp> {
    let mut apps: HashMap<String, (u8, DesktopApp)> = HashMap::new();
    for dir in entry_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "desktop") {
                continue;
            }
            let Some(app) = parse(&path) else {
                continue;
            };
            let score = fitness(&path, &app.key);
            match apps.get(&app.key) {
                Some((best, _)) if *best >= score => {}
                _ => {
                    apps.insert(app.key.clone(), (score, app));
                }
            }
        }
    }
    let apps: HashMap<String, DesktopApp> =
        apps.into_iter().map(|(key, (_, app))| (key, app)).collect();
    let mut apps: Vec<DesktopApp> = apps.into_values().collect();
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_named_after_the_key_wins() {
        assert_eq!(fitness(Path::new("/x/steam.desktop"), "steam"), 2);
        assert_eq!(fitness(Path::new("/x/steam-native.desktop"), "steam"), 1);
        assert_eq!(fitness(Path::new("/x/cs2.desktop"), "steam"), 0);
    }

    #[test]
    fn exec_lines_reduce_to_a_binary() {
        assert_eq!(key_from_exec("mpv --gui -- %U").as_deref(), Some("mpv"));
        assert_eq!(key_from_exec("/usr/bin/vlc %F").as_deref(), Some("vlc"));
        assert_eq!(
            key_from_exec("env GDK_BACKEND=x11 /usr/bin/brave %U").as_deref(),
            Some("brave")
        );
        assert_eq!(
            key_from_exec("/usr/bin/flatpak run --branch=stable com.spotify.Client").as_deref(),
            Some("com.spotify.Client")
        );
        assert_eq!(key_from_exec("%U").as_deref(), None);
    }
}
