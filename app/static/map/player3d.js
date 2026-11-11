// A Minecraft player as a 3D model for the 3D map (shown when the camera is close; farther away
// the map shows only the head marker). Built from the skin texture (skins/<uuid>.png, 64×64 or old
// 64×32) with the game's box layout: head, body, arms (slim "Alex" arms: 3 px), legs, each with
// its outer layer slightly larger. Feet at the group's origin, front towards +z, ~1.8 blocks tall.

import * as THREE from "three";

const PX = 0.9375 / 16; // 32 px figure -> 1.875 blocks (the game draws players at 15/16 scale)

// face shades (unlit material): sides a bit darker, the bottom darkest
const SHADE = [0.8, 0.8, 1, 0.55, 0.9, 0.7]; // +x, -x, +y, -y, +z, -z

/** UV rectangles [u, v, w, h] (pixels) of the 6 faces of a box with texture origin (u, v), in
 *  BoxGeometry face order (+x, -x, +y, -y, +z front, -z back). Facing +z the player's right
 *  side is -x. */
function faces(u, v, w, h, d) {
  return [
    [u + d + w, v + d, d, h], // +x: left side
    [u, v + d, d, h],         // -x: right side
    [u + d, v, w, d],         // +y: top
    [u + d + w, v, w, d],     // -y: bottom
    [u + d, v + d, w, h],     // +z: front
    [u + 2 * d + w, v + d, w, h], // -z: back
  ];
}

function box(w, h, d, uv, size, grow = 0) {
  const g = new THREE.BoxGeometry(w + grow * 2, h + grow * 2, d + grow * 2);
  const uvs = g.attributes.uv;
  const colors = new Float32Array(uvs.count * 3);
  const rects = faces(...uv, w, h, d);
  for (let f = 0; f < 6; f++) {
    const [ru, rv, rw, rh] = rects[f];
    // BoxGeometry: 4 vertices per face, uv (0,1) (1,1) (0,0) (1,0) = top-left .. bottom-right
    for (let i = 0; i < 4; i++) {
      const k = f * 4 + i;
      const u0 = uvs.getX(k), v0 = uvs.getY(k);
      let u = ru + u0 * rw, v = rv + (1 - v0) * rh;
      if (f === 3) v = rv + v0 * rh; // the bottom is stored upside down
      uvs.setXY(k, u / size[0], 1 - v / size[1]);
      colors.fill(SHADE[f], k * 3, k * 3 + 3);
    }
  }
  g.setAttribute("color", new THREE.BufferAttribute(colors, 3));
  return g;
}

const loader = new THREE.TextureLoader();

