"""Data packs (Vanilla and Paper): a library outside the world plus installed copies in the world.

- Library: `<DATA_DIR>/datapacks/` keeps every uploaded pack, independent of the world (survives
  "New world" and restores) and shared by all servers of the panel.
- World: `<world>/datapacks/` holds copies of the installed packs; this is what Minecraft loads.
- Enabled/disabled is per world and lives in level.dat (`Data.DataPacks.Enabled/Disabled`, ids like
  `file/<name>`). A pack in the world folder that is in neither list is new and gets enabled on the
  next start or `/reload`. While the server runs, packs are switched with `/datapack`; while it is
  stopped, level.dat is edited directly.
"""

import hashlib
import json
import re
import shutil
import zipfile
from pathlib import Path

from . import backup, config, nbt, world  # functions with `inst` take an instances.Instance

MAX_UPLOAD = 100 * 1024 * 1024
# No quotes or backslashes (the name goes into a quoted console command), no path separators
_NAME_RE = re.compile(r"^[\w\-. ()+\[\]!&',]{1,100}$")


def library() -> Path:
    return config.DATA_DIR / "datapacks"


def folder(inst) -> Path:
    """The world's datapacks folder."""
    return inst.server_dir / backup.level_name(inst) / "datapacks"


def pack_id(name: str) -> str:
    return f"file/{name}"


def validate_name(name: str) -> str:
    if not _NAME_RE.match(name) or name.startswith(".") or name.endswith((".part", ".tmp")):
        raise ValueError("Invalid data pack name")
    return name


def _existing(base: Path, name: str, where: str) -> Path:
    path = base / validate_name(name)
    if not path.exists():
        raise FileNotFoundError(f"Data pack {name} is not in the {where}")
    return path


def path_of(inst, name: str) -> Path:
    """Installed copy in the world."""
    return _existing(folder(inst), name, "world")


def library_path(name: str) -> Path:
    return _existing(library(), name, "library")


# ---------- reading ----------


def _text(desc) -> str:
    """pack.mcmeta description: plain string or text component(s)."""
    if isinstance(desc, str):
        return desc
    if isinstance(desc, list):
        return "".join(_text(d) for d in desc)
    if isinstance(desc, dict):
        return _text(desc.get("text", "")) + _text(desc.get("extra", []))
    return ""


def _mcmeta(path: Path) -> dict | None:
    try:
        if path.is_dir():
            raw = (path / "pack.mcmeta").read_bytes()
        else:
            with zipfile.ZipFile(path) as zf:
                raw = zf.read("pack.mcmeta")
        meta = json.loads(raw.decode("utf-8-sig"))
        return meta if isinstance(meta, dict) else None
    except (OSError, KeyError, ValueError, zipfile.BadZipFile):
        return None


def _files(path: Path) -> list[Path]:
    return [path] if path.is_file() else sorted(p for p in path.rglob("*") if p.is_file())


def _size(path: Path) -> int:
    return sum(p.stat().st_size for p in _files(path))


def _digest(path: Path) -> str:
    h = hashlib.sha1()
    for p in _files(path):
        h.update(p.relative_to(path).as_posix().encode() if path.is_dir() else b"")
        h.update(p.read_bytes())
    return h.hexdigest()


def _entries(base: Path) -> dict[str, Path]:
    if not base.is_dir():
        return {}
    return {p.name: p for p in base.iterdir()
            if (p.is_dir() or p.suffix.lower() == ".zip") and _NAME_RE.match(p.name)
            and not p.name.startswith(".")}


def _lists(root: dict) -> tuple[list, list]:
    """The Enabled and Disabled string lists of level.dat (created if missing)."""
    data = root.setdefault("Data", (nbt.COMPOUND, {}))[1]
    packs = data.setdefault("DataPacks", (nbt.COMPOUND, {}))[1]
    result = []
    for key in ("Enabled", "Disabled"):
        tag = packs.get(key)
        if tag is None or tag[0] != nbt.LIST or (tag[1][1] and tag[1][0] != nbt.STRING):
            tag = packs[key] = (nbt.LIST, (nbt.STRING, []))
        elif tag[1][0] != nbt.STRING:  # empty list stored with item type END
            tag = packs[key] = (nbt.LIST, (nbt.STRING, tag[1][1]))
        result.append(tag[1][1])
    return result[0], result[1]


def state(inst) -> tuple[set[str], set[str]] | None:
    """(enabled ids, disabled ids) from level.dat, None if the world doesn't exist yet."""
    path = world.level_dat(inst)
    if not path.exists():
        return None
    _, root = nbt.load(path)
    enabled, disabled = _lists(root)
    return set(enabled), set(disabled)


# Built-in packs that are always on (not experiments)
_BASE_PACKS = {"vanilla", "paper"}


