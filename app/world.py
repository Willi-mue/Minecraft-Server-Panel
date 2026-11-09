"""Read the Minecraft version a world was last saved with, from `level.dat`.

level.dat is a gzip-compressed NBT file; the version lives at Data.Version.Name (since 1.9).
This is a minimal NBT reader, no extra dependency needed.
"""

import gzip
import struct
from pathlib import Path

from . import backup, config

# NBT tag ids
_END, _BYTE, _SHORT, _INT, _LONG, _FLOAT, _DOUBLE, _BYTES, _STRING, _LIST, _COMPOUND, _INTS, _LONGS = range(13)


class _Reader:
    def __init__(self, data: bytes) -> None:
        self.data, self.pos = data, 0

    def take(self, n: int) -> bytes:
        if self.pos + n > len(self.data):
            raise ValueError("truncated NBT")
        chunk = self.data[self.pos:self.pos + n]
        self.pos += n
        return chunk

    def unpack(self, fmt: str):
        return struct.unpack(">" + fmt, self.take(struct.calcsize(">" + fmt)))[0]

    def string(self) -> str:
        return self.take(self.unpack("H")).decode("utf-8", errors="replace")

    def payload(self, tag: int):
        if tag == _BYTE:
            return self.unpack("b")
        if tag == _SHORT:
            return self.unpack("h")
        if tag == _INT:
            return self.unpack("i")
        if tag == _LONG:
            return self.unpack("q")
        if tag == _FLOAT:
            return self.unpack("f")
        if tag == _DOUBLE:
            return self.unpack("d")
        if tag == _BYTES:
            return self.take(self.unpack("i"))
        if tag == _STRING:
            return self.string()
        if tag == _LIST:
            item, count = self.unpack("b"), self.unpack("i")
            return [self.payload(item) for _ in range(max(0, count))]
        if tag == _COMPOUND:
            result = {}
            while (child := self.unpack("b")) != _END:
                name = self.string()
                result[name] = self.payload(child)
            return result
        if tag == _INTS:
            return [self.unpack("i") for _ in range(self.unpack("i"))]
        if tag == _LONGS:
            self.take(8 * self.unpack("i"))  # contents not needed
            return []
        raise ValueError(f"unknown NBT tag {tag}")


def parse_nbt(data: bytes) -> dict:
    r = _Reader(data)
    if r.unpack("b") != _COMPOUND:
        raise ValueError("NBT root is not a compound")
    r.string()  # root name
    return r.payload(_COMPOUND)


def level_dat() -> Path:
    return config.SERVER_DIR / backup.level_name() / "level.dat"


def saved_version() -> str | None:
    """Minecraft version that last saved the world, or None if unknown / no world."""
    try:
        root = parse_nbt(gzip.decompress(level_dat().read_bytes()))
        name = root.get("Data", {}).get("Version", {}).get("Name")
        return name if isinstance(name, str) else None
    except (OSError, ValueError, EOFError, gzip.BadGzipFile, struct.error):
        return None
