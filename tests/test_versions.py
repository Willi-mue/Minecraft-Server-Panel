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


# ---------- Fabric ----------


def _jar_bytes():
    import io
    import zipfile
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        zf.writestr("META-INF/MANIFEST.MF", "Main-Class: net.fabricmc.installer.ServerLauncher\n")
    return buf.getvalue()


def _fabric_api(monkeypatch, loaders):
    def get_json(url):
        if url.endswith("/versions/game"):
            return [{"version": "1.21.5-rc1", "stable": False}, {"version": "1.21.4", "stable": True}]
        if "/versions/loader/" in url:
            return [{"loader": {"version": v, "stable": stable}} for v, stable in loaders]
        if url.endswith("/versions/installer"):
            return [{"version": "1.1.0", "stable": True}]
        if url == versions.MOJANG_MANIFEST:
            return {"versions": [{"id": "1.21.4", "url": "https://example.invalid/1.21.4.json"}]}
        if url.endswith("1.21.4.json"):
            return {"javaVersion": {"majorVersion": 21}}
        raise AssertionError(url)

    fetched = []

    def fetch_zip(url, target):
        fetched.append(url)
        target.write_bytes(_jar_bytes())

    versions._cache.clear()
    monkeypatch.setattr(versions, "_get_json", get_json)
    monkeypatch.setattr(versions, "_fetch_zip", fetch_zip)
    return fetched


def test_fabric_version_list(data, monkeypatch):
    _fabric_api(monkeypatch, [])
    assert [v["id"] for v in versions.list_versions("fabric")["versions"]] == ["1.21.4"]
    assert versions.list_versions("fabric", True)["latest"] == "1.21.4"


def test_fabric_download_uses_newest_stable_loader(data, monkeypatch):
    fetched = _fabric_api(monkeypatch, [("0.17.0-beta", False), ("0.16.10", True)])
    versions.download("fabric", "1.21.4", lambda _: None)
    assert fetched[0].endswith("/versions/loader/1.21.4/0.16.10/1.1.0/server/jar")
    assert versions.meta("fabric", "1.21.4") == {"java": 21, "build": "0.16.10", "installer": "1.1.0"}
    assert [(v["flavor"], v["build"]) for v in versions.installed()] == [("fabric", "0.16.10")]


def test_fabric_loader_update(data, monkeypatch):
    _fabric_api(monkeypatch, [("0.16.10", True)])
    versions.download("fabric", "1.21.4", lambda _: None)
    assert versions.update("fabric", "1.21.4", lambda _: None) is False
    _fabric_api(monkeypatch, [("0.16.11", True)])
    assert versions.update("fabric", "1.21.4", lambda _: None) is True
    assert versions.meta("fabric", "1.21.4")["build"] == "0.16.11"


def test_vanilla_has_no_update(data):
    with pytest.raises(ValueError):
        versions.update("vanilla", "1.21.4")


def test_fabric_jar_must_be_a_jar(data, tmp_path, monkeypatch):
    class Resp:
        def __init__(self, body):
            self.body = body

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

        def read(self, n):
            chunk, self.body = self.body[:n], self.body[n:]
            return chunk

    monkeypatch.setattr(versions.urllib.request, "urlopen", lambda req, timeout: Resp(b"<html>error</html>"))
    target = tmp_path / "x.jar"
    with pytest.raises(ValueError):
        versions._fetch_zip("https://meta.fabricmc.net/x", target)
    assert not target.exists() and not (tmp_path / "x.jar.part").exists()
    monkeypatch.setattr(versions.urllib.request, "urlopen", lambda req, timeout: Resp(_jar_bytes()))
    versions._fetch_zip("https://meta.fabricmc.net/x", target)
    assert target.exists()
