"""World backups as timestamped zip files with retention, and restore.

Every function takes the server instance (`instances.Instance`) whose world/backups it works on.
"""

import os
import re
import shutil
import zipfile
from datetime import datetime
from pathlib import Path

from . import config

# Region/entity files are already compressed by Minecraft: storing them uncompressed saves a lot
# of CPU during (live) backups at almost no size cost.
_PRECOMPRESSED = {".mca", ".mcc", ".gz", ".zip", ".png"}
# Server folders a restore must never overwrite, even if a manipulated zip contains them
_NOT_A_WORLD = {"plugins", "cache", "libraries", "versions", "logs", "config", "crash-reports"}
_TOP_RE = re.compile(r"^[\w\-. ]{1,64}$")
# World uploads ("Upload world"): size limit of the zip, in GB
MAX_UPLOAD = int(float(os.environ.get("WORLD_UPLOAD_MAX_GB") or 4) * 2**30)
_JUNK = ("__MACOSX/", ".DS_Store", "Thumbs.db")


def level_name(inst=None) -> str:
    # None = the first server (only the live map still calls it that way)
    props = (config.SERVER_DIR if inst is None else inst.server_dir) / "server.properties"
    if props.exists():
        for line in props.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("level-name="):
                return line.split("=", 1)[1].strip() or "world"
    return "world"


def world_dirs(inst) -> list[Path]:
    name = level_name(inst)
    # Vanilla keeps nether/end inside the level dir; Spigot/Paper split them out.
    candidates = [name, f"{name}_nether", f"{name}_the_end"]
    return [inst.server_dir / c for c in candidates if (inst.server_dir / c).is_dir()]


def create_backup(inst, suffix: str = "", protect: Path | None = None) -> Path:
    stamp = datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
    target = inst.backup_dir / f"backup_{stamp}{'_' + suffix if suffix else ''}.zip"
    _zip_world(inst, target)
    cleanup(inst, protect=protect)
    return target


def create_archive(inst, label: str = "") -> Path:
    """Permanent copy of the world (e.g. before a new world is created).
    Archives are named archive_* and therefore never touched by cleanup()."""
    stamp = datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
    safe = re.sub(r"[^\w.\-]", "-", label)[:40]
    target = inst.backup_dir / f"archive_{stamp}{'_' + safe if safe else ''}.zip"
    _zip_world(inst, target)
    return target


def _zip_world(inst, target: Path) -> None:
    dirs = world_dirs(inst)
    if not dirs:
        raise FileNotFoundError("No world found to back up (has the server run yet?)")
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_suffix(".part")

    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as zf:
        for d in dirs:
            for file in d.rglob("*"):
                # session.lock is held open by the running server
                if file.is_file() and file.name != "session.lock":
                    method = zipfile.ZIP_STORED if file.suffix in _PRECOMPRESSED else zipfile.ZIP_DEFLATED
                    zf.write(file, file.relative_to(inst.server_dir), compress_type=method)
    tmp.replace(target)


def _note(f: Path) -> str | None:
    if f.name.startswith("archive_"):
        return "archive · kept forever"
    if f.stem.endswith("_before-restore"):
        return "before restore"
    if m := re.search(r"_before-(vanilla|paper|fabric)$", f.stem):
        return f"before switching to {m.group(1).capitalize()}"
    if f.stem.endswith("_upload"):
        return "uploaded"
    return None


def list_backups(inst) -> list[dict]:
    files = [*inst.backup_dir.glob("backup_*.zip"), *inst.backup_dir.glob("archive_*.zip")]
    files.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    return [
        {
            "name": f.name,
            "size_mb": round(f.stat().st_size / 1024 / 1024, 1),
            "created": datetime.fromtimestamp(f.stat().st_mtime).isoformat(timespec="seconds"),
            "archive": f.name.startswith("archive_"),
            "note": _note(f),
        }
        for f in files
    ]


def backup_file(inst, name: str) -> Path:
    path = (inst.backup_dir / name).resolve()
    if (path.parent != inst.backup_dir.resolve() or not path.name.startswith(("backup_", "archive_"))
            or path.suffix != ".zip" or not path.exists()):
        raise FileNotFoundError(name)
    return path


def remove_world(inst) -> list[str]:
    """Delete the world folders (callers archive them first). Returns the removed names."""
    removed = []
    for d in world_dirs(inst):
        shutil.rmtree(d)
        removed.append(d.name)
    return removed


def cleanup(inst, protect: Path | None = None) -> None:
    files = sorted(inst.backup_dir.glob("backup_*.zip"), key=lambda p: p.stat().st_mtime)
    if protect is not None:
        files = [f for f in files if f.resolve() != protect.resolve()]
        keep = max(0, config.BACKUP_KEEP - 1)  # the protected one counts towards the limit
    else:
        keep = config.BACKUP_KEEP
    for old in files[: max(0, len(files) - keep)]:
        old.unlink(missing_ok=True)


