// Web worker of the map viewer (used by both the 2D and the 3D view, one instance each):
// decodes region tiles, builds the 3D block meshes and draws the textured 2D tiles, so the page
// stays smooth.
//
// With block textures (mapgen ran with the Minecraft client: blocks.json, atlas.png, b/ and v/
// tiles) blocks are drawn like in the game: boxes with their textures, plants as crossed planes,
// vines on the walls they hang on, see-through leaves, translucent water, biome colours blended
// between neighbouring columns. Without them the map-item colours are used.
//
// Messages in:  {type: "init", stamp, base}             (texture tables version, "" = none;
//                base = URL of the viewer, all files are fetched relative to it)
//               {type: "load", key, heights?, color?, blocks?, voxels?}  (ArrayBuffers of the files)
//               {type: "build", key, rx, rz, step, x0, z0, size, id}  (one 3D tile of a region;
//                key prefix "<dim>/", neighbour regions are found by key)
//               {type: "tile2d", id, dim, v, x0, z0, scale}  (512 px 2D tile, scale = px per block)
//               {type: "drop", key} | {type: "clear"}
// Messages out: {type: "init", textured}
//               {type: "loaded", key, ok, heights}      (Int16Array, top face y, NONE = no data)
//               {type: "mesh", key, id, step, opaque?, cutout?, water?}  (each {position, color, uv, index};
//                positions in 1/16 block, relative to the region origin)
//               {type: "tile2d", id, bitmap}

"use strict";

const SIZE = 512;
const NONE = -32768;
const TEX = 16;
const U = 16; // mesh units per block

// Minecraft's base map colours, index = map colour id. Must match BASE in mapgen/src/colors.rs
// (checked by a cargo test).
// prettier-ignore
const PALETTE = [0,0,0,127,178,56,247,233,163,199,199,199,255,0,0,160,160,255,167,167,167,0,124,0,255,255,255,164,168,184,151,109,77,112,112,112,64,64,255,143,119,72,255,252,245,216,127,51,178,76,216,102,153,216,229,229,51,127,204,25,242,127,165,76,76,76,153,153,153,76,127,153,127,63,178,51,76,178,102,76,51,102,127,51,153,51,51,25,25,25,250,238,77,92,219,213,74,128,255,0,217,58,129,86,49,112,2,0,209,177,161,159,82,36,149,87,108,112,108,138,186,133,36,103,117,53,160,77,78,57,41,35,135,107,98,87,92,92,122,73,88,76,62,92,76,50,35,76,82,42,142,60,46,37,22,16,189,48,49,148,63,97,92,25,29,22,126,134,58,142,140,86,44,62,20,180,133,100,100,100,216,175,147,127,167,150];
const WATER = 12;

// Without textures the 2D colour tiles carry the map's slope shading (4 brightness levels); turn
// every shaded colour back into its base colour (water keeps its depth tint).
const UNSHADE = new Map();
for (const m of [255, 220, 180, 135]) {
  for (let c = 1; c < PALETTE.length / 3; c++) {
    if (c === WATER) continue;
    const [r, g, b] = [0, 1, 2].map((k) => Math.floor((PALETTE[c * 3 + k] * m) / 255));
    const key = (r << 16) | (g << 8) | b;
    if (!UNSHADE.has(key)) UNSHADE.set(key, c);
  }
}

// blocks.json tint bits
const TINT_TOP = 1, TINT_SIDE = 2, TINT_BOTTOM = 4;

/** Texture tables: {blocks, biomes, atlas: {w, h, data}, under, waterId} or null. */
let assets = null;
let assetsStamp = "";
let base = self.location.href;
const url = (path) => new URL(path, base).href;

/** key -> {h: Int16Array (top face y), c: Uint8Array (rgb), b: Uint16Array (block ids) | null,
 *  bio: Uint8Array | null, vox: {off, ys, ids} | null, tints: blended biome colours (lazy)} */
const store = new Map();

// ---------- decoding ----------

