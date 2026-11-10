"""Mods (Fabric) and plugins (Paper) of one server: `<server>/mods/` resp. `<server>/plugins/`.

- Metadata comes from the jar itself: `fabric.mod.json` (incl. jar-in-jar modules, e.g. Fabric API)
  resp. `plugin.yml` / `paper-plugin.yml`. Missing dependencies and a wrong Minecraft version are
  flagged.
- Disabling renames `x.jar` to `x.jar.disabled` (neither loader picks that up).
- Search, install (with required dependencies) and updates go through the Modrinth API; every
  download is checked against Modrinth's SHA-512 and must come from Modrinth's CDN.
Changes on disk apply on the next server start.
"""

import hashlib
import io
import json
import re
import threading
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

from .versions import USER_AGENT

KINDS = {"fabric": "mods", "paper": "plugins"}
LOADERS = {"mods": ["fabric"], "plugins": ["paper", "spigot", "bukkit"]}
MODRINTH = "https://api.modrinth.com/v2"
MAX_UPLOAD = 100 * 1024 * 1024
DISABLED = ".disabled"
_FILE_RE = re.compile(r"^[\w\-. +()\[\]!,'&]{1,120}\.jar$")
_PROJECT_RE = re.compile(r"^[A-Za-z0-9_\-]{1,64}$")
_CDN_HOSTS = {"cdn.modrinth.com", "cdn-raw.modrinth.com"}
# Provided by the Fabric loader itself
_FABRIC_BUILTIN = {"minecraft", "java", "fabricloader", "fabric-loader", "mixinextras"}

_meta_cache: dict[tuple[str, int, int], dict | None] = {}
_meta_lock = threading.Lock()


def kind_of(flavor: str) -> str | None:
    """'mods' (Fabric), 'plugins' (Paper) or None (Vanilla has neither)."""
    return KINDS.get(flavor)


def folder(inst, kind: str) -> Path:
    if kind not in LOADERS:
        raise ValueError(f"Unknown add-on kind: {kind}")
    return inst.server_dir / kind


def validate_file(name: str) -> str:
    """'x.jar' or 'x.jar.disabled' (no paths, no hidden files)."""
    base = name[: -len(DISABLED)] if name.endswith(DISABLED) else name
    if not _FILE_RE.match(base) or base.startswith("."):
        raise ValueError("Invalid file name (expected a .jar)")
    return name


def path_of(inst, kind: str, name: str) -> Path:
    path = folder(inst, kind) / validate_file(name)
    if not path.is_file():
        raise FileNotFoundError(f"{name} is not installed")
    return path


# ---------- metadata ----------


def _yaml_scalar(text: str):
    text = text.strip()
    if len(text) >= 2 and text[0] == text[-1] and text[0] in "'\"":
        return text[1:-1]
    if text.startswith("[") and text.endswith("]"):
        return [_yaml_scalar(x) for x in text[1:-1].split(",") if x.strip()]
    return text


def parse_plugin_yml(text: str) -> dict:
    """The small part of YAML that plugin.yml files use: top-level `key: value`, flow lists `[a, b]`,
    block lists (`- a`), block text (`|`, `>`) and nested maps (paper-plugin.yml `dependencies`)."""
    result: dict = {}
    key = None
    stack: list[tuple[int, dict]] = []  # open nested maps below the current top-level key
    for raw in text.splitlines():
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        line = re.sub(r"\s+#.*$", "", raw.rstrip())
        indent = len(line) - len(line.lstrip())
        body = line.strip()
        if indent == 0 and not body.startswith("-"):
            k, _, v = body.partition(":")
            key, v = k.strip(), v.strip()
            stack = []
            result[key] = "" if v in ("|", ">", "|-", ">-", "|+", ">+") else None if v == "" else _yaml_scalar(v)
            continue
        if key is None:
            continue
        cur = result.get(key)
        if body == "-" or body.startswith("- "):
            if cur is None:
                cur = result[key] = []
            if isinstance(cur, list):
                cur.append(_yaml_scalar(body[1:]))
            continue
        if isinstance(cur, str):  # block text / multi-line scalar continues
            result[key] = (cur + " " + body).strip()
            continue
        if cur is None:
            cur = result[key] = {}
        if not isinstance(cur, dict):
            continue
        while stack and stack[-1][0] >= indent:
            stack.pop()
        parent = stack[-1][1] if stack else cur
        k, _, v = body.partition(":")
        k = k.strip().strip("'\"")
        if v.strip():
            parent[k] = _yaml_scalar(v)
        else:
            parent[k] = {}
            stack.append((indent, parent[k]))
    return {k: [] if v is None else v for k, v in result.items()}


