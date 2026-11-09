"""FastAPI web application: REST actions + WebSocket live feed."""

import asyncio
import hashlib
import hmac
import logging
import secrets
import time
from contextlib import asynccontextmanager
from pathlib import Path

import psutil
from fastapi import Depends, FastAPI, HTTPException, Request, Response, WebSocket
from fastapi.responses import FileResponse, HTMLResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

from . import access, backup, config, datapacks, java, livemap, properties, versions, world
from .mapserver import MapServer
from .server import BusyError, manager

log = logging.getLogger("mcgui")
STATIC_DIR = Path(__file__).parent / "static"
COOKIE = "mcgui_session"

_password = config.PASSWORD
_sessions: dict[str, float] = {}
SESSION_TTL = 7 * 24 * 3600


# ---------- auth ----------


def _valid(token: str | None) -> bool:
    if not token:
        return False
    expires = _sessions.get(token)
    if not expires or expires < time.time():
        _sessions.pop(token, None)
        return False
    return True


def require_auth(request: Request) -> None:
    if not _valid(request.cookies.get(COOKIE)):
        raise HTTPException(401, "Not logged in")


# ---------- lifecycle ----------


# Inside a container the network counters only cover this container, i.e. exactly
# Minecraft + panel. On a normal host they would be machine-wide, so they're omitted there.
IN_CONTAINER = Path("/.dockerenv").exists()
CPU_COUNT = psutil.cpu_count() or 1


def _usage(proc: psutil.Process) -> dict:
    """CPU as % of the whole machine (not per core) and resident memory in MB."""
    with proc.oneshot():
        return {
            "cpu": round(proc.cpu_percent() / CPU_COUNT, 1),
            "ram": proc.memory_info().rss // 2**20,
        }


async def _stats_loop() -> None:
    panel = psutil.Process()
    panel.cpu_percent()  # first call only primes the measurement
    mc: psutil.Process | None = None
    last_net = psutil.net_io_counters() if IN_CONTAINER else None
    last_time = time.monotonic()
    while True:
        await asyncio.sleep(2)
        if not manager.has_subscribers():
            # Nobody is watching: skip the work (keep the baseline fresh for the next sample)
            last_time = time.monotonic()
            if last_net is not None:
                last_net = psutil.net_io_counters()
            continue
        try:
            now = time.monotonic()
            elapsed, last_time = now - last_time, now

            # Keep the same Process object between samples, cpu_percent() needs the previous one
            mc_stats = None
            pid = manager.process.pid if manager.is_alive() else None
            if pid is None:
                mc = None
            else:
                try:
                    if mc is None or mc.pid != pid:
                        mc = psutil.Process(pid)
                        mc.cpu_percent()
                    mc_stats = _usage(mc)
                    mc_stats["ram_max"] = manager.running_max_ram_mb
                except psutil.Error:
                    mc = None

            stats = {
                "minecraft": mc_stats,
                "panel": _usage(panel),
                "net": None,
                "uptime": manager.status()["uptime"],
            }
            if last_net is not None:
                net = psutil.net_io_counters()
                stats["net"] = {
                    "up": round((net.bytes_sent - last_net.bytes_sent) / elapsed / 1024, 1),
                    "down": round((net.bytes_recv - last_net.bytes_recv) / elapsed / 1024, 1),
                }
                last_net = net
            manager._emit({"type": "stats", "stats": stats})
        except Exception as e:  # noqa: BLE001
            log.warning("stats failed: %s", e)


async def _map_render_loop() -> None:
    """Keep the map current: once at startup, then every MAP_INTERVAL s while the server runs.
    mapgen only touches changed region files, so a check without changes costs milliseconds.
    (Minecraft writes chunks to disk on autosave, so that is how fresh the map can be.)"""
    first = True
    while True:
        if manager.map_enabled and (first or manager.state.value == "running"):
            had_map = livemap.available()
            await asyncio.to_thread(livemap.renderer.render, manager.info)
            if not had_map and livemap.available():
                manager._emit({"type": "status", "status": manager.status()})  # map link appears
        first = False
        await asyncio.sleep(config.MAP_INTERVAL)


