"""What a key, or a dial's part of the touch strip, shows: the badge the
mixer draws for the object, its level around it or under it, and whether
it is muted or heard.

The looks are Pipedeck's own, from `crates/pipedeck/src/presets.rs`; the
icons are copied next to this file by `install.sh`.
"""

import io
import os
from functools import lru_cache
from typing import Optional

import cairosvg
from PIL import Image, ImageDraw

ICONS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "assets", "icons")

# The look of each preset key: its icon, and the colour of a channel's badge.
LOOKS = {
    "one": ("pd-1-symbolic", "#8a93a6"),
    "two": ("pd-2-symbolic", "#8a93a6"),
    "three": ("pd-3-symbolic", "#8a93a6"),
    "four": ("pd-4-symbolic", "#8a93a6"),
    "five": ("pd-5-symbolic", "#8a93a6"),
    "music": ("pd-music-symbolic", "#e35db5"),
    "browser": ("pd-browser-symbolic", "#9b6ef3"),
    "system": ("pd-system-symbolic", "#3a8ee6"),
    "game": ("pd-game-symbolic", "#e6584f"),
    "sfx": ("pd-sfx-symbolic", "#f08a24"),
    "voice": ("pd-voice-symbolic", "#e8c33a"),
    "aux1": ("pd-aux-symbolic", "#2fb9ad"),
    "speaker": ("pd-speaker-symbolic", "#4cc26a"),
    "headset": ("pd-headset-symbolic", "#3ba7c9"),
    "mic": ("pd-mic-symbolic", "#d8594f"),
    "star": ("pd-star-symbolic", "#e0b13a"),
    "people": ("pd-people-symbolic", "#5b8cf5"),
    "video": ("pd-video-symbolic", "#a563e8"),
    "stream": ("pd-stream-symbolic", "#e35db5"),
    "record": ("pd-record-symbolic", "#e0514b"),
    "fun": ("pd-fun-symbolic", "#7ec44f"),
}

WHITE = "#f2f2f2"
GLYPH = "#12141a"
MUTED = "#e01b24"
GREY = "#5e5c64"
TRACK = (255, 255, 255, 50)
LEVEL = (255, 255, 255, 230)


def look(key: Optional[str], is_input: bool = False) -> tuple:
    """The icon and colour an object wears, falling back as the mixer does."""
    return LOOKS.get(key or "") or LOOKS["mic" if is_input else "speaker"]


@lru_cache(maxsize=64)
def glyph(icon: str, size: int, color: str) -> Image.Image:
    path = os.path.join(ICONS, f"{icon}.svg")
    try:
        with open(path) as f:
            svg = f.read()
    except OSError:
        return Image.new("RGBA", (size, size))
    svg = svg.replace("<path ", f'<path fill="{color}" ')
    png = cairosvg.svg2png(bytestring=svg.encode(), output_width=size, output_height=size)
    return Image.open(io.BytesIO(png)).convert("RGBA")


def badge(icon: str, color: str, size: int, muted: bool = False, lit: bool = True) -> Image.Image:
    """A rounded square of a colour with the icon on it, as in the mixer."""
    scale = 4  # drawn large and brought down, for smooth corners
    big = size * scale
    image = Image.new("RGBA", (big, big))
    draw = ImageDraw.Draw(image)
    fill = GREY if muted else color
    draw.rounded_rectangle((0, 0, big - 1, big - 1), radius=big * 0.22, fill=fill)
    inner = int(big * 0.62)
    mark = glyph(icon, inner, GLYPH)
    image.alpha_composite(mark, ((big - inner) // 2, (big - inner) // 2))
    if muted:
        width = max(int(big * 0.09), 1)
        draw.line((big * 0.12, big * 0.88, big * 0.88, big * 0.12), fill="#12141a", width=width + scale * 4)
        draw.line((big * 0.12, big * 0.88, big * 0.88, big * 0.12), fill=MUTED, width=width)
    image = image.resize((size, size), Image.LANCZOS)
    if not lit:
        alpha = image.getchannel("A").point(lambda a: a * 35 // 100)
        image.putalpha(alpha)
    return image


def render(
    size: tuple,
    icon: str,
    color: str,
    level: Optional[float] = None,
    muted: bool = False,
    lit: bool = True,
) -> Image.Image:
    """The whole picture for an input of `size`.

    A key shows the badge in the middle, ringed by the level when there is
    one; a dial's part of the touch strip, wider than tall, shows the badge
    on the left and the level as a bar beside it. Labels go over this.
    """
    width, height = size
    image = Image.new("RGBA", (width, height))
    if width > height * 1.4:
        side = int(height * 0.46)
        top = (height - side) // 2
        image.alpha_composite(badge(icon, color, side, muted, lit), (int(width * 0.07), top))
        if level is not None:
            draw = ImageDraw.Draw(image)
            left = int(width * 0.07) + side + int(width * 0.07)
            right = int(width * 0.93)
            thick = max(height // 12, 4)
            y = height // 2 - thick // 2
            draw.rounded_rectangle((left, y, right, y + thick), radius=thick // 2, fill=TRACK)
            filled = left + (right - left) * max(0.0, min(1.0, level))
            if filled > left + thick:
                draw.rounded_rectangle(
                    (left, y, filled, y + thick), radius=thick // 2, fill=MUTED if muted else LEVEL
                )
        return image

    # Room is left above and below for the labels.
    side = int(min(width, height) * (0.33 if level is not None else 0.5))
    image.alpha_composite(badge(icon, color, side, muted, lit), ((width - side) // 2, (height - side) // 2))
    if level is not None:
        # A ring with a gap at the bottom, like a knob's travel.
        scale = 4
        ring = Image.new("RGBA", (width * scale, height * scale))
        draw = ImageDraw.Draw(ring)
        # Clear of the badge's corners.
        reach = side * scale * 0.72 + width * scale * 0.035
        cx, cy = width * scale / 2, height * scale / 2
        box = (cx - reach, cy - reach, cx + reach, cy + reach)
        thick = max(int(width * scale * 0.045), scale)
        start, end = 135, 405
        draw.arc(box, start, end, fill=TRACK, width=thick)
        part = max(0.0, min(1.0, level))
        if part > 0:
            draw.arc(box, start, start + (end - start) * part, fill=MUTED if muted else LEVEL, width=thick)
        image.alpha_composite(ring.resize((width, height), Image.LANCZOS))
    return image


def percent(level: float) -> str:
    return f"{round(level * 100)}%"


if __name__ == "__main__":
    # A sheet of what the keys look like, to check without a deck.
    sheet = Image.new("RGBA", (72 * 4 + 200, 100), "#000000")
    sheet.alpha_composite(render((72, 72), *look("music"), level=0.7), (0, 0))
    sheet.alpha_composite(render((72, 72), *look("game"), level=0.4, muted=True), (72, 0))
    sheet.alpha_composite(render((72, 72), "pd-1-symbolic", WHITE), (144, 0))
    sheet.alpha_composite(render((72, 72), "pd-2-symbolic", WHITE, lit=False), (216, 0))
    sheet.alpha_composite(render((200, 100), *look("voice"), level=0.55), (288, 0))
    out = os.environ.get("OUT", "keys.png")
    sheet.save(out)
    print(f"saved {out}")
