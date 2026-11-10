"""Hall of fame for the live map: advancements and statistics of every player of a server, for a
tier list and lots of "who is the best at X" records.

Read from the world (26.x `world/players/{stats,advancements}/<uuid>.json`, before that
`world/{stats,advancements}/<uuid>.json`); names from `usercache.json`. Advancement titles,
descriptions and frames (task / goal / challenge) and the names of mobs and blocks come from the
Minecraft client jar the live map already downloaded (`data/minecraft/advancement/**`,
`assets/minecraft/lang/en_us.json`); without it, ids are turned into names.

Minecraft writes these files only on autosave (every 5 minutes) or when a player leaves: while
players are online, opening the hall of fame asks the server for a `save-all` (at most once a
minute, never during a backup) and the page reloads a few seconds later.

Served on the public map port (`/stats.json`), so it holds only what the hall of fame shows: names,
skin ids, counts and advancement / mob / block ids. No positions, no inventories, no uuids.

"Me" (`/me.json`): the address each player last logged in from is kept in memory only (never on
disk, never sent anywhere); a map visitor from exactly one player's address is offered that player.
"""
import ipaddress

import json
import threading
import time
import zipfile
from pathlib import Path

from . import config, livemap, players
from .instances import Instance

FRAMES = ("task", "goal", "challenge")
SAVE_EVERY = 60     # s: at most one save-all per server
RECHECK_EVERY = 3   # s: how often the file times are compared

# tier by the share of all advancements done (recipes don't count)
TIERS = (("S", 0.6), ("A", 0.4), ("B", 0.25), ("C", 0.1), ("D", 0.0))

# podiums: group, key, title, unit ("m" = metres, shown as km from 1000 m; "♥" = hearts)
RECORDS = (
    ("Movement", "walked", "Walked", "m"),
    ("Movement", "sprinted", "Sprinted", "m"),
    ("Movement", "swum", "Swum", "m"),
    ("Movement", "elytra", "Flown with elytra", "m"),
    ("Movement", "boat", "Sailed by boat", "m"),
    ("Movement", "horse", "Ridden on horseback", "m"),
    ("Movement", "minecart", "Rode a minecart", "m"),
    ("Movement", "climbed", "Climbed", "m"),
    ("Movement", "fallen", "Fallen", "m"),
    ("Movement", "sneaked", "Sneaked", "m"),
    ("Movement", "jumps", "Jumps", ""),
    ("Combat", "mob_kills", "Mobs killed", ""),
    ("Combat", "player_kills", "Players killed", ""),
    ("Combat", "damage_dealt", "Damage dealt", "♥"),
    ("Combat", "damage_taken", "Damage taken", "♥"),
    ("Combat", "deaths", "Deaths", ""),
    ("Combat", "alive", "Alive since the last death", "h"),
    ("Mining", "mined", "Blocks mined", ""),
    ("Mining", "ores", "Ores mined", ""),
    ("Mining", "diamonds", "Diamond ore mined", ""),
    ("Mining", "debris", "Ancient debris mined", ""),
    ("Mining", "logs", "Logs chopped", ""),
    ("Mining", "tools_broken", "Tools worn out", ""),
    ("Life", "play_time", "Time played", "h"),
    ("Life", "advancements", "Advancements", ""),
    ("Life", "challenges", "Challenges", ""),
    ("Life", "fish", "Fish caught", ""),
    ("Life", "bred", "Animals bred", ""),
    ("Life", "trades", "Villager trades", ""),
    ("Life", "crafted", "Items crafted", ""),
    ("Life", "enchants", "Items enchanted", ""),
    ("Life", "cake", "Cake slices eaten", ""),
    ("Life", "sleeps", "Nights slept", ""),
    ("Life", "chests", "Chests opened", ""),
    ("Life", "noteblocks", "Note blocks played", ""),
)

# distances in minecraft:custom (cm) -> our keys (m)
_DISTANCES = {
    "walked": ("walk_one_cm",), "sprinted": ("sprint_one_cm",), "sneaked": ("crouch_one_cm",),
    "swum": ("swim_one_cm", "walk_under_water_one_cm", "walk_on_water_one_cm"), "elytra": ("aviate_one_cm",),
    "boat": ("boat_one_cm",), "horse": ("horse_one_cm",), "minecart": ("minecart_one_cm",),
    "climbed": ("climb_one_cm",), "fallen": ("fall_one_cm",),
}
# plain counters in minecraft:custom
_COUNTERS = {
    "jumps": "jump", "mob_kills": "mob_kills", "player_kills": "player_kills", "deaths": "deaths",
    "fish": "fish_caught", "bred": "animals_bred", "trades": "traded_with_villager", "enchants": "enchant_item",
    "cake": "eat_cake_slice", "sleeps": "sleep_in_bed", "chests": "open_chest", "noteblocks": "play_noteblock",
}