async def _map_players_loop() -> None:
    """Live player positions, only while somebody has the map open (no idle console traffic)."""
    while True:
        await asyncio.sleep(2)
        if not (manager.map_enabled and manager.state.value == "running" and livemap.players.watching()):
            continue
        for cmd in livemap.players.queries(set(manager.players)):
            try:
                manager.send_command(cmd)
            except RuntimeError:
                break


async def _backup_schedule() -> None:
    """Automatic backups every BACKUP_INTERVAL_HOURS, only while the server runs
    (a stopped world doesn't change, and extra copies would rotate out older good ones)."""
    while True:
        await asyncio.sleep(config.BACKUP_INTERVAL_HOURS * 3600)
        if manager.state.value != "running":
            continue
        try:
            manager.info("Scheduled backup")
            manager.make_backup()
        except BusyError:
            manager.info("Scheduled backup skipped: another action is running.")


@asynccontextmanager
async def lifespan(app: FastAPI):
    if not _password:
        raise RuntimeError("MCGUI_PASSWORD is not set. Refusing to start without a panel password.")
    config.ensure_dirs()
    manager.bind_loop(asyncio.get_running_loop())
    # Pick up Mojang jars Paper downloaded in earlier runs (needs network, so in background)
    asyncio.get_running_loop().run_in_executor(None, versions.import_paper_cache, manager.info)
    tasks = [
        asyncio.create_task(_stats_loop()),
        asyncio.create_task(_map_render_loop()),
        asyncio.create_task(_map_players_loop()),
    ]
    if config.BACKUP_INTERVAL_HOURS > 0:
        tasks.append(asyncio.create_task(_backup_schedule()))
    map_server = MapServer()
    try:
        map_server.start()
    except Exception as e:  # noqa: BLE001 - the panel must work without the map
        log.warning("live map server not started: %s", e)
    yield
    for t in tasks:
        t.cancel()
    map_server.stop()
    await asyncio.to_thread(manager.shutdown)


app = FastAPI(title="Minecraft Server GUI", lifespan=lifespan)


# ---------- models ----------


class LoginBody(BaseModel):
    password: str


class CommandBody(BaseModel):
    command: str


class StopBody(BaseModel):
    countdown: int = 0


class SettingsBody(BaseModel):
    map_enabled: bool | None = None
    flavor: str | None = None
    version: str | None = None
    min_ram: str | None = None
    max_ram: str | None = None


class VersionBody(BaseModel):
    flavor: str = "vanilla"
    version: str


def _busy(fn, *args):
    try:
        fn(*args)
    except BusyError as e:
        raise HTTPException(409, str(e))
    except (RuntimeError, ValueError) as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


# ---------- routes ----------


@app.post("/api/login")
async def login(body: LoginBody, response: Response):
    if not hmac.compare_digest(body.password.encode(), _password.encode()):
        await asyncio.sleep(1)  # slow down guessing
        raise HTTPException(401, "Wrong password")
    now = time.time()
    for t in [t for t, exp in _sessions.items() if exp < now]:
        del _sessions[t]  # prune expired sessions
    token = secrets.token_urlsafe(32)
    _sessions[token] = now + SESSION_TTL
    response.set_cookie(COOKIE, token, httponly=True, samesite="strict", max_age=SESSION_TTL)
    return {"ok": True}


@app.post("/api/logout")
async def logout(request: Request, response: Response):
    _sessions.pop(request.cookies.get(COOKIE, ""), None)
    response.delete_cookie(COOKIE)
    return {"ok": True}


auth = [Depends(require_auth)]


