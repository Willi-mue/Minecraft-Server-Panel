"""Public live map web server (separate port from the admin panel).

Serves the map viewer (app/static/map), the tiles rendered by `mapgen` (DATA_DIR/map) and the
live player positions. Nothing else: no panel API, no login. Works while Minecraft is stopped.
"""

import asyncio
import base64
import json
import threading
import time
from pathlib import Path

import uvicorn
from starlette.applications import Starlette
from starlette.exceptions import HTTPException
from starlette.requests import Request
from starlette.responses import HTMLResponse, Response
from starlette.routing import Mount, Route
from starlette.staticfiles import StaticFiles

from . import config, livemap

VIEWER_DIR = Path(__file__).parent / "static" / "map"

_PLACEHOLDER = """<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>Live map</title>
<style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#1c1f26;color:#dde2ea;
font:15px system-ui,sans-serif;text-align:center;padding:16px}}p{{color:#959eae}}</style></head>
<body><div><h1>Live map</h1><p>{text}</p></div></body></html>"""

# 1×1 transparent PNG
_EMPTY_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=")

_enabled_cache = (0.0, True)


def _map_enabled() -> bool:
    """settings.json is re-read at most every 5 s (tile requests come in bursts)."""
    global _enabled_cache
    checked, value = _enabled_cache
    if time.monotonic() - checked > 5:
        value = config.load_settings().map_enabled
        _enabled_cache = (time.monotonic(), value)
    return value


def _off() -> Response:
    return HTMLResponse(_PLACEHOLDER.format(text="The live map is turned off."), status_code=503)


class GuardedFiles(StaticFiles):
    def __init__(self, *args, no_cache: bool = False, **kwargs) -> None:
        super().__init__(*args, **kwargs)
        self.no_cache = no_cache

    async def check_config(self) -> None:
        # The tile folder only exists after the first render
        if Path(self.directory).is_dir():
            await super().check_config()

    async def get_response(self, path: str, scope) -> Response:
        if not _map_enabled():
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


async def maps_json(request: Request) -> Response:
    if not _map_enabled():
        return _off()
    path = livemap.out_dir() / "maps.json"
    if not path.exists():
        return Response('{"dimensions": []}', media_type="application/json", headers={"Cache-Control": "no-cache"})
    return Response(path.read_bytes(), media_type="application/json", headers={"Cache-Control": "no-cache"})


# Texture tables written by mapgen next to the tiles (only with block textures). The viewer asks
# for them with ?v=<stamp>, so they can be cached for long.
_ASSET_FILES = {"blocks.json": "application/json", "atlas.png": "image/png"}


async def asset_file(request: Request) -> Response:
    if not _map_enabled():
        return _off()
    name = request.url.path.lstrip("/")
    path = livemap.out_dir() / name
    if name not in _ASSET_FILES or not path.exists():
        return Response("Not found", status_code=404)
    return Response(path.read_bytes(), media_type=_ASSET_FILES[name],
                    headers={"Cache-Control": "public, max-age=86400"})


async def players_json(request: Request) -> Response:
    if not _map_enabled():
        return _off()
    livemap.players.touch()  # someone is watching: keep player tracking on
    body = json.dumps({"players": livemap.players.snapshot()})
    return Response(body, media_type="application/json", headers={"Cache-Control": "no-store"})


async def _not_found(request: Request, exc) -> Response:
    return Response("Not found", status_code=404)


app = Starlette(
    routes=[
        Route("/maps.json", maps_json),
        Route("/players.json", players_json),
        Route("/blocks.json", asset_file),
        Route("/atlas.png", asset_file),
        Mount("/tiles", GuardedFiles(directory=livemap.out_dir(), check_dir=False, no_cache=True)),
        Mount("/", GuardedFiles(directory=VIEWER_DIR, html=True)),
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
