//! What the interface remembers about itself.
//!
//! The mixer's own state belongs to the engine, which owns its config file.
//! These are the choices the window makes for itself, kept beside it so the
//! two never write to the same file.

use std::path::{Path, PathBuf};

use pipedeck_engine::stereotool;
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

    if source.is_dir() && source.extension().and_then(|e| e.to_str()) != Some("vst3") {
        return Err("a bundle directory has to be named something.vst3".into());
    }
    if !source.is_dir() && source.extension().and_then(|e| e.to_str()) != Some("so") {
        return Err("that is neither a .vst3 bundle nor a .so".into());
    }
    let copied = if source.is_dir() {
        copy_tree(source, &bundle).map_err(|e| format!("cannot copy the bundle: {e}"))
    } else {
        let inside = bundle.join("Contents").join(architecture());
        std::fs::create_dir_all(&inside)
            .map_err(|e| format!("cannot make the bundle: {e}"))
            .and_then(|()| {
                std::fs::copy(source, inside.join(format!("{name}.so")))
                    .map(|_| ())
                    .map_err(|e| format!("cannot copy the plug-in: {e}"))
            })
    };
    // Half a bundle left behind would be found broken on every scan, and
    // would have the next try told it is already installed.
    if let Err(e) = copied {
        let _ = std::fs::remove_dir_all(&bundle);
        return Err(e);
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

/// Put Stereo Tool where the mixer will find it.
///
/// Thimeo ships it as an archive holding one shared library per machine, and
/// nothing of it is redistributable, so the user downloads it and points at
/// what they got: the `.zip` as it came, or a `libStereoTool*.so` taken out
/// of it. Only the builds for this machine are kept — half a gigabyte of
/// other architectures is not worth copying — under the names they came
/// with, flat, since that is where the engine looks.
pub fn import_stereotool(source: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = stereotool::library_dir().ok_or("no home directory to install into")?;
    import_stereotool_into(source, &directory)
}

/// The same, into a directory of the caller's choosing.
pub fn import_stereotool_into(source: &Path, directory: &Path) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(directory)
        .map_err(|e| format!("cannot make {}: {e}", directory.display()))?;

    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("that file has no usable name")?;

    if stereotool::is_library(name) {
        let target = directory.join(name);
        replace(
            &target,
            &std::fs::read(source).map_err(|e| format!("cannot read it: {e}"))?,
        )?;
        return Ok(vec![target]);
    }
    if source.extension().and_then(|e| e.to_str()) != Some("zip") {
        return Err("that is neither the Stereo Tool archive nor a libStereoTool .so".into());
    }

    let file = std::fs::File::open(source).map_err(|e| format!("cannot read it: {e}"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("cannot read the archive: {e}"))?;
    let mut installed = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("cannot read the archive: {e}"))?;
        if !entry.is_file() {
            continue;
        }
        let inside = entry.name().to_owned();
        // Only the base name is used, so nothing in the archive can name a
        // path of its own choosing. This archive is built on Windows and
        // separates with backslashes, which no Unix path parser cuts on, so
        // both separators are cut here.
        let name = basename(&inside);
        if !stereotool::is_library(name) || !stereotool::is_for_this_machine(name) {
            continue;
        }
        // The download carries the Kantar build of the same name beside the
        // plain one. It is a different product, with a licence of its own,
        // and taking it would silently shadow the one that was asked for.
        if inside
            .split(['/', '\\'])
            .any(|part| part.eq_ignore_ascii_case("kantar"))
        {
            continue;
        }
        let target = directory.join(name);
        if installed.contains(&target) {
            continue;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        std::io::copy(&mut entry, &mut bytes).map_err(|e| format!("cannot unpack {name}: {e}"))?;
        replace(&target, &bytes)?;
        installed.push(target);
    }
    if installed.is_empty() {
        return Err(format!(
            "that archive holds no libStereoTool library for {}",
            std::env::consts::ARCH
        ));
    }
    installed.sort();
    Ok(installed)
}

/// The file name of an archive entry, whichever separator it was written
/// with. A zip made on Windows carries backslashes.
fn basename(entry: &str) -> &str {
    entry.rsplit(['/', '\\']).next().unwrap_or(entry)
}

