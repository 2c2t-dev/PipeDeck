"""Pipedeck on a Stream Deck, the way Elgato's Wave Link plugin puts Wave
Link there: channel and mix levels with their meters, the mix heard in the
headphones and the device it is heard on, through Pipedeck's control
socket."""

from gi.repository import GLib, Gtk
from loguru import logger as log

import globals as gl

from src.backend.DeckManagement.ImageHelpers import image2pixbuf
from src.backend.DeckManagement.InputIdentifier import Input
from src.backend.PluginManager.ActionHolder import ActionHolder
from src.backend.PluginManager.ActionInputSupport import ActionInputSupport
from src.backend.PluginManager.PluginBase import PluginBase

from . import draw
from .actions import (
    AddApp,
    CallPage,
    CallVoice,
    ChannelEffect,
    ChannelLevel,
    MainOutput,
    MixLevel,
    MonitorMix,
    switch_page,
)
from .client import Pipedeck

KEYS_AND_DIALS = {
    Input.Key: ActionInputSupport.SUPPORTED,
    Input.Dial: ActionInputSupport.SUPPORTED,
    Input.Touchscreen: ActionInputSupport.UNSUPPORTED,
}


class PipedeckPlugin(PluginBase):
    def __init__(self):
        super().__init__(use_legacy_locale=False)
        self.pipedeck = Pipedeck(log=log.warning)

        for action, suffix, name in [
            (ChannelLevel, "ChannelLevel", "Channel Level"),
            (MixLevel, "MixLevel", "Mix Level"),
            (MonitorMix, "MonitorMix", "Monitor Mix"),
            (MainOutput, "MainOutput", "Main Output Device"),
            (CallVoice, "CallVoice", "Call Voice"),
            (ChannelEffect, "ChannelEffect", "Channel Effect"),
            (AddApp, "AddApp", "Add to Channel"),
            (CallPage, "CallPage", "Call"),
        ]:
            self.add_action_holder(
                ActionHolder(
                    plugin_base=self,
                    action_core=action,
                    action_id_suffix=suffix,
                    action_name=name,
                    action_support=KEYS_AND_DIALS,
                )
            )

        self.register()

        # How many were in the call when last told, to tell a call starting
        # or ending; None until Pipedeck has said.
        self._people = None
        self.pipedeck.listen(self.on_mixer)

    def on_mixer(self) -> None:
        state = self.pipedeck.state
        people = sum(len(c["voices"]) for c in state["channels"]) if state else None
        before, self._people = self._people, people
        if before is None or people is None or (before == 0) == (people == 0):
            return
        if (self.get_settings() or {}).get("follow_call"):
            GLib.idle_add(self.follow, people > 0)

    def follow(self, started: bool) -> bool:
        """Take every deck on a Pipedeck page to the call's as a call
        starts, and back to the mixer's as it ends. A deck on a page of the
        user's own is left there."""
        for controller in gl.deck_manager.deck_controller:
            page = controller.active_page
            name = page.get_name() if page else ""
            if not name.startswith("Pipedeck"):
                continue
            if started and not name.endswith(" Call"):
                switch_page(controller, f"{name} Call")
            elif not started and name.endswith(" Call"):
                switch_page(controller, name[: -len(" Call")])
        return False

    def get_selector_icon(self) -> Gtk.Widget:
        icon = draw.badge_image("pd-listen-symbolic", draw.WHITE, 64)
        return Gtk.Image.new_from_pixbuf(image2pixbuf(icon))
