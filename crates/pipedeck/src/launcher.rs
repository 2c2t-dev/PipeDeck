//! Pipedeck's own place on the desktop: its icon, and the desktop entry
//! that puts it in the launcher and gives its window that icon in the task
//! bar, which on Wayland is found by the application's id.
//!
//! A package would install these; a build run from where it was built
//! installs them itself, in the user's own folders, and keeps them pointing
//! at the program that runs.

use std::path::{Path, PathBuf};

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

/// The desktop entry, for the program at `binary`.
fn entry(app_id: &str, binary: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Pipedeck\n\
         GenericName=Audio mixer\n\
         Comment=Mix what plays into mixes to hear, record and stream\n\
         Exec={}\n\
         Icon={app_id}\n\
         Terminal=false\n\
         Categories=AudioVideo;Audio;Mixer;\n\
         Keywords=mixer;audio;pipewire;stream deck;\n\
         StartupNotify=true\n",
        binary.display()
    )
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

/// Install the icon and the desktop entry, or bring them up to date.
pub fn install(app_id: &str) {
    let (Some(data), Ok(binary)) = (data_dir(), std::env::current_exe()) else {
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
