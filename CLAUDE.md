# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A web panel (FastAPI backend + plain HTML/JS/CSS frontend, no build step) that runs and controls a
Minecraft Java server (Vanilla or Paper) as a child process. Ships as a Docker image or runs directly
with Python 3.10+.

All code lives in `app/` (the old PyQt5 desktop version has been removed).

## Commands

```bash
pip install -r requirements.txt
PANEL_PASSWORD=secret python -m app       # PowerShell: $env:PANEL_PASSWORD="secret"; python -m app
# -> http://localhost:47318, data in ./data (override with DATA_DIR)

cp .env.example .env && docker compose up -d --build   # Docker; EXTRA_JAVA build arg adds Java runtimes
```

Dev container: `docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d` bind-mounts
`./app` read-only, so frontend edits are live without rebuild/restart (Python changes:
`docker compose restart`). Note `.env.example` sets `COMPOSE_FILE` to include the dev overlay, so a
plain `docker compose up` after copying it already runs in dev mode. Don't add auto-reload for
Python: a reload runs the lifespan shutdown, which stops the Minecraft server.

Env vars: `.env.example` lists the user-facing ones; `app/config.py` (and `MAP_TEXTURES` in
`app/livemap.py`) has the full set, e.g. `DATA_DIR`, `JAVA_DIR`, `MAPGEN_BIN`.

Map renderer: `cd mapgen && cargo test && cargo build --release` (the panel finds
`mapgen/target/release/mapgen[.exe]`, `mapgen` on PATH, or `MAPGEN_BIN`).

Tests: `pip install -r requirements-dev.txt && pytest`; single test:
`pytest tests/test_server_parsing.py -k <name>`, Rust: `cd mapgen && cargo test <name>` (pure functions + file handling, no
Minecraft/Java/network; the `data` fixture in `tests/conftest.py` redirects all data paths to a temp
dir). Starlette's `TestClient` needs an extra package, so API behaviour is not unit-tested. No linter
config or frontend build. The app refuses to start without `PANEL_PASSWORD` (old name `MCGUI_PASSWORD` is still read as a fallback). Running locally needs `java` on `PATH` (or runtimes under `JAVA_DIR/<major>/bin/`).
Static files are served with content-hash query strings, so frontend edits show up on reload
without restarting; Python changes need a restart (no `--reload` is configured).

## Architecture

**Threading model (the key thing to understand).** `app/server.py` holds a single global
`ServerManager` (`manager`). The Java process runs via `subprocess.Popen` with a dedicated reader
thread parsing stdout line by line. Long actions (start/stop/restart/backup/download) go through
`_exclusive()`, which takes a non-blocking lock and runs the action in a daemon thread. A second
concurrent action raises `BusyError`, which `main._busy()` maps to HTTP 409 (and `RuntimeError` /
`ValueError` to 400). Errors inside the thread are reported to the UI via `manager.info()` rather
than raised.

**Live feed.** Threads push events to WebSocket clients through `manager._emit()`, which uses
`loop.call_soon_threadsafe` onto per-client `asyncio.Queue`s (slow clients get events dropped).
Event types: `log`, `chat`, `status`, `stats` — handled in the `switch (msg.type)` in
`app/static/app.js`. The frontend loads the full snapshot from `GET /api/state` and then applies
WS events. `_stats_loop` in `main.py` skips work when there are no subscribers (keep it that way;
idle cost is a stated feature).

**Server state comes from parsing console output.** Regexes at the top of `server.py` detect
"Done" (→ RUNNING), joins/leaves, chat, `list` output, whitelist rejections, "Saved the game"
(used to synchronise live backups: `save-off` → `save-all flush` → wait → zip → `save-on`), and
EULA prompts. Changes to Minecraft's log format break these.

**Online vs. offline mutations.** Whitelist/ops changes (`app/access.py`, `_change` in `main.py`)
send server commands when the server is RUNNING, edit `whitelist.json` / `ops.json` /
`server.properties` directly when it is stopped, and refuse with 409 while starting/stopping.

**Configuration layering** (`app/config.py`): env vars are static defaults; anything changed in the
UI (flavor, version, RAM, map toggle) is persisted to `<DATA_DIR>/settings.json` and overrides
them. `settings.json` also records `world_version`/`world_flavor` (written when the server reaches
"Done") which drives the downgrade warning in the UI.

