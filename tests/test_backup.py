import os
import time
import zipfile

import pytest

from app import backup, config


def make_world(data, content=b"x" * 100):
    region = config.SERVER_DIR / "world" / "region"
    region.mkdir(parents=True, exist_ok=True)
    (region / "r.0.0.mca").write_bytes(content)
    (config.SERVER_DIR / "world" / "level.dat").write_bytes(b"level")
    (config.SERVER_DIR / "world" / "session.lock").write_bytes(b"lock")


def age(path, seconds):
    t = time.time() - seconds
    os.utime(path, (t, t))


def test_backup_skips_lock_and_stores_region_files_uncompressed(data):
    make_world(data)
    path = backup.create_backup()
    with zipfile.ZipFile(path) as zf:
        assert sorted(zf.namelist()) == ["world/level.dat", "world/region/r.0.0.mca"]
        assert zf.getinfo("world/region/r.0.0.mca").compress_type == zipfile.ZIP_STORED


def test_no_world_raises(data):
    with pytest.raises(FileNotFoundError):
        backup.create_backup()


def test_retention_keeps_newest(data, monkeypatch):
    monkeypatch.setattr(config, "BACKUP_KEEP", 2)
    make_world(data)
    for i in range(3):
        p = config.BACKUP_DIR / f"backup_old{i}.zip"
        p.write_bytes(b"")
        age(p, 100 - i)
    newest = backup.create_backup()
    assert sorted(p.name for p in config.BACKUP_DIR.glob("*.zip")) == sorted(["backup_old2.zip", newest.name])


def test_cleanup_never_deletes_protected_backup(data, monkeypatch):
    monkeypatch.setattr(config, "BACKUP_KEEP", 2)
    oldest = config.BACKUP_DIR / "backup_oldest.zip"
    oldest.write_bytes(b"")
    age(oldest, 1000)
    for i in range(3):
        (config.BACKUP_DIR / f"backup_n{i}.zip").write_bytes(b"")
    backup.cleanup(protect=oldest)
    assert oldest.exists()
    assert len(list(config.BACKUP_DIR.glob("*.zip"))) == 2


def test_restore_replaces_world_and_saves_current_first(data):
    make_world(data, b"old")
    saved = backup.create_backup()
    make_world(data, b"new")
    safety = backup.restore(saved.name)
    assert (config.SERVER_DIR / "world/region/r.0.0.mca").read_bytes() == b"old"
    assert safety is not None and safety.name.endswith("_before-restore.zip")
    with zipfile.ZipFile(safety) as zf:
        assert zf.read("world/region/r.0.0.mca") == b"new"


@pytest.mark.parametrize("member", ["../evil.txt", "/abs/x", "plugins/x.jar", "toplevel.txt"])
def test_restore_rejects_unsafe_archives(data, member):
    bad = config.BACKUP_DIR / "backup_bad.zip"
    with zipfile.ZipFile(bad, "w") as zf:
        zf.writestr(member, "x")
    with pytest.raises(ValueError):
        backup.restore(bad.name)


def test_archives_are_never_cleaned_up(data, monkeypatch):
    monkeypatch.setattr(config, "BACKUP_KEEP", 1)
    make_world(data)
    archive = backup.create_archive("1.21.4")
    age(archive, 10_000)  # older than everything else
    for _ in range(3):
        backup.create_backup()
        time.sleep(1.05)
    assert archive.exists()
    names = [b["name"] for b in backup.list_backups()]
    assert archive.name in names and len(names) == 2
    assert next(b for b in backup.list_backups() if b["archive"])["note"] == "archive · kept forever"


def test_new_world_archives_and_removes_world(data):
    from app import access, livemap
    from app.server import ServerManager
    make_world(data, b"old world")
    (config.SERVER_DIR / "world_nether").mkdir()
    (livemap.out_dir() / "overworld" / "0").mkdir(parents=True)
    m = ServerManager()
    m._new_world("12345")
    assert backup.world_dirs() == []
    assert not livemap.out_dir().exists()
    assert access.get_property("level-seed") == "12345"
    (archive,) = config.BACKUP_DIR.glob("archive_*.zip")
    with zipfile.ZipFile(archive) as zf:
        assert zf.read("world/region/r.0.0.mca") == b"old world"
    # an archive can be restored like a backup
    backup.restore(archive.name)
    assert (config.SERVER_DIR / "world/region/r.0.0.mca").read_bytes() == b"old world"


def test_backup_file_rejects_traversal(data):
    with pytest.raises(FileNotFoundError):
        backup.backup_file("../settings.json")
