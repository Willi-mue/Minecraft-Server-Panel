"""Live map: renders the world files with `mapgen` (Rust, see mapgen/) and tracks players.

Works for every server type because it reads the world files directly; nothing runs inside
Minecraft. Rendering is incremental (only changed region files) and runs with low priority.
Player positions are polled via `data get entity` only while somebody looks at the map.
Every server instance has its own map (`Instance.map_dir`), renderer and player tracker; the
functions take the instance (None = the first server, `main`).
"""

import base64
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request
from pathlib import Path

from . import backup, config, instances, versions
from .instances import Instance

REPO_DIR = Path(__file__).resolve().parent.parent

# Block textures for the map come from the Minecraft client of the world's version, downloaded once
# from Mojang (kept only on this server, never redistributed). MAP_TEXTURES=false: map-item colours.
MAP_TEXTURES = os.environ.get("MAP_TEXTURES", "true").lower() != "false"
_client_failed: dict[str, float] = {}


def _inst(inst: Instance | None) -> Instance:
    return instances.MAIN if inst is None else inst


def out_dir(inst: Instance | None = None) -> Path:
    return _inst(inst).map_dir


# ---------- what the 3D map shows below the surface ----------
# Normally caves are shown only as far as they are open to the sky (`cave_reach` columns from the
# opening). With `all_caves`, mapgen also writes stacks with every cave (`c/`), which the viewer
# loads while its height slice is on (that reveals hidden caves to everyone who can open the map).
# Kept in DATA_DIR/map_visibility.json (not in the map folder: that one is public).

CAVE_REACH_DEFAULT = min(256, max(0, int(os.environ.get("MAP_CAVE_REACH") or 32)))
ALL_CAVES_DEFAULT = os.environ.get("MAP_ALL_CAVES", "false").strip().lower() in ("1", "true", "yes", "on")


def _visibility_file() -> Path:
    return config.DATA_DIR / "map_visibility.json"


def validate_visibility(data: dict) -> dict:
    """Checked copy of {"cave_reach": int, "all_caves": bool}. Raises ValueError."""
    reach = data.get("cave_reach", CAVE_REACH_DEFAULT)
    if type(reach) is not int or not 0 <= reach <= 256:
        raise ValueError("Cave reach must be a whole number from 0 to 256")
    all_caves = data.get("all_caves", ALL_CAVES_DEFAULT)
    if type(all_caves) is not bool:
        raise ValueError("all_caves must be true or false")
    return {"cave_reach": reach, "all_caves": all_caves}


def load_visibility() -> dict:
    try:
        return validate_visibility(json.loads(_visibility_file().read_text(encoding="utf-8")))
    except (OSError, ValueError, AttributeError):
        return {"cave_reach": CAVE_REACH_DEFAULT, "all_caves": ALL_CAVES_DEFAULT}


def save_visibility(data: dict) -> dict:
    """Validate and store (keys left out keep their value); the next render re-renders the 3D
    stacks (mapgen notices the change)."""
    v = validate_visibility({**load_visibility(), **data})
    _visibility_file().write_text(json.dumps(v, indent=2), encoding="utf-8")
    return v


def _visibility_args() -> list[str]:
    v = load_visibility()
    return ["--cave-reach", str(v["cave_reach"])] + (["--all-caves"] if v["all_caves"] else [])


def available(inst: Instance | None = None) -> bool:
    return (out_dir(inst) / "maps.json").exists()


def clear(inst: Instance | None = None) -> None:
    """Forget the rendered map (after a new world was created)."""
    shutil.rmtree(out_dir(inst), ignore_errors=True)


def find_binary() -> str | None:
    if config.MAPGEN_BIN:
        return config.MAPGEN_BIN if Path(config.MAPGEN_BIN).exists() else None
    if found := shutil.which("mapgen"):
        return found
    exe = "mapgen.exe" if sys.platform == "win32" else "mapgen"
    local = REPO_DIR / "mapgen" / "target" / "release" / exe
    return str(local) if local.exists() else None


def remove_squaremap(inst: Instance | None = None) -> bool:
    """The map used to be the squaremap plugin (Paper only); remove it if still installed."""
    removed = False
    for jar in (_inst(inst).server_dir / "plugins").glob("squaremap*.jar"):
        jar.unlink(missing_ok=True)
        removed = True
    return removed


def _map_version(inst: Instance) -> str | None:
    """The Minecraft version whose client jar the map of this server needs (validated)."""
    s = config.load_settings(inst)
    try:
        return config.validate_version(s.world_version or s.version)
    except ValueError:
        return None


