"use strict";

// Map coordinates: Leaflet CRS.Simple with 1 unit = 1 block at zoom 0.
// lng = x, lat = -z (so north is up). Tiles: /tiles/<dim>/<zoom>/<x>_<z>.png, 512 px each.

const NAMES = { overworld: "Overworld", the_nether: "Nether", the_end: "End" };

// Every server has its own map at /s/<id>/ (the viewer is served there too, so all relative URLs
// stay as they are); / shows the first server (`main`), or sends the browser on to the server it
// showed last. servers.json lists the maps to switch to.
const PREFIXED = location.pathname.match(/\/s\/([a-z0-9-]+)\/$/);
const SERVER = PREFIXED ? PREFIXED[1] : "main";
const ROOT = new URL(PREFIXED ? "../../" : "./", location.href);
const LAST_SERVER_KEY = "map-last-server";
const EMPTY_TILE = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

const state = {
  maps: null, dim: null, layer: null, version: 0, markers: new Map(), mode: "2d", view3d: null,
  selected: null, // player whose card is open in the list
  follow: null,   // player the view follows
  picked: null,   // {dim, x, y, z} spot clicked on the map (copy as /tp)
};

const map = L.map("map", {
  crs: L.CRS.Simple,
  zoomSnap: 1,
  attributionControl: false,
  minZoom: -8,
  maxZoom: 4,
});

const toLatLng = (x, z) => L.latLng(-z, x);

/** View from the URL, else the one this browser showed last. */
function parseHash() {
  let hash = location.hash;
  if (hash.length < 2) {
    try {
      hash = localStorage.getItem(VIEW_KEY) || "";
    } catch { /* none */ }
  }
  const [dim, x, z, zoom, mode] = hash.slice(1).split("/");
  return { dim, x: Number(x), z: Number(z), zoom: Number(zoom), mode: mode === "3d" ? "3d" : "2d" };
}

// 3D camera distance <-> 2D zoom (zoom 0 = 1 block per pixel)
const distanceFor = (zoom) => 900 / 2 ** zoom;
const zoomFor = (distance) => Math.max(map.getMinZoom(), Math.min(4, Math.round(Math.log2(900 / distance))));

/** Current view centre and zoom, from whichever view is active. */
function currentView() {
  if (state.mode === "3d" && state.view3d) {
    const c = state.view3d.center;
    return { x: c.x, z: c.z, zoom: zoomFor(c.distance) };
  }
  const c = map.getCenter();
  return { x: c.lng, z: -c.lat, zoom: map.getZoom() };
}

let hashTimer;
function writeHash() {
  clearTimeout(hashTimer);
  hashTimer = setTimeout(() => {
    const v = currentView();
    const mode = state.mode === "3d" ? "/3d" : "";
    const hash = `#${state.dim}/${Math.round(v.x)}/${Math.round(v.z)}/${v.zoom}${mode}`;
    history.replaceState(null, "", hash);
    try {
      localStorage.setItem(VIEW_KEY, hash);
    } catch { /* not remembered */ }
  }, 300);
}

// ---------- textured 2D tiles ----------
// With block textures (maps.json "blocks"), zoom levels from 4 px per block on are drawn by a
// worker from the block tiles + texture atlas; the colour tiles stay underneath while it works.

const TEXTURED_FROM = 2; // Leaflet zoom: 2^2 = 4 px per block
let tileWorker = null, tileStamp = null, tileId = 0;
const tileWaiting = new Map();

function tileWorkerFor(stamp) {
  if (!tileWorker) {
    tileWorker = new Worker("map3d-worker.js");
    tileWorker.onmessage = ({ data }) => {
      const done = tileWaiting.get(data.id);
      tileWaiting.delete(data.id);
      done?.(data);
    };
  }
  if (stamp !== tileStamp) {
    tileStamp = stamp;
    tileWorker.postMessage({ type: "init", stamp, base: location.href });
  }
  return tileWorker;
}

const TexturedLayer = L.GridLayer.extend({
  createTile(coords, done) {
    const tile = document.createElement("canvas");
    tile.width = tile.height = 512;
    const scale = 2 ** coords.z; // px per block
    const blocks = 512 / scale;
    const id = ++tileId;
    tileWaiting.set(id, ({ bitmap }) => {
      if (bitmap) {
        tile.getContext("2d").drawImage(bitmap, 0, 0);
        bitmap.close();
      }
      done(null, tile);
    });
    tileWorkerFor(this.options.stamp).postMessage({
      type: "tile2d", id, dim: this.options.dim, v: state.version, x0: coords.x * blocks, z0: coords.y * blocks, scale,
    });
    return tile;
  },
});

