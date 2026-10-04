//! Pipedeck's own place on the desktop: its icon, and the desktop entry
//! that puts it in the launcher and gives its window that icon in the task
//! bar, which on Wayland is found by the application's id.
//!
//! A package installs these; a build run from where it was built, or an
//! AppImage, installs them itself, in the user's own folders, and keeps them
//! pointing at the program that runs.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Reverse-DNS id of 2c2t.dev. GApplication (like D-Bus) forbids an element
/// starting with a digit, so the `2c2t` component takes the conventional
/// leading underscore. Changing this later also renames the .desktop file,
/// the GSettings path and the Flatpak sandbox, so it is meant to be stable.
pub const APP_ID: &str = "dev._2c2t.Pipedeck";

const ENTRY: &str = include_str!("../data/dev._2c2t.Pipedeck.desktop");
const ICON: &str = include_str!("../data/dev._2c2t.Pipedeck.svg");
const SYMBOLIC: &str = include_str!("../data/dev._2c2t.Pipedeck-symbolic.svg");

/// The icon drawn ahead at fixed sizes. GTK gives the desktop a window's
/// icon as pictures, and draws a scalable one at 48 pixels only, which the
/// task switcher then blows up; with these, it hands over each size and the
/// desktop takes the one it shows at. Larger ones are left to the scalable
/// icon, which is what the theme's larger folders hold.
const SIZES: [(u32, &[u8]); 7] = [
    (16, include_bytes!("../data/icons/16.png")),
    (24, include_bytes!("../data/icons/24.png")),
    (32, include_bytes!("../data/icons/32.png")),
    (48, include_bytes!("../data/icons/48.png")),
    (64, include_bytes!("../data/icons/64.png")),
    (96, include_bytes!("../data/icons/96.png")),
    (128, include_bytes!("../data/icons/128.png")),
];

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

/// The program a desktop entry starts: the AppImage when this runs from
/// one, since what runs is unpacked somewhere new each time, or else this
/// program.
pub fn program() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
}

