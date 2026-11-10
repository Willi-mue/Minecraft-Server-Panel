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
//               {type: "build", key, rx, rz, step, x0, z0, size, id, fine, slice}  (one 3D tile of a
//                region; key prefix "<dim>/", neighbour regions are found by key; slice = {lo, hi}:
//                only blocks lo <= y < hi, cut faces on top, or null)
//               {type: "tile2d", id, dim, v, x0, z0, scale}  (512 px 2D tile, scale = px per block)
//               {type: "height", id, dim, v, x, z}      (top face y of one block column, 2D view)
//               {type: "adopt", key, entry}             (a region another worker decoded, or null)
//               {type: "drop", key} | {type: "clear"}
// Messages out: {type: "init", textured}
//               {type: "loaded", key, ok, heights, entry}  (heights: Int16Array, top face y, NONE =
//                no data; entry: the decoded region for "adopt" in the other workers)
//               {type: "mesh", key, id, step, opaque?, cutout?, water?, plants?, leaves?}  (each
//                {position, color, uv, tex, index}; positions in 1/16 block, relative to the region
//                origin; uv in texture widths, tex = texture index, see uvSub)
//               {type: "tile2d", id, bitmap}
//               {type: "height", id, y}                 (null = unknown)

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

/** Plant tile (R, G = id of the plant / flat block over the ground, A = 255 where there is one)
 *  -> ids (0 = none). */
async function decodePlants(buffer) {
  const img = await pixels(buffer, SIZE);
  if (!img || !assets) return null;
  const rgba = img.data;
  const ids = new Uint16Array(SIZE * SIZE);
  for (let i = 0; i < ids.length; i++) {
    if (rgba[i * 4 + 3] === 0) continue;
    const id = (rgba[i * 4] << 8) | rgba[i * 4 + 1];
    ids[i] = id < assets.blocks.length ? id : 0;
  }
  return ids;
}

// Stack markers (mapgen render.rs): FILL = solid rock the map does not show, down to the next
// entry; AIR_MARK = visible air starts here (after a FILL run; as the last entry: air down to the
// world bottom, the End's void)
const FILL = 0xffff, AIR_MARK = 0xfffe;
const isMark = (id) => id >= AIR_MARK;