def client_jar(log=print, inst: Instance | None = None) -> Path | None:
    """The Minecraft client jar for the world's version (download on first use, SHA-1 verified).
    None if textures are off or the download failed (retried after an hour)."""
    if not MAP_TEXTURES:
        return None
    version = _map_version(_inst(inst))
    if version is None:
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
    # only the versions some server's map needs are kept
    needed = {f"client-{v}.jar" for e in instances.all_entries() if (v := _map_version(Instance(e["id"])))}
    for old in folder.glob("client-*.jar"):
        if old != path and old.name not in needed:
            old.unlink(missing_ok=True)
    return path


def _world_args(inst: Instance | None = None) -> list[str]:
    inst = _inst(inst)
    name = backup.level_name(inst)
    args = ["--world", str(inst.server_dir / name)]
    # Paper/Spigot before 26.x keep Nether and End in sibling folders
    for suffix in ("_nether", "_the_end"):
        extra = inst.server_dir / f"{name}{suffix}"
        if extra.is_dir():
            args += ["--extra", str(extra)]
    return args


# One mapgen process at a time over all servers (each one already uses MAP_THREADS threads)
_render_slot = threading.Lock()


class Renderer:
    def __init__(self, inst: Instance | None = None) -> None:
        self.inst = _inst(inst)
        self._lock = threading.Lock()
        self.last: dict | None = None

    @property
    def busy(self) -> bool:
        return self._lock.locked()

    def render(self, log=print, force: bool = False, announce: bool = False) -> dict | None:
        """Run mapgen once. Returns its summary, or None if skipped/failed."""
        inst = self.inst
        if not (inst.server_dir / backup.level_name(inst)).is_dir():
            return None  # no world yet
        binary = find_binary()
        if not binary:
            log("Live map: renderer 'mapgen' not found (build it with: cargo build --release in mapgen/).")
            return None
        if not self._lock.acquire(blocking=False):
            return None  # a render is already running
        _render_slot.acquire()  # waits while another server's map renders
        try:
            cmd = [binary, "render", *_world_args(inst), "--out", str(out_dir(inst)),
                   "--threads", str(config.MAP_THREADS), *_visibility_args()] + (["--force"] if force else [])
            if jar := client_jar(log, inst):
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
            _render_slot.release()
            self._lock.release()


_renderers: dict[str, Renderer] = {}
_trackers: dict[str, "PlayerTracker"] = {}
_registry_lock = threading.Lock()


def renderer_for(inst: Instance | None = None) -> Renderer:
    inst = _inst(inst)
    with _registry_lock:
        if inst.id not in _renderers:
            _renderers[inst.id] = Renderer(inst)
        return _renderers[inst.id]


def tracker_for(inst: Instance | None = None) -> "PlayerTracker":
    inst = _inst(inst)
    with _registry_lock:
        if inst.id not in _trackers:
            _trackers[inst.id] = PlayerTracker(inst)
        return _trackers[inst.id]


# ---------- live player positions ----------

# "Steve has the following entity data: [12.5d, 64.0d, -3.25d]"
POS_RE = re.compile(r"\]: (?:System chat: )?(\w{1,16}) has the following entity data: \[(-?[\d.E]+)d, (-?[\d.E]+)d, (-?[\d.E]+)d\]$")
# 'Steve has the following entity data: "minecraft:overworld"'
DIM_RE = re.compile(r'\]: (?:System chat: )?(\w{1,16}) has the following entity data: "([\w:./-]+)"$')

WATCH_SECONDS = 30   # keep polling this long after the last map request
PENDING_SECONDS = 5  # answers to our queries arrive within this time


class PlayerTracker:
    def __init__(self, inst: Instance | None = None) -> None:
        self.inst = _inst(inst)
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
        skins.want(online, self.inst)
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
            out = [dict(p) for p in self.positions.values() if "x" in p]
        for p in out:
            # the viewer shows the face of the skin (skins/<uuid>.png on the map server)
            uuid = skins.uuid_of(p["name"], self.inst)
            if uuid and skin_path(uuid).exists():
                p["skin"] = uuid
        return out

    def clear(self) -> None:
        with self._lock:
            self.positions.clear()
            self._pending.clear()



