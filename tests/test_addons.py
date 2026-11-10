import hashlib
import io
import json
import zipfile

import pytest

from app import addons, config
from app.instances import MAIN


def jar(files: dict) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        for name, content in files.items():
            zf.writestr(name, content if isinstance(content, (bytes, str)) else json.dumps(content))
    return buf.getvalue()


def mod(mod_id, depends=None, environment="*", jars=None, nested=None, **extra):
    meta = {"schemaVersion": 1, "id": mod_id, "version": "1.0", "name": mod_id.title(),
            "environment": environment, "depends": depends or {}, **extra}
    files = {"fabric.mod.json": meta}
    if nested:
        meta["jars"] = [{"file": f"META-INF/jars/{n}.jar"} for n in nested]
        for n in nested:
            files[f"META-INF/jars/{n}.jar"] = jar({"fabric.mod.json": {"schemaVersion": 1, "id": n, "version": "1"}})
    return jar(files)


def put(kind, name, content):
    d = config.SERVER_DIR / kind
    d.mkdir(parents=True, exist_ok=True)
    (d / name).write_bytes(content)


# ---------- metadata ----------


def test_plugin_yml_subset():
    y = addons.parse_plugin_yml(
        "name: Essentials\nversion: '2.20.1'\napi-version: \"1.20\"\nauthors: [a, b]\n"
        "depend: [Vault]\nsoftdepend:\n  - LuckPerms\ndescription: >\n  Line one\n  line two\n"
        "dependencies:\n  server:\n    Vault:\n      required: true\n    Opt:\n      required: false\n")
    assert y["name"] == "Essentials" and y["version"] == "2.20.1" and y["api-version"] == "1.20"
    assert y["authors"] == ["a", "b"] and y["depend"] == ["Vault"] and y["softdepend"] == ["LuckPerms"]
    assert y["description"] == "Line one line two"
    assert y["dependencies"]["server"]["Opt"] == {"required": "false"}


@pytest.mark.parametrize("version,spec,expected", [
    ("1.21.4", ">=1.21", True), ("1.21.4", "~1.21.4", True), ("1.21.4", "1.21.x", True),
    ("1.20.1", ">=1.21", False), ("1.21.4", [">=1.20 <1.21", "1.21.4"], True), ("1.21.4", "~1.20", False),
    ("1.21.4", ">=1.21- <1.22-", True), ("25w14a", ">=1.21", None), ("1.21.4", "*", True),
])
def test_fabric_version_ranges(version, spec, expected):
    assert addons.matches(version, spec) is expected


def test_list_flags_missing_dependency_wrong_version_and_side(data):
    put("mods", "fabric-api.jar", mod("fabric-api", nested=["fabric-api-base", "fabric-networking-api-v1"]))
    put("mods", "sodium.jar", mod("sodium", {"minecraft": "~1.20.1", "fabric-api-base": "*"}, environment="client"))
    put("mods", "lithium.jar", mod("lithium", {"minecraft": ">=1.21", "fabricloader": ">=0.16", "cloth-config": "*"}))
    put("mods", "old.jar.disabled", mod("old", {"minecraft": "1.21.4"}, environment="server"))
    items = {i["id"]: i for i in addons.list_addons(MAIN, "mods", "1.21.4")}
    assert items["sodium"]["missing"] == [] and items["sodium"]["incompatible"] == "Made for Minecraft ~1.20.1"
    assert items["sodium"]["side"] == "client"
    assert items["lithium"]["missing"] == ["cloth-config"] and items["lithium"]["incompatible"] is None
    assert items["lithium"]["side"] == "both"
    assert items["old"]["enabled"] is False and items["old"]["side"] == "server"


def test_disabled_mod_does_not_satisfy_dependencies(data):
    put("mods", "a.jar", mod("a", {"b": "*"}))
    put("mods", "b.jar.disabled", mod("b"))
    (item,) = [i for i in addons.list_addons(MAIN, "mods", "1.21.4") if i["id"] == "a"]
    assert item["missing"] == ["b"]


def test_plugins(data):
    put("plugins", "a.jar", jar({"plugin.yml": "name: A\nversion: 1\napi-version: '1.21'\ndepend: [Vault]\n"}))
    put("plugins", "vault.jar", jar({"plugin.yml": "name: Vault\nversion: 1.7\n"}))
    put("plugins", "new.jar", jar({"paper-plugin.yml": "name: New\nversion: 2\napi-version: '26.4'\n"
                                                       "dependencies:\n  server:\n    Missing:\n      required: true\n"}))
    put("plugins", "amod.jar", mod("amod"))
    items = {i["file"]: i for i in addons.list_addons(MAIN, "plugins", "1.21.4")}
    assert items["a.jar"]["missing"] == [] and items["a.jar"]["incompatible"] is None
    assert items["new.jar"]["missing"] == ["Missing"] and items["new.jar"]["incompatible"] == "Needs Paper 26.4 or newer"
    assert items["amod.jar"]["problems"] == ["This is a Fabric mod, not a plugin"]