def _authors(value) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [a if isinstance(a, str) else str(a.get("name", "")) for a in value
                if isinstance(a, (str, dict))][:5]
    return []


def _fabric_ids(meta: dict, zf: zipfile.ZipFile, depth: int = 0) -> set[str]:
    """Mod id + `provides` + ids of bundled jar-in-jar mods (Fabric API is a bundle of modules)."""
    ids = {meta.get("id", "")} | {p for p in meta.get("provides", []) if isinstance(p, str)}
    if depth < 2:
        for entry in meta.get("jars", []):
            inner = entry.get("file") if isinstance(entry, dict) else None
            if not isinstance(inner, str):
                continue
            try:
                with zipfile.ZipFile(io.BytesIO(zf.read(inner))) as nested:
                    nested_meta = json.loads(nested.read("fabric.mod.json").decode("utf-8-sig"), strict=False)
                    if isinstance(nested_meta, dict):
                        ids |= _fabric_ids(nested_meta, nested, depth + 1)
            except (KeyError, ValueError, zipfile.BadZipFile, OSError):
                continue
    return {i for i in ids if i}


def _read_meta(path: Path) -> dict | None:
    try:
        with zipfile.ZipFile(path) as zf:
            names = set(zf.namelist())
            if "fabric.mod.json" in names:
                m = json.loads(zf.read("fabric.mod.json").decode("utf-8-sig"), strict=False)
                if not isinstance(m, dict):
                    return None
                deps = m.get("depends", {})
                return {
                    "type": "fabric",
                    "id": str(m.get("id", "")),
                    "name": str(m.get("name") or m.get("id") or path.stem),
                    "version": str(m.get("version", "")),
                    "description": str(m.get("description", ""))[:300],
                    "authors": _authors(m.get("authors")),
                    "environment": str(m.get("environment", "*")),
                    "depends": {k: v for k, v in deps.items() if isinstance(k, str)} if isinstance(deps, dict) else {},
                    "provides": sorted(_fabric_ids(m, zf)),
                }
            for yml, paper in (("paper-plugin.yml", True), ("plugin.yml", False)):
                if yml in names:
                    y = parse_plugin_yml(zf.read(yml).decode("utf-8-sig", errors="replace"))
                    if paper:
                        server = (y.get("dependencies") or {}).get("server") if isinstance(y.get("dependencies"), dict) else None
                        depends = [n for n, v in (server or {}).items()
                                   if not isinstance(v, dict) or str(v.get("required", "true")).lower() != "false"]
                    else:
                        depends = y.get("depend") if isinstance(y.get("depend"), list) else []
                    name = str(y.get("name") or path.stem)
                    return {
                        "type": "paper",
                        "id": name,
                        "name": name,
                        "version": str(y.get("version", "")),
                        "description": str(y.get("description", ""))[:300],
                        "authors": _authors(y.get("authors") or y.get("author")),
                        "api_version": str(y.get("api-version", "")),
                        "depends": [str(d) for d in depends],
                        "provides": [name] + [str(p) for p in y.get("provides", []) if isinstance(y.get("provides"), list)],
                    }
            if "META-INF/mods.toml" in names or "META-INF/neoforge.mods.toml" in names:
                return {"type": "forge", "id": "", "name": path.stem}
            if "quilt.mod.json" in names:
                return {"type": "quilt", "id": "", "name": path.stem}
    except (OSError, ValueError, zipfile.BadZipFile, KeyError, AttributeError, TypeError):
        return None
    return None


def read_meta(path: Path) -> dict | None:
    st = path.stat()
    key = (str(path), st.st_mtime_ns, st.st_size)
    with _meta_lock:
        if key in _meta_cache:
            return _meta_cache[key]
    meta = _read_meta(path)
    with _meta_lock:
        if len(_meta_cache) > 500:
            _meta_cache.clear()
        _meta_cache[key] = meta
    return meta


# ---------- version checks ----------


def _nums(version: str) -> tuple[int, ...] | None:
    m = re.match(r"^v?(\d+(?:\.\d+)*)", version.strip())
    return tuple(int(x) for x in m.group(1).split(".")) if m else None


