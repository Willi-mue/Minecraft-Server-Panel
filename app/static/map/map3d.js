// 3D view in blocks: every block column of the map is drawn as a box (top at its height, sides down
// to the lower neighbour), built from the 16-bit height tiles and either the block tiles + texture
// atlas (real Minecraft textures, when mapgen had the client jar) or the colour tiles.
// Decoding and meshing run in web workers (map3d-worker.js, a small pool; every worker gets every
// region, the meshes are spread over them).
// Loaded on demand (dynamic import from map.js).
//
// World units = blocks: x = block x, y = height, z = block z. Regions (512×512 blocks) are loaded
// around the camera and split into 4×4 tiles of 128 blocks; each tile is one mesh whose detail
// depends on its distance (far away, several blocks are merged into bigger boxes).

import * as THREE from "three";
import { MapControls } from "three/addons/controls/MapControls.js";

const SIZE = 512;
const NONE = -32768;
const BG = 0x1c1f26;
// sky per dimension: zenith, horizon (= fog colour, so distant terrain fades into the sky), below
// the horizon (seen past the world edge); unknown (custom) dimensions get the overworld sky
const SKIES = {
  overworld: [0x78a7ff, 0xc0d8ff, 0x8a9cb8],
  the_nether: [0x330808, 0x330808, 0x1a0404],
  the_end: [0x0d0a10, 0x2b2233, 0x0d0a10],
};
const MAX_RADIUS = 3072; // blocks around the view centre that get loaded at most
const PARALLEL = 6;      // region downloads at a time
const POOL = Math.max(1, Math.min(3, (navigator.hardwareConcurrency || 2) - 1)); // mesh workers
const BUILDS = POOL * 2; // meshes being built at a time
const TILE = 128;        // blocks per mesh tile
const TILES = SIZE / TILE;

/** Block size of the boxes for a tile at distance d from the camera (1 = every block). */
const stepFor = (d) => (d < 360 ? 1 : d < 800 ? 2 : d < 1600 ? 4 : d < 3000 ? 8 : 16);

// The tile colors are final sRGB values: pass them (and the fog colour) through unchanged
THREE.ColorManagement.enabled = false;

