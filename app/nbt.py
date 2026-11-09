"""Typed NBT reader/writer that round-trips byte for byte (used to edit level.dat).

A tag is a tuple `(type, value)`:
- numbers, STRING: the Python value; BYTES: bytes; INTS/LONGS: list of int
- LIST: `(item_type, [payload, ...])` (payloads without their own type)
- COMPOUND: dict name -> tag (insertion order is kept)
"""

import gzip
import os
import shutil
import struct
from pathlib import Path

END, BYTE, SHORT, INT, LONG, FLOAT, DOUBLE, BYTES, STRING, LIST, COMPOUND, INTS, LONGS = range(13)
_NUM = {BYTE: "b", SHORT: "h", INT: "i", LONG: "q", FLOAT: "f", DOUBLE: "d"}


class _Reader:
    def __init__(self, data: bytes) -> None:
        self.data, self.pos = data, 0

    def take(self, n: int) -> bytes:
        if n < 0 or self.pos + n > len(self.data):
            raise ValueError("truncated NBT")
        chunk = self.data[self.pos:self.pos + n]
        self.pos += n
        return chunk

    def unpack(self, fmt: str):
        return struct.unpack(">" + fmt, self.take(struct.calcsize(">" + fmt)))[0]

    def string(self) -> str:
        # Java's modified UTF-8 equals UTF-8 for everything level.dat normally holds
        return self.take(self.unpack("H")).decode("utf-8", errors="surrogatepass")

    def payload(self, t: int):
        if t in _NUM:
            return self.unpack(_NUM[t])
        if t == BYTES:
            return self.take(self.unpack("i"))
        if t == STRING:
            return self.string()
        if t == LIST:
            item, count = self.unpack("b"), self.unpack("i")
            return (item, [self.payload(item) for _ in range(max(0, count))])
        if t == COMPOUND:
            result = {}
            while (child := self.unpack("b")) != END:
                name = self.string()
                result[name] = (child, self.payload(child))
            return result
        if t in (INTS, LONGS):
            n = self.unpack("i")
            return list(struct.unpack(f">{n}{'i' if t == INTS else 'q'}", self.take(n * (4 if t == INTS else 8))))
        raise ValueError(f"unknown NBT tag {t}")


def _string(out: bytearray, s: str) -> None:
    raw = s.encode("utf-8", errors="surrogatepass")
    out += struct.pack(">H", len(raw)) + raw


def _payload(out: bytearray, t: int, v) -> None:
    if t in _NUM:
        out += struct.pack(">" + _NUM[t], v)
    elif t == BYTES:
        out += struct.pack(">i", len(v)) + bytes(v)
    elif t == STRING:
        _string(out, v)
    elif t == LIST:
        item, values = v
        out += struct.pack(">bi", item, len(values))
        for x in values:
            _payload(out, item, x)
    elif t == COMPOUND:
        for name, (ct, cv) in v.items():
            out += struct.pack(">b", ct)
            _string(out, name)
            _payload(out, ct, cv)
        out += b"\x00"
    elif t in (INTS, LONGS):
        out += struct.pack(f">i{len(v)}{'i' if t == INTS else 'q'}", len(v), *v)
    else:
        raise ValueError(f"unknown NBT tag {t}")


def decode(data: bytes) -> tuple[str, dict]:
    """Uncompressed NBT -> (root name, root compound)."""
    r = _Reader(data)
    if r.unpack("b") != COMPOUND:
        raise ValueError("NBT root is not a compound")
    name = r.string()
    return name, r.payload(COMPOUND)


def encode(name: str, root: dict) -> bytes:
    out = bytearray(b"\x0a")
    _string(out, name)
    _payload(out, COMPOUND, root)
    return bytes(out)


def load(path: Path) -> tuple[str, dict]:
    return decode(gzip.decompress(path.read_bytes()))


def save(path: Path, name: str, root: dict) -> None:
    """Write gzip NBT atomically; the previous file is kept as `<name>_old` (like Minecraft does)."""
    data = gzip.compress(encode(name, root))
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_bytes(data)
    if path.exists():
        shutil.copy2(path, path.with_name(path.name + "_old"))
    os.replace(tmp, path)