function showDim(slug, view, refresh = false) {
  const dim = state.maps.dimensions.find((d) => d.slug === slug) || state.maps.dimensions[0];
  state.dim = dim.slug;
  if (state.layer) map.removeLayer(state.layer);
  const [minX, minZ, maxX, maxZ] = dim.bounds;
  const bounds = L.latLngBounds(toLatLng(minX * 512, (maxZ + 1) * 512), toLatLng((maxX + 1) * 512, minZ * 512));
  state.layer = L.tileLayer(`tiles/${dim.slug}/{z}/{x}_{y}.png?v={v}`, {
    v: () => state.version,
    tileSize: 512,
    minNativeZoom: dim.min_zoom,
    maxNativeZoom: 0,
    minZoom: dim.min_zoom - 1,
    maxZoom: 4,
    noWrap: true,
    bounds: bounds.pad(0.05),
    errorTileUrl: EMPTY_TILE,
    keepBuffer: 1,
  }).addTo(map);
  if (state.texLayer) map.removeLayer(state.texLayer);
  state.texLayer = null;
  if (state.maps.blocks) {
    state.texLayer = new TexturedLayer({
      dim: dim.slug, stamp: state.maps.blocks, tileSize: 512, minZoom: TEXTURED_FROM, maxZoom: 4,
      noWrap: true, bounds: bounds.pad(0.05), keepBuffer: 1,
    }).addTo(map);
  }
  map.setMinZoom(dim.min_zoom - 1);
  map.setMaxBounds(bounds.pad(0.5));

  if (view && Number.isFinite(view.x)) {
    map.setView(toLatLng(view.x, view.z), Number.isFinite(view.zoom) ? view.zoom : 0);
  } else {
    map.fitBounds(bounds);
  }
  if (state.mode === "3d" && state.view3d) {
    if (refresh) state.view3d.refresh(dim.bounds, state.maps.blocks || "");
    else {
      const c = map.getCenter();
      state.view3d.show(dim.slug, dim.bounds, c.lng, -c.lat, distanceFor(map.getZoom()), state.maps.blocks || "");
    }
    loadHeads();
  }
  document.querySelectorAll(".dims button").forEach((b) => b.classList.toggle("active", b.dataset.slug === dim.slug));
  showSpawn();
  if (typeof drawWaypoints === "function") drawWaypoints(); // waypoints.js
  showBorder(dim, bounds);
  applySettings(); // slice range of this dimension, border on/off
  updatePlayers(state.lastPlayers || []);
  writeHash();
}

// World border (only in maps.json when smaller than the default): dashed line, outside darkened
function showBorder(dim, bounds) {
  if (state.borderLayer) map.removeLayer(state.borderLayer);
  state.borderLayer = null;
  const b = dim.border;
  if (!b) return;
  const h = b.size / 2;
  const inner = L.latLngBounds(toLatLng(b.center_x - h, b.center_z + h), toLatLng(b.center_x + h, b.center_z - h));
  const outer = L.latLngBounds(bounds.getSouthWest(), bounds.getNorthEast()).extend(inner).pad(1);
  const ring = (r) => [r.getSouthWest(), r.getNorthWest(), r.getNorthEast(), r.getSouthEast()];
  state.borderLayer = L.layerGroup([
    L.polygon([ring(outer), ring(inner)], { stroke: false, fillColor: "#000", fillOpacity: 0.35, interactive: false }),
    L.rectangle(inner, { color: "#e05d5d", weight: 2, dashArray: "6 6", fill: false, interactive: false }),
  ]).addTo(map);
}

const ORDER = ["overworld", "the_nether", "the_end"];
const rank = (slug) => (ORDER.includes(slug) ? ORDER.indexOf(slug) : ORDER.length);

function renderDimButtons() {
  state.maps.dimensions.sort((a, b) => rank(a.slug) - rank(b.slug) || a.slug.localeCompare(b.slug));
  const box = document.getElementById("dims");
  box.replaceChildren(...state.maps.dimensions.map((d) => {
    const b = document.createElement("button");
    b.textContent = NAMES[d.slug] || d.id;
    b.dataset.slug = d.slug;
    b.addEventListener("click", () => showDim(d.slug));
    return b;
  }));
  box.classList.toggle("hidden", state.maps.dimensions.length < 2);
}

async function loadMaps(first) {
  try {
    const res = await fetch("maps.json", { cache: "no-cache" });
    if (!res.ok) throw new Error(res.status);
    const maps = await res.json();
    const changed = !state.maps || maps.generated !== state.maps.generated;
    if (!maps.dimensions.length) throw new Error("empty");
    document.getElementById("empty").classList.add("hidden");
    if (first) {
      state.maps = maps;
      state.version = maps.generated;
      renderDimButtons();
      const view = parseHash();
      showDim(view.dim, view);
      if (view.mode === "3d") setMode("3d");
    } else if (changed) {
      // New render: reload tiles in place (the ?v= parameter bypasses cached images)
      const dimsChanged = maps.dimensions.map((d) => d.slug).join() !== state.maps.dimensions.map((d) => d.slug).join();
      state.maps = maps;
      state.version = maps.generated;
      if (dimsChanged) renderDimButtons();
      showDim(state.dim, currentView(), true);
    }
  } catch {
    if (first) document.getElementById("empty").classList.remove("hidden");
  }
}

/** Player heads with their own skin (mapgen's heads.json, only with block textures): the 3D view
 *  puts the skin over the Steve head of the block model. */