def _world_folders(zf: zipfile.ZipFile) -> set[str]:
    """Top-level folders in the zip; rejects anything that could escape the server dir."""
    tops = set()
    for member in zf.namelist():
        parts = Path(member).parts
        if not parts or member.startswith(("/", "\\")) or ".." in parts or ":" in parts[0]:
            raise ValueError(f"Unsafe path in backup: {member}")
        top = parts[0]
        if len(parts) == 1 and not member.endswith("/"):
            raise ValueError(f"Unexpected file in backup: {member}")
        if not _TOP_RE.match(top) or top.lower() in _NOT_A_WORLD:
            raise ValueError(f"Backup contains a non-world folder: {top}")
        tops.add(top)
    if not tops:
        raise ValueError("Backup is empty")
    return tops


def restore(inst, name: str) -> Path | None:
    """Replace the world folders with the backup's. The server must be stopped.
    The current world is saved first; returns that safety backup (None if there was no world)."""
    src = backup_file(inst, name)
    with zipfile.ZipFile(src) as zf:
        tops = _world_folders(zf)
        safety = create_backup(inst, "before-restore", protect=src) if world_dirs(inst) else None

        staging = inst.server_dir / ".restore-tmp"
        shutil.rmtree(staging, ignore_errors=True)
        zf.extractall(staging)
    try:
        for top in tops:
            shutil.rmtree(inst.server_dir / top, ignore_errors=True)
            (staging / top).replace(inst.server_dir / top)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    return safety


# ---------- uploaded worlds ----------


def _upload_layout(names: list[str], level: str):
    """Function zip path -> path in the server for an uploaded world. Accepts a server world
    (`<level>/level.dat`, maybe with Paper's `_nether`/`_the_end` folders) and a single-player save
    (level.dat in the zip root or in exactly one folder)."""
    if "level.dat" in names:  # save zipped from the inside: everything belongs into the world folder
        return lambda name: f"{level}/{name}"
    tops = {n.split("/", 1)[0] for n in names if "/" in n}
    if any("/" not in n for n in names):
        raise ValueError("Unexpected file in the zip root: " + next(n for n in names if "/" not in n))
    with_level = sorted(t for t in tops if f"{t}/level.dat" in names)
    if len(with_level) != 1:
        raise ValueError("No world found: the zip needs a level.dat (in the root or in one folder)"
                         if not with_level else "The zip contains more than one world (several level.dat)")
    base = with_level[0]
    mapping = {base: level}
    for top in tops - {base}:
        suffix = top[len(base):] if top.startswith(base) else ""
        if suffix not in ("_nether", "_the_end"):
            raise ValueError(f"Unexpected folder next to the world: {top}")
        mapping[top] = level + suffix
    return lambda name: mapping[name.split("/", 1)[0]] + "/" + name.split("/", 1)[1]


def add_upload(inst, src: Path) -> Path:
    """Check an uploaded world zip and store it as `backup_<stamp>_upload.zip` with the folder names
    this server uses (so restore() can use it like any backup)."""
    try:
        zf = zipfile.ZipFile(src)
    except zipfile.BadZipFile:
        raise ValueError("Not a valid zip file") from None
    with zf:
        files = []
        for info in zf.infolist():
            name = info.filename.replace("\\", "/")
            if info.is_dir() or name.startswith(_JUNK) or "/__MACOSX/" in name or name.endswith(_JUNK):
                continue
            parts = Path(name).parts
            if name.startswith("/") or ".." in parts or (parts and ":" in parts[0]):
                raise ValueError(f"Unsafe path in the zip: {name}")
            files.append((info, name))
        if sum(i.file_size for i, _ in files) > 20 * MAX_UPLOAD:
            raise ValueError("The unpacked world would be too large")
        rename = _upload_layout([n for _, n in files], level_name(inst))

        stamp = datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
        inst.backup_dir.mkdir(parents=True, exist_ok=True)
        target = inst.backup_dir / f"backup_{stamp}_upload.zip"
        tmp = target.with_suffix(".part")
        try:
            with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as out:
                for info, name in files:
                    if name.endswith("session.lock"):
                        continue
                    entry = zipfile.ZipInfo(rename(name), info.date_time)
                    # like our own backups: region files are compressed already
                    entry.compress_type = zipfile.ZIP_STORED if Path(name).suffix in _PRECOMPRESSED else zipfile.ZIP_DEFLATED
                    with zf.open(info) as fin, out.open(entry, "w") as fout:
                        shutil.copyfileobj(fin, fout, 1 << 20)
            with zipfile.ZipFile(tmp) as check:
                _world_folders(check)  # same rules as a restore
            tmp.replace(target)
        finally:
            tmp.unlink(missing_ok=True)
    cleanup(inst, protect=target)
    return target