def _state() -> dict:
    s = config.load_settings()
    if not s.world_version:
        # World not started through the panel yet (e.g. imported): ask level.dat
        s.world_version = world.saved_version() or ""
    return {
        "status": manager.status(),
        "settings": {"flavor": s.flavor, "version": s.version, "min_ram": s.min_ram, "max_ram": s.max_ram,
                     "map_enabled": s.map_enabled},
        "world": {
            "version": s.world_version or None,
            "flavor": s.world_flavor or None,
            "rank": versions.rank(s.world_version) if s.world_version else None,
        },
        "installed": versions.installed(),
        "java": java.runtimes(),
        "log": list(manager.log),
        "chat": list(manager.chat),
        "backups": backup.list_backups(),
        "backup_keep": config.BACKUP_KEEP,
        "backup_interval": config.BACKUP_INTERVAL_HOURS,
    }


@app.get("/api/state", dependencies=auth)
async def state():
    # May hit the network (version ranks) and spawn `java -version`: keep it off the event loop
    return await asyncio.to_thread(_state)


@app.post("/api/start", dependencies=auth)
async def start():
    return _busy(manager.start)


@app.post("/api/stop", dependencies=auth)
async def stop(body: StopBody):
    return _busy(manager.stop, max(0, min(body.countdown, 300)))


@app.post("/api/restart", dependencies=auth)
async def restart(body: StopBody):
    return _busy(manager.restart, max(0, min(body.countdown, 300)))


@app.post("/api/kill", dependencies=auth)
async def kill():
    manager.kill()
    return {"ok": True}


@app.post("/api/cancel", dependencies=auth)
async def cancel():
    manager.cancel_countdown()
    return {"ok": True}


@app.post("/api/command", dependencies=auth)
async def command(body: CommandBody):
    cmd = body.command.strip().lstrip("/")
    if not cmd or "\n" in cmd or "\r" in cmd:
        raise HTTPException(400, "Invalid command")
    return _busy(manager.send_command, cmd)


@app.post("/api/eula", dependencies=auth)
async def eula():
    manager.accept_eula()
    return {"ok": True}


@app.get("/api/versions", dependencies=auth)
async def list_versions(flavor: str = "vanilla", snapshots: bool = False):
    try:
        versions.validate_flavor(flavor)
    except ValueError as e:
        raise HTTPException(400, str(e))
    try:
        return await asyncio.to_thread(versions.list_versions, flavor, snapshots)
    except (OSError, ValueError, KeyError) as e:
        source = "Mojang" if flavor == "vanilla" else "PaperMC"
        raise HTTPException(502, f"Could not load version list from {source}: {e}")


@app.post("/api/versions/install", dependencies=auth)
async def install(body: VersionBody):
    try:
        versions.jar_path(body.flavor, body.version)  # validates both
    except ValueError as e:
        raise HTTPException(400, str(e))
    return _busy(manager.install, body.flavor, body.version)


@app.post("/api/versions/paper/{version}/update", dependencies=auth)
async def update_paper(version: str):
    try:
        if not versions.jar_path("paper", version).exists():  # validates the id
            raise HTTPException(404, f"Paper {version} is not downloaded")
    except ValueError as e:
        raise HTTPException(400, str(e))
    return _busy(manager.update_paper, version)


@app.delete("/api/versions/{flavor}/{version}", dependencies=auth)
async def delete_version(flavor: str, version: str):
    if manager.is_alive() and (manager.running_flavor, manager.running_version) == (flavor, version):
        raise HTTPException(409, "This server version is currently running")
    if manager.task == "download":
        raise HTTPException(409, "A download is in progress")
    try:
        versions.delete(flavor, version)
    except ValueError as e:
        raise HTTPException(400, str(e))
    except FileNotFoundError as e:
        raise HTTPException(404, str(e))
    return {"ok": True}


