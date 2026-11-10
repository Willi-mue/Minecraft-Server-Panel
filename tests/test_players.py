import json

from app import config, nbt, players
from app.instances import MAIN

UUID = "c2c64826-10bf-428e-9097-606bda04cd1d"


def world():
    w = config.SERVER_DIR / "world"
    w.mkdir(parents=True, exist_ok=True)
    return w


def usercache():
    (config.SERVER_DIR / "usercache.json").write_text(json.dumps(
        [{"uuid": UUID, "name": "Steve", "expiresOn": "2026-12-09 20:22:59 +0100"}]))


def test_uuid_from_usercache_and_lists(data):
    usercache()
    assert players.uuid_of(MAIN, "steve") == UUID
    assert players.uuid_of(MAIN, "Alex") is None
    (config.SERVER_DIR / "ops.json").write_text(json.dumps([{"uuid": "0" * 32, "name": "Alex", "level": 4}]))
    assert players.uuid_of(MAIN, "alex") == "00000000-0000-0000-0000-000000000000"


def test_seen_first_and_last(data):
    players.record(MAIN, "Steve", "join", now=100)
    players.record(MAIN, "Steve", "leave", now=200)
    players.record(MAIN, "Steve", "join", now=300)
    assert players.seen(MAIN, "STEVE") == {"name": "Steve", "first_seen": 100, "last_seen": 300, "online": True}
    players.all_offline(MAIN)
    assert players.seen(MAIN, "Steve")["online"] is False


def test_profile_26x_layout(data):
    usercache()
    w = world()
    (w / "players" / "stats").mkdir(parents=True)
    (w / "players" / "stats" / f"{UUID}.json").write_text(json.dumps({"stats": {"minecraft:custom": {
        "minecraft:play_time": 72000, "minecraft:deaths": 2, "minecraft:walk_one_cm": 150000}}}))
    (w / "players" / "data").mkdir(parents=True)
    nbt.save(w / "players" / "data" / f"{UUID}.dat", "", {
        "Pos": (nbt.LIST, (nbt.DOUBLE, [-6.37, 77.0, -15.86])),
        "Dimension": (nbt.STRING, "minecraft:the_nether"),
        "playerGameType": (nbt.INT, 1),
        "Health": (nbt.FLOAT, 20.0),
        "XpLevel": (nbt.INT, 7),
    })
    p = players.profile(MAIN, "steve")
    assert p["name"] == "Steve" and p["uuid"] == UUID
    assert p["stats"]["play_time"] == 3600 and p["stats"]["deaths"] == 2 and p["stats"]["distance_km"] == 1.5
    assert p["pos"] == [-6.4, 77.0, -15.9] and p["dimension"] == "minecraft:the_nether"
    assert p["gamemode"] == "creative" and p["health"] == 20.0 and p["level"] == 7


def test_profile_old_layout(data):
    usercache()
    w = world()
    (w / "stats").mkdir()
    (w / "stats" / f"{UUID}.json").write_text(json.dumps({"stats": {"minecraft:custom": {"minecraft:play_one_minute": 1200}}}))
    (w / "playerdata").mkdir()
    nbt.save(w / "playerdata" / f"{UUID}.dat", "", {"Dimension": (nbt.INT, -1)})
    p = players.profile(MAIN, "Steve")
    assert p["stats"]["play_time"] == 60 and p["dimension"] == "minecraft:the_nether"


def test_unknown_player_profile(data):
    p = players.profile(MAIN, "Nobody")
    assert p["uuid"] is None and "stats" not in p and p["ban"] is None


def test_world_spawn_both_formats(data):
    w = world()
    nbt.save(w / "level.dat", "", {"Data": (nbt.COMPOUND, {"spawn": (nbt.COMPOUND, {
        "pos": (nbt.INTS, [10, 65, -3]), "dimension": (nbt.STRING, "minecraft:overworld")})})})
    assert players.world_spawn(MAIN) == ("minecraft:overworld", [10, 65, -3])
    nbt.save(w / "level.dat", "", {"Data": (nbt.COMPOUND, {
        "SpawnX": (nbt.INT, 1), "SpawnY": (nbt.INT, 70), "SpawnZ": (nbt.INT, 2)})})
    assert players.world_spawn(MAIN) == ("minecraft:overworld", [1, 70, 2])


def test_no_skin_for_unknown_or_offline_mode(data, monkeypatch):
    calls = []
    monkeypatch.setattr(players.livemap, "fetch_skin", lambda u: calls.append(u))
    assert players.skin(MAIN, "Nobody") is None
    usercache()
    (config.SERVER_DIR / "server.properties").write_text("online-mode=false\n")
    assert players.skin(MAIN, "Steve") is None
    assert calls == []