/** Group with the player model; built once the skin is loaded (empty until then / without one). */
export function playerModel(url, onReady) {
  const group = new THREE.Group();
  group.scale.setScalar(PX);
  loader.load(url, (tex) => {
    tex.magFilter = THREE.NearestFilter;
    tex.minFilter = THREE.NearestFilter;
    tex.generateMipmaps = false;
    tex.colorSpace = THREE.SRGBColorSpace;
    const img = tex.image;
    const modern = img.height === 64;
    // slim arms leave x 54-55 of the arm rows empty
    let slim = false;
    const c = document.createElement("canvas");
    c.width = img.width;
    c.height = img.height;
    const ctx = c.getContext("2d", { willReadFrequently: true });
    ctx.drawImage(img, 0, 0);
    if (modern) slim = ctx.getImageData(54, 20, 1, 1).data[3] === 0;
    // old skins often fill the hat layer with solid colour, which the game ignores
    let hat = modern;
    if (!hat) {
      const a = ctx.getImageData(40, 8, 8, 8).data;
      for (let i = 3; i < a.length; i += 4) if (a[i] < 255) hat = true;
    }
    const size = [img.width, img.height];
    const base = new THREE.MeshBasicMaterial({ map: tex, vertexColors: true, alphaTest: 0.5, side: THREE.DoubleSide });
    const aw = slim ? 3 : 4;
    // [w, h, d, inner uv, outer uv | null, x, y, z (centre, px), mirrored]
    const parts = [
      [8, 8, 8, [0, 0], hat ? [32, 0] : null, 0, 28, 0],
      [8, 12, 4, [16, 16], modern ? [16, 32] : null, 0, 18, 0],
      [aw, 12, 4, [40, 16], modern ? [40, 32] : null, -(4 + aw / 2), 18, 0],
      modern ? [aw, 12, 4, [32, 48], [48, 48], 4 + aw / 2, 18, 0] : [aw, 12, 4, [40, 16], null, 4 + aw / 2, 18, 0, true],
      [4, 12, 4, [0, 16], modern ? [0, 32] : null, -2, 6, 0],
      modern ? [4, 12, 4, [16, 48], [0, 48], 2, 6, 0] : [4, 12, 4, [0, 16], null, 2, 6, 0, true],
    ];
    for (const [w, h, d, inner, outer, x, y, z, mirror] of parts) {
      for (const [uv, grow] of outer ? [[inner, 0], [outer, w === 8 && h === 8 ? 0.5 : 0.25]] : [[inner, 0]]) {
        const mesh = new THREE.Mesh(box(w, h, d, uv, size, grow), base);
        mesh.position.set(x, y, z);
        if (mirror) mesh.scale.x = -1; // old skins: the left limbs are the right ones mirrored
        group.add(mesh);
      }
    }
    onReady?.();
  }, undefined, () => {});
  return group;
}

// ---------- player heads placed in the world ----------
// mapgen draws every player head with Steve's face; heads with a skin of their own (heads.json)
// get this box over it: the skin's head (+ hat layer), a little larger than the block model.

const headSkins = new Map(); // url -> Promise<material | null>, shared by all heads with that skin

function headMaterial(url) {
  if (!headSkins.has(url)) {
    headSkins.set(url, new Promise((resolve) => {
      loader.load(url, (tex) => {
        tex.magFilter = THREE.NearestFilter;
        tex.minFilter = THREE.NearestFilter;
        tex.generateMipmaps = false;
        tex.colorSpace = THREE.SRGBColorSpace;
        const img = tex.image;
        let hat = img.height === 64;
        if (!hat) {
          // old skins often fill the hat layer with solid colour, which the game ignores
          const c = document.createElement("canvas");
          c.width = img.width;
          c.height = img.height;
          const ctx = c.getContext("2d", { willReadFrequently: true });
          ctx.drawImage(img, 0, 0);
          const a = ctx.getImageData(40, 8, 8, 8).data;
          for (let i = 3; i < a.length; i += 4) if (a[i] < 255) hat = true;
        }
        const material = new THREE.MeshBasicMaterial({ map: tex, vertexColors: true, alphaTest: 0.5, side: THREE.DoubleSide });
        resolve({ material, hat, size: [img.width, img.height] });
      }, undefined, () => resolve(null));
    }));
  }
  return headSkins.get(url);
}

/** A placed player head: group centred on the head, face towards +z, in block units. Empty until
 *  the skin is loaded (and when there is none: then the map's Steve head stays visible). */
export function headModel(url, onReady) {
  const group = new THREE.Group();
  group.scale.setScalar(1 / 16);
  headMaterial(url).then((skin) => {
    if (!skin) return;
    group.add(new THREE.Mesh(box(8, 8, 8, [0, 0], skin.size, 0.1), skin.material));
    if (skin.hat) group.add(new THREE.Mesh(box(8, 8, 8, [32, 0], skin.size, 0.6), skin.material));
    onReady?.();
  });
  return group;
}

/** Free a head (its skin material stays for the other heads). */
export function disposeHead(group) {
  group.traverse((o) => o.geometry?.dispose());
}

/** Free the GPU resources of a model. */
export function disposeModel(group) {
  const mats = new Set();
  group.traverse((o) => {
    if (o.geometry) o.geometry.dispose();
    if (o.material) mats.add(o.material);
  });
  for (const m of mats) {
    m.map?.dispose();
    m.dispose();
  }
}
