//! The application in front: the one whose window has the focus, among
//! those playing, so a Stream Deck key can put "this one" on a channel.
//!
//! The desktop says which window has the focus — on KDE a KWin script of
//! Pipedeck's, see the application's `kwin.rs` — by the process that owns
//! it and its window class. A window and the stream it plays are often not
//! the same process: a browser plays from a child of the one that draws its
//! window. So a stream belongs to the window when the window's process is
//! the stream's or one of its parents, and failing that, when they go by the
//! same name.

use crate::types::App;

use super::Graph;

/// The window with the focus, as the desktop said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Focus {
    pub pid: u32,
    pub class: String,
}

/// The process that started `pid`, as `/proc` says.
fn parent(pid: u32) -> Option<u32> {
    parent_in(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The parent's process, from what `/proc/<pid>/stat` says.
fn parent_in(stat: &str) -> Option<u32> {
    // The name, in brackets, may hold spaces and brackets of its own: the
    // fields after it start after the last closing one.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Whether `ancestor` is `pid` or one of the processes that led to it.
fn descends(pid: u32, ancestor: u32) -> bool {
    let mut at = pid;
    for _ in 0..64 {
        if at == ancestor {
            return true;
        }
        match parent(at) {
            Some(up) if up > 1 && up != at => at = up,
            _ => return false,
        }
    }
    false
}

/// What a process is called, as `/proc` says: its command, lower case.
fn command(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|comm| comm.trim().to_lowercase())
}

/// Which of the applications playing, each with the process of one of its
/// streams, is the one in the window that has the focus.
pub(super) fn focused<'a>(
    focus: &Focus,
    playing: impl IntoIterator<Item = (&'a App, Option<u32>)> + Clone,
) -> Option<App> {
    let by_process = playing
        .clone()
        .into_iter()
        .find(|(_, pid)| pid.is_some_and(|pid| descends(pid, focus.pid)));
    if let Some((app, _)) = by_process {
        return Some(app.clone());
    }
    let class = focus.class.to_lowercase();
    let comm = command(focus.pid);
    playing
        .into_iter()
        .find(|(app, _)| {
            let (key, name) = (app.key.to_lowercase(), app.name.to_lowercase());
            let base = key.rsplit('/').next().unwrap_or(&key).to_owned();
            (!class.is_empty() && (class == key || class == name || class == base))
                || comm
                    .as_deref()
                    .is_some_and(|comm| comm == base || comm == name)
        })
        .map(|(app, _)| app.clone())
}

impl Graph {
    /// Take which window has the focus now.
    pub fn set_focus(&mut self, pid: u32, class: String) {
        log::debug!("the focus is on {class} ({pid})");
        self.focus = Some(Focus { pid, class });
        self.emit_focus();
    }

    /// Tell which application is in front, when that changed: the focus
    /// moved, or the application in front started or stopped playing.
    pub(super) fn emit_focus(&mut self) {
        let app = self.focus.as_ref().and_then(|focus| {
            focused(
                focus,
                self.streams
                    .values()
                    .map(|stream| (&stream.app, stream.pid)),
            )
        });
        if app == self.focused {
            return;
        }
        self.focused = app.clone();
        self.emit(crate::engine::Event::Focused { app });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(key: &str, name: &str) -> App {
        App {
            key: key.into(),
            name: name.into(),
            icon: None,
        }
    }

    #[test]
    fn a_stream_is_found_by_its_process_or_a_parent_of_it() {
        let me = std::process::id();
        let spotify = app("spotify", "Spotify");
        let focus = Focus {
            pid: me,
            class: "nothing-alike".into(),
        };
        assert_eq!(
            focused(&focus, [(&spotify, Some(me))]),
            Some(spotify.clone())
        );
        // The window's process is the parent of the one playing.
        let parent_pid = parent(me).unwrap();
        let focus = Focus {
            pid: parent_pid,
            class: String::new(),
        };
        assert_eq!(focused(&focus, [(&spotify, Some(me))]), Some(spotify));
    }

    #[test]
    fn failing_that_by_its_name() {
        let firefox = app("firefox", "Firefox");
        let focus = Focus {
            pid: u32::MAX,
            class: "Firefox".into(),
        };
        assert_eq!(focused(&focus, [(&firefox, None)]), Some(firefox));
        let focus = Focus {
            pid: u32::MAX,
            class: "org.kde.dolphin".into(),
        };
        assert_eq!(focused(&focus, [(&app("spotify", "Spotify"), None)]), None);
    }

    #[test]
    fn a_process_name_with_brackets_is_read_past() {
        assert_eq!(parent_in("4242 (Web (Content)) S 17 4242 0"), Some(17));
    }
}