@app.post("/api/settings", dependencies=auth)
async def update_settings(body: SettingsBody):
    s = config.load_settings()
    try:
        if body.min_ram is not None:
            s.min_ram = config.validate_ram(body.min_ram)
        if body.max_ram is not None:
            s.max_ram = config.validate_ram(body.max_ram)
    except ValueError as e:
        raise HTTPException(400, str(e))
    if config.ram_mb(s.min_ram) > config.ram_mb(s.max_ram):
        raise HTTPException(400, "Min RAM must not exceed max RAM")
    new_flavor = body.flavor if body.flavor is not None else s.flavor
    new_version = body.version if body.version is not None else s.version
    if (new_flavor, new_version) != (s.flavor, s.version):
        if manager.is_alive():
            raise HTTPException(409, "Stop the server before changing the version")
        try:
            s.flavor = versions.validate_flavor(new_flavor)
            s.version = config.validate_version(new_version) if new_version else ""
        except ValueError as e:
            raise HTTPException(400, str(e))
    if body.map_enabled is not None:
        s.map_enabled = body.map_enabled  # plugin install/removal applies on next (re)start
        manager.map_enabled = body.map_enabled  # public map page follows immediately
    config.save_settings(s)
    manager._emit({"type": "status", "status": manager.status()})
    return {"ok": True}


# ---------- whitelist & operators ----------


class AccessBody(BaseModel):
    action: str  # "add" | "remove"
    name: str


class EnabledBody(BaseModel):
    enabled: bool


def _server_mode() -> str:
    """'online' = use server commands, 'offline' = edit files; refuse in between."""
    if not manager.is_alive():
        return "offline"
    if manager.state.value == "running":
        return "online"
    raise HTTPException(409, f"Server is {manager.state.value}, try again in a moment")


@app.get("/api/access", dependencies=auth)
async def get_access():
    return access.state()


async def _change(kind: str, body: AccessBody):
    if body.action not in ("add", "remove"):
        raise HTTPException(400, "action must be add or remove")
    try:
        name = access.validate_name(body.name)
    except ValueError as e:
        raise HTTPException(400, str(e))

    if _server_mode() == "online":
        file = "whitelist.json" if kind == "whitelist" else "ops.json"
        cmd = {("whitelist", "add"): "whitelist add", ("whitelist", "remove"): "whitelist remove",
               ("ops", "add"): "op", ("ops", "remove"): "deop"}[(kind, body.action)]
        before = access.mtime(file)
        _busy(manager.send_command, f"{cmd} {name}")
        # The server answers in the console (e.g. unknown player); wait for its file update
        await asyncio.to_thread(access.wait_for_change, file, before)
        return {"ok": True, "message": f"Sent: /{cmd} {name}", **access.state()}

    try:
        msg = await asyncio.to_thread(access.offline_change, kind, body.action, name)
    except LookupError as e:
        raise HTTPException(404, str(e))
    except OSError as e:
        raise HTTPException(502, f"Could not reach Mojang to look up {name}: {e}")
    manager.info(msg)
    return {"ok": True, "message": msg, **access.state()}


@app.post("/api/access/whitelist", dependencies=auth)
async def change_whitelist(body: AccessBody):
    return await _change("whitelist", body)


@app.post("/api/access/ops", dependencies=auth)
async def change_ops(body: AccessBody):
    return await _change("ops", body)


@app.post("/api/access/whitelist/enabled", dependencies=auth)
async def set_whitelist_enabled(body: EnabledBody):
    if _server_mode() == "online":
        _busy(manager.send_command, f"whitelist {'on' if body.enabled else 'off'}")
        await asyncio.sleep(0.5)  # server writes server.properties
    else:
        access.set_property("white-list", "true" if body.enabled else "false")
        manager.info(f"Whitelist {'enabled' if body.enabled else 'disabled'} (applies on next start)")
    return {"ok": True, **access.state()}


@app.post("/api/backups", dependencies=auth)
async def create_backup():
    return _busy(manager.make_backup)