/// Write a library, taking the old one out of the way first.
///
/// A file that is already loaded must not be written over: the copy in
/// memory is that very file. Unlinking it leaves whoever loaded it with the
/// old one and puts a new file in its place, which is what an upgrade while
/// the mixer runs needs.
fn replace(target: &Path, bytes: &[u8]) -> Result<(), String> {
    let _ = std::fs::remove_file(target);
    std::fs::write(target, bytes).map_err(|e| format!("cannot write {}: {e}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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

    /// The download as Thimeo builds it: made on Windows, so the entries are
    /// separated with backslashes, every machine's build side by side, and
    /// the Kantar edition under the same names in a directory of its own.
    fn thimeo_archive(path: &Path, arch: &str) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for entry in [
            format!("libStereoTool_1105\\lib\\Linux\\A\\libStereoTool_{arch}.so"),
            format!("libStereoTool_1105\\lib\\Linux\\A\\libStereoTool_noX11_{arch}.so"),
            format!("libStereoTool_1105\\lib\\Linux\\A\\Kantar\\libStereoTool_{arch}.so"),
            "libStereoTool_1105\\lib\\Linux\\A\\Kantar\\libKantarPlugin300.so".to_owned(),
            "libStereoTool_1105\\lib\\Linux\\B\\libStereoTool_somewhereelse.so".to_owned(),
            "libStereoTool_1105\\readme.txt".to_owned(),
        ] {
            zip.start_file(&entry, options).unwrap();
            zip.write_all(entry.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    /// What this machine's build is called, or nothing to check on an
    /// architecture Thimeo does not name.
    fn this_machine() -> Option<&'static str> {
        match std::env::consts::ARCH {
            "x86_64" => Some("intel64"),
            "x86" => Some("intel32"),
            "aarch64" => Some("arm64"),
            "arm" => Some("arm32"),
            _ => None,
        }
    }

    #[test]
    fn a_stereo_tool_archive_gives_up_this_machine_s_libraries() {
        let Some(arch) = this_machine() else {
            return;
        };
        let home = scratch("import-stereotool");
        let archive = home.join("stereo_tool.zip");
        thimeo_archive(&archive, arch);
        let into = home.join("stereotool");

        let installed = import_stereotool_into(&archive, &into).expect("it installs");
        assert_eq!(
            installed,
            vec![
                into.join(format!("libStereoTool_{arch}.so")),
                into.join(format!("libStereoTool_noX11_{arch}.so")),
            ],
            "this machine's builds, and not the Kantar edition of the same name"
        );
        // The entries are nested with backslashes, and they land flat under
        // their own names rather than as one long one.
        assert!(std::fs::read_dir(&into)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains('\\')));
        assert!(!into.join("readme.txt").exists());
        assert!(!into.join("libKantarPlugin300.so").exists());

        // The plain build is the one taken, not the Kantar file that shares
        // its name.
        let plain = std::fs::read_to_string(into.join(format!("libStereoTool_{arch}.so"))).unwrap();
        assert!(!plain.contains("Kantar"));

        // Again is an upgrade, not a refusal.
        assert!(import_stereotool_into(&archive, &into).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_archive_for_another_machine_leaves_nothing_behind() {
        let Some(mine) = this_machine() else {
            return;
        };
        let other = if mine == "arm64" { "intel64" } else { "arm64" };
        let home = scratch("import-stereotool-foreign");
        let archive = home.join("stereo_tool.zip");
        thimeo_archive(&archive, other);
        let into = home.join("stereotool");

        assert!(import_stereotool_into(&archive, &into).is_err());
        assert!(std::fs::read_dir(&into).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_bare_stereo_tool_library_is_taken_as_it_is() {
        let home = scratch("import-stereotool-so");
        let loose = home.join("libStereoTool_intel64.so");
        std::fs::write(&loose, b"not really a library").unwrap();
        let into = home.join("stereotool");

        let installed = import_stereotool_into(&loose, &into).expect("it installs");
        assert_eq!(installed, vec![into.join("libStereoTool_intel64.so")]);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_archive_without_the_library_is_refused() {
        let home = scratch("import-stereotool-junk");
        let archive = home.join("holiday.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("photo.jpg", options).unwrap();
            zip.write_all(b"not audio software").unwrap();
            zip.finish().unwrap();
        }
        let into = home.join("stereotool");

        assert!(import_stereotool_into(&archive, &into).is_err());
        assert!(std::fs::read_dir(&into).unwrap().next().is_none());

        let text = home.join("notes.txt");
        std::fs::write(&text, b"hello").unwrap();
        assert!(import_stereotool_into(&text, &into).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }
}
