//! Putting Pipedeck's plugin into Vesktop, so the people of a Discord call
//! can each have a sub-track.
//!
//! Vencord, the mod Vesktop carries, only runs the plugins built into it,
//! and this one is not among them: installing it means building Vencord
//! with the plugin in, into a folder of Pipedeck's, and pointing Vesktop at
//! that folder instead of its own copy. Removing it points Vesktop back.
//!
//! The plugin's sources are part of this program, so it can install them
//! from wherever it runs. Building needs git and Node.js, which fetch
//! Vencord and the tools it is built with.
//!
//! Vesktop writes its state when it quits, over whatever was put there
//! while it ran, so the last step waits for it to be closed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

/// The plugin, as the repository has it.
const PLUGIN_INDEX: &str =
    include_str!("../../../integrations/vencord/pipedeckVoices.vesktop/index.ts");
const PLUGIN_NATIVE: &str =
    include_str!("../../../integrations/vencord/pipedeckVoices.vesktop/native.ts");
/// The plugin's folder inside Vencord's sources; the suffix builds it into
/// the Vesktop flavour only.
const PLUGIN_DIR: &str = "pipedeckVoices.vesktop";
const PLUGIN_NAME: &str = "PipedeckVoices";
const VENCORD_REPO: &str = "https://github.com/Vendicated/Vencord";
/// Left in the build once it has been checked to hold the plugin, so that
/// is known without reading megabytes of it again.
const MARKER: &str = "pipedeck-voices";

/// Set while an install or a removal runs: two at once would build in the
/// same folder and write the same files.
static BUSY: AtomicBool = AtomicBool::new(false);

/// Is an install or a removal running? The settings window can be closed
/// and opened again while one does.
pub fn busy() -> bool {
    BUSY.load(Ordering::Acquire)
}

/// The last thing said by an install or a removal, and how it ended: kept
/// here, so a settings window opened while one runs, or after, can say it
/// too.
static SAID: Mutex<Option<String>> = Mutex::new(None);

/// What the install or removal running, or the last one, last said.
pub fn last_said() -> Option<String> {
    SAID.lock().ok().and_then(|said| said.clone())
}

fn note(said: &str) {
    if let Ok(mut kept) = SAID.lock() {
        *kept = Some(said.to_owned());
    }
}

/// Keep how it ended, in the words the window shows.
fn ended(result: Result<(), String>, done: &str) -> Result<(), String> {
    note(&match &result {
        Ok(()) => done.to_owned(),
        Err(e) => format!("It did not work: {e}"),
    });
    result
}

/// Holds [`BUSY`] for as long as it lives.
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

/// Where Vesktop stands with the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Vesktop is not on this machine.
    Missing,
    /// Vesktop is here, running its own Vencord.
    Available,
    /// Vesktop runs the Vencord built here, with the plugin in.
    Installed,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}

/// Vesktop's own settings.
fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("vesktop")
}

/// Where Vencord is built.
fn vencord_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
        .join("pipedeck")
        .join("vencord")
}

fn dist() -> PathBuf {
    vencord_dir().join("dist")
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// Where Vesktop stands now.
pub fn state() -> State {
    if !on_path("vesktop") && !config_dir().is_dir() {
        return State::Missing;
    }
    let pointed = read_json(&config_dir().join("state.json"))
        .ok()
        .and_then(|state| state.get("vencordDir")?.as_str().map(PathBuf::from));
    let renderer = dist().join("vencordDesktopRenderer.js");
    let built = renderer.is_file()
        && (dist().join(MARKER).is_file()
            // Built before the marker was left: read once, and marked.
            || std::fs::read_to_string(&renderer).is_ok_and(|text| {
                text.contains(PLUGIN_NAME)
                    && std::fs::write(dist().join(MARKER), PLUGIN_NAME).is_ok()
            }));
    if pointed.as_deref() == Some(dist().as_path()) && built {
        State::Installed
    } else {
        State::Available
    }
}

/// Is Vesktop running? Its own build's processes are called so; one run
/// on the system's Electron, as distributions package it, is an electron
/// process with Vesktop on its command line.
pub fn is_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) else {
            return false;
        };
        let comm = comm.trim();
        // Vesktop's own archive, as the electron runs it: not any electron
        // application that has a path with Vesktop in it open.
        comm == "vesktop"
            || (comm.starts_with("electron")
                && std::fs::read(entry.path().join("cmdline")).is_ok_and(|line| {
                    String::from_utf8_lossy(&line).split('\0').any(|arg| {
                        let arg = arg.to_lowercase();
                        arg.contains("vesktop") && arg.ends_with(".asar")
                    })
                }))
    })
}