@app.post("/api/map/render", dependencies=auth)
async def render_map():
    if not livemap.find_binary():
        raise HTTPException(400, "Map renderer 'mapgen' not found (build it: cargo build --release in mapgen/)")
    return _busy(manager.render_map)


class NewWorldBody(BaseModel):
    seed: str = ""
    datapacks: list[str] = []  # installed into the new world before it is generated
    experiments: list[str] = []  # built-in feature packs, e.g. trade_rebalance


@app.post("/api/world/new", dependencies=auth)
async def new_world(body: NewWorldBody):
    return _busy(manager.new_world, body.seed, body.datapacks, body.experiments)


@app.post("/api/backups/{name}/restore", dependencies=auth)
async def restore_backup(name: str):
    try:
        return _busy(manager.restore, name)
    except FileNotFoundError:
        raise HTTPException(404, "Backup not found")


# ---------- data packs ----------


def _datapack_errors(fn, *args):
    try:
        return fn(*args)
    except FileNotFoundError as e:
        raise HTTPException(404, str(e))
    except FileExistsError as e:
        raise HTTPException(409, str(e))
    except PermissionError as e:  # Windows: the running server keeps the zip open
        raise HTTPException(409, f"File in use, stop the server first ({e})")
    except (RuntimeError, ValueError) as e:
        raise HTTPException(400, str(e))


async def _apply_online(cmd: str, expect) -> bool:
    """Send a /datapack or /reload command, then save until level.dat shows the result."""
    if manager.task:
        raise HTTPException(409, f"Another action is running: {manager.task}")
    _busy(manager.send_command, cmd)
    for _ in range(3):  # the reload behind the command runs asynchronously
        await asyncio.sleep(0.5)
        await asyncio.to_thread(manager.save_and_wait, 30)
        st = await asyncio.to_thread(datapacks.state)
        if st and expect(st):
            return True
    return False


async def _install(name: str) -> None:
    """Copy library -> world; while running, /reload so Minecraft picks it up."""
    mode = _server_mode()
    await asyncio.to_thread(_datapack_errors, datapacks.install, name)
    if mode == "online":
        pid = datapacks.pack_id(name)
        # /reload discovers new packs and enables them (a pack disabled before stays disabled)
        ok = await _apply_online("minecraft:reload", lambda st: pid in st[0] | st[1])
        manager.info(f"Data pack {name} installed" + (" and loaded." if ok else ": see the console for errors."))
    else:
        manager.info(f"Data pack {name} installed in the world, loaded on next start.")


def _packs():
    return asyncio.to_thread(datapacks.list_packs)


@app.get("/api/datapacks", dependencies=auth)
async def list_datapacks():
    return await _packs()


@app.post("/api/datapacks/upload", dependencies=auth)
async def upload_datapack(name: str, request: Request):
    """Raw .zip body -> library, then installed into the current world."""
    _server_mode()  # refuse early while starting/stopping
    _datapack_errors(datapacks.validate_name, name)
    tmp = config.DATA_DIR / f".upload-{secrets.token_hex(8)}"
    try:
        with open(tmp, "wb") as f:
            size = 0
            async for chunk in request.stream():
                size += len(chunk)
                if size > datapacks.MAX_UPLOAD:
                    raise HTTPException(413, f"File too large (max {datapacks.MAX_UPLOAD // 2**20} MB)")
                f.write(chunk)
        await asyncio.to_thread(_datapack_errors, datapacks.add_upload, name, tmp)
    finally:
        tmp.unlink(missing_ok=True)
    manager.info(f"Data pack {name} added to the library.")
    await _install(name)
    return await _packs()


@app.post("/api/datapacks/{name}/install", dependencies=auth)
async def install_datapack(name: str):
    await _install(name)
    return await _packs()


@app.post("/api/datapacks/{name}/adopt", dependencies=auth)
async def adopt_datapack(name: str):
    await asyncio.to_thread(_datapack_errors, datapacks.adopt, name)
    manager.info(f"Data pack {name} copied to the library.")
    return await _packs()