def _cmp(a: tuple[int, ...], b: tuple[int, ...]) -> int:
    n = max(len(a), len(b))
    a, b = a + (0,) * (n - len(a)), b + (0,) * (n - len(b))
    return (a > b) - (a < b)


def _matches_one(version: tuple[int, ...], constraint: str) -> bool | None:
    """One Fabric version predicate (`>=1.21`, `~1.21.4`, `1.21.x`, ...). None = can't tell."""
    m = re.match(r"^(>=|<=|>|<|=|~|\^)?\s*(.+)$", constraint.strip())
    if not m:
        return None
    op, value = m.group(1) or "=", m.group(2)
    if value in ("*", "x", "X"):
        return True
    wildcard = re.match(r"^(\d+(?:\.\d+)*)\.[xX*]", value)
    if wildcard:
        prefix = tuple(int(x) for x in wildcard.group(1).split("."))
        return version[:len(prefix)] == prefix if op == "=" else None
    target = _nums(value)
    if target is None:
        return None
    c = _cmp(version, target)
    if op == "=":
        return c == 0
    if op == ">=":
        return c >= 0
    if op == "<=":
        return c <= 0
    if op == ">":
        return c > 0
    if op == "<":
        # "<1.21-" excludes pre-releases of 1.21 too: same as < 1.21 for releases
        return c < 0
    if op == "~":  # same minor
        upper = target[:2] if len(target) >= 2 else target
        return c >= 0 and version[:len(upper)] == upper
    if op == "^":  # same major
        return c >= 0 and version[:1] == target[:1]
    return None


def matches(version: str, spec) -> bool | None:
    """Fabric dependency spec (string or list = any of) against a Minecraft version.
    None if either side can't be parsed (snapshots): then nothing is flagged."""
    v = _nums(version)
    if v is None or not re.match(r"^\d+(\.\d+)*$", version.strip()):
        return None
    specs = spec if isinstance(spec, list) else [spec]
    results = []
    for s in specs:
        if not isinstance(s, str):
            continue
        parts = [_matches_one(v, p) for p in s.split()] or [True]
        if any(p is False for p in parts):
            results.append(False)
        elif all(p is True for p in parts):
            results.append(True)
        else:
            results.append(None)
    if any(r is True for r in results):
        return True
    if results and all(r is False for r in results):
        return False
    return None


# ---------- listing ----------


def _files(inst, kind: str) -> list[Path]:
    base = folder(inst, kind)
    if not base.is_dir():
        return []
    out = []
    for p in base.iterdir():
        try:
            if p.is_file() and validate_file(p.name):
                out.append(p)
        except ValueError:
            continue
    return sorted(out, key=lambda p: p.name.lower())


def list_addons(inst, kind: str, game_version: str) -> list[dict]:
    items = []
    for p in _files(inst, kind):
        meta = read_meta(p)
        items.append({"file": p.name, "enabled": not p.name.endswith(DISABLED),
                      "size_kb": round(p.stat().st_size / 1024), "meta": meta})

    want = "fabric" if kind == "mods" else "paper"
    provided = set(_FABRIC_BUILTIN) if kind == "mods" else set()
    for it in items:
        m = it["meta"]
        if it["enabled"] and m and m["type"] == want:
            provided |= {x.lower() for x in m.get("provides", [])} | {m["id"].lower()}

    result = []
    for it in items:
        m = it["meta"] or {}
        problems = []
        if not m:
            problems.append("No mod/plugin metadata found")
        elif m["type"] != want:
            problems.append({"fabric": "This is a Fabric mod, not a plugin", "paper": "This is a plugin, not a Fabric mod",
                             "forge": "Forge/NeoForge mod: doesn't run on this server",
                             "quilt": "Quilt mod: doesn't run on a Fabric server"}[m["type"]])
        missing: list[str] = []
        incompatible = None
        if m.get("type") == want == "fabric":
            missing = sorted(d for d in m["depends"] if d.lower() not in provided)
            if "minecraft" in m["depends"] and game_version and matches(game_version, m["depends"]["minecraft"]) is False:
                spec = m["depends"]["minecraft"]
                incompatible = f"Made for Minecraft {' or '.join(spec) if isinstance(spec, list) else spec}"
        elif m.get("type") == want == "paper":
            missing = sorted(d for d in m["depends"] if d.lower() not in provided)
            api, gv = _nums(m.get("api_version", "")), _nums(game_version or "")
            if api and gv and _cmp(api, gv[:len(api)]) > 0:
                incompatible = f"Needs Paper {m['api_version']} or newer"
        env = m.get("environment", "*")
        result.append({
            "file": it["file"],
            "enabled": it["enabled"],
            "size_kb": it["size_kb"],
            "id": m.get("id", ""),
            "name": m.get("name") or it["file"],
            "version": m.get("version", ""),
            "description": m.get("description", ""),
            "authors": m.get("authors", []),
            # Fabric: "*" = players need it too, "server" = server only, "client" = useless on a server
            "side": {"*": "both", "server": "server", "client": "client"}.get(env, "both") if m.get("type") == "fabric" else None,
            "missing": missing,
            "incompatible": incompatible,
            "problems": problems,
        })
    return result


