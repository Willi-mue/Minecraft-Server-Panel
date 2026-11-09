"""Live map: renders the world files with `mapgen` (Rust, see mapgen/) and tracks players.

Works for every server type because it reads the world files directly; nothing runs inside
Minecraft. Rendering is incremental (only changed region files) and runs with low priority.
Player positions are polled via `data get entity` only while somebody looks at the map.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

from . import backup, config, versions

REPO_DIR = Path(__file__).resolve().parent.parent

# Block textures for the map come from the Minecraft client of the world's version, downloaded once
# from Mojang (kept only on this server, never redistributed). MAP_TEXTURES=false: map-item colours.
MAP_TEXTURES = os.environ.get("MAP_TEXTURES", "true").lower() != "false"
_client_failed: dict[str, float] = {}


def out_dir() -> Path:
    return config.DATA_DIR / "map"


def available() -> bool:
    return (out_dir() / "maps.json").exists()


def clear() -> None:
    """Forget the rendered map (after a new world was created)."""
    shutil.rmtree(out_dir(), ignore_errors=True)


def find_binary() -> str | None:
    if config.MAPGEN_BIN:
        return config.MAPGEN_BIN if Path(config.MAPGEN_BIN).exists() else None
    if found := shutil.which("mapgen"):
        return found
    exe = "mapgen.exe" if sys.platform == "win32" else "mapgen"
    local = REPO_DIR / "mapgen" / "target" / "release" / exe
    return str(local) if local.exists() else None


def remove_squaremap() -> bool:
    """The map used to be the squaremap plugin (Paper only); remove it if still installed."""
    removed = False
    for jar in (config.SERVER_DIR / "plugins").glob("squaremap*.jar"):
        jar.unlink(missing_ok=True)
        removed = True
    return removed


def client_jar(log=print) -> Path | None:
    """The Minecraft client jar for the world's version (download on first use, SHA-1 verified).
    None if textures are off or the download failed (retried after an hour)."""
    if not MAP_TEXTURES:
        return None
    s = config.load_settings()
    try:
        version = config.validate_version(s.world_version or s.version)
    except ValueError:
        return None
    folder = config.DATA_DIR / "cache"
    path = folder / f"client-{version}.jar"
    if path.exists():
        return path
    if time.monotonic() - _client_failed.get(version, -1e9) < 3600:
        return None
    try:
        entry = next((v for v in versions._cached(versions.MOJANG_MANIFEST)["versions"] if v["id"] == version), None)
        if entry is None:
            raise ValueError(f"Mojang doesn't know version {version}")
        client = versions._get_json(entry["url"])["downloads"]["client"]
        folder.mkdir(parents=True, exist_ok=True)
        log(f"Live map: downloading the Minecraft {version} block textures from Mojang "
            f"({client['size'] // 2**20} MB, once) ...")
        versions._fetch(client["url"], path, "sha1", client["sha1"])
    except (OSError, ValueError, KeyError) as e:
        _client_failed[version] = time.monotonic()
        log(f"Live map: block textures unavailable ({e}), using map colours.")
        return None
    for old in folder.glob("client-*.jar"):  # only the current version is needed
        if old != path:
            old.unlink(missing_ok=True)
    return path


def _world_args() -> list[str]:
    name = backup.level_name()
    args = ["--world", str(config.SERVER_DIR / name)]
    # Paper/Spigot before 26.x keep Nether and End in sibling folders
    for suffix in ("_nether", "_the_end"):
        extra = config.SERVER_DIR / f"{name}{suffix}"
        if extra.is_dir():
            args += ["--extra", str(extra)]
    return args


class Renderer:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self.last: dict | None = None

    @property
    def busy(self) -> bool:
        return self._lock.locked()

    def render(self, log=print, force: bool = False, announce: bool = False) -> dict | None:
        """Run mapgen once. Returns its summary, or None if skipped/failed."""
        if not (config.SERVER_DIR / backup.level_name()).is_dir():
            return None  # no world yet
        binary = find_binary()
        if not binary:
            log("Live map: renderer 'mapgen' not found (build it with: cargo build --release in mapgen/).")
            return None
        if not self._lock.acquire(blocking=False):
            return None  # a render is already running
        try:
            cmd = [binary, "render", *_world_args(), "--out", str(out_dir()),
                   "--threads", str(config.MAP_THREADS)] + (["--force"] if force else [])
            if jar := client_jar(log):
                cmd += ["--client", str(jar)]
            kwargs: dict = {}
            if sys.platform == "win32":
                kwargs["creationflags"] = 0x00004000 | 0x08000000  # below normal priority, no window
            else:
                kwargs["preexec_fn"] = lambda: os.nice(15)  # never compete with the game
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=3600, **kwargs)
            line = (proc.stdout.strip().splitlines() or [""])[-1]
            try:
                summary = json.loads(line)
            except ValueError:
                log(f"Live map render failed: {(proc.stderr or proc.stdout).strip()[:300]}")
                return None
            self.last = summary
            rendered = sum(d["rendered"] for d in summary["dimensions"])
            errors = [e for d in summary["dimensions"] for e in d["errors"]]
            if rendered or force or announce:
                log(f"Live map updated: {rendered} changed region(s) in {summary['ms'] / 1000:.1f} s.")
            for e in errors[:3]:
                log(f"Live map: {e}")
            return summary
        except (OSError, subprocess.SubprocessError) as e:
            log(f"Live map render failed: {e}")
            return None
        finally:
            self._lock.release()


renderer = Renderer()


# ---------- live player positions ----------

# "Steve has the following entity data: [12.5d, 64.0d, -3.25d]"
POS_RE = re.compile(r"\]: (?:System chat: )?(\w{1,16}) has the following entity data: \[(-?[\d.E]+)d, (-?[\d.E]+)d, (-?[\d.E]+)d\]$")
# 'Steve has the following entity data: "minecraft:overworld"'
DIM_RE = re.compile(r'\]: (?:System chat: )?(\w{1,16}) has the following entity data: "([\w:./-]+)"$')

WATCH_SECONDS = 30   # keep polling this long after the last map request
PENDING_SECONDS = 5  # answers to our queries arrive within this time


class PlayerTracker:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self.positions: dict[str, dict] = {}
        self._pending: dict[str, float] = {}
        self._last_view = 0.0

    def touch(self) -> None:
        self._last_view = time.monotonic()

    def watching(self) -> bool:
        return time.monotonic() - self._last_view < WATCH_SECONDS

    def queries(self, online: set[str]) -> list[str]:
        """Commands to send now; remembers who was asked so the answers can be filtered."""
        now = time.monotonic()
        with self._lock:
            for name in list(self.positions):
                if name not in online:
                    del self.positions[name]
            for name in online:
                self._pending[name] = now
        return [cmd for name in sorted(online)
                for cmd in (f"data get entity {name} Pos", f"data get entity {name} Dimension")]

    def consume(self, line: str) -> bool:
        """True if the console line is an answer to our own query (then it isn't shown)."""
        if "has the following entity data" not in line:
            return False
        m = POS_RE.search(line) or DIM_RE.search(line)
        if not m:
            return False
        name = m.group(1)
        with self._lock:
            asked = self._pending.get(name)
            if asked is None or time.monotonic() - asked > PENDING_SECONDS:
                return False  # somebody typed it in the console: show it
            entry = self.positions.setdefault(name, {"name": name, "dimension": "minecraft:overworld"})
            if m.re is POS_RE:
                entry.update(x=float(m.group(2)), y=float(m.group(3)), z=float(m.group(4)))
            else:
                entry["dimension"] = m.group(2)
        return True

    def snapshot(self) -> list[dict]:
        with self._lock:
            return [dict(p) for p in self.positions.values() if "x" in p]

    def clear(self) -> None:
        with self._lock:
            self.positions.clear()
            self._pending.clear()


players = PlayerTracker()
