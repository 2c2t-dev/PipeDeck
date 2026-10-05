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
    which(program).is_some()
}

/// Where a program is found on the PATH, as running it would find it.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
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
    let nodes = managed_nodes(&home(), |var| std::env::var_os(var));
    if !on_path("git") {
        return Err("git is not installed. Building Vencord needs git and Node.js.".into());
    }
    if !on_path("npx") && nodes.is_empty() {
        return Err("npx is not installed. Building Vencord needs git and Node.js.".into());
    }
    let dir = vencord_dir();
    // Not marked as holding the plugin until this build is checked to.
    let _ = std::fs::remove_file(dist().join(MARKER));
    if dir.join(".git").is_dir() {
        say("Updating Vencord…");
        run(
            "Updating Vencord",
            crate::launcher::host_command("git")
                .args(["pull", "--ff-only", "--quiet"])
                .current_dir(&dir),
        )?;
    } else {
        say("Downloading Vencord…");
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        run(
            "Downloading Vencord",
            crate::launcher::host_command("git")
                .args(["clone", "--quiet", "--depth", "1", VENCORD_REPO])
                .arg(&dir),
        )?;
    }

    let plugin = dir.join("src").join("userplugins").join(PLUGIN_DIR);
    let _ = std::fs::remove_dir_all(&plugin);
    std::fs::create_dir_all(&plugin).map_err(|e| e.to_string())?;
    std::fs::write(plugin.join("index.ts"), PLUGIN_INDEX).map_err(|e| e.to_string())?;
    std::fs::write(plugin.join("native.ts"), PLUGIN_NATIVE).map_err(|e| e.to_string())?;

    // Vencord is built with the pnpm its package names, fetched by npx.
    let pnpm = format!("pnpm@{}", pnpm_version(&dir)?);
    let node = node_for_build(&dir, &pnpm, &nodes)?;
    let npx = || {
        let mut command = crate::launcher::host_command(
            node.as_ref()
                .map_or_else(|| PathBuf::from("npx"), |bin| bin.join("npx")),
        );
        // npx and pnpm run on whichever node the PATH finds first.
        if let Some(bin) = &node {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let paths = std::iter::once(bin.clone()).chain(std::env::split_paths(&path));
            if let Ok(joined) = std::env::join_paths(paths) {
                command.env("PATH", joined);
            }
        }
        command
    };
    say("Fetching what Vencord is built with…");
    run(
        "Fetching what Vencord is built with",
        npx()
            .args(["--yes", &pnpm, "install", "--frozen-lockfile", "--silent"])
            .current_dir(&dir),
    )?;
    say("Building Vencord with the plugin…");
    run(
        "Building Vencord",
        npx().args(["--yes", &pnpm, "build"]).current_dir(&dir),
    )?;
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
            Stale::Plugin => "Newer plugin available",
            Stale::Vesktop => "Vesktop was updated",
            Stale::Unknown => "Built by an older Pipedeck",
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

/// The Node.js to build Vencord with: the one on the PATH when it is as
/// new as Vencord and its pnpm ask, as `None`; or else the newest a version
/// manager keeps that is, by the folder its programs are in. The desktop
/// does not run the shell that puts a version manager's Node.js on the
/// PATH, so Pipedeck started from a menu finds the system's, often older,
/// and an older one fails deep inside pnpm, with an error that says
/// nothing of Node's version. When the versions cannot be read, the build
/// is left to say what it will.
fn node_for_build(
    dir: &Path,
    pnpm: &str,
    managed: &[(Version, PathBuf)],
) -> Result<Option<PathBuf>, String> {
    let have = command_output("node", &["--version"])
        .as_deref()
        .and_then(version_in);
    let npm = if on_path("npm") {
        PathBuf::from("npm")
    } else if let Some((_, bin)) = managed.last() {
        bin.join("npm")
    } else {
        return Ok(None);
    };
    let vencord = read_json(&dir.join("package.json"))
        .ok()
        .and_then(|package| package["engines"]["node"].as_str().and_then(lowest));
    let pnpm = command_output(npm, &["view", pnpm, "engines.node"])
        .as_deref()
        .and_then(lowest);
    let Some(needs) = vencord.into_iter().chain(pnpm).max() else {
        return Ok(None);
    };
    pick_node(have, needs, managed, which("node").as_deref())
}

/// Which Node.js to build with, from what is known of them: the system's,
/// `have`, found at `system`, when it is at least `needs`, as `None`; else
/// the newest kept one that is; else why there is none.
fn pick_node(
    have: Option<Version>,
    needs: Version,
    managed: &[(Version, PathBuf)],
    system: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    if have.is_some_and(|have| have >= needs) {
        return Ok(None);
    }
    if let Some((_, bin)) = newest_from(managed, needs) {
        return Ok(Some(bin.clone()));
    }
    // Named by its path: the Node.js a terminal finds is often another.
    let found = match (system, have) {
        (Some(path), Some(have)) => format!(
            "the one Pipedeck finds, {}, is {}",
            path.display(),
            shown(have)
        ),
        _ => "Pipedeck finds none".to_owned(),
    };
    Err(format!(
        "Building Vencord needs Node.js {} or newer, and {found}, nor do nvm, fnm or \
         volta keep one. Install a newer one, then try again.",
        shown(needs)
    ))
}

