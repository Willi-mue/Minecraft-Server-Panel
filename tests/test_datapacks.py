import gzip
import io
import json
import zipfile

import pytest

from app import datapacks, nbt, world


def _level_dat(enabled=("vanilla",), disabled=()):
    root = {"Data": (nbt.COMPOUND, {
        "Version": (nbt.COMPOUND, {"Name": (nbt.STRING, "1.21.4")}),
        "DataPacks": (nbt.COMPOUND, {
            "Enabled": (nbt.LIST, (nbt.STRING, list(enabled))),
            "Disabled": (nbt.LIST, (nbt.STRING if disabled else nbt.END, list(disabled))),
        }),
        "Time": (nbt.LONG, 123456789),
        "Seeds": (nbt.LONGS, [1, -2]),
        "Pos": (nbt.LIST, (nbt.DOUBLE, [0.5, 64.0])),
        "Flags": (nbt.BYTES, b"\x01\x02"),
    })}
    path = world.level_dat()
    path.parent.mkdir(parents=True, exist_ok=True)
    nbt.save(path, "", root)
    return path


def _zip(files: dict) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        for name, text in files.items():
            zf.writestr(name, text)
    return buf.getvalue()


MCMETA = json.dumps({"pack": {"pack_format": 61, "description": [{"text": "Better "}, {"text": "trees"}]}})


def _add_pack(name="trees.zip", files=None):
    folder = datapacks.folder()
    folder.mkdir(parents=True, exist_ok=True)
    (folder / name).write_bytes(_zip(files or {"pack.mcmeta": MCMETA}))


# ---------- nbt ----------


def test_nbt_round_trip_is_byte_identical(data):
    path = _level_dat(disabled=("file/x.zip",))
    raw = gzip.decompress(path.read_bytes())
    name, root = nbt.decode(raw)
    assert nbt.encode(name, root) == raw
    assert root["Data"][1]["Seeds"] == (nbt.LONGS, [1, -2])
    assert world.saved_version() == "1.21.4"  # the existing reader understands what we write


def test_nbt_save_keeps_old_copy(data):
    path = _level_dat()
    before = path.read_bytes()
    nbt.save(path, "", nbt.load(path)[1])
    assert path.with_name("level.dat_old").read_bytes() == before


# ---------- listing ----------


def test_list_before_world_exists(data):
    _add_pack()
    result = datapacks.list_packs()
    assert result["world"] is False
    assert result["packs"][0]["status"] == "new"
    assert result["packs"][0]["description"] == "Better trees"


def test_list_status_from_level_dat(data):
    _add_pack("a.zip")
    _add_pack("b.zip")
    _add_pack("c.zip")
    _add_pack("broken.zip", {"data/x.json": "{}"})
    _level_dat(enabled=("vanilla", "file/a.zip"), disabled=("file/b.zip",))
    packs = {p["name"]: p for p in datapacks.list_packs()["packs"]}
    assert packs["a.zip"]["status"] == "enabled"
    assert packs["b.zip"]["status"] == "disabled"
    assert packs["c.zip"]["status"] == "new"
    assert packs["broken.zip"]["valid"] is False


# ---------- offline toggle ----------


def test_disable_and_enable_offline(data):
    _add_pack()
    _level_dat(enabled=("vanilla", "file/trees.zip"))
    datapacks.set_enabled_offline("trees.zip", False)
    assert datapacks.state() == ({"vanilla"}, {"file/trees.zip"})
    datapacks.set_enabled_offline("trees.zip", True)
    assert datapacks.state() == ({"vanilla", "file/trees.zip"}, set())
    enabled = nbt.load(world.level_dat())[1]["Data"][1]["DataPacks"][1]["Enabled"][1][1]
    assert enabled[-1] == "file/trees.zip"  # highest priority, like /datapack enable


def test_disable_needs_world(data):
    _add_pack()
    with pytest.raises(RuntimeError):
        datapacks.set_enabled_offline("trees.zip", False)


def test_toggle_unknown_pack(data):
    _level_dat()
    with pytest.raises(FileNotFoundError):
        datapacks.set_enabled_offline("missing.zip", True)


@pytest.mark.parametrize("name", ["../x.zip", "a/b.zip", 'q"uote.zip', ".hidden.zip", "a\\b.zip", ""])
def test_invalid_names(name):
    with pytest.raises(ValueError):
        datapacks.validate_name(name)


# ---------- library ----------


def _upload(tmp_path, name="p.zip", files=None):
    src = tmp_path / "upload"
    src.write_bytes(_zip(files or {"pack.mcmeta": MCMETA}))
    return datapacks.add_upload(name, src)


