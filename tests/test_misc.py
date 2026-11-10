import gzip
import json
import struct
import uuid

import pytest

from app import access, config, java, livemap, properties, world
from app.instances import MAIN


# ---------- config ----------

@pytest.mark.parametrize("value", ["512M", "4G", " 2g "])
def test_valid_ram(value):
    assert config.validate_ram(value) == value.strip().upper()


@pytest.mark.parametrize("value", ["0G", "abc", "1T", ""])
def test_invalid_ram(value):
    with pytest.raises(ValueError):
        config.validate_ram(value)


@pytest.mark.parametrize("value", ["1.21.4", "26.4-snapshot-7", "1.14 Pre-Release 1", "b1.7.3"])
def test_valid_versions(value):
    assert config.validate_version(value) == value


@pytest.mark.parametrize("value", ["../x", "a/b", "..\\x", "", ".hidden"])
def test_invalid_versions(value):
    with pytest.raises(ValueError):
        config.validate_version(value)


def test_settings_ignore_wrong_types(data):
    config.SETTINGS_FILE.write_text(json.dumps({"version": "1.21.4", "map_enabled": "yes", "max_ram": 5}))
    s = config.load_settings()
    assert s.version == "1.21.4"
    assert s.map_enabled is True  # string ignored, default kept
    assert s.max_ram == "3G"


# ---------- java ----------

def test_java_pick(monkeypatch):
    monkeypatch.setattr(java, "runtimes", lambda: {17: "j17", 21: "j21", 25: "j25"})
    assert java.pick(21) == ("j21", 21)
    assert java.pick(18) == ("j21", 21)  # smallest that is new enough
    assert java.pick(8) == ("j17", 17)
    assert java.pick(26) == ("j25", 25)  # best effort: newest


# ---------- access ----------

def test_offline_uuid_matches_java():
    # Known value of UUID.nameUUIDFromBytes("OfflinePlayer:Steve")
    assert access._offline_uuid("Steve") == "5627dd98-e6be-3c21-b8a8-e92344183641"
    assert uuid.UUID(access._offline_uuid("x")).version == 3


def test_properties_roundtrip_keeps_other_lines(data):
    p = config.SERVER_DIR / "server.properties"
    p.write_text("#comment\nmotd=Hi\nonline-mode=true\n")
    access.set_property(MAIN, "online-mode", "false")
    access.set_property(MAIN, "white-list", "true")
    assert p.read_text().splitlines() == ["#comment", "motd=Hi", "online-mode=false", "white-list=true"]
    assert access.get_property(MAIN, "motd") == "Hi"


def test_offline_whitelist_add_remove(data):
    access.set_property(MAIN, "online-mode", "false")  # no network lookup
    access.offline_change(MAIN, "whitelist", "add", "Steve")
    with pytest.raises(LookupError):
        access.offline_change(MAIN, "whitelist", "add", "steve")
    assert access.state(MAIN)["whitelist"] == ["Steve"]
    access.offline_change(MAIN, "whitelist", "remove", "STEVE")
    assert access.state(MAIN)["whitelist"] == []


@pytest.mark.parametrize("name", ["ab", "a" * 17, "a;op x", "a b"])
def test_invalid_player_names(name):
    with pytest.raises(ValueError):
        access.validate_name(name)


# ---------- server.properties editor ----------

def test_properties_validation(data):
    assert properties.update(MAIN, {"view-distance": 6, "difficulty": "hard", "allow-flight": True})["view-distance"] == 6
    for bad in ({"view-distance": 99}, {"difficulty": "extreme"}, {"motd": "a\nb"}, {"online-mode": False},
                {"max-players": True}):
        with pytest.raises(ValueError):
            properties.update(MAIN, bad)
    assert properties.get_all(MAIN)["difficulty"] == "hard"


# ---------- world version from level.dat ----------