export function create({ container, makeMarker, onHover, onChange }) {
  const renderer = new THREE.WebGLRenderer({ antialias: true });
  renderer.outputColorSpace = THREE.LinearSRGBColorSpace;
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  renderer.setClearColor(BG);
  container.appendChild(renderer.domElement);
  const overlay = document.createElement("div");
  overlay.className = "overlay3d";
  container.appendChild(overlay);

  const scene = new THREE.Scene();
  scene.fog = new THREE.Fog(BG, 1000, 2000); // adjusted in updateFog()

  // Sky: a unit sphere around the camera whose colour depends only on the view direction. It is
  // drawn first at infinite distance (no depth test/write), so it never hides or clips terrain.
  const skyMaterial = new THREE.ShaderMaterial({
    uniforms: {
      zenith: { value: new THREE.Color() },
      horizon: { value: new THREE.Color() },
      below: { value: new THREE.Color() },
    },
    vertexShader: `
      varying vec3 vDir;
      void main() {
        vDir = position;
        vec4 p = projectionMatrix * vec4(mat3(viewMatrix) * position, 1.0);
        gl_Position = vec4(p.xy, p.w * 0.9999, p.w); // just inside the far plane
      }`,
    fragmentShader: `
      uniform vec3 zenith, horizon, below;
      varying vec3 vDir;
      void main() {
        float y = normalize(vDir).y;
        vec3 c = y >= 0.0 ? mix(horizon, zenith, pow(y, 0.6)) : mix(horizon, below, min(1.0, -y * 4.0));
        gl_FragColor = vec4(c, 1.0);
      }`,
    side: THREE.BackSide,
    depthTest: false,
    depthWrite: false,
  });
  const sky = new THREE.Mesh(new THREE.SphereGeometry(1, 32, 16), skyMaterial);
  sky.frustumCulled = false;
  sky.renderOrder = -1;
  scene.add(sky);

  function setSky(slug) {
    const [zenith, horizon, below] = SKIES[slug] || SKIES.overworld;
    skyMaterial.uniforms.zenith.value.setHex(zenith);
    skyMaterial.uniforms.horizon.value.setHex(horizon);
    skyMaterial.uniforms.below.value.setHex(below);
    scene.fog.color.setHex(horizon);
    renderer.setClearColor(horizon);
    requestRender();
  }
  // Face shading and biome tints are baked into the vertex colors (like Minecraft), so no lights
  // are needed; the map is the texture atlas (or plain white without textures)
  const white = new THREE.DataTexture(new Uint8Array([255, 255, 255, 255]), 1, 1);
  white.needsUpdate = true;
  const material = new THREE.MeshBasicMaterial({ vertexColors: true, map: white });
  // leaves, plants, vines, glass, the grass side overlay: cut out along the texture's alpha, both
  // sides; pulled slightly forward so the grass overlay never fights with the side below it
  const cutoutMaterial = new THREE.MeshBasicMaterial({
    vertexColors: true, map: white, alphaTest: 0.5, side: THREE.DoubleSide,
    polygonOffset: true, polygonOffsetFactor: -1, polygonOffsetUnits: -1,
  });
  const waterMaterial = new THREE.MeshBasicMaterial({
    vertexColors: true, map: white, transparent: true, opacity: 0.8, depthWrite: false, side: THREE.DoubleSide,
  });
  const materials = { opaque: material, cutout: cutoutMaterial, water: waterMaterial };
  let stamp = null; // texture tables in use ("" = none)

  /** The atlas with its own mipmaps that stop at 1 px per 16 px texture: the GPU's automatic
   *  levels go further and blend neighbouring textures, which shows as coloured lines along the
   *  block edges at a distance. */
  function atlasTexture(bitmap) {
    const w0 = bitmap.width, h0 = bitmap.height;
    const canvas = new OffscreenCanvas(w0, h0);
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    ctx.drawImage(bitmap, 0, 0);
    bitmap.close();
    const src = ctx.getImageData(0, 0, w0, h0).data;
    // Every 16 px texture gets a 32 px cell with an 8 px border of its own edge pixels
    // (map3d-worker.js uvRect knows this layout): smooth (linear) filtering and the smaller
    // levels never pick up a neighbouring texture
    const cols = w0 / 16, rows = h0 / 16, w = cols * 32, h = rows * 32;
    const padded = new Uint8Array(w * h * 4);
    for (let cy = 0; cy < rows; cy++) {
      for (let cx = 0; cx < cols; cx++) {
        for (let y = 0; y < 32; y++) {
          const sy = cy * 16 + Math.min(15, Math.max(0, y - 8));
          for (let x = 0; x < 32; x++) {
            const sx = cx * 16 + Math.min(15, Math.max(0, x - 8));
            const s = (sy * w0 + sx) * 4, d = ((cy * 32 + y) * w + cx * 32 + x) * 4;
            padded[d] = src[s]; padded[d + 1] = src[s + 1]; padded[d + 2] = src[s + 2]; padded[d + 3] = src[s + 3];
          }
        }
      }
    }
    const levels = [{ data: padded, width: w, height: h }];
    for (let l = 1; l <= 4; l++) {
      const p = levels[l - 1], nw = p.width >> 1, nh = p.height >> 1;
      const d = new Uint8Array(nw * nh * 4);
      for (let y = 0; y < nh; y++) {
        for (let x = 0; x < nw; x++) {
          for (let k = 0; k < 4; k++) {
            const i = ((y * 2) * p.width + x * 2) * 4 + k, row = p.width * 4;
            d[(y * nw + x) * 4 + k] = (p.data[i] + p.data[i + 4] + p.data[i + row] + p.data[i + row + 4]) >> 2;
          }
        }
      }
      levels.push({ data: d, width: nw, height: nh });
    }
    const tex = new THREE.DataTexture(levels[0].data, w, h);
    tex.mipmaps = levels;
    tex.generateMipmaps = false;
    tex.flipY = false;
    tex.magFilter = THREE.NearestFilter; // crisp pixels up close
    tex.minFilter = THREE.LinearMipmapLinearFilter; // smooth in the distance: no grid / shimmer
    tex.anisotropy = renderer.capabilities.getMaxAnisotropy();
    tex.needsUpdate = true;
    return tex;
  }

  async function setAssets(next) {
    if (next === stamp) return;
    stamp = next;
    clear();
    await ask("init", { type: "init", stamp: next, base: location.href });
    let tex = white;
    if (next) {
      try {
        const res = await fetch(`atlas.png?v=${encodeURIComponent(next)}`);
        const bitmap = await createImageBitmap(await res.blob(), { imageOrientation: "none", premultiplyAlpha: "none" });
        tex = atlasTexture(bitmap);
      } catch {
        tex = white;
      }
    }
    if (material.map !== white) material.map.dispose();
    for (const m of Object.values(materials)) {
      m.map = tex;
      m.needsUpdate = true;
    }
    scheduleUpdate();
  }

  const camera = new THREE.PerspectiveCamera(50, 1, 1, 4000);
  const controls = new MapControls(camera, renderer.domElement);
  controls.maxPolarAngle = Math.PI / 2 - 0.05;
  controls.minDistance = 8;
  controls.maxDistance = 4000;
  controls.zoomToCursor = true;

  const workers = Array.from({ length: POOL }, () => {
    const w = new Worker(new URL("./map3d-worker.js", import.meta.url));
    w.busy = 0;
    w.waiting = new Map(); // "loaded:<key>" / "init" / "mesh:<id>" -> resolve
    w.onmessage = ({ data }) => {
      const id = data.type === "loaded" ? `loaded:${data.key}` : data.type === "init" ? "init" : `mesh:${data.id}`;
      const resolve = w.waiting.get(id);
      w.waiting.delete(id);
      resolve?.(data);
    };
    return w;
  });
  const askOne = (w, id, msg, transfer = []) => new Promise((resolve) => {
    w.waiting.set(id, resolve);
    w.postMessage(msg, transfer);
  });
  /** Same message to every worker (copies; the last one gets the buffers), first answer. */
  const ask = (id, msg, transfer = []) =>
    Promise.all(workers.map((w, i) => askOne(w, id, msg, i === workers.length - 1 ? transfer : []))).then((r) => r[0]);
  const broadcast = (msg) => workers.forEach((w) => w.postMessage(msg));

  // "x,z" -> {x, z, key, state, heights, stale, etagH, etagC, tiles: [{tx, tz, step, mesh, building}]}
  const regions = new Map();
  let dim = null, bounds = null, active = false, frame = 0, loading = 0, building = 0, buildId = 0;
  let markers = [];

  function requestRender() {
    if (active && !frame) frame = requestAnimationFrame(draw);
  }

  function resize() {
    const w = container.clientWidth, h = container.clientHeight;
    renderer.setSize(w, h);
    camera.aspect = w / Math.max(h, 1);
    camera.updateProjectionMatrix();
    requestRender();
  }
  new ResizeObserver(resize).observe(container);

  /** y of the top face of the block column at (x, z), null if unknown. */
  function heightAt(x, z) {
    const r = regions.get(`${Math.floor(x / SIZE)},${Math.floor(z / SIZE)}`);
    if (!r || !r.heights) return null;
    const h = r.heights[(Math.floor(z) - r.z * SIZE) * SIZE + Math.floor(x) - r.x * SIZE];
    return h === NONE ? null : h;
  }

  // ----- loading -----

  async function fetchTagged(url, etag) {
    const res = await fetch(url, { cache: "no-cache" });
    if (!res.ok) throw new Error(res.status);
    const tag = res.headers.get("etag");
    if (etag && tag === etag) return { same: true };
    return { tag, buffer: await res.arrayBuffer() };
  }

  async function load(r) {
    r.state = "loading";
    loading++;
    const d = dim, textured = !!stamp;
    try {
      const [h, c, v] = await Promise.all([
        fetchTagged(`tiles/${d}/h/${r.x}_${r.z}.png`, r.etagH),
        fetchTagged(`tiles/${d}/${textured ? "b" : "0"}/${r.x}_${r.z}.png`, r.etagC),
        // block stacks (real blocks in 3D); missing for maps rendered without textures
        textured ? fetchTagged(`tiles/${d}/v/${r.x}_${r.z}.bin`, r.etagV).catch(() => ({ same: true })) : { same: true },
      ]);
      if (d !== dim || regions.get(`${r.x},${r.z}`) !== r) return; // dimension changed meanwhile
      if (h.same && c.same && v.same) {
        r.state = r.heights ? "ready" : "empty";
        return;
      }
      const msg = { type: "load", key: r.key, heights: h.buffer, [textured ? "blocks" : "color"]: c.buffer, voxels: v.buffer };
      const res = await ask(`loaded:${r.key}`, msg, [h.buffer, c.buffer, v.buffer].filter(Boolean));
      if (d !== dim || regions.get(`${r.x},${r.z}`) !== r) return;
      r.etagH = h.tag || r.etagH;
      r.etagC = c.tag || r.etagC;
      r.etagV = v.tag || r.etagV;
      r.heights = res.heights;
      r.state = res.ok ? "ready" : "empty";
      for (const t of r.tiles) {
        t.step = 0; // rebuild
        if (!res.ok) dropMesh(t);
      }
      // the neighbours' edge tiles draw their walls against our blocks: rebuild them too
      for (const [dx, dz] of [[-1, 0], [1, 0], [0, -1], [0, 1]]) {
        const n = regions.get(`${r.x + dx},${r.z + dz}`);
        if (!n) continue;
        for (const t of n.tiles) {
          const edge = dx === -1 ? t.tx === TILES - 1 : dx === 1 ? t.tx === 0 : dz === -1 ? t.tz === TILES - 1 : t.tz === 0;
          if (edge) t.step = 0;
        }
      }
    } catch {
      r.state = "error";
    } finally {
      loading--;
      scheduleUpdate();
    }
  }

  /** The tiles north, south, west and east of tile `t` of region `r` (undefined if not loaded). */
  function neighbourTiles(r, t) {
    return [[0, -1], [0, 1], [-1, 0], [1, 0]].map(([dx, dz]) => {
      const tx = t.tx + dx, tz = t.tz + dz;
      const nr = regions.get(`${r.x + Math.floor(tx / TILES)},${r.z + Math.floor(tz / TILES)}`);
      return nr?.tiles[(((tz % TILES) + TILES) % TILES) * TILES + (((tx % TILES) + TILES) % TILES)];
    });
  }

  async function buildMesh(r, t, step) {
    t.building = true;
    building++;
    const id = ++buildId;
    const d = dim;
    try {
      const w = workers.reduce((a, b) => (b.busy < a.busy ? b : a));
      w.busy++;
      const m = await askOne(w, `mesh:${id}`, {
        type: "build", key: r.key, rx: r.x, rz: r.z, step, x0: t.tx * TILE, z0: t.tz * TILE, size: TILE, id,
        fine: neighbourTiles(r, t).map((n) => !!n && n.step === 1),
      }).finally(() => w.busy--);
      if (m.error) console.error("3D mesh", r.key, m.error);
      if (d !== dim || regions.get(`${r.x},${r.z}`) !== r) return;
      dropMesh(t);
      if (!m.opaque && !m.cutout && !m.water) return;
      // positions are in 1/16 block, relative to the region origin
      const group = new THREE.Group();
      group.position.set(r.x * SIZE, 0, r.z * SIZE);
      group.scale.setScalar(1 / 16);
      for (const part of ["opaque", "cutout", "water"]) {
        const p = m[part];
        if (!p) continue;
        const g = new THREE.BufferGeometry();
        g.setAttribute("position", new THREE.BufferAttribute(p.position, 3));
        g.setAttribute("color", new THREE.BufferAttribute(p.color, 3, true));
        g.setAttribute("uv", new THREE.BufferAttribute(p.uv, 2));
        g.setIndex(new THREE.BufferAttribute(p.index, 1));
        g.computeBoundingSphere();
        const mesh = new THREE.Mesh(g, materials[part]);
        if (part === "water") mesh.renderOrder = 1; // after everything it lets shine through
        group.add(mesh);
      }
      group.updateMatrixWorld(true);
      group.matrixAutoUpdate = false;
      t.mesh = group;
      scene.add(group);
      snapTarget();
      requestRender();
    } finally {
      t.building = false;
      building--;
      scheduleUpdate();
    }
  }

  function dropMesh(t) {
    if (!t.mesh) return;
    scene.remove(t.mesh);
    for (const m of t.mesh.children) m.geometry.dispose();
    t.mesh = null;
  }

  function unload(key, r) {
    r.tiles.forEach(dropMesh);
    broadcast({ type: "drop", key: r.key });
    regions.delete(key);
  }

  // ----- which regions, which detail -----

  let updateTimer = 0;
  function scheduleUpdate() {
    if (!updateTimer) updateTimer = setTimeout(() => { updateTimer = 0; update(); }, 50);
  }

  // fog and far plane move with the camera distance, so zooming out never hides everything
  function updateFog(radius) {
    const d = camera.position.distanceTo(controls.target);
    scene.fog.near = d + radius * 0.4;
    scene.fog.far = d + radius;
    camera.far = d + radius * 1.2;
    camera.near = Math.max(0.5, d / 400);
    camera.updateProjectionMatrix();
  }

  function update() {
    if (!active || !bounds || stamp === null) return;
    const t = controls.target, cam = camera.position;
    const radius = Math.min(MAX_RADIUS, Math.max(1024, cam.distanceTo(t) * 2.5));
    updateFog(radius);
    const [minX, minZ, maxX, maxZ] = bounds;
    const wanted = [];
    for (let x = minX; x <= maxX; x++) {
      for (let z = minZ; z <= maxZ; z++) {
        // nearest point of the region to the camera
        const nx = Math.min(Math.max(cam.x, x * SIZE), (x + 1) * SIZE);
        const nz = Math.min(Math.max(cam.z, z * SIZE), (z + 1) * SIZE);
        const ground = Math.hypot(nx - t.x, nz - t.z);
        if (ground < radius) wanted.push({ x, z, key: `${x},${z}`, d: Math.hypot(nx - cam.x, nz - cam.z, t.y - cam.y) });
      }
    }
    wanted.sort((a, b) => a.d - b.d);
    const keep = new Set(wanted.map((w) => w.key));
    for (const [key, r] of regions) {
      if (!keep.has(key) && r.state !== "loading" && !r.tiles.some((tl) => tl.building)) unload(key, r);
    }

    let pending = false;
    const todo = [];
    for (const w of wanted) {
      let r = regions.get(w.key);
      if (!r) {
        r = { x: w.x, z: w.z, key: `${dim}/${w.key}`, state: "new", tiles: [] };
        for (let tz = 0; tz < TILES; tz++) for (let tx = 0; tx < TILES; tx++) r.tiles.push({ tx, tz, step: 0 });
        regions.set(w.key, r);
      }
      if (r.state === "new" || r.stale) {
        pending = true;
        if (loading < PARALLEL && r.state !== "loading") {
          r.stale = false;
          load(r);
        }
        continue;
      }
      if (r.state !== "ready") continue;
      for (const tl of r.tiles) {
        // nearest point of the tile to the camera
        const x0 = r.x * SIZE + tl.tx * TILE, z0 = r.z * SIZE + tl.tz * TILE;
        const nx = Math.min(Math.max(cam.x, x0), x0 + TILE), nz = Math.min(Math.max(cam.z, z0), z0 + TILE);
        const dist = Math.hypot(nx - cam.x, nz - cam.z, t.y - cam.y);
        const step = stepFor(dist);
        // near a threshold keep what is built: no rebuilding back and forth while moving
        if (!tl.dirty && (step === tl.step || (tl.step && stepFor(dist * 0.9) !== stepFor(dist * 1.1)))) continue;
        todo.push({ r, tl, step, d: Math.hypot(nx - cam.x, nz - cam.z) });
      }
    }
    // nearest tiles first
    todo.sort((a, b) => a.d - b.d);
    for (const { r, tl, step } of todo) {
      pending = true;
      if (building >= BUILDS) break;
      if (tl.building) continue;
      // full-detail tiles draw "just in case" faces only towards coarser neighbours: when this
      // tile starts or stops being full detail, its full-detail neighbours are rebuilt
      if ((tl.step === 1) !== (step === 1)) {
        for (const n of neighbourTiles(r, tl)) if (n && n.step === 1) n.dirty = true;
      }
      tl.step = step;
      tl.dirty = false;
      buildMesh(r, tl, step);
    }
    if (pending) scheduleUpdate();
    requestRender();
  }

  // The first time the ground under the view centre is known, put the orbit centre onto it
  let snapped = false;
  function snapTarget() {
    if (snapped) return;
    // the highest surface around the centre (not the bottom of a ravine right below it)
    let h = null;
    for (let dz = -6; dz <= 6; dz += 2) {
      for (let dx = -6; dx <= 6; dx += 2) {
        const v = heightAt(controls.target.x + dx, controls.target.z + dz);
        if (v != null && (h == null || v > h)) h = v;
      }
    }
    if (h == null) return;
    const dy = h - controls.target.y;
    controls.target.y += dy;
    camera.position.y += dy;
    controls.update();
    snapped = true;
    keepAboveGround();
  }

  // Never inside the terrain (zooming into a ravine would put the camera into the rock, from where
  // nothing is visible): at least 2 blocks above the highest block around the camera
  let lifting = false;
  function keepAboveGround() {
    if (lifting) return;
    const p = camera.position;
    let top = -Infinity;
    for (const [dx, dz] of [[0, 0], [-1, 0], [1, 0], [0, -1], [0, 1]]) {
      const h = heightAt(p.x + dx, p.z + dz);
      if (h != null && h > top) top = h;
    }
    if (!Number.isFinite(top) || p.y >= top + 2) return;
    lifting = true;
    p.y = top + 2;
    controls.update();
    lifting = false;
  }

  // ----- markers -----

  function setMarkers(list) {
    overlay.replaceChildren();
    markers = list.map((m) => {
      const wrap = document.createElement("div");
      wrap.className = "marker3d";
      wrap.append(makeMarker(m.name, m.kind));
      overlay.append(wrap);
      return { el: wrap, x: m.x, y: m.y, z: m.z };
    });
    requestRender();
  }

  const v = new THREE.Vector3();
  function placeMarkers() {
    const w = container.clientWidth, h = container.clientHeight;
    for (const m of markers) {
      const y = m.y ?? heightAt(m.x, m.z) ?? controls.target.y;
      v.set(m.x, y, m.z).project(camera);
      const visible = v.z < 1 && Math.abs(v.x) < 1.2 && Math.abs(v.y) < 1.2;
      m.el.style.display = visible ? "" : "none";
      if (visible) m.el.style.transform = `translate(${(v.x + 1) / 2 * w}px, ${(1 - v.y) / 2 * h}px)`;
    }
  }

  // ----- rendering & input -----

  function draw() {
    frame = 0;
    renderer.render(scene, camera);
    placeMarkers();
  }

  controls.addEventListener("change", () => {
    keepAboveGround();
    requestRender();
    scheduleUpdate();
    onChange?.();
  });

  // Hover coordinates: march along the mouse ray over the height data (much cheaper than
  // ray-casting millions of triangles)
  const raycaster = new THREE.Raycaster();
  const pointer = new THREE.Vector2();
  let hoverTimer = 0;
  renderer.domElement.addEventListener("pointermove", (e) => {
    if (hoverTimer) return;
    hoverTimer = setTimeout(() => { hoverTimer = 0; }, 60);
    const rect = renderer.domElement.getBoundingClientRect();
    pointer.set(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
    raycaster.setFromCamera(pointer, camera);
    const { origin, direction } = raycaster.ray;
    for (let s = 0, dt = 0.5; s < camera.far; s += dt, dt = Math.min(dt * 1.01, 4)) {
      const x = origin.x + direction.x * s, y = origin.y + direction.y * s, z = origin.z + direction.z * s;
      const h = heightAt(x, z);
      if (h != null && y <= h) {
        onHover?.(x, z, h);
        return;
      }
    }
  });

  // ----- public API -----

  function clear() {
    for (const [key, r] of regions) unload(key, r);
    broadcast({ type: "clear" });
  }

  return {
    /** Show a dimension around (x, z); `distance` = camera distance in blocks; `assetsStamp` =
     *  maps.json "blocks" (texture tables) or "". */
    show(slug, dimBounds, x, z, distance, assetsStamp = "") {
      if (slug !== dim) clear();
      setAssets(assetsStamp);
      dim = slug;
      setSky(slug);
      bounds = dimBounds;
      active = true;
      container.classList.remove("hidden");
      snapped = false;
      const y = heightAt(x, z) ?? 64;
      controls.target.set(x, y, z);
      // looking from the south, 55° down
      camera.position.set(x, y + distance * 0.82, z + distance * 0.57);
      controls.update();
      resize();
      update();
    },
    hide() {
      active = false;
      container.classList.add("hidden");
    },
    /** A new render happened: re-check loaded regions (unchanged tiles are skipped via ETag). */
    refresh(dimBounds, assetsStamp = "") {
      bounds = dimBounds;
      setAssets(assetsStamp);
      for (const r of regions.values()) if (r.state !== "loading") r.stale = true;
      scheduleUpdate();
    },
    setMarkers,
    get center() {
      return { x: controls.target.x, z: controls.target.z, distance: camera.position.distanceTo(controls.target) };
    },
  };
}
