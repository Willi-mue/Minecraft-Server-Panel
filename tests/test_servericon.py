import re
import struct
import zlib
from pathlib import Path

import pytest

from app import instances, servericon

SVG = Path(__file__).parent.parent / "app" / "static" / "map" / "logo.svg"


def test_logo_png_matches_the_svg():
    svg = SVG.read_text(encoding="utf-8")
    rects = re.findall(r'<rect x="(\d+)" y="(\d+)" width="(\d+)" height="(\d+)" fill="(#[0-9a-f]{6})"/>', svg)
    assert [(int(x), int(y), int(w), int(h), c) for x, y, w, h, c in rects] == servericon.LOGO_RECTS
    bg = re.search(r'<rect width="16" height="16" rx="(\d+)" fill="(#[0-9a-f]{6})"/>', svg)
    assert (int(bg.group(1)), bg.group(2)) == servericon.LOGO_BG


def _pixels(png: bytes) -> tuple[int, int, bytes]:
    w, h = struct.unpack(">II", png[16:24])
    idat = png[png.index(b"IDAT") + 4:]
    raw = zlib.decompress(idat[:struct.unpack(">I", png[png.index(b"IDAT") - 4:png.index(b"IDAT")])[0]])
    rows = [raw[y * (w * 4 + 1) + 1:(y + 1) * (w * 4 + 1)] for y in range(h)]  # filter 0 only
    return w, h, b"".join(rows)


def test_default_icon_is_a_64px_png_with_the_logo():
    png = servericon.default_png()
    servericon.validate(png)
    w, h, px = _pixels(png)
    assert (w, h) == (64, 64)
    at = lambda x, y: tuple(px[(y * 64 + x) * 4:(y * 64 + x) * 4 + 4])
    assert at(0, 0)[3] == 0                               # rounded corner: transparent
    assert at(32, 18) == (0x54, 0x5a, 0x61, 255)           # stone between the nuggets (grid 8, 4)
    assert at(14, 13) == (0xf2, 0xa8, 0x78, 255)           # bare copper highlight (grid 3, 3)
    assert at(42, 46) == (0x4c, 0xbf, 0x9f, 255)           # verdigris nugget (grid 10, 11)


def test_validate_rejects_other_files():
    with pytest.raises(ValueError, match="PNG"):
        servericon.validate(b"GIF89a" + b"\0" * 40)
    big = servericon._png(128, 128, bytes(128 * 128 * 4))
    with pytest.raises(ValueError, match="64×64"):
        servericon.validate(big)
    with pytest.raises(ValueError, match="too big"):
        servericon.validate(servericon.default_png() + b"\0" * servericon.MAX_BYTES)


def test_ensure_save_reset(data):
    inst = instances.Instance("main")
    assert servericon.read(inst) == servericon.default_png()  # nothing on disk yet
    servericon.ensure(inst)
    assert servericon.path(inst).read_bytes() == servericon.default_png()
    own = servericon._png(64, 64, bytes([200, 10, 10, 255]) * 64 * 64)
    servericon.save(inst, own)
    servericon.ensure(inst)  # keeps the user's icon
    assert servericon.read(inst) == own
    servericon.reset(inst)
    assert servericon.read(inst) == servericon.default_png()
