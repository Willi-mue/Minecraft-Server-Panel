"""Live map web server: one map per server (main at /, others at /s/<id>/)."""

import json

import pytest

from app import config, instances, livemap, mapserver

pytest.importorskip("httpx")
from starlette.testclient import TestClient  # noqa: E402


@pytest.fixture
def client(data, monkeypatch):
    monkeypatch.setattr(mapserver, "_enabled_cache", {})
    return TestClient(mapserver.app)


def _render(inst, marker: str) -> None:
    out = livemap.out_dir(inst)
    (out / "overworld" / "0").mkdir(parents=True)
    (out / "overworld" / "0" / "0_0.png").write_bytes(marker.encode())
    (out / "maps.json").write_text(json.dumps({"dimensions": [], "who": marker}), encoding="utf-8")


def test_each_server_has_its_own_map(client):
    other = instances.create("Other world")
    _render(instances.MAIN, "main")
    _render(other, "other")
    assert client.get("/maps.json").json()["who"] == "main"
    assert client.get(f"/s/{other.id}/maps.json").json()["who"] == "other"
    assert client.get("/s/main/maps.json").json()["who"] == "main"  # main also has its own path
    assert client.get("/tiles/overworld/0/0_0.png").content == b"main"
    assert client.get(f"/s/{other.id}/tiles/overworld/0/0_0.png").content == b"other"
    # missing tiles: empty image, no 404
    assert client.get(f"/s/{other.id}/tiles/overworld/0/5_5.png").headers["content-type"] == "image/png"
    # the viewer is served under the prefix too (relative URLs), with a redirect to the slash
    assert "Live Map" in client.get(f"/s/{other.id}/").text
    assert client.get(f"/s/{other.id}", follow_redirects=False).status_code in (301, 302, 307)
    assert client.get(f"/s/{other.id}/map.js").status_code == 200


def test_unknown_or_bad_server_ids_are_404(client):
    assert client.get("/s/nope/maps.json").status_code == 404
    assert client.get("/s/..%2F..%2Fetc/maps.json").status_code == 404
    assert client.get("/s/nope/").status_code == 404


def test_server_list_only_has_enabled_rendered_maps(client):
    other = instances.create("Other")
    third = instances.create("Third")
    _render(instances.MAIN, "main")
    _render(other, "other")
    _render(third, "third")
    s = config.load_settings(third)
    s.map_enabled = False
    config.save_settings(s, third)
    servers = client.get("/servers.json").json()["servers"]
    assert servers == [{"id": "main", "name": "Server", "path": "s/main/"},
                       {"id": other.id, "name": "Other", "path": f"s/{other.id}/"}]
    # turned off: its files are not served
    assert client.get(f"/s/{third.id}/maps.json").status_code == 503


def test_player_tracking_per_server(client):
    other = instances.create("Other")
    livemap.tracker_for(other).positions["Alex"] = {"name": "Alex", "dimension": "minecraft:overworld",
                                                    "x": 1.0, "y": 2.0, "z": 3.0}
    assert client.get("/players.json").json()["players"] == []
    assert [p["name"] for p in client.get(f"/s/{other.id}/players.json").json()["players"]] == ["Alex"]
    assert livemap.tracker_for(other).watching()
    livemap.tracker_for(other).clear()