/// A version as people write it: `22.13` rather than `22.13.0`.
fn shown((major, minor, patch): Version) -> String {
    match (minor, patch) {
        (0, 0) => major.to_string(),
        (_, 0) => format!("{major}.{minor}"),
        _ => format!("{major}.{minor}.{patch}"),
    }
}

/// The newest of the Node.js versions kept that is at least `needs`.
fn newest_from(managed: &[(Version, PathBuf)], needs: Version) -> Option<&(Version, PathBuf)> {
    managed.iter().rev().find(|(version, _)| *version >= needs)
}

/// The Node.js versions nvm, fnm and volta keep, oldest first, with the
/// folder each one's programs are in. Read from their folders, where each
/// version is named, rather than by running every one.
fn managed_nodes(
    home: &Path,
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<(Version, PathBuf)> {
    let set_or = |name: &str, fallback: PathBuf| var(name).map_or(fallback, PathBuf::from);
    let data = set_or("XDG_DATA_HOME", home.join(".local/share"));
    // Where each keeps its versions, and where a version's programs are.
    let kept = [
        (
            set_or("NVM_DIR", home.join(".nvm")).join("versions/node"),
            "bin",
        ),
        (
            set_or("FNM_DIR", data.join("fnm")).join("node-versions"),
            "installation/bin",
        ),
        (home.join(".fnm/node-versions"), "installation/bin"),
        (
            set_or("VOLTA_HOME", home.join(".volta")).join("tools/image/node"),
            "bin",
        ),
    ];
    let mut nodes: Vec<(Version, PathBuf)> = kept
        .iter()
        .filter_map(|(versions, bin)| Some((std::fs::read_dir(versions).ok()?, *bin)))
        .flat_map(|(entries, bin)| {
            entries.flatten().filter_map(move |entry| {
                let version = version_in(entry.file_name().to_str()?)?;
                let programs = entry.path().join(bin);
                programs
                    .join("node")
                    .is_file()
                    .then_some((version, programs))
            })
        })
        .collect();
    nodes.sort();
    nodes.dedup_by(|a, b| a.0 == b.0);
    nodes
}

/// A version, as major, minor and patch.
type Version = (u32, u32, u32);

/// What a program prints, when it runs and succeeds.
fn command_output(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Option<String> {
    let output = crate::launcher::host_command(program)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// A version as `node --version` writes it, `v20.20.2`, or as a range's
/// bound, `22.13`.
fn version_in(text: &str) -> Option<Version> {
    let mut parts = text
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u32>().ok());
    let major = parts.next()??;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// The lowest version a range of the `>=22.13` kind lets in; none for a
/// range of another kind, which is left to the build.
fn lowest(range: &str) -> Option<Version> {
    version_in(range.trim().strip_prefix(">=")?)
}

/// Run one step of the build, and say which failed and why if one does.
/// What a program prints when it fails ends as often in a stack or an
/// object as in its reason, so the line naming the error is what is said,
/// and the whole of it is kept in a log beside Vencord.
fn run(step: &str, command: &mut Command) -> Result<(), String> {
    run_logged(step, command, &build_log())
}

/// The same, keeping what a failed step printed in `log`.
fn run_logged(step: &str, command: &mut Command, log: &Path) -> Result<(), String> {
    let output = command.output().map_err(|e| format!("{step}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let kept = std::fs::write(log, &said).is_ok();
    let mut reason = reason_in(&said);
    if kept {
        reason.push_str(&format!(" (all of it is in {})", log.display()));
    }
    Err(format!("{step} failed: {reason}"))
}

/// Where the output of a failed step is kept.
fn build_log() -> PathBuf {
    vencord_dir().with_file_name("vencord-build.log")
}

/// The line of a failed program's output that names what went wrong: the
/// first that says error, or else the last that says anything.
fn reason_in(said: &str) -> String {
    let lines = || said.lines().map(str::trim).filter(|line| !line.is_empty());
    lines()
        .find(|line| line.to_lowercase().contains("error"))
        .or_else(|| lines().next_back())
        .unwrap_or("it said nothing")
        .to_owned()
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
    fn a_version_manager_s_node_is_found_where_it_keeps_it() {
        let home = std::env::temp_dir().join(format!("pipedeck-nodes-{}", std::process::id()));
        let node = |path: &str| {
            let bin = home.join(path);
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("node"), "").unwrap();
            bin
        };
        node(".nvm/versions/node/v20.20.2/bin");
        let nvm = node(".nvm/versions/node/v26.10.0/bin");
        let fnm = node(".local/share/fnm/node-versions/v22.13.0/installation/bin");
        let volta = node("volta/tools/image/node/24.1.0/bin");
        // A folder without the program is no version.
        std::fs::create_dir_all(home.join(".nvm/versions/node/v30.0.0")).unwrap();

        let found = managed_nodes(&home, |var| {
            (var == "VOLTA_HOME").then(|| home.join("volta").into_os_string())
        });
        let _ = std::fs::remove_dir_all(&home);

        let versions: Vec<Version> = found.iter().map(|(version, _)| *version).collect();
        assert_eq!(
            versions,
            [(20, 20, 2), (22, 13, 0), (24, 1, 0), (26, 10, 0)]
        );
        assert_eq!(
            newest_from(&found, (22, 13, 0)).map(|(_, bin)| bin),
            Some(&nvm)
        );
        assert_eq!(
            newest_from(&found[..3], (22, 13, 0)).map(|(_, bin)| bin),
            Some(&volta)
        );
        assert_eq!(
            newest_from(&found[..2], (22, 13, 0)).map(|(_, bin)| bin),
            Some(&fnm)
        );
        assert_eq!(newest_from(&found[..1], (22, 13, 0)), None);
    }

    #[test]
    fn the_node_built_with_is_the_system_s_or_a_newer_kept_one() {
        let needs = (22, 13, 0);
        let kept = vec![
            ((20, 1, 0), PathBuf::from("/nvm/v20/bin")),
            ((26, 10, 0), PathBuf::from("/nvm/v26/bin")),
        ];
        let system = Some(Path::new("/usr/bin/node"));
        assert_eq!(pick_node(Some((24, 0, 0)), needs, &kept, system), Ok(None));
        assert_eq!(
            pick_node(Some((20, 20, 2)), needs, &kept, system),
            Ok(Some(PathBuf::from("/nvm/v26/bin")))
        );
        assert_eq!(
            pick_node(None, needs, &kept, None),
            Ok(Some(PathBuf::from("/nvm/v26/bin")))
        );
        let old = pick_node(Some((20, 20, 2)), needs, &kept[..1], system).unwrap_err();
        assert!(old.contains("needs Node.js 22.13 or newer"), "{old}");
        assert!(old.contains("/usr/bin/node, is 20.20.2"), "{old}");
        let none = pick_node(None, needs, &[], None).unwrap_err();
        assert!(none.contains("Pipedeck finds none"), "{none}");
        assert_eq!(shown((22, 0, 0)), "22");
        assert_eq!(shown((22, 13, 1)), "22.13.1");
    }

    #[test]
    fn a_step_says_what_failed_and_keeps_all_of_it() {
        let dir = std::env::temp_dir().join(format!("pipedeck-steps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("build.log");
        let sh = |script: &str| {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            command
        };

        assert_eq!(run_logged("Fine", &mut sh("true"), &log), Ok(()));
        assert!(!log.exists(), "nothing is kept of a step that went well");
        let failed = run_logged(
            "Building Vencord",
            &mut sh("echo building; echo 'Error: no such module' >&2; echo '}' >&2; exit 1"),
            &log,
        );
        let kept = std::fs::read_to_string(&log).unwrap_or_default();
        let missing = run_logged(
            "Missing",
            &mut Command::new("pipedeck-no-such-program"),
            &log,
        );
        let _ = std::fs::remove_dir_all(&dir);

        let failed = failed.unwrap_err();
        assert!(
            failed.starts_with("Building Vencord failed: Error: no such module"),
            "{failed}"
        );
        assert!(failed.contains(&log.display().to_string()), "{failed}");
        assert!(kept.contains("building") && kept.contains('}'), "{kept}");
        assert!(missing.unwrap_err().starts_with("Missing: "));
        assert_eq!(
            command_output("sh", &["-c", "echo ' 22.13 '"]).as_deref(),
            Some("22.13")
        );
        assert_eq!(command_output("sh", &["-c", "exit 1"]), None);
    }

    #[test]
    fn versions_are_read_as_node_and_ranges_write_them() {
        assert_eq!(version_in("v20.20.2"), Some((20, 20, 2)));
        assert_eq!(version_in("v26.10.0\n"), Some((26, 10, 0)));
        assert_eq!(lowest(">=22.13"), Some((22, 13, 0)));
        assert_eq!(lowest(">=22"), Some((22, 0, 0)));
        assert_eq!(lowest("^18 || >=20"), None);
        assert!(version_in("v20.20.2") < lowest(">=22.13"));
    }

    #[test]
    fn a_failure_is_said_by_the_line_naming_it() {
        let node = "node:internal/modules/cjs/loader:1234\n  throw err;\n\n\
                    Error [ERR_UNKNOWN_BUILTIN_MODULE]: No such built-in module: node:x\n\
                    at Module._load (node:internal)\n{\n  code: 'ERR_UNKNOWN_BUILTIN_MODULE'\n}\n";
        assert_eq!(
            reason_in(node),
            "Error [ERR_UNKNOWN_BUILTIN_MODULE]: No such built-in module: node:x"
        );
        assert_eq!(
            reason_in("fatal: not a git repository\n"),
            "fatal: not a git repository"
        );
        assert_eq!(reason_in("\n  \n"), "it said nothing");
    }

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