/// Build Vencord with the plugin in and point Vesktop at it, saying how it
/// goes along the way. Also what updating is: Vencord is fetched again and
/// the plugin put back in.
pub fn install(say: impl Fn(&str)) -> Result<(), String> {
    let _busy = Busy::take()?;
    let say = |said: &str| {
        note(said);
        say(said);
    };
    ended(
        build_and_point(&say),
        "Done. Start Vesktop: the plugin is on.",
    )
}

fn build_and_point(say: &impl Fn(&str)) -> Result<(), String> {
    for program in ["git", "npx"] {
        if !on_path(program) {
            return Err(format!(
                "{program} is not installed. Building Vencord needs git and Node.js."
            ));
        }
    }
    let dir = vencord_dir();
    // Not marked as holding the plugin until this build is checked to.
    let _ = std::fs::remove_file(dist().join(MARKER));
    if dir.join(".git").is_dir() {
        say("Updating Vencord…");
        run(Command::new("git")
            .args(["pull", "--ff-only", "--quiet"])
            .current_dir(&dir))?;
    } else {
        say("Downloading Vencord…");
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        run(Command::new("git")
            .args(["clone", "--quiet", "--depth", "1", VENCORD_REPO])
            .arg(&dir))?;
    }

    let plugin = dir.join("src").join("userplugins").join(PLUGIN_DIR);
    let _ = std::fs::remove_dir_all(&plugin);
    std::fs::create_dir_all(&plugin).map_err(|e| e.to_string())?;
    std::fs::write(plugin.join("index.ts"), PLUGIN_INDEX).map_err(|e| e.to_string())?;
    std::fs::write(plugin.join("native.ts"), PLUGIN_NATIVE).map_err(|e| e.to_string())?;

    // Vencord is built with the pnpm its package names, fetched by npx.
    let pnpm = format!("pnpm@{}", pnpm_version(&dir)?);
    say("Fetching what Vencord is built with…");
    run(Command::new("npx")
        .args(["--yes", &pnpm, "install", "--frozen-lockfile", "--silent"])
        .current_dir(&dir))?;
    say("Building Vencord with the plugin…");
    run(Command::new("npx")
        .args(["--yes", &pnpm, "build"])
        .current_dir(&dir))?;
    let renderer = std::fs::read_to_string(dist().join("vencordDesktopRenderer.js"))
        .map_err(|e| format!("the build left no Vesktop files: {e}"))?;
    if !renderer.contains(PLUGIN_NAME) {
        return Err("the build left the plugin out".into());
    }
    // What was built, and against which Vesktop: either changing is what
    // tells an update is due.
    let mark = json!({ "plugin": plugin_fingerprint(), "vesktop": vesktop_fingerprint() });
    std::fs::write(dist().join(MARKER), mark.to_string()).map_err(|e| e.to_string())?;

    wait_for_vesktop(say);
    say("Pointing Vesktop at it…");
    edit_json(&config_dir().join("state.json"), |state| {
        state["vencordDir"] = json!(dist());
    })?;
    // Turned on, so there is nothing to find in Vencord's settings.
    edit_json(
        &config_dir().join("settings").join("settings.json"),
        |settings| {
            if !settings["plugins"].is_object() {
                settings["plugins"] = json!({});
            }
            if !settings["plugins"][PLUGIN_NAME].is_object() {
                settings["plugins"][PLUGIN_NAME] = json!({});
            }
            settings["plugins"][PLUGIN_NAME]["enabled"] = json!(true);
        },
    )?;
    Ok(())
}

/// Why the plugin installed in Vesktop should be built again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stale {
    /// This Pipedeck carries a newer plugin than the one built.
    Plugin,
    /// Vesktop was installed again, or updated, since the build.
    Vesktop,
    /// Built before Pipedeck kept track, so what it was built from is not
    /// known.
    Unknown,
}

impl Stale {
    /// Why, in the words the window shows.
    pub fn reason(self) -> &'static str {
        match self {
            Stale::Plugin => "This Pipedeck carries a newer version of the plugin",
            Stale::Vesktop => "Vesktop was updated since the plugin was built",
            Stale::Unknown => "The plugin was built before Pipedeck kept track of it",
        }
    }
}

/// Whether the plugin should be built again, and why: when it is installed
/// and either the plugin this Pipedeck carries or Vesktop has changed since.
/// Reads a few files; nothing is fetched.
pub fn stale() -> Option<Stale> {
    if state() != State::Installed {
        return None;
    }
    let Ok(mark) = read_json(&dist().join(MARKER)) else {
        return Some(Stale::Unknown);
    };
    if mark["plugin"].as_str() != Some(plugin_fingerprint().as_str()) {
        return Some(Stale::Plugin);
    }
    let vesktop = vesktop_fingerprint();
    if vesktop.is_some() && mark["vesktop"].as_str() != vesktop.as_deref() {
        return Some(Stale::Vesktop);
    }
    None
}

