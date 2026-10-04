"""What a key, or a dial's part of the touch strip, shows: the badge the
mixer draws for the object, its level around it or beside it with its
meter, whether it is muted or heard, and its name.

Drawn as the OpenDeck plugin draws it (`integrations/opendeck/src/draw.rs`),
as SVG, rendered here by cairosvg, so a deck looks the same under either.
The looks are Pipedeck's own, from `crates/pipedeck/src/presets.rs`; the
icons are copied next to this file by `install.sh`.
"""

import base64
import io
import math
import os
from functools import lru_cache
from typing import NamedTuple, Optional

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
GREY = "#5e5c64"
RED = "#ed333b"
TEXT = "#ffffff"
FAINT = "#a0a0a0"
TRACK = "rgba(255,255,255,0.2)"
LEVEL = "rgba(255,255,255,0.9)"

# A meter's colours, by where it reads: green, then yellow from 6 dB under
# full scale, red from 1 dB under it.
METER = [(0.0, 0.794, "#57e389"), (0.794, 0.962, "#f6d32d"), (0.962, 1.0, "#ed333b")]


class Picture(NamedTuple):
    name: str
    # The badge's icon and colour.
    look: tuple
    # The icon of the mix a channel's level is taken in, in the badge's
    # corner, as Wave Link marks a level by its mix.
    corner: Optional[str] = None
    level: Optional[float] = None
    # What the meter reads, as a position from 0 to 1, when it is shown.
    meter: Optional[float] = None
    muted: bool = False
    # Drawn faint when off: a mix not heard, a device not listened on.
    dim: bool = False
    # What it is doing, and in what colour.
    below: tuple = ("", TEXT)
    # A person's picture, round, as a PNG data URL, worn in place of the
    # badge.
    avatar: Optional[str] = None


def look(key: Optional[str], is_input: bool = False, mix: bool = False) -> tuple:
    """The icon and colour an object wears, falling back as the mixer does."""
    icon, color = LOOKS.get(key or "") or LOOKS["mic" if is_input else "speaker"]
    return icon, WHITE if mix else color


def meter_position(peak: float) -> float:
    """Where a peak sits on a meter, as the mixer's own meters put it: the
    cube root spreads it as the faders are spread."""
    return max(0.0, min(1.0, peak)) ** (1 / 3)


def percent(level: float) -> str:
    return f"{round(level * 100)}%"


@lru_cache(maxsize=64)
def glyph(icon: str) -> str:
    """The outline of an icon: the path its file draws."""
    for name in (icon, "pd-speaker-symbolic"):
        try:
            with open(os.path.join(ICONS, f"{name}.svg")) as f:
                svg = f.read()
        except OSError:
            continue
        start = svg.find(' d="')
        if start >= 0:
            return svg[start + 4 : svg.index('"', start + 4)]
    return ""


def _escape(words: str) -> str:
    return (
        words.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&apos;")
    )


def _shorten(words: str, length: int) -> str:
    return words if len(words) <= length else words[: length - 1] + "…"


def _text(x, y, size, anchor, color, words) -> str:
    return (
        f'<text x="{x}" y="{y}" font-family="sans-serif" font-size="{size}" font-weight="600" '
        f'fill="{color}" text-anchor="{anchor}">{_escape(words)}</text>'
    )


def _badge(icon, color, x, y, side, muted=False, dim=False) -> str:
    fill = GREY if muted else color
    inner = side * 0.62
    pad = (side - inner) / 2
    svg = (
        f'<g opacity="{0.35 if dim else 1}"><rect x="{x}" y="{y}" width="{side}" height="{side}" '
        f'rx="{side * 0.22}" fill="{fill}"/><svg x="{x + pad}" y="{y + pad}" width="{inner}" '
        f'height="{inner}" viewBox="0 -960 960 960"><path d="{glyph(icon)}" fill="{GLYPH}"/></svg>'
    )
    if muted:
        svg += _slash(x, y, side)
    return svg + "</g>"


