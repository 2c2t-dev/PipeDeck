"""Pipedeck on a Stream Deck: mute, levels, the mix heard in the headphones
and the device it is heard on, through Pipedeck's control socket."""

from gi.repository import Gtk
from loguru import logger as log

from src.backend.DeckManagement.ImageHelpers import image2pixbuf
from src.backend.DeckManagement.InputIdentifier import Input
from src.backend.PluginManager.ActionHolder import ActionHolder
from src.backend.PluginManager.ActionInputSupport import ActionInputSupport
from src.backend.PluginManager.PluginBase import PluginBase

from . import draw
from .actions import Hear, Mute, Output, Volume
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
            (Mute, "Mute", "Mute"),
            (Volume, "Volume", "Volume"),
            (Hear, "Hear", "Hear mix"),
            (Output, "Output", "Listen on"),
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

    def get_selector_icon(self) -> Gtk.Widget:
        icon = draw.badge("pd-listen-symbolic", draw.WHITE, 64)
        return Gtk.Image.new_from_pixbuf(image2pixbuf(icon))
