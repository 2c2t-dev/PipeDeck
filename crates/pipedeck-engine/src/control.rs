//! A local socket other programs tell the mixer things through.
//!
//! For now, one thing: who is in the call Vesktop is in. Pipedeck's plugin
//! for Vesktop sends the list whenever it changes, one JSON object to a
//! line:
//!
//! ```json
//! {"call": [{"id": "235088799074484224", "name": "Alice"}]}
//! ```
//!
//! and gets back, on a line of its own, the name of the sink made for each
//! of them, which is what it looks their output up by:
//!
//! ```json
//! {"labels": {"235088799074484224": "Alice (Discord)"}}
//! ```
//!
//! A connection that closes ends the call: Vesktop has gone, and so have
//! the voices it was playing.
//!
//! The socket is `$XDG_RUNTIME_DIR/pipedeck/control.sock`, which only the
//! user can reach. A second mixer finds it answering and leaves it to the
//! first.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use pipewire as pw;
use serde::{Deserialize, Serialize};

use crate::engine::Command;
use crate::types::{voice_labels, CallMember};

/// Where the socket is.
pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("pipedeck").join("control.sock")
}

#[derive(Debug, Deserialize)]
struct Message {
    call: Option<Vec<CallMember>>,
}

#[derive(Debug, Serialize)]
struct Answer {
    labels: HashMap<String, String>,
}

/// Listen on the socket, on a thread of its own, handing what comes in to
/// the engine. Does nothing when another mixer already listens.
pub fn listen(commands: pw::channel::Sender<Command>) {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        log::info!("another mixer answers on {}", path.display());
        return;
    }
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::error!("cannot make {}: {e}", dir.display());
            return;
        }
    }
    // Nobody answered, so a file left there is from a mixer that is gone.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            log::error!("cannot listen on {}: {e}", path.display());
            return;
        }
    };
    log::info!("listening on {}", path.display());
    let spawned = std::thread::Builder::new()
        .name("pipedeck-control".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let commands = commands.clone();
                        let _ = std::thread::Builder::new()
                            .name("pipedeck-client".into())
                            .spawn(move || serve(stream, commands));
                    }
                    Err(e) => log::warn!("a client could not connect: {e}"),
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot start listening: {e}");
    }
}

/// Answer one client until it goes.
fn serve(stream: UnixStream, commands: pw::channel::Sender<Command>) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut in_call = false;
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let message: Message = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(e) => {
                log::warn!("a client said something unclear: {e}");
                continue;
            }
        };
        let Some(members) = message.call else {
            continue;
        };
        let labels = voice_labels(&members);
        let answer = Answer {
            labels: members
                .iter()
                .map(|member| member.id.clone())
                .zip(labels)
                .collect(),
        };
        in_call = !members.is_empty();
        if commands.send(Command::SetCall { members }).is_err() {
            break;
        }
        let Ok(mut text) = serde_json::to_string(&answer) else {
            continue;
        };
        text.push('\n');
        if writer.write_all(text.as_bytes()).is_err() {
            break;
        }
    }
    if in_call {
        let _ = commands.send(Command::SetCall {
            members: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_is_read_and_answered_with_unique_labels() {
        let message: Message = serde_json::from_str(
            r#"{"call":[{"id":"1","name":"Alice","avatar":"/tmp/a.png"},{"id":"2","name":"Alice"},{"id":"3","name":"Bob"}]}"#,
        )
        .expect("a call");
        let members = message.call.expect("members");
        assert_eq!(members[0].avatar.as_deref(), Some("/tmp/a.png"));
        assert_eq!(
            voice_labels(&members),
            ["Alice (Discord)", "Alice (Discord) 2", "Bob (Discord)"]
        );
    }
}
