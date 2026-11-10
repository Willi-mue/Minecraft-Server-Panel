import pytest

from app import config, gamerules, nbt
from app.instances import MAIN


def new_format(rules):
    path = config.SERVER_DIR / "world" / "dimensions" / "minecraft" / "overworld" / "data" / "minecraft"
    path.mkdir(parents=True)
    nbt.save(path / "game_rules.dat", "", {"DataVersion": (nbt.INT, 5023), "data": (nbt.COMPOUND, rules)})
    return path / "game_rules.dat"


def old_format(rules):
    path = config.SERVER_DIR / "world"
    path.mkdir(parents=True)
    nbt.save(path / "level.dat", "", {"Data": (nbt.COMPOUND, {"GameRules": (nbt.COMPOUND, rules)})})
    return path / "level.dat"


def test_new_format_typed_values(data):
    new_format({"minecraft:keep_inventory": (nbt.BYTE, 0), "minecraft:random_tick_speed": (nbt.INT, 3),
                "minecraft:spawn_wardens": (nbt.BYTE, 1)})
    rules = gamerules.read(MAIN)
    assert rules == {"minecraft:keep_inventory": False, "minecraft:random_tick_speed": 3, "minecraft:spawn_wardens": True}
    described = gamerules.describe(rules)
    # Known rules first (with a label), unknown ones after
    assert [r["command"] for r in described] == ["keep_inventory", "random_tick_speed", "spawn_wardens"]
    assert described[0]["label"] == "Keep inventory after death" and described[2]["label"] is None


def test_old_format_strings(data):
    old_format({"keepInventory": (nbt.STRING, "true"), "randomTickSpeed": (nbt.STRING, "3"),
                "odd": (nbt.STRING, "text")})
    assert gamerules.read(MAIN) == {"keepInventory": True, "randomTickSpeed": 3}


def test_validate(data):
    rules = {"minecraft:keep_inventory": False, "minecraft:random_tick_speed": 3}
    assert gamerules.validate(rules, "minecraft:keep_inventory", True) is True
    assert gamerules.validate(rules, "minecraft:random_tick_speed", 10) == 10
    for name, value in [("minecraft:keep_inventory", 1), ("minecraft:random_tick_speed", True),
                        ("minecraft:random_tick_speed", 2**31), ("minecraft:other", True), ("x y", True)]:
        with pytest.raises(ValueError):
            gamerules.validate(rules, name, value)


def test_write_offline_keeps_types(data):
    path = new_format({"minecraft:keep_inventory": (nbt.BYTE, 0), "minecraft:random_tick_speed": (nbt.INT, 3)})
    gamerules.write_offline(MAIN, "minecraft:keep_inventory", True)
    gamerules.write_offline(MAIN, "minecraft:random_tick_speed", 0)
    _, root = nbt.load(path)
    assert root["data"][1] == {"minecraft:keep_inventory": (nbt.BYTE, 1), "minecraft:random_tick_speed": (nbt.INT, 0)}
    assert root["DataVersion"] == (nbt.INT, 5023)


def test_write_offline_old_format(data):
    path = old_format({"keepInventory": (nbt.STRING, "false")})
    gamerules.write_offline(MAIN, "keepInventory", True)
    assert nbt.load(path)[1]["Data"][1]["GameRules"][1]["keepInventory"] == (nbt.STRING, "true")


def test_no_world(data):
    assert gamerules.read(MAIN) == {}
    with pytest.raises(RuntimeError):
        gamerules.write_offline(MAIN, "keepInventory", True)


@pytest.mark.parametrize("line,name,value", [
    ("[x] [Server thread/INFO]: Gamerule keepInventory is currently set to: false", "keepInventory", "false"),
    ("[x] [Server thread/INFO]: System chat: Gamerule keep_inventory is now set to: true", "keep_inventory", "true"),
    ("[x INFO]: Game rule minecraft:random_tick_speed is currently set to: 3", "minecraft:random_tick_speed", "3"),
])
def test_answers(line, name, value):
    m = gamerules.ANSWER_RE.search(line)
    assert (m.group(1), m.group(2)) == (name, value)
