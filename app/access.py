"""Whitelist and operator (OP) management.

While the server runs, changes go through its own commands (`whitelist add`, `op`, ...)
so they take effect immediately and the server writes the JSON files itself.
While it is stopped, `whitelist.json` / `ops.json` / `server.properties` are edited directly.
"""

import hashlib
import json
import re
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

from . import config
from .versions import USER_AGENT

NAME_RE = re.compile(r"^\w{3,16}$")
PROFILE_URL = "https://api.mojang.com/users/profiles/minecraft/{}"


def validate_name(name: str) -> str:
    name = name.strip()
    if not NAME_RE.match(name):
        raise ValueError("Player names have 3-16 characters: letters, digits, underscore")
    return name


def _file(name: str) -> Path:
    return config.SERVER_DIR / name


def _read_list(name: str) -> list[dict]:
    try:
        data = json.loads(_file(name).read_text(encoding="utf-8"))
        return data if isinstance(data, list) else []
    except (OSError, ValueError):
        return []


def _write_list(name: str, entries: list[dict]) -> None:
    _file(name).write_text(json.dumps(entries, indent=2), encoding="utf-8")


# ---------- server.properties ----------


def _properties() -> list[str]:
    try:
        return _file("server.properties").read_text(encoding="utf-8").splitlines()
    except OSError:
        return []


def get_property(key: str, default: str | None = "") -> str | None:
    for line in _properties():
        k, sep, v = line.partition("=")
        if sep and k.strip() == key:
            return v.strip()
    return default


def set_property(key: str, value: str) -> None:
    lines = _properties()
    for i, line in enumerate(lines):
        k, sep, _ = line.partition("=")
        if sep and k.strip() == key:
            lines[i] = f"{key}={value}"
            break
    else:
        lines.append(f"{key}={value}")
    _file("server.properties").write_text("\n".join(lines) + "\n", encoding="utf-8")


# ---------- UUIDs ----------


def _offline_uuid(name: str) -> str:
    # Same as Java's UUID.nameUUIDFromBytes("OfflinePlayer:" + name)
    digest = bytearray(hashlib.md5(f"OfflinePlayer:{name}".encode()).digest())
    digest[6] = (digest[6] & 0x0F) | 0x30
    digest[8] = (digest[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(digest)))


def resolve(name: str) -> dict:
    """Name -> {"uuid", "name"} with the correct spelling."""
    if get_property("online-mode", "true").lower() == "false":
        return {"uuid": _offline_uuid(name), "name": name}
    req = urllib.request.Request(PROFILE_URL.format(name), headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            if resp.status == 204:
                raise LookupError(f"No Minecraft account named {name}")
            data = json.load(resp)
    except urllib.error.HTTPError as e:
        if e.code in (204, 404):
            raise LookupError(f"No Minecraft account named {name}") from e
        raise
    return {"uuid": str(uuid.UUID(data["id"])), "name": data["name"]}


# ---------- reading ----------


def state() -> dict:
    return {
        "whitelist_enabled": get_property("white-list", "false").lower() == "true",
        "whitelist": sorted((e.get("name", "?") for e in _read_list("whitelist.json")), key=str.lower),
        "ops": sorted(
            ({"name": e.get("name", "?"), "level": e.get("level", 4)} for e in _read_list("ops.json")),
            key=lambda o: o["name"].lower(),
        ),
    }


# ---------- changes while the server is stopped ----------


def offline_change(kind: str, action: str, name: str) -> str:
    """kind: "whitelist" | "ops", action: "add" | "remove". Returns a message."""
    file = "whitelist.json" if kind == "whitelist" else "ops.json"
    entries = _read_list(file)
    label = "whitelist" if kind == "whitelist" else "operators"
    if action == "remove":
        kept = [e for e in entries if e.get("name", "").lower() != name.lower()]
        if len(kept) == len(entries):
            raise LookupError(f"{name} is not on the whitelist" if kind == "whitelist" else f"{name} is not an operator")
        _write_list(file, kept)
        return f"Removed {name} from the {label}"

    if any(e.get("name", "").lower() == name.lower() for e in entries):
        raise LookupError(f"{name} is already on the whitelist" if kind == "whitelist" else f"{name} is already an operator")
    profile = resolve(name)
    entry = dict(profile)
    if kind == "ops":
        entry.update(level=4, bypassesPlayerLimit=False)
    entries.append(entry)
    _write_list(file, entries)
    return f"Added {profile['name']} to the {label}"


def mtime(file: str) -> float | None:
    try:
        return _file(file).stat().st_mtime_ns
    except OSError:
        return None


def wait_for_change(file: str, before: float | None, timeout: float = 3.0) -> None:
    """After a server command, wait until the server has rewritten the file."""
    end = time.time() + timeout
    while time.time() < end:
        if mtime(file) != before:
            return
        time.sleep(0.1)
