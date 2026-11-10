"""Game rules of a world, read and written without hard-coding the list.

Minecraft renamed them over time, so names and values always come from the world itself:
- 26.x: `<world>/dimensions/minecraft/overworld/data/minecraft/game_rules.dat`, `data` compound with
  typed values (byte = boolean, int) and ids like `minecraft:keep_inventory`.
- Before: level.dat `Data.GameRules`, string values ("true", "3"), names like `keepInventory`.
While the server runs, values are read with `gamerule <name>` and changed with `gamerule <name> <value>`;
while it is stopped, the file is edited.
"""

import re
from pathlib import Path

from . import backup, nbt

INT_MIN, INT_MAX = -(2**31), 2**31 - 1
_NAME_RE = re.compile(r"^[a-z0-9_.\-]+:[a-z0-9_./\-]+$|^[A-Za-z0-9_]{1,64}$")
# "Gamerule keepInventory is currently set to: false" / "... is now set to: true"
ANSWER_RE = re.compile(r"\]: (?:System chat: )?Game ?rule ([\w:./-]+) is (?:currently|now) set to:? (\S+)", re.I)

# Well-known rules with a plain description; matched by `_key()` (old camelCase and new snake_case ids)
KNOWN = [
    (("keepInventory", "keep_inventory"), "Keep inventory after death"),
    (("doDaylightCycle", "advance_time"), "Day/night cycle"),
    (("doWeatherCycle", "advance_weather"), "Weather changes"),
    (("doMobSpawning", "spawn_mobs"), "Mobs spawn"),
    (("spawn_monsters",), "Monsters spawn"),
    (("mobGriefing", "mob_griefing"), "Mobs change blocks (creepers, endermen)"),
    (("pvp",), "Players can hurt each other (PvP)"),
    (("playersSleepingPercentage", "players_sleeping_percentage"), "Players that must sleep to skip the night (%)"),
    (("naturalRegeneration", "natural_health_regeneration"), "Health regenerates when fed"),
    (("doImmediateRespawn", "immediate_respawn"), "Respawn without the death screen"),
    (("showDeathMessages", "show_death_messages"), "Death messages in chat"),
    (("announceAdvancements", "show_advancement_messages"), "Advancement messages in chat"),
    (("doInsomnia", "spawn_phantoms"), "Phantoms spawn"),
    (("doFireTick",), "Fire spreads"),
    (("fire_spread_radius_around_player",), "Fire spreads within this distance of players (0 = never)"),
    (("tntExplodes", "tnt_explodes"), "TNT explodes"),
    (("fallDamage", "fall_damage"), "Fall damage"),
    (("randomTickSpeed", "random_tick_speed"), "Random tick speed (crop growth, 3 = normal)"),
    (("spawnRadius", "respawn_radius"), "Respawn radius around the world spawn"),
    (("doTraderSpawning", "spawn_wandering_traders"), "Wandering traders spawn"),
    (("doPatrolSpawning", "spawn_patrols"), "Pillager patrols spawn"),
    (("disableRaids", "raids"), "Raids"),
    (("locatorBar", "locator_bar"), "Locator bar"),
    (("sendCommandFeedback", "send_command_feedback"), "Command feedback in chat"),
    (("commandBlockOutput", "command_block_output"), "Command blocks report in chat"),
]


def _key(name: str) -> str:
    return name.split(":")[-1].replace("_", "").lower()


_LABELS = {_key(n): label for names, label in KNOWN for n in names}
_ORDER = {_key(n): i for i, (names, _) in enumerate(KNOWN) for n in names}


def _world(inst) -> Path:
    return inst.server_dir / backup.level_name(inst)


def _new_file(inst) -> Path:
    return _world(inst) / "dimensions" / "minecraft" / "overworld" / "data" / "minecraft" / "game_rules.dat"


def _source(inst) -> tuple[Path, str, dict, dict] | None:
    """(file, root name, root, rules compound) of where this world keeps its rules."""
    new = _new_file(inst)
    if new.is_file():
        name, root = nbt.load(new)
        data = root.get("data")
        if data and data[0] == nbt.COMPOUND:
            return new, name, root, data[1]
    old = _world(inst) / "level.dat"
    if old.is_file():
        name, root = nbt.load(old)
        data = root.get("Data", (nbt.COMPOUND, {}))[1]
        rules = data.get("GameRules")
        if rules and rules[0] == nbt.COMPOUND:
            return old, name, root, rules[1]
    return None


def _typed(tag) -> bool | int | None:
    t, v = tag
    if t == nbt.BYTE:
        return bool(v)
    if t in (nbt.INT, nbt.SHORT, nbt.LONG):
        return int(v)
    if t == nbt.STRING:
        return parse_value(v)
    return None


def parse_value(text: str) -> bool | int | None:
    text = text.strip().rstrip(".")
    if text.lower() in ("true", "false"):
        return text.lower() == "true"
    try:
        return int(text)
    except ValueError:
        return None


def read(inst) -> dict[str, bool | int]:
    """name -> value as stored in the world (as of its last save). Empty if there is no world yet."""
    try:
        src = _source(inst)
    except (OSError, ValueError, EOFError):
        return {}
    if not src:
        return {}
    out = {}
    for name, tag in src[3].items():
        value = _typed(tag)
        if value is not None:
            out[name] = value
    return out


def command_name(name: str) -> str:
    """The id to use in `/gamerule` (the default namespace may be left out)."""
    return name.removeprefix("minecraft:")


def describe(rules: dict[str, bool | int]) -> list[dict]:
    """Rules for the UI: well-known ones first (with a label), then the rest alphabetically."""
    out = [{"name": n, "command": command_name(n), "value": v, "type": "bool" if isinstance(v, bool) else "int",
            "label": _LABELS.get(_key(n))} for n, v in rules.items()]
    out.sort(key=lambda r: (r["label"] is None, _ORDER.get(_key(r["name"]), 0), r["name"].lower()))
    return out


def validate(rules: dict, name: str, value) -> bool | int:
    """Check a change against the rules the world has. Returns the value to set."""
    if not isinstance(name, str) or not _NAME_RE.match(name) or name not in rules:
        raise ValueError(f"Unknown game rule: {name}")
    if isinstance(rules[name], bool):
        if not isinstance(value, bool):
            raise ValueError(f"{name}: true or false")
        return value
    if isinstance(value, bool) or not isinstance(value, int) or not INT_MIN <= value <= INT_MAX:
        raise ValueError(f"{name}: a whole number")
    return value


def text(value: bool | int) -> str:
    return ("true" if value else "false") if isinstance(value, bool) else str(value)


def write_offline(inst, name: str, value: bool | int) -> None:
    """Change one rule in the world file (server stopped). Keeps the stored type."""
    src = _source(inst)
    if not src:
        raise RuntimeError("The world doesn't exist yet: start the server once")
    path, root_name, root, rules = src
    t = rules[name][0] if name in rules else None
    if t is None:
        raise ValueError(f"Unknown game rule: {name}")
    if t == nbt.STRING:
        rules[name] = (nbt.STRING, text(value))
    elif t == nbt.BYTE:
        rules[name] = (nbt.BYTE, 1 if value else 0)
    else:
        rules[name] = (t, int(value))
    nbt.save(path, root_name, root)