async function loadHeads() {
  const view = state.view3d, dim = state.dim, v = state.version;
  if (!view || !state.maps?.blocks) return;
  let list = [];
  try {
    const res = await fetch(`tiles/${dim}/heads.json?v=${v}`);
    if (res.ok) list = await res.json();
  } catch { /* none */ }
  if (state.view3d === view && state.dim === dim && state.mode === "3d") view.setHeads(list);
  loadDecor();
}

/** Banner patterns and pot sherds (mapgen's decor.json, only with block textures): the 3D view
 *  draws them over the block models. */
async function loadDecor() {
  const view = state.view3d, dim = state.dim, v = state.version;
  if (!view || !state.maps?.blocks) return;
  let list = [];
  try {
    const res = await fetch(`tiles/${dim}/decor.json?v=${v}`);
    if (res.ok) list = await res.json();
  } catch { /* none */ }
  if (state.view3d === view && state.dim === dim && state.mode === "3d") view.setDecor(list);
}

/** Top face y of a block column in the current dimension (2D view: from the height tiles). */
function heightAt2d(x, z) {
  const id = ++tileId;
  return new Promise((resolve) => {
    tileWaiting.set(id, (data) => resolve(data.y ?? null));
    tileWorkerFor(state.maps.blocks || "").postMessage({ type: "height", id, dim: state.dim, v: state.version, x, z });
  });
}

// ---------- server switch ----------

/** Fills the server switch. True if the page moves on to the server the browser showed last
 *  (only when opened as / without a position in the URL). */
async function loadServers() {
  const select = document.getElementById("server");
  let last = null;
  try {
    last = localStorage.getItem(LAST_SERVER_KEY);
  } catch { /* not remembered */ }
  try {
    const res = await fetch("servers.json", { cache: "no-cache" });
    if (!res.ok) throw new Error(res.status);
    const servers = (await res.json()).servers || [];
    const target = !PREFIXED && location.hash.length < 2 && last && last !== SERVER && servers.find((s) => s.id === last);
    if (target) {
      location.replace(new URL(target.path, ROOT).href);
      return true;
    }
    const here = servers.find((s) => s.id === SERVER);
    if (here) {
      try {
        localStorage.setItem(LAST_SERVER_KEY, SERVER);
      } catch { /* not remembered */ }
    }
    if (here) document.title = `${here.name} · Live Map`;
    if (!here) servers.push({ id: SERVER, name: SERVER, path: null });
    setupServerMenu(select, servers);
    select.classList.toggle("hidden", servers.length < 2);
  } catch {
    select.classList.add("hidden");
  }
  return false;
}

/** The server switch: a button with the current server's name that opens a list of the others. */
function setupServerMenu(box, servers) {
  const btn = box.querySelector(".srv-btn");
  const menu = box.querySelector(".srv-menu");
  const current = servers.find((s) => s.id === SERVER);
  box.querySelector(".srv-name").textContent = current ? current.name : SERVER;

  const items = servers.map((s) => {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "srv-item" + (s.id === SERVER ? " current" : "");
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", String(s.id === SERVER));
    const dot = document.createElement("span");
    dot.className = "srv-dot";
    const name = document.createElement("span");
    name.className = "srv-item-name";
    name.textContent = s.name;
    item.append(dot, name);
    item.onclick = () => {
      close();
      if (s.id !== SERVER && s.path != null) location.href = new URL(s.path, ROOT).href;
    };
    return item;
  });
  menu.replaceChildren(...items);

  const isOpen = () => !menu.classList.contains("hidden");
  function open() {
    menu.classList.remove("hidden");
    btn.setAttribute("aria-expanded", "true");
    (items.find((i) => i.classList.contains("current")) || items[0])?.focus();
  }
  function close() {
    menu.classList.add("hidden");
    btn.setAttribute("aria-expanded", "false");
  }
  btn.onclick = () => (isOpen() ? close() : open());
  document.addEventListener("pointerdown", (e) => {
    if (isOpen() && !box.contains(e.target)) close();
  });
  box.addEventListener("keydown", (e) => {
    if (!isOpen()) return;
    const i = items.indexOf(document.activeElement);
    if (e.key === "Escape") {
      close();
      btn.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const step = e.key === "ArrowDown" ? 1 : -1;
      items[(i + step + items.length) % items.length].focus();
    } else return;
    e.preventDefault();
    e.stopPropagation(); // arrows also fly / pan the map
  });
}

// ---------- players ----------

const dimOf = (p) => (p.dimension || "").split(":").pop();

function showSpawn() {
  if (state.spawnMarker) map.removeLayer(state.spawnMarker);
  state.spawnMarker = null;
  const s = state.maps.spawn;
  if (!s || s.dimension.split(":").pop() !== state.dim) return;
  state.spawnMarker = L.marker(toLatLng(s.x + 0.5, s.z + 0.5), {
    icon: markerIcon("Spawn", "spawn"), interactive: false, keyboard: false, zIndexOffset: -100,
  }).addTo(map);
}

