"""Editable subset of server.properties (comments, order and other keys are preserved)."""

from . import access

# Written to server.properties before the first start if missing (panel defaults, differ from Minecraft's)
PRESETS = {"view-distance": "12", "simulation-distance": "12"}

# key -> (type, default, constraint). Defaults are what the server uses if the key is missing.
FIELDS: dict[str, tuple[str, object, object]] = {
    "motd": ("str", "A Minecraft Server", 120),
    "difficulty": ("enum", "easy", ("peaceful", "easy", "normal", "hard")),
    "gamemode": ("enum", "survival", ("survival", "creative", "adventure", "spectator")),
    "max-players": ("int", 20, (1, 1000)),
    # The two biggest performance levers: chunks sent to / ticked around each player
    "view-distance": ("int", 12, (2, 32)),
    "simulation-distance": ("int", 12, (2, 32)),
    "spawn-protection": ("int", 16, (0, 1000)),
    "allow-flight": ("bool", False, None),
    "hardcore": ("bool", False, None),
}


def _parse(key: str, raw: str):
    kind, default, _ = FIELDS[key]
    try:
        if kind == "int":
            return int(raw)
        if kind == "bool":
            return raw.strip().lower() == "true"
        return raw
    except ValueError:
        return default


def get_all() -> dict:
    result = {}
    for key, (_, default, _) in FIELDS.items():
        raw = access.get_property(key, None)
        result[key] = default if raw is None else _parse(key, raw)
    return result


def validate(key: str, value) -> str:
    """Return the value as it is written to the file, or raise ValueError."""
    if key not in FIELDS:
        raise ValueError(f"{key} can't be changed here")
    kind, _, rule = FIELDS[key]
    if kind == "str":
        if not isinstance(value, str) or len(value) > rule or "\n" in value or "\r" in value:
            raise ValueError(f"{key}: text up to {rule} characters, single line")
        return value
    if kind == "enum":
        if value not in rule:
            raise ValueError(f"{key}: one of {', '.join(rule)}")
        return value
    if kind == "int":
        lo, hi = rule
        if isinstance(value, bool) or not isinstance(value, int) or not lo <= value <= hi:
            raise ValueError(f"{key}: whole number from {lo} to {hi}")
        return str(value)
    if kind == "bool":
        if not isinstance(value, bool):
            raise ValueError(f"{key}: true or false")
        return "true" if value else "false"
    raise ValueError(key)


def apply_presets() -> list[str]:
    """Write PRESETS for keys not yet in server.properties. Returns the keys written."""
    written = []
    for key, value in PRESETS.items():
        if access.get_property(key, None) is None:
            access.set_property(key, value)
            written.append(key)
    return written


def update(values: dict) -> dict:
    checked = {key: validate(key, value) for key, value in values.items()}  # all or nothing
    for key, text in checked.items():
        access.set_property(key, text)
    return get_all()