def _slash(x, y, side) -> str:
    x0, y0, x1, y1 = x + side * 0.12, y + side * 0.88, x + side * 0.88, y + side * 0.12
    width = side * 0.09
    return (
        f'<line x1="{x0}" y1="{y0}" x2="{x1}" y2="{y1}" stroke="{GLYPH}" stroke-width="{width * 2.2}" '
        f'stroke-linecap="round"/><line x1="{x0}" y1="{y0}" x2="{x1}" y2="{y1}" stroke="{RED}" '
        f'stroke-width="{width}" stroke-linecap="round"/>'
    )


def _badges(picture: Picture, x, y, side) -> str:
    """The badge, and the mix's in its corner when there is one; or the
    person's picture."""
    if picture.avatar:
        svg = f'<image x="{x}" y="{y}" width="{side}" height="{side}" href="{picture.avatar}"/>'
        if picture.muted or picture.dim:
            shade = 0.65 if picture.dim else 0.5
            svg += f'<circle cx="{x + side / 2}" cy="{y + side / 2}" r="{side / 2}" fill="rgba(0,0,0,{shade})"/>'
        if picture.muted:
            svg += _slash(x, y, side)
        return svg
    svg = _badge(*picture.look, x, y, side, picture.muted, picture.dim)
    if picture.corner:
        small = side * 0.5
        cx, cy = x + side - small * 0.6, y + side - small * 0.6
        svg += (
            f'<rect x="{cx - 2.5}" y="{cy - 2.5}" width="{small + 5}" height="{small + 5}" '
            f'rx="{(small + 5) * 0.22}" fill="#000000"/>'
        )
        svg += _badge(picture.corner, WHITE, cx, cy, small, False, picture.dim)
    return svg


def _arc(cx, cy, r, start_part, end_part) -> str:
    """Part of a knob's travel round `cx, cy`, from the bottom left to the
    bottom right."""

    def at(part):
        return math.radians(135 + 270 * max(0.0, min(1.0, part)))

    start, end = at(start_part), at(end_part)
    end = min(end, start + 2 * math.pi - 0.001)
    x0, y0 = cx + r * math.cos(start), cy + r * math.sin(start)
    x1, y1 = cx + r * math.cos(end), cy + r * math.sin(end)
    large = 1 if end - start > math.pi else 0
    return f"M {x0:.2f} {y0:.2f} A {r} {r} 0 {large} 1 {x1:.2f} {y1:.2f}"


def key_svg(picture: Picture) -> str:
    """A key: the name above, the badge in the middle, ringed by the level
    and its meter when there is one, and what it is doing below."""
    svg = '<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144"><rect width="144" height="144" fill="#000000"/>'
    svg += _text(72, 25, 18, "middle", TEXT, _shorten(picture.name, 13))
    side = 50 if picture.level is not None else 62
    cx, cy = 72, 76
    if picture.level is not None:
        r = side * 0.72 + 6
        svg += f'<path d="{_arc(cx, cy, r, 0, 1)}" fill="none" stroke="{TRACK}" stroke-width="7" stroke-linecap="round"/>'
        if picture.level > 0:
            fill = RED if picture.muted else LEVEL
            svg += f'<path d="{_arc(cx, cy, r, 0, picture.level)}" fill="none" stroke="{fill}" stroke-width="7" stroke-linecap="round"/>'
        if picture.meter is not None:
            for start, end, color in METER:
                if picture.meter > start:
                    svg += f'<path d="{_arc(cx, cy, r - 7, start, min(picture.meter, end))}" fill="none" stroke="{color}" stroke-width="3"/>'
    svg += _badges(picture, cx - side / 2, cy - side / 2, side)
    svg += _text(72, 136, 18, "middle", picture.below[1], picture.below[0])
    return svg + "</svg>"