// ---------- player skins ----------
// The panel downloads each player's skin from Mojang (skins/<uuid>.png). Cut out here: the head
// (face 8×8 at 8,8 + hat layer at 40,8) and the whole figure seen from the front (16×32: head,
// body, arms, legs, each with its outer layer). Without a skin: a plain dot.

const skinCuts = new Map(); // "<head|body>:<uuid>" -> Promise<data URL | null>

function loadSkin(uuid) {
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      const c = document.createElement("canvas");
      c.width = img.width;
      c.height = img.height;
      const ctx = c.getContext("2d", { willReadFrequently: true });
      ctx.drawImage(img, 0, 0);
      resolve(c);
    };
    img.onerror = () => resolve(null);
    img.src = `skins/${uuid}.png`;
  });
}

/** Old 64×32 skins often fill the hat layer with solid colour, which the game ignores. */
function hatShown(skin) {
  if (skin.height === 64) return true;
  const a = skin.getContext("2d").getImageData(40, 8, 8, 8).data;
  for (let i = 3; i < a.length; i += 4) if (a[i] < 255) return true;
  return false;
}

function drawHead(skin) {
  const c = document.createElement("canvas");
  c.width = c.height = 8;
  const ctx = c.getContext("2d");
  ctx.drawImage(skin, 8, 8, 8, 8, 0, 0, 8, 8);
  if (hatShown(skin)) ctx.drawImage(skin, 40, 8, 8, 8, 0, 0, 8, 8);
  return c.toDataURL();
}

function drawBody(skin) {
  const modern = skin.height === 64;
  // slim ("Alex") arms are 3 px wide; their texture leaves x 54-55 of the arm rows empty
  const slim = modern && skin.getContext("2d").getImageData(54, 20, 1, 1).data[3] === 0;
  const aw = slim ? 3 : 4;
  const c = document.createElement("canvas");
  c.width = 16;
  c.height = 32;
  const ctx = c.getContext("2d");
  // [sx, sy, w, h, dx, dy, mirrored]; the player's right side is on the viewer's left
  const part = ([sx, sy, w, h, dx, dy, mirror]) => {
    if (!mirror) return ctx.drawImage(skin, sx, sy, w, h, dx, dy, w, h);
    ctx.save();
    ctx.translate(dx + w, dy);
    ctx.scale(-1, 1);
    ctx.drawImage(skin, sx, sy, w, h, 0, 0, w, h);
    ctx.restore();
  };
  const ax = 4 - aw;
  [
    [8, 8, 8, 8, 4, 0], [20, 20, 8, 12, 4, 8],                     // head, body
    [44, 20, aw, 12, ax, 8], modern ? [36, 52, aw, 12, 12, 8] : [44, 20, aw, 12, 12, 8, true], // arms
    [4, 20, 4, 12, 4, 20], modern ? [20, 52, 4, 12, 8, 20] : [4, 20, 4, 12, 8, 20, true],      // legs
  ].forEach(part);
  if (hatShown(skin)) part([40, 8, 8, 8, 4, 0]);
  if (modern) {
    [[20, 36, 8, 12, 4, 8], [44, 36, aw, 12, ax, 8], [52, 52, aw, 12, 12, 8],
      [4, 36, 4, 12, 4, 20], [4, 52, 4, 12, 8, 20]].forEach(part);
  }
  return c.toDataURL();
}

function skinUrl(uuid, kind) {
  const key = `${kind}:${uuid}`;
  if (!skinCuts.has(key)) {
    skinCuts.set(key, loadSkin(uuid).then((skin) => (skin ? (kind === "body" ? drawBody(skin) : drawHead(skin)) : null)));
  }
  return skinCuts.get(key);
}

/** A player's head or whole figure (`body`), a dot until / unless the skin is there. */
function skinEl(uuid, cls, body = false) {
  const el = document.createElement("span");
  el.className = cls;
  if (uuid) {
    skinUrl(uuid, body ? "body" : "head").then((src) => {
      if (!src) return;
      const img = document.createElement("img");
      img.src = src;
      img.alt = "";
      el.replaceChildren(img);
      el.classList.add(body ? "body" : "head");
    });
  }
  return el;
}

/** `wp`: the waypoint of a "waypoint" marker (colour, id for clicks; see waypoints.js). */
function markerEl(name, kind = "", skin = null, wp = null) {
  const el = document.createElement("div");
  el.className = `marker ${kind}`;
  if (kind === "waypoint" && wp) {
    el.classList.add(`wp-${wp.color}`);
    el.dataset.wp = wp.id;
  }
  const label = document.createElement("div");
  label.className = "name";
  label.textContent = name;
  // players: the head (the 3D view shows a whole 3D model up close instead)
  el.append(label, kind === "player" ? skinEl(skin, "pin") : Object.assign(document.createElement("div"), { className: "pin" }));
  return el;
}

function markerIcon(name, kind = "", skin = null, wp = null) {
  return L.divIcon({ html: markerEl(name, kind, skin, wp), className: "", iconSize: null, iconAnchor: [0, 0] });
}