# ---------- player skins (for the heads on the map) ----------
# Downloaded once per player from Mojang (session server -> textures.minecraft.net) into
# DATA_DIR/cache/skins/<uuid>.png and served by the map server; the viewer cuts out the face.
# Offline-mode servers have no Mojang profiles: then the map shows a plain head.

_UUID_RE = re.compile(r"^[0-9a-f]{32}$")
SKIN_MAX_BYTES = 64 * 1024
SKIN_MAX_AGE = 24 * 3600  # re-download once a day (players change skins)
SKIN_RETRY = 3600         # after a failure (offline-mode uuid, no network)


def skin_path(uuid: str) -> Path:
    if not _UUID_RE.match(uuid):
        raise ValueError("invalid uuid")
    return config.DATA_DIR / "cache" / "skins" / f"{uuid}.png"


def _skin_url(profile: dict) -> str | None:
    """The skin URL of a session server profile, only if it is on Mojang's texture server."""
    for prop in profile.get("properties", []):
        if prop.get("name") != "textures":
            continue
        textures = json.loads(base64.b64decode(prop.get("value", "")))
        url = textures.get("textures", {}).get("SKIN", {}).get("url", "")
        u = urllib.parse.urlparse(url)
        if u.scheme in ("http", "https") and u.hostname == "textures.minecraft.net":
            return urllib.parse.urlunparse(u._replace(scheme="https"))
    return None


def _http_get(url: str, limit: int) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": versions.USER_AGENT})
    with urllib.request.urlopen(req, timeout=15) as resp:
        if resp.status != 200:
            raise OSError(f"HTTP {resp.status}")
        data = resp.read(limit + 1)
    if len(data) > limit:
        raise OSError("response too large")
    return data


def fetch_skin(uuid: str) -> Path:
    """Download the skin of a player (uuid without dashes). Raises OSError/ValueError."""
    path = skin_path(uuid)
    profile = json.loads(_http_get(f"https://sessionserver.mojang.com/session/minecraft/profile/{uuid}", 16 * 1024))
    url = _skin_url(profile)
    if not url:
        raise ValueError("no skin")
    png = _http_get(url, SKIN_MAX_BYTES)
    if not png.startswith(bytes.fromhex("89504e470d0a1a0a")):
        raise ValueError("not a PNG")
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(".tmp")
    tmp.write_bytes(png)
    tmp.replace(path)
    return path


class Skins:
    """Name -> uuid from the server's usercache.json; downloads in one background thread."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._cache: dict[str, tuple[float, dict[str, str]]] = {}  # server id -> (mtime, names)
        self._failed: dict[str, float] = {}
        self._queue: list[str] = []
        self._thread: threading.Thread | None = None

    def uuid_of(self, name: str, inst: Instance | None = None) -> str | None:
        inst = _inst(inst)
        path = inst.server_dir / "usercache.json"
        try:
            mtime = path.stat().st_mtime
        except OSError:
            return None
        with self._lock:
            if self._cache.get(inst.id, (None,))[0] != mtime:
                try:
                    entries = json.loads(path.read_text(encoding="utf-8"))
                    names = {str(e["name"]).lower(): str(e["uuid"]).replace("-", "").lower()
                             for e in entries if isinstance(e, dict) and "name" in e and "uuid" in e}
                except (OSError, ValueError, TypeError):
                    names = {}
                self._cache[inst.id] = (mtime, names)
            uuid = self._cache[inst.id][1].get(name.lower())
        return uuid if uuid and _UUID_RE.match(uuid) else None

    def want(self, names, inst: Instance | None = None) -> None:
        """Queue downloads for these players if their skin is missing or old."""
        now = time.time()
        for name in names:
            uuid = self.uuid_of(name, inst)
            if not uuid:
                continue
            path = skin_path(uuid)
            try:
                fresh = now - path.stat().st_mtime < SKIN_MAX_AGE
            except OSError:
                fresh = False
            with self._lock:
                if fresh or uuid in self._queue or now - self._failed.get(uuid, 0) < SKIN_RETRY:
                    continue
                self._queue.append(uuid)
                if not self._thread or not self._thread.is_alive():
                    self._thread = threading.Thread(target=self._work, name="skins", daemon=True)
                    self._thread.start()

    def _work(self) -> None:
        while True:
            with self._lock:
                if not self._queue:
                    return
                uuid = self._queue[0]
            try:
                fetch_skin(uuid)
            except (OSError, ValueError, KeyError, TypeError):
                with self._lock:
                    self._failed[uuid] = time.time()
            with self._lock:
                self._queue.remove(uuid)


skins = Skins()
