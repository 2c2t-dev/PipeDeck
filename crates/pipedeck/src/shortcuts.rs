//! Keyboard shortcuts for the soundboard that work with the window in the
//! background, or under a game: the desktop's own, asked for through the
//! GlobalShortcuts portal.
//!
//! Pipedeck says which shortcuts there are, one per sound and one stopping
//! them all, and hears them pressed; the keys are the user's to pick, in
//! the window the desktop opens for that. KDE asks about each new one as it
//! is bound, and binds the ones it knows without a word. Each change of the sounds binds
//! them again, on a session of its own, the portal binding a session's
//! shortcuts once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::gtk::{gio, glib};
use adw::prelude::*;
use libadwaita as adw;

use pipedeck_engine::{Command, SoundConfig, SoundId};

use crate::engine_link::EngineLink;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
/// The shortcut stopping every sound.
const STOP: &str = "stop-sounds";
/// What every sound's shortcut is called, before the sound's id.
const SOUND: &str = "sound-";

/// The shortcuts as the desktop has them.
pub struct Shortcuts {
    connection: gio::DBusConnection,
    version: u32,
    /// The session they are bound on, which the presses come with.
    session: Rc<RefCell<Option<String>>>,
    /// What was bound last, by id and description.
    bound: RefCell<Vec<(String, String)>>,
    /// Bindings asked for while one is under way, the last one only.
    queued: RefCell<Option<Vec<(String, String)>>>,
    busy: std::cell::Cell<bool>,
    _pressed: gio::SignalSubscription,
}

/// What a press of a shortcut asks of the engine.
fn command(shortcut: &str) -> Option<Command> {
    if shortcut == STOP {
        return Some(Command::StopSounds);
    }
    let id = shortcut.strip_prefix(SOUND)?.parse().ok()?;
    Some(Command::PlaySound(SoundId(id)))
}

/// The shortcuts there are for these sounds, by id, with what each does:
/// none without a sound, since the desktop asks about every new shortcut,
/// and someone with no soundboard has nothing to be asked.
fn wanted(sounds: &[SoundConfig]) -> Vec<(String, String)> {
    if sounds.is_empty() {
        return Vec::new();
    }
    std::iter::once((STOP.to_owned(), "Stop every sound".to_owned()))
        .chain(sounds.iter().map(|sound| {
            (
                format!("{SOUND}{}", sound.id),
                format!("Play {}", sound.name),
            )
        }))
        .collect()
}

