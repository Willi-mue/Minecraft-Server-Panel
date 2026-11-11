# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A web panel (Rust backend + plain HTML/JS/CSS frontend, no build step) that runs and controls
Minecraft Java servers (Vanilla, Paper, Fabric) as child processes. Ships as a Docker image or runs
directly as one binary.

- `panel/` — the backend (Rust, axum + tokio; replaced the former Python/FastAPI backend, same API
  paths, JSON shapes and file formats). `api.rs` = all panel routes, `tasks.rs` = background loops,
  `main.rs` = startup/shutdown, `skins.rs` = skin downloads, `statics.rs` = file serving with ETag;
  the other modules are named after what they do (`server.rs`, `instances.rs`, `backup.rs`, ...).
- `app/static/` — the frontend (panel + map viewer), served by the Rust binary (`STATIC_DIR`).

## Commands

```bash
cd panel && cargo build --release
PANEL_PASSWORD=secret panel/target/release/panel   # PowerShell: $env:PANEL_PASSWORD="secret"; panel\target\release\panel.exe
# -> http://localhost:47318, data in ./data (override with DATA_DIR); finds app/static by itself
cd panel && cargo test                              # single test: cargo test <name>

cp .env.example .env && docker compose up -d --build   # Docker; EXTRA_JAVA build arg adds Java runtimes
```

Dev container: `docker compose -f docker-compose.yml -f docker-compose.dev.yml up -d` bind-mounts
`./app` read-only, so frontend edits are live without rebuild/restart (backend changes need
`--build`). Note `.env.example` sets `COMPOSE_FILE` to include the dev overlay, so a plain
`docker compose up` after copying it already runs in dev mode. `panel health` is the container's
HEALTHCHECK (no curl/Python in the image).

Env vars: `.env.example` lists the user-facing ones; `panel/src/config.rs` has the full set, e.g.
`DATA_DIR`, `JAVA_DIR`, `MAPGEN_BIN`, `MAP_TEXTURES`, `STATIC_DIR`.

Map renderer: `cd mapgen && cargo test && cargo build --release` (the panel finds
`mapgen/target/release/mapgen[.exe]`, `mapgen` on PATH, or `MAPGEN_BIN`).

Tests: `cd panel && cargo test` (pure functions + file handling, no Minecraft/Java; a few touch the
network and tolerate being offline). `testutil::data_dir()` points all data paths at a temp dir and
serialises those tests (the configuration is global). Rust map renderer: `cd mapgen && cargo test`.
No linter config or frontend build. The app refuses to start without `PANEL_PASSWORD` (old name
`MCGUI_PASSWORD` is still read as a fallback). Running locally needs `java` on `PATH` (or runtimes
under `JAVA_DIR/<major>/bin/`). Static files are served with content-hash query strings, so
frontend edits show up on reload without restarting.

## Architecture