async function inflate(buffer) {
  const stream = new Blob([buffer]).stream().pipeThrough(new DecompressionStream("deflate"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

async function decodeHeights(buffer) {
  const b = new Uint8Array(buffer);
  const dv = new DataView(buffer);
  let p = 8, w = 0, h = 0, ok = false;
  const idat = [];
  while (p + 8 <= b.length) {
    const len = dv.getUint32(p);
    const type = String.fromCharCode(b[p + 4], b[p + 5], b[p + 6], b[p + 7]);
    if (type === "IHDR") {
      w = dv.getUint32(p + 8);
      h = dv.getUint32(p + 12);
      ok = b[p + 16] === 16 && b[p + 17] === 0 && b[p + 20] === 0; // 16 bit, gray, not interlaced
    } else if (type === "IDAT") {
      idat.push(b.subarray(p + 8, p + 8 + len));
    } else if (type === "IEND") {
      break;
    }
    p += 12 + len;
  }
  if (!ok || w !== SIZE || h !== SIZE) return null; // e.g. the 1×1 placeholder for missing tiles
  const total = idat.reduce((n, a) => n + a.length, 0);
  const joined = new Uint8Array(total);
  let o = 0;
  for (const a of idat) {
    joined.set(a, o);
    o += a.length;
  }
  const raw = await inflate(joined);
  const stride = w * 2;
  const out = new Int16Array(w * h);
  let prev = new Uint8Array(stride), cur = new Uint8Array(stride);
  for (let y = 0; y < h; y++) {
    const o = y * (stride + 1);
    const filter = raw[o];
    for (let i = 0; i < stride; i++) {
      const x = raw[o + 1 + i];
      const a = i >= 2 ? cur[i - 2] : 0;
      const up = prev[i];
      let v;
      switch (filter) {
        case 1: v = x + a; break;
        case 2: v = x + up; break;
        case 3: v = x + ((a + up) >> 1); break;
        case 4: {
          const c = i >= 2 ? prev[i - 2] : 0;
          const pa = Math.abs(up - c), pb = Math.abs(a - c), pc = Math.abs(a + up - 2 * c);
          v = x + (pa <= pb && pa <= pc ? a : pb <= pc ? up : c);
          break;
        }
        default: v = x;
      }
      cur[i] = v & 255;
    }
    for (let x = 0; x < w; x++) {
      const v = (cur[2 * x] << 8) | cur[2 * x + 1]; // y + 32768, 0 = no data
      out[y * w + x] = v ? v - 32768 + 1 : NONE;     // +1: the top face of the block
    }
    [prev, cur] = [cur, prev];
  }
  return out;
}

/** RGBA pixels of a PNG (exact values: no premultiplying, no colour conversion). */
async function pixels(buffer, size) {
  const bitmap = await createImageBitmap(new Blob([buffer]), { premultiplyAlpha: "none", colorSpaceConversion: "none" });
  if (size && (bitmap.width !== size || bitmap.height !== size)) {
    bitmap.close();
    return null;
  }
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  ctx.drawImage(bitmap, 0, 0);
  const data = ctx.getImageData(0, 0, bitmap.width, bitmap.height).data;
  const result = { w: bitmap.width, h: bitmap.height, data };
  bitmap.close();
  return result;
}

/** Map-colour tile -> base colours (no texture tables). */
async function decodeColors(buffer) {
  const img = await pixels(buffer, SIZE);
  if (!img) return null;
  const rgba = img.data;
  const out = new Uint8Array(SIZE * SIZE * 3);
  for (let i = 0, j = 0; i < rgba.length; i += 4, j += 3) {
    if (rgba[i + 3] === 0) continue;
    const c = UNSHADE.get((rgba[i] << 16) | (rgba[i + 1] << 8) | rgba[i + 2]);
    if (c) {
      out[j] = PALETTE[c * 3]; out[j + 1] = PALETTE[c * 3 + 1]; out[j + 2] = PALETTE[c * 3 + 2];
    } else {
      out[j] = rgba[i]; out[j + 1] = rgba[i + 1]; out[j + 2] = rgba[i + 2];
    }
  }
  return out;
}

/** Block tile (R, G = block id, B = biome id) -> ids, biomes and the tinted average colours. */
async function decodeBlocks(buffer) {
  const img = await pixels(buffer, SIZE);
  if (!img || !assets) return null;
  const rgba = img.data;
  const ids = new Uint16Array(SIZE * SIZE);
  const bio = new Uint8Array(SIZE * SIZE);
  const c = new Uint8Array(SIZE * SIZE * 3);
  for (let i = 0; i < ids.length; i++) {
    if (rgba[i * 4 + 3] === 0) continue;
    const id = (rgba[i * 4] << 8) | rgba[i * 4 + 1];
    ids[i] = id < assets.blocks.length ? id : 0;
    bio[i] = rgba[i * 4 + 2];
    const col = blockColor(ids[i], bio[i]);
    c[i * 3] = col[0]; c[i * 3 + 1] = col[1]; c[i * 3 + 2] = col[2];
  }
  return { ids, bio, c };
}

/** Block stacks ("MGV1", u8 count per column, then i16 y + u16 id per entry, zlib). */
async function decodeVoxels(buffer) {
  const raw = await inflate(buffer);
  if (raw.length < 4 + SIZE * SIZE || String.fromCharCode(...raw.subarray(0, 4)) !== "MGV1") return null;
  const off = new Uint32Array(SIZE * SIZE + 1);
  for (let i = 0; i < SIZE * SIZE; i++) off[i + 1] = off[i] + raw[4 + i];
  const n = off[SIZE * SIZE];
  const dv = new DataView(raw.buffer, raw.byteOffset + 4 + SIZE * SIZE);
  const ys = new Int16Array(n), ids = new Uint16Array(n);
  const max = assets ? assets.blocks.length : 0;
  for (let k = 0; k < n; k++) {
    ys[k] = dv.getInt16(k * 4, true);
    const id = dv.getUint16(k * 4 + 2, true);
    ids[k] = id < max ? id : 0;
  }
  return { off, ys, ids };
}

// ---------- texture tables, tints ----------

async function loadAssets(stamp) {
  if (stamp === assetsStamp) return;
  assetsStamp = stamp;
  assets = null;
  store.clear();
  regions2d.clear();
  tinted.clear();
  if (!stamp) return;
  try {
    const v = encodeURIComponent(stamp);
    const [json, atlasBuf] = await Promise.all([
      fetch(url(`blocks.json?v=${v}`)).then((r) => (r.ok ? r.json() : Promise.reject(r.status))),
      fetch(url(`atlas.png?v=${v}`)).then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(r.status))),
    ]);
    const atlas = await pixels(atlasBuf);
    if (!atlas || !json.blocks[0] || json.blocks[0].kind === undefined) return; // old table format
    const index = new Map(json.blocks.map((b, i) => [b.name, i]));
    // for fill walls below the known blocks: what is under grass/podzol/mycelium is dirt
    const dirt = json.blocks[index.get("minecraft:dirt")];
    const under = json.blocks.map((b) =>
      dirt && /^minecraft:(grass_block|podzol|mycelium|dirt_path)$/.test(b.name) ? dirt.side : b.side);
    const leaves = json.blocks.map((b) => b.name.endsWith("_leaves"));
    const full = json.blocks.map((b) => isFull(b.box || FULL_BOX));
    assets = { blocks: json.blocks, biomes: json.biomes, atlas, under, leaves, full, waterId: index.get("minecraft:water") || -1 };
  } catch (e) {
    console.error("map textures could not be loaded", e);
    assets = null;
  }
}

function tintOf(block, biome) {
  const t = block.tint;
  if (!t || t === "none") return null;
  if (typeof t === "object") return t.fixed;
  const b = assets.biomes[biome] || assets.biomes[0];
  return b[t] || null;
}

function blockColor(id, biome) {
  const b = assets.blocks[id];
  const t = tintOf(b, biome);
  return t ? [0, 1, 2].map((k) => (b.color[k] * t[k] / 255) | 0) : b.color;
}

/** Biome colours of one kind (grass / foliage / water) per column, blended over 5×5 columns like
 *  the game does (computed on first use, per region and kind). */
function blendedTints(entry, kind) {
  entry.tints ||= {};
  if (entry.tints[kind]) return entry.tints[kind];
  const R = 2;
  {
    const src = new Float32Array(SIZE * SIZE * 3), w = new Float32Array(SIZE * SIZE);
    for (let i = 0; i < SIZE * SIZE; i++) {
      if (entry.h[i] === NONE) continue;
      const c = (assets.biomes[entry.bio[i]] || assets.biomes[0])[kind];
      src[i * 3] = c[0]; src[i * 3 + 1] = c[1]; src[i * 3 + 2] = c[2];
      w[i] = 1;
    }
    // separable box blur (sum of colours and of weights)
    const pass = (s, sw, horizontal) => {
      const d = new Float32Array(s.length), dw = new Float32Array(sw.length);
      for (let a = 0; a < SIZE; a++) {
        for (let b = 0; b < SIZE; b++) {
          let r = 0, g = 0, bl = 0, n = 0;
          for (let k = -R; k <= R; k++) {
            const t = b + k;
            if (t < 0 || t >= SIZE) continue;
            const i = horizontal ? a * SIZE + t : t * SIZE + a;
            r += s[i * 3]; g += s[i * 3 + 1]; bl += s[i * 3 + 2]; n += sw[i];
          }
          const i = horizontal ? a * SIZE + b : b * SIZE + a;
          d[i * 3] = r; d[i * 3 + 1] = g; d[i * 3 + 2] = bl; dw[i] = n;
        }
      }
      return [d, dw];
    };
    const [h1, hw] = pass(src, w, true);
    const [v1, vw] = pass(h1, hw, false);
    const res = new Uint8Array(SIZE * SIZE * 3);
    for (let i = 0; i < SIZE * SIZE; i++) {
      const n = vw[i] || 1;
      res[i * 3] = v1[i * 3] / n; res[i * 3 + 1] = v1[i * 3 + 1] / n; res[i * 3 + 2] = v1[i * 3 + 2] / n;
    }
    entry.tints[kind] = res;
    return res;
  }
}

/** Tint of a block in a column (blended biome colour, fixed colour or white). */
function tintAt(block, entry, idx) {
  const t = block.tint;
  if (!t || t === "none") return WHITE_RGB;
  if (typeof t === "object") return t.fixed;
  const a = blendedTints(entry, t);
  return [a[idx * 3], a[idx * 3 + 1], a[idx * 3 + 2]];
}
const WHITE_RGB = [255, 255, 255];

/** Atlas UV rectangle of texture `i`, slightly inset so neighbours never bleed in. */
function uvRect(i) {
  return uvSub(i, 0, 0, TEX, TEX);
}
const WHITE = [0.5 / 512, 0.5 / 512, 0.5 / 512, 0.5 / 512]; // a texel of the white atlas tile

/** Part of texture `i` (pixels 0..16), for boxes smaller than a block (slabs, cactus).
 *  The 3D view uploads the atlas with every texture in a 32 px cell (8 px border), see
 *  atlasTexture in map3d.js. */
function uvSub(i, a, b, c, d) {
  const cols = assets.atlas.w / TEX, rows = assets.atlas.h / TEX;
  const W = cols * 32, H = rows * 32;
  const x = (i % cols) * 32 + 8, y = Math.floor(i / cols) * 32 + 8;
  const e = 0.02;
  return [(x + a + e) / W, (y + b + e) / H, (x + c - e) / W, (y + d - e) / H];
}

const FULL_BOX = [0, 0, 0, 16, 16, 16];
const isFull = (bx) => bx[0] === 0 && bx[1] === 0 && bx[2] === 0 && bx[3] === 16 && bx[4] === 16 && bx[5] === 16;

// ---------- 3D meshing ----------

/** Growable typed arrays for one mesh. */
class Buffers {
  constructor(quads) {
    this.cap = Math.max(quads, 256);
    this.n = 0;
    this.pos = new Int16Array(this.cap * 12);
    this.col = new Uint8Array(this.cap * 12);
    this.uv = new Float32Array(this.cap * 8);
  }
  grow() {
    this.cap *= 2;
    const pos = new Int16Array(this.cap * 12), col = new Uint8Array(this.cap * 12), uv = new Float32Array(this.cap * 8);
    pos.set(this.pos);
    col.set(this.col);
    uv.set(this.uv);
    this.pos = pos;
    this.col = col;
    this.uv = uv;
  }
  /** One quad: 4 corners (counter-clockwise seen from outside; for walls: top-start, bottom-start,
   *  bottom-end, top-end), one colour (0..255, clamped), UV rectangle [u0, v0, u1, v1]. */
  quad(c, r, g, b, t) {
    if (this.n === this.cap) this.grow();
    const o = this.n * 12;
    for (let k = 0; k < 12; k++) this.pos[o + k] = c[k];
    r = r > 255 ? 255 : r; g = g > 255 ? 255 : g; b = b > 255 ? 255 : b;
    for (let k = 0; k < 12; k += 3) {
      this.col[o + k] = r; this.col[o + k + 1] = g; this.col[o + k + 2] = b;
    }
    const u = this.n * 8;
    this.uv[u] = t[0]; this.uv[u + 1] = t[1];
    this.uv[u + 2] = t[0]; this.uv[u + 3] = t[3];
    this.uv[u + 4] = t[2]; this.uv[u + 5] = t[3];
    this.uv[u + 6] = t[2]; this.uv[u + 7] = t[1];
    this.n++;
  }
  result() {
    const quads = this.n;
    if (!quads) return null;
    const index = new Uint32Array(quads * 6);
    for (let k = 0; k < quads; k++) {
      const v = k * 4, o = k * 6;
      index[o] = v; index[o + 1] = v + 1; index[o + 2] = v + 2;
      index[o + 3] = v; index[o + 4] = v + 2; index[o + 5] = v + 3;
    }
    return { position: this.pos.slice(0, quads * 12), color: this.col.slice(0, quads * 12), uv: this.uv.slice(0, quads * 8), index };
  }
}

// Minecraft-like face shading
const SHADE_TOP = 1, SHADE_NS = 0.8, SHADE_EW = 0.62, SHADE_BOTTOM = 0.5, SHADE_PLANT = 0.9;
// horizontal directions: dx, dz, shade
const DIRS = [[0, -1, SHADE_NS], [0, 1, SHADE_NS], [-1, 0, SHADE_EW], [1, 0, SHADE_EW]];

/** Corners of a vertical face of the block column at (x, z) towards `dir`, from y16a to y16b
 *  (1/16 units); `inset` moves the plane into the block (vines). */
function wallCorners(q, dir, x, z, a, b, inset = 0) {
  const X = x * U, Z = z * U, X1 = X + U, Z1 = Z + U;
  let sx, sz, ex, ez;
  switch (dir) {
    case 0: sx = X1; ex = X; sz = ez = Z + inset; break;      // north
    case 1: sx = X; ex = X1; sz = ez = Z1 - inset; break;     // south
    case 2: sz = Z; ez = Z1; sx = ex = X + inset; break;      // west
    default: sz = Z1; ez = Z; sx = ex = X1 - inset; break;    // east
  }
  q[0] = sx; q[1] = b; q[2] = sz;  q[3] = sx; q[4] = a; q[5] = sz;
  q[6] = ex; q[7] = a; q[8] = ez;  q[9] = ex; q[10] = b; q[11] = ez;
}

function topCorners(q, x, z, y16, x1 = x + 1, z1 = z + 1) {
  q[0] = x * U; q[1] = y16; q[2] = z * U;   q[3] = x * U; q[4] = y16; q[5] = z1 * U;
  q[6] = x1 * U; q[7] = y16; q[8] = z1 * U; q[9] = x1 * U; q[10] = y16; q[11] = z * U;
}

function bottomCorners(q, x, z, y16) {
  q[0] = x * U; q[1] = y16; q[2] = z * U;   q[3] = (x + 1) * U; q[4] = y16; q[5] = z * U;
  q[6] = (x + 1) * U; q[7] = y16; q[8] = (z + 1) * U; q[9] = x * U; q[10] = y16; q[11] = (z + 1) * U;
}

const OPPOSITE = [1, 0, 3, 2];

/** Vertical face of a box smaller than a block (cactus, slab …) towards `dir`. */
function boxWall(q, dir, x, z, y, bx) {
  const X0 = x * U + bx[0], X1 = x * U + bx[3], Z0 = z * U + bx[2], Z1 = z * U + bx[5];
  const a = y * U + bx[1], b = y * U + bx[4];
  let sx, sz, ex, ez;
  switch (dir) {
    case 0: sx = X1; ex = X0; sz = ez = Z0; break;
    case 1: sx = X0; ex = X1; sz = ez = Z1; break;
    case 2: sz = Z0; ez = Z1; sx = ex = X0; break;
    default: sz = Z1; ez = Z0; sx = ex = X1; break;
  }
  q[0] = sx; q[1] = b; q[2] = sz;  q[3] = sx; q[4] = a; q[5] = sz;
  q[6] = ex; q[7] = a; q[8] = ez;  q[9] = ex; q[10] = b; q[11] = ez;
}

// What is at a height in a block stack
const AIR = -1, SOLID = -2, UNKNOWN = -3;

/** Block at height y in the stack of column `idx` of `entry` (block id, AIR, SOLID below the stack,
 *  or the water id inside a water run). */
function cellAt(entry, idx, y) {
  if (!entry || !entry.vox) return UNKNOWN;
  const { off, ys, ids } = entry.vox;
  const o0 = off[idx], o1 = off[idx + 1];
  if (o0 === o1) return entry.h[idx] === NONE ? AIR : UNKNOWN;
  if (y > ys[o0]) return AIR;
  for (let k = o0; k < o1; k++) {
    if (ys[k] === y) return ids[k];
    if (ys[k] < y) return ids[k - 1] === assets.waterId ? assets.waterId : AIR; // inside a water run?
  }
  return ids[o1 - 1] === assets.waterId ? assets.waterId : SOLID;
}

/** Does the neighbour `n` hide the face of block `id` (a box) next to it? */
function hides(n, id) {
  if (n === SOLID) return true;
  if (n < 0) return false; // air, unknown
  const nb = assets.blocks[n];
  if (nb.kind !== "cube" || !assets.full[n]) return false; // plants, slabs, cactus hide nothing
  if (!nb.transparent) return true;
  // see-through neighbour: same kind of glass/water hides, leaves never (fancy leaves)
  return n === id && !assets.leaves[n];
}

/** Mesh of one tile of a region: blocks [x0, x0 + size) × [z0, z0 + size), `step` blocks per box.
 *  Textured block stacks at step 1 when they are loaded, otherwise coloured height boxes. */
function build(key, rx, rz, step, x0, z0, size, fine = [false, false, false, false]) {
  const self = store.get(key);
  const prefix = key.slice(0, key.indexOf("/") + 1);
  const neighbour = (dx, dz) => store.get(`${prefix}${rx + dx},${rz + dz}`);
  const h = self.h;

  // Lowest height of the region: walls towards unknown/empty columns go down to here
  let base = Infinity;
  for (let i = 0; i < h.length; i += 5) if (h[i] !== NONE && h[i] < base) base = h[i];
  if (!Number.isFinite(base)) return null;
  base -= 4;

  /** (entry, column index) of region-local coordinates, possibly in a neighbour region. */
  const locate = (x, z) => {
    if (x >= 0 && x < SIZE && z >= 0 && z < SIZE) return [self, z * SIZE + x];
    const r = neighbour(Math.floor(x / SIZE), Math.floor(z / SIZE));
    return [r, (((z % SIZE) + SIZE) % SIZE) * SIZE + (((x % SIZE) + SIZE) % SIZE)];
  };
  const heightAt = (x, z) => {
    const [r, i] = locate(x, z);
    return r ? r.h[i] : undefined;
  };
  /** Lowest top along an edge segment in the neighbouring tile (its detail level is unknown, so
   *  look at a 16-block band aligned to the coarsest grid). */
  const edgeMin = (ax, az, bx, bz) => {
    let m = Infinity;
    for (let z = az; z < bz; z++) {
      for (let x = ax; x < bx; x++) {
        const v = heightAt(x, z);
        if (v === undefined || v === NONE) return base;
        if (v < m) m = v;
      }
    }
    return m;
  };
  const xe = x0 + size, ze = z0 + size;
  const a16 = (v) => Math.floor(v / 16) * 16;
  /** Neighbour height across the tile edge in direction d for the cell [x, x1) × [z, z1). */
  /** Lowest top the neighbouring tile can show at column (nx, nz): it is drawn with one of the
   *  steps 1..16, whose boxes take the height at the step-aligned column. */
  const coverMin = (nx, nz) => {
    let m = Infinity;
    for (const s of [1, 2, 4, 8, 16]) {
      const v = heightAt(Math.floor(nx / s) * s, Math.floor(nz / s) * s);
      if (v === undefined || v === NONE) return base;
      if (v < m) m = v;
    }
    return m;
  };
  const bandMin = (d, x, z, x1, z1) => {
    switch (d) {
      case 0: return edgeMin(Math.min(x, a16(x)), z0 - 16, Math.max(x1, a16(x) + 16), z0);
      case 1: return edgeMin(Math.min(x, a16(x)), ze, Math.max(x1, a16(x) + 16), ze + 16);
      case 2: return edgeMin(x0 - 16, Math.min(z, a16(z)), x0, Math.max(z1, a16(z) + 16));
      default: return edgeMin(xe, Math.min(z, a16(z)), xe + 16, Math.max(z1, a16(z) + 16));
    }
  };

  const opaque = new Buffers(4096), cutout = new Buffers(256), water = new Buffers(256);
  const q = new Array(12);

  if (assets && self.vox && step === 1) {
    buildStacks(self, rx, rz, x0, z0, xe, ze, locate, coverMin, base, opaque, cutout, water, q, fine);
  } else {
    buildBoxes(self, rx, rz, step, x0, z0, xe, ze, h, bandMin, opaque, q);
  }
  const result = { opaque: opaque.result(), cutout: cutout.result(), water: water.result() };
  return result.opaque || result.cutout || result.water ? result : null;
}

/** Real blocks: every entry of the block stacks with its shape and textures. */
/** `fine[d]`: the neighbour tile in direction d is drawn in full detail too, so nothing needs to
 *  be drawn "just in case" towards it. */
function buildStacks(self, rx, rz, x0, z0, xe, ze, locate, coverMin, base, opaque, cutout, water, q, fine) {
  const { off, ys, ids } = self.vox;
  const blocks = assets.blocks;
  for (let z = z0; z < ze; z++) {
    for (let x = x0; x < xe; x++) {
      const idx = z * SIZE + x;
      const o0 = off[idx], o1 = off[idx + 1];
      if (o0 === o1) continue;
      let waterTop = null; // y of the water surface above the current block
      let fog = WHITE_RGB;  // colour of the water above (tints what lies under it)
      for (let k = o0; k < o1; k++) {
        const y = ys[k], id = ids[k], b = blocks[id];
        if (!id) continue;
        const tint = tintAt(b, self, idx);
        // under water: darker and more of the water's colour with depth, like the game's water fog
        let depth = 1, mix = 0;
        if (waterTop !== null) {
          const d = waterTop - y;
          depth = Math.max(0.55, 0.92 - d * 0.04);
          mix = Math.min(0.7, 0.3 + d * 0.08);
        }
        const col = (shade, tinted) => {
          const s = shade * depth;
          const c = tinted ? tint : WHITE_RGB;
          return [0, 1, 2].map((i) => c[i] * s * (1 - mix + (mix * fog[i]) / 255));
        };

        if (id === assets.waterId) {
          // surface only, a bit below the block edge; the run below is filled with water
          const up = cellAt(self, idx, y + 1);
          if (up === AIR || up === UNKNOWN || (up >= 0 && blocks[up].kind !== "cube")) {
            topCorners(q, x, z, y * U + 14);
            water.quad(q, ...col(SHADE_TOP, true), uvRect(b.top));
          }
          for (let d = 0; d < 4; d++) {
            const [dx, dz, shade] = DIRS[d];
            const [ne, ni] = locate(x + dx, z + dz);
            const n = cellAt(ne, ni, y);
            if (n === AIR) {
              wallCorners(q, d, x, z, y * U, y * U + 14);
              water.quad(q, ...col(shade, true), uvRect(b.side));
            }
          }
          waterTop = y;
          fog = tint;
          continue;
        }

        if (b.kind === "cross") {
          const t = uvRect(b.top), c = col(SHADE_PLANT, b.tinted & TINT_TOP);
          const X = x * U, Z = z * U, Y = y * U, lo = 2, hi = U - 2;
          q[0] = X + lo; q[1] = Y + U; q[2] = Z + lo; q[3] = X + lo; q[4] = Y; q[5] = Z + lo;
          q[6] = X + hi; q[7] = Y; q[8] = Z + hi;     q[9] = X + hi; q[10] = Y + U; q[11] = Z + hi;
          cutout.quad(q, ...c, t);
          q[0] = X + lo; q[1] = Y + U; q[2] = Z + hi; q[3] = X + lo; q[4] = Y; q[5] = Z + hi;
          q[6] = X + hi; q[7] = Y; q[8] = Z + lo;     q[9] = X + hi; q[10] = Y + U; q[11] = Z + lo;
          cutout.quad(q, ...c, t);
          continue;
        }
        if (b.kind === "flat") {
          // corners NW, SW, SE, NE (in 1/16 block) with their heights; rails: slope + rotation
          const s = b.slope || 0, lo = 1, hi = U + 1;
          const hN = s > 0 ? hi : lo, hS = s < 0 ? hi : lo;
          const pts = [[0, hN, 0], [0, hS, U], [U, hS, U], [U, hN, 0]];
          for (let r = b.rot || 0; r > 0; r--) {
            for (const p of pts) { const px = p[0]; p[0] = U - p[2]; p[2] = px; } // clockwise from above
          }
          for (let c = 0; c < 4; c++) {
            q[c * 3] = x * U + pts[c][0]; q[c * 3 + 1] = y * U + pts[c][1]; q[c * 3 + 2] = z * U + pts[c][2];
          }
          cutout.quad(q, ...col(SHADE_TOP, b.tinted & TINT_TOP), uvRect(b.top));
          continue;
        }
        if (b.kind === "vine") {
          // on every wall it can hang on (the stored block has no properties)
          let any = false;
          for (let d = 0; d < 4; d++) {
            const [dx, dz, shade] = DIRS[d];
            const [ne, ni] = locate(x + dx, z + dz);
            const n = cellAt(ne, ni, y);
            if (n === SOLID || (n >= 0 && blocks[n].kind === "cube" && n !== assets.waterId)) {
              wallCorners(q, d, x, z, y * U, y * U + U, 1);
              cutout.quad(q, ...col(shade, b.tinted & TINT_TOP), uvRect(b.top));
              any = true;
            }
          }
          if (!any) { // hanging free (from leaves above): a cross
            const t = uvRect(b.top), c = col(SHADE_PLANT, b.tinted & TINT_TOP);
            wallCorners(q, 0, x, z, y * U, y * U + U, 8);
            cutout.quad(q, ...c, t);
            wallCorners(q, 2, x, z, y * U, y * U + U, 8);
            cutout.quad(q, ...c, t);
          }
          continue;
        }
        if (b.kind !== "cube") continue;

        const target = b.transparent ? cutout : opaque;
        const bx = b.box || FULL_BOX;
        const leaf = assets.leaves[id];
        // between two leaf blocks one face is enough (cutout faces are drawn from both sides)
        const shared = (n, keep) => leaf && n >= 0 && assets.leaves[n] && !keep;
        // top
        const above = cellAt(self, idx, y + 1);
        if (bx[4] < 16 || (!hides(above, id) && !shared(above, true))) {
          const Y = y * U + bx[4];
          // at the tile edge reach 1/16 into the neighbour tile: covers the hairline cracks where
          // its coarser grid meets ours (T-junctions)
          const ex0 = x === x0 && bx[0] === 0 ? 1 : 0, ex1 = x === xe - 1 && bx[3] === 16 ? 1 : 0;
          const ez0 = z === z0 && bx[2] === 0 ? 1 : 0, ez1 = z === ze - 1 && bx[5] === 16 ? 1 : 0;
          const X0 = x * U + bx[0] - ex0, X1 = x * U + bx[3] + ex1, Z0 = z * U + bx[2] - ez0, Z1 = z * U + bx[5] + ez1;
          q[0] = X0; q[1] = Y; q[2] = Z0;  q[3] = X0; q[4] = Y; q[5] = Z1;
          q[6] = X1; q[7] = Y; q[8] = Z1;  q[9] = X1; q[10] = Y; q[11] = Z0;
          target.quad(q, ...col(SHADE_TOP, b.tinted & TINT_TOP), uvSub(b.top, bx[0], bx[2], bx[3], bx[5]));
        }
        // bottom (only where there is air below, e.g. under tree crowns)
        const below = cellAt(self, idx, y - 1);
        if (bx[1] > 0 || (below === AIR && !shared(below, false))) {
          const Y = y * U + bx[1];
          q[0] = x * U + bx[0]; q[1] = Y; q[2] = z * U + bx[2];  q[3] = x * U + bx[3]; q[4] = Y; q[5] = z * U + bx[2];
          q[6] = x * U + bx[3]; q[7] = Y; q[8] = z * U + bx[5];  q[9] = x * U + bx[0]; q[10] = Y; q[11] = z * U + bx[5];
          target.quad(q, ...col(SHADE_BOTTOM, b.tinted & TINT_BOTTOM), uvSub(b.bottom, bx[0], bx[2], bx[3], bx[5]));
        }
        // sides
        const full = assets.full[id];
        for (let d = 0; d < 4; d++) {
          const [dx, dz, shade] = DIRS[d];
          const nx = x + dx, nz = z + dz;
          const inset = d === 0 ? bx[2] : d === 1 ? 16 - bx[5] : d === 2 ? bx[0] : 16 - bx[3];
          let visible = true, safety = false;
          if (inset === 0) {
            const [ne, ni] = locate(nx, nz);
            const n = cellAt(ne, ni, y);
            visible = !hides(n, id) && !shared(n, d === 1 || d === 3);
            // across the tile edge the neighbour may be drawn coarser: show what it might not cover
            if (!visible && !fine[d] && (nx < x0 || nx >= xe || nz < z0 || nz >= ze)) visible = safety = y + 1 > coverMin(nx, nz);
          }
          if (!visible) continue;
          // such a "just in case" face stops 1/16 below the top, so it never shares an edge with the
          // neighbour's top (z-fighting showed as dashed lines along tile edges)
          if (full) wallCorners(q, d, x, z, y * U, y * U + U - (safety ? 1 : 0));
          else boxWall(q, d, x, z, y, bx);
          const t = full ? uvRect(b.side) : uvSub(b.side, d < 2 ? bx[0] : bx[2], 16 - bx[4], d < 2 ? bx[3] : bx[5], 16 - bx[1]);
          target.quad(q, ...col(shade, b.tinted & TINT_SIDE), t);
          if (b.overlay) cutout.quad(q, ...col(shade, true), full ? uvRect(b.overlay) : t);
        }
      }

      // Looking into a cave / under an overhang: where an air gap of this column meets a
      // neighbour that is solid below its known blocks, draw that neighbour's wall
      for (let k = o0; k + 1 < o1; k++) {
        if (ids[k] === assets.waterId) continue; // the gap below a water surface is water
        const gapTop = ys[k] - 1, gapBottom = ys[k + 1] + 1;
        if (gapTop < gapBottom) continue;
        const wallUv = uvRect(assets.under[ids[k + 1]] || assets.under[ids[k]]);
        for (let y = gapTop; y >= gapBottom; y--) {
          for (let d = 0; d < 4; d++) {
            const [dx, dz, shade] = DIRS[d];
            const [ne, ni] = locate(x + dx, z + dz);
            if (cellAt(ne, ni, y) !== SOLID) continue;
            wallCorners(q, OPPOSITE[d], x + dx, z + dz, y * U, y * U + U);
            opaque.quad(q, 200 * shade, 200 * shade, 200 * shade, wallUv);
          }
        }
      }

      // Fill walls under the lowest known block where a neighbour (another tile / region, maybe
      // drawn coarser) is lower: no gaps between tiles
      const lowest = ys[o1 - 1], lowId = ids[o1 - 1];
      if (!lowId || lowId === assets.waterId) continue;
      const lb = blocks[lowId];
      if (lb.kind !== "cube") continue;
      for (let d = 0; d < 4; d++) {
        const [dx, dz, shade] = DIRS[d];
        const nx = x + dx, nz = z + dz;
        const inside = nx >= x0 && nx < xe && nz >= z0 && nz < ze;
        let n;
        if (inside || fine[d]) continue; // full-detail stacks always reach down far enough
        n = coverMin(nx, nz);
        if (n >= lowest) continue;
        const sideUv = uvRect(assets.under[lowId]);
        for (let y = lowest - 1; y >= Math.max(n, lowest - 64); y--) {
          wallCorners(q, d, x, z, y * U, y * U + U);
          opaque.quad(q, 255 * shade, 255 * shade, 255 * shade, sideUv);
        }
      }
    }
  }
}

/** Slight per-block brightness variation for untextured boxes, so flat areas still read as blocks. */
function noise(x, z) {
  let h = Math.imul(x, 73856093) ^ Math.imul(z, 19349663);
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return 0.92 + ((h >>> 16) & 7) * 0.0114;
}

/** Far away / without textures: one coloured box per `step`×`step` blocks from the height map. */
function buildBoxes(self, rx, rz, step, x0, z0, xe, ze, h, bandMin, out, q) {
  const col = self.c;
  const n = (xe - x0) / step;
  for (let j = 0; j < n; j++) {
    const z = z0 + j * step, z1 = z + step;
    for (let i = 0; i < n; i++) {
      const x = x0 + i * step, x1 = x + step;
      const idx = z * SIZE + x;
      const H = h[idx];
      if (H === NONE) continue;
      const f = assets ? 1 : noise(rx * SIZE + x, rz * SIZE + z);
      const r = col[idx * 3] * f, g = col[idx * 3 + 1] * f, b = col[idx * 3 + 2] * f;
      topCorners(q, x, z, H * U, x1, z1);
      // tile edge: overlap the neighbour tile by 1/16 against hairline cracks (T-junctions)
      if (x === x0) q[0] = q[3] -= 1;
      if (x1 === xe) q[6] = q[9] += 1;
      if (z === z0) q[2] = q[11] -= 1;
      if (z1 === ze) q[5] = q[8] += 1;
      out.quad(q, r, g, b, WHITE);
      const nh = [
        z > z0 ? h[(z - step) * SIZE + x] : bandMin(0, x, z, x1, z1),
        z1 < ze ? h[z1 * SIZE + x] : bandMin(1, x, z, x1, z1),
        x > x0 ? h[z * SIZE + x - step] : bandMin(2, x, z, x1, z1),
        x1 < xe ? h[z * SIZE + x1] : bandMin(3, x, z, x1, z1),
      ];
      for (let d = 0; d < 4; d++) {
        const L = nh[d] === NONE ? H - 8 : nh[d];
        if (L >= H) continue;
        const s = DIRS[d][2];
        // a wide box: corners along the whole step
        const X = x * U, Z = z * U, X1 = x1 * U, Z1 = z1 * U, a = L * U, c = H * U;
        switch (d) {
          case 0: q[0] = X1; q[2] = Z; q[3] = X1; q[5] = Z; q[6] = X; q[8] = Z; q[9] = X; q[11] = Z; break;
          case 1: q[0] = X; q[2] = Z1; q[3] = X; q[5] = Z1; q[6] = X1; q[8] = Z1; q[9] = X1; q[11] = Z1; break;
          case 2: q[0] = X; q[2] = Z; q[3] = X; q[5] = Z; q[6] = X; q[8] = Z1; q[9] = X; q[11] = Z1; break;
          default: q[0] = X1; q[2] = Z1; q[3] = X1; q[5] = Z1; q[6] = X1; q[8] = Z; q[9] = X1; q[11] = Z; break;
        }
        q[1] = c; q[4] = a; q[7] = a; q[10] = c;
        out.quad(q, r * s, g * s, b * s, WHITE);
      }
    }
  }
}

// ---------- 2D textured tiles ----------

/** "dim/x,z@v" -> Promise<{h, bio, ids} | null>, a few dozen kept */
const regions2d = new Map();
/** "texture:r,g,b" -> 16×16 canvas */
const tinted = new Map();

function region2d(dim, rx, rz, v) {
  const key = `${dim}/${rx},${rz}@${v}`;
  let p = regions2d.get(key);
  if (!p) {
    p = (async () => {
      const get = (kind) => fetch(url(`tiles/${dim}/${kind}/${rx}_${rz}.png?v=${v}`)).then((r) => (r.ok ? r.arrayBuffer() : null));
      const [hb, bb] = await Promise.all([get("h"), get("b")]);
      if (!hb || !bb) return null;
      const [h, blocks] = await Promise.all([decodeHeights(hb), decodeBlocks(bb)]);
      return h && blocks ? { h, ids: blocks.ids, bio: blocks.bio } : null;
    })().catch(() => null);
    regions2d.set(key, p);
    if (regions2d.size > 48) regions2d.delete(regions2d.keys().next().value);
  }
  return p;
}

/** A texture tinted (only the tinted faces) and with see-through pixels filled darker, as seen
 *  from above. */
function tintedTexture(tex, tint) {
  const key = `${tex}:${tint[0]},${tint[1]},${tint[2]}`;
  let c = tinted.get(key);
  if (!c) {
    const { w, data } = assets.atlas;
    const cols = w / TEX;
    const ox = (tex % cols) * TEX, oy = Math.floor(tex / cols) * TEX;
    const img = new ImageData(TEX, TEX);
    let sum = [0, 0, 0], n = 0;
    for (let y = 0; y < TEX; y++) {
      for (let x = 0; x < TEX; x++) {
        const s = ((oy + y) * w + ox + x) * 4;
        if (data[s + 3] > 0) {
          for (let k = 0; k < 3; k++) sum[k] += data[s + k];
          n++;
        }
      }
    }
    const avg = sum.map((v) => (n ? v / n : 128) * 0.85);
    for (let y = 0; y < TEX; y++) {
      for (let x = 0; x < TEX; x++) {
        const s = ((oy + y) * w + ox + x) * 4, d = (y * TEX + x) * 4;
        const solid = data[s + 3] >= 128;
        for (let k = 0; k < 3; k++) img.data[d + k] = ((solid ? data[s + k] : avg[k]) * tint[k]) / 255;
        img.data[d + 3] = 255;
      }
    }
    c = new OffscreenCanvas(TEX, TEX);
    c.getContext("2d").putImageData(img, 0, 0);
    tinted.set(key, c);
  }
  return c;
}

async function tile2d({ dim, v, x0, z0, scale }) {
  if (!assets) return null;
  const rx = Math.floor(x0 / SIZE), rz = Math.floor(z0 / SIZE);
  const r = await region2d(dim, rx, rz, v);
  if (!r || !assets) return null;
  const n = SIZE / scale; // blocks per tile side
  const lx0 = x0 - rx * SIZE, lz0 = z0 - rz * SIZE;
  const canvas = new OffscreenCanvas(SIZE, SIZE);
  const ctx = canvas.getContext("2d");
  ctx.imageSmoothingEnabled = false;
  const light = [[], [], []], dark = [[], [], []];
  for (let j = 0; j < n; j++) {
    const lz = lz0 + j;
    for (let i = 0; i < n; i++) {
      const lx = lx0 + i;
      const idx = lz * SIZE + lx;
      const H = r.h[idx];
      if (H === NONE || !r.ids[idx]) continue;
      const block = assets.blocks[r.ids[idx]];
      const tint = block.tinted & TINT_TOP ? tintAt(block, r, idx) : WHITE_RGB;
      ctx.drawImage(tintedTexture(block.top, tint), i * scale, j * scale, scale, scale);
      // relief of the ground: lighter when rising towards the south, darker when falling, a bit
      // more for bigger steps; not around tree crowns (they would cast hard black lines)
      if (lz === 0 || assets.leaves[r.ids[idx]] || assets.leaves[r.ids[idx - SIZE]]) continue;
      const north = r.h[idx - SIZE];
      if (north === NONE || north === H) continue;
      const step = Math.min(3, Math.abs(H - north)) - 1; // 0..2
      (H > north ? light : dark)[step].push(i, j);
    }
  }
  const shade = (list, style) => {
    if (!list.length) return;
    ctx.fillStyle = style;
    ctx.beginPath();
    for (let k = 0; k < list.length; k += 2) ctx.rect(list[k] * scale, list[k + 1] * scale, scale, scale);
    ctx.fill();
  };
  [0.07, 0.11, 0.15].forEach((a, k) => shade(light[k], `rgba(255,255,255,${a})`));
  [0.1, 0.16, 0.22].forEach((a, k) => shade(dark[k], `rgba(0,0,0,${a})`));
  return canvas.transferToImageBitmap();
}

// ---------- messages ----------

// The texture tables must be loaded before tiles are decoded; everything else runs as it comes
let ready = Promise.resolve();

self.onmessage = ({ data: m }) => {
  if (m.type === "init") ready = ready.then(() => handle(m)).catch((e) => console.error(e));
  else ready.then(() => handle(m)).catch((e) => console.error(e));
};

async function handle(m) {
  if (m.type === "init") {
    if (m.base) base = m.base;
    await loadAssets(m.stamp || "");
    postMessage({ type: "init", textured: !!assets });
  } else if (m.type === "load") {
    let entry = store.get(m.key) || { h: null, c: null, b: null, bio: null, vox: null };
    try {
      if (m.heights) entry.h = await decodeHeights(m.heights);
      if (m.blocks) {
        const blocks = await decodeBlocks(m.blocks);
        entry.b = blocks?.ids || null;
        entry.bio = blocks?.bio || null;
        entry.c = blocks?.c || null;
      } else if (m.color) {
        entry.c = await decodeColors(m.color);
        entry.b = entry.bio = null;
      }
      if (m.voxels) entry.vox = await decodeVoxels(m.voxels);
      entry.tints = null;
    } catch {
      entry = { h: null, c: null, b: null, bio: null, vox: null };
    }
    const ok = !!(entry.h && entry.c);
    if (ok) store.set(m.key, entry);
    else store.delete(m.key);
    postMessage({ type: "loaded", key: m.key, ok, heights: ok ? entry.h.slice() : null });
  } else if (m.type === "build") {
    let mesh = null, error;
    try {
      mesh = store.has(m.key) ? build(m.key, m.rx, m.rz, m.step, m.x0, m.z0, m.size, m.fine) : null;
    } catch (e) {
      error = String(e && e.stack || e); // always answer, or the page would wait forever
    }
    const transfer = [];
    for (const part of mesh ? [mesh.opaque, mesh.cutout, mesh.water] : []) {
      if (part) transfer.push(part.position.buffer, part.color.buffer, part.uv.buffer, part.index.buffer);
    }
    postMessage({ type: "mesh", key: m.key, id: m.id, step: m.step, error, ...(mesh || {}) }, transfer);
  } else if (m.type === "tile2d") {
    let bitmap = null;
    try {
      bitmap = await tile2d(m);
    } catch {
      bitmap = null;
    }
    postMessage({ type: "tile2d", id: m.id, bitmap }, bitmap ? [bitmap] : []);
  } else if (m.type === "drop") {
    store.delete(m.key);
  } else if (m.type === "clear") {
    store.clear();
  }
}
