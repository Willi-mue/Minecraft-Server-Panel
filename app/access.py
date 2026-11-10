"""Whitelist, operator (OP) and ban management.

While the server runs, changes go through its own commands (`whitelist add`, `op`, ...)
so they take effect immediately and the server writes the JSON files itself.
While it is stopped, `whitelist.json` / `ops.json` / `server.properties` are edited directly.
Every function takes the server instance (`instances.Instance`) whose files it works on.
"""

import hashlib
import json
import re
import time
from datetime import datetime
import urllib.error
import urllib.request
import uuid
from pathlib import Path

from .versions import USER_AGENT

NAME_RE = re.compile(r"^\w{3,16}$")
PROFILE_URL = "https://api.mojang.com/users/profiles/minecraft/{}"


FILES = {"whitelist": "whitelist.json", "ops": "ops.json", "bans": "banned-players.json"}
LABELS = {"whitelist": "whitelist", "ops": "operators", "bans": "ban list"}
REASON_MAX = 200


def validate_reason(reason: str) -> str:
    reason = reason.strip()
    if len(reason) > REASON_MAX or "\n" in reason or "\r" in reason:
        raise ValueError(f"Reason: up to {REASON_MAX} characters, single line")
    return reason


def validate_name(name: str) -> str:
    name = name.strip()
    if not NAME_RE.match(name):
        raise ValueError("Player names have 3-16 characters: letters, digits, underscore")
    return name


def _file(inst, name: str) -> Path:
    return inst.server_dir / name


def _read_list(inst, name: str) -> list[dict]:
    try:
        data = json.loads(_file(inst, name).read_text(encoding="utf-8"))
        return data if isinstance(data, list) else []
    except (OSError, ValueError):
        return []


def _write_list(inst, name: str, entries: list[dict]) -> None:
    _file(inst, name).write_text(json.dumps(entries, indent=2), encoding="utf-8")


# ---------- server.properties ----------


def _properties(inst) -> list[str]:
    try:
        return _file(inst, "server.properties").read_text(encoding="utf-8").splitlines()
    except OSError:
        return []


def get_property(inst, key: str, default: str | None = "") -> str | None:
    for line in _properties(inst):
        k, sep, v = line.partition("=")
        if sep and k.strip() == key:
            return v.strip()
    return default


def set_property(inst, key: str, value: str) -> None:
    lines = _properties(inst)
    for i, line in enumerate(lines):
        k, sep, _ = line.partition("=")
        if sep and k.strip() == key:
            lines[i] = f"{key}={value}"
            break
    else:
        lines.append(f"{key}={value}")
    path = _file(inst, "server.properties")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


# ---------- UUIDs ----------


def _offline_uuid(name: str) -> str:
    # Same as Java's UUID.nameUUIDFromBytes("OfflinePlayer:" + name)
    digest = bytearray(hashlib.md5(f"OfflinePlayer:{name}".encode()).digest())
    digest[6] = (digest[6] & 0x0F) | 0x30
    digest[8] = (digest[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(digest)))


def resolve(inst, name: str) -> dict:
    """Name -> {"uuid", "name"} with the correct spelling."""
    if get_property(inst, "online-mode", "true").lower() == "false":
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


def state(inst) -> dict:
    return {
        "whitelist_enabled": get_property(inst, "white-list", "false").lower() == "true",
        "whitelist": sorted((e.get("name", "?") for e in _read_list(inst, "whitelist.json")), key=str.lower),
        "ops": sorted(
            ({"name": e.get("name", "?"), "level": e.get("level", 4)} for e in _read_list(inst, "ops.json")),
            key=lambda o: o["name"].lower(),
        ),
        "bans": sorted(
            ({"name": e.get("name", "?"), "reason": e.get("reason", ""), "source": e.get("source", ""),
              "created": e.get("created", ""), "expires": e.get("expires", "forever")}
             for e in _read_list(inst, "banned-players.json")),
            key=lambda b: b["name"].lower(),
        ),
    }


# ---------- changes while the server is stopped ----------


_NOT_ON = {"whitelist": "{} is not on the whitelist", "ops": "{} is not an operator", "bans": "{} is not banned"}
_ALREADY = {"whitelist": "{} is already on the whitelist", "ops": "{} is already an operator", "bans": "{} is already banned"}


def offline_change(inst, kind: str, action: str, name: str, reason: str = "") -> str:
    """kind: "whitelist" | "ops" | "bans", action: "add" | "remove". Returns a message."""
    file = FILES[kind]
    entries = _read_list(inst, file)
    label = LABELS[kind]
    if action == "remove":
        kept = [e for e in entries if e.get("name", "").lower() != name.lower()]
        if len(kept) == len(entries):
            raise LookupError(_NOT_ON[kind].format(name))
        _write_list(inst, file, kept)
        return f"Removed {name} from the {label}" if kind != "bans" else f"Unbanned {name}"

    if any(e.get("name", "").lower() == name.lower() for e in entries):
        raise LookupError(_ALREADY[kind].format(name))
    profile = resolve(inst, name)
    entry = dict(profile)
    if kind == "ops":
        entry.update(level=4, bypassesPlayerLimit=False)
    elif kind == "bans":
        # Same fields and date format the server writes
        entry.update(created=datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S %z"), source="Server panel",
                     expires="forever", reason=reason or "Banned by an operator.")
    entries.append(entry)
    _write_list(inst, file, entries)
    return f"Added {profile['name']} to the {label}" if kind != "bans" else f"Banned {profile['name']}"


def mtime(inst, file: str) -> float | None:
    try:
        return _file(inst, file).stat().st_mtime_ns
    except OSError:
        return None


def wait_for_change(inst, file: str, before: float | None, timeout: float = 3.0) -> None:
    """After a server command, wait until the server has rewritten the file."""
    end = time.time() + timeout
    while time.time() < end:
        if mtime(inst, file) != before:
            return
        time.sleep(0.1)