# ---------- local changes ----------


def add_upload(inst, kind: str, name: str, src: Path) -> str:
    """Check an uploaded jar and move it into the folder (replacing a file of the same name)."""
    validate_file(name)
    if name.endswith(DISABLED):
        raise ValueError("Upload a .jar file")
    meta = _read_meta(src)
    if meta is None:
        try:
            with zipfile.ZipFile(src):
                pass
        except zipfile.BadZipFile:
            raise ValueError("Not a valid .jar file") from None
        raise ValueError("No fabric.mod.json / plugin.yml in the jar: not a Fabric mod or Paper plugin")
    want = "fabric" if kind == "mods" else "paper"
    if meta["type"] != want:
        raise ValueError(f"{name} is not a {'Fabric mod' if kind == 'mods' else 'Paper/Bukkit plugin'}")
    target = folder(inst, kind) / name
    target.parent.mkdir(parents=True, exist_ok=True)
    (target.with_name(name + DISABLED)).unlink(missing_ok=True)
    src.replace(target)
    return name


def set_enabled(inst, kind: str, name: str, enabled: bool) -> str:
    """Rename to/from .jar.disabled. Returns the new file name."""
    path = path_of(inst, kind, name)
    base = name[: -len(DISABLED)] if name.endswith(DISABLED) else name
    new = base if enabled else base + DISABLED
    if new == name:
        return name
    target = path.with_name(new)
    if target.exists():
        raise FileExistsError(f"{new} exists already")
    path.rename(target)
    return new


def delete(inst, kind: str, name: str) -> None:
    path_of(inst, kind, name).unlink()


# ---------- Modrinth ----------


