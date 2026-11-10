"""Public live map web server (separate port from the admin panel).

Serves the map viewer (app/static/map), the tiles rendered by `mapgen` (DATA_DIR/map) and the
live player positions. Nothing else: no panel API, no login. Works while Minecraft is stopped.

Every server has its own map at `/s/<id>/` (the whole viewer is served there too, so its relative
URLs need no prefix); `/` serves the first server (`main`), the viewer there moves on to the server
the browser showed last. `/servers.json` lists the servers
whose map is turned on, for the viewer's server switch.
"""

import asyncio
import base64
import gzip
import json
import threading
import time
from pathlib import Path

import uvicorn
from starlette.applications import Starlette
from starlette.exceptions import HTTPException
from starlette.middleware import Middleware
from starlette.requests import Request
from starlette.responses import HTMLResponse, RedirectResponse, Response
from starlette.routing import Route
from starlette.staticfiles import StaticFiles

from . import config, instances, leaderboard, livemap
from .instances import Instance

VIEWER_DIR = Path(__file__).parent / "static" / "map"

_PLACEHOLDER = """<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Live map</title>
<style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#1c1f26;color:#dde2ea;
font:15px system-ui,sans-serif;text-align:center;padding:16px}}p{{color:#959eae}}</style></head>
<body><div><h1>Live map</h1><p>{text}</p></div></body></html>"""

# 1×1 transparent PNG
_EMPTY_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=")

_enabled_cache: dict[str, tuple[float, bool]] = {}


def _map_enabled(inst: Instance) -> bool:
    """settings.json is re-read at most every 5 s (tile requests come in bursts)."""
    checked, value = _enabled_cache.get(inst.id, (-1e9, True))
    if time.monotonic() - checked > 5:
        value = config.load_settings(inst).map_enabled
        _enabled_cache[inst.id] = (time.monotonic(), value)
    return value


def _instance(request: Request) -> Instance:
    """The server of this URL (`/s/<id>/…`, else main); 404 for unknown ids."""
    sid = request.path_params.get("sid")
    if sid is None:
        return instances.MAIN
    try:
        return instances.get(instances.validate_id(sid))
    except (KeyError, ValueError):
        raise HTTPException(404)


def _off() -> Response:
    return HTMLResponse(_PLACEHOLDER.format(text="The live map is turned off."), status_code=503)