/// `binary` as the `Exec` key of a desktop entry has it: quoted when it
/// holds what the key would read otherwise.
pub fn exec(binary: &Path) -> String {
    let path = binary.display().to_string();
    if !path.contains(|c: char| c.is_whitespace() || "\"'\\`$;&|<>()*?#~".contains(c)) {
        return path;
    }
    let mut quoted = String::from('"');
    for c in path.chars() {
        if "\"`$\\".contains(c) {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// The desktop entry, for the program at `binary`.
fn entry(app_id: &str, binary: &Path) -> String {
    ENTRY
        .lines()
        .map(|line| match line.split_once('=') {
            Some(("Exec", _)) => format!("Exec={}\n", exec(binary)),
            Some(("Icon", _)) => format!("Icon={app_id}\n"),
            _ => format!("{line}\n"),
        })
        .collect()
}

/// What an AppImage's start sets for the GTK it carries.
const APPIMAGE_ONLY: [&str; 10] = [
    "APPDIR",
    "APPIMAGE",
    "ARGV0",
    "OWD",
    "GTK_DATA_PREFIX",
    "GSETTINGS_SCHEMA_DIR",
    "GI_TYPELIB_PATH",
    "GTK_EXE_PREFIX",
    "GTK_PATH",
    "GDK_PIXBUF_MODULE_FILE",
];

/// A command for one of the system's programs, OpenDeck or `git`, left
/// without what the AppImage this may run from set for its own GTK: the
/// program has the system's, and one reading `APPIMAGE` would take itself
/// for an AppImage.
pub fn host_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    let Some(appdir) = std::env::var_os("APPDIR").filter(|v| !v.is_empty()) else {
        return command;
    };
    for name in APPIMAGE_ONLY {
        command.env_remove(name);
    }
    if let Some(dirs) = std::env::var_os("XDG_DATA_DIRS") {
        match outside(&dirs, Path::new(&appdir)) {
            Some(kept) => command.env("XDG_DATA_DIRS", kept),
            None => command.env_remove("XDG_DATA_DIRS"),
        };
    }
    command
}

/// The folders of a search path that are not in `appdir`.
fn outside(dirs: &OsStr, appdir: &Path) -> Option<OsString> {
    let kept: Vec<PathBuf> = std::env::split_paths(dirs)
        .filter(|dir| !dir.as_os_str().is_empty() && !dir.starts_with(appdir))
        .collect();
    if kept.is_empty() {
        return None;
    }
    std::env::join_paths(kept).ok()
}

/// Whether a package put the desktop entry among the system's, which it
/// then keeps up itself.
fn packaged(app_id: &str) -> bool {
    if std::env::var_os("APPIMAGE").is_some() {
        return false;
    }
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    std::env::split_paths(&dirs)
        .any(|dir| dir.join(format!("applications/{app_id}.desktop")).is_file())
}

/// Write a file unless it already holds that.
fn write(path: &Path, text: impl AsRef<[u8]>) -> std::io::Result<()> {
    let text = text.as_ref();
    if std::fs::read(path).is_ok_and(|kept| kept == text) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

/// Install the icon and the desktop entry, or bring them up to date,
/// unless a package did.
pub fn install(app_id: &str) {
    if packaged(app_id) {
        return;
    }
    let (Some(data), Some(binary)) = (data_dir(), program()) else {
        return;
    };
    let icons = data.join("icons/hicolor");
    let written = write(&icons.join(format!("scalable/apps/{app_id}.svg")), ICON)
        .and_then(|()| {
            write(
                &icons.join(format!("symbolic/apps/{app_id}-symbolic.svg")),
                SYMBOLIC,
            )
        })
        .and_then(|()| {
            write(
                &data.join(format!("applications/{app_id}.desktop")),
                entry(app_id, &binary),
            )
        })
        .and_then(|()| {
            SIZES.iter().try_for_each(|(size, png)| {
                write(&icons.join(format!("{size}x{size}/apps/{app_id}.png")), png)
            })
        });
    if let Err(e) = written {
        log::warn!("cannot put Pipedeck in the launcher: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_starts_this_program_under_its_own_icon() {
        let entry = entry("dev._2c2t.Pipedeck", Path::new("/opt/pipedeck/pipedeck"));
        assert!(entry.contains("\nExec=/opt/pipedeck/pipedeck\n"));
        assert!(entry.contains("\nIcon=dev._2c2t.Pipedeck\n"));
        assert!(entry.starts_with("[Desktop Entry]\n"));
    }

    #[test]
    fn the_appimage_folders_are_left_out_of_a_search_path() {
        let appdir = Path::new("/tmp/.mount_PipedeXYZ");
        assert_eq!(
            outside(
                OsStr::new("/tmp/.mount_PipedeXYZ/usr/share:/usr/share:/usr/local/share:"),
                appdir
            ),
            Some(OsString::from("/usr/share:/usr/local/share"))
        );
        assert_eq!(
            outside(OsStr::new("/tmp/.mount_PipedeXYZ/usr/share"), appdir),
            None
        );
    }

    #[test]
    fn a_path_with_spaces_is_quoted() {
        assert_eq!(
            exec(Path::new("/home/me/My Apps/Pipedeck.AppImage")),
            "\"/home/me/My Apps/Pipedeck.AppImage\""
        );
        assert_eq!(exec(Path::new("/opt/a$b")), "\"/opt/a\\$b\"");
    }

    #[test]
    fn the_icons_are_carried_whole() {
        assert!(ICON.starts_with("<svg") && ICON.trim_end().ends_with("</svg>"));
        assert!(SYMBOLIC.contains("viewBox=\"0 0 16 16\""));
        for (size, png) in SIZES {
            // The width and height of a PNG's header.
            assert_eq!(png[16..20], size.to_be_bytes());
            assert_eq!(png[20..24], size.to_be_bytes());
        }
    }
}