**Several servers (instances).** `panel/src/instances.rs`: the panel runs any number of independent
servers. `Instance(id)` only computes paths (`server_dir`, `backup_dir`, `settings_file`, `map_dir`);
the first server is `main` and keeps the old paths (`DATA_DIR/server`, `BACKUP_DIR`,
`DATA_DIR/settings.json`), others live in `DATA_DIR/servers/<id>/` with backups in
`BACKUP_DIR/<id>/`; names in `DATA_DIR/servers.json`. Jars, Java and the data pack library are
shared (`versions._download_lock` serialises downloads). Every module function that touches server
files takes the instance as first parameter (`backup.*`, `access.*`, `properties.*`,
`world.*`, `datapacks.*` except the library) — pass it explicitly, never fall back to a global;
only `config.load_settings()`, `backup.level_name()` and the `livemap.*` functions default to `main`
(None = main). Every server has its own live map (`Instance.map_dir`, `livemap.renderer_for(inst)` /
`tracker_for(inst)`; one mapgen process at a time over all servers). Per-server API
routes live on the `srv` router under `/api/servers/{sid}/…` (dependency `server()` → manager);
global: login, `/api/servers` (list/create/rename/delete), version list, jar deletion. Each
additional server gets a free `server-port` from `SERVER_PORTS`; the user can change it while
stopped (`PUT …/port`, `instances::set_port`: 1024–65535, unique, not the panel's ports);
`_check_port()` refuses to start two running servers on one port. Confirmations use the panel's
own `ask()` dialog in app.js (optional text field), never the browser's `confirm()`/`prompt()`.

**Threading model (the key thing to understand).** `panel/src/server.rs` holds one `ServerManager` per
instance in `managers` (`get_manager(id)`). The Java process runs via `subprocess.Popen` with a dedicated reader
thread parsing stdout line by line. Long actions (start/stop/restart/backup/download) go through
`_exclusive()`, which takes a non-blocking lock and runs the action in a daemon thread. A second
concurrent action raises `BusyError`, which `api::ApiError` maps to HTTP 409 (and `Error::Invalid` to 400). Errors inside the thread are reported to the UI via `m.info()` rather
than raised.

**Live feed.** Threads push events to WebSocket clients through `ServerManager._emit()` → the shared
`hub` (`EventHub`), which tags every event with `"server": id` and uses
`loop.call_soon_threadsafe` onto per-client `asyncio.Queue`s (slow clients get events dropped).
Event types: `log`, `chat`, `status`, `stats` — handled in the `switch (msg.type)` in
`app/static/app.js` (events of other servers only update their status dot in the sidebar). The
frontend is a sidebar + pages layout with hash routing `#/<server>/<page>` (`route()`,
`switchServer()`); per-server URLs are built with `S(path)`, and async loads drop their result if
the selected server changed meanwhile. It loads the full snapshot from
`GET /api/servers/<id>/state` and then applies WS events. `stats_loop` in `tasks.rs` skips work when there are no subscribers (keep it that way;
idle cost is a stated feature).

**Server state comes from parsing console output.** Regexes at the top of `server.rs` detect
"Done" (→ RUNNING), joins/leaves, chat, `list` output, whitelist rejections, "Saved the game"
(used to synchronise live backups: `save-off` → `save-all flush` → wait → zip → `save-on`), and
EULA prompts. Changes to Minecraft's log format break these. Joins/leaves are also written to
`players.json` (first/last seen, `players.record`). The panel's own queries go through
`ServerManager.ask(commands, pattern, expected, timeout)`: matching answer lines are swallowed
(`_consume_query`, checked first in `_handle_line`) — used for TPS (`query_tick`: Paper `tps` +
`mspt`, else `tick query` on 1.20.3+) and game rules (`gamerule <name>`). `_tick_loop` asks every 10 s
only while a WebSocket is connected; `_history_loop` keeps one point per 30 s (CPU/RAM/players/TPS,
`ServerManager.history`, 1 h) only for running servers.

**Online vs. offline mutations.** Whitelist/ops/ban changes (`panel/src/access.rs`, `change` in `api.rs`)
send server commands when the server is RUNNING, edit `whitelist.json` / `ops.json` /
`banned-players.json` / `server.properties` directly when it is stopped, and refuse with 409 while
starting/stopping. Game rules work the same way (`gamerules.rs`, `/api/servers/{sid}/gamerules`).
Player actions (`POST …/players/{name}/action`: gamemode, spawn, heal, clear, kick) build the
console command server-side from a validated name — never pass client text into a command.

**Configuration layering** (`panel/src/config.rs`): env vars are static defaults; anything changed in the
UI (flavor, version, RAM, map toggle) is persisted to the server's `settings.json` and overrides
them. `settings.json` also records `world_version`/`world_flavor` (written when the server reaches
"Done") which drives the downgrade warning in the UI.

**Jars** (`panel/src/versions.rs`): stored as `<JARS_DIR>/<flavor>-<version>.jar` with a sidecar
`.json` holding required Java major, build and JVM flags. Vanilla comes from Mojang's version
manifest, Paper from the PaperMC Fill v3 API (hash-verified), Fabric from meta.fabricmc.net (the
server *launcher* jar: no checksum, so size + valid jar are checked; `build` = loader version; it
downloads Vanilla + libraries into the server folder on first start). `versions.update()` = newest
Paper build / Fabric loader. Changing the flavor in the settings makes a `_before-<flavor>` backup
first (`ServerManager.backup_before`). `import_paper_cache`
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
  and `atlas.png` (16 px textures with real alpha). Rebuilt when the jar or `FORMAT` changes (or blocks.json is missing); the stamp (`Assets::stamp`, maps.json `blocks`, viewers cache by it) includes a checksum of blocks.json, so new ids always mean a new stamp; a
  change of tables re-renders everything (`state.json` `assets`). Per region it then also writes
  `b/<x>_<z>.png` (top box: block id R,G + biome B), `p/<x>_<z>.png` (plant / flat block over it,
  id R,G; drawn over the ground in the textured 2D view) and `v/<x>_<z>.bin` (zlib "MGV2": u8 count per
  column + i16 y / u16 id entries, top down). It holds only what the map may show: air runs open to
  the sky (`open_runs`: runs touching the open space above a column's top or a lower neighbour's,
  then BFS through overlapping runs of neighbour columns up to `--cave-reach` columns, default 32)
  and the solid blocks next to that air (`Chunk::visible_stack`). Hidden rock/caves become `FILL` runs
  (0xFFFF: solid, not shown, down to the next entry), visible air after FILL starts with `AIR_MARK`
  (0xFFFE), gaps are air, below a water entry water, below the last entry solid (air when the last entry is `AIR_MARK`:
  the End's void; runs reaching the world bottom and void columns count as open); so the files do not
  give away hidden caves. `--all-caves` writes the same stacks with every air run shown to `c/`
  (same pass; maps.json `caves`); the viewer loads them only while the height slice is on.
  Computed on a grid padded with 2 chunks of the 4 neighbour regions; a
  change of `VOXEL_FORMAT`/`VOXEL_RULES`, the reach or all-caves re-renders (`state.json` `visibility`). The panel
  keeps reach + `all_caves` in `DATA_DIR/map_visibility.json` (`livemap.load_visibility`,
  `GET/PUT /api/map/visibility`; not in `DATA_DIR/map/`, which is public). Rails are registered per `shape`
  state (`minecraft:rail[shape=east_west]`, `rot`/`slope`; `Assets::block_state_id`). Boxes keep their model size (`box`, each face taken from the
  element that owns it: cactus, slabs, carpets). Detailed blocks (`detailed()`: stairs, fences, walls,
  panes, bars, doors, trapdoors, gates, torches, lanterns, chains, rods, beds, signs) are registered per state (multipart
  = every combination of the tested properties) and point to a shared `shapes` entry: boxes with per-face
  UV + texture slot (0 top, 1 side, 2 bottom, 3… the block's `extra` textures; `model_parts`, tilted boxes are
  moved, not tilted; tall ones leaning sideways like wall torches are cut into pieces moved one by one);
  missing state properties fall back to `default_value` (= the block's default state, which 26.x
  palettes leave out; checked against the server's `--reports` blocks.json: banners/signs rotation 8,
  chests single, beds foot). Blocks the game draws as entities have no
  model in the jar: `entity_models` builds boxes per state (chests, mob/dragon heads, shulker boxes,
  banners in their dye colour, conduit, decorated pot per facing, copper golem statues in their
  4 poses, approximated by turned limbs; the conduit has a made-up `active=true` state = cage + eye,
  which `render.rs` sets when `conduit_active` finds the game's frame: 26 water neighbours, ≥ 16 of
  the 42 ring positions prismarine / sea lantern, neighbour-region chunks loaded for it) and `entity_extras` adds the bell to its jar frame, with textures cut out of the entity
  textures (`crop|<texture>|<base width>|x,y,w,h[,v];…[|*rrggbb]` texture names, `*` = tint).
  Parts are i16: entity models (`Applied.free`) may reach out of their block (standing banners up,
  wall banners down, dragon snouts), jar models are cut to 0..16. blocks.json is served gzipped. The 2D colour tiles use
  texture average × biome tint.
  Player heads with a skin of their own: mapgen reads the skull block entities (`profile` /
  `SkullOwner`, base64 `textures` → texture hash, `chunk::skin_hash`), keeps those the normal stacks
  show (a head in a hidden cave stays hidden) and writes `k/<x>_<z>.json` per region, merged into
  `<dim>/heads.json` (`{x, y, z, skin, rot, wall}`). The panel downloads the skins once from
  `textures.minecraft.net/texture/<hash>` (`livemap.fetch_head_skins` after each render, PNG ≤ 64 KB,
  `DATA_DIR/cache/heads/`), served at `heads/<hash>.png`; the 3D viewer puts a box with the skin
  (`player3d.headModel`) over the block model's Steve head within 160 blocks of the camera.
  Banner patterns and pot sherds the same way: `chunk::Decor` from the block entities (`patterns`
  incl. the pre-1.20.5 `Patterns` codes, `sherds` as 26.x map of item stacks or old list), kept
  when the stacks show the block, `d/<x>_<z>.json` -> `<dim>/decor.json` (`{x, y, z, rot, wall,
  banner: [[front, back, "rrggbb"]…] | pot: [back, left, right, front]}`, atlas indices; blocks.json
  `patterns` / `sherds` map ids to them, pattern textures are white masks of the flag). The viewer
  (`placeDecor` in map3d.js) draws them as quads with the cutout material over the flag faces / pot
  sides within 160 blocks (one mesh). Pot front faces the placing player (rot = facing + 180);
  left/right are as seen from the front.
  `cargo test` covers parsing, colors, shading, tiles, model/biome resolution.
- `panel/src/livemap.rs` runs mapgen as a low-priority subprocess (`Renderer`, one at a time) and tracks
  player positions (`PlayerTracker`): `data get entity <name> Pos/Dimension` is sent only while the
  map was requested in the last 30 s, and the answers are swallowed in `ServerManager._handle_line`
  (only for names we just queried). `tasks::map_render_loop` renders at startup and every
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
  Air gaps next to a neighbour that is solid there (FILL / below its stack: a cave beyond the reach)
  get a plain rock wall, gaps ending on FILL a floor/ceiling; between two leaf blocks only one face. A pool of up to 3 workers builds the meshes; one
  worker decodes a region and the others `adopt` it (SharedArrayBuffer when the page is
  cross-origin isolated, else a copy). Step = `stepFor(distance / (view height / 900, clamped 1..2))`, doubled
  for tiles outside the view frustum (built last) except within `NEAR_FULL` blocks; while flying the
  distance is also measured from where the camera is `FLY_AHEAD` s later; ±10 % hysteresis; from step 4 a whole region is
  one mesh (`r.whole`, the tiles stay until it is built and vice versa; `syncWhole` never shows both: tiles stay hidden while the whole mesh is there). Farther: coloured height
  boxes, 2–16 blocks (equal tops merged per row). Biome colours are blended over 5×5 columns. Towards a coarser neighbour tile
  (the build message's `fine[n,s,w,e]` says which are step 1 too; tiles turn `dirty` when that
  flips) step-1 tiles add "just in case" faces / fill walls down to `coverMin` (heights at the
  step-aligned columns), ending 1/16 below the top (else z-fighting lines); top faces overlap tile
  edges by 1/16 (T-junction cracks); coarse tiles use a 16-block band minimum. Greedy meshing (worker `Merger`):
  equal full-block tops become rectangles per height plane, equal walls vertical runs that are then
  joined along their plane (grass side overlay included); colours rounded to steps of 4. Textures
  repeat in the shader (`repeatTextures` in map3d.js): per vertex a texture index (`tex`) and `uv`
  in texture widths, fract() + textureGrad picks the atlas cell. The atlas is uploaded with 32 px
  cells (8 px border, `atlasTexture`; the shader must match) so linear mipmaps + anisotropy work
  without bleeding. Geometry arrays are dropped after the GPU upload (`freeArray`).
  `state.view3d.stats` gives tiles/steps/triangles per part. The camera stays ≥2 blocks above the terrain.
  Water, plants (cross/flat/vine) and leaves are own meshes (`plants`/`leaves` buffers) so the
  viewer's display settings hide them without a rebuild. Height slice (`setOptions({slice: {lo, hi}})`,
  band ≤ 64 blocks, moved over the world height): clipping planes at once, then the tiles are
  rebuilt with `slice` (blocks outside skipped, top faces at the cut, plain dark cut faces over FILL,
  coarse boxes clamped; `heightAt` is clamped too, so the camera can go down). Right drag (Ctrl + left) turns the camera about itself (`turn`, MapControls only pans/zooms). WASD/arrows fly along the
  view direction (several keys at once), Space/Shift up/down, mouse wheel while a fly key is held =
  speed (`fly` in map3d.js, `options.flySpeed`); the slice band is set on a bar at the right edge. Display settings (`map.js` `settings`, localStorage):
  markers/names/spawn via `<body>` classes, the rest via `setOptions`. 2D: from Leaflet zoom 2 (4 px/block) a GridLayer draws textured tiles in the worker over
  the colour tiles. Tiles are re-fetched with
  ETag after a render. Headless `--screenshot` doesn't wait for workers: test via DevTools protocol
  (Playwright with `channel="msedge"` works on Windows).
  Players: 2D markers and far 3D markers show the head; within `MODEL_NEAR` blocks the 3D view shows
  a 3D model built from the skin (`player3d.js`, turned towards the camera: no facing is tracked).
  Clicking a player in the list opens a card (whole skin from the front, position, Follow, Copy
  /tp); follow mode glides the view (`goTo(p, FOLLOW_MS)`, `view3d.glide`) and ends on drag/fly.
  A click on the map picks a spot (y from the height tiles, worker message `height`); clicking the
  coordinate box copies `/execute in <dim> run tp @s x y z` (execCommand fallback on plain http).
- `panel/src/mapserver.rs` serves viewer (Leaflet and three.js vendored), tiles, `maps.json`,
  `blocks.json`/`atlas.png`, `players.json` and `skins/<uuid>.png` on `MAP_PORT` (separate uvicorn server in its own thread):
  every server at `/s/<id>/` (viewer served there too, so relative URLs need no prefix; status field
  `map_path`); `/` serves main, but its viewer moves on to the server this browser showed last
  (localStorage `map-last-server`, not when the URL has a position hash), `servers.json` = maps that are on and rendered (the viewer's
  server switch). The map port must never
  expose anything else. All responses carry COOP/COEP (`_Isolation`) for the viewer's shared
  worker memory: everything the viewer loads must stay same-origin (or send CORP).
- Hall of fame (`panel/src/leaderboard.rs`, `stats.json` on the map port, modal `static/map/leaderboard.js`):
  tier list by share of advancements done (recipes excluded; S ≥ 60 %, A ≥ 40 %, B ≥ 25 %, C ≥ 10 %),
  top 3 per record (`RECORDS`: movement, combat, mining, life; derived from `minecraft:custom` /
  `mined` / `crafted` / `broken` in `stats_of`), monster hunters / nemesis per mob (`killed`,
  `killed_by`), every advancement with who has it, a page per player. Titles, frames and mob names
  from the client jar the map already has (never downloads), else from the ids. Recomputed when a
  player file changes; Minecraft writes them only on autosave / leave, so opening it sends a
  `save-all` while players are online (≤ 1/min, not during an action) and the page reloads once
  (`saving`). Public on purpose: only names, skin ids, counts and advancement / mob ids. "Me" tab:
  the player picked in this browser (localStorage `lb-me:<path>`), else `me.json`: the player whose
  last login (`LOGIN_RE` in server.rs -> `leaderboard::remember_login`, memory only) came from the
  visitor's address, if exactly one. Addresses are never stored or sent.
- Waypoints (`panel/src/waypoints.rs`, viewer `static/map/waypoints.js`): per server in
  `<Instance.root>/waypoints.json` (never under the public map folder). The map port's only writes:
  `GET waypoints.json`, `POST waypoints`, `PUT/DELETE waypoints/<id>` (JSON content type + same
  Origin required, 4 KB bodies, strict validation, per-player and total limits). Owner = the player
  the browser picked (`X-Map-Player`, shared localStorage key with the hall of fame's "Me") or
  `leaderboard.me`. The picked name is only a claim, so editing needs the creating browser's key
  (`X-Map-Key`, stored as SHA-256), or the owner / an op recognised by login address; the panel
  (`/api/servers/{sid}/waypoints`) may change all. The map stays LAN-only by decision (no reverse
  proxy, no `X-Forwarded-For`): the address recognition relies on seeing the players' real addresses.
- Player heads: `livemap.skins` maps names to uuids via the server's `usercache.json` and downloads
  skins from Mojang (session server → only `textures.minecraft.net`, PNG ≤ 64 KB, daily refresh,
  1 h back-off) into `DATA_DIR/cache/skins/`; `players.json` names the `skin` once it exists; the
  viewer cuts out face + hat (hat ignored on old 64×32 skins when fully opaque).
- The old squaremap plugin is deleted on server start (`remove_squaremap`).

**Other modules:** `servericon.rs` (`server-icon.png` for Minecraft's server list: the logo
`app/static/map/logo.svg` as a 64×64 PNG drawn from `LOGO_RECTS` (a test keeps both equal), put in
place before each start when missing; upload via `PUT /icon`, the page scales the image), `datapacks.rs` (library `<DATA_DIR>/datapacks/` = source of truth, copies
installed into `<world>/datapacks/`; enabled state lives per world in level.dat
`Data.DataPacks.Enabled/Disabled` — online via `minecraft:datapack` + `save-all` until level.dat shows
it, offline by editing level.dat with `nbt.rs`, a byte-exact NBT round-trip codec (Java modified UTF-8); "New world"
installs the selected packs before generation), `properties.rs` (whitelisted, validated subset of `server.properties` for the
game settings dialog), `world.rs` (minimal NBT reader: `level.dat` → `Data.Version.Name`, used when
`settings.json` has no `world_version`), `backup.rs` (`restore()` validates zip member paths, makes a
`_before-restore` safety backup and protects the restored file from retention cleanup).
`archive_*.zip` files (written by "New world", `ServerManager._new_world`) must never be deleted:
`cleanup()` only globs `backup_*.zip` — keep it that way. `backup.add_upload()` turns an uploaded world
zip (server world, or single-player save with level.dat in the root / one folder) into
`backup_<stamp>_upload.zip` with this server's folder names, checked like a restore.
`addons.rs`: mods (Fabric, `server/mods/`) and plugins (Paper, `server/plugins/`), kind from the
server's flavor; metadata from `fabric.mod.json` (incl. jar-in-jar ids for dependency checks, version
ranges via `addons.matches`) or `plugin.yml`/`paper-plugin.yml` (tiny YAML subset parser, no PyYAML);
disabling = rename to `.jar.disabled`; Modrinth search/install (required deps)/updates (by SHA-512 via
`/version_files/update`), downloads only from Modrinth's CDN with SHA-512 check.
`players.rs`: profiles from `usercache.json`, `world/players/{stats,data}` (26.x) or
`world/{stats,playerdata}` (older), world spawn from level.dat (`Data.spawn` or `SpawnX/Y/Z`), skins via
`livemap.fetch_skin` (shared cache); the browser cuts out the head (`headUrl()` in app.js).
`gamerules.rs`: 26.x keeps rules in `<world>/dimensions/minecraft/overworld/data/minecraft/game_rules.dat`
(typed, `minecraft:snake_case`), older worlds in level.dat `Data.GameRules` (strings); never hard-code the
list — `KNOWN` only adds labels. `sessions.rs`: see below.

**Security-relevant invariants:**
- User-supplied flavor/version ids are validated (`validate_flavor`, `config.validate_version`)
  before being used in file paths — keep that for any new path built from request data. Same for
  server ids (`instances.validate_id`, applied in the `server()` dependency).
- Console commands are rejected if they contain newlines (stdin injection).
- Sessions survive restarts: `DATA_DIR/sessions.json` holds only SHA-256 hashes of the tokens plus
  a fingerprint of the password (a new `PANEL_PASSWORD` logs everybody out); 30 days, sliding
  (`require_auth` re-sends the cookie on renewal). Cookie `panel_session`, httponly + samesite=strict.
- Add-on and world uploads are streamed to a temp file (`api::receive`) with a size limit.

## Docker notes

The image is based on `eclipse-temurin:25-jre-noble`; extra Java runtimes from the `EXTRA_JAVA`
build arg are installed to `/opt/java/<major>` by `docker/install-java.py`. The entrypoint fixes
mount ownership as root then drops to uid 1000 (`minecraft`). `stop_grace_period: 90s` gives the
server time to save; `shutdown_all()` sends `stop` to every running server in parallel on app exit.
`HEALTHCHECK` polls the unauthenticated `GET /api/health`. Ports: 47318 panel, 47319 live map,
25565 Minecraft, 25566–25569 additional servers.

## Project conventions

- Backlogs are split by area so two Claude sessions can work in parallel: `TODO_DASHBOARD.md` /
  `DEV_TODO_DASHBOARD.md` (panel) and `TODO_LIVEMAP.md` / `DEV_TODO_LIVEMAP.md` (`mapgen/`,
  `panel/src/livemap.rs`, `mapserver.rs`, `leaderboard.rs`, `waypoints.rs`, `static/map/`). `TODO_*` = code tasks, `DEV_TODO_*` = decisions
  needing the developer. Work only on your area's list; each file names the paths it owns. Shared
  files (`panel/src/main.rs`, `api.rs`, `server.rs`, `config.rs`, `tasks.rs`, Docker files, README, this file) may be edited by the
  other session at the same time: re-read right before editing and keep changes small.
  The backlogs are written in German; code, comments and README are in English.
- Never read `.env` (denied in `.claude/settings.json`); use `.env.example` for the variable list.
- Git write operations (commit, add, push, branch, etc.) are denied for Claude in
  `.claude/settings.json`; leave version control to the user.
