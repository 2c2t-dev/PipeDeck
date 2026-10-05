//! Knowing which window has the focus, on KDE: a KWin script of Pipedeck's
//! tells it over D-Bus whenever another window comes in front, so a Stream
//! Deck key can put the application playing in it on a channel.
//!
//! The script is installed in the user's KWin scripts and switched on in
//! KWin's settings, the way the desktop's own settings do it; taking it out
//! switches it off and removes it. It calls the object this module puts on
//! the application's own D-Bus name, which is there while Pipedeck runs;
//! while it does not, KWin's calls go nowhere.
//!
//! A Flatpak reaches neither KWin's scripts nor its settings: it hands KWin
//! the script over D-Bus, from the folder it shares with the system, each
//! time it starts.

use std::path::PathBuf;

use adw::gtk::{gio, glib};
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::Command as EngineCommand;

use crate::engine_link::EngineLink;

const SCRIPT: &str = "pipedeck-focus";
const PATH: &str = "/dev/_2c2t/Pipedeck/Focus";
const INTERFACE: &str = "dev._2c2t.Pipedeck.Focus";
const INTERFACE_XML: &str = r#"<node>
  <interface name="dev._2c2t.Pipedeck.Focus">
    <method name="Activated">
      <arg type="i" name="pid" direction="in"/>
      <arg type="s" name="class" direction="in"/>
    </method>
  </interface>
</node>"#;

const METADATA: &str = r#"{
    "KPackageStructure": "KWin/Script",
    "KPlugin": {
        "Id": "pipedeck-focus",
        "Name": "Pipedeck: application in front",
        "Description": "Tells Pipedeck which window has the focus, for its Stream Deck keys.",
        "Authors": [{ "Name": "Pipedeck" }],
        "License": "MIT",
        "Version": "1"
    },
    "X-Plasma-API": "javascript",
    "X-Plasma-MainScript": "code/main.js"
}
"#;

const MAIN_JS: &str = r#"// Tell Pipedeck which window has the focus: the process owning it, and its
// class. See Pipedeck's kwin.rs.
function tell(window) {
    if (!window) {
        return;
    }
    callDBus("dev._2c2t.Pipedeck", "/dev/_2c2t/Pipedeck/Focus", "dev._2c2t.Pipedeck.Focus",
        "Activated", window.pid, String(window.resourceClass || ""));
}
workspace.windowActivated.connect(tell);
tell(workspace.activeWindow);
"#;

/// Is this KDE, where the script runs?
pub fn available() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktop| desktop.contains("KDE"))
}

fn script_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_default()
        .join("kwin/scripts")
        .join(SCRIPT)
}

/// Is the script installed? In a Flatpak, whether it is asked for.
pub fn installed() -> bool {
    if crate::launcher::sandboxed() {
        return crate::settings::Settings::load().focus_script;
    }
    script_dir().join("contents/code/main.js").is_file()
}

/// In a Flatpak, where the script is kept for KWin to read: the folder the
/// sandbox shares with the system for the control socket.
fn shared_script() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pipedeck")
        .join(format!("{SCRIPT}.js"))
}

fn call_kwin(
    path: &str,
    interface: &str,
    method: &str,
    args: Option<&glib::Variant>,
) -> Result<glib::Variant, String> {
    gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .and_then(|bus| {
            bus.call_sync(
                Some("org.kde.KWin"),
                path,
                interface,
                method,
                args,
                None,
                gio::DBusCallFlags::NONE,
                5000,
                gio::Cancellable::NONE,
            )
        })
        .map_err(|e| format!("KWin: {e}"))
}

/// Hand KWin the script and run it, in place of one handed over before.
fn load() -> Result<(), String> {
    let path = shared_script();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, MAIN_JS).map_err(|e| format!("{}: {e}", path.display()))?;
    unload()?;
    let loaded = call_kwin(
        "/Scripting",
        "org.kde.kwin.Scripting",
        "loadScript",
        Some(&(path.display().to_string(), SCRIPT).to_variant()),
    )?;
    let id: i32 = loaded.child_value(0).get().unwrap_or(-1);
    if id < 0 {
        return Err("KWin did not take the script".into());
    }
    call_kwin(
        &format!("/Scripting/Script{id}"),
        "org.kde.kwin.Script",
        "run",
        None,
    )
    .map(|_| ())
}

fn unload() -> Result<(), String> {
    call_kwin(
        "/Scripting",
        "org.kde.kwin.Scripting",
        "unloadScript",
        Some(&(SCRIPT,).to_variant()),
    )
    .map(|_| ())
}

/// Switch the script on or off in KWin's settings, and have KWin read them
/// again.
fn switch(on: bool) -> Result<(), String> {
    let set = crate::launcher::host_command("kwriteconfig6")
        .args(["--file", "kwinrc", "--group", "Plugins", "--key"])
        .arg(format!("{SCRIPT}Enabled"))
        .arg(if on { "true" } else { "false" })
        .status()
        .map_err(|e| format!("kwriteconfig6: {e}"))?;
    if !set.success() {
        return Err("KWin's settings could not be written".into());
    }
    let reconfigure =
        gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).and_then(|bus| {
            bus.call_sync(
                Some("org.kde.KWin"),
                "/KWin",
                "org.kde.KWin",
                "reconfigure",
                None,
                None,
                gio::DBusCallFlags::NONE,
                5000,
                gio::Cancellable::NONE,
            )
        });
    reconfigure.map(|_| ()).map_err(|e| format!("KWin: {e}"))
}

/// Install the script and switch it on.
pub fn install() -> Result<(), String> {
    if crate::launcher::sandboxed() {
        return load();
    }
    let dir = script_dir();
    let write = |path: PathBuf, text: &str| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    };
    write(dir.join("metadata.json"), METADATA)?;
    write(dir.join("contents/code/main.js"), MAIN_JS)?;
    switch(true)
}

/// Switch the script off and take it out.
pub fn remove() -> Result<(), String> {
    if crate::launcher::sandboxed() {
        let _ = std::fs::remove_file(shared_script());
        return unload();
    }
    switch(false)?;
    let dir = script_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

/// Hear what the script says, on the application's own connection, and
/// pass it to the engine. Kept for as long as the application runs.
pub fn listen(app: &adw::Application, engine: &EngineLink) {
    let Some(connection) = app.dbus_connection() else {
        return;
    };
    let Some(info) = gio::DBusNodeInfo::for_xml(INTERFACE_XML)
        .ok()
        .and_then(|node| node.lookup_interface(INTERFACE))
    else {
        return;
    };
    let engine = engine.clone();
    // Registered for as long as the connection lasts, which is as long as
    // the application.
    let registered = connection
        .register_object(PATH, &info)
        .method_call(move |_, _, _, _, method, params, invocation| {
            if method == "Activated" {
                if let Some((pid, class)) = params.get::<(i32, String)>() {
                    if let Ok(pid) = u32::try_from(pid) {
                        engine.send(EngineCommand::SetFocus { pid, class });
                    }
                }
            }
            invocation.return_value(None);
        })
        .build();
    if let Err(e) = registered {
        log::warn!("cannot hear which window has the focus: {e}");
    }
    // A Flatpak's script lasts as long as KWin's session, not across them.
    if crate::launcher::sandboxed() && installed() {
        if let Err(e) = load() {
            log::warn!("cannot tell KWin to say which window has the focus: {e}");
        }
    }
}
