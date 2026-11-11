// 3D view in blocks: every block column of the map is drawn as a box (top at its height, sides down
// to the lower neighbour), built from the 16-bit height tiles and either the block tiles + texture
// atlas (real Minecraft textures, when mapgen had the client jar) or the colour tiles.
// Decoding and meshing run in web workers (map3d-worker.js, a small pool; every worker gets every
// region, the meshes are spread over them).
// Loaded on demand (dynamic import from map.js).
//
// World units = blocks: x = block x, y = height, z = block z. Regions (512×512 blocks) are loaded
// around the camera and split into 4×4 tiles of 128 blocks; each tile is one mesh whose detail
// depends on its distance and on the window size (far away, several blocks are merged into bigger
// boxes and from step 4 the whole region is one mesh; outside the view half the detail).
// Options (setOptions): water / plants / leaves / fog can be hidden without a rebuild (own meshes),
// a height slice {lo, hi} clips everything outside lo <= y < hi at once (clipping planes) and then
// rebuilds the tiles with cut faces on top; while it is on, the stacks with every cave (`c/`,
// maps.json "caves") are loaded instead of `v/`. WASD / arrow keys fly in the direction the camera
// looks, Space / Shift up and down; the mouse wheel sets the flight speed while a fly key is held.

import * as THREE from "three";
import { MapControls } from "three/addons/controls/MapControls.js";
import { disposeHead, disposeModel, headModel, playerModel } from "./player3d.js";

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
const stepFor = (d) => (d < 420 ? 1 : d < 800 ? 2 : d < 1600 ? 4 : d < 3000 ? 8 : 16);
/** Tiles this close keep full detail even outside the view (turning / flying never shows them coarse). */
const NEAR_FULL = 192;
/** While flying: tiles are also measured from where the camera will be this many seconds ahead. */
const FLY_AHEAD = 1.5;
/** From this step on a whole region is one mesh (fewer draw calls far away). */
const WHOLE_FROM = 4;

/** BufferAttribute upload callback: drop the CPU copy. */
function freeArray() {
  this.array = null;
}

// The tile colors are final sRGB values: pass them (and the fog colour) through unchanged
THREE.ColorManagement.enabled = false;

export const FLY_SPEED = { min: 2, max: 2000, default: 40 }; // blocks per second

