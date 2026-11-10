"""The server list icon (`server-icon.png`, 64×64 PNG in the server folder, read by Minecraft at
start). Without one the panel puts its logo there; users can upload their own."""

import struct
import zlib
from functools import lru_cache
from pathlib import Path

from .instances import Instance

SIZE = 64
MAX_BYTES = 256 * 1024
FILE = "server-icon.png"
_PNG_SIG = b"\x89PNG\r\n\x1a\n"

# The logo (app/static/map/logo.svg) on its 16×16 grid: background with rounded corners, then
# rectangles (x, y, w, h, colour) in drawing order. A test checks that both stay the same.
LOGO_BG = (3, "#545a61")  # corner radius, colour
LOGO_RECTS = [
    (7, 3, 1, 1, "#646b73"), (13, 7, 1, 2, "#646b73"), (2, 6, 1, 1, "#464b52"), (7, 8, 2, 1, "#646b73"),
    (6, 13, 2, 1, "#464b52"), (13, 13, 1, 1, "#646b73"), (8, 6, 1, 1, "#464b52"), (3, 3, 3, 1, "#d9824f"),
    (3, 4, 4, 1, "#d9824f"), (4, 5, 2, 1, "#d9824f"), (3, 3, 2, 1, "#f2a878"), (6, 4, 1, 1, "#a85f35"),
    (5, 5, 1, 1, "#a85f35"), (9, 2, 3, 1, "#c08a5c"), (9, 3, 4, 1, "#c08a5c"), (10, 4, 2, 1, "#c08a5c"),
    (9, 2, 1, 1, "#dba277"), (12, 3, 1, 1, "#8f6440"), (11, 4, 1, 1, "#8f6440"), (12, 3, 1, 1, "#7cbaa0"),
    (2, 9, 3, 1, "#9ea47a"), (2, 10, 4, 1, "#9ea47a"), (3, 11, 2, 1, "#9ea47a"), (2, 9, 1, 1, "#bcc096"),
    (5, 10, 1, 1, "#727a58"), (4, 11, 1, 1, "#727a58"), (4, 10, 1, 1, "#6fb59a"), (9, 10, 3, 1, "#4cbf9f"),
    (9, 11, 4, 1, "#4cbf9f"), (10, 12, 2, 1, "#4cbf9f"), (9, 10, 2, 1, "#7cdcbe"), (12, 11, 1, 1, "#2f8f74"),
    (11, 12, 1, 1, "#2f8f74"),
]


def _rgb(hex_colour: str) -> tuple[int, int, int]:
    return tuple(int(hex_colour[i:i + 2], 16) for i in (1, 3, 5))


def _png(width: int, height: int, rgba: bytes) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = b"".join(b"\x00" + rgba[y * width * 4:(y + 1) * width * 4] for y in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return _PNG_SIG + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")


@lru_cache(maxsize=1)
def default_png() -> bytes:
    """The logo as a 64×64 PNG (4 pixels per grid unit, rounded corners anti-aliased)."""
    scale = SIZE // 16
    radius, bg = LOGO_BG
    r = radius * scale
    px = bytearray(SIZE * SIZE * 4)
    for y in range(SIZE):
        for x in range(SIZE):
            # rounded corners: coverage from 4×4 samples per pixel
            alpha = 255
            if (x < r or x >= SIZE - r) and (y < r or y >= SIZE - r):
                ox, oy = (r if x < r else SIZE - r), (r if y < r else SIZE - r)
                inside = sum((x + (i + .5) / 4 - ox) ** 2 + (y + (j + .5) / 4 - oy) ** 2 <= r * r
                             for i in range(4) for j in range(4))
                alpha = round(255 * inside / 16)
            colour = _rgb(bg)
            for rx, ry, rw, rh, c in LOGO_RECTS:
                if rx * scale <= x < (rx + rw) * scale and ry * scale <= y < (ry + rh) * scale:
                    colour = _rgb(c)
            i = (y * SIZE + x) * 4
            px[i:i + 4] = bytes((*colour, alpha))
    return _png(SIZE, SIZE, bytes(px))


def path(inst: Instance) -> Path:
    return inst.server_dir / FILE


def validate(data: bytes) -> None:
    """A PNG of 64×64 pixels, not too big. Raises ValueError."""
    if len(data) > MAX_BYTES:
        raise ValueError("The icon is too big (at most 256 KB)")
    if len(data) < 33 or not data.startswith(_PNG_SIG) or data[12:16] != b"IHDR":
        raise ValueError("The icon must be a PNG image")
    width, height = struct.unpack(">II", data[16:24])
    if (width, height) != (SIZE, SIZE):
        raise ValueError(f"The icon must be {SIZE}×{SIZE} pixels (this one is {width}×{height})")


def save(inst: Instance, data: bytes) -> None:
    """Store an uploaded icon (Minecraft reads it on the next start)."""
    validate(data)
    inst.server_dir.mkdir(parents=True, exist_ok=True)
    tmp = path(inst).with_suffix(".tmp")
    tmp.write_bytes(data)
    tmp.replace(path(inst))


def reset(inst: Instance) -> None:
    """Back to the panel logo."""
    save(inst, default_png())


def ensure(inst: Instance) -> None:
    """Put the panel logo in place when the server has no icon yet (called before each start)."""
    if not path(inst).exists():
        reset(inst)


def read(inst: Instance) -> bytes:
    """The icon the server shows (the default one when there is none yet)."""
    try:
        return path(inst).read_bytes()
    except OSError:
        return default_png()
