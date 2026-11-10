"""FastAPI web application: REST actions + WebSocket live feed.

Everything that concerns one server lives under `/api/servers/{sid}/...` (see instances.py);
login, version lists, downloaded jars and the server list itself are global.
"""

import asyncio
import hashlib
import hmac
import logging
import re
import secrets
import time
import urllib.error
from contextlib import asynccontextmanager
from pathlib import Path

import psutil
from fastapi import APIRouter, Depends, FastAPI, HTTPException, Request, Response, WebSocket
from fastapi.responses import FileResponse, HTMLResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

from . import (access, addons, backup, config, datapacks, gamerules, instances, java, livemap, players,
               properties, servericon, sessions, versions, world)
from .mapserver import MapServer
from .server import (BusyError, ServerManager, add_manager, get_manager, hub, load_managers, managers,
                     remove_manager, shutdown_all)

log = logging.getLogger("panel")
STATIC_DIR = Path(__file__).parent / "static"
COOKIE = "panel_session"

_password = config.PASSWORD


# ---------- auth (sessions survive restarts, see sessions.py) ----------


def _valid(token: str | None) -> bool:
    return bool(sessions.check(token))


def _set_cookie(response: Response, token: str) -> None:
    response.set_cookie(COOKIE, token, httponly=True, samesite="strict", max_age=sessions.TTL)


def require_auth(request: Request, response: Response) -> None:
    token = request.cookies.get(COOKIE)
    result = sessions.check(token)
    if not result:
        raise HTTPException(401, "Not logged in")
    if result == "renewed":
        _set_cookie(response, token)  # sliding expiry: keep the browser cookie alive too


def server(sid: str) -> ServerManager:
    """Path parameter -> manager of that server (404 if unknown)."""
    try:
        return get_manager(instances.validate_id(sid))
    except (KeyError, ValueError):
        raise HTTPException(404, "Unknown server") from None


def main_manager() -> ServerManager:
    return get_manager(instances.MAIN_ID)


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
    procs: dict[str, psutil.Process] = {}  # per server; cpu_percent() needs the previous object
    last_net = psutil.net_io_counters() if IN_CONTAINER else None
    last_time = time.monotonic()
    while True:
        await asyncio.sleep(2)
        if not hub.has_subscribers():
            # Nobody is watching: skip the work (keep the baseline fresh for the next sample)
            last_time = time.monotonic()
            if last_net is not None:
                last_net = psutil.net_io_counters()
            continue
        try:
            now = time.monotonic()
            elapsed, last_time = now - last_time, now
            shared = {"panel": _usage(panel), "net": None}
            if last_net is not None:
                net = psutil.net_io_counters()
                shared["net"] = {
                    "up": round((net.bytes_sent - last_net.bytes_sent) / elapsed / 1024, 1),
                    "down": round((net.bytes_recv - last_net.bytes_recv) / elapsed / 1024, 1),
                }
                last_net = net

            for sid, m in list(managers.items()):
                mc_stats = None
                pid = m.process.pid if m.is_alive() else None
                if pid is None:
                    procs.pop(sid, None)
                else:
                    try:
                        proc = procs.get(sid)
                        if proc is None or proc.pid != pid:
                            proc = procs[sid] = psutil.Process(pid)
                            proc.cpu_percent()
                        mc_stats = _usage(proc)
                        mc_stats["ram_max"] = m.running_max_ram_mb
                    except psutil.Error:
                        procs.pop(sid, None)
                tick = m.tick if m.tick and time.time() - m.tick["t"] < 30 else None
                m._emit({"type": "stats", "stats": {"minecraft": mc_stats, "uptime": m.status()["uptime"],
                                                    "tick": tick, **shared}})
        except Exception as e:  # noqa: BLE001
            log.warning("stats failed: %s", e)


HISTORY_INTERVAL = 30


async def _history_loop() -> None:
    """One point per running server every 30 s for the last hour (CPU averaged over the interval).
    Stopped servers cost nothing; TPS is only there while somebody had the panel open."""
    procs: dict[str, psutil.Process] = {}
    while True:
        await asyncio.sleep(HISTORY_INTERVAL)
        for sid, m in list(managers.items()):
            pid = m.process.pid if m.is_alive() else None
            if pid is None:
                procs.pop(sid, None)
                m.history.clear()
                continue
            try:
                proc = procs.get(sid)
                if proc is None or proc.pid != pid:
                    proc = procs[sid] = psutil.Process(pid)
                    proc.cpu_percent()
                    continue  # needs one interval as baseline
                usage = _usage(proc)
            except psutil.Error:
                procs.pop(sid, None)
                continue
            tick = m.tick if m.tick and time.time() - m.tick["t"] < HISTORY_INTERVAL + 5 else {}
            m.history.append({"t": int(time.time()), **usage, "players": len(m.players),
                              "tps": tick.get("tps"), "mspt": tick.get("mspt")})