_lock = threading.Lock()
_cache: dict[str, tuple[float, tuple, dict]] = {}  # id -> (checked, signature, result)
_saved: dict[str, float] = {}
_jar_cache: dict[str, tuple[float, dict, dict]] = {}  # jar -> (mtime, advancements, lang)
_logins: dict[str, dict[str, str]] = {}  # server id -> player name -> address of the last login


def _address(raw: str) -> str | None:
    """Normalised IP (IPv4-mapped IPv6 as IPv4), None if it is none."""
    try:
        ip = ipaddress.ip_address(raw.strip("[]"))
    except ValueError:
        return None
    if isinstance(ip, ipaddress.IPv6Address) and ip.ipv4_mapped:
        ip = ip.ipv4_mapped
    return str(ip)


def remember_login(inst: Instance, name: str, raw_address: str) -> None:
    """From the server log ("Name[/1.2.3.4:5678] logged in ..."); memory only."""
    address = _address(raw_address)
    if address:
        with _lock:
            _logins.setdefault(inst.id, {})[name] = address


def me(inst: Instance, raw_address: str | None) -> str | None:
    """The player who last logged in from this address, if it is exactly one."""
    address = _address(raw_address or "")
    with _lock:
        names = [n for n, a in _logins.get(inst.id, {}).items() if a == address] if address else []
    return names[0] if len(names) == 1 else None


def _pretty(an_id: str) -> str:
    return an_id.split(":")[-1].split("/")[-1].replace("_", " ").capitalize()


def _jar_tables(jar: Path | None) -> tuple[dict, dict]:
    """(advancements, lang) from the client jar; empty without one."""
    if not jar or not jar.exists():
        return {}, {}
    key, mtime = str(jar), jar.stat().st_mtime
    hit = _jar_cache.get(key)
    if hit and hit[0] == mtime:
        return hit[1], hit[2]
    advs = {}
    with zipfile.ZipFile(jar) as z:
        try:
            lang = json.loads(z.read("assets/minecraft/lang/en_us.json"))
        except (KeyError, ValueError):
            lang = {}
        for name in z.namelist():
            for folder in ("data/minecraft/advancement/", "data/minecraft/advancements/"):
                if not name.startswith(folder) or not name.endswith(".json"):
                    continue
                path = name[len(folder):-5]
                if path.startswith("recipes/"):
                    continue
                try:
                    display = json.loads(z.read(name)).get("display")
                except (ValueError, AttributeError):
                    continue
                if not isinstance(display, dict):
                    continue
                text = lambda part: (lang.get(display[part].get("translate", ""))
                                     if isinstance(display.get(part), dict) else None)
                advs[f"minecraft:{path}"] = {
                    "title": text("title") or _pretty(path),
                    "description": text("description") or "",
                    "frame": display.get("frame") if display.get("frame") in FRAMES else "task",
                    "category": path.split("/")[0],
                }
    lang = {k: v for k, v in lang.items() if k.startswith(("entity.minecraft.", "block.minecraft."))}
    _jar_cache[key] = (mtime, advs, lang)
    return advs, lang


def catalog(jar: Path | None) -> dict:
    """id -> {title, description, frame, category} of every advancement with a display (no recipes)."""
    return _jar_tables(jar)[0]


def _client_jar(inst: Instance) -> Path | None:
    """The client jar the live map uses for this world, only if it is already there (never downloads)."""
    version = livemap._map_version(inst)
    path = config.DATA_DIR / "cache" / f"client-{version}.jar"
    return path if version and path.exists() else None


def _read_json(path: Path) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def _player_files(world: Path, kind: str) -> dict[str, Path]:
    """uuid -> file of `kind` (stats / advancements); the 26.x layout wins."""
    out: dict[str, Path] = {}
    for folder in (world / kind, world / "players" / kind):
        if folder.is_dir():
            for f in folder.glob("*.json"):
                out[f.stem.lower()] = f
    return out


def _num(v) -> int:
    return int(v) if isinstance(v, (int, float)) else 0


def _group(s: dict, name: str) -> dict:
    g = s.get(f"minecraft:{name}")
    return {k: _num(v) for k, v in g.items()} if isinstance(g, dict) else {}


def stats_of(data: dict) -> tuple[dict, dict, dict]:
    """(stats, killed, killed_by) of a stats file: our keys -> numbers, entity id -> count."""
    s = data.get("stats") if isinstance(data.get("stats"), dict) else {}
    custom, mined = _group(s, "custom"), _group(s, "mined")
    c = lambda key: custom.get(f"minecraft:{key}", 0)
    out = {k: round(sum(c(x) for x in keys) / 100) for k, keys in _DISTANCES.items()}
    out.update({k: c(v) for k, v in _COUNTERS.items()})
    out["play_time"] = round((c("play_time") or c("play_one_minute")) / 72000, 1)  # ticks -> hours
    out["alive"] = round(c("time_since_death") / 72000, 1) if c("time_since_death") else 0
    out["damage_dealt"] = round(c("damage_dealt") / 20)  # tenths of a health point -> hearts
    out["damage_taken"] = round(c("damage_taken") / 20)
    out["mined"] = sum(mined.values())
    out["ores"] = sum(v for k, v in mined.items() if k.endswith("_ore") or k == "minecraft:ancient_debris")
    out["diamonds"] = mined.get("minecraft:diamond_ore", 0) + mined.get("minecraft:deepslate_diamond_ore", 0)
    out["debris"] = mined.get("minecraft:ancient_debris", 0)
    out["logs"] = sum(v for k, v in mined.items() if k.endswith(("_log", "_stem")))
    out["crafted"] = sum(_group(s, "crafted").values())
    out["tools_broken"] = sum(_group(s, "broken").values())
    return out, _group(s, "killed"), _group(s, "killed_by")


