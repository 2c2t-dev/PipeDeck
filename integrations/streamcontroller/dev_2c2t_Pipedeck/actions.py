"""The keys and dials: what each sends Pipedeck, and what it shows.

The actions are Wave Link's, as its Stream Deck plugin has them, and as
the OpenDeck plugin has them too (`integrations/opendeck/src/deck.rs`):

- **Channel Level**: a channel's level, its own or in one mix, or a
  person's of a call. A key mutes it, sets it, or moves it by a step; a
  dial moves it and mutes it when pressed.
- **Mix Level**: the same for a mix.
- **Monitor Mix**: the mix heard in the headphones, one, or the other of
  two.
- **Main Output Device**: the device it is heard on, one, or the other of
  two.
- **Call Voice**, Pipedeck's own: whoever is at a place in a Discord call,
  the first, the second, and on, following the call as people come and go.
- **Channel Effect**: one effect of a channel, switched off or on.
- **Add to Channel**: an application put on a channel, or taken off it.
- **Call**: to the call's page, saying how many are in the call, or back
  to the mixer's; see `main.py` for the decks following calls by
  themselves.

Everything is kept by id, so renaming a channel does not lose its key, and
shown as Pipedeck says it is, whoever changed it.
"""

import json
import threading
import time

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, GLib, Gtk  # noqa: E402
from loguru import logger as log  # noqa: E402

from src.backend.DeckManagement.InputIdentifier import Input  # noqa: E402
from src.backend.PluginManager.ActionCore import ActionCore  # noqa: E402
from src.backend.PluginManager.EventAssigner import EventAssigner  # noqa: E402

import globals as gl  # noqa: E402

from . import draw  # noqa: E402

GONE = "(gone)"

# What a list offers before anything is picked.
PICK_ONE = "Pick one"
# A drop-down's choice changing.
SELECTED = "notify::selected"
FADES = [(0, "None"), (500, "0.5 s"), (1000, "1 s"), (2000, "2 s"), (5000, "5 s")]


def ordinal(n: int) -> str:
    """English ordinals: 1st, 2nd, 3rd, but 11th to 13th, and 21st again."""
    if 11 <= n % 100 <= 13:
        return f"{n}th"
    return f"{n}" + {1: "st", 2: "nd", 3: "rd"}.get(n % 10, "th")


# A fader on its way somewhere, by its target, so a new fade replaces it.
_fades: dict = {}
_fades_lock = threading.Lock()


def fade(pipedeck, target: dict, start: float, end: float, seconds: float) -> None:
    """Move a level from `start` to `end` over `seconds`, a step every 40 ms."""
    key = json.dumps(target, sort_keys=True)
    token = object()
    with _fades_lock:
        _fades[key] = token

    def run():
        began = time.monotonic()
        while True:
            with _fades_lock:
                if _fades.get(key) is not token:
                    return
            part = 1.0 if seconds <= 0 else min(1.0, (time.monotonic() - began) / seconds)
            pipedeck.do(dict(target, volume=start + (end - start) * part))
            if part >= 1.0:
                with _fades_lock:
                    if _fades.get(key) is token:
                        del _fades[key]
                return
            time.sleep(0.04)

    threading.Thread(target=run, name="pipedeck-fade", daemon=True).start()


