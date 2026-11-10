"""Server instances: several independent Minecraft servers in one panel.

The original installation is the instance `main` and keeps its old paths (`DATA_DIR/server`,
`BACKUP_DIR`, `DATA_DIR/settings.json`, `DATA_DIR/map`), so nothing has to be migrated. Further
instances live in `DATA_DIR/servers/<id>/` with their backups in `BACKUP_DIR/<id>/`. Jars, Java
runtimes and the data pack library are shared.

Paths are computed on every access from `config`, so tests (and env overrides) can redirect them.
"""

import json
import re
import shutil
import threading
from dataclasses import dataclass
from pathlib import Path

from . import access, config

MAIN_ID = "main"
# Becomes part of file paths: keep it strict
_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,31}$")
_NAME_MAX = 40

_lock = threading.Lock()


@dataclass(frozen=True)
class Instance:
    id: str

    @property
    def is_main(self) -> bool:
        return self.id == MAIN_ID

    @property
    def root(self) -> Path:
        return config.DATA_DIR if self.is_main else config.DATA_DIR / "servers" / self.id

    @property
    def server_dir(self) -> Path:
        return config.SERVER_DIR if self.is_main else self.root / "server"

    @property
    def backup_dir(self) -> Path:
        return config.BACKUP_DIR if self.is_main else config.BACKUP_DIR / self.id

    @property
    def settings_file(self) -> Path:
        return config.SETTINGS_FILE if self.is_main else self.root / "settings.json"

    @property
    def map_dir(self) -> Path:
        return config.DATA_DIR / "map" if self.is_main else self.root / "map"

    def ensure_dirs(self) -> None:
        for d in (self.server_dir, self.backup_dir):
            d.mkdir(parents=True, exist_ok=True)


MAIN = Instance(MAIN_ID)


def validate_id(value: str) -> str:
    if not _ID_RE.match(value):
        raise ValueError("Invalid server id")
    return value


def validate_name(name: str) -> str:
    name = name.strip()
    if not name or len(name) > _NAME_MAX or any(c in name for c in "\r\n\t"):
        raise ValueError(f"Server name: 1-{_NAME_MAX} characters, single line")
    return name


# ---------- registry (DATA_DIR/servers.json) ----------


def _registry_file() -> Path:
    return config.DATA_DIR / "servers.json"


def _read() -> list[dict]:
    """[{id, name}], always starting with main."""
    entries: list[dict] = []
    try:
        data = json.loads(_registry_file().read_text(encoding="utf-8"))
        if isinstance(data, list):
            entries = [e for e in data if isinstance(e, dict) and isinstance(e.get("id"), str)
                       and _ID_RE.match(e["id"]) and isinstance(e.get("name"), str)]
    except (OSError, ValueError):
        pass
    if not any(e["id"] == MAIN_ID for e in entries):
        entries.insert(0, {"id": MAIN_ID, "name": "Server"})
    return entries


def _write(entries: list[dict]) -> None:
    tmp = _registry_file().with_suffix(".tmp")
    tmp.write_text(json.dumps(entries, indent=2), encoding="utf-8")
    tmp.replace(_registry_file())


def all_entries() -> list[dict]:
    with _lock:
        return _read()


def get(server_id: str) -> Instance:
    """KeyError if there is no such server."""
    if not any(e["id"] == server_id for e in all_entries()):
        raise KeyError(server_id)
    return Instance(server_id)


def name_of(inst: Instance) -> str:
    return next((e["name"] for e in all_entries() if e["id"] == inst.id), inst.id)


def _slug(name: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")[:24].strip("-")
    return slug or "server"


def create(name: str) -> Instance:
    """Register a new instance and create its folders. The caller writes its settings."""
    name = validate_name(name)
    with _lock:
        entries = _read()
        taken = {e["id"] for e in entries}
        base = _slug(name)
        sid, n = base, 2
        while sid in taken or sid == MAIN_ID:
            sid, n = f"{base}-{n}", n + 1
        inst = Instance(validate_id(sid))
        inst.ensure_dirs()
        entries.append({"id": sid, "name": name})
        _write(entries)
    return inst


def rename(inst: Instance, name: str) -> None:
    name = validate_name(name)
    with _lock:
        entries = _read()
        for e in entries:
            if e["id"] == inst.id:
                e["name"] = name
        _write(entries)


def remove(inst: Instance) -> None:
    """Delete the server folder (world, settings, map). Its backups are kept."""
    if inst.is_main:
        raise ValueError("The first server can't be deleted")
    with _lock:
        entries = [e for e in _read() if e["id"] != inst.id]
        _write(entries)
    shutil.rmtree(inst.root, ignore_errors=True)


# ---------- game ports ----------


def game_port(inst: Instance) -> int:
    """server-port from server.properties (Minecraft's default if missing)."""
    try:
        return int(access.get_property(inst, "server-port", "25565") or 25565)
    except ValueError:
        return 25565


def free_port(exclude: Instance | None = None) -> int:
    """First port of SERVER_PORTS that no other instance uses."""
    used = {game_port(Instance(e["id"])) for e in all_entries() if exclude is None or e["id"] != exclude.id}
    for port in config.SERVER_PORTS:
        if port not in used:
            return port
    raise RuntimeError(f"No free game port left in SERVER_PORTS ({config.SERVER_PORTS[0]}-{config.SERVER_PORTS[-1]})")