/** Spawn + players of the current dimension as 3D markers. */
function update3dMarkers() {
  if (!state.view3d) return;
  const list = [];
  const s = state.maps && state.maps.spawn;
  if (s && s.dimension.split(":").pop() === state.dim) list.push({ name: "Spawn", kind: "spawn", x: s.x + 0.5, z: s.z + 0.5 });
  for (const p of state.lastPlayers || []) {
    if (dimOf(p) === state.dim) list.push({ name: p.name, kind: "player", skin: p.skin, x: p.x, y: p.y, z: p.z });
  }
  for (const w of state.waypoints || []) {
    if (w.dim === state.dim) list.push({ name: w.name, kind: "waypoint", id: w.id, color: w.color, x: w.x + 0.5, y: w.y ?? undefined, z: w.z + 0.5 });
  }
  const pk = state.picked;
  if (pk && pk.dim === state.dim) list.push({ name: "", kind: "picked", x: pk.x + 0.5, y: pk.y ?? undefined, z: pk.z + 0.5 });
  state.view3d.setMarkers(list);
}

const FOLLOW_MS = 2000; // = the position poll interval: the view glides from one to the next

/** Centre the view on a player (switching dimension if needed); `ms` > 0 glides there. */
function goTo(p, ms = 0) {
  const slug = dimOf(p);
  if (slug !== state.dim) {
    if (!state.maps.dimensions.some((d) => d.slug === slug)) return;
    showDim(slug, { x: p.x, z: p.z, zoom: Math.max(currentView().zoom, 1) });
  } else if (state.mode === "3d") {
    if (ms) state.view3d.glide(p.x, p.y, p.z, ms);
    else {
      const dim = state.maps.dimensions.find((d) => d.slug === state.dim);
      state.view3d.show(state.dim, dim.bounds, p.x, p.z, Math.min(state.view3d.center.distance, 300), state.maps.blocks || "");
    }
  } else if (ms) {
    map.panTo(toLatLng(p.x, p.z), { animate: true, duration: ms / 1000, easeLinearity: 1, noMoveStart: true });
  } else {
    map.setView(toLatLng(p.x, p.z), Math.max(map.getZoom(), 1));
  }
}

function setFollow(name) {
  state.follow = name;
  if (name) {
    state.selected = name;
    const p = (state.lastPlayers || []).find((x) => x.name === name);
    if (p) goTo(p);
    showToast(`Following ${name}: drag the map to stop`);
  }
  updatePlayers(state.lastPlayers || []);
}

function stopFollow() {
  if (!state.follow) return;
  setFollow(null);
  showToast("Stopped following");
}

/** Card of the selected player: whole skin, position, follow and /tp buttons. */
function playerCard(p) {
  const card = document.createElement("div");
  card.className = "player-card";
  const figure = skinEl(p.skin, "figure", true);
  const info = document.createElement("div");
  info.className = "info";
  const pos = document.createElement("div");
  pos.className = "pos";
  pos.textContent = `${Math.floor(p.x)}  ${Math.floor(p.y)}  ${Math.floor(p.z)}`;
  const dim = document.createElement("div");
  dim.className = "muted";
  dim.textContent = NAMES[dimOf(p)] || dimOf(p);
  const following = state.follow === p.name;
  const follow = Object.assign(document.createElement("button"), {
    className: `mode small${following ? " active" : ""}`,
    textContent: following ? "Stop following" : "Follow",
    title: following ? "Stop moving the map with this player" : "Keep this player in the centre of the map",
  });
  follow.addEventListener("click", (e) => {
    e.stopPropagation();
    if (following) stopFollow();
    else setFollow(p.name);
  });
  const tp = Object.assign(document.createElement("button"), {
    className: "mode small", textContent: "Copy /tp", title: `Copy a command that teleports you to ${p.name}`,
  });
  tp.addEventListener("click", (e) => {
    e.stopPropagation();
    copyCommand(`/tp @s ${p.name}`);
  });
  info.append(pos, dim, follow, tp);
  card.append(figure, info);
  return card;
}