class DatapackToggleBody(BaseModel):
    enabled: bool


@app.post("/api/datapacks/{name}/enabled", dependencies=auth)
async def toggle_datapack(name: str, body: DatapackToggleBody):
    _datapack_errors(datapacks.path_of, name)
    if _server_mode() == "online":
        pid = datapacks.pack_id(name)
        verb = "enable" if body.enabled else "disable"
        ok = await _apply_online(f'minecraft:datapack {verb} "{pid}"',
                                 lambda st: (pid in st[0]) == body.enabled)
        if not ok:
            raise HTTPException(400, f"The server did not {verb} {name}: see the console")
        manager.info(f"Data pack {name} {verb}d.")
    else:
        manager.info(await asyncio.to_thread(_datapack_errors, datapacks.set_enabled_offline, name, body.enabled))
    return await _packs()


@app.delete("/api/datapacks/{name}/world", dependencies=auth)
async def uninstall_datapack(name: str):
    if manager.is_alive():
        raise HTTPException(409, "Stop the server before removing a data pack from the world (or disable it)")
    await asyncio.to_thread(_datapack_errors, datapacks.uninstall, name)
    manager.info(f"Data pack {name} removed from the world.")
    return await _packs()


@app.delete("/api/datapacks/{name}/library", dependencies=auth)
async def delete_datapack(name: str):
    await asyncio.to_thread(_datapack_errors, datapacks.delete_from_library, name)
    manager.info(f"Data pack {name} deleted from the library.")
    return await _packs()


# ---------- server.properties ----------


@app.get("/api/properties", dependencies=auth)
async def get_properties():
    return {"values": properties.get_all(), "fields": {
        k: {"type": t, "default": d, "rule": r} for k, (t, d, r) in properties.FIELDS.items()}}


@app.post("/api/properties", dependencies=auth)
async def set_properties(values: dict):
    try:
        result = properties.update(values)
    except ValueError as e:
        raise HTTPException(400, str(e))
    manager.info("server.properties updated" + (" (applies after a restart)" if manager.is_alive() else ""))
    return {"values": result}


# ---------- health (no login; for Docker's HEALTHCHECK) ----------


@app.get("/api/health")
async def health():
    return {"ok": True}


@app.get("/api/backups", dependencies=auth)
async def get_backups():
    return backup.list_backups()


@app.get("/api/backups/{name}", dependencies=auth)
async def download_backup(name: str):
    try:
        return FileResponse(backup.backup_file(name), filename=name)
    except FileNotFoundError:
        raise HTTPException(404, "Backup not found")


@app.websocket("/ws")
async def websocket(ws: WebSocket):
    if not _valid(ws.cookies.get(COOKIE)):
        await ws.close(code=4401)
        return
    await ws.accept()
    q = manager.subscribe()
    try:
        while True:
            await ws.send_json(await q.get())
    except Exception:  # noqa: BLE001 - any send failure means the client is gone
        pass
    finally:
        manager.unsubscribe(q)


def _asset_version(name: str) -> str:
    """Short content hash, so a changed file gets a new URL (no stale browser cache)."""
    return hashlib.sha1((STATIC_DIR / name).read_bytes()).hexdigest()[:10]


@app.get("/")
async def index():
    html = (STATIC_DIR / "index.html").read_text(encoding="utf-8")
    for name in ("app.js", "style.css", "icons.svg"):
        html = html.replace(f"/static/{name}", f"/static/{name}?v={_asset_version(name)}")
    return HTMLResponse(html, headers={"Cache-Control": "no-cache"})


@app.middleware("http")
async def revalidate_static(request: Request, call_next):
    response = await call_next(request)
    if request.url.path.startswith("/static/"):
        # Browser may cache, but must check with the server (cheap 304) before reusing
        response.headers["Cache-Control"] = "no-cache"
    return response


app.mount("/static", StaticFiles(directory=STATIC_DIR), name="static")