**Jars** (`app/versions.py`): stored as `<JARS_DIR>/<flavor>-<version>.jar` with a sidecar
`.json` holding required Java major, build and JVM flags. Vanilla comes from Mojang's version
manifest, Paper from the PaperMC Fill v3 API; downloads are hash-verified. `import_paper_cache`
hard-links the Mojang jar Paper downloads into `server/cache/` as a Vanilla install.
`java.pick()` chooses the smallest installed runtime ≥ the required major.

**Live map** — works for every server type, nothing runs inside Minecraft:
- `mapgen/` (Rust) renders the world files into `DATA_DIR/map/<dim>/<zoom>/<x>_<z>.png` (512 px =
  one region, zoom 0 = 1 block/px, negative zooms = 2×2 downscales), 16-bit height tiles in `h/` (for
  the 3D view), `state.json` (region mtimes → incremental) and `maps.json` (incl. spawn and, per dimension, the
  world border when smaller than default: 26.x `<dim>/data/minecraft/world_border.dat`). It discovers the
  26.x layout `world/dimensions/<ns>/<name>/region` as well as `region`/`DIM-1`/`DIM1` and old Paper
  sibling folders (`--extra`). Chunk parsing handles the 26.x palette forms (plain strings,
  `{"": …}`-wrapped entries and `{id, properties}` for non-default states) and must use `yPos` for the world bottom (the section list contains
  light-only sections). Colors: `colors.rs` (Minecraft map colors + block-name rules), used when
  there are no block textures.
  **Block textures** (`assets.rs`, `--client <jar>`): reads blockstates → models → top/side texture
  of every block (26.x texture variables may be `{"sprite": …}` objects) and its `kind` (cube /
  cross = plants / vine / flat = rails etc. / none = torches, entities), tint per face, grass side
  overlay, `transparent`; biome colours from `data/minecraft/worldgen/biome` + colormaps. Writes
  `blocks.json` (block + biome registry; ids are stable — append only — since the tiles store them)
  and `atlas.png` (16 px textures with real alpha). Rebuilt when the jar or `FORMAT` changes; a
  change of tables re-renders everything (`state.json` `assets`). Per region it then also writes
  `b/<x>_<z>.png` (top box: block id R,G + biome B) and `v/<x>_<z>.bin` (zlib "MGV1": u8 count per
  column + i16 y / u16 id entries = the block stack from the top down to the lowest solid ground of
  the column and its neighbours, always ending on a solid block so caves/overhangs have a floor; of
  a water run only the surface; where a neighbour shows air (cave, overhang) a stack goes down
  to that neighbour's lowest visible air (fixed point over per-column air runs, spreads only
  through air, `CAVE_REACH` columns), computed on a grid padded with the adjacent chunks of the 4 neighbour regions,
  so cave walls are real blocks, also across region borders). Rails are registered per `shape`
  state (`minecraft:rail[shape=east_west]`, `rot`/`slope`; `Assets::block_state_id`). Boxes keep their model size (`box`, each face taken from the
  element that owns it: cactus, slabs, carpets). The 2D colour tiles use
  texture average × biome tint.
  `cargo test` covers parsing, colors, shading, tiles, model/biome resolution.
- `app/livemap.py` runs mapgen as a low-priority subprocess (`Renderer`, one at a time) and tracks
  player positions (`PlayerTracker`): `data get entity <name> Pos/Dimension` is sent only while the
  map was requested in the last 30 s, and the answers are swallowed in `ServerManager._handle_line`
  (only for names we just queried). `main._map_render_loop` renders at startup and every
  `MAP_INTERVAL` s while running; the server exit and "Render now" (`save-all flush` first) trigger too.
  `client_jar()` downloads the Minecraft client of the world's version once from Mojang
  (SHA-1 verified, `DATA_DIR/cache/client-<version>.jar`, only kept locally, never shipped) and
  passes it as `--client`; `MAP_TEXTURES=false` or a failed download (retried after 1 h) = map colours.