def strip_svg(picture: Picture) -> str:
    """A dial's part of the touch strip: the badge on the left, the name
    and the level as a bar beside it, with the meter in it and the level as
    a handle on it, and what it is doing under them."""
    svg = '<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100"><rect width="200" height="100" fill="#000000"/>'
    svg += _badges(picture, 12, 27, 46)
    left = 70
    name = _shorten(picture.name, 14)
    words, color = picture.below
    if picture.level is None:
        svg += _text(left, 46, 16, "start", TEXT, name)
        svg += _text(left, 70, 15, "start", color, words)
        return svg + "</svg>"
    svg += _text(left, 38, 16, "start", TEXT, name)
    svg += f'<rect x="{left}" y="48" width="118" height="8" rx="4" fill="{TRACK}"/>'
    filled = 118 * max(0.0, min(1.0, picture.level))
    if picture.meter is not None:
        for start, end, meter_color in METER:
            if picture.meter > start:
                svg += f'<rect x="{left + 118 * start:.1f}" y="48" width="{118 * (min(picture.meter, end) - start):.1f}" height="8" fill="{meter_color}"/>'
        fill = RED if picture.muted else TEXT
        svg += f'<rect x="{left + filled - 2.5:.1f}" y="43" width="5" height="18" rx="2.5" fill="{fill}" stroke="#000000" stroke-width="1.5"/>'
    elif filled > 0:
        fill = RED if picture.muted else LEVEL
        svg += f'<rect x="{left}" y="48" width="{filled:.1f}" height="8" rx="4" fill="{fill}"/>'
    svg += _text(left, 78, 15, "start", color, words)
    return svg + "</svg>"


def waiting(name: str, why: str) -> Picture:
    """What a key shows while it has nothing to show, and why."""
    return Picture(name=name, look=("pd-speaker-symbolic", GREY), dim=True, below=(why, FAINT))


@lru_cache(maxsize=64)
def avatar(path: Optional[str]) -> Optional[str]:
    """A person's picture, the PNG their client fetched, made round and
    carried in a data URL; None when there is none to read."""
    if not path:
        return None
    try:
        with Image.open(path) as image:
            image = image.convert("RGBA")
    except OSError:
        return None
    # Drawn four times larger and brought down, for a smooth edge.
    width, height = image.size
    mask = Image.new("L", (width * 4, height * 4), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, width * 4 - 1, height * 4 - 1), fill=255)
    mask = mask.resize((width, height), Image.LANCZOS)
    image.putalpha(Image.composite(image.getchannel("A"), Image.new("L", image.size, 0), mask))
    out = io.BytesIO()
    image.save(out, format="PNG")
    return "data:image/png;base64," + base64.b64encode(out.getvalue()).decode()


def render(svg: str, size: tuple) -> Image.Image:
    png = cairosvg.svg2png(bytestring=svg.encode(), output_width=size[0], output_height=size[1])
    return Image.open(io.BytesIO(png)).convert("RGBA")


def picture(picture: Picture, size: tuple) -> Image.Image:
    """The picture for an input of `size`: a key's, or a strip's when it
    is wider than tall."""
    wide = size[0] > size[1] * 1.4
    return render(strip_svg(picture) if wide else key_svg(picture), size)


def badge_image(icon: str, color: str, side: int) -> Image.Image:
    """A badge alone, for the plugin's icon in StreamController's list."""
    svg = f'<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}">{_badge(icon, color, 0, 0, side)}</svg>'
    return render(svg, (side, side))


if __name__ == "__main__":
    # A sheet of what the keys look like, to check without a deck.
    sheet = Image.new("RGBA", (72 * 4 + 200, 100), "#000000")
    music = look("music")
    sheet.alpha_composite(picture(Picture("Music", music, level=0.7, meter=0.6, below=("70%", TEXT)), (72, 72)), (0, 0))
    sheet.alpha_composite(picture(Picture("Game", look("game"), level=0.4, muted=True, below=("Muted", RED)), (72, 72)), (72, 0))
    sheet.alpha_composite(picture(Picture("Stream", look("stream", mix=True), below=("Heard", TEXT)), (72, 72)), (144, 0))
    sheet.alpha_composite(picture(waiting("Chat", "Offline"), (72, 72)), (216, 0))
    sheet.alpha_composite(picture(Picture("Music", music, corner="pd-stream-symbolic", level=0.55, meter=0.8, below=("55%", TEXT)), (200, 100)), (288, 0))
    out = os.environ.get("OUT", "keys.png")
    sheet.save(out)
    print(f"saved {out}")
