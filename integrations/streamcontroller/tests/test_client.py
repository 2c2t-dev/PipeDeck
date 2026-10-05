"""The plugin's connection to Pipedeck, against a Pipedeck made up here: a
socket in a folder of the test's own, answering as the mixer does."""

import json
import os
import socket
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "dev_2c2t_Pipedeck"))

import client  # noqa: E402

STATE = {
    "channels": [
        {"id": 1, "name": "Music", "icon": "music", "input": False, "volume": 0.5, "muted": False, "voices": []},
        {
            "id": 3,
            "name": "Voice chat",
            "icon": "voice",
            "input": False,
            "volume": 1.0,
            "muted": False,
            "voices": [{"user": "42", "name": "Alice", "volume": 0.8, "muted": True}],
        },
    ],
    "mixes": [{"id": 1, "name": "Personal", "icon": "headset", "volume": 1.0, "muted": False, "listening": True}],
    "cells": [{"channel": 1, "mix": 1, "volume": 0.25, "muted": True}],
    "outputs": [{"name": "alsa.phones", "description": "Headphones"}],
    "listen": "alsa.phones",
}


def wait_for(condition, timeout=5.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if condition():
            return True
        time.sleep(0.01)
    return False


class SocketPath(unittest.TestCase):
    def test_it_is_in_the_runtime_directory(self):
        with mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": "/run/user/1000"}):
            self.assertEqual(client.socket_path(), "/run/user/1000/pipedeck/control.sock")

    def test_without_one_it_is_in_the_temporary_directory(self):
        with mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": ""}):
            expected = os.path.join(tempfile.gettempdir(), "pipedeck", "control.sock")
            self.assertEqual(client.socket_path(), expected)


class Trusted(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        self.folder = os.path.join(self.home.name, "pipedeck")
        os.mkdir(self.folder)

    def tearDown(self):
        self.home.cleanup()

    def test_a_folder_of_the_users_own_and_closed_is_trusted(self):
        os.chmod(self.folder, 0o700)
        self.assertTrue(client.trusted(os.path.join(self.folder, "control.sock")))

    def test_one_open_to_others_is_not(self):
        os.chmod(self.folder, 0o755)
        self.assertFalse(client.trusted(os.path.join(self.folder, "control.sock")))

    def test_one_that_is_not_there_is_not(self):
        self.assertFalse(client.trusted(os.path.join(self.home.name, "nowhere", "control.sock")))


class Reading(unittest.TestCase):
    """What a key reads of the state, without a connection."""

    def setUp(self):
        with mock.patch.object(threading.Thread, "start"):
            self.pipedeck = client.Pipedeck(path="/nowhere/control.sock", log=lambda _: None)
        self.pipedeck.state = STATE
        self.pipedeck.levels = {"channels": [[1, 0.5]], "mixes": [[1, 0.7]], "voices": [[3, "42", 0.3]]}

    def test_a_channel_a_mix_a_cell_a_voice_and_an_output_are_found(self):
        find = self.pipedeck.find
        self.assertEqual(find({"what": "channel", "id": 1})["name"], "Music")
        self.assertTrue(find({"what": "mix", "id": 1})["mix"])
        cell = find({"what": "cell", "channel": 1, "mix": 1})
        self.assertEqual((cell["name"], cell["volume"], cell["muted"], cell["within"]["name"]), ("Music", 0.25, True, "Personal"))
        voice = find({"what": "voice", "channel": 3, "user": "42"})
        self.assertEqual((voice["name"], voice["icon"]), ("Alice", "people"))
        output = find({"what": "output", "device": "alsa.phones"})
        self.assertEqual((output["name"], output["listening"]), ("Headphones", True))

    def test_what_is_not_there_is_none(self):
        find = self.pipedeck.find
        self.assertIsNone(find({"what": "channel", "id": 9}))
        self.assertIsNone(find({"what": "cell", "channel": 3, "mix": 1}))
        self.assertIsNone(find({"what": "voice", "channel": 3, "user": "7"}))
        self.assertIsNone(find({"what": "output", "device": "alsa.gone"}))
        self.assertIsNone(find({"what": "something else"}))
        self.assertIsNone(find(None))
        self.pipedeck.state = None
        self.assertIsNone(find({"what": "channel", "id": 1}))

    def test_meters_read_by_target(self):
        meter = self.pipedeck.meter
        self.assertAlmostEqual(meter({"what": "channel", "id": 1}), 0.5)
        self.assertAlmostEqual(meter({"what": "cell", "channel": 1, "mix": 1}), 0.5)
        self.assertAlmostEqual(meter({"what": "mix", "id": 1}), 0.7)
        self.assertAlmostEqual(meter({"what": "voice", "channel": 3, "user": "42"}), 0.3)
        self.assertAlmostEqual(meter({"what": "output", "device": "alsa.phones"}), 0.0)
        self.assertAlmostEqual(meter({"what": "channel", "id": 9}), 0.0)
        self.assertAlmostEqual(meter(None), 0.0)


class Connection(unittest.TestCase):
    """The whole round: subscribing, being told, telling, losing Pipedeck."""

    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        folder = os.path.join(self.home.name, "pipedeck")
        os.mkdir(folder, 0o700)
        self.path = os.path.join(folder, "control.sock")
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(self.path)
        self.server.listen(1)
        self.retry = mock.patch.object(client.Pipedeck, "RETRY", 0.05)
        self.retry.start()

    def tearDown(self):
        self.retry.stop()
        self.server.close()
        self.home.cleanup()

    def test_it_follows_the_mixer_and_carries_orders(self):
        logged = []
        pipedeck = client.Pipedeck(path=self.path, log=logged.append)
        told = threading.Event()
        pipedeck.listen(told.set)

        conn, _ = self.server.accept()
        reader = conn.makefile("r", encoding="utf-8")
        self.assertEqual(json.loads(reader.readline()), {"subscribe": True})
        self.assertEqual(json.loads(reader.readline()), {"meters": True})

        conn.sendall((json.dumps({"state": STATE}) + "\nnot json\n").encode())
        self.assertTrue(told.wait(5))
        self.assertTrue(wait_for(lambda: pipedeck.state == STATE))
        conn.sendall((json.dumps({"levels": {"channels": [[1, 0.9]]}}) + "\n").encode())
        self.assertTrue(wait_for(lambda: abs(pipedeck.meter({"what": "channel", "id": 1}) - 0.9) < 1e-9))
        conn.sendall((json.dumps({"error": "no mix 9"}) + "\n").encode())
        self.assertTrue(wait_for(lambda: "pipedeck: no mix 9" in logged))

        self.assertTrue(pipedeck.do({"channel": 1, "mute": "toggle"}))
        self.assertEqual(json.loads(reader.readline()), {"do": {"channel": 1, "mute": "toggle"}})

        # Pipedeck goes: the state is forgotten, and an order goes nowhere.
        # A socket closes for good once the file read from it is closed too.
        reader.close()
        conn.close()
        self.assertTrue(wait_for(lambda: pipedeck.state is None))
        self.assertFalse(pipedeck.do({"channel": 1, "mute": "toggle"}))

        # And comes back.
        conn, _ = self.server.accept()
        conn.close()

    def test_a_listener_that_fails_does_not_stop_the_others(self):
        logged = []
        pipedeck = client.Pipedeck(path=self.path, log=logged.append)
        told = threading.Event()

        def broken():
            raise RuntimeError("broken key")

        pipedeck.listen(broken)
        pipedeck.listen(told.set)
        conn, _ = self.server.accept()
        conn.sendall((json.dumps({"state": STATE}) + "\n").encode())
        self.assertTrue(told.wait(5))
        self.assertTrue(wait_for(lambda: any("broken key" in line for line in logged)))
        conn.close()

    def test_a_bound_listener_is_dropped_once_its_owner_is_gone(self):
        pipedeck = client.Pipedeck(path=self.path, log=lambda _: None)

        class Key:
            calls = 0

            def changed(self):
                Key.calls += 1

        key = Key()
        pipedeck.listen(key.changed)
        del key
        conn, _ = self.server.accept()
        conn.sendall((json.dumps({"state": STATE}) + "\n").encode())
        self.assertTrue(wait_for(lambda: pipedeck.state == STATE))
        self.assertEqual(Key.calls, 0)
        conn.close()


class Untrusted(unittest.TestCase):
    def test_a_socket_in_a_folder_open_to_others_is_left_alone(self):
        with tempfile.TemporaryDirectory() as home:
            folder = os.path.join(home, "pipedeck")
            os.mkdir(folder)
            os.chmod(folder, 0o755)
            path = os.path.join(folder, "control.sock")
            server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            server.bind(path)
            server.listen(1)
            server.settimeout(0.5)
            with mock.patch.object(client.Pipedeck, "RETRY", 0.05):
                client.Pipedeck(path=path, log=lambda _: None)
                with self.assertRaises(socket.timeout):
                    server.accept()
            server.close()


if __name__ == "__main__":
    unittest.main()
