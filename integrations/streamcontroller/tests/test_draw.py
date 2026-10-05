"""What the plugin draws on a key and on the touch strip, read from its SVG
and from the pictures it renders. The icons are the mixer's own, which
Pipedeck copies beside the plugin when it installs it."""

import base64
import io
import os
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ElementTree

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "dev_2c2t_Pipedeck"))

try:
    import draw
except ImportError:  # cairosvg or Pillow, which StreamController brings
    draw = None

ICONS = os.path.join(os.path.dirname(__file__), "..", "..", "..", "crates", "pipedeck", "icons")


@unittest.skipIf(draw is None, "cairosvg and Pillow are not installed")
class Drawing(unittest.TestCase):
    def setUp(self):
        draw.ICONS = ICONS
        draw.glyph.cache_clear()

    def test_a_look_falls_back_as_the_mixer_does(self):
        self.assertEqual(draw.look("music"), ("pd-music-symbolic", "#e35db5"))
        self.assertEqual(draw.look(None), draw.LOOKS["speaker"])
        self.assertEqual(draw.look("nothing", is_input=True), draw.LOOKS["mic"])
        self.assertEqual(draw.look("stream", mix=True), ("pd-stream-symbolic", draw.WHITE))

    def test_meters_and_levels_are_placed_as_the_mixers_are(self):
        self.assertEqual(draw.meter_position(0.0), 0.0)
        self.assertAlmostEqual(draw.meter_position(0.125), 0.5)
        self.assertEqual(draw.meter_position(2.0), 1.0)
        self.assertEqual(draw.meter_position(-1.0), 0.0)
        self.assertEqual(draw.percent(0.555), "56%")

    def test_words_are_escaped_and_shortened(self):
        self.assertEqual(draw._escape("""<a & "b" 'c'>"""), "&lt;a &amp; &quot;b&quot; &apos;c&apos;&gt;")
        self.assertEqual(draw._shorten("Voice chat", 20), "Voice chat")
        self.assertEqual(draw._shorten("A very long channel name", 8), "A very …")

    def test_an_icon_is_read_from_its_file_or_the_speakers(self):
        self.assertTrue(draw.glyph("pd-music-symbolic"))
        self.assertEqual(draw.glyph("pd-none-such"), draw.glyph("pd-speaker-symbolic"))

    def test_a_key_is_well_formed_svg_with_its_name_level_and_meter(self):
        picture = draw.Picture(
            "Music & more",
            draw.look("music"),
            corner="pd-stream-symbolic",
            level=0.7,
            meter=0.98,
            below=("70%", draw.TEXT),
        )
        svg = draw.key_svg(picture)
        ElementTree.fromstring(svg)
        self.assertIn("Music &amp; more", svg)
        self.assertIn("70%", svg)
        # Past the yellow and into the red, every colour of the meter shows.
        for _, _, colour in draw.METER:
            self.assertIn(colour, svg)

    def test_a_muted_key_and_a_waiting_one(self):
        muted = draw.key_svg(draw.Picture("Game", draw.look("game"), level=0.4, muted=True))
        self.assertIn(draw.RED, muted)
        waiting = draw.waiting("Chat", "Offline")
        self.assertTrue(waiting.dim)
        ElementTree.fromstring(draw.key_svg(waiting))

    def test_the_strip_with_and_without_a_level(self):
        level = draw.strip_svg(draw.Picture("Music", draw.look("music"), level=0.5, meter=0.6, below=("50%", draw.TEXT)))
        ElementTree.fromstring(level)
        self.assertIn('width="200"', level)
        plain = draw.strip_svg(draw.Picture("Stream", draw.look("stream", mix=True), below=("Heard", draw.TEXT)))
        ElementTree.fromstring(plain)
        self.assertIn("Heard", plain)

    def test_pictures_come_out_at_the_inputs_size(self):
        picture = draw.Picture("Music", draw.look("music"), level=0.5)
        self.assertEqual(draw.picture(picture, (72, 72)).size, (72, 72))
        self.assertEqual(draw.picture(picture, (200, 100)).size, (200, 100))
        self.assertEqual(draw.badge_image("pd-music-symbolic", "#e35db5", 64).size, (64, 64))

    def test_an_avatar_is_made_round(self):
        from PIL import Image

        with tempfile.TemporaryDirectory() as home:
            path = os.path.join(home, "alice.png")
            Image.new("RGBA", (64, 64), (200, 100, 50, 255)).save(path)
            url = draw.avatar(path)
            self.assertTrue(url.startswith("data:image/png;base64,"))
            round_picture = Image.open(io.BytesIO(base64.b64decode(url.split(",", 1)[1])))
            self.assertEqual(round_picture.getpixel((0, 0))[3], 0, "a corner is cut away")
            self.assertEqual(round_picture.getpixel((32, 32))[3], 255, "the middle stays")
        self.assertIsNone(draw.avatar(None))
        self.assertIsNone(draw.avatar("/nowhere/alice.png"))


if __name__ == "__main__":
    unittest.main()