class GuardedFiles(StaticFiles):
    def __init__(self, *args, inst: Instance | None = None, no_cache: bool = False, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self.inst = inst or instances.MAIN
        self.no_cache = no_cache

    async def check_config(self) -> None:
        # The tile folder only exists after the first render
        if Path(self.directory).is_dir():
            await super().check_config()

    async def get_response(self, path: str, scope) -> Response:
        if not _map_enabled(self.inst):
            return _off()
        try:
            if not Path(self.directory).is_dir():
                raise HTTPException(404)
            response = await super().get_response(path, scope)
        except HTTPException as e:
            if e.status_code == 404 and self.no_cache and path.endswith(".png"):
                # Areas without generated chunks: an empty tile instead of a 404 in the browser console
                return Response(_EMPTY_PNG, media_type="image/png", headers={"Cache-Control": "no-cache"})
            raise
        # Tiles change with every render: always revalidate (cheap 304 via ETag)
        response.headers["Cache-Control"] = "no-cache" if self.no_cache else "public, max-age=300"
        return response


async def viewer(request: Request) -> Response:
    inst = _instance(request)
    if "path" not in request.path_params:
        return RedirectResponse(request.url.path + "/")
    files = GuardedFiles(directory=VIEWER_DIR, html=True, inst=inst)
    return await files.get_response(request.path_params["path"], request.scope)


async def tiles(request: Request) -> Response:
    inst = _instance(request)
    files = GuardedFiles(directory=livemap.out_dir(inst), check_dir=False, no_cache=True, inst=inst)
    return await files.get_response(request.path_params["path"], request.scope)


async def servers_json(request: Request) -> Response:
    """The servers whose map is on and rendered: [{id, name, path}] (path relative to /)."""
    out = []
    for e in instances.all_entries():
        inst = Instance(e["id"])
        if _map_enabled(inst) and livemap.available(inst):
            out.append({"id": inst.id, "name": e["name"], "path": f"s/{inst.id}/"})
    return Response(json.dumps({"servers": out}), media_type="application/json", headers={"Cache-Control": "no-cache"})


async def maps_json(request: Request) -> Response:
    inst = _instance(request)
    if not _map_enabled(inst):
        return _off()
    path = livemap.out_dir(inst) / "maps.json"
    if not path.exists():
        return Response('{"dimensions": []}', media_type="application/json", headers={"Cache-Control": "no-cache"})
    return Response(path.read_bytes(), media_type="application/json", headers={"Cache-Control": "no-cache"})


# Texture tables written by mapgen next to the tiles (only with block textures). The viewer asks
# for them with ?v=<stamp>, so they can be cached for long.
_ASSET_FILES = {"blocks.json": "application/json", "atlas.png": "image/png"}
# blocks.json is a few MB of repetitive JSON (one entry per block state): sent gzipped, compressed
# once per file version
_gzipped: dict[str, tuple[float, bytes]] = {}


async def asset_file(request: Request) -> Response:
    inst = _instance(request)
    if not _map_enabled(inst):
        return _off()
    name = request.url.path.rsplit("/", 1)[-1]
    if name not in _ASSET_FILES:
        return Response("Not found", status_code=404)
    path = livemap.out_dir(inst) / name
    if not path.exists():
        return Response("Not found", status_code=404)
    headers = {"Cache-Control": "public, max-age=86400", "Vary": "Accept-Encoding"}
    if name.endswith(".json") and "gzip" in request.headers.get("accept-encoding", ""):
        mtime = path.stat().st_mtime
        key = f"{inst.id}/{name}"
        cached = _gzipped.get(key)
        if not cached or cached[0] != mtime:
            body = await asyncio.to_thread(lambda: gzip.compress(path.read_bytes(), 6))
            cached = _gzipped[key] = (mtime, body)
        return Response(cached[1], media_type=_ASSET_FILES[name], headers={**headers, "Content-Encoding": "gzip"})
    return Response(path.read_bytes(), media_type=_ASSET_FILES[name], headers=headers)


async def skin_png(request: Request) -> Response:
    """A player's skin (downloaded from Mojang by the panel), for the heads on the map."""
    if not _map_enabled(_instance(request)):
        return _off()
    try:
        path = livemap.skin_path(request.path_params["uuid"])
    except ValueError:
        return Response("Not found", status_code=404)
    if not path.exists():
        return Response("Not found", status_code=404)
    return Response(path.read_bytes(), media_type="image/png", headers={"Cache-Control": "public, max-age=3600"})


async def players_json(request: Request) -> Response:
    inst = _instance(request)
    if not _map_enabled(inst):
        return _off()
    tracker = livemap.tracker_for(inst)
    tracker.touch()  # someone is watching: keep player tracking on
    body = json.dumps({"players": tracker.snapshot()})
    return Response(body, media_type="application/json", headers={"Cache-Control": "no-store"})


async def stats_json(request: Request) -> Response:
    """Tier list and records of the server's players (see leaderboard.py for what it contains)."""
    inst = _instance(request)
    if not _map_enabled(inst):
        return _off()
    body = json.dumps(await asyncio.to_thread(leaderboard.build, inst))
    return Response(body, media_type="application/json", headers={"Cache-Control": "no-cache"})


async def me_json(request: Request) -> Response:
    """Which player this visitor probably is (by the address of their last login); never an address."""
    inst = _instance(request)
    if not _map_enabled(inst):
        return _off()
    name = leaderboard.me(inst, request.client.host if request.client else None)
    return Response(json.dumps({"name": name}), media_type="application/json", headers={"Cache-Control": "no-store"})


async def _not_found(request: Request, exc) -> Response:
    return Response("Not found", status_code=404)


# Cross-origin isolation: lets the 3D viewer share decoded regions between its web workers
# (SharedArrayBuffer) instead of copying them. Everything the viewer loads is same-origin, so
# nothing needs a CORP opt-in. Browsers only honour it on https or localhost; elsewhere the viewer
# falls back to copies.
_ISOLATION = [(b"cross-origin-opener-policy", b"same-origin"), (b"cross-origin-embedder-policy", b"require-corp")]


class _Isolation:
    def __init__(self, app) -> None:
        self.app = app

    async def __call__(self, scope, receive, send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return

        async def send_with_headers(message) -> None:
            if message["type"] == "http.response.start":
                message["headers"] = list(message.get("headers", [])) + _ISOLATION
            await send(message)

        await self.app(scope, receive, send_with_headers)


def _routes(prefix: str) -> list[Route]:
    """The map of one server: main at "", the others at "/s/{sid}"."""
    return [
        Route(f"{prefix}/maps.json", maps_json),
        Route(f"{prefix}/players.json", players_json),
        Route(f"{prefix}/stats.json", stats_json),
        Route(f"{prefix}/me.json", me_json),
        Route(f"{prefix}/servers.json", servers_json),
        Route(f"{prefix}/skins/{{uuid}}.png", skin_png),
        Route(f"{prefix}/blocks.json", asset_file),
        Route(f"{prefix}/atlas.png", asset_file),
        Route(f"{prefix}/tiles/{{path:path}}", tiles),
    ]


app = Starlette(
    middleware=[Middleware(_Isolation)],
    routes=[
        *_routes("/s/{sid}"),
        Route("/s/{sid}", viewer),
        Route("/s/{sid}/{path:path}", viewer),
        *_routes(""),
        Route("/{path:path}", viewer),
    ],
    exception_handlers={404: _not_found},
)


class MapServer:
    """Runs in its own thread with its own event loop, so it doesn't touch the panel's
    signal handling and shuts down together with it."""

    def __init__(self) -> None:
        cfg = uvicorn.Config(app, host=config.WEB_HOST, port=config.MAP_PORT,
                             log_level="warning", access_log=False)
        self.server = uvicorn.Server(cfg)
        self.thread = threading.Thread(target=lambda: asyncio.run(self.server.serve()),
                                       name="map-server", daemon=True)

    def start(self) -> None:
        self.thread.start()

    def stop(self) -> None:
        self.server.should_exit = True
        self.thread.join(timeout=5)
