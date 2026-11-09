"""Server jars: Vanilla (Mojang) and Paper (PaperMC Fill API v3).

Jars are stored as `<JARS_DIR>/<flavor>-<version>.jar` with a sidecar
`<flavor>-<version>.json` holding the required Java version, build and JVM flags.
"""

import hashlib
import json
import shutil
import time
import urllib.request
from pathlib import Path

from . import config

MOJANG_MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
PAPER_API = "https://fill.papermc.io/v3/projects/paper"
# PaperMC asks API clients to identify themselves with a descriptive User-Agent
USER_AGENT = "minecraft-server-gui/1.0 (self-hosted server panel)"
FLAVORS = ("vanilla", "paper")
_CACHE_TTL = 600

_cache: dict[str, tuple[float, object]] = {}


def _get_json(url: str):
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.load(resp)


def _cached(url: str):
    hit = _cache.get(url)
    if hit and time.time() - hit[0] < _CACHE_TTL:
        return hit[1]
    data = _get_json(url)
    _cache[url] = (time.time(), data)
    return data


def validate_flavor(flavor: str) -> str:
    if flavor not in FLAVORS:
        raise ValueError(f"Unknown server type: {flavor}")
    return flavor


# ---------- version ordering (for upgrade/downgrade detection) ----------


def _mojang_ranks() -> dict[str, int]:
    """Version id -> position in Mojang's manifest (0 = newest)."""
    try:
        return {v["id"]: i for i, v in enumerate(_cached(MOJANG_MANIFEST)["versions"])}
    except (OSError, ValueError, KeyError):
        return {}


def rank(version_id: str) -> int | None:
    return _mojang_ranks().get(version_id)


# ---------- listing ----------


def list_versions(flavor: str, include_snapshots: bool = False) -> dict:
    validate_flavor(flavor)
    ranks = _mojang_ranks()
    if flavor == "vanilla":
        m = _cached(MOJANG_MANIFEST)
        versions = [
            {"id": v["id"], "snapshot": v["type"] != "release", "rank": ranks.get(v["id"])}
            for v in m["versions"]
            if v["type"] == "release" or (include_snapshots and v["type"] == "snapshot")
        ]
        return {"latest": m["latest"]["release"], "versions": versions}

    project = _cached(PAPER_API)
    ids = [v for group in project["versions"].values() for v in group]
    versions = []
    for vid in ids:
        snapshot = "-" in vid  # "26.3-rc-3", "1.21-pre1", ...
        if snapshot and not include_snapshots:
            continue
        versions.append({"id": vid, "snapshot": snapshot, "rank": ranks.get(vid)})
    # Newest first: by Mojang order where known, unknown ids keep API order at the end
    versions.sort(key=lambda v: (v["rank"] is None, v["rank"] or 0))
    latest = next((v["id"] for v in versions if not v["snapshot"]), None)
    return {"latest": latest, "versions": versions}


# ---------- installed jars ----------


def _path(flavor: str, version_id: str, ext: str) -> Path:
    # Validation prevents path traversal through user-supplied ids
    return config.JARS_DIR / f"{validate_flavor(flavor)}-{config.validate_version(version_id)}{ext}"


def jar_path(flavor: str, version_id: str) -> Path:
    return _path(flavor, version_id, ".jar")


def meta_path(flavor: str, version_id: str) -> Path:
    return _path(flavor, version_id, ".json")


def meta(flavor: str, version_id: str) -> dict:
    try:
        return json.loads(meta_path(flavor, version_id).read_text())
    except (OSError, ValueError):
        return {}


def required_java(flavor: str, version_id: str) -> int:
    return int(meta(flavor, version_id).get("java", 8))


def jvm_flags(flavor: str, version_id: str) -> list[str]:
    return [f for f in meta(flavor, version_id).get("flags", []) if f.startswith("-XX:")]


def installed() -> list[dict]:
    result = []
    for jar in sorted(config.JARS_DIR.glob("*.jar")):
        flavor, sep, version_id = jar.stem.partition("-")
        if not sep or flavor not in FLAVORS:
            continue
        info = meta(flavor, version_id)
        result.append({
            "flavor": flavor,
            "version": version_id,
            "build": info.get("build"),
            "java": info.get("java"),
            "size_mb": round(jar.stat().st_size / 1024 / 1024, 1),
            "rank": rank(version_id),
        })
    result.sort(key=lambda v: (v["rank"] is None, v["rank"] or 0, v["flavor"]))
    return result


def _ignored_file() -> Path:
    return config.JARS_DIR / "ignored-paper-cache.json"


def _ignored() -> set[str]:
    try:
        return set(json.loads(_ignored_file().read_text()))
    except (OSError, ValueError):
        return set()


def delete(flavor: str, version_id: str) -> None:
    jar = jar_path(flavor, version_id)
    if not jar.exists():
        raise FileNotFoundError(f"{flavor} {version_id} is not downloaded")
    jar.unlink()
    meta_path(flavor, version_id).unlink(missing_ok=True)
    if flavor == "vanilla" and (config.SERVER_DIR / "cache" / f"mojang_{version_id}.jar").exists():
        # Don't re-import it from Paper's cache after the user deleted it
        _ignored_file().write_text(json.dumps(sorted(_ignored() | {version_id})))


# ---------- download ----------