async def _tick_loop() -> None:
    """TPS/MSPT every 10 s, only while somebody has the panel open (no console traffic otherwise)."""
    while True:
        await asyncio.sleep(10)
        if not hub.has_subscribers():
            continue
        for m in list(managers.values()):
            if m.state.value == "running" and not m.task:
                try:
                    await asyncio.to_thread(m.query_tick)
                except RuntimeError:
                    pass


async def _map_render_loop() -> None:
    """Keep the map current: once at startup, then every MAP_INTERVAL s while the server runs.
    mapgen only touches changed region files, so a check without changes costs milliseconds.
    (Minecraft writes chunks to disk on autosave, so that is how fresh the map can be.)
    Every server with the map turned on, one after the other."""
    first = True
    while True:
        for m in list(managers.values()):
            if m.map_enabled and (first or m.state.value == "running"):
                had_map = livemap.available(m.inst)
                await asyncio.to_thread(livemap.renderer_for(m.inst).render, m.info)
                if not had_map and livemap.available(m.inst):
                    m.emit_status()  # map link appears
        first = False
        await asyncio.sleep(config.MAP_INTERVAL)


async def _map_players_loop() -> None:
    """Live player positions, only while somebody has the map open (no idle console traffic)."""
    while True:
        await asyncio.sleep(2)
        for m in list(managers.values()):
            tracker = livemap.tracker_for(m.inst)
            if not (m.map_enabled and m.state.value == "running" and tracker.watching()):
                continue
            for cmd in tracker.queries(set(m.players)):
                try:
                    m.send_command(cmd)
                except RuntimeError:
                    break


async def _backup_schedule() -> None:
    """Automatic backups every BACKUP_INTERVAL_HOURS, only for running servers
    (a stopped world doesn't change, and extra copies would rotate out older good ones)."""
    while True:
        await asyncio.sleep(config.BACKUP_INTERVAL_HOURS * 3600)
        for m in list(managers.values()):
            if m.state.value != "running":
                continue
            try:
                m.info("Scheduled backup")
                m.make_backup()
            except BusyError:
                m.info("Scheduled backup skipped: another action is running.")


def _import_paper_caches() -> None:
    for m in list(managers.values()):
        versions.import_paper_cache(m.inst.server_dir, m.info)


