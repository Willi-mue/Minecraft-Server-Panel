import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app import config  # noqa: E402


@pytest.fixture
def data(tmp_path, monkeypatch):
    """Point all data paths at a fresh temporary directory."""
    monkeypatch.setattr(config, "DATA_DIR", tmp_path)
    monkeypatch.setattr(config, "SERVER_DIR", tmp_path / "server")
    monkeypatch.setattr(config, "JARS_DIR", tmp_path / "jars")
    monkeypatch.setattr(config, "BACKUP_DIR", tmp_path / "backups")
    monkeypatch.setattr(config, "SETTINGS_FILE", tmp_path / "settings.json")
    config.ensure_dirs()
    return tmp_path