/// The plugin this program carries, as a short hash of its sources.
fn plugin_fingerprint() -> String {
    // FNV-1a: small, and the same from one build of Pipedeck to the next.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in PLUGIN_INDEX.bytes().chain(PLUGIN_NATIVE.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Vesktop's installation, as when and how big its archive was written.
///
/// Packages keep the date a file was built, not the one it was installed:
/// the time the file itself last changed on this disk is what moves when
/// Vesktop is installed again.
fn vesktop_fingerprint() -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let archive = vesktop_archive()?;
    let meta = std::fs::metadata(&archive).ok()?;
    Some(format!("{}:{}", meta.ctime(), meta.size()))
}

/// Where Vesktop's app.asar is: beside the program the `vesktop` on the
/// path runs, which is a script that starts it for most packages.
fn vesktop_archive() -> Option<PathBuf> {
    let mut programs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let launcher = dir.join("vesktop");
            if !launcher.is_file() {
                continue;
            }
            programs.push(launcher.canonicalize().unwrap_or(launcher.clone()));
            // The script's own target: `exec /opt/vesktop/vesktop …`.
            if let Ok(script) = std::fs::read_to_string(&launcher) {
                for word in script.split_whitespace() {
                    if word.starts_with('/') && word.ends_with("vesktop") {
                        programs.push(PathBuf::from(word));
                    }
                }
            }
        }
    }
    let mut candidates: Vec<PathBuf> = programs
        .iter()
        .filter_map(|program| program.parent())
        .flat_map(|dir| [dir.join("resources/app.asar"), dir.join("app.asar")])
        .collect();
    candidates.extend(
        [
            "/opt/vesktop/resources/app.asar",
            "/usr/lib/vesktop/app.asar",
            "/usr/lib/vesktop/resources/app.asar",
        ]
        .map(PathBuf::from),
    );
    candidates.into_iter().find(|archive| archive.is_file())
}

/// Point Vesktop back at its own Vencord.
pub fn remove(say: impl Fn(&str)) -> Result<(), String> {
    let _busy = Busy::take()?;
    let say = |said: &str| {
        note(said);
        say(said);
    };
    ended(
        point_back(&say),
        "Removed. Vesktop runs its own Vencord again.",
    )
}

fn point_back(say: &impl Fn(&str)) -> Result<(), String> {
    wait_for_vesktop(say);
    edit_json(&config_dir().join("state.json"), |state| {
        if let Some(state) = state.as_object_mut() {
            state.remove("vencordDir");
        }
    })
}

/// Wait for Vesktop to be closed, saying so while it is not.
fn wait_for_vesktop(say: &impl Fn(&str)) {
    if !is_running() {
        return;
    }
    say("Quit Vesktop to finish: right-click its icon, then Quit.");
    while is_running() {
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The pnpm version Vencord's package asks for.
fn pnpm_version(dir: &Path) -> Result<String, String> {
    let package = read_json(&dir.join("package.json"))?;
    package["packageManager"]
        .as_str()
        .and_then(|manager| manager.strip_prefix("pnpm@"))
        .map(|version| version.split('+').next().unwrap_or(version).to_owned())
        .ok_or_else(|| "Vencord's package names no pnpm".into())
}

/// Run a program, and say what it said if it failed.
fn run(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let last: Vec<&str> = said.lines().rev().take(4).collect();
    Err(last.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Change a JSON file, making it if it is not there yet.
fn edit_json(path: &Path, change: impl FnOnce(&mut Value)) -> Result<(), String> {
    let mut value = if path.exists() {
        read_json(path)?
    } else {
        json!({})
    };
    change(&mut value);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugin_is_carried_whole() {
        assert!(PLUGIN_INDEX.contains(&format!("name: \"{PLUGIN_NAME}\"")));
        assert!(PLUGIN_NATIVE.contains("export function setCall"));
    }

    #[test]
    fn the_plugin_has_one_fingerprint() {
        let print = plugin_fingerprint();
        assert_eq!(print, plugin_fingerprint());
        assert_eq!(print.len(), 16);
    }

    #[test]
    fn vesktop_is_found_where_its_package_puts_it() {
        // Only where Vesktop is installed the way vesktop-bin does it.
        if Path::new("/opt/vesktop/resources/app.asar").is_file() {
            assert!(vesktop_archive().is_some());
            assert!(vesktop_fingerprint().is_some_and(|print| print.contains(':')));
        }
    }

    #[test]
    fn a_json_file_is_changed_and_the_rest_kept() {
        let dir = std::env::temp_dir().join(format!("pipedeck-vesktop-{}", std::process::id()));
        let path = dir.join("state.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"firstLaunch": false, "vencordDir": "/old"}"#).unwrap();
        edit_json(&path, |state| state["vencordDir"] = json!("/new")).unwrap();
        let state = read_json(&path).unwrap();
        assert_eq!(state["vencordDir"], "/new");
        assert_eq!(state["firstLaunch"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