def _done(data: dict) -> list[str]:
    return sorted(k for k, v in data.items()
                  if isinstance(v, dict) and v.get("done") and ":" in k and not k.split(":", 1)[1].startswith("recipes/"))


def tier_of(share: float) -> str:
    return next(t for t, low in TIERS if share >= low)


def _signature(inst: Instance) -> tuple:
    """What the result depends on: the player files, usercache.json and the client jar."""
    world = players._world(inst)
    files = [inst.server_dir / "usercache.json"]
    for kind in ("stats", "advancements"):
        files += _player_files(world, kind).values()
    jar = _client_jar(inst)
    if jar:
        files.append(jar)
    sig = []
    for f in files:
        try:
            sig.append((str(f), f.stat().st_mtime_ns))
        except OSError:
            pass
    return tuple(sorted(sig))


def request_save(inst: Instance) -> bool:
    """While players are online, ask the server to write their stats (save-all), at most once a
    minute and never during another action (a backup runs with saving turned off)."""
    from .server import managers  # late: server.py is the bigger module
    m = managers.get(inst.id)
    if m is None or not m.is_alive() or m.task or not m.players:
        return False
    if time.monotonic() - _saved.get(inst.id, -1e9) < SAVE_EVERY:
        return False
    try:
        m.send_command("save-all")
    except RuntimeError:
        return False
    _saved[inst.id] = time.monotonic()
    return True


def _compute(inst: Instance) -> dict:
    world = players._world(inst)
    advs, lang = _jar_tables(_client_jar(inst))
    names = {e["uuid"]: e["name"] for e in players.usercache(inst).values()}
    stat_files, adv_files = _player_files(world, "stats"), _player_files(world, "advancements")
    rows, seen_ids = [], set()
    for uuid in sorted(set(stat_files) | set(adv_files)):
        dashed = players._dashed(uuid) if len(uuid.replace("-", "")) == 32 else uuid
        name = names.get(dashed)
        if not name:
            continue  # unknown to this server's usercache (e.g. a copied world): no name to show
        done = _done(_read_json(adv_files[uuid])) if uuid in adv_files else []
        if advs:
            done = [a for a in done if a in advs]
        stats, killed, killed_by = stats_of(_read_json(stat_files[uuid]) if uuid in stat_files else {})
        stats["advancements"] = len(done)
        stats["challenges"] = sum(1 for a in done if advs.get(a, {}).get("frame") == "challenge")
        seen_ids.update(killed, killed_by)
        plain = dashed.replace("-", "")  # skins are stored without dashes (livemap.skin_path)
        rows.append({"name": name, "skin": plain if livemap.skin_path(plain).exists() else None,
                     "done": done, "stats": stats, "killed": killed, "killed_by": killed_by})
    total = len(advs) or max((len(r["done"]) for r in rows), default=0) or 1
    for r in rows:
        r["share"] = round(len(r["done"]) / total, 3)
        r["tier"] = tier_of(r["share"])
    rows.sort(key=lambda r: (-r["share"], -r["stats"]["play_time"], r["name"].lower()))
    known = advs or {a: {"title": _pretty(a), "description": "", "frame": "task",
                         "category": a.split(":", 1)[1].split("/")[0]} for r in rows for a in r["done"]}
    mob_names = {i: lang.get(f"entity.{i.replace(':', '.')}") or _pretty(i) for i in sorted(seen_ids)}
    return {
        "generated": int(time.time()),
        "total": total,
        "tiers": [{"tier": t, "from": low} for t, low in TIERS],
        "records": [{"group": g, "key": k, "title": title, "unit": unit} for g, k, title, unit in RECORDS],
        "advancements": known,
        "mobs": mob_names,
        "players": rows,
    }


def build(inst: Instance) -> dict:
    """The hall of fame of a server; recomputed when a player file changed. `saving`: a save-all
    was just requested, newer numbers follow in a few seconds."""
    saving = request_save(inst)
    now = time.monotonic()
    with _lock:
        hit = _cache.get(inst.id)
    if hit and now - hit[0] < RECHECK_EVERY:
        return {**hit[2], "saving": saving}
    sig = _signature(inst)
    if hit and hit[1] == sig:
        result = hit[2]
    else:
        result = _compute(inst)
    with _lock:
        _cache[inst.id] = (now, sig, result)
    return {**result, "saving": saving}
