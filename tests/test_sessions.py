import json
import time

import pytest

from app import config, sessions


@pytest.fixture
def store(data, monkeypatch):
    monkeypatch.setattr(config, "PASSWORD", "secret")
    sessions.reset_cache()
    yield
    sessions.reset_cache()


def test_session_survives_a_restart(store):
    token = sessions.create()
    assert sessions.check(token) == "ok"
    sessions.reset_cache()  # = panel restart
    assert sessions.check(token) == "ok"
    assert sessions.check("other") == ""
    assert sessions.check(None) == ""


def test_only_hashes_are_stored(store):
    token = sessions.create()
    text = (config.DATA_DIR / "sessions.json").read_text()
    assert token not in text


def test_logout_removes_the_session(store):
    token = sessions.create()
    sessions.remove(token)
    sessions.reset_cache()
    assert sessions.check(token) == ""


def test_new_password_logs_everybody_out(store, monkeypatch):
    token = sessions.create()
    monkeypatch.setattr(config, "PASSWORD", "changed")
    sessions.reset_cache()
    assert sessions.check(token) == ""


def test_expired_and_renewed(store):
    token = sessions.create()
    path = config.DATA_DIR / "sessions.json"
    data = json.loads(path.read_text())
    (h,) = data["sessions"]
    data["sessions"][h] = time.time() + 60  # almost over: gets renewed on use
    path.write_text(json.dumps(data))
    sessions.reset_cache()
    assert sessions.check(token) == "renewed"
    assert sessions.check(token) == "ok"
    data["sessions"][h] = time.time() - 1
    path.write_text(json.dumps(data))
    sessions.reset_cache()
    assert sessions.check(token) == ""


def test_broken_file_is_ignored(store):
    (config.DATA_DIR / "sessions.json").write_text("{broken")
    sessions.reset_cache()
    assert sessions.check("x") == ""
    assert sessions.check(sessions.create()) == "ok"