def _nbt_string(s):
    b = s.encode()
    return struct.pack(">H", len(b)) + b


def _level_dat(version_name):
    version = b"\x08" + _nbt_string("Name") + _nbt_string(version_name) + b"\x03" + _nbt_string("Id") + struct.pack(">i", 4325) + b"\x00"
    data = (b"\x0a" + _nbt_string("Version") + version
            + b"\x01" + _nbt_string("hardcore") + b"\x00"
            + b"\x09" + _nbt_string("list") + b"\x03" + struct.pack(">i", 2) + struct.pack(">ii", 1, 2)
            + b"\x0c" + _nbt_string("longs") + struct.pack(">i", 1) + struct.pack(">q", 7)
            + b"\x00")
    return gzip.compress(b"\x0a" + _nbt_string("") + b"\x0a" + _nbt_string("Data") + data + b"\x00")


def test_world_version_from_level_dat(data):
    (config.SERVER_DIR / "world").mkdir()
    world.level_dat(MAIN).write_bytes(_level_dat("1.21.4"))
    assert world.saved_version(MAIN) == "1.21.4"


def test_world_version_missing_or_broken(data):
    assert world.saved_version(MAIN) is None
    (config.SERVER_DIR / "world").mkdir()
    world.level_dat(MAIN).write_bytes(b"not gzip")
    assert world.saved_version(MAIN) is None


# ---------- live map ----------

def test_player_positions_are_parsed_and_hidden_from_console():
    t = livemap.PlayerTracker()
    cmds = t.queries({"Steve"})
    assert cmds == ["data get entity Steve Pos", "data get entity Steve Dimension"]
    assert t.consume("[12:00:00 INFO]: Steve has the following entity data: [12.5d, 64.0d, -3.25d]")
    assert t.consume('[12:00:00 INFO]: Steve has the following entity data: "minecraft:the_nether"')
    assert t.snapshot() == [{"name": "Steve", "dimension": "minecraft:the_nether", "x": 12.5, "y": 64.0, "z": -3.25}]
    # not asked for (typed by a user) -> stays visible in the console
    assert not t.consume("[12:00:00 INFO]: Alex has the following entity data: [1.0d, 2.0d, 3.0d]")
    assert not t.consume("[12:00:00 INFO]: <Steve> Steve has the following entity data: haha")
    # players who left disappear
    t.queries(set())
    assert t.snapshot() == []


def test_player_tracking_only_while_watched(monkeypatch):
    t = livemap.PlayerTracker()
    assert not t.watching()
    t.touch()
    assert t.watching()


def test_old_squaremap_plugin_is_removed(data):
    plugins = config.SERVER_DIR / "plugins"
    plugins.mkdir()
    (plugins / "squaremap-paper-mc26.3-1.4.3.jar").write_bytes(b"x")
    (plugins / "other.jar").write_bytes(b"x")
    assert livemap.remove_squaremap()
    assert [p.name for p in plugins.iterdir()] == ["other.jar"]
    assert not livemap.remove_squaremap()


def test_world_args_include_old_paper_dimension_folders(data):
    (config.SERVER_DIR / "world").mkdir()
    (config.SERVER_DIR / "world_nether").mkdir()
    args = livemap._world_args()
    assert args[:2] == ["--world", str(config.SERVER_DIR / "world")]
    assert args[2:] == ["--extra", str(config.SERVER_DIR / "world_nether")]


