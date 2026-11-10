"""Player profiles: name <-> uuid, play time, first/last seen, last position, game mode, skins.

- `usercache.json` (written by the server) maps names to uuids.
- Statistics and player data live in the world: 26.x `world/players/{stats,data}/<uuid>`, before
  that `world/stats/<uuid>.json` and `world/playerdata/<uuid>.dat`.
- First/last seen is recorded by the panel on join/leave in `<instance root>/players.json`.
- Skins: only from Mojang (session server -> textures.minecraft.net), shared cache with the live map
  (`livemap.skin_path`, `DATA_DIR/cache/skins/<uuid>.png`). The browser cuts out the face.
"""

import json
import re
import threading
import time
from pathlib import Path

from . import access, backup, livemap, nbt

GAMEMODES = ("survival", "creative", "adventure", "spectator")
_UUID_RE = re.compile(r"^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$")

_seen_lock = threading.Lock()
_skin_failed: dict[str, float] = {}
_skin_lock = threading.Lock()


def _dashed(u: str) -> str:
    u = u.replace("-", "").lower()
    return f"{u[:8]}-{u[8:12]}-{u[12:16]}-{u[16:20]}-{u[20:]}"


# ---------- name <-> uuid ----------


def usercache(inst) -> dict[str, dict]:
    """lower-case name -> {"name", "uuid"} (uuid with dashes)."""
    try:
        entries = json.loads((inst.server_dir / "usercache.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    out = {}
    for e in entries if isinstance(entries, list) else []:
        if isinstance(e, dict) and isinstance(e.get("name"), str) and _UUID_RE.match(str(e.get("uuid", "")).lower()):
            out[e["name"].lower()] = {"name": e["name"], "uuid": _dashed(e["uuid"])}
    return out


def uuid_of(inst, name: str) -> str | None:
    """Dashed uuid from usercache.json, then from whitelist/ops/bans (the server writes uuids there)."""
    hit = usercache(inst).get(name.lower())
    if hit:
        return hit["uuid"]
    for file in ("whitelist.json", "ops.json", "banned-players.json"):
        for e in access._read_list(inst, file):
            if str(e.get("name", "")).lower() == name.lower() and _UUID_RE.match(str(e.get("uuid", "")).lower()):
                return _dashed(e["uuid"])
    return None


# ---------- first / last seen ----------


def _seen_file(inst) -> Path:
    return inst.root / "players.json"


def _read_seen(inst) -> dict:
    try:
        data = json.loads(_seen_file(inst).read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def record(inst, name: str, event: str, now: float | None = None) -> None:
    """event: "join" | "leave"."""
    now = int(now or time.time())
    with _seen_lock:
        data = _read_seen(inst)
        entry = data.setdefault(name.lower(), {"name": name, "first_seen": now})
        entry["name"] = name
        entry["last_seen"] = now
        entry["online"] = event == "join"
        try:
            path = _seen_file(inst)
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(".tmp")
            tmp.write_text(json.dumps(data, indent=1), encoding="utf-8")
            tmp.replace(path)
        except OSError:
            pass


def all_offline(inst) -> None:
    """Server stopped: everybody who was online left now."""
    for e in [e for e in _read_seen(inst).values() if isinstance(e, dict) and e.get("online")]:
        record(inst, e.get("name", "?"), "leave")


def seen(inst, name: str) -> dict:
    e = _read_seen(inst).get(name.lower())
    return e if isinstance(e, dict) else {}


# ---------- world data ----------


def _world(inst) -> Path:
    return inst.server_dir / backup.level_name(inst)


def _first(*paths: Path) -> Path | None:
    return next((p for p in paths if p.is_file()), None)


def stats(inst, uuid: str) -> dict:
    w = _world(inst)
    path = _first(w / "players" / "stats" / f"{uuid}.json", w / "stats" / f"{uuid}.json")
    if not path:
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        custom = data.get("stats", {}).get("minecraft:custom", {})
    except (OSError, ValueError, AttributeError):
        return {}
    ticks = custom.get("minecraft:play_time", custom.get("minecraft:play_one_minute", 0))
    return {
        "play_time": int(ticks) // 20,  # seconds
        "deaths": int(custom.get("minecraft:deaths", 0)),
        "mob_kills": int(custom.get("minecraft:mob_kills", 0)),
        "player_kills": int(custom.get("minecraft:player_kills", 0)),
        "distance_km": round(sum(int(v) for k, v in custom.items() if k.endswith("_one_cm")) / 100_000, 1),
    }


def player_data(inst, uuid: str) -> dict:
    """Last saved position, dimension, game mode, health, level (as of the last world save)."""
    w = _world(inst)
    path = _first(w / "players" / "data" / f"{uuid}.dat", w / "playerdata" / f"{uuid}.dat")
    if not path:
        return {}
    try:
        _, root = nbt.load(path)
    except (OSError, ValueError, EOFError):
        return {}
    out: dict = {}
    pos = root.get("Pos")
    if pos and pos[0] == nbt.LIST and len(pos[1][1]) == 3:
        out["pos"] = [round(float(v), 1) for v in pos[1][1]]
    dim = root.get("Dimension")
    if dim:
        out["dimension"] = dim[1] if dim[0] == nbt.STRING else {-1: "minecraft:the_nether", 0: "minecraft:overworld",
                                                                  1: "minecraft:the_end"}.get(dim[1], str(dim[1]))
    mode = root.get("playerGameType")
    if mode and isinstance(mode[1], int) and 0 <= mode[1] < len(GAMEMODES):
        out["gamemode"] = GAMEMODES[mode[1]]
    for key, name in (("Health", "health"), ("XpLevel", "level"), ("foodLevel", "food")):
        if key in root and isinstance(root[key][1], (int, float)):
            out[name] = round(float(root[key][1]), 1) if key == "Health" else int(root[key][1])
    return out


def world_spawn(inst) -> tuple[str, list[int]] | None:
    """(dimension, [x, y, z]) of the world spawn from level.dat (26.x `Data.spawn`, else SpawnX/Y/Z)."""
    try:
        _, root = nbt.load(_world(inst) / "level.dat")
    except (OSError, ValueError, EOFError):
        return None
    data = root.get("Data", (nbt.COMPOUND, {}))[1]
    spawn = data.get("spawn")
    if spawn and spawn[0] == nbt.COMPOUND:
        pos = spawn[1].get("pos")
        dim = spawn[1].get("dimension", (nbt.STRING, "minecraft:overworld"))[1]
        if pos and pos[0] == nbt.INTS and len(pos[1]) == 3:
            return str(dim), list(pos[1])
    if all(k in data for k in ("SpawnX", "SpawnY", "SpawnZ")):
        return "minecraft:overworld", [int(data[k][1]) for k in ("SpawnX", "SpawnY", "SpawnZ")]
    return None


def ban_entry(inst, name: str) -> dict | None:
    for e in access._read_list(inst, "banned-players.json"):
        if str(e.get("name", "")).lower() == name.lower():
            return {k: e.get(k) for k in ("reason", "source", "created", "expires")}
    return None


def profile(inst, name: str) -> dict:
    uuid = uuid_of(inst, name)
    s = seen(inst, name)
    out = {
        "name": s.get("name") or (usercache(inst).get(name.lower()) or {}).get("name") or name,
        "uuid": uuid,
        "first_seen": s.get("first_seen"),
        "last_seen": s.get("last_seen"),
        "ban": ban_entry(inst, name),
    }
    if uuid:
        if s := stats(inst, uuid):  # no file before the player's first visit
            out["stats"] = s
        out.update(player_data(inst, uuid))
    return out


# ---------- skins ----------

SKIN_RETRY = 3600


def skin(inst, name: str) -> Path | None:
    """The cached skin PNG of a player, downloaded from Mojang if missing/old. None = use a default head
    (offline-mode server, unknown player, Mojang unreachable)."""
    uuid = uuid_of(inst, name)
    if not uuid:
        return None
    raw = uuid.replace("-", "")
    path = livemap.skin_path(raw)
    try:
        fresh = time.time() - path.stat().st_mtime < livemap.SKIN_MAX_AGE
    except OSError:
        fresh = False
    if fresh:
        return path
    if access.get_property(inst, "online-mode", "true").lower() == "false":
        return path if path.exists() else None  # offline uuids aren't Mojang accounts
    with _skin_lock:
        if time.time() - _skin_failed.get(raw, 0) < SKIN_RETRY:
            return path if path.exists() else None
    try:
        return livemap.fetch_skin(raw)
    except (OSError, ValueError, KeyError, TypeError):
        with _skin_lock:
            _skin_failed[raw] = time.time()
        return path if path.exists() else None
