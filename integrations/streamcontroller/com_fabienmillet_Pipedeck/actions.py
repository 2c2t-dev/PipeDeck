"""The keys and dials: what each sends Pipedeck, and what it shows.

Every action aims at one thing in the mixer, picked in its settings from
what Pipedeck has now, and kept by id: a channel renamed is still the same
key. It shows that thing as Pipedeck says it is, whoever changed it.
"""

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, GLib, Gtk  # noqa: E402
from loguru import logger as log  # noqa: E402

from src.backend.DeckManagement.InputIdentifier import Input  # noqa: E402
from src.backend.PluginManager.ActionCore import ActionCore  # noqa: E402
from src.backend.PluginManager.EventAssigner import EventAssigner  # noqa: E402

from . import draw  # noqa: E402

GONE = [160, 160, 160, 255]
RED = [237, 51, 59, 255]


class PipedeckAction(ActionCore):
    """What every Pipedeck key has: a target, picked in its settings, and a
    picture kept up with the mixer."""

    # The kinds of thing this action can aim at.
    KINDS: tuple = ()
    # What the picker says before anything is picked.
    PICK = "Pick what this controls"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.has_configuration = True
        self.pipedeck = self.plugin_base.pipedeck
        self.pipedeck.listen(self.changed)
        self._picker = None
        # What the picture was last drawn from, so a change elsewhere in the
        # mixer does not redraw every key.
        self._shown = None
        self.create_event_assigners()

    def create_event_assigners(self) -> None:
        pass

    # -- The target ----------------------------------------------------------

    def target(self):
        return self.get_settings().get("target")

    def send(self, **change) -> None:
        target = self.target()
        if not target or not self.pipedeck.do(dict(target, **change)):
            self.show_error(1)

    # -- Drawing -------------------------------------------------------------

    def changed(self) -> None:
        # Told from the connection's thread; drawn on the main one.
        GLib.idle_add(self.redraw, False)

    def on_ready(self) -> None:
        self.redraw()

    def on_update(self) -> None:
        self.redraw()

    def redraw(self, always: bool = True) -> bool:
        if not self.on_ready_called or not self.get_is_present():
            return False
        settings = self.get_settings()
        shown = (self.pipedeck.state is None, repr(settings), repr(self.pipedeck.find(settings.get("target"))))
        if shown == self._shown and not always:
            return False
        self._shown = shown
        try:
            self.draw()
        except Exception as e:
            log.exception(f"pipedeck: cannot draw {self.action_id}: {e}")
        return False

    def draw(self) -> None:
        settings = self.get_settings()
        found = self.pipedeck.find(settings.get("target"))
        if found is None:
            # Not running, nothing picked, or picked and since removed.
            if self.pipedeck.state is None:
                why = "Offline"
            elif settings.get("target"):
                why = "Gone"
            else:
                why = "Pick one"
            self.picture(icon=draw.look(None)[0], color=draw.GREY, lit=False)
            self.caption(settings.get("label", ""), why, GONE)
            return
        self.show(found)

    def show(self, found: dict) -> None:
        raise NotImplementedError

    def picture(self, icon, color, level=None, muted=False, lit=True) -> None:
        size = self.get_input().get_image_size()
        if not size or not size[0]:
            return
        self.set_media(image=draw.render(size, icon, color, level, muted, lit), size=1.0, update=False)

    def caption(self, top: str, bottom: str, bottom_color=None) -> None:
        width = (self.get_input().get_image_size() or (72, 72))[0]
        self.set_top_label(shorten(top, max(width // 8, 6)), font_size=11, update=False)
        self.set_bottom_label(bottom, color=bottom_color, font_size=11, update=True)

    def look_of(self, found: dict):
        """The badge an object wears: a mix white, anything else in colour."""
        icon, color = draw.look(found.get("icon"), found.get("input", False))
        if self.target().get("what") == "mix":
            color = draw.WHITE
        return icon, color

    # -- Settings ------------------------------------------------------------

    def get_config_rows(self) -> list:
        model = Gtk.StringList()
        self._picker = Adw.ComboRow(model=model, title="Controls")
        self._choices = []
        settings = self.get_settings()
        saved = settings.get("target")
        choices = self.pipedeck.targets(self.KINDS)
        if saved and not any(target == saved for target, _ in choices):
            # Kept while Pipedeck is away, or the thing gone, so it is not
            # lost by opening the settings.
            choices.append((saved, settings.get("label") or "(gone)"))
        if not choices:
            choices = [(None, "Start Pipedeck to pick" if self.pipedeck.state is None else "Nothing to pick")]
        elif not saved:
            choices.insert(0, (None, self.PICK))
        for target, label in choices:
            model.append(label)
            self._choices.append((target, label))
        selected = next((i for i, (target, _) in enumerate(self._choices) if target == saved), 0)
        self._picker.set_selected(selected)
        self._picker.connect("notify::selected", self.on_pick)
        return [self._picker] + self.more_rows()

    def more_rows(self) -> list:
        return []

    def on_pick(self, row, *_):
        index = row.get_selected()
        if index >= len(self._choices):
            return
        target, label = self._choices[index]
        if target is None:
            return
        settings = self.get_settings()
        settings["target"] = target
        settings["label"] = label
        self.set_settings(settings)
        self.redraw()


def shorten(text: str, length: int) -> str:
    return text if len(text) <= length else text[: length - 1] + "…"


class Mute(PipedeckAction):
    KINDS = ("channel", "mix", "cell", "voice")

    def create_event_assigners(self) -> None:
        self.add_event_assigner(
            EventAssigner(
                id="toggle-mute",
                ui_label="Mute or unmute",
                default_events=[
                    Input.Key.Events.DOWN,
                    Input.Dial.Events.DOWN,
                    Input.Dial.Events.SHORT_TOUCH_PRESS,
                ],
                callback=lambda data=None: self.send(mute="toggle"),
            )
        )

    def show(self, found: dict) -> None:
        icon, color = self.look_of(found)
        self.picture(icon, color, muted=found["muted"])
        self.caption(found["name"], "Muted" if found["muted"] else "", RED)


class Volume(PipedeckAction):
    """A key moves the level by its step; a dial moves it either way and
    mutes when pressed."""

    KINDS = ("channel", "mix", "cell", "voice")

    def create_event_assigners(self) -> None:
        self.add_event_assigner(
            EventAssigner(
                id="step",
                ui_label="Move by the step",
                default_event=Input.Key.Events.DOWN,
                callback=lambda data=None: self.nudge(self.step()),
            )
        )
        self.add_event_assigner(
            EventAssigner(
                id="up",
                ui_label="Turn up",
                default_event=Input.Dial.Events.TURN_CW,
                callback=lambda data=None: self.nudge(abs(self.step())),
            )
        )
        self.add_event_assigner(
            EventAssigner(
                id="down",
                ui_label="Turn down",
                default_event=Input.Dial.Events.TURN_CCW,
                callback=lambda data=None: self.nudge(-abs(self.step())),
            )
        )
        self.add_event_assigner(
            EventAssigner(
                id="toggle-mute",
                ui_label="Mute or unmute",
                default_events=[Input.Dial.Events.DOWN, Input.Dial.Events.SHORT_TOUCH_PRESS],
                callback=lambda data=None: self.send(mute="toggle"),
            )
        )

    def step(self) -> int:
        return int(self.get_settings().get("step", 5))

    def nudge(self, percent: int) -> None:
        self.send(nudge=percent / 100)

    def show(self, found: dict) -> None:
        icon, color = self.look_of(found)
        self.picture(icon, color, level=found["volume"], muted=found["muted"])
        if found["muted"]:
            self.caption(found["name"], "Muted", RED)
        else:
            self.caption(found["name"], draw.percent(found["volume"]))

    def more_rows(self) -> list:
        row = Adw.SpinRow.new_with_range(-100, 100, 1)
        row.set_title("Step")
        row.set_subtitle("How far a press moves the level, in percent; below zero lowers it. A dial turns by as much either way.")
        row.set_value(self.step())
        row.connect("notify::value", self.on_step)
        return [row]

    def on_step(self, row, *_):
        settings = self.get_settings()
        settings["step"] = int(row.get_value())
        self.set_settings(settings)


class Hear(PipedeckAction):
    """Which mix is heard in the headphones: this one alone, the way a
    monitor mix is switched, or this one on and off beside the others."""

    KINDS = ("mix",)
    PICK = "Pick a mix"

    def create_event_assigners(self) -> None:
        self.add_event_assigner(
            EventAssigner(
                id="hear",
                ui_label="Hear this mix",
                default_events=[Input.Key.Events.DOWN, Input.Dial.Events.DOWN],
                callback=self.on_press,
            )
        )

    def only(self) -> bool:
        return bool(self.get_settings().get("only", True))

    def on_press(self, data=None) -> None:
        target = self.target()
        if not target:
            self.show_error(1)
            return
        if self.only():
            action = {"what": "hear", "mix": target["id"], "only": True}
        else:
            action = {"what": "hear", "mix": target["id"], "listening": "toggle"}
        if not self.pipedeck.do(action):
            self.show_error(1)

    def show(self, found: dict) -> None:
        icon, color = self.look_of(found)
        self.picture(icon, color, lit=found["listening"])
        self.caption(found["name"], "Heard" if found["listening"] else "", None)

    def more_rows(self) -> list:
        row = Adw.SwitchRow(title="Only this mix", subtitle="Stop hearing the other mixes. Off, a press turns this one on or off.")
        row.set_active(self.only())
        row.connect("notify::active", self.on_only)
        return [row]

    def on_only(self, row, *_):
        settings = self.get_settings()
        settings["only"] = row.get_active()
        self.set_settings(settings)


class Output(PipedeckAction):
    """The device the mixes are heard on: headphones or speakers."""

    KINDS = ("output",)
    PICK = "Pick a device"

    def create_event_assigners(self) -> None:
        self.add_event_assigner(
            EventAssigner(
                id="listen-here",
                ui_label="Listen on this device",
                default_events=[Input.Key.Events.DOWN, Input.Dial.Events.DOWN],
                callback=self.on_press,
            )
        )

    def on_press(self, data=None) -> None:
        target = self.target()
        if not target or not self.pipedeck.do({"what": "listen", "device": target["device"]}):
            self.show_error(1)

    def show(self, found: dict) -> None:
        icon, color = draw.look("headset")
        self.picture(icon, color, lit=found["listening"])
        self.caption(found["name"], "Listening" if found["listening"] else "", None)