def _fetch(url: str, target: Path, algo: str, expected: str) -> None:
    tmp = target.with_name(target.name + ".part")
    digest = hashlib.new(algo)
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=60) as resp, open(tmp, "wb") as f:
            while chunk := resp.read(1 << 16):
                digest.update(chunk)
                f.write(chunk)
        if digest.hexdigest() != expected:
            raise ValueError("Checksum mismatch, download discarded")
        tmp.replace(target)
    finally:
        tmp.unlink(missing_ok=True)


def _download_vanilla(version_id: str, log) -> dict:
    entry = next((v for v in _cached(MOJANG_MANIFEST)["versions"] if v["id"] == version_id), None)
    if entry is None:
        raise ValueError(f"Unknown Minecraft version: {version_id}")
    info = _get_json(entry["url"])
    server = info.get("downloads", {}).get("server")
    if not server:
        raise ValueError(f"Mojang provides no server jar for {version_id}")
    log(f"Downloading Vanilla {version_id} ({server['size'] // 2**20} MB) ...")
    _fetch(server["url"], jar_path("vanilla", version_id), "sha1", server["sha1"])
    return {"java": info.get("javaVersion", {}).get("majorVersion", 8)}


def _latest_paper_build(version_id: str, log) -> tuple[dict, dict]:
    """(version info, newest build) - the newest STABLE build, else the newest of any channel."""
    info = _get_json(f"{PAPER_API}/versions/{version_id}")["version"]
    builds = _get_json(f"{PAPER_API}/versions/{version_id}/builds?channel=STABLE")
    if not builds:
        builds = _get_json(f"{PAPER_API}/versions/{version_id}/builds")
        if not builds:
            raise ValueError(f"Paper has no builds for {version_id}")
        log(f"Note: no stable Paper build for {version_id}, using {builds[0]['channel'].lower()} build.")
    return info, builds[0]


def _fetch_paper(version_id: str, info: dict, build: dict, log) -> dict:
    dl = build["downloads"]["server:default"]
    log(f"Downloading Paper {version_id} build {build['id']} ({dl['size'] // 2**20} MB) ...")
    _fetch(dl["url"], jar_path("paper", version_id), "sha256", dl["checksums"]["sha256"])
    java = info.get("java", {})
    return {
        "java": java.get("version", {}).get("minimum", 21),
        "build": build["id"],
        "flags": java.get("flags", {}).get("recommended", []),
    }


def _download_paper(version_id: str, log) -> dict:
    info, build = _latest_paper_build(version_id, log)
    return _fetch_paper(version_id, info, build, log)


def update_paper(version_id: str, log=print) -> bool:
    """Replace a downloaded Paper jar with the newest build. Returns True if it was replaced."""
    jar = jar_path("paper", version_id)
    if not jar.exists():
        raise FileNotFoundError(f"Paper {version_id} is not downloaded")
    current = meta("paper", version_id).get("build")
    info, build = _latest_paper_build(version_id, log)
    if isinstance(current, int) and build["id"] <= current:
        log(f"Paper {version_id} is up to date (build {current}).")
        return False
    try:
        new_meta = _fetch_paper(version_id, info, build, log)
    except PermissionError as e:
        # Windows keeps the jar of a running server locked
        raise RuntimeError("The jar is in use: stop the server first, then update") from e
    meta_path("paper", version_id).write_text(json.dumps(new_meta))
    log(f"Paper {version_id} updated to build {build['id']}"
        + (f" (was {current})" if current else "") + ". Takes effect on the next server start.")
    return True


def _file_hash(path: Path, algo: str) -> str:
    digest = hashlib.new(algo)
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def import_paper_cache(log=print) -> list[str]:
    """Paper downloads the original Mojang jar to `server/cache/mojang_<version>.jar`.
    Register those as downloaded Vanilla servers so they needn't be fetched again.
    Each file is verified against Mojang's SHA1 first. Returns the imported versions."""
    imported = []
    ignored = _ignored()
    for cached in sorted((config.SERVER_DIR / "cache").glob("mojang_*.jar")):
        version_id = cached.stem.removeprefix("mojang_")
        if version_id in ignored:
            continue
        try:
            target = jar_path("vanilla", version_id)
        except ValueError:
            continue
        if target.exists():
            continue
        try:
            entry = next((v for v in _cached(MOJANG_MANIFEST)["versions"] if v["id"] == version_id), None)
            if entry is None:
                continue
            info = _get_json(entry["url"])
            server = info.get("downloads", {}).get("server")
            if not server or _file_hash(cached, "sha1") != server["sha1"]:
                continue  # incomplete or not the official jar
            try:
                target.hardlink_to(cached)  # no extra disk space
            except OSError:
                shutil.copy2(cached, target)
            meta_path("vanilla", version_id).write_text(
                json.dumps({"java": info.get("javaVersion", {}).get("majorVersion", 8)})
            )
            imported.append(version_id)
            log(f"Vanilla {version_id} taken over from Paper's cache, added to downloaded servers.")
        except (OSError, ValueError, KeyError):
            continue  # offline or unexpected data: try again next time
    return imported


def download(flavor: str, version_id: str, log=print) -> Path:
    """Download and verify a server jar (no-op if already present)."""
    target = jar_path(flavor, version_id)
    if target.exists():
        return target
    info = _download_vanilla(version_id, log) if flavor == "vanilla" else _download_paper(version_id, log)
    meta_path(flavor, version_id).write_text(json.dumps(info))
    if flavor == "vanilla" and version_id in (ignored := _ignored()):
        _ignored_file().write_text(json.dumps(sorted(ignored - {version_id})))
    log(f"{flavor.capitalize()} {version_id} downloaded (needs Java {info['java']}).")
    return target
