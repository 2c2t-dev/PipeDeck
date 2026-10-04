"""Pipedeck's control socket, followed for as long as the deck runs.

One connection, kept open and opened again whenever Pipedeck comes back:
it subscribes to the mixer's state, which comes as a line of JSON after
every change, and carries the deck's orders the other way. The protocol is
described in `crates/pipedeck-engine/src/control.rs`.

Nothing here knows about StreamController, so it can be tried on its own:
`python3 client.py` prints the state as it changes.
"""

import json
import os
import socket
import threading
import time
import weakref

from typing import Callable, Optional


def socket_path() -> str:
    runtime = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
    return os.path.join(runtime, "pipedeck", "control.sock")


class Pipedeck:
    """The mixer as last told, and a way to tell it things."""

    # How long to wait before trying again when Pipedeck is not running.
    RETRY = 2.0

    def __init__(self, path: Optional[str] = None, log: Callable[[str], None] = print):
        self.path = path or socket_path()
        self._log = log
        self.state: Optional[dict] = None
        self._sock: Optional[socket.socket] = None
        self._send_lock = threading.Lock()
        self._listeners_lock = threading.Lock()
        self._listeners: list = []
        threading.Thread(target=self._run, name="pipedeck", daemon=True).start()

    # -- Listening -----------------------------------------------------------

    def listen(self, callback: Callable[[], None]) -> None:
        """Call `callback` after every change, and when Pipedeck comes or goes.

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
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            sock.connect(self.path)
            sock.sendall(b'{"subscribe": true}\n')
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
                elif "error" in message:
                    self._log(f"pipedeck: {message['error']}")
        finally:
            sock.close()

    # -- Reading the state ---------------------------------------------------

    def find(self, target: Optional[dict]) -> Optional[dict]:
        """What a target is now: its name, look, level and mute, or None
        when it is not there (or Pipedeck is not running)."""
        state = self.state
        if state is None or not target:
            return None
        what = target.get("what")
        if what == "channel":
            return _by(state["channels"], "id", target.get("id"))
        if what == "mix":
            return _by(state["mixes"], "id", target.get("id"))
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
                cell,
                name=f"{channel['name']} → {mix['name']}",
                icon=channel.get("icon"),
                input=channel["input"],
            )
        if what == "voice":
            channel = _by(state["channels"], "id", target.get("channel"))
            voice = channel and _by(channel["voices"], "user", target.get("user"))
            if not voice:
                return None
            return dict(voice, icon="people")
        if what == "output":
            device = _by(state["outputs"], "name", target.get("device"))
            if not device:
                return None
            return {
                "name": device["description"],
                "icon": "headset",
                "listening": state.get("listen") == device["name"],
            }
        return None

    def targets(self, kinds: tuple) -> list:
        """Everything that can be aimed at, of the kinds asked, as
        (target, label) pairs in the mixer's order."""
        state = self.state
        if state is None:
            return []
        found = []
        if "channel" in kinds:
            for channel in state["channels"]:
                found.append(({"what": "channel", "id": channel["id"]}, f"Channel · {channel['name']}"))
        if "mix" in kinds:
            for mix in state["mixes"]:
                found.append(({"what": "mix", "id": mix["id"]}, f"Mix · {mix['name']}"))
        if "cell" in kinds:
            names = {c["id"]: c["name"] for c in state["channels"]}
            mixes = {m["id"]: m["name"] for m in state["mixes"]}
            for cell in state["cells"]:
                found.append(
                    (
                        {"what": "cell", "channel": cell["channel"], "mix": cell["mix"]},
                        f"{names.get(cell['channel'], '?')} → {mixes.get(cell['mix'], '?')}",
                    )
                )
        if "voice" in kinds:
            for channel in state["channels"]:
                for voice in channel["voices"]:
                    found.append(
                        (
                            {"what": "voice", "channel": channel["id"], "user": voice["user"]},
                            f"Voice · {voice['name']} ({channel['name']})",
                        )
                    )
        if "output" in kinds:
            for device in state["outputs"]:
                found.append(({"what": "output", "device": device["name"]}, device["description"]))
        return found


def _by(items: list, key: str, value) -> Optional[dict]:
    return next((item for item in items if item.get(key) == value), None)


if __name__ == "__main__":
    pipedeck = Pipedeck()
    pipedeck.listen(lambda: print(json.dumps(pipedeck.state, ensure_ascii=False)))
    while True:
        time.sleep(3600)