def test_upload_goes_to_library_then_install(data, tmp_path):
    _upload(tmp_path, "My Pack (v2).zip")
    assert (datapacks.library() / "My Pack (v2).zip").exists()
    [p] = datapacks.list_packs()["packs"]
    assert p["in_library"] and not p["installed"] and p["status"] is None
    datapacks.install("My Pack (v2).zip")
    [p] = datapacks.list_packs()["packs"]
    assert p["installed"] and p["status"] == "new" and not p["outdated"]


@pytest.mark.parametrize("files, message", [
    ({"inner/pack.mcmeta": MCMETA}, "one folder too deep"),
    ({"data/x.json": "{}"}, "pack.mcmeta is missing"),
    ({"pack.mcmeta": "{not json"}, "not valid JSON"),
])
def test_upload_rejects(data, tmp_path, files, message):
    with pytest.raises(ValueError, match=message):
        _upload(tmp_path, files=files)


def test_upload_rejects_non_zip(data, tmp_path):
    src = tmp_path / "upload"
    src.write_bytes(b"not a zip")
    with pytest.raises(ValueError):
        datapacks.add_upload("p.zip", src)
    with pytest.raises(ValueError):
        datapacks.add_upload("p.txt", src)


def test_new_version_marks_world_copy_outdated(data, tmp_path):
    _upload(tmp_path)
    datapacks.install("p.zip")
    _upload(tmp_path, files={"pack.mcmeta": MCMETA, "data/new.json": "{}"})
    assert datapacks.list_packs()["packs"][0]["outdated"] is True
    datapacks.install("p.zip")  # update the world copy
    assert datapacks.list_packs()["packs"][0]["outdated"] is False


def test_adopt_world_only_pack(data):
    _add_pack("old.zip")  # e.g. from a restored backup
    [p] = datapacks.list_packs()["packs"]
    assert not p["in_library"] and p["installed"]
    datapacks.adopt("old.zip")
    assert datapacks.list_packs()["packs"][0]["in_library"] is True


def test_folder_pack_round_trip(data):
    pack = datapacks.folder() / "folderpack"
    pack.mkdir(parents=True)
    (pack / "pack.mcmeta").write_text(MCMETA)
    datapacks.adopt("folderpack")
    assert (datapacks.library() / "folderpack" / "pack.mcmeta").exists()
    assert datapacks.list_packs()["packs"][0]["outdated"] is False


def test_uninstall_and_delete(data, tmp_path):
    _upload(tmp_path)
    datapacks.install("p.zip")
    datapacks.uninstall("p.zip")
    assert datapacks.list_packs()["packs"][0]["installed"] is False
    datapacks.delete_from_library("p.zip")
    assert datapacks.list_packs()["packs"] == []
    with pytest.raises(FileNotFoundError):
        datapacks.install("p.zip")


def test_new_world_keeps_selected_packs(data, tmp_path):
    from app import backup
    _upload(tmp_path, "lib.zip")
    _add_pack("worldonly.zip")
    _add_pack("unselected.zip")
    names = datapacks.prepare_new_world(["lib.zip", "worldonly.zip", "missing.zip"])
    assert names == ["lib.zip", "worldonly.zip"]
    backup.remove_world()
    datapacks.install_many(names)
    installed = {p["name"] for p in datapacks.list_packs()["packs"] if p["installed"]}
    assert installed == {"lib.zip", "worldonly.zip"}


# ---------- experimental feature packs ----------


def test_experiments_without_world():
    assert datapacks.experiments(None) == {"enabled": [], "available": datapacks.DEFAULT_EXPERIMENTS}


def test_experiments_from_level_dat(data):
    _level_dat(enabled=("vanilla", "paper", "trade_rebalance", "file/x.zip"),
               disabled=("minecart_improvements", "file/y.zip"))
    exp = datapacks.list_packs()["experiments"]
    assert exp == {"enabled": ["trade_rebalance"], "available": ["minecart_improvements", "trade_rebalance"]}


def test_initial_packs_value():
    assert datapacks.initial_packs_value([]) == "vanilla"
    assert datapacks.initial_packs_value(["trade_rebalance", "trade_rebalance"]) == "vanilla,trade_rebalance"
    for bad in ["file/x.zip", "a,b", "vanilla", "Trade"]:
        with pytest.raises(ValueError):
            datapacks.initial_packs_value([bad])


def test_new_world_sets_initial_packs(data):
    from app import access
    from app.server import ServerManager
    ServerManager()._new_world("", [], datapacks.initial_packs_value(["trade_rebalance"]))
    assert access.get_property("initial-enabled-packs") == "vanilla,trade_rebalance"
    ServerManager()._new_world("")
    assert access.get_property("initial-enabled-packs") == "vanilla"