class PipedeckAction(ActionCore):
    """What every Pipedeck key has: settings picked from the mixer, and a
    picture kept up with it."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.has_configuration = True
        self.pipedeck = self.plugin_base.pipedeck
        self.pipedeck.listen(self.changed)
        # What the picture was last drawn from, so a change elsewhere in the
        # mixer, or a meter elsewhere, does not redraw every key.
        self._shown = None
        self.create_event_assigners()

    def create_event_assigners(self) -> None:
        """Each action says what its key and dial do."""

    def on_dial(self) -> bool:
        return isinstance(self.input_ident, Input.Dial)

    def settings(self) -> dict:
        return self.get_settings() or {}

    def save(self, **changes) -> None:
        settings = self.settings()
        for name, value in changes.items():
            if value is None:
                settings.pop(name, None)
            else:
                settings[name] = value
        self.set_settings(settings)
        self.redraw()

    # -- Drawing -------------------------------------------------------------

    def changed(self) -> None:
        # Told from the connection's thread; drawn on the main one.
        GLib.idle_add(self._redraw_once)

    def _redraw_once(self) -> bool:
        """Draw again if what the key shows changed; run once by GLib."""
        self.redraw(always=False)
        return GLib.SOURCE_REMOVE

    def on_ready(self) -> None:
        self.redraw()

    def on_update(self) -> None:
        self.redraw()

    def redraw(self, always: bool = True) -> None:
        if not self.on_ready_called or not self.get_is_present():
            return
        try:
            size = self.get_input().get_image_size()
            if not size or not size[0]:
                return
            picture = self.now()
            if (picture, size) == self._shown and not always:
                return
            self._shown = (picture, size)
            self.set_media(image=draw.picture(picture, size), size=1.0)
        except Exception as e:
            log.exception(f"pipedeck: cannot draw {self.action_id}: {e}")

    def now(self) -> draw.Picture:
        """What the key shows now."""
        settings = self.settings()
        label = settings.get("label", "")
        if self.pipedeck.state is None:
            return draw.waiting(label, "Offline")
        try:
            return self.picture(settings)
        except LookupError as why:
            return draw.waiting(label, why.args[0])

    def picture(self, settings: dict) -> draw.Picture:
        """What the key shows, or LookupError saying why it cannot."""
        raise NotImplementedError

    # -- Settings ------------------------------------------------------------

    def combo(self, title: str, options: list, chosen, missing: str = GONE) -> Adw.ComboRow:
        """A list to pick from, as (value, label) pairs. What was chosen is
        kept even while the mixer does not have it, so opening the settings
        loses nothing."""
        options = list(options)
        if chosen is not None and not any(value == chosen for value, _ in options):
            options.append((chosen, missing))
        if chosen is None:
            options.insert(0, (None, PICK_ONE))
        row = Adw.ComboRow(title=title)
        self.fill(row, options, chosen)
        return row

    @staticmethod
    def fill(row: Adw.ComboRow, options: list, chosen) -> None:
        row._options = options
        row._filling = True
        row.set_model(Gtk.StringList.new([label for _, label in options]))
        row.set_selected(next((i for i, (value, _) in enumerate(options) if value == chosen), 0))
        row._filling = False

    @staticmethod
    def picked(row: Adw.ComboRow, callback) -> None:
        """Call `callback` with the value picked, when one is."""

        def on_selected(row, *_):
            if getattr(row, "_filling", False):
                return
            index = row.get_selected()
            if index < len(row._options) and row._options[index][0] is not None:
                callback(*row._options[index])

        row.connect(SELECTED, on_selected)


class LevelAction(PipedeckAction):
    """A level: muted, set or moved by a key, turned by a dial."""

    def target(self, settings: dict):
        raise NotImplementedError

    def create_event_assigners(self) -> None:
        self.add_event_assigner(
            EventAssigner(
                id="key-press",
                ui_label="Key press (mute, set or adjust)",
                default_event=Input.Key.Events.DOWN,
                callback=lambda data=None: self.press(),
            )
        )
        self.add_event_assigner(
            EventAssigner(
                id="turn-up",
                ui_label="Turn up",
                default_event=Input.Dial.Events.TURN_CW,
                callback=lambda data=None: self.turn(1),
            )
        )
        self.add_event_assigner(
            EventAssigner(
                id="turn-down",
                ui_label="Turn down",
                default_event=Input.Dial.Events.TURN_CCW,
                callback=lambda data=None: self.turn(-1),
            )
        )
        self.add_event_assigner(
            EventAssigner(
                id="mute",
                ui_label="Mute or unmute",
                default_events=[Input.Dial.Events.DOWN, Input.Dial.Events.SHORT_TOUCH_PRESS],
                callback=lambda data=None: self.send(mute="toggle"),
            )
        )

    def step(self) -> float:
        return float(self.settings().get("step", 5)) / 100

    def send(self, **change) -> None:
        target = self.target(self.settings())
        if not target or not self.pipedeck.do(dict(target, **change)):
            self.show_error(1)

    def turn(self, way: int) -> None:
        self.send(nudge=abs(self.step()) * way)

    def press(self) -> None:
        settings = self.settings()
        mode = settings.get("mode", "mute")
        if mode == "adjust":
            self.send(nudge=self.step())
        elif mode == "set":
            target = self.target(settings)
            found = self.pipedeck.find(target)
            if not found:
                self.show_error(1)
                return
            end = max(0.0, min(100.0, float(settings.get("volume", 100)))) / 100
            fade(self.pipedeck, target, found["volume"], end, settings.get("fade", 0) / 1000)
        else:
            self.send(mute="toggle")

    def picture(self, settings: dict) -> draw.Picture:
        target = self.target(settings)
        if not target:
            raise LookupError(PICK_ONE)
        found = self.pipedeck.find(target)
        if not found:
            raise LookupError("Gone")
        meter = None
        if settings.get("display") != "volume":
            # In steps a key can show, so a meter that barely moved is not
            # drawn again.
            meter = round(draw.meter_position(self.pipedeck.meter(target)) * 40) / 40
        within = found.get("within")
        return draw.Picture(
            name=found["name"],
            look=draw.look(found.get("icon"), found["input"], found["mix"]),
            corner=within and draw.look(within.get("icon"), mix=True)[0],
            level=found["volume"],
            meter=meter,
            muted=found["muted"],
            below=("Muted", draw.RED) if found["muted"] else (draw.percent(found["volume"]), draw.TEXT),
            avatar=draw.avatar(found.get("avatar")),
        )

    def level_rows(self) -> list:
        settings = self.settings()
        display = self.combo(
            "Display",
            [("meter", "Level meter"), ("volume", "Volume only")],
            "volume" if settings.get("display") == "volume" else "meter",
        )
        self.picked(display, lambda value, _: self.save(display=value))
        rows = [display]

        mode = self.combo(
            "Key press",
            [("mute", "Mute"), ("set", "Set volume"), ("adjust", "Adjust volume")],
            settings.get("mode", "mute"),
        )
        volume = Adw.SpinRow.new_with_range(0, 100, 1)
        volume.set_title("Volume (%)")
        volume.set_value(float(settings.get("volume", 100)))
        volume.connect("notify::value", lambda row, *_: self.save(volume=int(row.get_value())))
        fade_row = self.combo("Fade", FADES, int(settings.get("fade", 0)))
        self.picked(fade_row, lambda value, _: self.save(fade=value))
        step = Adw.SpinRow.new_with_range(-100, 100, 1)
        step.set_title("Step (%)")
        step.set_value(float(settings.get("step", 5)))
        step.connect("notify::value", lambda row, *_: self.save(step=int(row.get_value())))

        def show(chosen):
            dial = self.on_dial()
            mode.set_visible(not dial)
            volume.set_visible(not dial and chosen == "set")
            fade_row.set_visible(not dial and chosen == "set")
            step.set_visible(dial or chosen == "adjust")
            step.set_subtitle(
                "How far each notch of the dial moves the level. Pressing or touching it mutes."
                if dial
                else "How far a press moves the level; below zero lowers it."
            )

        def on_mode(value, _):
            self.save(mode=value)
            show(value)

        self.picked(mode, on_mode)
        show(settings.get("mode", "mute"))
        return rows + [mode, volume, fade_row, step]


class ChannelLevel(LevelAction):
    def target(self, settings: dict):
        channel = settings.get("channel")
        if channel is None:
            return None
        if settings.get("user"):
            return {"what": "voice", "channel": channel, "user": settings["user"]}
        if settings.get("mix") is not None:
            return {"what": "cell", "channel": channel, "mix": settings["mix"]}
        return {"what": "channel", "id": channel}

    def get_config_rows(self) -> list:
        settings = self.settings()
        state = self.pipedeck.state or {"channels": [], "mixes": [], "cells": []}
        options = []
        for channel in state["channels"]:
            options.append((("c", channel["id"]), channel["name"]))
            for voice in channel["voices"]:
                options.append((("v", channel["id"], voice["user"]), f"{channel['name']} › {voice['name']}"))
        chosen = None
        if settings.get("channel") is not None:
            chosen = ("v", settings["channel"], settings["user"]) if settings.get("user") else ("c", settings["channel"])
        channel = self.combo("Channel", options, chosen, settings.get("label", GONE))
        level = Adw.ComboRow(title="Level")

        def fill_levels(channel_id, user):
            # A person of a call has one level; a channel has its own and
            # one in each mix it feeds.
            level.set_visible(channel_id is not None and not user)
            fed = [
                (mix["id"], f"In {mix['name']}")
                for mix in state["mixes"]
                if any(c["channel"] == channel_id and c["mix"] == mix["id"] for c in state["cells"])
            ]
            self.fill(level, [(None, "Main level")] + fed, self.settings().get("mix"))

        def on_channel(value, label):
            user = value[2] if value[0] == "v" else None
            self.save(channel=value[1], user=user, mix=None, label=label)
            fill_levels(value[1], user)

        def on_level(row, *_):
            if getattr(row, "_filling", False):
                return
            index = row.get_selected()
            if index < len(row._options):
                self.save(mix=row._options[index][0])

        self.picked(channel, on_channel)
        level.connect(SELECTED, on_level)
        fill_levels(settings.get("channel"), settings.get("user"))
        return [channel, level] + self.level_rows()


class MixLevel(LevelAction):
    def target(self, settings: dict):
        mix = settings.get("mix")
        return None if mix is None else {"what": "mix", "id": mix}

    def get_config_rows(self) -> list:
        settings = self.settings()
        mixes = (self.pipedeck.state or {}).get("mixes", [])
        mix = self.combo("Mix", [(m["id"], m["name"]) for m in mixes], settings.get("mix"), settings.get("label", GONE))
        self.picked(mix, lambda value, label: self.save(mix=value, label=label))
        return [mix] + self.level_rows()


class CallVoice(LevelAction):
    """A person of the call, by their place in it, from 1: the people in the
    order the mixer first met them, closing up when someone leaves."""

    def place(self, settings: dict) -> int:
        return max(1, int(settings.get("slot", 1)))

    def target(self, settings: dict):
        people = [
            (channel["id"], voice["user"])
            for channel in (self.pipedeck.state or {}).get("channels", [])
            for voice in channel["voices"]
        ]
        place = self.place(settings)
        if place > len(people):
            return None
        channel, user = people[place - 1]
        return {"what": "voice", "channel": channel, "user": user}

    def picture(self, settings: dict) -> draw.Picture:
        if self.target(settings) is None:
            # Nobody there: the place shows it is free.
            return draw.Picture(
                name=f"Person {self.place(settings)}",
                look=draw.look("people"),
                dim=True,
                below=("Empty", draw.FAINT),
            )
        return super().picture(settings)

    def get_config_rows(self) -> list:
        settings = self.settings()
        places = [(n, f"{ordinal(n)} in the call") for n in range(1, 25)]
        place = self.combo("Person", places, self.place(settings))
        self.picked(place, lambda value, _: self.save(slot=value, label=f"Person {value}"))
        return [place] + self.level_rows()


class PressAction(PipedeckAction):
    """An action a press does once, a key's or a dial's."""

    def create_event_assigners(self) -> None:
        self.add_event_assigner(
            EventAssigner(
                id="press",
                ui_label="Press",
                default_events=[
                    Input.Key.Events.DOWN,
                    Input.Dial.Events.DOWN,
                    Input.Dial.Events.SHORT_TOUCH_PRESS,
                ],
                callback=lambda data=None: self.press(),
            )
        )

    def press(self) -> None:
        order = self.pipedeck.state and self.order(self.settings())
        if not order or not self.pipedeck.do(order):
            self.show_error(1)

    def order(self, settings: dict):
        raise NotImplementedError

    def channel(self, settings: dict):
        return next(
            (c for c in (self.pipedeck.state or {}).get("channels", []) if c["id"] == settings.get("channel")),
            None,
        )

    def channel_row(self, settings: dict, then=None) -> Adw.ComboRow:
        channels = (self.pipedeck.state or {}).get("channels", [])
        row = self.combo("Channel", [(c["id"], c["name"]) for c in channels], settings.get("channel"))

        def on_channel(value, _):
            self.save(channel=value)
            if then:
                then(value)

        self.picked(row, on_channel)
        return row


class ChannelEffect(PressAction):
    """One effect of a channel, by its name, or its place when renamed."""

    def place(self, settings: dict):
        channel = self.channel(settings)
        if not channel:
            return None
        names = [e["name"] for e in channel.get("effects", [])]
        if settings.get("effect") in names:
            return channel["id"], names.index(settings["effect"])
        index = settings.get("index")
        return (channel["id"], index) if index is not None and index < len(names) else None

    def order(self, settings: dict):
        place = self.place(settings)
        return place and {"what": "effect", "channel": place[0], "index": place[1], "bypass": "toggle"}

    def picture(self, settings: dict) -> draw.Picture:
        if settings.get("channel") is None or settings.get("effect") is None:
            raise LookupError(PICK_ONE)
        place = self.place(settings)
        if not place:
            raise LookupError("Gone")
        channel = self.channel(settings)
        effect = channel["effects"][place[1]]
        off = effect["bypassed"]
        return draw.Picture(
            name=effect["name"],
            look=draw.look(channel.get("icon"), channel["input"]),
            dim=off,
            below=("Off", draw.FAINT) if off else ("On", draw.TEXT),
        )

    def get_config_rows(self) -> list:
        settings = self.settings()
        effect = Adw.ComboRow(title="Effect")

        def fill_effects(channel_id):
            channel = self.channel({"channel": channel_id}) or {}
            names = [e["name"] for e in channel.get("effects", [])]
            chosen = self.settings().get("index")
            if self.settings().get("effect") in names:
                chosen = names.index(self.settings()["effect"])
            self.fill(effect, [(None, PICK_ONE)] + list(enumerate(names)), chosen)

        def on_effect(row, *_):
            if getattr(row, "_filling", False):
                return
            index = row.get_selected()
            if index < len(row._options) and row._options[index][0] is not None:
                value, name = row._options[index]
                self.save(index=value, effect=name, label=name)

        effect.connect(SELECTED, on_effect)
        channel = self.channel_row(settings, then=lambda value: (self.save(index=None, effect=None), fill_effects(value)))
        fill_effects(settings.get("channel"))
        return [channel, effect]


# What Add to Channel is set to for the application in front, whichever it
# is when the key is pressed.
FRONT = "@front"


class AddApp(PressAction):
    """An application put on a channel; pressed again, taken off it."""

    def app(self, settings: dict):
        app = settings.get("app")
        if app == FRONT:
            front = (self.pipedeck.state or {}).get("focused")
            return front and front["key"]
        return app

    def order(self, settings: dict):
        channel, app = settings.get("channel"), self.app(settings)
        if channel is None or not app:
            return None
        here = app in (self.channel(settings) or {}).get("apps", [])
        return {"what": "app", "app": app, "channel": channel, "release": here}

    def picture(self, settings: dict) -> draw.Picture:
        if settings.get("channel") is None or not settings.get("app"):
            raise LookupError(PICK_ONE)
        channel = self.channel(settings)
        if not channel:
            raise LookupError("Gone")
        app = self.app(settings)
        if not app:
            return draw.Picture(
                name="In front",
                look=draw.look(channel.get("icon"), channel["input"]),
                dim=True,
                below=("Nothing playing", draw.FAINT),
            )
        playing = {a["key"]: a["name"] for a in self.pipedeck.state.get("apps", [])}
        here = app in channel.get("apps", [])
        return draw.Picture(
            name=playing.get(app, settings.get("label", app)),
            look=draw.look(channel.get("icon"), channel["input"]),
            dim=not here,
            below=(f"On {channel['name']}", draw.TEXT) if here else (f"To {channel['name']}", draw.FAINT),
        )

    def get_config_rows(self) -> list:
        settings = self.settings()
        state = self.pipedeck.state or {}
        # What plays now, and what is on a channel already.
        apps = {a["key"]: a["name"] for a in state.get("apps", [])}
        for channel in state.get("channels", []):
            for key in channel.get("apps", []):
                apps.setdefault(key, key)
        app = self.combo(
            "Application",
            [(FRONT, "The application in front")] + list(apps.items()),
            settings.get("app"),
            settings.get("label", GONE),
        )
        self.picked(app, lambda value, label: self.save(app=value, label=label))
        return [app, self.channel_row(settings)]


def switch_page(controller, name: str) -> bool:
    """Show the page of that name on a deck, as StreamController's own
    actions do. Says whether there was one."""
    path = gl.page_manager.find_matching_page_path(name)
    if not path:
        return False
    controller.load_page(gl.page_manager.get_page(path, controller))
    return True


class CallPage(PressAction):
    """To the call's page, or back to the mixer's, by its name."""

    def press(self) -> None:
        to = self.settings().get("to")
        if not to or not switch_page(self.deck_controller, to):
            self.show_error(1)

    def picture(self, settings: dict) -> draw.Picture:
        if settings.get("page") == "mixer":
            return draw.Picture(name="Mixer", look=("pd-listen-symbolic", draw.WHITE), below=("Back", draw.TEXT))
        people = sum(len(c["voices"]) for c in self.pipedeck.state["channels"])
        return draw.Picture(
            name="Call",
            look=draw.look("people"),
            dim=people == 0,
            below={0: ("No call", draw.FAINT), 1: ("1 person", draw.TEXT)}.get(people, (f"{people} people", draw.TEXT)),
        )

    def get_config_rows(self) -> list:
        settings = self.settings()
        pages = gl.page_manager.get_page_names()
        to = self.combo("Goes to", [(name, name) for name in pages], settings.get("to"))
        # Back to the mixer when the page it goes to is not a call's.
        self.picked(to, lambda value, _: self.save(to=value, page="call" if value.endswith(" Call") else "mixer"))
        follow = Adw.SwitchRow(
            title="Follow calls",
            subtitle="Every deck on a Pipedeck page goes to the call as one starts, and back as it ends",
        )
        follow.set_active(bool((self.plugin_base.get_settings() or {}).get("follow_call")))

        def on_follow(row, *_):
            kept = self.plugin_base.get_settings() or {}
            kept["follow_call"] = row.get_active()
            self.plugin_base.set_settings(kept)

        follow.connect("notify::active", on_follow)
        return [to, follow]


class SwitchAction(PipedeckAction):
    """One thing to switch to, or the other of two."""

    # The settings the two are kept under, and what they are called.
    FIRST, SECOND, WHAT = "", "", ""

    def create_event_assigners(self) -> None:
        self.add_event_assigner(
            EventAssigner(
                id="switch",
                ui_label="Switch",
                default_events=[
                    Input.Key.Events.DOWN,
                    Input.Dial.Events.DOWN,
                    Input.Dial.Events.SHORT_TOUCH_PRESS,
                ],
                callback=lambda data=None: self.switch(),
            )
        )

    def options(self) -> list:
        raise NotImplementedError

    def is_on(self, value) -> bool:
        raise NotImplementedError

    def order(self, value) -> dict:
        raise NotImplementedError

    def shown(self, settings: dict):
        """Of two, the one on, or the first when neither is."""
        first, second = settings.get(self.FIRST), settings.get(self.SECOND)
        if first is None:
            raise LookupError(PICK_ONE)
        if settings.get("mode") == "toggle" and second is not None and self.is_on(second) and not self.is_on(first):
            return second
        return first

    def switch(self) -> None:
        settings = self.settings()
        first, second = settings.get(self.FIRST), settings.get(self.SECOND)
        if first is None or self.pipedeck.state is None:
            self.show_error(1)
            return
        value = second if settings.get("mode") == "toggle" and second is not None and self.is_on(first) else first
        if not self.pipedeck.do(self.order(value)):
            self.show_error(1)

    def get_config_rows(self) -> list:
        settings = self.settings()
        options = self.options()
        toggle = settings.get("mode") == "toggle"
        mode = self.combo("Mode", [("select", "Select one"), ("toggle", "Toggle between two")], "toggle" if toggle else "select")
        first = self.combo(self.WHAT, options, settings.get(self.FIRST), settings.get("label", GONE))
        second = self.combo(f"Second {self.WHAT.lower()}", options, settings.get(self.SECOND))

        def show(toggled):
            first.set_title(f"First {self.WHAT.lower()}" if toggled else self.WHAT)
            second.set_visible(toggled)

        def on_mode(value, _):
            self.save(mode=value)
            show(value == "toggle")

        self.picked(mode, on_mode)
        self.picked(first, lambda value, label: self.save(**{self.FIRST: value, "label": label}))
        self.picked(second, lambda value, _: self.save(**{self.SECOND: value}))
        show(toggle)
        return [mode, first, second]


class MonitorMix(SwitchAction):
    FIRST, SECOND, WHAT = "mix", "mix2", "Mix"

    def options(self) -> list:
        return [(m["id"], m["name"]) for m in (self.pipedeck.state or {}).get("mixes", [])]

    def is_on(self, value) -> bool:
        return any(m["id"] == value and m["listening"] for m in self.pipedeck.state["mixes"])

    def order(self, value) -> dict:
        return {"what": "hear", "mix": value, "only": True}

    def picture(self, settings: dict) -> draw.Picture:
        mix = next((m for m in self.pipedeck.state["mixes"] if m["id"] == self.shown(settings)), None)
        if not mix:
            raise LookupError("Gone")
        return draw.Picture(
            name=mix["name"],
            look=draw.look(mix.get("icon"), mix=True),
            dim=not mix["listening"],
            below=("Heard", draw.TEXT) if mix["listening"] else ("Hear", draw.FAINT),
        )


class MainOutput(SwitchAction):
    FIRST, SECOND, WHAT = "device", "device2", "Device"

    def options(self) -> list:
        return [(o["name"], o["description"]) for o in (self.pipedeck.state or {}).get("outputs", [])]

    def is_on(self, value) -> bool:
        return self.pipedeck.state.get("listen") == value

    def order(self, value) -> dict:
        return {"what": "listen", "device": value}

    def picture(self, settings: dict) -> draw.Picture:
        found = self.pipedeck.find({"what": "output", "device": self.shown(settings)})
        if not found:
            raise LookupError("Gone")
        return draw.Picture(
            name=found["name"],
            look=draw.look("headset"),
            dim=not found["listening"],
            below=("Listening", draw.TEXT) if found["listening"] else ("Listen", draw.FAINT),
        )
