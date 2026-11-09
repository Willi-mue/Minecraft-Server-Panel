"""World backups as timestamped zip files with retention, and restore."""

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


def level_name() -> str:
    props = config.SERVER_DIR / "server.properties"
    if props.exists():
        for line in props.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("level-name="):
                return line.split("=", 1)[1].strip() or "world"
    return "world"


def world_dirs() -> list[Path]:
    name = level_name()
    # Vanilla keeps nether/end inside the level dir; Spigot/Paper split them out.
    candidates = [name, f"{name}_nether", f"{name}_the_end"]
    return [config.SERVER_DIR / c for c in candidates if (config.SERVER_DIR / c).is_dir()]


def create_backup(suffix: str = "", protect: Path | None = None) -> Path:
    stamp = datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
    target = config.BACKUP_DIR / f"backup_{stamp}{'_' + suffix if suffix else ''}.zip"
    _zip_world(target)
    cleanup(protect=protect)
    return target


def create_archive(label: str = "") -> Path:
    """Permanent copy of the world (e.g. before a new world is created).
    Archives are named archive_* and therefore never touched by cleanup()."""
    stamp = datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
    safe = re.sub(r"[^\w.\-]", "-", label)[:40]
    target = config.BACKUP_DIR / f"archive_{stamp}{'_' + safe if safe else ''}.zip"
    _zip_world(target)
    return target


def _zip_world(target: Path) -> None:
    dirs = world_dirs()
    if not dirs:
        raise FileNotFoundError("No world found to back up (has the server run yet?)")
    tmp = target.with_suffix(".part")

    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as zf:
        for d in dirs:
            for file in d.rglob("*"):
                # session.lock is held open by the running server
                if file.is_file() and file.name != "session.lock":
                    method = zipfile.ZIP_STORED if file.suffix in _PRECOMPRESSED else zipfile.ZIP_DEFLATED
                    zf.write(file, file.relative_to(config.SERVER_DIR), compress_type=method)
    tmp.replace(target)


def _note(f: Path) -> str | None:
    if f.name.startswith("archive_"):
        return "archive · kept forever"
    if f.stem.endswith("_before-restore"):
        return "before restore"
    return None


def list_backups() -> list[dict]:
    files = [*config.BACKUP_DIR.glob("backup_*.zip"), *config.BACKUP_DIR.glob("archive_*.zip")]
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


def backup_file(name: str) -> Path:
    path = (config.BACKUP_DIR / name).resolve()
    if (path.parent != config.BACKUP_DIR or not path.name.startswith(("backup_", "archive_"))
            or path.suffix != ".zip" or not path.exists()):
        raise FileNotFoundError(name)
    return path


def remove_world() -> list[str]:
    """Delete the world folders (callers archive them first). Returns the removed names."""
    removed = []
    for d in world_dirs():
        shutil.rmtree(d)
        removed.append(d.name)
    return removed


def cleanup(protect: Path | None = None) -> None:
    files = sorted(config.BACKUP_DIR.glob("backup_*.zip"), key=lambda p: p.stat().st_mtime)
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


def restore(name: str) -> Path | None:
    """Replace the world folders with the backup's. The server must be stopped.
    The current world is saved first; returns that safety backup (None if there was no world)."""
    src = backup_file(name)
    with zipfile.ZipFile(src) as zf:
        tops = _world_folders(zf)
        safety = create_backup("before-restore", protect=src) if world_dirs() else None

        staging = config.SERVER_DIR / ".restore-tmp"
        shutil.rmtree(staging, ignore_errors=True)
        zf.extractall(staging)
    try:
        for top in tops:
            shutil.rmtree(config.SERVER_DIR / top, ignore_errors=True)
            (staging / top).replace(config.SERVER_DIR / top)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    return safety