def experiments(st: tuple[set[str], set[str]] | None) -> dict:
    """Built-in feature packs (experiments) the current world has on; shown as a note only."""
    if st is None:
        return {"enabled": []}
    return {"enabled": sorted(i for i in st[0] if not i.startswith("file/") and i not in _BASE_PACKS)}


def list_packs(inst) -> dict:
    """Library and world packs merged. `status`: enabled / disabled / new (installed, enabled on next
    start) / None (not installed in this world)."""
    try:
        st = state(inst)
    except (OSError, ValueError, EOFError):
        st = None
    lib, in_world = _entries(library()), _entries(folder(inst))
    packs = []
    for name in sorted(lib.keys() | in_world.keys(), key=str.lower):
        src = lib.get(name) or in_world[name]
        meta = _mcmeta(src)
        pid = pack_id(name)
        if name not in in_world:
            status = None
        elif st is None or (pid not in st[0] and pid not in st[1]):
            status = "new"
        else:
            status = "enabled" if pid in st[0] else "disabled"
        info = (meta or {}).get("pack")
        packs.append({
            "name": name,
            "in_library": name in lib,
            "installed": name in in_world,
            "status": status,
            # Library has a different version than the world: offer to update the world copy
            "outdated": name in lib and name in in_world and _digest(lib[name]) != _digest(in_world[name]),
            "valid": meta is not None,
            "description": _text(info.get("description", "") if isinstance(info, dict) else "")[:300],
            "size_kb": round(_size(src) / 1024),
        })
    return {"world": st is not None, "packs": packs, "experiments": experiments(st)}


# ---------- library ----------


def add_upload(name: str, src: Path) -> Path:
    """Check an uploaded file is a data pack zip and move it into the library (replacing an older
    version of the same name)."""
    validate_name(name)
    if not name.lower().endswith(".zip"):
        raise ValueError("Upload a .zip file")
    _check_zip(src)
    target = library() / name
    if target.is_dir():
        raise FileExistsError(f"A folder pack named {name} is already in the library")
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(src, target)
    return target


def _check_zip(path: Path) -> None:
    try:
        with zipfile.ZipFile(path) as zf:
            names = zf.namelist()
    except zipfile.BadZipFile:
        raise ValueError("Not a valid zip file") from None
    if "pack.mcmeta" in names:
        if _mcmeta(path) is None:
            raise ValueError("pack.mcmeta is not valid JSON")
        return
    if any(n.endswith("/pack.mcmeta") for n in names):
        raise ValueError("pack.mcmeta must be at the top of the zip (it is zipped one folder too deep)")
    raise ValueError("Not a data pack: pack.mcmeta is missing")


def _copy(src: Path, target: Path) -> None:
    """Copy a pack (zip or folder), replacing the target via a temporary name."""
    tmp = target.with_name(target.name + ".tmp")
    _remove(tmp)
    target.parent.mkdir(parents=True, exist_ok=True)
    if src.is_dir():
        shutil.copytree(src, tmp)
    else:
        shutil.copy2(src, tmp)
    _remove(target)
    tmp.rename(target)


def _remove(path: Path) -> None:
    if path.is_dir():
        shutil.rmtree(path)
    elif path.exists():
        path.unlink()


def adopt(inst, name: str) -> None:
    """Copy a pack that only exists in the world (e.g. from a restored backup) into the library."""
    _copy(path_of(inst, name), library() / name)


def delete_from_library(name: str) -> None:
    _remove(library_path(name))


# ---------- world ----------


def install(inst, name: str) -> None:
    """Copy the library version into the world (also updates an older installed copy)."""
    _copy(library_path(name), folder(inst) / name)


def uninstall(inst, name: str) -> None:
    _remove(path_of(inst, name))


def prepare_new_world(inst, names: list[str]) -> list[str]:
    """Before the old world is removed: copy packs that only exist in the world into the library, so
    the selected ones can be installed into the new world. Returns the names that are available."""
    lib, in_world = _entries(library()), _entries(folder(inst))
    for name in names:
        if name not in lib and name in in_world:
            adopt(inst, name)
    return [n for n in names if (library() / n).exists()]


def install_many(inst, names: list[str]) -> None:
    for name in names:
        install(inst, name)


def set_enabled_offline(inst, name: str, enabled: bool) -> str:
    """Edit level.dat while the server is stopped. Returns a message for the log."""
    path_of(inst, name)
    pid = pack_id(name)
    dat = world.level_dat(inst)
    if not dat.exists():
        if enabled:
            return f"Data pack {name} will be enabled when the world is created."
        raise RuntimeError("The world doesn't exist yet: start the server once, then disable packs")
    root_name, root = nbt.load(dat)
    en, dis = _lists(root)
    for lst in (en, dis):
        while pid in lst:
            lst.remove(pid)
    (en if enabled else dis).append(pid)  # end of Enabled = highest priority, like /datapack enable
    nbt.save(dat, root_name, root)
    return f"Data pack {name} {'enabled' if enabled else 'disabled'} (applies on next start)."