# ---------- local changes ----------


@pytest.mark.parametrize("name", ["../x.jar", "x.zip", ".hidden.jar", "a/b.jar", "x.jar.part"])
def test_invalid_file_names(name):
    with pytest.raises(ValueError):
        addons.validate_file(name)


def test_toggle_and_delete(data):
    put("mods", "a.jar", mod("a"))
    assert addons.set_enabled(MAIN, "mods", "a.jar", False) == "a.jar.disabled"
    assert (config.SERVER_DIR / "mods" / "a.jar.disabled").exists()
    assert addons.set_enabled(MAIN, "mods", "a.jar.disabled", True) == "a.jar"
    addons.delete(MAIN, "mods", "a.jar")
    with pytest.raises(FileNotFoundError):
        addons.delete(MAIN, "mods", "a.jar")


def test_upload_checks_the_jar(data, tmp_path):
    src = tmp_path / "up"
    src.write_bytes(b"not a zip")
    with pytest.raises(ValueError, match="valid"):
        addons.add_upload(MAIN, "mods", "x.jar", src)
    src.write_bytes(jar({"plugin.yml": "name: P\n"}))
    with pytest.raises(ValueError, match="not a Fabric mod"):
        addons.add_upload(MAIN, "mods", "x.jar", src)
    src.write_bytes(mod("x"))
    addons.add_upload(MAIN, "mods", "x.jar", src)
    assert (config.SERVER_DIR / "mods" / "x.jar").exists()


# ---------- Modrinth (no network) ----------


def version(pid, filename, content, deps=()):
    return {"id": f"v-{pid}", "version_number": "1.0", "version_type": "release",
            "files": [{"primary": True, "filename": filename, "url": f"https://cdn.modrinth.com/data/{pid}/{filename}",
                       "hashes": {"sha512": hashlib.sha512(content).hexdigest()}}],
            "dependencies": [{"project_id": d, "dependency_type": "required"} for d in deps]}


def test_install_with_required_dependency(data, monkeypatch):
    lith, api = mod("lithium", {"fabric-api": "*"}), mod("fabric-api")
    calls = []

    def fake_api(path, body=None):
        calls.append(path)
        if path.startswith("/project/LITH/version"):
            return [version("LITH", "lithium.jar", lith, deps=["FAPI"])]
        if path.startswith("/project/FAPI/version"):
            return [version("FAPI", "fabric-api.jar", api)]
        if path == "/project/FAPI":
            return {"slug": "fabric-api"}
        raise AssertionError(path)

    contents = {"lithium.jar": lith, "fabric-api.jar": api}

    def fake_download(file, target_dir, disabled=False):
        target_dir.mkdir(parents=True, exist_ok=True)
        (target_dir / file["filename"]).write_bytes(contents[file["filename"]])
        return file["filename"]

    monkeypatch.setattr(addons, "_api", fake_api)
    monkeypatch.setattr(addons, "_download", fake_download)
    assert addons.install(MAIN, "mods", "1.21.4", "LITH") == ["lithium.jar", "fabric-api.jar"]
    # Second time: same files are there already
    assert addons.install(MAIN, "mods", "1.21.4", "LITH") == []
    assert "loaders=" in calls[0] and "game_versions=" in calls[0]


def test_download_checks_host_and_hash(data, monkeypatch):
    content = b"jar bytes"
    good = version("X", "x.jar", content)["files"][0]
    with pytest.raises(ValueError, match="host"):
        addons._download({**good, "url": "https://evil.example/x.jar"}, config.SERVER_DIR / "mods")

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

    monkeypatch.setattr(addons.urllib.request, "urlopen", lambda req, timeout: Resp(b"tampered"))
    with pytest.raises(ValueError, match="Checksum"):
        addons._download(good, config.SERVER_DIR / "mods")
    assert not list((config.SERVER_DIR / "mods").iterdir())
    monkeypatch.setattr(addons.urllib.request, "urlopen", lambda req, timeout: Resp(content))
    assert addons._download(good, config.SERVER_DIR / "mods", disabled=True) == "x.jar.disabled"


def test_updates_by_hash(data, monkeypatch):
    old, new = mod("a", version="1"), mod("a", version="2")
    put("mods", "a-1.jar", old)
    h = hashlib.sha512(old).hexdigest()
    monkeypatch.setattr(addons, "_api", lambda path, body=None: {h: version("A", "a-2.jar", new)})
    assert addons.check_updates(MAIN, "mods", "1.21.4") == {"a-1.jar": {"version": "1.0", "filename": "a-2.jar"}}

    def fake_download(file, target_dir, disabled=False):
        (target_dir / file["filename"]).write_bytes(new)
        return file["filename"]

    monkeypatch.setattr(addons, "_download", fake_download)
    assert addons.update(MAIN, "mods", "1.21.4", "a-1.jar") == "a-2.jar"
    assert sorted(p.name for p in (config.SERVER_DIR / "mods").iterdir()) == ["a-2.jar"]