function updatePlayers(players) {
  state.lastPlayers = players;
  if (state.follow) {
    const p = players.find((x) => x.name === state.follow);
    if (p) goTo(p, FOLLOW_MS);
    else {
      showToast(`${state.follow} is no longer online`);
      state.follow = null;
    }
  }
  update3dMarkers();
  const here = players.filter((p) => dimOf(p) === state.dim);
  // markers
  const seen = new Set();
  for (const p of here) {
    seen.add(p.name);
    let m = state.markers.get(p.name);
    if (!m || m.skin !== p.skin) {
      if (m) map.removeLayer(m);
      m = L.marker(toLatLng(p.x, p.z), { icon: markerIcon(p.name, "player", p.skin), interactive: false, keyboard: false });
      m.skin = p.skin;
      state.markers.set(p.name, m);
    }
    m.setLatLng(toLatLng(p.x, p.z));
    if (!map.hasLayer(m)) m.addTo(map);
  }
  for (const [name, m] of state.markers) {
    if (!seen.has(name)) {
      map.removeLayer(m);
      state.markers.delete(name);
    }
  }
  // list
  const box = document.getElementById("players-box");
  if (!players.length) {
    box.innerHTML = '<div class="muted">No players online</div>';
    return;
  }
  if (state.selected && !players.some((p) => p.name === state.selected)) state.selected = null;
  box.replaceChildren(...players.map((p) => {
    const entry = document.createElement("div");
    entry.className = `player-entry${state.selected === p.name ? " open" : ""}${state.follow === p.name ? " following" : ""}`;
    const row = document.createElement("div");
    row.className = "player";
    row.title = state.selected === p.name ? "Close" : "Show on the map";
    const dot = skinEl(p.skin, "dot");
    const name = document.createElement("span");
    name.textContent = p.name;
    const where = document.createElement("span");
    where.className = "where";
    const slug = dimOf(p);
    where.textContent = slug === state.dim ? `${Math.round(p.x)}, ${Math.round(p.z)}` : (NAMES[slug] || slug);
    row.append(dot, name, where);
    row.addEventListener("click", () => {
      if (state.selected === p.name) state.selected = null;
      else {
        state.selected = p.name;
        if (state.follow && state.follow !== p.name) state.follow = p.name; // follow the new one
        goTo(p);
      }
      updatePlayers(state.lastPlayers || []);
    });
    entry.append(row);
    if (state.selected === p.name) entry.append(playerCard(p));
    return entry;
  }));
}

async function pollPlayers() {
  try {
    const res = await fetch("players.json", { cache: "no-cache" });
    if (res.ok) updatePlayers((await res.json()).players || []);
  } catch { /* offline: keep last state */ }
}

// ---------- display settings ----------
// Kept per browser (localStorage), like the last view (map-last-view, used when the URL has no
// position). Markers and labels: CSS classes on <body>; the 3D view gets the rest via setOptions.
// The height slice shows a band of at most SLICE_MAX blocks that can be moved over the whole
// height of the world (3D only, bar on the right edge); with it on, the 3D view loads the stacks
// with every cave (maps.json "caves").

const SLICE_MAX = 64, SLICE_MIN = 4;
const DIM_Y = { overworld: [-64, 320], the_nether: [0, 128], the_end: [0, 256] };
const dimY = () => DIM_Y[state.dim] || DIM_Y.overworld;
const SETTINGS_KEY = "map-settings", VIEW_KEY = SERVER === "main" ? "map-last-view" : `map-last-view:${SERVER}`;
const settings = {
  players: true, names: true, spawn: true, waypoints: true, border: true,
  water: true, plants: true, leaves: true, fog: true,
  slice: false, sliceHeight: SLICE_MAX, sliceBottom: 40, flySpeed: 40,
};
try {
  Object.assign(settings, JSON.parse(localStorage.getItem(SETTINGS_KEY) || "{}"));
} catch { /* private mode / broken entry: defaults */ }

function sliceRange() {
  const [minY, maxY] = dimY();
  const height = Math.max(SLICE_MIN, Math.min(SLICE_MAX, Math.round(settings.sliceHeight) || SLICE_MAX));
  const lo = Math.max(minY, Math.min(maxY - height, Math.round(settings.sliceBottom) || 0));
  return { lo, hi: lo + height };
}

const sliceBar = document.getElementById("slicebar"), sliceTrack = document.getElementById("slice-track");
const sliceBand = document.getElementById("slice-band");

function drawSliceBar() {
  const [minY, maxY] = dimY(), { lo, hi } = sliceRange(), span = maxY - minY;
  sliceBand.style.top = `${((maxY - hi) / span) * 100}%`;
  sliceBand.style.height = `${((hi - lo) / span) * 100}%`;
  document.getElementById("slice-hi").textContent = `y ${hi - 1}`;
  document.getElementById("slice-lo").textContent = `y ${lo}`;
  sliceBar.classList.toggle("off", !settings.slice);
  sliceBar.classList.toggle("hidden", state.mode !== "3d");
  document.getElementById("slice-toggle").classList.toggle("active", settings.slice);
}

function applySettings() {
  document.body.classList.toggle("hide-players", !settings.players);
  document.body.classList.toggle("hide-names", !settings.names);
  document.body.classList.toggle("hide-spawn", !settings.spawn);
  document.body.classList.toggle("hide-waypoints", !settings.waypoints);
  if (state.borderLayer) {
    if (settings.border) state.borderLayer.addTo(map);
    else map.removeLayer(state.borderLayer);
  }
  for (const box of document.querySelectorAll("#settings input[type=checkbox]")) box.checked = !!settings[box.dataset.key];
  drawSliceBar();
  state.view3d?.setOptions({
    players: settings.players, water: settings.water, plants: settings.plants, leaves: settings.leaves, fog: settings.fog,
    slice: settings.slice ? sliceRange() : null, caves: !!state.maps?.caves, flySpeed: settings.flySpeed,
  });
}

function saveSettings() {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch { /* not stored: still applies for this visit */ }
}

for (const box of document.querySelectorAll("#settings input[type=checkbox]")) {
  box.addEventListener("change", () => {
    settings[box.dataset.key] = box.checked;
    saveSettings();
    applySettings();
  });
}

