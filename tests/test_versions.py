import json

import pytest

from app import versions


def _fake_api(monkeypatch, build_id):
    def get_json(url):
        if url.endswith("/builds?channel=STABLE"):
            return [{"id": build_id, "channel": "STABLE", "downloads": {"server:default": {
                "url": "https://example.invalid/paper.jar", "size": 1 << 20,
                "checksums": {"sha256": "abc"}}}}]
        return {"version": {"java": {"version": {"minimum": 21}, "flags": {"recommended": ["-XX:+X"]}}}}

    fetched = []

    def fetch(url, target, algo, expected):
        target.write_bytes(f"build {build_id}".encode())
        fetched.append(url)

    monkeypatch.setattr(versions, "_get_json", get_json)
    monkeypatch.setattr(versions, "_fetch", fetch)
    return fetched


def _install(build):
    versions.jar_path("paper", "1.21.4").write_bytes(b"old")
    versions.meta_path("paper", "1.21.4").write_text(json.dumps({"java": 21, "build": build}))


def test_update_paper_replaces_older_build(data, monkeypatch):
    _install(100)
    fetched = _fake_api(monkeypatch, 120)
    logs = []
    assert versions.update_paper("1.21.4", logs.append) is True
    assert fetched
    assert versions.jar_path("paper", "1.21.4").read_bytes() == b"build 120"
    assert versions.meta("paper", "1.21.4")["build"] == 120
    assert any("updated to build 120" in line for line in logs)


def test_update_paper_keeps_current_build(data, monkeypatch):
    _install(120)
    fetched = _fake_api(monkeypatch, 120)
    assert versions.update_paper("1.21.4", lambda _: None) is False
    assert not fetched
    assert versions.jar_path("paper", "1.21.4").read_bytes() == b"old"


def test_update_paper_requires_download(data, monkeypatch):
    _fake_api(monkeypatch, 120)
    with pytest.raises(FileNotFoundError):
        versions.update_paper("1.21.4", lambda _: None)
