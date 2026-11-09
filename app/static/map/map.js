"use strict";

// Map coordinates: Leaflet CRS.Simple with 1 unit = 1 block at zoom 0.
// lng = x, lat = -z (so north is up). Tiles: /tiles/<dim>/<zoom>/<x>_<z>.png, 512 px each.

const NAMES = { overworld: "Overworld", the_nether: "Nether", the_end: "End" };
const EMPTY_TILE = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

const state = { maps: null, dim: null, layer: null, version: 0, markers: new Map(), mode: "2d", view3d: null };

const map = L.map("map", {
  crs: L.CRS.Simple,
  zoomSnap: 1,
  attributionControl: false,
  minZoom: -8,
  maxZoom: 4,
});

const toLatLng = (x, z) => L.latLng(-z, x);

function parseHash() {
  const [dim, x, z, zoom, mode] = location.hash.slice(1).split("/");
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
    history.replaceState(null, "", `#${state.dim}/${Math.round(v.x)}/${Math.round(v.z)}/${v.zoom}${mode}`);
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
      done?.(data.bitmap);
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
    tileWaiting.set(id, (bitmap) => {
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
  }
  document.querySelectorAll(".dims button").forEach((b) => b.classList.toggle("active", b.dataset.slug === dim.slug));
  showSpawn();
  showBorder(dim, bounds);
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

// ---------- players ----------

function showSpawn() {
  if (state.spawnMarker) map.removeLayer(state.spawnMarker);
  state.spawnMarker = null;
  const s = state.maps.spawn;
  if (!s || s.dimension.split(":").pop() !== state.dim) return;
  state.spawnMarker = L.marker(toLatLng(s.x + 0.5, s.z + 0.5), {
    icon: markerIcon("Spawn", "spawn"), interactive: false, keyboard: false, zIndexOffset: -100,
  }).addTo(map);
}

function markerEl(name, kind = "") {
  const el = document.createElement("div");
  el.className = `marker ${kind}`;
  const label = document.createElement("div");
  label.className = "name";
  label.textContent = name;
  const pin = document.createElement("div");
  pin.className = "pin";
  el.append(label, pin);
  return el;
}

function markerIcon(name, kind = "") {
  return L.divIcon({ html: markerEl(name, kind), className: "", iconSize: null, iconAnchor: [0, 0] });
}

/** Spawn + players of the current dimension as 3D markers. */
function update3dMarkers() {
  if (!state.view3d) return;
  const list = [];
  const s = state.maps && state.maps.spawn;
  if (s && s.dimension.split(":").pop() === state.dim) list.push({ name: "Spawn", kind: "spawn", x: s.x + 0.5, z: s.z + 0.5 });
  for (const p of state.lastPlayers || []) {
    if ((p.dimension || "").split(":").pop() === state.dim) list.push({ name: p.name, x: p.x, y: p.y, z: p.z });
  }
  state.view3d.setMarkers(list);
}

function updatePlayers(players) {
  state.lastPlayers = players;
  update3dMarkers();
  const here = players.filter((p) => (p.dimension || "").split(":").pop() === state.dim);
  // markers
  const seen = new Set();
  for (const p of here) {
    seen.add(p.name);
    let m = state.markers.get(p.name);
    if (!m) {
      m = L.marker(toLatLng(p.x, p.z), { icon: markerIcon(p.name), interactive: false, keyboard: false });
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
  box.replaceChildren(...players.map((p) => {
    const row = document.createElement("div");
    row.className = "player";
    const dot = document.createElement("span");
    dot.className = "dot";
    const name = document.createElement("span");
    name.textContent = p.name;
    const where = document.createElement("span");
    where.className = "where";
    const slug = (p.dimension || "").split(":").pop();
    where.textContent = slug === state.dim ? `${Math.round(p.x)}, ${Math.round(p.z)}` : (NAMES[slug] || slug);
    row.append(dot, name, where);
    row.addEventListener("click", () => {
      if (slug !== state.dim) showDim(slug, { x: p.x, z: p.z, zoom: 1 });
      else if (state.mode === "3d") {
        const dim = state.maps.dimensions.find((d) => d.slug === state.dim);
        state.view3d.show(state.dim, dim.bounds, p.x, p.z, Math.min(state.view3d.center.distance, 300), state.maps.blocks || "");
      } else map.setView(toLatLng(p.x, p.z), Math.max(map.getZoom(), 1));
    });
    return row;
  }));
}

async function pollPlayers() {
  try {
    const res = await fetch("players.json", { cache: "no-cache" });
    if (res.ok) updatePlayers((await res.json()).players || []);
  } catch { /* offline: keep last state */ }
}

// ---------- coordinates & link ----------

map.on("mousemove", (e) => {
  document.getElementById("coords").textContent = `x ${Math.floor(e.latlng.lng)}, z ${Math.floor(-e.latlng.lat)}`;
});
map.on("moveend zoomend", writeHash);

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
          document.getElementById("coords").textContent =
            `x ${Math.floor(x)}, z ${Math.floor(z)}` + (y == null ? "" : `, y ${Math.round(y) - 1}`);
        },
        onChange: writeHash,
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
  } else {
    if (state.view3d) {
      const c = state.view3d.center;
      map.setView(toLatLng(c.x, c.z), zoomFor(c.distance));
      state.view3d.hide();
    }
    state.mode = "2d";
  }
  modeButton.textContent = state.mode === "3d" ? "2D" : "3D";
  modeButton.title = state.mode === "3d" ? "Back to the flat map" : "3D view (left drag: move, right drag: rotate)";
  writeHash();
}

modeButton.addEventListener("click", () => setMode(state.mode === "3d" ? "2d" : "3d"));

loadMaps(true);
setInterval(loadMaps, 30000);
pollPlayers();
setInterval(pollPlayers, 2000);