def test_map_visibility_defaults_validation_and_args(data):
    default = {"cave_reach": livemap.CAVE_REACH_DEFAULT, "all_caves": livemap.ALL_CAVES_DEFAULT}
    assert livemap.load_visibility() == default
    assert livemap.save_visibility({"cave_reach": 16, "all_caves": False}) == {"cave_reach": 16, "all_caves": False}
    assert livemap.load_visibility() == {"cave_reach": 16, "all_caves": False}
    assert livemap._visibility_args() == ["--cave-reach", "16"]
    # keys left out keep their stored value
    assert livemap.save_visibility({"all_caves": True}) == {"cave_reach": 16, "all_caves": True}
    assert livemap._visibility_args() == ["--cave-reach", "16", "--all-caves"]
    for bad in ({"cave_reach": -1}, {"cave_reach": "32"}, {"cave_reach": 1000}, {"cave_reach": 2.5}, {"all_caves": "yes"}):
        with pytest.raises(ValueError):
            livemap.validate_visibility(bad)
    # a broken file falls back to the defaults
    (data / "map_visibility.json").write_text("{nope", encoding="utf-8")
    assert livemap.load_visibility() == default


def test_skin_url_only_from_mojang():
    import base64

    def profile(url):
        value = base64.b64encode(json.dumps({"textures": {"SKIN": {"url": url}}}).encode()).decode()
        return {"properties": [{"name": "textures", "value": value}]}

    assert livemap._skin_url(profile("http://textures.minecraft.net/texture/abc")) == "https://textures.minecraft.net/texture/abc"
    assert livemap._skin_url(profile("https://evil.example/texture/abc")) is None
    assert livemap._skin_url(profile("file:///etc/passwd")) is None
    assert livemap._skin_url({}) is None


def test_skin_paths_and_player_uuids(data):
    with pytest.raises(ValueError):
        livemap.skin_path("../../x")
    (config.SERVER_DIR / "usercache.json").write_text(json.dumps(
        [{"name": "Steve", "uuid": "c2c64826-10bf-428e-9097-606bda04cd1d", "expiresOn": "x"}]), encoding="utf-8")
    skins = livemap.Skins()
    assert skins.uuid_of("steve") == "c2c6482610bf428e9097606bda04cd1d"
    assert skins.uuid_of("Alex") is None
    # the snapshot names the skin once it is downloaded
    t = livemap.PlayerTracker()
    t.positions["Steve"] = {"name": "Steve", "dimension": "minecraft:overworld", "x": 1.0, "y": 2.0, "z": 3.0}
    assert "skin" not in t.snapshot()[0]
    path = livemap.skin_path("c2c6482610bf428e9097606bda04cd1d")
    path.parent.mkdir(parents=True)
    path.write_bytes(b"png")
    assert t.snapshot()[0]["skin"] == "c2c6482610bf428e9097606bda04cd1d"


def _set_versions(world: str, server: str = "") -> None:
    s = config.load_settings()
    s.world_version, s.version = world, server
    config.save_settings(s)


def test_client_jar_uses_cached_download_and_cleans_old(data, monkeypatch):
    monkeypatch.setattr(livemap, "MAP_TEXTURES", True)
    _set_versions("26.3")
    (data / "cache").mkdir()
    (data / "cache" / "client-26.3.jar").write_bytes(b"jar")
    assert livemap.client_jar() == data / "cache" / "client-26.3.jar"


def test_client_jar_off_or_bad_version(data, monkeypatch):
    monkeypatch.setattr(livemap, "MAP_TEXTURES", False)
    _set_versions("26.3")
    assert livemap.client_jar() is None
    monkeypatch.setattr(livemap, "MAP_TEXTURES", True)
    _set_versions("../evil")  # never used in a path
    assert livemap.client_jar() is None


def test_client_jar_failed_download_falls_back(data, monkeypatch):
    monkeypatch.setattr(livemap, "MAP_TEXTURES", True)
    monkeypatch.setattr(livemap, "_client_failed", {})
    _set_versions("", "26.3")  # no world yet: the server version counts

    def offline(url):
        raise OSError("offline")

    monkeypatch.setattr(livemap.versions, "_cached", offline)
    logs = []
    assert livemap.client_jar(logs.append) is None
    assert "map colours" in logs[-1]
    # not retried right away
    logs.clear()
    assert livemap.client_jar(logs.append) is None
    assert logs == []