/// Reach the portal, and hear the shortcuts pressed from then on. None
/// where the desktop has no such portal.
pub async fn start(app: &adw::Application, engine: EngineLink) -> Option<Rc<Shortcuts>> {
    let connection = app.dbus_connection()?;
    let version = connection
        .call_future(
            Some(PORTAL),
            PATH,
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&(INTERFACE, "version").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5000,
        )
        .await
        .ok()
        .and_then(|reply| reply.child_value(0).as_variant())
        .and_then(|version| version.get::<u32>());
    let Some(version) = version else {
        log::info!("the desktop offers no global shortcuts");
        return None;
    };
    // Outside a sandbox, the portal is told which application this is,
    // which it otherwise learns from the sandbox: it is what the desktop
    // files the shortcuts under.
    if !crate::launcher::sandboxed() {
        if let Some(id) = app.application_id() {
            let registered = connection
                .call_future(
                    Some(PORTAL),
                    PATH,
                    "org.freedesktop.host.portal.Registry",
                    "Register",
                    Some(&(id.as_str(), HashMap::<String, glib::Variant>::new()).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    5000,
                )
                .await;
            if let Err(e) = registered {
                log::debug!("not registered with the portal: {e}");
            }
        }
    }

    let session: Rc<RefCell<Option<String>>> = Rc::default();
    let pressed = connection.subscribe_to_signal(
        Some(PORTAL),
        Some(INTERFACE),
        Some("Activated"),
        Some(PATH),
        None,
        gio::DBusSignalFlags::NONE,
        {
            let session = session.clone();
            move |signal| {
                let params = signal.parameters;
                let from = params.child_value(0);
                if from.str() != session.borrow().as_deref() {
                    return;
                }
                let shortcut = params.child_value(1);
                if let Some(command) = shortcut.str().and_then(command) {
                    engine.send(command);
                }
            }
        },
    );
    Some(Rc::new(Shortcuts {
        connection,
        version,
        session,
        bound: RefCell::new(Vec::new()),
        queued: RefCell::new(None),
        busy: std::cell::Cell::new(false),
        _pressed: pressed,
    }))
}

impl Shortcuts {
    /// Whether the desktop can open its own window for the keys.
    pub fn can_configure(&self) -> bool {
        self.version >= 2
    }

    /// Open the desktop's window for the keys of the shortcuts.
    pub fn configure(&self) {
        let Some(session) = self.session.borrow().clone() else {
            return;
        };
        let Ok(session) = glib::variant::ObjectPath::try_from(session) else {
            return;
        };
        let connection = self.connection.clone();
        glib::spawn_future_local(async move {
            let opened = connection
                .call_future(
                    Some(PORTAL),
                    PATH,
                    INTERFACE,
                    "ConfigureShortcuts",
                    Some(&(session, "", HashMap::<String, glib::Variant>::new()).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    -1,
                )
                .await;
            if let Err(e) = opened {
                log::warn!("cannot open the shortcuts' settings: {e}");
            }
        });
    }

    /// Have the desktop know the shortcuts these sounds call for, unless it
    /// does already.
    pub fn bind(self: &Rc<Self>, sounds: &[SoundConfig]) {
        let wanted = wanted(sounds);
        if self.busy.get() {
            *self.queued.borrow_mut() = Some(wanted);
            return;
        }
        if *self.bound.borrow() == wanted {
            return;
        }
        self.busy.set(true);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let mut wanted = wanted;
            loop {
                match this.bind_on_new_session(&wanted).await {
                    Ok(()) => *this.bound.borrow_mut() = wanted,
                    Err(e) => log::warn!("the soundboard's shortcuts are not bound: {e}"),
                }
                let Some(next) = this.queued.borrow_mut().take() else {
                    break;
                };
                if *this.bound.borrow() == next {
                    break;
                }
                wanted = next;
            }
            this.busy.set(false);
        });
    }

    /// Close the session the shortcuts were on, open another, and bind
    /// them there.
    async fn bind_on_new_session(&self, wanted: &[(String, String)]) -> Result<(), String> {
        let old = self.session.borrow_mut().take();
        if let Some(old) = old {
            let _ = self
                .connection
                .call_future(
                    Some(PORTAL),
                    &old,
                    "org.freedesktop.portal.Session",
                    "Close",
                    None,
                    None,
                    gio::DBusCallFlags::NONE,
                    5000,
                )
                .await;
        }
        let created = request(&self.connection, "CreateSession", |token| {
            let mut options: HashMap<String, glib::Variant> = HashMap::new();
            options.insert("handle_token".into(), token.to_variant());
            options.insert("session_handle_token".into(), "pipedeck".to_variant());
            (options,).to_variant()
        })
        .await?;
        let session = created
            .get("session_handle")
            .and_then(|handle| handle.str().map(str::to_owned))
            .ok_or("the portal made no session")?;
        let path = glib::variant::ObjectPath::try_from(session.clone())
            .map_err(|_| format!("the portal's session is no path: {session}"))?;
        *self.session.borrow_mut() = Some(session);

        let shortcuts: Vec<(String, HashMap<String, glib::Variant>)> = wanted
            .iter()
            .map(|(id, description)| {
                let mut about: HashMap<String, glib::Variant> = HashMap::new();
                about.insert("description".into(), description.to_variant());
                (id.clone(), about)
            })
            .collect();
        request(&self.connection, "BindShortcuts", |token| {
            let mut options: HashMap<String, glib::Variant> = HashMap::new();
            options.insert("handle_token".into(), token.to_variant());
            (path.clone(), shortcuts.clone(), "", options).to_variant()
        })
        .await?;
        Ok(())
    }
}

/// Call one of the portal's methods that answer later, on a request object,
/// and wait for that answer.
async fn request(
    connection: &gio::DBusConnection,
    method: &str,
    args: impl FnOnce(&str) -> glib::Variant,
) -> Result<HashMap<String, glib::Variant>, String> {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let token = format!(
        "pipedeck{}",
        COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let sender = connection
        .unique_name()
        .ok_or("not on the bus")?
        .trim_start_matches(':')
        .replace('.', "_");
    let handle = format!("{PATH}/request/{sender}/{token}");
    // Listened for before asking, so an answer that comes at once is heard.
    let (tx, rx) = async_channel::bounded(1);
    let _answer = connection.subscribe_to_signal(
        Some(PORTAL),
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(&handle),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let _ = tx.try_send(signal.parameters.clone());
        },
    );
    connection
        .call_future(
            Some(PORTAL),
            PATH,
            INTERFACE,
            method,
            Some(&args(&token)),
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
        .map_err(|e| format!("{method}: {e}"))?;
    let answer = rx
        .recv()
        .await
        .map_err(|_| format!("{method}: no answer"))?;
    let code: u32 = answer.child_value(0).get().unwrap_or(2);
    if code != 0 {
        return Err(format!("{method} was refused ({code})"));
    }
    Ok(answer.child_value(1).get().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_pressed_plays_its_sound_or_stops_them_all() {
        assert_eq!(command("sound-4"), Some(Command::PlaySound(SoundId(4))));
        assert_eq!(command("stop-sounds"), Some(Command::StopSounds));
        assert_eq!(command("sound-x"), None);
        assert_eq!(command("other"), None);
    }

    #[test]
    fn there_is_a_shortcut_per_sound_and_one_to_stop() {
        let sounds = [SoundConfig {
            id: SoundId(2),
            name: "Airhorn".into(),
            file: "2.mp3".into(),
        }];
        assert!(wanted(&[]).is_empty());
        assert_eq!(
            wanted(&sounds),
            [
                ("stop-sounds".to_owned(), "Stop every sound".to_owned()),
                ("sound-2".to_owned(), "Play Airhorn".to_owned()),
            ]
        );
    }
}
