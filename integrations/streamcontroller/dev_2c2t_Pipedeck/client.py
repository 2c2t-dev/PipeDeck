"""Pipedeck's control socket, followed for as long as the deck runs.

One connection, kept open and opened again whenever Pipedeck comes back:
it subscribes to the mixer's state, which comes as a line of JSON after
every change, and to its meters, ten times a second, and carries the
deck's orders the other way. The protocol is
described in `crates/pipedeck-engine/src/control.rs`.

Nothing here knows about StreamController, so it can be tried on its own:
`python3 client.py` prints the state as it changes.
"""

import json
import os
import socket
import tempfile
import threading
import time
import weakref

from typing import Callable, Optional


def socket_path() -> str:
    """Where Pipedeck listens: in the runtime directory, or failing one in
    the temporary directory, which the engine and the other plugins pick
    the same way."""
    runtime = os.environ.get("XDG_RUNTIME_DIR") or tempfile.gettempdir()
    return os.path.join(runtime, "pipedeck", "control.sock")


def trusted(path: str) -> bool:
    """Is the socket's folder the user's own, and closed to others? Pipedeck
    makes it so; without a runtime directory it is in the shared temporary
    directory, where one that is not was made by someone else."""
    try:
        folder = os.stat(os.path.dirname(path))
    except OSError:
        return False
    return folder.st_uid == os.getuid() and folder.st_mode & 0o077 == 0


class Pipedeck:
    """The mixer as last told, and a way to tell it things."""

    # How long to wait before trying again when Pipedeck is not running.
    RETRY = 2.0

    def __init__(self, path: Optional[str] = None, log: Callable[[str], None] = print):
        self.path = path or socket_path()
        self._log = log
        self.state: Optional[dict] = None
        # What the meters last read: {"channels": [[id, peak]], ...}.
        self.levels: dict = {}
        self._sock: Optional[socket.socket] = None
        self._send_lock = threading.Lock()
        self._listeners_lock = threading.Lock()
        self._listeners: list = []
        threading.Thread(target=self._run, name="pipedeck", daemon=True).start()

    # -- Listening -----------------------------------------------------------

    def listen(self, callback: Callable[[], None]) -> None:
        """Call `callback` after every change, when Pipedeck comes or goes,
        and when the meters read again.

        A bound method is held weakly, so an action that is gone stops being
        told without having to say so.
        """
        if hasattr(callback, "__self__"):
            ref = weakref.WeakMethod(callback)
        else:
            ref = lambda: callback  # noqa: E731
        with self._listeners_lock:
            self._listeners.append(ref)

    def _tell(self) -> None:
        with self._listeners_lock:
            alive = []
            callbacks = []
            for ref in self._listeners:
                callback = ref()
                if callback is not None:
                    alive.append(ref)
                    callbacks.append(callback)
            self._listeners = alive
        for callback in callbacks:
            try:
                callback()
            except Exception as e:  # one broken key must not stop the others
                self._log(f"pipedeck: a listener failed: {e}")

    # -- Telling -------------------------------------------------------------

    def do(self, action: dict) -> bool:
        """Send an order, as the `do` of a message. Says whether it went."""
        line = (json.dumps({"do": action}) + "\n").encode()
        with self._send_lock:
            if self._sock is None:
                return False
            try:
                self._sock.sendall(line)
                return True
            except OSError:
                return False

    # -- The connection ------------------------------------------------------

    def _run(self) -> None:
        while True:
            try:
                self._follow()
            except OSError:
                pass
            with self._send_lock:
                self._sock = None
            if self.state is not None:
                self.state = None
                self._tell()
            time.sleep(self.RETRY)

    def _follow(self) -> None:
        if not trusted(self.path):
            raise PermissionError(f"{self.path} is not in a folder of this user's")
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.connect(self.path)
            sock.sendall(b'{"subscribe": true}\n{"meters": true}\n')
            with self._send_lock:
                self._sock = sock
            for line in sock.makefile("r", encoding="utf-8"):
                try:
                    message = json.loads(line)
                except ValueError:
                    continue
                if "state" in message:
                    self.state = message["state"]
                    self._tell()
                elif "levels" in message:
                    self.levels = message["levels"]
                    self._tell()
                elif "error" in message:
                    self._log(f"pipedeck: {message['error']}")
        finally:
            sock.close()

    # -- Reading the state ---------------------------------------------------

    def find(self, target: Optional[dict]) -> Optional[dict]:
        """What a target is now: its name, look, level, mute and whether it
        is heard, or None when it is not there (or Pipedeck is not
        running). A channel's level in a mix says which mix as `within`."""
        state = self.state
        if state is None or not target:
            return None
        what = target.get("what")
        found = {"input": False, "mix": False, "listening": False, "within": None}
        if what == "channel":
            channel = _by(state["channels"], "id", target.get("id"))
            return channel and dict(found, **channel)
        if what == "mix":
            mix = _by(state["mixes"], "id", target.get("id"))
            return mix and dict(found, **mix, mix=True)
        if what == "cell":
            cell = next(
                (
                    c
                    for c in state["cells"]
                    if c["channel"] == target.get("channel") and c["mix"] == target.get("mix")
                ),
                None,
            )
            channel = _by(state["channels"], "id", target.get("channel"))
            mix = _by(state["mixes"], "id", target.get("mix"))
            if cell is None or channel is None or mix is None:
                return None
            return dict(
                found,
                name=channel["name"],
                icon=channel.get("icon"),
                input=channel["input"],
                volume=cell["volume"],
                muted=cell["muted"],
                within=mix,
            )
        if what == "voice":
            channel = _by(state["channels"], "id", target.get("channel"))
            voice = channel and _by(channel["voices"], "user", target.get("user"))
            return voice and dict(found, **voice, icon="people")
        if what == "output":
            device = _by(state["outputs"], "name", target.get("device"))
            if not device:
                return None
            return dict(
                found,
                name=device["description"],
                icon="headset",
                volume=0.0,
                muted=False,
                listening=state.get("listen") == device["name"],
            )
        return None

    def meter(self, target: Optional[dict]) -> float:
        """What a target's meter reads: a channel's own, or its level in a
        mix, which is measured before the mix; a person's; a mix's."""
        levels = self.levels or {}
        what = (target or {}).get("what")
        if what in ("channel", "cell"):
            key = target.get("id", target.get("channel"))
            return next((p for i, p in levels.get("channels", []) if i == key), 0.0)
        if what == "mix":
            return next((p for i, p in levels.get("mixes", []) if i == target.get("id")), 0.0)
        if what == "voice":
            return next(
                (p for c, u, p in levels.get("voices", []) if c == target.get("channel") and u == target.get("user")),
                0.0,
            )
        return 0.0


def _by(items: list, key: str, value) -> Optional[dict]:
    return next((item for item in items if item.get(key) == value), None)


if __name__ == "__main__":
    pipedeck = Pipedeck()
    pipedeck.listen(lambda: print(json.dumps(pipedeck.state, ensure_ascii=False)))
    while True:
        time.sleep(3600)