export function create({ container, makeMarker, onHover, onChange, onSpeed }) {
  const renderer = new THREE.WebGLRenderer({ antialias: true });
  renderer.localClippingEnabled = true;
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
  // Textures repeat across a quad (the worker merges equal neighbouring faces into one quad): per
  // vertex the texture index (`tex`) and coordinates in texture widths (`uv`, 0..n); the shader
  // picks the atlas cell and wraps with fract(). textureGrad keeps the mip level of the unwrapped
  // coordinates, so the wrap shows no seams. Atlas layout: see atlasTexture.
  const atlasInfo = { cols: { value: 1 }, size: { value: new THREE.Vector2(32, 32) } };
  const repeatTextures = (shader) => {
    shader.uniforms.atlasCols = atlasInfo.cols;
    shader.uniforms.atlasSize = atlasInfo.size;
    shader.vertexShader = shader.vertexShader
      .replace("#include <common>", "#include <common>\nattribute float tex;\nflat varying float vTex;\nvarying vec2 vLocal;")
      .replace("#include <uv_vertex>", "#include <uv_vertex>\nvTex = tex;\nvLocal = uv;");
    shader.fragmentShader = shader.fragmentShader
      .replace("#include <common>", "#include <common>\nuniform float atlasCols;\nuniform vec2 atlasSize;\nflat varying float vTex;\nvarying vec2 vLocal;")
      .replace("#include <map_fragment>", `
        float cellIndex = floor(vTex + 0.5);
        vec2 cell = vec2(mod(cellIndex, atlasCols), floor(cellIndex / atlasCols));
        vec2 cellSize = 16.0 / atlasSize;
        vec2 atlasUv = (cell * 32.0 + 8.0) / atlasSize + fract(vLocal) * cellSize;
        diffuseColor *= textureGrad(map, atlasUv, dFdx(vLocal) * cellSize, dFdy(vLocal) * cellSize);`);
  };
  for (const m of [material, cutoutMaterial, waterMaterial]) m.onBeforeCompile = repeatTextures;
  // plants and leaves are separate meshes (so they can be hidden) with the cutout material
  const materials = { opaque: material, cutout: cutoutMaterial, water: waterMaterial, plants: cutoutMaterial, leaves: cutoutMaterial };
  const PARTS = Object.keys(materials);
  const options = { water: true, plants: true, leaves: true, fog: true, slice: null, caves: false, flySpeed: FLY_SPEED.default };
  /** Which stack files to load: every cave only while the height slice is on (else hidden anyway). */
  const voxelSet = () => (options.caves && options.slice ? "c" : "v");
  // height slice: keep lo <= y <= hi (a hair above hi, so the cut faces at hi survive)
  const clipTop = new THREE.Plane(new THREE.Vector3(0, -1, 0), 0);
  const clipBottom = new THREE.Plane(new THREE.Vector3(0, 1, 0), 0);
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
    atlasInfo.cols.value = 1;
    atlasInfo.size.value.set(32, 32); // plain white: any cell samples the 1×1 texture
    if (next) {
      try {
        const res = await fetch(`atlas.png?v=${encodeURIComponent(next)}`);
        const bitmap = await createImageBitmap(await res.blob(), { imageOrientation: "none", premultiplyAlpha: "none" });
        const cols = bitmap.width / 16, rows = bitmap.height / 16;
        tex = atlasTexture(bitmap);
        atlasInfo.cols.value = cols;
        atlasInfo.size.value.set(cols * 32, rows * 32);
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
  // Looking around is our own (`look` below): the camera turns about itself like a player, the
  // orbit centre (used for pan/zoom/detail) is carried along in front of it. MapControls only pans
  // (left drag) and zooms (wheel); its polar limits allow any view direction from straight down
  // to almost straight up.
  controls.enableRotate = false;
  controls.minPolarAngle = 0.01;
  controls.maxPolarAngle = Math.PI - 0.01;
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

  /** y of the top face of the block column at (x, z), null if unknown (with a height slice: at
   *  most its top, so the camera can go down into the slice). */
  function heightAt(x, z) {
    const r = regions.get(`${Math.floor(x / SIZE)},${Math.floor(z / SIZE)}`);
    if (!r || !r.heights) return null;
    const h = r.heights[(Math.floor(z) - r.z * SIZE) * SIZE + Math.floor(x) - r.x * SIZE];
    if (h === NONE) return null;
    return options.slice ? Math.min(h, options.slice.hi) : h;
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
    const d = dim, textured = !!stamp, set = voxelSet();
    if (r.voxelSet !== set) r.etagV = null; // other stack files: fetch them even if unchanged
    try {
      const [h, c, v] = await Promise.all([
        fetchTagged(`tiles/${d}/h/${r.x}_${r.z}.png`, r.etagH),
        fetchTagged(`tiles/${d}/${textured ? "b" : "0"}/${r.x}_${r.z}.png`, r.etagC),
        // block stacks (real blocks in 3D); missing for maps rendered without textures
        textured ? fetchTagged(`tiles/${d}/${set}/${r.x}_${r.z}.bin`, r.etagV).catch(() => ({ same: true })) : { same: true },
      ]);
      r.voxelSet = set;
      if (d !== dim || regions.get(`${r.x},${r.z}`) !== r) return; // dimension changed meanwhile
      if (h.same && c.same && v.same) {
        r.state = r.heights ? "ready" : "empty";
        return;
      }
      // one worker decodes, the others adopt its result (no decoding the same files three times)
      const msg = { type: "load", key: r.key, heights: h.buffer, [textured ? "blocks" : "color"]: c.buffer, voxels: v.buffer };
      const w = workers.reduce((a, b) => (b.busy < a.busy ? b : a));
      w.busy++;
      const res = await askOne(w, `loaded:${r.key}`, msg, [h.buffer, c.buffer, v.buffer].filter(Boolean)).finally(() => w.busy--);
      for (const o of workers) if (o !== w) o.postMessage({ type: "adopt", key: r.key, entry: res.entry });
      if (d !== dim || regions.get(`${r.x},${r.z}`) !== r) return;
      r.etagH = h.tag || r.etagH;
      r.etagC = c.tag || r.etagC;
      r.etagV = v.tag || r.etagV;
      r.heights = res.heights;
      r.state = res.ok ? "ready" : "empty";
      for (const t of meshesOf(r)) {
        t.step = 0; // rebuild
        if (!res.ok) dropMesh(t);
      }
      // the neighbours' edge tiles draw their walls against our blocks: rebuild them too
      for (const [dx, dz] of [[-1, 0], [1, 0], [0, -1], [0, 1]]) {
        const n = regions.get(`${r.x + dx},${r.z + dz}`);
        if (!n) continue;
        if (n.whole) n.whole.step = 0;
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

  /** All mesh slots of a region: its 16 tiles and the whole-region mesh (if any). */
  const meshesOf = (r) => (r.whole ? [...r.tiles, r.whole] : r.tiles);
  /** Are all 16 tiles built? `current` = the tile whose build is just finishing (still `building`). */
  const tilesReady = (r, current) => r.tiles.every((t) => t.step && (t === current || !t.building));

  /** Never both levels at once: while the whole-region mesh is there its tiles stay hidden (they
   *  replace it in one go when all are built), else the tiles show. */
  function syncWhole(r) {
    const whole = !!r.whole?.mesh;
    for (const t of r.tiles) if (t.mesh) t.mesh.visible = !whole;
    requestRender();
  }

  /** Build one mesh slot: a tile of TILE blocks, or with `t.whole` the whole region (far away). */
  async function buildMesh(r, t, step) {
    t.building = true;
    building++;
    const id = ++buildId;
    const d = dim;
    try {
      const w = workers.reduce((a, b) => (b.busy < a.busy ? b : a));
      w.busy++;
      const m = await askOne(w, `mesh:${id}`, {
        type: "build", key: r.key, rx: r.x, rz: r.z, step, id, slice: options.slice,
        x0: t.whole ? 0 : t.tx * TILE, z0: t.whole ? 0 : t.tz * TILE, size: t.whole ? SIZE : TILE,
        fine: t.whole ? [false, false, false, false] : neighbourTiles(r, t).map((n) => !!n && n.step === 1),
      }).finally(() => w.busy--);
      if (m.error) console.error("3D mesh", r.key, m.error);
      if (d !== dim || regions.get(`${r.x},${r.z}`) !== r) return;
      dropMesh(t);
      if (t.whole) {
        if (r.mode === "whole") {
          // the region is one mesh now: its tiles go (and are rebuilt when it comes closer)
          for (const tl of r.tiles) {
            dropMesh(tl);
            tl.step = 0;
          }
        } else if (tilesReady(r)) {
          r.whole = null; // came back closer meanwhile and the tiles are there already
          syncWhole(r);
          return;
        }
      } else if (r.whole && r.mode !== "whole" && tilesReady(r, t)) {
        dropMesh(r.whole); // the tiles replace the whole-region mesh
        r.whole = null;
        syncWhole(r);
      }
      if (!PARTS.some((part) => m[part])) return;
      // positions are in 1/16 block, relative to the region origin
      const group = new THREE.Group();
      group.position.set(r.x * SIZE, 0, r.z * SIZE);
      group.scale.setScalar(1 / 16);
      for (const part of PARTS) {
        const p = m[part];
        if (!p) continue;
        const g = new THREE.BufferGeometry();
        g.setAttribute("position", new THREE.BufferAttribute(p.position, 3));
        g.setAttribute("color", new THREE.BufferAttribute(p.color, 3, true));
        g.setAttribute("uv", new THREE.BufferAttribute(p.uv, 2));
        g.setAttribute("tex", new THREE.BufferAttribute(p.tex, 1));
        g.setIndex(new THREE.BufferAttribute(p.index, 1));
        g.computeBoundingSphere();
        // once on the GPU the arrays are not needed here any more (nothing reads them back)
        for (const a of [...Object.values(g.attributes), g.index]) a.onUpload(freeArray);
        const mesh = new THREE.Mesh(g, materials[part]);
        mesh.userData.part = part;
        mesh.visible = options[part] !== false;
        if (part === "water") mesh.renderOrder = 1; // after everything it lets shine through
        group.add(mesh);
      }
      group.updateMatrixWorld(true);
      group.matrixAutoUpdate = false;
      t.mesh = group;
      scene.add(group);
      syncWhole(r);
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
    meshesOf(r).forEach(dropMesh);
    broadcast({ type: "drop", key: r.key });
    regions.delete(key);
  }

  // ----- which regions, which detail -----

  const frustum = new THREE.Frustum(), projView = new THREE.Matrix4(), box = new THREE.Box3();
  let updateTimer = 0;
  function scheduleUpdate() {
    if (!updateTimer) updateTimer = setTimeout(() => { updateTimer = 0; update(); }, 50);
  }

  // fog and far plane move with the camera distance, so zooming out never hides everything
  function updateFog(radius) {
    const d = camera.position.distanceTo(controls.target);
    scene.fog.near = options.fog ? d + radius * 0.4 : 1e7;
    scene.fog.far = options.fog ? d + radius : 2e7;
    camera.far = d + radius * 1.2;
    camera.near = Math.max(0.5, d / 400);
    camera.updateProjectionMatrix();
  }

  // ----- player heads with their own skin (heads.json) -----
  // A box with the skin over the map's Steve head, for heads within HEAD_NEAR blocks of the camera.

  const HEAD_NEAR = 160, HEAD_MAX = 300;
  let heads = [];
  const headModels = new Map(); // "x,y,z" -> group

  function setHeads(list) {
    heads = Array.isArray(list) ? list : [];
    placeHeads();
  }

  function placeHeads() {
    const cam = camera.position, s = options.slice;
    const want = new Map();
    for (const h of heads) {
      if (s && (h.y < s.lo || h.y >= s.hi)) continue;
      const d = Math.hypot(h.x + 0.5 - cam.x, h.y - cam.y, h.z + 0.5 - cam.z);
      if (d < HEAD_NEAR) want.set(`${h.x},${h.y},${h.z}`, [d, h]);
    }
    const keep = new Set([...want].sort((a, b) => a[1][0] - b[1][0]).slice(0, HEAD_MAX).map(([k]) => k));
    for (const [k, g] of headModels) {
      if (!keep.has(k) || g.userData.skin !== want.get(k)[1].skin) {
        scene.remove(g);
        disposeHead(g);
        headModels.delete(k);
      }
    }
    for (const k of keep) {
      if (headModels.has(k)) continue;
      const h = want.get(k)[1];
      const g = headModel(`heads/${h.skin}.png`, requestRender);
      g.userData.skin = h.skin;
      // the block model's box (front north before turning): on the floor 8 px high in the middle,
      // on a wall 4 px up and against the wall behind it; rot turns clockwise seen from above
      const r = (h.rot * Math.PI) / 180;
      const back = h.wall ? 0.25 : 0;
      g.position.set(h.x + 0.5 - back * Math.sin(r), h.y + (h.wall ? 0.5 : 0.25), h.z + 0.5 + back * Math.cos(r));
      g.rotation.y = Math.PI - r;
      scene.add(g);
      headModels.set(k, g);
    }
  }

  function clearHeads() {
    for (const g of headModels.values()) {
      scene.remove(g);
      disposeHead(g);
    }
    headModels.clear();
    heads = [];
  }

  // ----- banner patterns and pot sherds (decor.json) -----
  // Quads over the block models' flag faces / pot sides with atlas textures (the cutout material's
  // shader), within HEAD_NEAR blocks: farther away the tiles show no block models. One mesh, rebuilt
  // when the set in range changes. Positions follow the models in mapgen assets.rs (`entity_models`,
  // front on the north side, `rot` clockwise seen from above) in 1/16 block.

  let decor = [];
  let decorMesh = null, decorKey = "";

  function setDecor(list) {
    decor = Array.isArray(list) ? list : [];
    decorKey = "";
    placeDecor();
  }

  // face shading as in the worker (SHADE_NS / SHADE_EW)
  const SHADE_NS = 0.8, SHADE_EW = 0.62;

  /** Quads of one decoration: [corners (4 × [x, y, z] in 1/16 block, model space), tex, rgb]. The
   *  corners run top-left, top-right, bottom-right, bottom-left as seen from the front of the face. */
  function decorQuads(d) {
    const out = [];
    // a vertical face on the plane z (facing north: seen from -z, its left is +x) or x
    const north = (z, x0, x1, y0, y1) => [[x1, y1, z], [x0, y1, z], [x0, y0, z], [x1, y0, z]];
    const south = (z, x0, x1, y0, y1) => [[x0, y1, z], [x1, y1, z], [x1, y0, z], [x0, y0, z]];
    const east = (x, z0, z1, y0, y1) => [[x, y1, z1], [x, y1, z0], [x, y0, z0], [x, y0, z1]];
    const west = (x, z0, z1, y0, y1) => [[x, y1, z0], [x, y1, z1], [x, y0, z1], [x, y0, z0]];
    const e = 0.06; // 1/16 px per layer in front of the face below
    if (d.banner) {
      // flag: standing x 1..15, y 2..28, z 6..7; on a wall y -13..12, z 13..14
      const [y0, y1, z0, z1] = d.wall ? [-13, 12, 13, 14] : [2, 28, 6, 7];
      d.banner.forEach(([front, back, hex], i) => {
        const rgb = parseInt(hex, 16), off = e * (i + 1);
        out.push([north(z0 - off, 1, 15, y0, y1), front, rgb]);
        out.push([south(z1 + off, 1, 15, y0, y1), back, rgb]);
      });
    }
    if (d.pot) {
      // body x/z 1..15, y 0..16; back, left, right, front (left/right as seen from the front)
      const [back, left, right, front] = d.pot;
      if (back) out.push([south(15 + e, 1, 15, 0, 16), back, 0xffffff]);
      if (left) out.push([east(15 + e, 1, 15, 0, 16), left, 0xffffff]);
      if (right) out.push([west(1 - e, 1, 15, 0, 16), right, 0xffffff]);
      if (front) out.push([north(1 - e, 1, 15, 0, 16), front, 0xffffff]);
    }
    return out;
  }

  function placeDecor() {
    const cam = camera.position, s = options.slice;
    const near = decor.filter((d) => (!s || (d.y >= s.lo && d.y < s.hi))
      && Math.hypot(d.x + 0.5 - cam.x, d.y - cam.y, d.z + 0.5 - cam.z) < HEAD_NEAR);
    const key = near.map((d) => `${d.x},${d.y},${d.z}`).join(";");
    if (key === decorKey) return;
    decorKey = key;
    clearDecorMesh();
    const quads = [];
    for (const d of near) {
      const r = (((d.rot || 0) % 360) + 360) % 360 / 90;
      // model space (front north) -> world: quarter turns clockwise seen from above
      const turn = ([x, y, z]) => {
        for (let k = 0; k < r; k++) [x, z] = [16 - z, x];
        return [d.x + x / 16, d.y + y / 16, d.z + z / 16];
      };
      for (const [corners, tex, rgb] of decorQuads(d)) quads.push([corners.map(turn), tex, rgb]);
    }
    if (!quads.length) return;
    const n = quads.length;
    const position = new Float32Array(n * 12), uv = new Float32Array(n * 8), tex = new Float32Array(n * 4);
    const color = new Uint8Array(n * 12), index = new Uint32Array(n * 6);
    const UV = [0, 0, 1, 0, 1, 1, 0, 1];
    quads.forEach(([corners, t, rgb], q) => {
      // shade by the direction the face looks: along z (north / south) or along x
      const [a, b, c] = corners;
      const ux = b[0] - a[0], uz = b[2] - a[2], vy = c[1] - b[1];
      const nx = -uz * vy, nz = ux * vy; // horizontal normal (sign does not matter)
      const shade = Math.abs(nz) >= Math.abs(nx) ? SHADE_NS : SHADE_EW;
      const col = [(rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255].map((v) => Math.round(v * shade));
      corners.forEach((p, k) => {
        position.set(p, q * 12 + k * 3);
        color.set(col, q * 12 + k * 3);
        tex[q * 4 + k] = t;
      });
      uv.set(UV, q * 8);
      index.set([0, 1, 2, 0, 2, 3].map((i) => q * 4 + i), q * 6);
    });
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(position, 3));
    g.setAttribute("color", new THREE.BufferAttribute(color, 3, true));
    g.setAttribute("uv", new THREE.BufferAttribute(uv, 2));
    g.setAttribute("tex", new THREE.BufferAttribute(tex, 1));
    g.setIndex(new THREE.BufferAttribute(index, 1));
    g.computeBoundingSphere();
    decorMesh = new THREE.Mesh(g, cutoutMaterial);
    scene.add(decorMesh);
    requestRender();
  }

  function clearDecorMesh() {
    if (!decorMesh) return;
    scene.remove(decorMesh);
    decorMesh.geometry.dispose();
    decorMesh = null;
  }

  function clearDecor() {
    clearDecorMesh();
    decor = [];
    decorKey = "";
  }

  function update() {
    if (!active || !bounds || stamp === null) return;
    placeHeads();
    placeDecor();
    const t = controls.target, cam = camera.position;
    // what the camera sees: tiles outside get half the detail and come last
    camera.updateMatrixWorld();
    projView.multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse);
    frustum.setFromProjectionMatrix(projView);
    const seen = (x0, z0, size) => {
      box.min.set(x0 - 24, -64, z0 - 24);
      box.max.set(x0 + size + 24, 320, z0 + size + 24);
      return frustum.intersectsBox(box);
    };
    // the step thresholds are for a ~900 px high view: a bigger window gets more detail (a smaller
    // one not less: the near blocks would turn into coarse boxes right in front of the camera)
    const scale = Math.min(2, Math.max(1, container.clientHeight / 900));
    const want = (dist, visible) => {
      const s = stepFor(dist / scale);
      return visible || dist < NEAR_FULL ? s : Math.min(16, Math.max(2, s * 2));
    };
    // flying: also the spot the camera reaches soon, so the tiles ahead are built in time
    const flying = performance.now() - flyMoved < 300;
    const ahead = flying ? cam.clone().addScaledVector(flyVelocity, FLY_AHEAD) : null;
    const near = (x0, z0, size, h) => {
      const d = (p) => Math.hypot(Math.min(Math.max(p.x, x0), x0 + size) - p.x, Math.min(Math.max(p.z, z0), z0 + size) - p.z, h);
      return ahead ? Math.min(d(cam), d(ahead)) : d(cam);
    };
    // height of the camera above the ground (for the distance to tiles)
    const above = Math.max(0, cam.y - (groundBelow(cam) ?? t.y));
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
      if (!keep.has(key) && r.state !== "loading" && !meshesOf(r).some((tl) => tl.building)) unload(key, r);
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
      // slice switched on/off while every-cave stacks exist: the other stack files are needed
      if (stamp && r.state === "ready" && r.voxelSet !== voxelSet()) r.stale = true;
      if (r.state === "new" || r.stale) {
        pending = true;
        if (loading < PARALLEL && r.state !== "loading") {
          r.stale = false;
          load(r);
        }
        continue;
      }
      if (r.state !== "ready") continue;
      // far away the whole region is one mesh (with ±10 % hysteresis on the switch)
      {
        const x0 = r.x * SIZE, z0 = r.z * SIZE;
        const nx = Math.min(Math.max(cam.x, x0), x0 + SIZE), nz = Math.min(Math.max(cam.z, z0), z0 + SIZE);
        const dist = near(x0, z0, SIZE, above), vis = seen(x0, z0, SIZE);
        r.mode = want(dist * (r.mode === "whole" ? 1.1 : 0.9), vis) >= WHOLE_FROM ? "whole" : "tiles";
        if (r.mode === "whole") {
          const W = (r.whole ||= { whole: true, step: 0 });
          const step = want(dist, vis);
          const settled = want(dist * 0.9, vis) === want(dist * 1.1, vis);
          if (!W.building && (W.dirty || !W.step || (step !== W.step && settled))) {
            todo.push({ r, tl: W, step, d: Math.hypot(nx - cam.x, nz - cam.z) + (vis ? 0 : 1e6) });
          }
          continue;
        }
      }
      for (const tl of r.tiles) {
        // nearest point of the tile to the camera
        const x0 = r.x * SIZE + tl.tx * TILE, z0 = r.z * SIZE + tl.tz * TILE;
        const nx = Math.min(Math.max(cam.x, x0), x0 + TILE), nz = Math.min(Math.max(cam.z, z0), z0 + TILE);
        const dist = near(x0, z0, TILE, above), vis = seen(x0, z0, TILE);
        const step = want(dist, vis);
        // near a threshold keep what is built: no rebuilding back and forth while moving
        if (!tl.dirty && (step === tl.step || (tl.step && want(dist * 0.9, vis) !== want(dist * 1.1, vis)))) continue;
        todo.push({ r, tl, step, d: Math.hypot(nx - cam.x, nz - cam.z) + (vis ? 0 : 1e6) });
      }
    }
    // what is in view first, nearest first
    todo.sort((a, b) => a.d - b.d);
    for (const { r, tl, step } of todo) {
      pending = true;
      if (building >= BUILDS) break;
      if (tl.building) continue;
      // full-detail tiles draw "just in case" faces only towards coarser neighbours: when this
      // tile starts or stops being full detail, its full-detail neighbours are rebuilt
      if (!tl.whole && (tl.step === 1) !== (step === 1)) {
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
  /** Highest block top at and around p (null if unknown). */
  function groundBelow(p) {
    let top = null;
    for (const [dx, dz] of [[0, 0], [-1, 0], [1, 0], [0, -1], [0, 1]]) {
      const h = heightAt(p.x + dx, p.z + dz);
      if (h != null && (top == null || h > top)) top = h;
    }
    return top;
  }

  let lifting = false;
  function keepAboveGround() {
    if (lifting) return;
    const p = camera.position;
    const top = groundBelow(p);
    if (top == null || p.y >= top + 2) return;
    lifting = true;
    p.y = top + 2;
    controls.update();
    lifting = false;
  }

  // ----- markers -----
  // HTML markers (name + head) over the canvas. Players with a skin also get a 3D model, shown
  // instead of the head while the camera is closer than MODEL_NEAR blocks (the name stays above).

  const MODEL_NEAR = 72;
  const models = new Map(); // player name -> {group, skin}

  function setMarkers(list) {
    overlay.replaceChildren();
    const seen = new Set();
    markers = list.map((m) => {
      const wrap = document.createElement("div");
      wrap.className = "marker3d";
      wrap.append(makeMarker(m.name, m.kind, m.skin, m));
      overlay.append(wrap);
      let model = null;
      if (m.kind === "player" && m.skin) {
        seen.add(m.name);
        let entry = models.get(m.name);
        if (entry && entry.skin !== m.skin) {
          scene.remove(entry.group);
          disposeModel(entry.group);
          entry = null;
        }
        if (!entry) {
          entry = { skin: m.skin, group: playerModel(`skins/${m.skin}.png`, requestRender) };
          entry.group.visible = false;
          scene.add(entry.group);
          models.set(m.name, entry);
        }
        model = entry.group;
      }
      return { el: wrap, x: m.x, y: m.y, z: m.z, model };
    });
    for (const [name, entry] of models) {
      if (!seen.has(name)) {
        scene.remove(entry.group);
        disposeModel(entry.group);
        models.delete(name);
      }
    }
    requestRender();
  }

  const v = new THREE.Vector3();
  function placeMarkers() {
    camera.updateMatrixWorld();
    const w = container.clientWidth, h = container.clientHeight;
    for (const m of markers) {
      let y = m.y ?? heightAt(m.x, m.z) ?? controls.target.y;
      if (m.model) {
        const near = options.players !== false && camera.position.distanceTo(v.set(m.x, y, m.z)) < MODEL_NEAR;
        m.model.visible = near;
        m.el.classList.toggle("near", near);
        if (near) {
          m.model.position.set(m.x, y, m.z);
          // no facing direction is known: turn towards the camera
          m.model.rotation.y = Math.atan2(camera.position.x - m.x, camera.position.z - m.z);
          y += 2; // the name above the model's head
        }
      }
      v.set(m.x, y, m.z).project(camera);
      const visible = v.z < 1 && Math.abs(v.x) < 1.2 && Math.abs(v.y) < 1.2;
      m.el.style.display = visible ? "" : "none";
      if (visible) m.el.style.transform = `translate(${(v.x + 1) / 2 * w}px, ${(1 - v.y) / 2 * h}px)`;
    }
  }

  // ----- rendering & input -----

  function draw() {
    frame = 0;
    placeMarkers(); // first: places the player models for this frame
    renderer.render(scene, camera);
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

  // ----- looking around (right drag, or Ctrl + left drag) -----
  // Turns the view direction about the camera's own position (yaw/pitch, like a player's head);
  // the orbit centre stays at the same distance in the new direction.

  const LOOK_SPEED = 0.005; // radians per pixel
  const MAX_PITCH = THREE.MathUtils.degToRad(85);
  const lookDir = new THREE.Vector3(), move = new THREE.Vector3();
  let look = null; // {id, x, y}
  renderer.domElement.addEventListener("pointerdown", (e) => {
    if (!(e.button === 2 || (e.button === 0 && e.ctrlKey)) || e.pointerType === "touch") return;
    e.preventDefault();
    e.stopImmediatePropagation(); // MapControls must not start a pan for Ctrl + left
    look = { id: e.pointerId, x: e.clientX, y: e.clientY };
    renderer.domElement.setPointerCapture(e.pointerId);
  }, { capture: true });
  renderer.domElement.addEventListener("pointermove", (e) => {
    if (!look || e.pointerId !== look.id) return;
    const dx = e.clientX - look.x, dy = e.clientY - look.y;
    look.x = e.clientX;
    look.y = e.clientY;
    turn(-dx * LOOK_SPEED, -dy * LOOK_SPEED);
  });
  const endLook = (e) => {
    if (look && e.pointerId === look.id) look = null;
  };
  renderer.domElement.addEventListener("pointerup", endLook);
  renderer.domElement.addEventListener("pointercancel", endLook);
  renderer.domElement.addEventListener("contextmenu", (e) => e.preventDefault());

  /** Turn the view by `dYaw` (left/right) and `dPitch` (up/down), keeping the camera in place. */
  function turn(dYaw, dPitch) {
    lookDir.subVectors(controls.target, camera.position);
    const dist = lookDir.length() || 1;
    let yaw = Math.atan2(lookDir.x, lookDir.z);
    let pitch = Math.asin(Math.max(-1, Math.min(1, lookDir.y / dist)));
    yaw += dYaw;
    pitch = Math.max(-MAX_PITCH, Math.min(MAX_PITCH, pitch + dPitch));
    lookDir.set(Math.sin(yaw) * Math.cos(pitch), Math.sin(pitch), Math.cos(yaw) * Math.cos(pitch));
    controls.target.copy(camera.position).addScaledVector(lookDir, dist);
    controls.update();
  }

  // ----- gliding (follow mode) -----
  // Moves orbit centre and camera together towards (x, y, z) over `ms` milliseconds, linearly, so
  // a followed player (positions every ~2 s) is tracked without jumps. View angle and distance stay.

  let glideFrame = 0;
  function glide(x, y, z, ms = 0) {
    cancelAnimationFrame(glideFrame);
    const from = controls.target.clone(), t0 = performance.now();
    const step = (now) => {
      const t = ms > 0 ? Math.min(1, (now - t0) / ms) : 1;
      move.set(from.x + (x - from.x) * t, from.y + (y - from.y) * t, from.z + (z - from.z) * t).sub(controls.target);
      controls.target.add(move);
      camera.position.add(move);
      controls.update();
      glideFrame = t < 1 && active ? requestAnimationFrame(step) : 0;
    };
    glideFrame = requestAnimationFrame(step);
  }
  // any drag by the user ends a glide (it would fight the pan)
  renderer.domElement.addEventListener("pointerdown", () => {
    cancelAnimationFrame(glideFrame);
    glideFrame = 0;
  });

  // ----- flying with the keyboard -----

  const FLY_KEYS = new Set(["KeyW", "KeyA", "KeyS", "KeyD", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Space", "ShiftLeft", "ShiftRight"]);
  // Any number of keys at once (e.g. W + D + Space). W/S move along the view direction (also up or
  // down when the camera is tilted), A/D sideways, Space/Shift straight up/down; camera and orbit
  // centre move together, so the view keeps its angle.
  const keys = new Set();
  let flyFrame = 0, flyLast = 0;
  // last flight velocity (blocks / s) and when the camera last moved by flying (for `update`)
  const flyVelocity = new THREE.Vector3();
  let flyMoved = -1e9;
  const fwd = new THREE.Vector3(), side = new THREE.Vector3();
  const typing = (e) => e.target instanceof Element && !!e.target.closest("input, select, textarea, [contenteditable]");
  window.addEventListener("keydown", (e) => {
    if (!active || e.ctrlKey || e.metaKey || e.altKey || !FLY_KEYS.has(e.code) || typing(e)) return;
    e.preventDefault(); // no page scrolling, no "click" on a focused button with Space
    if (document.activeElement instanceof HTMLButtonElement) document.activeElement.blur();
    keys.add(e.code);
    if (!flyFrame) {
      flyLast = performance.now();
      flyFrame = requestAnimationFrame(fly);
    }
  });
  window.addEventListener("keyup", (e) => {
    if (keys.delete(e.code)) e.preventDefault();
  });
  window.addEventListener("blur", () => keys.clear());
  document.addEventListener("visibilitychange", () => keys.clear());

  function fly(now) {
    flyFrame = 0;
    if (!keys.size || !active) return;
    const dt = Math.min(0.1, (now - flyLast) / 1000);
    flyLast = now;
    camera.updateMatrixWorld();
    fwd.setFromMatrixColumn(camera.matrixWorld, 2).negate().normalize(); // view direction
    side.setFromMatrixColumn(camera.matrixWorld, 0).normalize();          // screen right
    const has = (...codes) => codes.some((c) => keys.has(c));
    move.set(0, 0, 0);
    if (has("KeyW", "ArrowUp")) move.add(fwd);
    if (has("KeyS", "ArrowDown")) move.sub(fwd);
    if (has("KeyD", "ArrowRight")) move.add(side);
    if (has("KeyA", "ArrowLeft")) move.sub(side);
    if (has("Space")) move.y += 1;
    if (has("ShiftLeft", "ShiftRight")) move.y -= 1;
    if (move.lengthSq() > 0) {
      move.normalize().multiplyScalar(options.flySpeed * dt);
      flyVelocity.copy(move).multiplyScalar(dt > 0 ? 1 / dt : 0);
      flyMoved = now;
      camera.position.add(move);
      controls.target.add(move);
      // not into the ground: lift both, so flying along a slope keeps the view angle
      const ground = groundBelow(camera.position);
      if (ground != null && camera.position.y < ground + 2) {
        const dy = ground + 2 - camera.position.y;
        camera.position.y += dy;
        controls.target.y += dy;
      }
      controls.update();
    }
    flyFrame = requestAnimationFrame(fly);
  }

  // Mouse wheel while a fly key is held: flight speed instead of zoom (×1.25 per notch)
  container.addEventListener("wheel", (e) => {
    if (!active || !keys.size || !e.deltaY) return;
    e.preventDefault();
    e.stopPropagation(); // capture phase on the container: MapControls never sees it
    const notches = Math.sign(e.deltaY) * Math.min(4, Math.max(1, Math.abs(e.deltaY) / 100));
    const speed = options.flySpeed * Math.pow(1.25, -notches);
    options.flySpeed = Math.round(Math.min(FLY_SPEED.max, Math.max(FLY_SPEED.min, speed)) * 10) / 10;
    onSpeed?.(options.flySpeed);
  }, { capture: true, passive: false });

  // ----- options -----

  let sliceTimer = 0;
  function setOptions(next) {
    const sliceBefore = JSON.stringify(options.slice);
    Object.assign(options, next);
    for (const r of regions.values()) {
      for (const t of meshesOf(r)) {
        for (const m of t.mesh ? t.mesh.children : []) m.visible = options[m.userData.part] !== false;
      }
    }
    if (JSON.stringify(options.slice) !== sliceBefore) {
      const s = options.slice;
      if (s) {
        clipTop.constant = s.hi + 0.02;
        clipBottom.constant = -s.lo;
      }
      for (const m of new Set(Object.values(materials))) {
        m.clippingPlanes = s ? [clipTop, clipBottom] : null;
        m.needsUpdate = true;
      }
      keepAboveGround();
      // rebuild with cut faces once the slider rests
      clearTimeout(sliceTimer);
      sliceTimer = setTimeout(() => {
        for (const r of regions.values()) for (const t of meshesOf(r)) t.dirty = true;
        scheduleUpdate();
      }, 250);
    }
    if (active) update();
    requestRender();
  }

  // ----- public API -----

  function clear() {
    clearHeads();
    clearDecor();
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
    /** Player heads with their own skin in the shown dimension (heads.json of mapgen). */
    setHeads,
    /** Banner patterns and pot sherds in the shown dimension (decor.json of mapgen). */
    setDecor,
    setOptions,
    /** Move the view to (x, y, z) over `ms` milliseconds (follow mode), keeping angle and distance. */
    glide,
    /** Numbers for tuning (DevTools: state.view3d.stats). */
    get stats() {
      const steps = {}, parts = {};
      let tiles = 0;
      for (const r of regions.values()) {
        for (const t of meshesOf(r)) {
          if (!t.mesh) continue;
          tiles++;
          const s = t.whole ? `${t.step} (region)` : t.step;
          steps[s] = (steps[s] || 0) + 1;
          for (const m of t.mesh.children) {
            const k = `${m.userData.part}@${t.step}`;
            parts[k] = (parts[k] || 0) + m.geometry.index.count / 3;
          }
        }
      }
      const { calls, triangles } = renderer.info.render;
      return {
        regions: regions.size, tiles, steps, calls, triangles, parts,
        geometries: renderer.info.memory.geometries, workers: POOL, shared: self.crossOriginIsolated,
      };
    },
    get center() {
      return { x: controls.target.x, z: controls.target.z, distance: camera.position.distanceTo(controls.target) };
    },
  };
}