- Viewer `app/static/map`: `map.js` (Leaflet 2D) and `map3d.js` (three.js r170, loaded only when
  switching to 3D; import map in `index.html`). `map3d-worker.js` (one instance for 2D, one for 3D;
  gets the page URL as `base` in `init`) decodes the 16-bit height tiles (own PNG decoder, the canvas
  API only gives 8 bits) and the `b/` block tiles + atlas, or without textures the colour tiles
  (shading turned back into base colours; `PALETTE` must match `colors.rs` BASE, a cargo test
  checks it). 3D near (step 1): the block stacks as real blocks — boxes with culled faces (fancy
  leaves: leaf faces are never culled), crossed planes for plants, vines on solid neighbours,
  translucent water surface with the bed darkened/tinted by depth; three materials per 128-block
  tile (opaque, cutout with alphaTest + polygonOffset, water); positions are Int16 in 1/16 block.
  Air gaps next to a neighbour that is solid below its stack get that neighbour's wall (cave
  walls); between two leaf blocks only one face. A pool of up to 3 workers builds the meshes (every
  worker decodes every region); step changes have a ±10 % hysteresis. Farther: coloured height
  boxes, 2–16 blocks. Biome colours are blended over 5×5 columns. Towards a coarser neighbour tile
  (the build message's `fine[n,s,w,e]` says which are step 1 too; tiles turn `dirty` when that
  flips) step-1 tiles add "just in case" faces / fill walls down to `coverMin` (heights at the
  step-aligned columns), ending 1/16 below the top (else z-fighting lines); top faces overlap tile
  edges by 1/16 (T-junction cracks); coarse tiles use a 16-block band minimum. The atlas is
  uploaded with 32 px cells (8 px border, `atlasTexture`; worker `uvSub` must match) so linear
  mipmaps + anisotropy work without bleeding. The camera stays ≥2 blocks above the terrain. 2D: from Leaflet zoom 2 (4 px/block) a GridLayer draws textured tiles in the worker over
  the colour tiles. Tiles are re-fetched with
  ETag after a render. Headless `--screenshot` doesn't wait for workers: test via DevTools protocol.
- `app/mapserver.py` serves viewer (Leaflet and three.js vendored), tiles, `maps.json`,
  `blocks.json`/`atlas.png` and `players.json` on `MAP_PORT` (separate uvicorn server in its own thread). The map port must never
  expose anything else.
- The old squaremap plugin is deleted on server start (`remove_squaremap`).

**Other modules:** `datapacks.py` (library `<DATA_DIR>/datapacks/` = source of truth, copies
installed into `<world>/datapacks/`; enabled state lives per world in level.dat
`Data.DataPacks.Enabled/Disabled` — online via `minecraft:datapack` + `save-all` until level.dat shows
it, offline by editing level.dat with `nbt.py`, a byte-exact NBT round-trip codec; "New world"
installs the selected packs before generation), `properties.py` (whitelisted, validated subset of `server.properties` for the
game settings dialog), `world.py` (minimal NBT reader: `level.dat` → `Data.Version.Name`, used when
`settings.json` has no `world_version`), `backup.py` (`restore()` validates zip member paths, makes a
`_before-restore` safety backup and protects the restored file from retention cleanup).
`archive_*.zip` files (written by "New world", `ServerManager._new_world`) must never be deleted:
`cleanup()` only globs `backup_*.zip` — keep it that way.

**Security-relevant invariants:**
- User-supplied flavor/version ids are validated (`validate_flavor`, `config.validate_version`)
  before being used in file paths — keep that for any new path built from request data.
- Console commands are rejected if they contain newlines (stdin injection).
- Sessions are in-memory (lost on restart), cookie `panel_session`, httponly + samesite=strict.

## Docker notes

The image is based on `eclipse-temurin:25-jre-noble`; extra Java runtimes from the `EXTRA_JAVA`
build arg are installed to `/opt/java/<major>` by `docker/install-java.py`. The entrypoint fixes
mount ownership as root then drops to uid 1000 (`minecraft`). `stop_grace_period: 90s` gives the
server time to save; `manager.shutdown()` sends `stop` on app exit. `HEALTHCHECK` polls the
unauthenticated `GET /api/health`. Ports: 47318 panel, 47319 live map, 25565 Minecraft.

## Project conventions

- Backlogs are split by area so two Claude sessions can work in parallel: `TODO_DASHBOARD.md` /
  `DEV_TODO_DASHBOARD.md` (panel) and `TODO_LIVEMAP.md` / `DEV_TODO_LIVEMAP.md` (`mapgen/`,
  `livemap.py`, `mapserver.py`, `static/map/`). `TODO_*` = code tasks, `DEV_TODO_*` = decisions
  needing the developer. Work only on your area's list; each file names the paths it owns. Shared
  files (`main.py`, `server.py`, `config.py`, Docker files, README, this file) may be edited by the
  other session at the same time: re-read right before editing and keep changes small.
  The backlogs are written in German; code, comments and README are in English.
- Never read `.env` (denied in `.claude/settings.json`); use `.env.example` for the variable list.
- Git write operations (commit, add, push, branch, etc.) are denied for Claude in
  `.claude/settings.json`; leave version control to the user.
