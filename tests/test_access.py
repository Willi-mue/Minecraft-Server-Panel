import json

import pytest

from app import access, config
from app.instances import MAIN


@pytest.fixture
def mojang(monkeypatch):
    monkeypatch.setattr(access, "resolve", lambda inst, name: {"uuid": "u-" + name.lower(), "name": name.capitalize()})


def test_ban_and_unban_offline(data, mojang):
    msg = access.offline_change(MAIN, "bans", "add", "steve", "griefing")
    assert msg == "Banned Steve"
    (entry,) = json.loads((config.SERVER_DIR / "banned-players.json").read_text())
    assert entry["name"] == "Steve" and entry["reason"] == "griefing" and entry["expires"] == "forever"
    assert entry["source"] == "Server panel" and len(entry["created"]) == len("2026-11-10 12:00:00 +0100")
    assert access.state(MAIN)["bans"][0]["reason"] == "griefing"
    with pytest.raises(LookupError):
        access.offline_change(MAIN, "bans", "add", "Steve")
    assert access.offline_change(MAIN, "bans", "remove", "STEVE") == "Unbanned STEVE"
    assert access.state(MAIN)["bans"] == []
    with pytest.raises(LookupError):
        access.offline_change(MAIN, "bans", "remove", "Steve")


def test_default_ban_reason(data, mojang):
    access.offline_change(MAIN, "bans", "add", "alex")
    assert access.state(MAIN)["bans"][0]["reason"] == "Banned by an operator."


@pytest.mark.parametrize("reason", ["a\nop Steve", "x" * 201, "a\rb"])
def test_reason_is_single_line(reason):
    with pytest.raises(ValueError):
        access.validate_reason(reason)