// slice bar: drag the band to move it, its ends to resize it, click beside it to move it there,
// scroll over it to move it (Shift: 1 block, else 4). Any of that switches the slice on.
document.getElementById("slice-toggle").addEventListener("click", () => {
  settings.slice = !settings.slice;
  saveSettings();
  applySettings();
});

sliceTrack.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  const [minY, maxY] = dimY(), rect = sliceTrack.getBoundingClientRect();
  const yAt = (clientY) => maxY - ((clientY - rect.top) / rect.height) * (maxY - minY);
  const y0 = yAt(e.clientY);
  let mode = e.target.closest(".grip.top") ? "top" : e.target.closest(".grip.bottom") ? "bottom" : "move";
  if (!e.target.closest("#slice-band")) {
    // beside the band: centre it there, then drag on
    const { lo, hi } = sliceRange();
    settings.sliceBottom = Math.round(y0 - (hi - lo) / 2);
    mode = "move";
  }
  settings.slice = true;
  const start = sliceRange();
  const onMove = (ev) => {
    const dy = Math.round(yAt(ev.clientY) - y0);
    if (mode === "move") {
      settings.sliceBottom = start.lo + dy;
    } else if (mode === "top") {
      const hi = Math.max(start.lo + SLICE_MIN, Math.min(start.lo + SLICE_MAX, maxY, start.hi + dy));
      settings.sliceBottom = start.lo;
      settings.sliceHeight = hi - start.lo;
    } else {
      const lo = Math.max(start.hi - SLICE_MAX, minY, Math.min(start.hi - SLICE_MIN, start.lo + dy));
      settings.sliceBottom = lo;
      settings.sliceHeight = start.hi - lo;
    }
    applySettings();
  };
  const onUp = () => {
    sliceTrack.removeEventListener("pointermove", onMove);
    sliceTrack.removeEventListener("pointerup", onUp);
    sliceTrack.removeEventListener("pointercancel", onUp);
    saveSettings();
  };
  sliceTrack.setPointerCapture(e.pointerId);
  sliceTrack.addEventListener("pointermove", onMove);
  sliceTrack.addEventListener("pointerup", onUp);
  sliceTrack.addEventListener("pointercancel", onUp);
  applySettings();
});

sliceBar.addEventListener("wheel", (e) => {
  e.preventDefault();
  if (!e.deltaY) return;
  settings.slice = true;
  settings.sliceBottom = sliceRange().lo - Math.sign(e.deltaY) * (e.shiftKey ? 1 : 4);
  applySettings();
  saveSettings();
}, { passive: false });

// flight speed (mouse wheel while flying in 3D): short notice, remembered
const toast = document.getElementById("toast");
let toastTimer = 0;
function showToast(text, ms = 1200) {
  toast.textContent = text;
  toast.classList.remove("hidden", "fade");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.add("fade"), ms);
}
function onFlySpeed(speed) {
  settings.flySpeed = speed;
  saveSettings();
  showToast(`Flight speed: ${speed < 10 ? speed.toFixed(1) : Math.round(speed)} blocks/s`);
}
const settingsButton = document.getElementById("settings-btn"), settingsPanel = document.getElementById("settings");
settingsButton.addEventListener("click", () => {
  settingsPanel.classList.toggle("hidden");
  settingsButton.classList.toggle("active", !settingsPanel.classList.contains("hidden"));
});
applySettings();

// ---------- coordinates & link ----------
// Hovering shows the coordinates; a click on the map picks that spot (marker + entry in the box at
// the bottom), and clicking the box copies a teleport command for it.

const coordsEl = document.getElementById("coords"), pickedEl = document.getElementById("picked");
let lastHover = null; // 3D: {x, z, y} under the mouse

map.on("mousemove", (e) => {
  coordsEl.textContent = `x ${Math.floor(e.latlng.lng)}, z ${Math.floor(-e.latlng.lat)}`;
});
map.on("moveend zoomend", writeHash);
map.on("dragstart", stopFollow);
map.on("click", (e) => pick(e.latlng.lng, -e.latlng.lat));

/** Pick a spot; y = top face of the column (standing height), looked up when not given. */
function pick(x, z, y) {
  const pk = { dim: state.dim, x: Math.floor(x), z: Math.floor(z), y: y == null ? null : Math.round(y) };
  state.picked = pk;
  drawPicked();
  if (pk.y == null && state.maps) {
    heightAt2d(pk.x + 0.5, pk.z + 0.5).then((h) => {
      if (state.picked !== pk || h == null) return;
      pk.y = h;
      drawPicked();
    });
  }
}

function clearPick() {
  state.picked = null;
  drawPicked();
}