def _api(path: str, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(f"{MODRINTH}{path}", data=data, headers={
        "User-Agent": USER_AGENT, **({"Content-Type": "application/json"} if data else {})})
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.load(resp)


def validate_project(project_id: str) -> str:
    if not _PROJECT_RE.match(project_id):
        raise ValueError("Invalid project id")
    return project_id


def search(kind: str, game_version: str, query: str, offset: int = 0) -> dict:
    query = query.strip()[:100]
    facets = [["server_side:required", "server_side:optional"]]
    if kind == "mods":
        facets += [["project_type:mod"], ["categories:fabric"]]
    else:
        facets += [[f"categories:{loader}" for loader in LOADERS["plugins"]]]
    if game_version:
        facets.append([f"versions:{game_version}"])
    params = {"query": query, "facets": json.dumps(facets), "limit": "20", "offset": str(max(0, int(offset))),
              "index": "relevance" if query else "downloads"}
    data = _api("/search?" + urllib.parse.urlencode(params))
    return {
        "total": data.get("total_hits", 0),
        "hits": [{
            "project_id": h.get("project_id"), "slug": h.get("slug"), "title": h.get("title"),
            "description": h.get("description", "")[:200], "author": h.get("author"),
            "downloads": h.get("downloads", 0), "icon_url": h.get("icon_url") or None,
            "client_side": h.get("client_side"), "server_side": h.get("server_side"),
        } for h in data.get("hits", [])],
    }


def _best_version(project_id: str, kind: str, game_version: str) -> dict:
    params = {"loaders": json.dumps(LOADERS[kind])}
    if game_version:
        params["game_versions"] = json.dumps([game_version])
    versions = _api(f"/project/{urllib.parse.quote(validate_project(project_id))}/version?"
                    + urllib.parse.urlencode(params))
    if not versions:
        raise LookupError(f"No version for Minecraft {game_version or '?'} on {', '.join(LOADERS[kind])}")
    # Releases first, newest first (the API already sorts by date)
    versions.sort(key=lambda v: v.get("version_type") != "release")
    return versions[0]


def _primary(version: dict) -> dict:
    files = version.get("files") or []
    f = next((f for f in files if f.get("primary")), files[0] if files else None)
    if not f:
        raise ValueError("Modrinth version has no files")
    return f


def _sha512(path: Path) -> str:
    h = hashlib.sha512()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


def _download(file: dict, target_dir: Path, disabled: bool = False) -> str:
    """Download a Modrinth file into the folder, SHA-512 verified. Returns the file name."""
    name = validate_file(file["filename"])
    url = urllib.parse.urlparse(file["url"])
    if url.scheme != "https" or url.hostname not in _CDN_HOSTS:
        raise ValueError(f"Unexpected download host: {url.hostname}")
    expected = file.get("hashes", {}).get("sha512")
    if not expected:
        raise ValueError("Modrinth gave no SHA-512 for this file")
    target_dir.mkdir(parents=True, exist_ok=True)
    tmp = target_dir / f".{name}.part"
    h = hashlib.sha512()
    req = urllib.request.Request(file["url"], headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=60) as resp, open(tmp, "wb") as f:
            size = 0
            while chunk := resp.read(1 << 16):
                size += len(chunk)
                if size > MAX_UPLOAD:
                    raise ValueError("Download too large")
                h.update(chunk)
                f.write(chunk)
        if h.hexdigest() != expected:
            raise ValueError("Checksum mismatch, download discarded")
        final = name + (DISABLED if disabled else "")
        tmp.replace(target_dir / final)
        return final
    finally:
        tmp.unlink(missing_ok=True)


def install(inst, kind: str, game_version: str, project_id: str, log=print) -> list[str]:
    """Install the best matching version of a project plus its required dependencies."""
    base = folder(inst, kind)
    installed: list[str] = []
    seen: set[str] = set()
    local_hashes = {_sha512(p) for p in _files(inst, kind)}
    local_ids = {(read_meta(p) or {}).get("id", "").lower() for p in _files(inst, kind)}

    def add(pid: str, depth: int) -> None:
        if pid in seen or depth > 4:
            return
        seen.add(pid)
        version = _best_version(pid, kind, game_version)
        file = _primary(version)
        if file.get("hashes", {}).get("sha512") in local_hashes:
            return  # exactly this file is there already
        if depth and local_ids:
            project = _api(f"/project/{urllib.parse.quote(validate_project(pid))}")
            if str(project.get("slug", "")).lower() in local_ids:
                return  # dependency installed under another file (e.g. uploaded by hand)
        name = _download(file, base)
        installed.append(name)
        log(f"Installed {name}")
        for dep in version.get("dependencies") or []:
            if dep.get("dependency_type") == "required" and dep.get("project_id"):
                add(dep["project_id"], depth + 1)

    add(validate_project(project_id), 0)
    return installed


def check_updates(inst, kind: str, game_version: str) -> dict:
    """{file: {"version": "1.2.3", "filename": ...}} for files Modrinth has a newer version of."""
    files = {p.name: _sha512(p) for p in _files(inst, kind)}
    if not files:
        return {}
    body = {"hashes": list(files.values()), "algorithm": "sha512", "loaders": LOADERS[kind]}
    if game_version:
        body["game_versions"] = [game_version]
    try:
        latest = _api("/version_files/update", body)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return {}
        raise
    result = {}
    for name, h in files.items():
        v = latest.get(h)
        if not v:
            continue
        f = _primary(v)
        if f.get("hashes", {}).get("sha512") != h:
            result[name] = {"version": v.get("version_number", ""), "filename": f.get("filename")}
    return result


def update(inst, kind: str, game_version: str, name: str, log=print) -> str:
    """Replace one file with Modrinth's newest compatible version. Returns the new file name."""
    path = path_of(inst, kind, name)
    h = _sha512(path)
    body = {"hashes": [h], "algorithm": "sha512", "loaders": LOADERS[kind]}
    if game_version:
        body["game_versions"] = [game_version]
    v = _api("/version_files/update", body).get(h)
    if not v:
        raise LookupError(f"Modrinth knows no newer version of {name}")
    f = _primary(v)
    if f.get("hashes", {}).get("sha512") == h:
        return name
    new = _download(f, path.parent, disabled=name.endswith(DISABLED))
    if new != name:
        path.unlink(missing_ok=True)
    log(f"Updated {name} to {v.get('version_number', new)}")
    return new