@asynccontextmanager
async def lifespan(app: FastAPI):
    if not _password:
        raise RuntimeError("PANEL_PASSWORD is not set. Refusing to start without a panel password.")
    config.ensure_dirs()
    load_managers()
    hub.bind_loop(asyncio.get_running_loop())
    # Pick up Mojang jars Paper downloaded in earlier runs (needs network, so in background)
    asyncio.get_running_loop().run_in_executor(None, _import_paper_caches)
    tasks = [
        asyncio.create_task(_stats_loop()),
        asyncio.create_task(_map_render_loop()),
        asyncio.create_task(_map_players_loop()),
        asyncio.create_task(_history_loop()),
        asyncio.create_task(_tick_loop()),
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
    await asyncio.to_thread(shutdown_all)


app = FastAPI(title="Minecraft Server Panel", lifespan=lifespan)


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


# ---------- global routes ----------


@app.post("/api/login")
async def login(body: LoginBody, response: Response):
    if not hmac.compare_digest(body.password.encode(), _password.encode()):
        await asyncio.sleep(1)  # slow down guessing
        raise HTTPException(401, "Wrong password")
    _set_cookie(response, await asyncio.to_thread(sessions.create))
    return {"ok": True}


@app.post("/api/logout")
async def logout(request: Request, response: Response):
    await asyncio.to_thread(sessions.remove, request.cookies.get(COOKIE))
    response.delete_cookie(COOKIE)
    return {"ok": True}


auth = [Depends(require_auth)]


@app.get("/api/versions", dependencies=auth)
async def list_versions(flavor: str = "vanilla", snapshots: bool = False):
    try:
        versions.validate_flavor(flavor)
    except ValueError as e:
        raise HTTPException(400, str(e))
    try:
        return await asyncio.to_thread(versions.list_versions, flavor, snapshots)
    except (OSError, ValueError, KeyError) as e:
        source = {"vanilla": "Mojang", "paper": "PaperMC", "fabric": "Fabric"}[flavor]
        raise HTTPException(502, f"Could not load version list from {source}: {e}")


@app.delete("/api/versions/{flavor}/{version}", dependencies=auth)
async def delete_version(flavor: str, version: str):
    for m in list(managers.values()):
        if m.is_alive() and (m.running_flavor, m.running_version) == (flavor, version):
            raise HTTPException(409, "This server version is currently running")
        if m.task == "download":
            raise HTTPException(409, "A download is in progress")
    try:
        versions.delete(flavor, version, [m.inst.server_dir for m in list(managers.values())])
    except ValueError as e:
        raise HTTPException(400, str(e))
    except FileNotFoundError as e:
        raise HTTPException(404, str(e))
    return {"ok": True}


# ---------- server list ----------


def _server_summary(entry: dict) -> dict:
    m = get_manager(entry["id"])
    s = config.load_settings(m.inst)
    return {
        "id": entry["id"],
        "name": entry["name"],
        "state": m.state.value,
        "task": m.task,
        "players": len(m.players),
        "port": instances.game_port(m.inst),
        "flavor": s.flavor,
        "version": s.version,
        "has_map": m.has_map,
    }


@app.get("/api/servers", dependencies=auth)
async def list_servers():
    return await asyncio.to_thread(lambda: [_server_summary(e) for e in instances.all_entries()])


class NewServerBody(BaseModel):
    name: str
    flavor: str = "paper"
    version: str = ""
    min_ram: str = "1G"
    max_ram: str = "2G"


def _create_server(body: NewServerBody) -> dict:
    flavor = versions.validate_flavor(body.flavor)
    version = config.validate_version(body.version) if body.version else ""
    min_ram, max_ram = config.validate_ram(body.min_ram), config.validate_ram(body.max_ram)
    if config.ram_mb(min_ram) > config.ram_mb(max_ram):
        raise ValueError("Min RAM must not exceed max RAM")
    instances.validate_name(body.name)
    port = instances.free_port()
    inst = instances.create(body.name)
    access.set_property(inst, "server-port", str(port))
    s = config.Settings(flavor=flavor, version=version, min_ram=min_ram, max_ram=max_ram, map_enabled=False)
    config.save_settings(s, inst)
    m = add_manager(inst)
    m.info(f"Server created (game port {port}).")
    return _server_summary({"id": inst.id, "name": instances.name_of(inst)})


@app.post("/api/servers", dependencies=auth)
async def create_server(body: NewServerBody):
    try:
        return await asyncio.to_thread(_create_server, body)
    except (RuntimeError, ValueError) as e:
        raise HTTPException(400, str(e))


class RenameBody(BaseModel):
    name: str


@app.patch("/api/servers/{sid}", dependencies=auth)
async def rename_server(body: RenameBody, m: ServerManager = Depends(server)):
    try:
        instances.rename(m.inst, body.name)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return _server_summary({"id": m.inst.id, "name": instances.name_of(m.inst)})


@app.delete("/api/servers/{sid}", dependencies=auth)
async def delete_server(m: ServerManager = Depends(server)):
    if m.inst.is_main:
        raise HTTPException(400, "The first server can't be deleted")
    if m.is_alive() or m.task:
        raise HTTPException(409, "Stop the server first")
    remove_manager(m.inst.id)
    await asyncio.to_thread(instances.remove, m.inst)
    return {"ok": True}


# ---------- per-server routes ----------

srv = APIRouter(prefix="/api/servers/{sid}", dependencies=auth)


def _state(m: ServerManager) -> dict:
    s = config.load_settings(m.inst)
    if not s.world_version:
        # World not started through the panel yet (e.g. imported): ask level.dat
        s.world_version = world.saved_version(m.inst) or ""
    return {
        "server": {"id": m.inst.id, "name": instances.name_of(m.inst), "port": instances.game_port(m.inst),
                   "has_map": m.has_map},
        "status": m.status(),
        "settings": {"flavor": s.flavor, "version": s.version, "min_ram": s.min_ram, "max_ram": s.max_ram,
                     "map_enabled": s.map_enabled},
        "world": {
            "version": s.world_version or None,
            "flavor": s.world_flavor or None,
            "rank": versions.rank(s.world_version) if s.world_version else None,
        },
        "installed": versions.installed(),
        "java": java.runtimes(),
        "log": list(m.log),
        "chat": list(m.chat),
        "backups": backup.list_backups(m.inst),
        "backup_keep": config.BACKUP_KEEP,
        "backup_interval": config.BACKUP_INTERVAL_HOURS,
    }


@srv.get("/state")
async def state(m: ServerManager = Depends(server)):
    # May hit the network (version ranks) and spawn `java -version`: keep it off the event loop
    return await asyncio.to_thread(_state, m)


@srv.post("/start")
async def start(m: ServerManager = Depends(server)):
    return _busy(m.start)


@srv.post("/stop")
async def stop(body: StopBody, m: ServerManager = Depends(server)):
    return _busy(m.stop, max(0, min(body.countdown, 300)))


@srv.post("/restart")
async def restart(body: StopBody, m: ServerManager = Depends(server)):
    return _busy(m.restart, max(0, min(body.countdown, 300)))


@srv.post("/kill")
async def kill(m: ServerManager = Depends(server)):
    m.kill()
    return {"ok": True}


@srv.post("/cancel")
async def cancel(m: ServerManager = Depends(server)):
    m.cancel_countdown()
    return {"ok": True}


@srv.post("/command")
async def command(body: CommandBody, m: ServerManager = Depends(server)):
    cmd = body.command.strip().lstrip("/")
    if not cmd or "\n" in cmd or "\r" in cmd:
        raise HTTPException(400, "Invalid command")
    return _busy(m.send_command, cmd)


@srv.post("/eula")
async def eula(m: ServerManager = Depends(server)):
    m.accept_eula()
    return {"ok": True}


@srv.post("/versions/install")
async def install(body: VersionBody, m: ServerManager = Depends(server)):
    try:
        versions.jar_path(body.flavor, body.version)  # validates both
    except ValueError as e:
        raise HTTPException(400, str(e))
    return _busy(m.install, body.flavor, body.version)


@srv.post("/versions/{flavor}/{version}/update")
async def update_jar(flavor: str, version: str, m: ServerManager = Depends(server)):
    """Newest Paper build / Fabric loader for a downloaded server."""
    try:
        if flavor not in ("paper", "fabric"):
            raise ValueError("Only Paper and Fabric servers have updates")
        if not versions.jar_path(flavor, version).exists():  # validates the id
            raise HTTPException(404, f"{flavor.capitalize()} {version} is not downloaded")
    except ValueError as e:
        raise HTTPException(400, str(e))
    return _busy(m.update_jar, flavor, version)


@srv.post("/settings")
async def update_settings(body: SettingsBody, m: ServerManager = Depends(server)):
    s = config.load_settings(m.inst)
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
        if m.is_alive():
            raise HTTPException(409, "Stop the server before changing the version")
        try:
            new_flavor = versions.validate_flavor(new_flavor)
            s.version = config.validate_version(new_version) if new_version else ""
        except ValueError as e:
            raise HTTPException(400, str(e))
        if new_flavor != s.flavor:
            # Another server type changes the world layout / drops mod blocks: keep a way back
            try:
                await asyncio.to_thread(m.backup_before, f"before-{new_flavor}")
            except BusyError as e:
                raise HTTPException(409, str(e))
            except (OSError, RuntimeError) as e:
                raise HTTPException(400, f"Backup before switching failed: {e}")
            s.flavor = new_flavor
    if body.map_enabled is not None:
        s.map_enabled = body.map_enabled
        m.map_enabled = body.map_enabled  # public map page follows immediately
    config.save_settings(s, m.inst)
    m.emit_status()
    return {"ok": True}


# ---------- whitelist & operators ----------


class AccessBody(BaseModel):
    action: str  # "add" | "remove"
    name: str
    reason: str = ""  # bans only


class EnabledBody(BaseModel):
    enabled: bool


def _server_mode(m: ServerManager) -> str:
    """'online' = use server commands, 'offline' = edit files; refuse in between."""
    if not m.is_alive():
        return "offline"
    if m.state.value == "running":
        return "online"
    raise HTTPException(409, f"Server is {m.state.value}, try again in a moment")


@srv.get("/access")
async def get_access(m: ServerManager = Depends(server)):
    return access.state(m.inst)


async def _change(m: ServerManager, kind: str, body: AccessBody):
    if body.action not in ("add", "remove"):
        raise HTTPException(400, "action must be add or remove")
    try:
        name = access.validate_name(body.name)
        reason = access.validate_reason(body.reason)
    except ValueError as e:
        raise HTTPException(400, str(e))

    if _server_mode(m) == "online":
        file = access.FILES[kind]
        cmd = {("whitelist", "add"): "whitelist add", ("whitelist", "remove"): "whitelist remove",
               ("ops", "add"): "op", ("ops", "remove"): "deop",
               ("bans", "add"): "ban", ("bans", "remove"): "pardon"}[(kind, body.action)]
        line = f"{cmd} {name}" + (f" {reason}" if kind == "bans" and body.action == "add" and reason else "")
        before = access.mtime(m.inst, file)
        _busy(m.send_command, line)
        # The server answers in the console (e.g. unknown player); wait for its file update
        await asyncio.to_thread(access.wait_for_change, m.inst, file, before)
        return {"ok": True, "message": f"Sent: /{line}", **access.state(m.inst)}

    try:
        msg = await asyncio.to_thread(access.offline_change, m.inst, kind, body.action, name, reason)
    except LookupError as e:
        raise HTTPException(404, str(e))
    except OSError as e:
        raise HTTPException(502, f"Could not reach Mojang to look up {name}: {e}")
    m.info(msg)
    return {"ok": True, "message": msg, **access.state(m.inst)}


@srv.post("/access/whitelist")
async def change_whitelist(body: AccessBody, m: ServerManager = Depends(server)):
    return await _change(m, "whitelist", body)


@srv.post("/access/ops")
async def change_ops(body: AccessBody, m: ServerManager = Depends(server)):
    return await _change(m, "ops", body)


@srv.post("/access/bans")
async def change_bans(body: AccessBody, m: ServerManager = Depends(server)):
    return await _change(m, "bans", body)


# ---------- player profiles & actions ----------


@srv.get("/players/{name}")
async def player_profile(name: str, m: ServerManager = Depends(server)):
    try:
        name = access.validate_name(name)
    except ValueError as e:
        raise HTTPException(400, str(e))
    p = await asyncio.to_thread(players.profile, m.inst, name)
    p["online"] = any(n.lower() == name.lower() for n in m.players)
    return p


@srv.get("/players/{name}/skin")
async def player_skin(name: str, m: ServerManager = Depends(server)):
    """The player's skin PNG (from Mojang, cached); the browser cuts out the face. 404 = default head."""
    try:
        name = access.validate_name(name)
    except ValueError as e:
        raise HTTPException(400, str(e))
    path = await asyncio.to_thread(players.skin, m.inst, name)
    if not path or not path.exists():
        raise HTTPException(404, "No skin")
    return FileResponse(path, media_type="image/png", headers={"Cache-Control": "private, max-age=3600"})


class PlayerActionBody(BaseModel):
    action: str  # gamemode | spawn | heal | clear | kick
    value: str = ""  # game mode, or the kick reason


@srv.post("/players/{name}/action")
async def player_action(name: str, body: PlayerActionBody, m: ServerManager = Depends(server)):
    """Console commands for one player; built here, so nothing but a validated name goes in."""
    try:
        name = access.validate_name(name)
        reason = access.validate_reason(body.value) if body.action == "kick" else ""
    except ValueError as e:
        raise HTTPException(400, str(e))
    if m.state.value != "running":
        raise HTTPException(409, "The server must be running")
    if body.action == "gamemode":
        if body.value not in players.GAMEMODES:
            raise HTTPException(400, "Unknown game mode")
        cmds = [f"gamemode {body.value} {name}"]
    elif body.action == "spawn":
        spawn = await asyncio.to_thread(players.world_spawn, m.inst)
        if not spawn:
            raise HTTPException(400, "World spawn not found in level.dat")
        dim, (x, y, z) = spawn
        if not re.match(r"^[a-z0-9_.\-]+:[a-z0-9_./\-]+$", dim):
            raise HTTPException(400, "Unexpected spawn dimension")
        cmds = [f"execute in {dim} run tp {name} {x + 0.5} {y} {z + 0.5}"]
    elif body.action == "heal":
        # Instant health 10 fills any health bar, saturation fills hunger
        cmds = [f"effect give {name} minecraft:instant_health 1 10 true",
                f"effect give {name} minecraft:saturation 1 10 true"]
    elif body.action == "clear":
        cmds = [f"clear {name}"]
    elif body.action == "kick":
        cmds = [f"kick {name}" + (f" {reason}" if reason else "")]
    else:
        raise HTTPException(400, "Unknown action")
    for cmd in cmds:
        _busy(m.send_command, cmd)
    return {"ok": True, "message": f"Sent: /{cmds[0]}"}


@srv.post("/access/whitelist/enabled")
async def set_whitelist_enabled(body: EnabledBody, m: ServerManager = Depends(server)):
    if _server_mode(m) == "online":
        _busy(m.send_command, f"whitelist {'on' if body.enabled else 'off'}")
        await asyncio.sleep(0.5)  # server writes server.properties
    else:
        access.set_property(m.inst, "white-list", "true" if body.enabled else "false")
        m.info(f"Whitelist {'enabled' if body.enabled else 'disabled'} (applies on next start)")
    return {"ok": True, **access.state(m.inst)}


# ---------- backups & world ----------


@srv.post("/backups")
async def create_backup(m: ServerManager = Depends(server)):
    return _busy(m.make_backup)


@srv.get("/backups")
async def get_backups(m: ServerManager = Depends(server)):
    return backup.list_backups(m.inst)


@srv.post("/backups/upload")
async def upload_world(request: Request, m: ServerManager = Depends(server)):
    """A world as .zip (server world or single-player save) -> stored as a backup, then restorable."""
    tmp = await _receive(request, backup.MAX_UPLOAD)
    try:
        path = await asyncio.to_thread(backup.add_upload, m.inst, tmp)
    except (ValueError, OSError) as e:
        raise HTTPException(400, str(e))
    finally:
        tmp.unlink(missing_ok=True)
    m.info(f"World uploaded as {path.name}: restore it to use it.")
    return {"name": path.name, "backups": backup.list_backups(m.inst)}


@srv.get("/backups/{name}")
async def download_backup(name: str, m: ServerManager = Depends(server)):
    try:
        return FileResponse(backup.backup_file(m.inst, name), filename=name)
    except FileNotFoundError:
        raise HTTPException(404, "Backup not found")


@srv.post("/backups/{name}/restore")
async def restore_backup(name: str, m: ServerManager = Depends(server)):
    try:
        return _busy(m.restore, name)
    except FileNotFoundError:
        raise HTTPException(404, "Backup not found")


@srv.post("/map/render")
async def render_map(m: ServerManager = Depends(server)):
    if not livemap.find_binary():
        raise HTTPException(400, "Map renderer 'mapgen' not found (build it: cargo build --release in mapgen/)")
    return _busy(m.render_map)


# What the 3D live map shows below the surface (cave reach); see livemap.py
@app.get("/api/map/visibility", dependencies=auth)
async def get_map_visibility():
    return livemap.load_visibility()


@app.put("/api/map/visibility", dependencies=auth)
async def put_map_visibility(body: dict):
    try:
        v = livemap.save_visibility(body)
    except ValueError as e:
        raise HTTPException(400, str(e))
    # re-render now (mapgen sees the changed settings and redoes the 3D stacks); skipped while busy,
    # then the next scheduled render does it
    if livemap.find_binary():
        for m in list(managers.values()):
            if m.map_enabled:
                asyncio.get_running_loop().run_in_executor(None, livemap.renderer_for(m.inst).render, m.info)
    return v


class NewWorldBody(BaseModel):
    seed: str = ""
    datapacks: list[str] = []  # installed into the new world before it is generated


@srv.post("/world/new")
async def new_world(body: NewWorldBody, m: ServerManager = Depends(server)):
    return _busy(m.new_world, body.seed, body.datapacks)


# ---------- uploads ----------


async def _receive(request: Request, limit: int) -> Path:
    """Stream a raw request body to a temporary file in DATA_DIR (the caller deletes it)."""
    tmp = config.DATA_DIR / f".upload-{secrets.token_hex(8)}"
    try:
        with open(tmp, "wb") as f:
            size = 0
            async for chunk in request.stream():
                size += len(chunk)
                if size > limit:
                    raise HTTPException(413, f"File too large (max {limit // 2**20} MB)")
                f.write(chunk)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    return tmp


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


async def _apply_online(m: ServerManager, cmd: str, expect) -> bool:
    """Send a /datapack or /reload command, then save until level.dat shows the result."""
    if m.task:
        raise HTTPException(409, f"Another action is running: {m.task}")
    _busy(m.send_command, cmd)
    for _ in range(3):  # the reload behind the command runs asynchronously
        await asyncio.sleep(0.5)
        await asyncio.to_thread(m.save_and_wait, 30)
        st = await asyncio.to_thread(datapacks.state, m.inst)
        if st and expect(st):
            return True
    return False


async def _install(m: ServerManager, name: str) -> None:
    """Copy library -> world; while running, /reload so Minecraft picks it up."""
    mode = _server_mode(m)
    await asyncio.to_thread(_datapack_errors, datapacks.install, m.inst, name)
    if mode == "online":
        pid = datapacks.pack_id(name)
        # /reload discovers new packs and enables them (a pack disabled before stays disabled)
        ok = await _apply_online(m, "minecraft:reload", lambda st: pid in st[0] | st[1])
        m.info(f"Data pack {name} installed" + (" and loaded." if ok else ": see the console for errors."))
    else:
        m.info(f"Data pack {name} installed in the world, loaded on next start.")


def _packs(m: ServerManager):
    return asyncio.to_thread(datapacks.list_packs, m.inst)


@srv.get("/datapacks")
async def list_datapacks(m: ServerManager = Depends(server)):
    return await _packs(m)


@srv.post("/datapacks/upload")
async def upload_datapack(name: str, request: Request, m: ServerManager = Depends(server)):
    """Raw .zip body -> library, then installed into this server's world."""
    _server_mode(m)  # refuse early while starting/stopping
    _datapack_errors(datapacks.validate_name, name)
    tmp = await _receive(request, datapacks.MAX_UPLOAD)
    try:
        await asyncio.to_thread(_datapack_errors, datapacks.add_upload, name, tmp)
    finally:
        tmp.unlink(missing_ok=True)
    m.info(f"Data pack {name} added to the library.")
    await _install(m, name)
    return await _packs(m)


@srv.post("/datapacks/{name}/install")
async def install_datapack(name: str, m: ServerManager = Depends(server)):
    await _install(m, name)
    return await _packs(m)


@srv.post("/datapacks/{name}/adopt")
async def adopt_datapack(name: str, m: ServerManager = Depends(server)):
    await asyncio.to_thread(_datapack_errors, datapacks.adopt, m.inst, name)
    m.info(f"Data pack {name} copied to the library.")
    return await _packs(m)


class DatapackToggleBody(BaseModel):
    enabled: bool


@srv.post("/datapacks/{name}/enabled")
async def toggle_datapack(name: str, body: DatapackToggleBody, m: ServerManager = Depends(server)):
    _datapack_errors(datapacks.path_of, m.inst, name)
    if _server_mode(m) == "online":
        pid = datapacks.pack_id(name)
        verb = "enable" if body.enabled else "disable"
        ok = await _apply_online(m, f'minecraft:datapack {verb} "{pid}"',
                                 lambda st: (pid in st[0]) == body.enabled)
        if not ok:
            raise HTTPException(400, f"The server did not {verb} {name}: see the console")
        m.info(f"Data pack {name} {verb}d.")
    else:
        m.info(await asyncio.to_thread(_datapack_errors, datapacks.set_enabled_offline, m.inst, name, body.enabled))
    return await _packs(m)


@srv.delete("/datapacks/{name}/world")
async def uninstall_datapack(name: str, m: ServerManager = Depends(server)):
    if m.is_alive():
        raise HTTPException(409, "Stop the server before removing a data pack from the world (or disable it)")
    await asyncio.to_thread(_datapack_errors, datapacks.uninstall, m.inst, name)
    m.info(f"Data pack {name} removed from the world.")
    return await _packs(m)


@srv.delete("/datapacks/{name}/library")
async def delete_datapack(name: str, m: ServerManager = Depends(server)):
    await asyncio.to_thread(_datapack_errors, datapacks.delete_from_library, name)
    m.info(f"Data pack {name} deleted from the library.")
    return await _packs(m)


# ---------- mods (Fabric) & plugins (Paper) ----------


def _addon_kind(m: ServerManager) -> tuple[str, str]:
    """(kind, game version) of the server's selected type; 400 for Vanilla."""
    s = config.load_settings(m.inst)
    kind = addons.kind_of(s.flavor)
    if not kind:
        raise HTTPException(400, "Vanilla servers have no mods or plugins: switch to Paper or Fabric")
    return kind, s.version


def _addon_errors(fn, *args):
    try:
        return fn(*args)
    except (FileNotFoundError, LookupError) as e:
        raise HTTPException(404, str(e))
    except FileExistsError as e:
        raise HTTPException(409, str(e))
    except PermissionError as e:  # Windows: the running server keeps its jars open
        raise HTTPException(409, f"File in use, stop the server first ({e})")
    except urllib.error.HTTPError as e:
        raise HTTPException(502, f"Modrinth answered {e.code}")
    except (OSError, urllib.error.URLError) as e:
        raise HTTPException(502, f"Could not reach Modrinth: {e}")
    except (RuntimeError, ValueError) as e:
        raise HTTPException(400, str(e))


def _addon_list(m: ServerManager) -> dict:
    s = config.load_settings(m.inst)
    kind = addons.kind_of(s.flavor)
    return {"kind": kind, "flavor": s.flavor, "version": s.version, "running": m.is_alive(),
            "items": addons.list_addons(m.inst, kind, s.version) if kind else []}


def _addon_changed(m: ServerManager, text: str) -> None:
    m.info(text + (" Applies after a restart." if m.is_alive() else ""))


@srv.get("/addons")
async def list_addons(m: ServerManager = Depends(server)):
    return await asyncio.to_thread(_addon_list, m)


@srv.post("/addons/upload")
async def upload_addon(name: str, request: Request, m: ServerManager = Depends(server)):
    kind, _ = _addon_kind(m)
    _addon_errors(addons.validate_file, name)
    tmp = await _receive(request, addons.MAX_UPLOAD)
    try:
        await asyncio.to_thread(_addon_errors, addons.add_upload, m.inst, kind, name, tmp)
    finally:
        tmp.unlink(missing_ok=True)
    _addon_changed(m, f"{name} added to {kind}.")
    return await asyncio.to_thread(_addon_list, m)


@srv.post("/addons/{name}/enabled")
async def toggle_addon(name: str, body: EnabledBody, m: ServerManager = Depends(server)):
    kind, _ = _addon_kind(m)
    new = await asyncio.to_thread(_addon_errors, addons.set_enabled, m.inst, kind, name, body.enabled)
    _addon_changed(m, f"{new.removesuffix(addons.DISABLED)} {'enabled' if body.enabled else 'disabled'}.")
    return await asyncio.to_thread(_addon_list, m)


@srv.delete("/addons/{name}")
async def delete_addon(name: str, m: ServerManager = Depends(server)):
    kind, _ = _addon_kind(m)
    await asyncio.to_thread(_addon_errors, addons.delete, m.inst, kind, name)
    _addon_changed(m, f"{name} deleted.")
    return await asyncio.to_thread(_addon_list, m)


@srv.get("/addons/search")
async def search_addons(q: str = "", offset: int = 0, m: ServerManager = Depends(server)):
    kind, version = _addon_kind(m)
    return await asyncio.to_thread(_addon_errors, addons.search, kind, version, q, offset)


class AddonInstallBody(BaseModel):
    project_id: str


@srv.post("/addons/install")
async def install_addon(body: AddonInstallBody, m: ServerManager = Depends(server)):
    kind, version = _addon_kind(m)
    if not version:
        raise HTTPException(400, "Choose a Minecraft version first (Server page)")
    names = await asyncio.to_thread(_addon_errors, addons.install, m.inst, kind, version, body.project_id, m.info)
    if names:
        _addon_changed(m, f"Installed from Modrinth: {', '.join(names)}.")
    return {**await asyncio.to_thread(_addon_list, m), "installed": names}


@srv.get("/addons/updates")
async def addon_updates(m: ServerManager = Depends(server)):
    kind, version = _addon_kind(m)
    return await asyncio.to_thread(_addon_errors, addons.check_updates, m.inst, kind, version)


@srv.post("/addons/{name}/update")
async def update_addon(name: str, m: ServerManager = Depends(server)):
    kind, version = _addon_kind(m)
    new = await asyncio.to_thread(_addon_errors, addons.update, m.inst, kind, version, name, m.info)
    _addon_changed(m, f"{name} updated to {new}.")
    return await asyncio.to_thread(_addon_list, m)


# ---------- history, game rules ----------


@srv.get("/history")
async def history(m: ServerManager = Depends(server)):
    """Last hour of CPU/RAM/players/TPS (one point per 30 s while running)."""
    return {"interval": HISTORY_INTERVAL, "points": list(m.history)}


def _rules_online(m: ServerManager, names: list[str]) -> dict:
    """Ask the running server for every rule (answers are swallowed). Missing answers keep the file value."""
    found = {}
    for match in m.ask([f"gamerule {gamerules.command_name(n)}" for n in names], gamerules.ANSWER_RE,
                       expected=len(names), timeout=4):
        found[match.group(1)] = gamerules.parse_value(match.group(2))
    return found


@srv.get("/gamerules")
async def get_gamerules(m: ServerManager = Depends(server)):
    rules = await asyncio.to_thread(gamerules.read, m.inst)
    live = False
    if rules and m.state.value == "running" and not m.task:
        answers = await asyncio.to_thread(_rules_online, m, list(rules))
        for name in rules:
            value = answers.get(name, answers.get(gamerules.command_name(name)))
            if value is not None and type(value) is type(rules[name]):
                rules[name] = value
        live = bool(answers)
    return {"rules": gamerules.describe(rules), "live": live, "world": bool(rules)}


class GameruleBody(BaseModel):
    name: str
    value: bool | int


@srv.post("/gamerules")
async def set_gamerule(body: GameruleBody, m: ServerManager = Depends(server)):
    rules = await asyncio.to_thread(gamerules.read, m.inst)
    try:
        value = gamerules.validate(rules, body.name, body.value)
    except ValueError as e:
        raise HTTPException(400, str(e))
    if _server_mode(m) == "online":
        cmd = f"gamerule {gamerules.command_name(body.name)} {gamerules.text(value)}"
        answers = await asyncio.to_thread(m.ask, [cmd], gamerules.ANSWER_RE, 1, 3)
        m.info(f"Game rule {gamerules.command_name(body.name)} set to {gamerules.text(value)}."
               if answers else f"Sent /{cmd} (no confirmation from the server: see the console).")
    else:
        try:
            await asyncio.to_thread(gamerules.write_offline, m.inst, body.name, value)
        except (OSError, RuntimeError, ValueError) as e:
            raise HTTPException(400, str(e))
        m.info(f"Game rule {gamerules.command_name(body.name)} set to {gamerules.text(value)} (applies on next start).")
    return {"ok": True, "name": body.name, "value": value}


# ---------- server.properties ----------


@srv.get("/properties")
async def get_properties(m: ServerManager = Depends(server)):
    return {"values": properties.get_all(m.inst), "fields": {
        k: {"type": t, "default": d, "rule": r} for k, (t, d, r) in properties.FIELDS.items()}}


@srv.post("/properties")
async def set_properties(values: dict, m: ServerManager = Depends(server)):
    try:
        result = properties.update(m.inst, values)
    except ValueError as e:
        raise HTTPException(400, str(e))
    m.info("server.properties updated" + (" (applies after a restart)" if m.is_alive() else ""))
    return {"values": result}



# ---------- server list icon (server-icon.png) ----------


@srv.get("/icon")
async def get_icon(m: ServerManager = Depends(server)):
    """The icon Minecraft shows in the server list (the panel logo until one is uploaded)."""
    return Response(servericon.read(m.inst), media_type="image/png", headers={"Cache-Control": "no-store"})


@srv.put("/icon")
async def put_icon(request: Request, m: ServerManager = Depends(server)):
    """Raw PNG body, 64×64 (the page scales the user's image)."""
    if int(request.headers.get("content-length") or 0) > servericon.MAX_BYTES:
        raise HTTPException(413, "The icon is too big (at most 256 KB)")
    data = await request.body()
    try:
        await asyncio.to_thread(servericon.save, m.inst, data)
    except ValueError as e:
        raise HTTPException(400, str(e))
    m.info("Server icon changed" + (" (shows after a restart)" if m.is_alive() else ""))
    return {"ok": True}


@srv.delete("/icon")
async def reset_icon(m: ServerManager = Depends(server)):
    await asyncio.to_thread(servericon.reset, m.inst)
    m.info("Server icon reset to the panel logo" + (" (shows after a restart)" if m.is_alive() else ""))
    return {"ok": True}


app.include_router(srv)


# ---------- health (no login; for Docker's HEALTHCHECK) ----------


@app.get("/api/health")
async def health():
    return {"ok": True}


@app.websocket("/ws")
async def websocket(ws: WebSocket):
    if not _valid(ws.cookies.get(COOKIE)):
        await ws.close(code=4401)
        return
    await ws.accept()
    q = hub.subscribe()
    try:
        while True:
            await ws.send_json(await q.get())
    except Exception:  # noqa: BLE001 - any send failure means the client is gone
        pass
    finally:
        hub.unsubscribe(q)


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