function drawPicked() {
  const pk = state.picked;
  if (state.pickMarker) map.removeLayer(state.pickMarker);
  state.pickMarker = null;
  pickedEl.classList.toggle("hidden", !pk);
  if (pk) {
    const where = pk.dim === "overworld" ? "" : ` (${NAMES[pk.dim] || pk.dim})`;
    document.getElementById("picked-text").textContent = `${pk.x} ${pk.y ?? "~"} ${pk.z}${where}`;
    if (pk.dim === state.dim) {
      state.pickMarker = L.marker(toLatLng(pk.x + 0.5, pk.z + 0.5), {
        icon: markerIcon("", "picked"), interactive: false, keyboard: false, zIndexOffset: -50,
      }).addTo(map);
    }
  }
  update3dMarkers();
  if (typeof renderWpList === "function") renderWpList(); // "add the picked spot" (waypoints.js)
}

/** `/execute in <dimension> run tp @s x y z`: works from any dimension. */
function tpCommand(pk) {
  const dim = state.maps.dimensions.find((d) => d.slug === pk.dim);
  const id = dim?.id || `minecraft:${pk.dim}`;
  return `/execute in ${id} run tp @s ${pk.x} ${pk.y ?? "~"} ${pk.z}`;
}

/** Clipboard, with the old fallback for pages on plain http (no Clipboard API there). */
async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch { /* not allowed / not available */ }
  const ta = Object.assign(document.createElement("textarea"), { value: text, readOnly: true });
  ta.style.cssText = "position:fixed;top:0;left:0;opacity:0";
  document.body.append(ta);
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch { /* nothing to do */ }
  ta.remove();
  return ok;
}

async function copyCommand(cmd) {
  showToast((await copyText(cmd)) ? `Copied: ${cmd}` : `Copy this: ${cmd}`, 2500);
}

document.getElementById("coords-box").addEventListener("click", (e) => {
  if (e.target.closest("#picked-clear")) {
    clearPick();
    return;
  }
  if (state.picked) copyCommand(tpCommand(state.picked));
  else showToast("Click a spot on the map first, then here to copy its /tp command", 2500);
});
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && state.picked) clearPick();
});

// ---------- 2D / 3D ----------

const modeButton = document.getElementById("mode");
const container3d = document.getElementById("map3d");

async function setMode(mode) {
  if (mode === "3d" && !state.view3d) {
    modeButton.disabled = true;
    try {
      const { create } = await import("./map3d.js");
      state.view3d = create({
        container: container3d,
        makeMarker: markerEl,
        onHover: (x, z, y) => {
          lastHover = { x, z, y };
          coordsEl.textContent = `x ${Math.floor(x)}, z ${Math.floor(z)}` + (y == null ? "" : `, y ${Math.round(y) - 1}`);
        },
        onChange: writeHash,
        onSpeed: onFlySpeed,
      });
    } catch (e) {
      console.error(e);
      modeButton.textContent = "3D unavailable";
      modeButton.title = "The 3D view could not be started (WebGL not available?)";
      return;
    } finally {
      modeButton.disabled = false;
    }
  }
  if (mode === "3d") {
    // keep the 2D map at the same spot for when we switch back
    state.mode = "3d";
    const c = map.getCenter();
    const dim = state.maps.dimensions.find((d) => d.slug === state.dim);
    state.view3d.show(dim.slug, dim.bounds, c.lng, -c.lat, distanceFor(map.getZoom()), state.maps.blocks || "");
    update3dMarkers();
    applySettings();
    loadHeads();
  } else {
    if (state.view3d) {
      const c = state.view3d.center;
      map.setView(toLatLng(c.x, c.z), zoomFor(c.distance));
      state.view3d.hide();
    }
    state.mode = "2d";
    drawSliceBar();
  }
  modeButton.textContent = state.mode === "3d" ? "2D" : "3D";
  modeButton.title = state.mode === "3d" ? "Back to the flat map" : "3D view (left drag: move, right drag: look around)";
  writeHash();
}

modeButton.addEventListener("click", () => setMode(state.mode === "3d" ? "2d" : "3d"));

// 3D: a click (no drag) picks the spot under the mouse; panning or flying ends follow mode
let press = null;
container3d.addEventListener("pointerdown", (e) => {
  press = e.button === 0 && !e.ctrlKey ? { x: e.clientX, y: e.clientY, moved: false } : null;
});
container3d.addEventListener("pointermove", (e) => {
  if (!press || !(e.buttons & 1)) return;
  if (Math.hypot(e.clientX - press.x, e.clientY - press.y) > 4) {
    press.moved = true;
    stopFollow();
  }
});
container3d.addEventListener("pointerup", () => {
  if (press && !press.moved && lastHover && lastHover.y != null) pick(lastHover.x, lastHover.z, lastHover.y);
  press = null;
});
const FLY_KEYS = new Set(["KeyW", "KeyA", "KeyS", "KeyD", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Space", "ShiftLeft", "ShiftRight"]);
window.addEventListener("keydown", (e) => {
  if (state.mode !== "3d" || !FLY_KEYS.has(e.code) || e.ctrlKey || e.metaKey || e.altKey) return;
  if (e.target instanceof Element && e.target.closest("input, select, textarea")) return;
  stopFollow();
});

loadServers().then((moved) => {
  if (!moved) loadMaps(true);
});
setInterval(loadMaps, 30000);
pollPlayers();
setInterval(pollPlayers, 2000);
