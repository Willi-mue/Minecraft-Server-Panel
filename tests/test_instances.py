import pytest

from app import access, backup, config, instances
from app.instances import MAIN
from app.server import ServerManager, get_manager, managers


def test_main_keeps_the_old_paths(data):
    assert MAIN.server_dir == config.SERVER_DIR
    assert MAIN.backup_dir == config.BACKUP_DIR
    assert MAIN.settings_file == config.SETTINGS_FILE
    assert MAIN.map_dir == data / "map"
    assert instances.all_entries() == [{"id": "main", "name": "Server"}]


def test_create_rename_remove(data):
    a = instances.create("Modded Fabric!")
    b = instances.create("Modded Fabric!")
    assert (a.id, b.id) == ("modded-fabric", "modded-fabric-2")
    assert a.server_dir == data / "servers" / "modded-fabric" / "server"
    assert a.backup_dir == config.BACKUP_DIR / "modded-fabric"
    assert a.server_dir.is_dir() and a.backup_dir.is_dir()
    instances.rename(a, "Mods")
    assert instances.name_of(a) == "Mods"
    assert instances.get("modded-fabric") == a

    (a.backup_dir / "backup_x.zip").write_bytes(b"")
    instances.remove(a)
    assert not a.root.exists()
    assert (a.backup_dir / "backup_x.zip").exists()  # backups are kept
    with pytest.raises(KeyError):
        instances.get("modded-fabric")
    with pytest.raises(ValueError):
        instances.remove(MAIN)


def test_ids_and_names_are_validated(data):
    for bad in ("", "Main", "../x", "a" * 33, "-x"):
        with pytest.raises(ValueError):
            instances.validate_id(bad)
    for bad in ("", " ", "a\nb", "x" * 41):
        with pytest.raises(ValueError):
            instances.validate_name(bad)
    assert instances.create("!!!").id == "server"
    assert instances.create("main").id == "main-2"  # never collides with the first server


def test_registry_ignores_broken_entries(data):
    (data / "servers.json").write_text('[{"id": "../evil", "name": "x"}, {"id": "ok", "name": "Ok"}, 5]')
    assert [e["id"] for e in instances.all_entries()] == ["main", "ok"]


def test_free_port_skips_used_ports(data, monkeypatch):
    monkeypatch.setattr(config, "SERVER_PORTS", [25565, 25566, 25567])
    assert instances.free_port() == 25566  # main uses Minecraft's default 25565
    a = instances.create("A")
    access.set_property(a, "server-port", "25566")
    assert instances.game_port(a) == 25566
    assert instances.free_port() == 25567
    b = instances.create("B")
    access.set_property(b, "server-port", "25567")
    with pytest.raises(RuntimeError):
        instances.free_port()


def test_settings_and_backups_are_separate(data):
    a = instances.create("Second")
    s = config.load_settings(a)
    s.version, s.flavor = "1.21.1", "vanilla"
    config.save_settings(s, a)
    assert config.load_settings(a).version == "1.21.1"
    assert config.load_settings(MAIN).version == config.Settings().version

    for inst, payload in ((MAIN, b"main world"), (a, b"second world")):
        (inst.server_dir / "world").mkdir(parents=True)
        (inst.server_dir / "world" / "level.dat").write_bytes(payload)
        backup.create_backup(inst)
    [_] = backup.list_backups(MAIN)
    [second_backup] = backup.list_backups(a)
    # A backup of one server can't be reached through another
    with pytest.raises(FileNotFoundError):
        backup.backup_file(MAIN, f"{a.id}/{second_backup['name']}")


def test_manager_per_instance_and_port_conflict(data, monkeypatch):
    managers.clear()
    a = instances.create("Other")
    m_main, m_a = get_manager("main"), get_manager(a.id)
    assert m_main.inst == MAIN and m_a.inst == a and m_main is not m_a
    assert m_main.map_path == "/s/main/" and m_a.map_path == f"/s/{a.id}/"
    with pytest.raises(KeyError):
        get_manager("nope")

    events = []
    monkeypatch.setattr("app.server.hub.emit", events.append)
    m_a.info("hello")
    assert events[-1] == {"type": "log", "line": "[Panel] hello", "server": a.id}

    # Same port as a running server: refuse to start
    class Alive:
        pid = 1

        def poll(self):
            return None

    m_main.process = Alive()
    with pytest.raises(RuntimeError, match="Port 25565"):
        m_a._check_port()
    access.set_property(a, "server-port", "25566")
    m_a._check_port()
    m_main.process = None
    managers.clear()


def test_eula_is_per_server(data):
    a = instances.create("Other")
    ServerManager(a).accept_eula()
    assert ServerManager(a)._eula_accepted()
    assert not ServerManager(MAIN)._eula_accepted()