/** Block stacks ("MGV2", u8 count per column, then i16 y + u16 id per entry, zlib). */
async function decodeVoxels(buffer) {
  const raw = await inflate(buffer);
  const magic = raw.length < 4 + SIZE * SIZE ? "" : String.fromCharCode(...raw.subarray(0, 4));
  if (magic !== "MGV2" && magic !== "MGV1") return null;
  const off = new Uint32Array(SIZE * SIZE + 1);
  for (let i = 0; i < SIZE * SIZE; i++) off[i + 1] = off[i] + raw[4 + i];
  const n = off[SIZE * SIZE];
  const dv = new DataView(raw.buffer, raw.byteOffset + 4 + SIZE * SIZE);
  const ys = new Int16Array(n), ids = new Uint16Array(n);
  const max = assets ? assets.blocks.length : 0;
  for (let k = 0; k < n; k++) {
    ys[k] = dv.getInt16(k * 4, true);
    const id = dv.getUint16(k * 4 + 2, true);
    ids[k] = id < max || isMark(id) ? id : 0;
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
    // detailed blocks (stairs, fences …) never hide their neighbours' faces
    const full = json.blocks.map((b) => isFull(b.box || FULL_BOX) && !b.shape);
    const shapes = json.shapes || [];
    assets = { blocks: json.blocks, biomes: json.biomes, atlas, under, leaves, full, shapes, waterId: index.get("minecraft:water") || -1 };
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

// Texture coordinates: [texture index, u0, v0, u1, v1] with u/v in texture widths (1 = one block).
// The shader (map3d.js `repeatTextures`) repeats the texture with fract(), so a quad merged from
// several equal faces simply spans u/v 0..n. Slightly inset, so a fragment exactly on the quad's
// edge never wraps around to the texture's other side.
const EPS = 0.001;

/** Whole texture `i`. */
function uvRect(i) {
  return uvSub(i, 0, 0, TEX, TEX);
}
const WHITE = [0, 0.5, 0.5, 0.5, 0.5]; // texture 0 is plain white

/** Part of texture `i` (pixels 0..16), for boxes smaller than a block (slabs, cactus). */
function uvSub(i, a, b, c, d) {
  const su = c >= a ? EPS : -EPS, sv = d >= b ? EPS : -EPS;
  return [i, a / TEX + su, b / TEX + sv, c / TEX - su, d / TEX - sv];
}

const FULL_BOX = [0, 0, 0, 16, 16, 16];
const isFull = (bx) => bx[0] === 0 && bx[1] === 0 && bx[2] === 0 && bx[3] === 16 && bx[4] === 16 && bx[5] === 16;

// ---------- 3D meshing ----------

/** Growable typed arrays for one mesh. */
class Buffers {
  constructor(quads) {
    this.id = Buffers.next = (Buffers.next || 0) + 1;
    this.cap = Math.max(quads, 256);
    this.n = 0;
    this.pos = new Int16Array(this.cap * 12);
    this.col = new Uint8Array(this.cap * 12);
    this.uv = new Float32Array(this.cap * 8);
    this.tex = new Uint16Array(this.cap * 4);
  }
  grow() {
    this.cap *= 2;
    const pos = new Int16Array(this.cap * 12), col = new Uint8Array(this.cap * 12);
    const uv = new Float32Array(this.cap * 8), tex = new Uint16Array(this.cap * 4);
    pos.set(this.pos);
    col.set(this.col);
    uv.set(this.uv);
    tex.set(this.tex);
    this.pos = pos;
    this.col = col;
    this.uv = uv;
    this.tex = tex;
  }
  /** One quad: 4 corners (counter-clockwise seen from outside; for walls: top-start, bottom-start,
   *  bottom-end, top-end), one colour (0..255, clamped), texture [index, u0, v0, u1, v1]. */
  quad(c, r, g, b, t) {
    if (this.n === this.cap) this.grow();
    const o = this.n * 12;
    for (let k = 0; k < 12; k++) this.pos[o + k] = c[k];
    r = r > 255 ? 255 : r; g = g > 255 ? 255 : g; b = b > 255 ? 255 : b;
    for (let k = 0; k < 12; k += 3) {
      this.col[o + k] = r; this.col[o + k + 1] = g; this.col[o + k + 2] = b;
    }
    const u = this.n * 8;
    this.uv[u] = t[1]; this.uv[u + 1] = t[2];
    this.uv[u + 2] = t[1]; this.uv[u + 3] = t[4];
    this.uv[u + 4] = t[3]; this.uv[u + 5] = t[4];
    this.uv[u + 6] = t[3]; this.uv[u + 7] = t[2];
    const x = this.n * 4;
    this.tex[x] = this.tex[x + 1] = this.tex[x + 2] = this.tex[x + 3] = t[0];
    this.n++;
  }
  result() {
    const quads = this.n;
    if (!quads) return null;
    const index = quads * 4 <= 65536 ? new Uint16Array(quads * 6) : new Uint32Array(quads * 6);
    for (let k = 0; k < quads; k++) {
      const v = k * 4, o = k * 6;
      index[o] = v; index[o + 1] = v + 1; index[o + 2] = v + 2;
      index[o + 3] = v; index[o + 4] = v + 2; index[o + 5] = v + 3;
    }
    return {
      position: this.pos.slice(0, quads * 12), color: this.col.slice(0, quads * 12),
      uv: this.uv.slice(0, quads * 8), tex: this.tex.slice(0, quads * 4), index,
    };
  }
}

/** Collects equal faces of one tile and emits them as few big quads (greedy meshing):
 *  top faces per height plane as rectangles, wall faces per column and direction as vertical runs.
 *  Equal = same buffer, texture and colour; the texture repeats across a merged quad. */
class Merger {
  constructor(x0, z0, size) {
    this.x0 = x0;
    this.z0 = z0;
    this.size = size;
    this.planes = new Map(); // "target|y16|edge" -> {target, y16, edge, grid, kinds: Map, list: []}
    this.runs = [null, null, null, null]; // current wall run per direction (one column at a time)
    this.walls = new Map(); // finished vertical runs: "target|d|plane|top|bottom|key" -> {run, along: []}
  }
  /** Colours are rounded to steps of 4 (not visible), so blended biome tints of neighbouring
   *  columns match more often. */
  static q(v) {
    v = ((v + 2) >> 2) << 2;
    return v > 255 ? 255 : v;
  }
  static key(cell, r, g, b) {
    return cell * 16777216 + Merger.q(r) * 65536 + Merger.q(g) * 256 + Merger.q(b); // < 2^53
  }
  /** A top face of block column (x, z) at height y16 (1/16 units); `edge`: may reach 1/16 into the
   *  neighbour tile at the tile border (against T-junction cracks). */
  top(target, y16, edge, x, z, cell, r, g, b) {
    const pk = `${target.id}|${y16}|${edge ? 1 : 0}`;
    let p = this.planes.get(pk);
    if (!p) {
      p = { target, y16, edge, grid: new Int32Array(this.size * this.size), kinds: new Map(), list: [0] };
      this.planes.set(pk, p);
    }
    const k = Merger.key(cell, r, g, b);
    let id = p.kinds.get(k);
    if (id === undefined) {
      id = p.list.length;
      p.kinds.set(k, id);
      p.list.push([cell, Merger.q(r), Merger.q(g), Merger.q(b)]);
    }
    p.grid[(z - this.z0) * this.size + x - this.x0] = id;
  }
  /** A full wall face of block (x, y, z) towards d; call top-down within a column. */
  wall(target, d, x, z, y, cell, r, g, b, overlay = null) {
    // overlay: [buffer, texture, r, g, b] drawn over the same quad (the tinted grass side overlay)
    const k = overlay ? `${Merger.key(cell, r, g, b)}+${overlay[0].id}:${Merger.key(overlay[1], overlay[2], overlay[3], overlay[4])}` : Merger.key(cell, r, g, b);
    const run = this.runs[d];
    if (run && run.target === target && run.k === k && run.bottom === y + 1 && run.x === x && run.z === z) {
      run.bottom = y;
      return;
    }
    if (run) this.flushWall(d);
    this.runs[d] = { target, k, x, z, top: y, bottom: y, cell, r, g, b, overlay };
  }
  /** A finished vertical run: kept until `finish`, where equal runs side by side along the wall
   *  (same plane, same top and bottom) become one quad. */
  flushWall(d) {
    const run = this.runs[d];
    if (!run) return;
    this.runs[d] = null;
    const ns = d < 2; // north/south faces lie in a z plane and run along x
    const plane = ns ? run.z : run.x, along = ns ? run.x : run.z;
    const wk = `${run.target.id}|${d}|${plane}|${run.top}|${run.bottom}|${run.k}`;
    let w = this.walls.get(wk);
    if (!w) this.walls.set(wk, (w = { run, d, plane, along: [] }));
    w.along.push(along);
  }
  /** End of a column: close the open wall runs. */
  column() {
    for (let d = 0; d < 4; d++) this.flushWall(d);
  }
  /** Emit the walls (merged along their plane) and the top faces (as rectangles). */
  finish() {
    this.column();
    const qw = new Array(12);
    for (const { run, d, plane, along } of this.walls.values()) {
      along.sort((a, b) => a - b);
      for (let i = 0; i < along.length; ) {
        let j = i;
        while (j + 1 < along.length && along[j + 1] === along[j] + 1) j++;
        mergedWall(qw, d, along[i], along[j], plane, run.bottom, run.top + 1);
        const r = Merger.q(run.r), g = Merger.q(run.g), b = Merger.q(run.b);
        const u1 = j - i + 1 - EPS, v1 = run.top - run.bottom + 1 - EPS;
        run.target.quad(qw, r, g, b, [run.cell, EPS, EPS, u1, v1]);
        const o = run.overlay;
        if (o) o[0].quad(qw, Merger.q(o[2]), Merger.q(o[3]), Merger.q(o[4]), [o[1], EPS, EPS, u1, v1]);
        i = j + 1;
      }
    }
    this.walls.clear();
    const S = this.size, q = new Array(12), xe = this.x0 + S, ze = this.z0 + S;
    for (const p of this.planes.values()) {
      const grid = p.grid;
      for (let j = 0; j < S; j++) {
        for (let i = 0; i < S; i++) {
          const v = grid[j * S + i];
          if (!v) continue;
          let w = 1;
          while (i + w < S && grid[j * S + i + w] === v) w++;
          let h = 1;
          grow: while (j + h < S) {
            for (let k = 0; k < w; k++) if (grid[(j + h) * S + i + k] !== v) break grow;
            h++;
          }
          for (let jj = 0; jj < h; jj++) grid.fill(0, (j + jj) * S + i, (j + jj) * S + i + w);
          const x = this.x0 + i, z = this.z0 + j, x1 = x + w, z1 = z + h;
          let X0 = x * U, X1 = x1 * U, Z0 = z * U, Z1 = z1 * U;
          if (p.edge) {
            if (x === this.x0) X0 -= 1;
            if (x1 === xe) X1 += 1;
            if (z === this.z0) Z0 -= 1;
            if (z1 === ze) Z1 += 1;
          }
          const Y = p.y16;
          q[0] = X0; q[1] = Y; q[2] = Z0;  q[3] = X0; q[4] = Y; q[5] = Z1;
          q[6] = X1; q[7] = Y; q[8] = Z1;  q[9] = X1; q[10] = Y; q[11] = Z0;
          const [cell, r, g, b] = p.list[v];
          p.target.quad(q, r, g, b, [cell, EPS, EPS, w - EPS, h - EPS]);
        }
      }
    }
    this.planes.clear();
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

/** A wall towards `dir` spanning the columns a0..a1 (inclusive) along it, in the column line
 *  `p` (z for north/south, x for west/east), from block y0 up to y1 (exclusive); corners like
 *  wallCorners. */
function mergedWall(q, dir, a0, a1, p, y0, y1) {
  let sx, sz, ex, ez;
  switch (dir) {
    case 0: sx = (a1 + 1) * U; ex = a0 * U; sz = ez = p * U; break;       // north
    case 1: sx = a0 * U; ex = (a1 + 1) * U; sz = ez = (p + 1) * U; break; // south
    case 2: sz = a0 * U; ez = (a1 + 1) * U; sx = ex = p * U; break;       // west
    default: sz = (a1 + 1) * U; ez = a0 * U; sx = ex = (p + 1) * U; break; // east
  }
  const a = y0 * U, b = y1 * U;
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

/** Block at height y in the stack of column `idx` of `entry` (block id, AIR, SOLID below the stack
 *  and in FILL runs, or the water id inside a water run). */
function cellAt(entry, idx, y) {
  if (!entry || !entry.vox) return UNKNOWN;
  const { off, ys, ids } = entry.vox;
  const o0 = off[idx], o1 = off[idx + 1];
  if (o0 === o1) return entry.h[idx] === NONE ? AIR : UNKNOWN;
  if (y > ys[o0]) return AIR;
  for (let k = o0; k < o1; k++) {
    if (ys[k] === y) return ids[k] === FILL ? SOLID : ids[k] === AIR_MARK ? AIR : ids[k];
    if (ys[k] < y) { // between two entries: inside a FILL or water run, or air
      const above = ids[k - 1];
      return above === FILL ? SOLID : above === assets.waterId ? assets.waterId : AIR;
    }
  }
  // below the last entry: solid, water below a water entry, air below a closing AIR_MARK (void)
  const last = ids[o1 - 1];
  return last === assets.waterId ? assets.waterId : last === AIR_MARK ? AIR : SOLID;
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

// Part layout of blocks.json `shapes` (see mapgen assets.rs): box x0 y0 z0 x1 y1 z1, then per face
// (up, down, north, south, west, east) u0 v0 u1 v1 slot (0 top, 1 side, 2 bottom, 3… `extra`);
// 255 = face not there
const NO_FACE = 255;

/** A detailed block (stairs, fence, door, torch …): the boxes of its model with their own UVs.
 *  Faces on the block border are culled against a neighbour that hides them. */
function drawShape(parts, b, id, x, z, y, self, idx, locate, col, target, q, cut = false) {
  if (!parts) return;
  const tex = b.extra ? [b.top, b.side, b.bottom, ...b.extra] : [b.top, b.side, b.bottom];
  const X = x * U, Y = y * U, Z = z * U;
  let above, below;
  const nb = [];
  for (let p = 0; p < parts.length; p++) {
    const bx = parts[p];
    for (let f = 0; f < 6; f++) {
      const o = 6 + f * 5;
      if (bx[o] === NO_FACE) continue;
      const t = uvSub(tex[bx[o + 4]] ?? b.side, bx[o], bx[o + 1], bx[o + 2], bx[o + 3]);
      if (f === 0) {
        if (bx[4] === 16 && !cut && hides(above ??= cellAt(self, idx, y + 1), id)) continue;
        const h = Y + bx[4];
        q[0] = X + bx[0]; q[1] = h; q[2] = Z + bx[2];  q[3] = X + bx[0]; q[4] = h; q[5] = Z + bx[5];
        q[6] = X + bx[3]; q[7] = h; q[8] = Z + bx[5];  q[9] = X + bx[3]; q[10] = h; q[11] = Z + bx[2];
        target.quad(q, ...col(SHADE_TOP, b.tinted & TINT_TOP), t);
      } else if (f === 1) {
        if (bx[1] === 0 && hides(below ??= cellAt(self, idx, y - 1), id)) continue;
        const h = Y + bx[1];
        q[0] = X + bx[0]; q[1] = h; q[2] = Z + bx[2];  q[3] = X + bx[3]; q[4] = h; q[5] = Z + bx[2];
        q[6] = X + bx[3]; q[7] = h; q[8] = Z + bx[5];  q[9] = X + bx[0]; q[10] = h; q[11] = Z + bx[5];
        target.quad(q, ...col(SHADE_BOTTOM, b.tinted & TINT_BOTTOM), t);
      } else {
        const d = f - 2; // DIRS order: north, south, west, east
        const edge = d === 0 ? bx[2] === 0 : d === 1 ? bx[5] === 16 : d === 2 ? bx[0] === 0 : bx[3] === 16;
        if (edge) {
          if (nb[d] === undefined) {
            const [ne, ni] = locate(x + DIRS[d][0], z + DIRS[d][1]);
            nb[d] = cellAt(ne, ni, y);
          }
          if (hides(nb[d], id)) continue;
        }
        boxWall(q, d, x, z, y, bx);
        target.quad(q, ...col(DIRS[d][2], b.tinted & TINT_SIDE), t);
      }
    }
  }
}

/** Mesh of one tile of a region: blocks [x0, x0 + size) × [z0, z0 + size), `step` blocks per box.
 *  Textured block stacks at step 1 when they are loaded, otherwise coloured height boxes. */
function build(key, rx, rz, step, x0, z0, size, fine = [false, false, false, false], slice = null) {
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
  // with a height slice the neighbours are drawn cut at its top too
  const heightAt = (x, z) => {
    const [r, i] = locate(x, z);
    if (!r) return undefined;
    const v = r.h[i];
    return slice && v !== NONE ? Math.min(v, slice.hi) : v;
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

  // separate buffers for what the viewer can hide without a rebuild (water, plants, leaves)
  const out = {
    opaque: new Buffers(4096), cutout: new Buffers(256), water: new Buffers(256),
    plants: new Buffers(256), leaves: new Buffers(256),
  };
  const q = new Array(12);

  if (assets && self.vox && step === 1) {
    const merge = new Merger(x0, z0, size);
    buildStacks(self, rx, rz, x0, z0, xe, ze, locate, coverMin, base, out, q, fine, slice, merge);
    merge.finish();
  } else {
    buildBoxes(self, rx, rz, step, x0, z0, xe, ze, h, bandMin, out.opaque, q, slice);
  }
  const result = {};
  for (const [k, b] of Object.entries(out)) {
    const r = b.result();
    if (r) result[k] = r;
  }
  return Object.keys(result).length ? result : null;
}

/** Real blocks: every entry of the block stacks with its shape and textures. */
/** `fine[d]`: the neighbour tile in direction d is drawn in full detail too, so nothing needs to
 *  be drawn "just in case" towards it. */
function buildStacks(self, rx, rz, x0, z0, xe, ze, locate, coverMin, base, out, q, fine, slice, merge) {
  const { off, ys, ids } = self.vox;
  const blocks = assets.blocks;
  const { opaque, cutout, water, plants, leaves } = out;
  const lo = slice ? slice.lo : -Infinity, hi = slice ? slice.hi : Infinity;
  // above the slice counts as air: the blocks at its top show their top faces
  const cellOwn = (idx, y) => (y >= hi ? AIR : cellAt(self, idx, y));
  for (let z = z0; z < ze; z++) {
    for (let x = x0; x < xe; x++) {
      const idx = z * SIZE + x;
      const o0 = off[idx], o1 = off[idx + 1];
      if (o0 === o1) continue;
      let waterTop = null; // y of the water surface above the current block
      let fog = WHITE_RGB;  // colour of the water above (tints what lies under it)
      for (let k = o0; k < o1; k++) {
        const y = ys[k], id = ids[k];
        if (id === FILL) { waterTop = null; fog = WHITE_RGB; continue; }
        if (!id || isMark(id) || y < lo) continue;
        const b = blocks[id];
        if (y >= hi) {
          // above the slice; a water run reaching into it gets its surface at the cut
          if (id === assets.waterId) {
            if (cellAt(self, idx, hi - 1) === assets.waterId) {
              merge.top(water, hi * U - 2, false, x, z, b.top, ...tintAt(b, self, idx));
            }
            waterTop = y;
            fog = tintAt(b, self, idx);
          }
          continue;
        }
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
          const up = cellOwn(idx, y + 1);
          if (up === AIR || up === UNKNOWN || (up >= 0 && blocks[up].kind !== "cube")) {
            merge.top(water, y * U + 14, false, x, z, b.top, ...col(SHADE_TOP, true));
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

        if (b.shape) {
          drawShape(assets.shapes[b.shape], b, id, x, z, y, self, idx, locate, col, b.transparent ? cutout : opaque, q, y + 1 >= hi);
          continue;
        }
        if (b.kind === "cross") {
          const t = uvRect(b.top), c = col(SHADE_PLANT, b.tinted & TINT_TOP);
          const X = x * U, Z = z * U, Y = y * U, lo = 2, hi = U - 2;
          q[0] = X + lo; q[1] = Y + U; q[2] = Z + lo; q[3] = X + lo; q[4] = Y; q[5] = Z + lo;
          q[6] = X + hi; q[7] = Y; q[8] = Z + hi;     q[9] = X + hi; q[10] = Y + U; q[11] = Z + hi;
          plants.quad(q, ...c, t);
          q[0] = X + lo; q[1] = Y + U; q[2] = Z + hi; q[3] = X + lo; q[4] = Y; q[5] = Z + hi;
          q[6] = X + hi; q[7] = Y; q[8] = Z + lo;     q[9] = X + hi; q[10] = Y + U; q[11] = Z + lo;
          plants.quad(q, ...c, t);
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
          plants.quad(q, ...col(SHADE_TOP, b.tinted & TINT_TOP), uvRect(b.top));
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
              plants.quad(q, ...col(shade, b.tinted & TINT_TOP), uvRect(b.top));
              any = true;
            }
          }
          if (!any) { // hanging free (from leaves above): a cross
            const t = uvRect(b.top), c = col(SHADE_PLANT, b.tinted & TINT_TOP);
            wallCorners(q, 0, x, z, y * U, y * U + U, 8);
            plants.quad(q, ...c, t);
            wallCorners(q, 2, x, z, y * U, y * U + U, 8);
            plants.quad(q, ...c, t);
          }
          continue;
        }
        if (b.kind !== "cube") continue;

        const leaf = assets.leaves[id];
        const target = leaf ? leaves : b.transparent ? cutout : opaque;
        const bx = b.box || FULL_BOX;
        // between two leaf blocks one face is enough (cutout faces are drawn from both sides)
        const shared = (n, keep) => leaf && n >= 0 && assets.leaves[n] && !keep;
        // top
        const above = cellOwn(idx, y + 1);
        const fullBox = isFull(bx);
        if (fullBox && !hides(above, id) && !shared(above, true)) {
          merge.top(target, y * U + 16, true, x, z, b.top, ...col(SHADE_TOP, b.tinted & TINT_TOP));
        } else if (bx[4] < 16 || (!hides(above, id) && !shared(above, true))) {
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
          if (full && !safety) {
            const overlay = b.overlay ? [cutout, b.overlay, ...col(shade, true)] : null;
            merge.wall(target, d, x, z, y, b.side, ...col(shade, b.tinted & TINT_SIDE), overlay);
            continue;
          }
          // such a "just in case" face stops 1/16 below the top, so it never shares an edge with the
          // neighbour's top (z-fighting showed as dashed lines along tile edges)
          if (full) wallCorners(q, d, x, z, y * U, y * U + U - (safety ? 1 : 0));
          else boxWall(q, d, x, z, y, bx);
          const t = full ? uvRect(b.side) : uvSub(b.side, d < 2 ? bx[0] : bx[2], 16 - bx[4], d < 2 ? bx[3] : bx[5], 16 - bx[1]);
          target.quad(q, ...col(shade, b.tinted & TINT_SIDE), t);
          if (b.overlay) cutout.quad(q, ...col(shade, true), full ? uvRect(b.overlay) : t);
        }
      }

      // Height slice cutting through rock the map does not show: a plain dark cut face on top
      // (no texture: it must not look like real blocks)
      if (ys[o0] >= hi && cellAt(self, idx, hi - 1) === SOLID) {
        topCorners(q, x, z, hi * U);
        const g = 62 * noise(rx * SIZE + x, rz * SIZE + z);
        opaque.quad(q, g, g, g + 6, WHITE);
      }

      // Looking into a cave / under an overhang: where an air gap of this column meets rock the
      // map does not show (a neighbour's FILL run or below its stack: a cave beyond the reach,
      // across a region border), close it with a plain rock wall; where the gap ends on FILL,
      // close it with a floor / ceiling
      for (let k = o0; k + 1 < o1; k++) {
        const top = ids[k];
        if (top === assets.waterId || top === FILL) continue; // water below a surface / rock
        const gapTop = top === AIR_MARK ? ys[k] : ys[k] - 1, gapBottom = ys[k + 1] + 1;
        if (gapTop < gapBottom) continue;
        const real = (id) => (id && !isMark(id) ? id : 0);
        const wallUv = uvRect(assets.under[real(ids[k + 1]) || real(top) || real(lastBlock(ids, o0, o1))] ?? 0);
        if (ids[k + 1] === FILL && gapBottom >= lo && gapBottom <= hi) {
          topCorners(q, x, z, gapBottom * U);
          opaque.quad(q, 150, 150, 150, wallUv);
        }
        if (top === AIR_MARK && k > o0 && ids[k - 1] === FILL && gapTop + 1 < hi && gapTop + 1 >= lo) {
          bottomCorners(q, x, z, (gapTop + 1) * U);
          opaque.quad(q, 110, 110, 110, wallUv);
        }
        // one quad per vertical run of rock beside the gap
        const yTop = Math.min(gapTop, hi - 1), yBottom = Math.max(gapBottom, lo);
        for (let d = 0; d < 4; d++) {
          const [dx, dz, shade] = DIRS[d];
          const [ne, ni] = locate(x + dx, z + dz);
          let runTop = null;
          for (let y = yTop; y >= yBottom - 1; y--) {
            const rock = y >= yBottom && cellAt(ne, ni, y) === SOLID;
            if (rock && runTop === null) runTop = y;
            if (!rock && runTop !== null) {
              wallCorners(q, OPPOSITE[d], x + dx, z + dz, (y + 1) * U, (runTop + 1) * U);
              opaque.quad(q, 200 * shade, 200 * shade, 200 * shade, [wallUv[0], EPS, EPS, 1 - EPS, runTop - y - EPS]);
              runTop = null;
            }
          }
        }
      }

      // Fill walls under the lowest known block where a neighbour (another tile / region, maybe
      // drawn coarser) is lower: no gaps between tiles
      const lowest = ys[o1 - 1], lowId = ids[o1 - 1];
      if (!lowId || isMark(lowId) || lowId === assets.waterId) continue;
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
        const yTop = Math.min(lowest - 1, hi - 1), yBottom = Math.max(n, lowest - 64, lo);
        if (yTop < yBottom) continue;
        wallCorners(q, d, x, z, yBottom * U, (yTop + 1) * U);
        opaque.quad(q, 255 * shade, 255 * shade, 255 * shade, [assets.under[lowId], EPS, EPS, 1 - EPS, yTop - yBottom + 1 - EPS]);
      }
    }
  }
}

/** The lowest real block of a stack (for the texture of placeholder walls). */
function lastBlock(ids, o0, o1) {
  for (let k = o1 - 1; k >= o0; k--) if (ids[k] && !isMark(ids[k])) return ids[k];
  return 0;
}

/** Slight per-block brightness variation for untextured boxes, so flat areas still read as blocks. */
function noise(x, z) {
  let h = Math.imul(x, 73856093) ^ Math.imul(z, 19349663);
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return 0.92 + ((h >>> 16) & 7) * 0.0114;
}

/** Far away / without textures: one coloured box per `step`×`step` blocks from the height map. */
function buildBoxes(self, rx, rz, step, x0, z0, xe, ze, h, bandMin, out, q, slice = null) {
  const lo = slice ? slice.lo : -Infinity, hi = slice ? slice.hi : Infinity;
  const col = self.c;
  const n = (xe - x0) / step;
  // tops: boxes of equal height and colour next to each other in a row become one quad
  let run = null; // {x, z, z1, x1, H, r, g, b, k}
  const flush = () => {
    if (!run) return;
    topCorners(q, run.x, run.z, run.H * U, run.x1, run.z1);
    // tile edge: overlap the neighbour tile by 1/16 against hairline cracks (T-junctions)
    if (run.x === x0) q[0] = q[3] -= 1;
    if (run.x1 === xe) q[6] = q[9] += 1;
    if (run.z === z0) q[2] = q[11] -= 1;
    if (run.z1 === ze) q[5] = q[8] += 1;
    out.quad(q, run.r, run.g, run.b, WHITE);
    run = null;
  };
  for (let j = 0; j < n; j++) {
    const z = z0 + j * step, z1 = z + step;
    flush();
    for (let i = 0; i < n; i++) {
      const x = x0 + i * step, x1 = x + step;
      const idx = z * SIZE + x;
      const H0 = h[idx];
      if (H0 === NONE || H0 <= lo) {
        flush();
        continue;
      }
      const H = Math.min(H0, hi);
      // cut by the height slice: darker top
      const f = (assets ? 1 : noise(rx * SIZE + x, rz * SIZE + z)) * (H0 > hi ? 0.55 : 1);
      const r = col[idx * 3] * f, g = col[idx * 3 + 1] * f, b = col[idx * 3 + 2] * f;
      const k = Merger.key(0, r, g, b);
      if (run && run.H === H && run.k === k) run.x1 = x1;
      else {
        flush();
        run = { x, z, z1, x1, H, r, g, b, k };
      }
      const nh = [
        z > z0 ? h[(z - step) * SIZE + x] : bandMin(0, x, z, x1, z1),
        z1 < ze ? h[z1 * SIZE + x] : bandMin(1, x, z, x1, z1),
        x > x0 ? h[z * SIZE + x - step] : bandMin(2, x, z, x1, z1),
        x1 < xe ? h[z * SIZE + x1] : bandMin(3, x, z, x1, z1),
      ];
      for (let d = 0; d < 4; d++) {
        const L = Math.max(Math.min(nh[d] === NONE ? H - 8 : nh[d], hi), lo);
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
  flush();
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
      const [hb, bb, pb] = await Promise.all([get("h"), get("b"), get("p")]);
      if (!hb || !bb) return null;
      const [h, blocks, plants] = await Promise.all([decodeHeights(hb), decodeBlocks(bb), pb && decodePlants(pb)]);
      return h && blocks ? { h, ids: blocks.ids, bio: blocks.bio, plants } : null;
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

/** A texture tinted, see-through pixels kept (plants drawn over the ground in 2D), turned by
 *  `rot` quarter turns clockwise (rails). */
function plantTexture(tex, tint, rot) {
  const key = `p${tex}:${tint[0]},${tint[1]},${tint[2]}:${rot}`;
  let c = tinted.get(key);
  if (!c) {
    const { w, data } = assets.atlas;
    const cols = w / TEX;
    const ox = (tex % cols) * TEX, oy = Math.floor(tex / cols) * TEX;
    const img = new ImageData(TEX, TEX);
    for (let y = 0; y < TEX; y++) {
      for (let x = 0; x < TEX; x++) {
        // clockwise: the target pixel (x, y) shows the source turned back
        let sx = x, sy = y;
        for (let r = 0; r < rot; r++) [sx, sy] = [sy, TEX - 1 - sx];
        const s = ((oy + sy) * w + ox + sx) * 4, d = (y * TEX + x) * 4;
        for (let k = 0; k < 3; k++) img.data[d + k] = (data[s + k] * tint[k]) / 255;
        img.data[d + 3] = data[s + 3] >= 128 ? 255 : 0;
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
      // flowers, grass, rails, lily pads … over it
      const pid = r.plants ? r.plants[idx] : 0;
      if (pid) {
        const plant = assets.blocks[pid];
        const pt = plant.tinted & TINT_TOP ? tintAt(plant, r, idx) : WHITE_RGB;
        ctx.drawImage(plantTexture(plant.top, pt, plant.kind === "flat" ? plant.rot || 0 : 0), i * scale, j * scale, scale, scale);
      }
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

/** Move the decoded arrays of a region into shared memory (only possible when the page is
 *  cross-origin isolated; the map server sends COOP/COEP), so passing them to the other workers
 *  and the page costs nothing. */
function shareEntry(entry) {
  if (!self.crossOriginIsolated) return;
  const share = (a) => {
    if (!a || a.buffer instanceof SharedArrayBuffer) return a;
    const s = new a.constructor(new SharedArrayBuffer(a.byteLength));
    s.set(a);
    return s;
  };
  for (const k of ["h", "c", "b", "bio"]) entry[k] = share(entry[k]);
  if (entry.vox) entry.vox = { off: share(entry.vox.off), ys: share(entry.vox.ys), ids: share(entry.vox.ids) };
}

// ---------- single heights (2D view: y of a picked spot) ----------

const heightTiles = new Map(); // "<dim>/<rx>_<rz>?<v>" -> Promise<Int16Array | null>, last few only

async function heightOf({ dim, v, x, z }) {
  const rx = Math.floor(x / SIZE), rz = Math.floor(z / SIZE);
  const key = `${dim}/${rx}_${rz}?${v}`;
  let p = heightTiles.get(key);
  if (!p) {
    p = fetch(url(`tiles/${dim}/h/${rx}_${rz}.png?v=${v}`))
      .then((r) => (r.ok ? r.arrayBuffer() : null))
      .then((b) => (b ? decodeHeights(b) : null))
      .catch(() => null);
    heightTiles.set(key, p);
    if (heightTiles.size > 4) heightTiles.delete(heightTiles.keys().next().value);
  }
  const h = await p;
  if (!h) return null;
  const y = h[(Math.floor(z) - rz * SIZE) * SIZE + Math.floor(x) - rx * SIZE];
  return y === NONE ? null : y;
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
    if (ok) {
      shareEntry(entry);
      store.set(m.key, entry);
    } else store.delete(m.key);
    // the other workers adopt the decoded data (shared memory when the page is cross-origin
    // isolated, else a copy) instead of decoding the files again
    const data = ok ? { h: entry.h, c: entry.c, b: entry.b, bio: entry.bio, vox: entry.vox } : null;
    postMessage({ type: "loaded", key: m.key, ok, heights: ok ? entry.h : null, entry: data });
  } else if (m.type === "adopt") {
    if (m.entry) store.set(m.key, { ...m.entry, tints: null });
    else store.delete(m.key);
  } else if (m.type === "build") {
    let mesh = null, error;
    try {
      mesh = store.has(m.key) ? build(m.key, m.rx, m.rz, m.step, m.x0, m.z0, m.size, m.fine, m.slice || null) : null;
    } catch (e) {
      error = String(e && e.stack || e); // always answer, or the page would wait forever
    }
    const transfer = [];
    for (const part of mesh ? Object.values(mesh) : []) {
      if (part) transfer.push(part.position.buffer, part.color.buffer, part.uv.buffer, part.tex.buffer, part.index.buffer);
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
  } else if (m.type === "height") {
    postMessage({ type: "height", id: m.id, y: await heightOf(m) });
  } else if (m.type === "drop") {
    store.delete(m.key);
  } else if (m.type === "clear") {
    store.clear();
  }
}
