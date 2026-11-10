//! Block textures from the Minecraft client jar (downloaded by the panel from Mojang, never shipped):
//! blockstates -> models -> top/side texture of every block, average colours, biome tints.
//!
//! Output (next to the tiles): `blocks.json` (block and biome registry, ids are stable because the
//! `b/` and `v/` tiles store them) and `atlas.png` (16×16 textures with their real alpha, first
//! animation frame). Both are only rebuilt when the client jar (or `FORMAT`) changes; otherwise
//! `blocks.json` is just read back.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufWriter, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TEX: usize = 16;
const ATLAS_COLS: usize = 32;
/// Bump when the tables change shape: forces a rebuild (and a re-render of all tiles).
const FORMAT: &str = "v8";

/// How the viewer draws a block.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// not drawn (air, torches, fences, entities like chests …)
    #[default]
    None,
    /// a box with top/side/bottom textures
    Cube,
    /// plants: two crossed planes
    Cross,
    /// a plane on the faces of neighbouring blocks (vines, glow lichen)
    Vine,
    /// a horizontal plane just above the block below (rails, lily pads, redstone)
    Flat,
    /// small boxes from the block model (fences, doors, torches …; not solid): see `shape`
    Parts,
}

/// Tint applies to: top, side, bottom (the side overlay is always tinted).
pub const TINT_TOP: u8 = 1;
pub const TINT_SIDE: u8 = 2;
pub const TINT_BOTTOM: u8 = 4;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tint {
    #[default]
    None,
    Grass,
    Foliage,
    Water,
    /// fixed colour (birch/spruce leaves, lily pad)
    Fixed([u8; 3]),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Block {
    pub name: String,
    pub kind: Kind,
    /// a box, seen from above in 2D (plants, rails, torches … are looked through)
    #[serde(default, skip_serializing_if = "is_false")]
    pub visible: bool,
    /// atlas indices of the textures (0 = plain white); planes use `top`
    pub top: u32,
    pub side: u32,
    pub bottom: u32,
    /// tinted overlay over the side (grass block), 0 = none
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub overlay: u32,
    /// average colour of the top texture (before tinting)
    pub color: [u8; 3],
    #[serde(default, skip_serializing_if = "is_no_tint")]
    pub tint: Tint,
    /// faces the tint applies to (TINT_* bits)
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tinted: u8,
    /// see-through texture or fluid (leaves, glass, water): neighbours stay visible
    #[serde(default, skip_serializing_if = "is_false")]
    pub transparent: bool,
    /// the box in 1/16 block: x0, y0, z0, x1, y1, z1 (slabs, cactus, carpets …), default full
    #[serde(rename = "box", default = "full_box", skip_serializing_if = "is_full_box")]
    pub bbox: [u8; 6],
    /// flat blocks: model rotation around y in quarter turns, clockwise seen from above
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rot: u8,
    /// flat blocks: ascending (rails): +1 = higher at the north edge, -1 = at the south edge
    /// (before `rot`)
    #[serde(default, skip_serializing_if = "is_zero_i")]
    pub slope: i8,
    /// detailed blocks (stairs, fences, doors, torches …): index into the `shapes` table
    /// (0 = none); the viewer draws those boxes instead of `box`
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub shape: u32,
    /// detailed blocks: atlas indices of more textures, for the shape's texture slots 3, 4, …
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<u32>,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_no_tint(v: &Tint) -> bool {
    *v == Tint::None
}

fn full_box() -> [u8; 6] {
    FULL
}

fn is_full_box(v: &[u8; 6]) -> bool {
    *v == FULL
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

fn is_zero_i(v: &i8) -> bool {
    *v == 0
}

pub const FULL: [u8; 6] = [0, 0, 0, 16, 16, 16];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Biome {
    pub name: String,
    pub grass: [u8; 3],
    pub foliage: [u8; 3],
    pub water: [u8; 3],
}

#[derive(Serialize, Deserialize, Default)]
struct Registry {
    /// identifies the client jar the tables were built from
    client: String,
    blocks: Vec<Block>,
    biomes: Vec<Biome>,
    /// shapes of detailed blocks, shared by all blocks with the same geometry (all wood stairs …):
    /// a list of boxes, each `PART_LEN` numbers: x0 y0 z0 x1 y1 z1 (1/16 block), then per face
    /// (up, down, north, south, west, east) u0 v0 u1 v1 (texture pixels) and the texture slot
    /// (0 = the block's top, 1 = side, 2 = bottom, 3… = `extra`); a face that is not there is all `NO_FACE`.
    /// Entry 0 is empty.
    #[serde(default)]
    shapes: Vec<Vec<Vec<u8>>>,
}

pub const PART_LEN: usize = 6 + 6 * 5;
const NO_FACE: u8 = 255;

pub struct Assets {
    reg: Registry,
    /// `reg.client` plus a checksum of blocks.json: see `stamp()`
    version: String,
    block_ids: HashMap<String, u16>,
    biome_ids: HashMap<String, u8>,
    /// blocks registered per state ("minecraft:rail[shape=east_west]"): the properties of their
    /// state keys in key order, with the value assumed when a palette entry does not give it
    state_props: HashMap<String, Vec<(String, String)>>,
}

const PLAINS: Biome = Biome { name: String::new(), grass: [145, 189, 89], foliage: [119, 171, 47], water: [63, 118, 228] };

impl Assets {
    /// Load the tables, rebuilding them from the client jar when it changed.
    /// None if there is neither a client jar nor an earlier `blocks.json`.
    pub fn load(client: Option<&Path>, out: &Path) -> Result<Option<Assets>, String> {
        let path = out.join("blocks.json");
        let old: Option<Registry> = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let reg = match client {
            Some(jar) => {
                let stamp = stamp(jar);
                match old {
                    Some(r) if r.client == stamp => r,
                    old => {
                        let reg = build(jar, stamp, old.unwrap_or_default(), out)?;
                        fs::create_dir_all(out).map_err(|e| e.to_string())?;
                        let tmp = out.join("blocks.json.tmp");
                        fs::write(&tmp, serde_json::to_vec(&reg).unwrap_or_default()).map_err(|e| e.to_string())?;
                        fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
                        reg
                    }
                }
            }
            None => match old {
                Some(r) => r,
                None => return Ok(None),
            },
        };
        // the tables are rebuilt from scratch when blocks.json is gone (e.g. a wiped map folder), and
        // the ids may then come out in another order: the version must change with the content
        let version = format!("{}:{:016x}", reg.client, fnv1a(&fs::read(&path).unwrap_or_default()));
        let block_ids = reg.blocks.iter().enumerate().skip(1).map(|(i, b)| (b.name.clone(), i as u16)).collect();
        let biome_ids = reg.biomes.iter().enumerate().skip(1).map(|(i, b)| (b.name.clone(), i as u8)).collect();
        // property -> values seen, per block registered by state
        let mut seen: HashMap<String, Vec<(String, Vec<String>)>> = HashMap::new();
        for b in &reg.blocks {
            if let Some((base, key)) = b.name.strip_suffix(']').and_then(|n| n.split_once('[')) {
                let props = seen.entry(base.to_string()).or_insert_with(|| {
                    key.split(',').filter_map(|kv| kv.split_once('=')).map(|(k, _)| (k.to_string(), vec![])).collect()
                });
                for (k, v) in key.split(',').filter_map(|kv| kv.split_once('=')) {
                    if let Some((_, vals)) = props.iter_mut().find(|(p, _)| p == k) {
                        if !vals.iter().any(|x| x == v) {
                            vals.push(v.to_string());
                        }
                    }
                }
            }
        }
        let state_props = seen
            .into_iter()
            .map(|(base, props)| (base, props.into_iter().map(|(k, vals)| { let d = default_value(&k, &vals); (k, d) }).collect()))
            .collect();
        Ok(Some(Assets { reg, version, block_ids, biome_ids, state_props }))
    }

    /// Identifies the tables: changes with the client jar and with the content of blocks.json.
    /// Viewers cache blocks.json / atlas.png by it and the tiles store ids into the tables, so it
    /// must never stay the same when the ids change.
    pub fn stamp(&self) -> &str {
        &self.version
    }

    /// Id of a block (0 = unknown), with namespace ("minecraft:stone").
    pub fn block_id(&self, name: &str) -> u16 {
        self.block_ids.get(name).copied().unwrap_or(0)
    }

    /// Id of a block in a state (`prop` looks up a property value); falls back to the block's id
    /// for blocks that are not registered per state or a state that is not known.
    pub fn block_state_id<'a>(&self, name: &str, prop: impl Fn(&str) -> Option<&'a str>) -> u16 {
        if let Some(props) = self.state_props.get(name) {
            let mut key = String::from(name);
            for (i, (p, default)) in props.iter().enumerate() {
                let v: &str = prop(p).unwrap_or(default.as_str());
                key.push(if i == 0 { '[' } else { ',' });
                key.push_str(p);
                key.push('=');
                key.push_str(v);
            }
            key.push(']');
            if let Some(&id) = self.block_ids.get(&key) {
                return id;
            }
        }
        self.block_id(name)
    }

    pub fn block(&self, id: u16) -> &Block {
        &self.reg.blocks[id as usize]
    }

    pub fn biome_id(&self, name: &str) -> u8 {
        self.biome_ids.get(name).copied().unwrap_or(0)
    }

    /// Colour of a block in a biome (average texture colour × biome tint).
    pub fn color(&self, block: u16, biome: u8) -> [u8; 3] {
        let b = self.block(block);
        let biome = self.reg.biomes.get(biome as usize).unwrap_or(&self.reg.biomes[0]);
        let tint = match &b.tint {
            Tint::None => return b.color,
            Tint::Grass => biome.grass,
            Tint::Foliage => biome.foliage,
            Tint::Water => biome.water,
            Tint::Fixed(c) => *c,
        };
        [0, 1, 2].map(|k| (b.color[k] as u32 * tint[k] as u32 / 255) as u8)
    }
}

/// 64-bit FNV-1a checksum.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Size + modification time: cheap and changes with every new jar.
fn stamp(jar: &Path) -> String {
    let m = fs::metadata(jar).ok();
    let len = m.as_ref().map_or(0, |m| m.len());
    let mtime = m
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    format!("{FORMAT}:{}:{len}:{mtime}", jar.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

// ---------- reading the jar ----------

struct Jar {
    zip: zip::ZipArchive<File>,
    json_cache: HashMap<String, Option<Value>>,
}

impl Jar {
    fn bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        let mut f = self.zip.by_name(name).ok()?;
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf).ok()?;
        Some(buf)
    }

    fn json(&mut self, name: &str) -> Option<Value> {
        if let Some(v) = self.json_cache.get(name) {
            return v.clone();
        }
        let v = self.bytes(name).and_then(|b| serde_json::from_slice(&b).ok());
        self.json_cache.insert(name.to_string(), v.clone());
        v
    }

    fn names(&self, prefix: &str, suffix: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .zip
            .file_names()
            .filter_map(|n| n.ok())
            .filter(|n| n.starts_with(prefix) && n.ends_with(suffix))
            .map(|n| n.into_owned())
            .collect();
        v.sort();
        v
    }
}

/// "minecraft:block/stone" / "block/stone" -> "block/stone" (None for other namespaces)
fn resource(name: &str) -> Option<&str> {
    match name.split_once(':') {
        Some(("minecraft", rest)) => Some(rest),
        Some(_) => None,
        None => Some(name),
    }
}

/// Model as needed for a map: merged texture variables and the elements.
struct Model {
    textures: HashMap<String, String>,
    elements: Vec<Value>,
}

fn load_model(jar: &mut Jar, name: &str) -> Option<Model> {
    let mut textures: HashMap<String, String> = HashMap::new();
    let mut elements: Option<Vec<Value>> = None;
    let mut current = resource(name)?.to_string();
    for _ in 0..16 {
        let Some(v) = jar.json(&format!("assets/minecraft/models/{current}.json")) else { break };
        if let Some(t) = v.get("textures").and_then(Value::as_object) {
            for (k, val) in t {
                // "block/x" or (26.x) {"sprite": "block/x", "force_translucent": true}
                if let Some(s) = val.as_str().or_else(|| val.get("sprite").and_then(Value::as_str)) {
                    textures.entry(k.clone()).or_insert_with(|| s.to_string()); // child wins
                }
            }
        }
        if elements.is_none() {
            elements = v.get("elements").and_then(Value::as_array).cloned();
        }
        match v.get("parent").and_then(Value::as_str).and_then(resource) {
            Some(p) => current = p.to_string(),
            None => break,
        }
    }
    Some(Model { textures, elements: elements.unwrap_or_default() })
}

/// "#side" -> "block/oak_log" following the texture variables.
fn resolve_texture<'a>(model: &'a Model, mut t: &'a str) -> Option<String> {
    for _ in 0..10 {
        match t.strip_prefix('#') {
            Some(var) => t = model.textures.get(var)?,
            None => return resource(t).map(String::from),
        }
    }
    None
}

/// The model a block shows in its most common state.
fn block_model(jar: &mut Jar, block: &str) -> Option<String> {
    let v = jar.json(&format!("assets/minecraft/blockstates/{block}.json"))?;
    let pick = |x: &Value| -> Option<String> {
        let x = if let Some(a) = x.as_array() { a.first()? } else { x };
        x.get("model")?.as_str().map(String::from)
    };
    if let Some(variants) = v.get("variants").and_then(Value::as_object) {
        // prefer upright / bottom / not-snowy states (keys are sorted, "axis=x" would come first)
        const PREFERRED: [&str; 7] = ["axis=y", "snowy=false", "half=bottom", "type=bottom", "facing=north", "age=0", "shape=north_south"];
        let key = variants
            .keys()
            .find(|k| k.is_empty())
            .or_else(|| PREFERRED.iter().find_map(|p| variants.keys().find(|k| k.contains(p))))
            .or_else(|| variants.keys().next())?;
        return pick(&variants[key]);
    }
    let parts = v.get("multipart")?.as_array()?;
    parts.iter().find_map(|p| pick(p.get("apply")?))
}

/// The states of a block whose variants differ by `shape` (rails): (state key, model, quarter turns).
fn shape_states(jar: &mut Jar, block: &str) -> Vec<(String, String, u8)> {
    let Some(v) = jar.json(&format!("assets/minecraft/blockstates/{block}.json")) else { return vec![] };
    let Some(variants) = v.get("variants").and_then(Value::as_object) else { return vec![] };
    variants
        .iter()
        .filter(|(k, _)| k.split(',').any(|kv| kv.starts_with("shape=")))
        .filter_map(|(k, x)| {
            let x = if let Some(a) = x.as_array() { a.first()? } else { x };
            let model = x.get("model")?.as_str()?.to_string();
            let rot = (x.get("y").and_then(Value::as_i64).unwrap_or(0).rem_euclid(360) / 90) as u8;
            Some((k.clone(), model, rot))
        })
        .collect()
}

/// Ascending planes (raised rails, rotated 45° around x): +1 = higher at the north edge.
fn slope(model: &Model) -> i8 {
    model
        .elements
        .iter()
        .find_map(|e| {
            let r = e.get("rotation")?;
            (r.get("axis")?.as_str()? == "x").then(|| r.get("angle")?.as_f64()).flatten()
        })
        .map_or(0, |a| if a > 0.0 { 1 } else if a < 0.0 { -1 } else { 0 })
}

/// The value a block state property most likely has when a palette entry leaves it out
/// (its default state).
fn default_value(prop: &str, values: &[String]) -> String {
    const PREFERRED: [&str; 10] = ["north", "bottom", "lower", "straight", "north_south", "false", "left", "none", "floor", "y"];
    // walls stand with a post, rods and chains upright
    let pref: &[&str] = match prop {
        "up" => &["true"],
        "facing" if values.iter().any(|v| v == "up") => &["up"],
        _ => &PREFERRED,
    };
    pref.iter()
        .find(|p| values.iter().any(|v| v == *p))
        .map(|p| p.to_string())
        .or_else(|| values.first().cloned())
        .unwrap_or_default()
}

/// Blocks drawn from their model's boxes, registered per state.
fn detailed(short: &str) -> bool {
    const SUFFIXES: [&str; 14] = [
        "_stairs", "_fence", "_fence_gate", "_wall", "_pane", "_bars", "_door", "_trapdoor", "torch", "lantern", "chain", "_rod", "_bed", "_sign",
    ];
    SUFFIXES.iter().any(|s| short.ends_with(s)) && !short.ends_with("o_lantern") && short != "sea_lantern"
}

/// A model as a blockstate applies it.
#[derive(Clone, Debug, PartialEq)]
struct Applied {
    model: String,
    x: i64,
    y: i64,
    uvlock: bool,
}

fn applied(x: &Value) -> Option<Applied> {
    let x = if let Some(a) = x.as_array() { a.first()? } else { x };
    Some(Applied {
        model: x.get("model")?.as_str()?.to_string(),
        x: x.get("x").and_then(Value::as_i64).unwrap_or(0),
        y: x.get("y").and_then(Value::as_i64).unwrap_or(0),
        uvlock: x.get("uvlock").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Properties and the values a multipart condition tests.
fn condition_props(when: &Value, out: &mut BTreeMap<String, Vec<String>>) {
    let Some(obj) = when.as_object() else { return };
    for (k, v) in obj {
        if k == "OR" || k == "AND" {
            for c in v.as_array().into_iter().flatten() {
                condition_props(c, out);
            }
        } else if let Some(v) = v.as_str() {
            let vals = out.entry(k.clone()).or_default();
            for v in v.split('|') {
                if !vals.iter().any(|x| x == v) {
                    vals.push(v.to_string());
                }
            }
        }
    }
}

fn condition_holds(when: &Value, state: &BTreeMap<String, String>) -> bool {
    let Some(obj) = when.as_object() else { return true };
    obj.iter().all(|(k, v)| match k.as_str() {
        "OR" => v.as_array().is_some_and(|a| a.iter().any(|c| condition_holds(c, state))),
        "AND" => v.as_array().is_none_or(|a| a.iter().all(|c| condition_holds(c, state))),
        _ => v.as_str().is_some_and(|v| state.get(k).is_some_and(|s| v.split('|').any(|x| x == s))),
    })
}

/// Every state of a blockstate file that looks different: (state key, models). Variants as
/// listed; multipart blocks (fences, walls, panes) with every combination of the tested properties.
fn block_states(v: &Value) -> Vec<(String, Vec<Applied>)> {
    if let Some(variants) = v.get("variants").and_then(Value::as_object) {
        return variants.iter().filter_map(|(k, x)| Some((k.clone(), vec![applied(x)?]))).collect();
    }
    let Some(parts) = v.get("multipart").and_then(Value::as_array) else { return vec![] };
    let mut props = BTreeMap::new();
    for p in parts {
        if let Some(w) = p.get("when") {
            condition_props(w, &mut props);
        }
    }
    // values that never appear in a condition: the "off" value of the property
    for vals in props.values_mut() {
        let other = if vals.iter().all(|v| v == "true") {
            "false"
        } else if vals.iter().all(|v| ["low", "tall", "side", "up"].contains(&v.as_str())) {
            "none"
        } else {
            continue;
        };
        vals.push(other.to_string());
    }
    if props.values().map(Vec::len).product::<usize>() > 1024 {
        return vec![];
    }
    let mut states: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
    for (k, vals) in &props {
        states = states
            .into_iter()
            .flat_map(|s| {
                vals.iter().map(move |v| {
                    let mut s = s.clone();
                    s.insert(k.clone(), v.clone());
                    s
                })
            })
            .collect();
    }
    states
        .into_iter()
        .map(|s| {
            let key = s.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
            let models = parts
                .iter()
                .filter(|p| p.get("when").is_none_or(|w| condition_holds(w, &s)))
                .filter_map(|p| applied(p.get("apply")?))
                .collect();
            (key, models)
        })
        .collect()
}

const DIR_NAMES: [&str; 6] = ["up", "down", "north", "south", "west", "east"];
const UP: usize = 0;
const DOWN: usize = 1;
const NORTH: usize = 2;
const SOUTH: usize = 3;
const WEST: usize = 4;
const EAST: usize = 5;

/// Texture pixels a face shows by default (Minecraft's automatic UV, also used with `uvlock`).
fn auto_uv(dir: usize, b: [f64; 6]) -> [f64; 4] {
    match dir {
        UP => [b[0], b[2], b[3], b[5]],
        DOWN => [b[0], 16.0 - b[5], b[3], 16.0 - b[2]],
        NORTH => [16.0 - b[3], 16.0 - b[4], 16.0 - b[0], 16.0 - b[1]],
        SOUTH => [b[0], 16.0 - b[4], b[3], 16.0 - b[1]],
        WEST => [b[2], 16.0 - b[4], b[5], 16.0 - b[1]],
        _ => [16.0 - b[5], 16.0 - b[4], 16.0 - b[2], 16.0 - b[1]],
    }
}

/// Rotates a point (1/16 block) and a face direction by the blockstate's x then y rotation
/// (y: clockwise seen from above, like the game).
fn rotate(mut p: [f64; 3], mut dir: usize, a: &Applied) -> ([f64; 3], usize) {
    for _ in 0..(a.x.rem_euclid(360) / 90) {
        p = [p[0], 16.0 - p[2], p[1]];
        dir = match dir {
            UP => SOUTH,
            SOUTH => DOWN,
            DOWN => NORTH,
            NORTH => UP,
            d => d,
        };
    }
    for _ in 0..(a.y.rem_euclid(360) / 90) {
        p = [16.0 - p[2], p[1], p[0]];
        dir = match dir {
            NORTH => EAST,
            EAST => SOUTH,
            SOUTH => WEST,
            WEST => NORTH,
            d => d,
        };
    }
    (p, dir)
}

/// Boxes of a model as the blockstate applies it, in the `shapes` part format. `slot` maps a
/// face's texture to the block's texture slot (top, side, bottom, extra …). Planes (iron bars,
/// chains) are kept as boxes of zero thickness; tilted boxes are moved, not tilted, except tall
/// ones leaning sideways (wall torches): those are cut into short pieces moved one by one, a
/// staircase that leans like the model.
fn model_parts(model: &Model, a: &Applied, mut slot: impl FnMut(&str) -> u8) -> Vec<Vec<u8>> {
    let mut out = vec![];
    for e in &model.elements {
        let (Some(from), Some(to)) = (e.get("from"), e.get("to")) else { continue };
        let lo0 = [0, 1, 2].map(|i| num(from, i).min(num(to, i)));
        let hi0 = [0, 1, 2].map(|i| num(from, i).max(num(to, i)));
        if (0..3).filter(|&i| hi0[i] - lo0[i] <= 0.0).count() > 1 {
            continue;
        }
        let Some(faces) = e.get("faces").and_then(Value::as_object) else { continue };
        let orig = [lo0[0], lo0[1], lo0[2], hi0[0], hi0[1], hi0[2]];
        let rotation = e.get("rotation").map(|r| {
            let angle = r.get("angle").and_then(Value::as_f64).unwrap_or(0.0);
            let o = r.get("origin").map_or([8.0; 3], |o| [0, 1, 2].map(|i| num(o, i)));
            (angle, o, r.get("axis").and_then(Value::as_str).unwrap_or(""))
        });
        let height = hi0[1] - lo0[1];
        let leaning = rotation.is_some_and(|(angle, _, axis)| angle.abs() >= 10.0 && (axis == "x" || axis == "z"));
        let pieces = if leaning && height >= 6.0 { (height / 3.5).ceil() as usize } else { 1 };
        for piece in 0..pieces {
            let (f0, f1) = (piece as f64 / pieces as f64, (piece + 1) as f64 / pieces as f64);
            let mut lo = lo0;
            let mut hi = hi0;
            lo[1] = lo0[1] + height * f0;
            hi[1] = lo0[1] + height * f1;
            if let Some((angle, o, axis)) = rotation {
                let c = [0, 1, 2].map(|i| (lo[i] + hi[i]) / 2.0 - o[i]);
                let (sin, cos) = angle.to_radians().sin_cos();
                let m = match axis {
                    "x" => [c[0], c[1] * cos - c[2] * sin, c[1] * sin + c[2] * cos],
                    "y" => [c[0] * cos + c[2] * sin, c[1], -c[0] * sin + c[2] * cos],
                    "z" => [c[0] * cos - c[1] * sin, c[0] * sin + c[1] * cos, c[2]],
                    _ => c,
                };
                for i in 0..3 {
                    lo[i] += m[i] - c[i];
                    hi[i] += m[i] - c[i];
                }
            }
            let (p0, _) = rotate(lo, UP, a);
            let (p1, _) = rotate(hi, UP, a);
            let b = [0, 1, 2].map(|i| p0[i].min(p1[i]).clamp(0.0, 16.0).round());
            let t = [0, 1, 2].map(|i| p0[i].max(p1[i]).clamp(0.0, 16.0).round());
            if (0..3).any(|i| t[i] < b[i]) || (0..3).filter(|&i| t[i] == b[i]).count() > 1 {
                continue;
            }
            let bx = [b[0], b[1], b[2], t[0], t[1], t[2]];
            let mut part = bx.map(|v| v as u8).to_vec();
            part.resize(PART_LEN, NO_FACE);
            for (d, name) in DIR_NAMES.iter().enumerate() {
                // of a cut box only the lowest piece has the bottom, the highest the top
                if (d == DOWN && piece > 0) || (d == UP && piece + 1 < pieces) {
                    continue;
                }
                let Some(f) = faces.get(*name) else { continue };
                let Some(tex) = f.get("texture").and_then(Value::as_str).and_then(|t| resolve_texture(model, t)) else { continue };
                let (_, to_dir) = rotate([0.0; 3], d, a);
                let given = f.get("uv").filter(|u| u.as_array().is_some_and(|u| u.len() == 4));
                let mut uv = match given {
                    _ if a.uvlock => auto_uv(to_dir, bx),
                    Some(u) => [0, 1, 2, 3].map(|i| num(u, i)),
                    None => auto_uv(d, orig),
                };
                if pieces > 1 && d != UP && d != DOWN && !a.uvlock {
                    // the piece's share of the side texture (v grows downwards)
                    let (v0, v1) = (uv[1], uv[3]);
                    uv[1] = v0 + (1.0 - f1) * (v1 - v0);
                    uv[3] = v0 + (1.0 - f0) * (v1 - v0);
                }
                let o = 6 + to_dir * 5;
                for (k, v) in uv.iter().enumerate() {
                    part[o + k] = v.clamp(0.0, 16.0).round() as u8;
                }
                part[o + 4] = slot(&tex);
            }
            if part[6..].iter().any(|&v| v != NO_FACE) {
                out.push(part);
            }
        }
    }
    out
}

/// Top, side and bottom texture (with "is tinted") of detailed models: up/down face of the
/// widest box, side face of the biggest one.
fn part_textures(models: &[&Model]) -> Faces {
    let mut best: [Option<(f64, String, bool)>; 3] = [None, None, None];
    for m in models {
        for e in &m.elements {
            let (Some(from), Some(to)) = (e.get("from"), e.get("to")) else { continue };
            let size = [0, 1, 2].map(|i| (num(to, i) - num(from, i)).abs());
            let Some(faces) = e.get("faces").and_then(Value::as_object) else { continue };
            let mut take = |slot: usize, score: f64, dirs: &[&str]| {
                let Some(f) = dirs.iter().find_map(|d| faces.get(*d)) else { return };
                let Some(tex) = f.get("texture").and_then(Value::as_str).and_then(|t| resolve_texture(m, t)) else { return };
                if best[slot].as_ref().is_none_or(|(s, _, _)| score > *s) {
                    best[slot] = Some((score, tex, f.get("tintindex").is_some()));
                }
            };
            take(0, size[0] * size[2], &["up"]);
            take(1, size[0] * size[1] * size[2], &["north", "south", "east", "west"]);
            take(2, size[0] * size[2], &["down"]);
        }
    }
    let [top, side, bottom] = best.map(|b| b.map(|(_, t, tinted)| (t, tinted)));
    let side = side.or_else(|| top.clone());
    Faces { bbox: FULL, kind: Kind::Parts, top: top.or_else(|| side.clone()), bottom: bottom.or_else(|| side.clone()), side, overlay: None }
}

fn num(v: &Value, i: usize) -> f64 {
    v.get(i).and_then(Value::as_f64).unwrap_or(0.0)
}

struct Faces {
    bbox: [u8; 6],
    kind: Kind,
    top: Option<(String, bool)>,
    side: Option<(String, bool)>,
    bottom: Option<(String, bool)>,
    overlay: Option<String>,
}

const VINES: [&str; 4] = ["vine", "glow_lichen", "sculk_vein", "resin_clump"];

/// Shape and textures (with "is tinted") of a block model.
fn faces(model: &Model, short: &str) -> Faces {
    let none = Faces { bbox: FULL, kind: Kind::None, top: None, side: None, bottom: None, overlay: None };
    let face_of = |e: &Value, dirs: &[&str]| {
        dirs.iter().find_map(|d| {
            let f = e.get("faces")?.get(*d)?;
            let tex = resolve_texture(model, f.get("texture")?.as_str()?)?;
            Some((tex, f.get("tintindex").is_some()))
        })
    };
    // the biggest real box decides; thin things (torches, rods, fence posts) are not drawn
    let mut best: Option<(&Value, f64)> = None;
    let mut plane: Option<(&Value, [f64; 3])> = None;
    for e in &model.elements {
        let (Some(from), Some(to)) = (e.get("from"), e.get("to")) else { continue };
        let size = [0, 1, 2].map(|i| (num(to, i) - num(from, i)).abs());
        if size.contains(&0.0) {
            plane = plane.or(Some((e, size)));
            continue;
        }
        if size[0] < 8.0 || size[2] < 8.0 {
            continue;
        }
        let area = size[0] * size[2];
        if best.is_none_or(|(_, a)| area > a) {
            best = Some((e, area));
        }
    }
    if best.is_some() {
        // every face from the box that has it (cactus: top/bottom from a full box, the sides from
        // two inset ones); the box of the block follows from where those faces lie
        let boxes: Vec<&Value> = model
            .elements
            .iter()
            .filter(|e| {
                let (Some(from), Some(to)) = (e.get("from"), e.get("to")) else { return false };
                let size = [0, 1, 2].map(|i| (num(to, i) - num(from, i)).abs());
                !size.contains(&0.0) && size[0] >= 8.0 && size[2] >= 8.0
            })
            .collect();
        let owner = |dir: &str| boxes.iter().copied().find(|e| e.get("faces").and_then(|f| f.get(dir)).is_some());
        let coord = |e: Option<&Value>, k: &str, i: usize, default: u8| {
            e.and_then(|e| e.get(k)).map_or(default, |v| num(v, i).round().clamp(0.0, 16.0) as u8)
        };
        let main = best.map(|(e, _)| e);
        let (up, down, north, south, west, east) =
            (owner("up"), owner("down"), owner("north"), owner("south"), owner("west"), owner("east"));
        let bbox = [
            coord(west.or(main), "from", 0, 0),
            coord(down.or(main), "from", 1, 0),
            coord(north.or(main), "from", 2, 0),
            coord(east.or(main), "to", 0, 16),
            coord(up.or(main), "to", 1, 16),
            coord(south.or(main), "to", 2, 16),
        ];
        let face = |e: Option<&Value>, dir: &str| e.and_then(|e| face_of(e, &[dir]));
        let overlay = model.textures.contains_key("overlay").then(|| resolve_texture(model, "#overlay")).flatten();
        let side = face(north, "north").or_else(|| face(south, "south")).or_else(|| face(east, "east")).or_else(|| face(west, "west"));
        let top = face(up, "up").or_else(|| side.clone());
        return Faces {
            bbox,
            kind: Kind::Cube,
            bottom: face(down, "down").or_else(|| top.clone()),
            side: side.or_else(|| top.clone()),
            top,
            overlay,
        };
    }
    let Some((e, size)) = plane else { return none };
    let tex = face_of(e, &["north", "south", "east", "west", "up", "down"]);
    let kind = if VINES.contains(&short) {
        Kind::Vine
    } else if size[1] == 0.0 {
        Kind::Flat
    } else {
        Kind::Cross
    };
    Faces { bbox: FULL, kind, top: tex.clone(), side: tex.clone(), bottom: tex, overlay: None }
}

fn tint_for(block: &str, tinted: bool) -> Tint {
    let name = block.trim_start_matches("minecraft:");
    match name {
        "water" | "bubble_column" => Tint::Water,
        "birch_leaves" => Tint::Fixed([128, 167, 85]),
        "spruce_leaves" => Tint::Fixed([97, 153, 97]),
        "lily_pad" => Tint::Fixed([32, 128, 48]),
        _ if !tinted => Tint::None,
        _ if name.ends_with("leaves") || name.contains("vine") => Tint::Foliage,
        _ => Tint::Grass,
    }
}

/// A texture of the jar as RGBA pixels with width and height.
fn decode_png(jar: &mut Jar, tex: &str) -> Option<(Vec<[u8; 4]>, usize, usize)> {
    let bytes = jar.bytes(&format!("assets/minecraft/textures/{tex}.png"))?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = info.color_type.samples();
    let px = buf[..w * h * channels]
        .chunks_exact(channels)
        .map(|px| match channels {
            1 => [px[0], px[0], px[0], 255],
            2 => [px[0], px[0], px[0], px[1]],
            3 => [px[0], px[1], px[2], 255],
            _ => [px[0], px[1], px[2], px[3]],
        })
        .collect();
    Some((px, w, h))
}

/// A rectangle of a crop spec: x, y, w, h (in base pixels), upside down.
type Rect = ([usize; 4], bool);

fn crop_rects(rects: &str) -> Vec<Rect> {
    rects
        .split(';')
        .filter_map(|r| {
            let v: Vec<&str> = r.split(',').collect();
            let n = |i: usize| v.get(i)?.parse::<usize>().ok();
            Some(([n(0)?, n(1)?, n(2)?, n(3)?], v.get(4) == Some(&"v")))
        })
        .collect()
}

/// First animation frame of a texture as 16×16 RGBA. `crop|<texture>|<base width>|x,y,w,h[,v];…`
/// (entity textures: chests, heads) cuts rectangles out of a texture (coordinates for the texture
/// at `base width`), stacks them top to bottom (`v`: upside down) and scales that to 16×16.
fn texture_rgba(jar: &mut Jar, tex: &str) -> Option<Vec<u8>> {
    if let Some(spec) = tex.strip_prefix("crop|") {
        let mut it = spec.split('|');
        let (name, base, rects) = (it.next()?, it.next()?.parse::<usize>().ok()?, crop_rects(it.next()?));
        let (px, w, h) = decode_png(jar, name)?;
        let k = w as f64 / base as f64;
        let width = rects.iter().map(|r| r.0[2]).max()?;
        // rows of the stacked image: (rectangle, row in it)
        let mut rows: Vec<(usize, usize)> = vec![];
        for (i, (r, flip)) in rects.iter().enumerate() {
            for y in 0..r[3] {
                rows.push((i, if *flip { r[3] - 1 - y } else { y }));
            }
        }
        if width == 0 || rows.is_empty() {
            return None;
        }
        let mut out = vec![0u8; TEX * TEX * 4];
        for y in 0..TEX {
            let (ri, ry) = rows[y * rows.len() / TEX];
            let r = rects[ri].0;
            for x in 0..TEX {
                let rx = (x * width / TEX).min(r[2].max(1) - 1);
                let (sx, sy) = (((r[0] + rx) as f64 * k) as usize, ((r[1] + ry) as f64 * k) as usize);
                if sx < w && sy < h {
                    out[(y * TEX + x) * 4..(y * TEX + x) * 4 + 4].copy_from_slice(&px[sy * w + sx]);
                }
            }
        }
        return Some(out);
    }
    let (px, w, h) = decode_png(jar, tex)?;
    if w == 0 || h < w {
        return None;
    }
    let mut out = vec![0u8; TEX * TEX * 4];
    for y in 0..TEX {
        for x in 0..TEX {
            let (sx, sy) = (x * w / TEX, y * w / TEX); // first frame = top w×w square
            out[(y * TEX + x) * 4..(y * TEX + x) * 4 + 4].copy_from_slice(&px[sy * w + sx]);
        }
    }
    Some(out)
}

/// Blocks the game draws as entities (no model in the jar): chests and mob heads, as boxes with
/// textures cut out of the entity textures. Per state: (state key, models as applied).
fn entity_models(short: &str) -> Option<Vec<(String, Vec<(Model, Applied)>)>> {
    const QUARTERS: [(&str, i64); 4] = [("north", 0), ("east", 90), ("south", 180), ("west", 270)];
    let model = |elements: Value, textures: &[(&str, String)]| Model {
        textures: textures.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        elements: elements.as_array().cloned().unwrap_or_default(),
    };
    let applied = |y: i64| Applied { model: String::new(), x: 0, y, uvlock: false };
    // one box, the front on the north side (turned by the state)
    let cuboid = |from: [u8; 3], to: [u8; 3]| {
        let face = |t: &str| serde_json::json!({"uv": [0, 0, 16, 16], "texture": t});
        serde_json::json!([{ "from": from, "to": to, "faces": {
            "up": face("#top"), "down": face("#bottom"), "north": face("#front"),
            "south": face("#back"), "west": face("#side"), "east": face("#side")
        }}])
    };
    // chests: lid (14×5×14) over body (14×10×14) of the 64×64 chest texture, the lock on the front
    let chest = match short.trim_start_matches("waxed_") {
        "chest" => Some("normal"),
        "trapped_chest" => Some("trapped"),
        "ender_chest" => Some("ender"),
        "copper_chest" => Some("copper"),
        "exposed_copper_chest" => Some("copper_exposed"),
        "weathered_copper_chest" => Some("copper_weathered"),
        "oxidized_copper_chest" => Some("copper_oxidized"),
        _ => None,
    };
    if let Some(t) = chest {
        let crop = |rects: &str| format!("crop|entity/chest/{t}|64|{rects}");
        let tex = [
            ("top", crop("28,0,14,14")),
            ("bottom", crop("14,19,14,14")),
            ("front", crop("42,14,14,5;42,33,14,10,v")),
            ("back", crop("14,14,14,5;14,33,14,10,v")),
            ("side", crop("0,14,14,5;0,33,14,10,v")),
        ];
        let mut out = vec![];
        for (facing, y) in QUARTERS {
            // the halves of a double chest reach the block edge where they meet (a left half of a
            // chest facing north: east)
            for (kind, x0, x1) in [("single", 1, 15), ("left", 1, 16), ("right", 0, 15)] {
                out.push((format!("facing={facing},type={kind}"), vec![(model(cuboid([x0, 0, 1], [x1, 14, 15]), &tex), applied(y))]));
            }
        }
        return Some(out);
    }
    // heads: the 8×8×8 head of the mob texture (piglin: 10 wide)
    let (base, wall) = match short.strip_suffix("_wall_skull").or_else(|| short.strip_suffix("_wall_head")) {
        Some(b) => (b, true),
        None => (short.strip_suffix("_skull").or_else(|| short.strip_suffix("_head"))?, false),
    };
    let texture = match base {
        "skeleton" => "entity/skeleton/skeleton",
        "wither_skeleton" => "entity/skeleton/wither_skeleton",
        "zombie" => "entity/zombie/zombie",
        "creeper" => "entity/creeper/creeper",
        "piglin" => "entity/piglin/piglin",
        "player" => "entity/player/wide/steve",
        _ => return None,
    };
    let crop = |rect: &str| format!("crop|{texture}|64|{rect}");
    let (tex, x0, x1) = if base == "piglin" {
        ([("top", crop("8,0,10,8")), ("bottom", crop("18,0,10,8")), ("front", crop("8,8,10,8")), ("back", crop("26,8,10,8")), ("side", crop("0,8,8,8"))], 3, 13)
    } else {
        ([("top", crop("8,0,8,8")), ("bottom", crop("16,0,8,8")), ("front", crop("8,8,8,8")), ("back", crop("24,8,8,8")), ("side", crop("0,8,8,8"))], 4, 12)
    };
    let mut out = vec![];
    if wall {
        // facing = where the face looks; the head hangs on the wall behind it
        for (facing, y) in QUARTERS {
            out.push((format!("facing={facing}"), vec![(model(cuboid([x0, 4, 8], [x1, 12, 16]), &tex), applied(y))]));
        }
    } else {
        // rotation 0 looks south, 4 west, 8 north, 12 east (to the nearest quarter)
        for r in 0..16 {
            let y = [180, 270, 0, 90][((r + 2) / 4) % 4];
            out.push((format!("rotation={r}"), vec![(model(cuboid([x0, 0, 4], [x1, 8, 12]), &tex), applied(y))]));
        }
    }
    Some(out)
}

/// Average colour of the visible pixels and whether the texture has see-through pixels.
fn average(rgba: &[u8]) -> ([u8; 3], bool) {
    let (mut sum, mut n, mut alpha) = ([0u64; 3], 0u64, false);
    for p in rgba.chunks_exact(4) {
        if p[3] > 0 {
            for k in 0..3 {
                sum[k] += p[k] as u64;
            }
            n += 1;
        }
        alpha |= p[3] < 255;
    }
    (if n == 0 { [128, 128, 128] } else { sum.map(|s| (s / n) as u8) }, alpha)
}

fn hex(s: Option<&str>) -> Option<[u8; 3]> {
    let v = u32::from_str_radix(s?.trim_start_matches('#'), 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Vanilla colormap lookup (grass.png / foliage.png, 256×256).
fn colormap(map: &Option<(Vec<u8>, usize)>, temperature: f64, downfall: f64) -> Option<[u8; 3]> {
    let (rgba, w) = map.as_ref()?;
    let t = temperature.clamp(0.0, 1.0);
    let d = downfall.clamp(0.0, 1.0) * t;
    let x = ((1.0 - t) * 255.0) as usize;
    let y = ((1.0 - d) * 255.0) as usize;
    let p = (y.min(w - 1) * w + x.min(w - 1)) * 4;
    rgba.get(p..p + 3).map(|c| [c[0], c[1], c[2]])
}

fn read_colormap(jar: &mut Jar, name: &str) -> Option<(Vec<u8>, usize)> {
    let bytes = jar.bytes(&format!("assets/minecraft/textures/colormap/{name}.png"))?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    Some((buf, info.width as usize))
}

fn biome_from_json(name: String, v: &Value, grass_map: &Option<(Vec<u8>, usize)>, foliage_map: &Option<(Vec<u8>, usize)>) -> Biome {
    let temperature = v.get("temperature").and_then(Value::as_f64).unwrap_or(0.8);
    let downfall = v.get("downfall").and_then(Value::as_f64).unwrap_or(0.4);
    let effects = v.get("effects");
    let effect = |k: &str| hex(effects.and_then(|e| e.get(k)).and_then(Value::as_str));
    let mut grass = effect("grass_color").or_else(|| colormap(grass_map, temperature, downfall)).unwrap_or(PLAINS.grass);
    match effects.and_then(|e| e.get("grass_color_modifier")).and_then(Value::as_str) {
        Some("swamp") => grass = [106, 112, 57],
        Some("dark_forest") => {
            let c = (((grass[0] as u32) << 16) | ((grass[1] as u32) << 8) | grass[2] as u32) & 0xFEFEFE;
            let c = (c + 0x28340A) >> 1;
            grass = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
        }
        _ => {}
    }
    let foliage = effect("foliage_color").or_else(|| colormap(foliage_map, temperature, downfall)).unwrap_or(PLAINS.foliage);
    let water = effect("water_color").unwrap_or(PLAINS.water);
    Biome { name, grass, foliage, water }
}

/// Build the registry from the jar, keeping the ids of `old` and appending new names.
fn build(jar_path: &Path, stamp: String, old: Registry, out: &Path) -> Result<Registry, String> {
    let file = File::open(jar_path).map_err(|e| format!("client jar: {e}"))?;
    let zip = zip::ZipArchive::new(file).map_err(|e| format!("client jar: {e}"))?;
    let mut jar = Jar { zip, json_cache: HashMap::new() };

    // ----- blocks -----
    // texture name -> (atlas index, average colour, has see-through pixels)
    let mut textures: BTreeMap<String, (u32, [u8; 3], bool)> = BTreeMap::new();
    let mut atlas: Vec<Vec<u8>> = vec![vec![255; TEX * TEX * 4]]; // 0 = white
    let mut texture = |jar: &mut Jar, name: &str| -> Option<(u32, [u8; 3], bool)> {
        if let Some(&t) = textures.get(name) {
            return Some(t);
        }
        let rgba = texture_rgba(jar, name)?;
        let (avg, alpha) = average(&rgba);
        let t = (atlas.len() as u32, avg, alpha);
        atlas.push(rgba);
        textures.insert(name.to_string(), t);
        Some(t)
    };

    let mut built: HashMap<String, Block> = HashMap::new();
    let mut shapes: Vec<Vec<Vec<u8>>> = vec![vec![]];
    let mut shape_ids: HashMap<Vec<Vec<u8>>, u32> = HashMap::new();
    for path in jar.names("assets/minecraft/blockstates/", ".json") {
        let short = path.trim_start_matches("assets/minecraft/blockstates/").trim_end_matches(".json").to_string();
        let name = format!("minecraft:{short}");
        let mut block = Block {
            name: name.clone(),
            kind: Kind::None,
            visible: false,
            top: 0,
            side: 0,
            bottom: 0,
            overlay: 0,
            color: [0, 0, 0],
            tint: Tint::None,
            tinted: 0,
            transparent: false,
            bbox: FULL,
            rot: 0,
            slope: 0,
            shape: 0,
            extra: vec![],
        };
        // fluids have no model elements; mushroom blocks are built from single faces
        let mushroom = format!("block/{short}");
        let whole = match short.as_str() {
            "water" | "bubble_column" => Some("block/water_still"),
            "lava" => Some("block/lava_still"),
            s if s.ends_with("_mushroom_block") || s == "mushroom_stem" => Some(mushroom.as_str()),
            _ => None,
        };
        if let Some(tex) = whole {
            if let Some((i, avg, _)) = texture(&mut jar, tex) {
                let water = short != "lava" && !short.contains("mushroom");
                block = Block {
                    kind: Kind::Cube,
                    visible: true,
                    top: i,
                    side: i,
                    bottom: i,
                    color: avg,
                    tint: if water { Tint::Water } else { Tint::None },
                    tinted: if water { TINT_TOP | TINT_SIDE | TINT_BOTTOM } else { 0 },
                    transparent: water,
                    ..block
                };
            }
        } else if let Some(model) = block_model(&mut jar, &short).and_then(|m| load_model(&mut jar, &m)) {
            let f = faces(&model, &short);
            if let Some((top_tex, top_tinted)) = f.top {
                let (top, color, alpha) = texture(&mut jar, &top_tex).unwrap_or((0, [128, 128, 128], false));
                let side = f.side.as_ref().and_then(|(t, _)| texture(&mut jar, t)).map_or(top, |t| t.0);
                let bottom = f.bottom.as_ref().and_then(|(t, _)| texture(&mut jar, t)).map_or(top, |t| t.0);
                let overlay = f.overlay.as_deref().and_then(|t| texture(&mut jar, t)).map_or(0, |t| t.0);
                let flag = |f: &Option<(String, bool)>, bit: u8| if f.as_ref().is_some_and(|(_, t)| *t) { bit } else { 0 };
                let tinted = flag(&Some((String::new(), top_tinted)), TINT_TOP) | flag(&f.side, TINT_SIDE) | flag(&f.bottom, TINT_BOTTOM);
                block = Block {
                    kind: f.kind,
                    visible: f.kind == Kind::Cube,
                    top,
                    side,
                    bottom,
                    overlay,
                    color,
                    tint: tint_for(&name, tinted != 0 || overlay != 0),
                    tinted,
                    transparent: alpha || short.contains("ice"),
                    bbox: f.bbox,
                    ..block
                };
            }
        }
        // rails: one entry per shape (direction, curves, slopes)
        if block.kind == Kind::Flat {
            for (key, model, rot) in shape_states(&mut jar, &short) {
                let Some(model) = load_model(&mut jar, &model) else { continue };
                let f = faces(&model, &short);
                let Some((tex, _)) = f.top.filter(|_| f.kind == Kind::Flat) else { continue };
                let Some((top, color, _)) = texture(&mut jar, &tex) else { continue };
                let state = format!("{name}[{key}]");
                let b = Block { name: state.clone(), top, side: top, bottom: top, color, rot, slope: slope(&model), ..block.clone() };
                built.insert(state, b);
            }
        }
        // detailed blocks: one entry per state, drawn from the model's boxes
        let entity = entity_models(&short);
        if detailed(&short) || entity.is_some() {
            let states: Vec<(String, Vec<(Model, Applied)>)> = match entity {
                Some(states) => states,
                None => jar
                    .json(&path)
                    .map(|v| block_states(&v))
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(key, applied)| {
                        let models: Option<Vec<(Model, Applied)>> =
                            applied.into_iter().map(|a| Some((load_model(&mut jar, &a.model)?, a))).collect();
                        Some((key, models?))
                    })
                    .collect(),
            };
            let mut per_state: Vec<(String, Block)> = vec![];
            for (key, with_models) in &states {
                let models: Vec<&Model> = with_models.iter().map(|(m, _)| m).collect();
                let f = part_textures(&models);
                let Some((top_tex, top_tinted)) = f.top.clone() else { continue };
                let side_tex = f.side.clone().map(|s| s.0).unwrap_or_else(|| top_tex.clone());
                let bottom_tex = f.bottom.clone().map(|s| s.0).unwrap_or_else(|| top_tex.clone());
                // further textures (bed sides, chest fronts …) get slots 3, 4, … (`extra`)
                let mut extra_tex: Vec<String> = vec![];
                let mut slot = |t: &str| -> u8 {
                    if t == top_tex {
                        0
                    } else if t == side_tex {
                        1
                    } else if t == bottom_tex {
                        2
                    } else if let Some(i) = extra_tex.iter().position(|x| x == t) {
                        3 + i as u8
                    } else if extra_tex.len() < 8 {
                        extra_tex.push(t.to_string());
                        2 + extra_tex.len() as u8
                    } else {
                        1
                    }
                };
                let mut all: Vec<Vec<u8>> = vec![];
                for (m, a) in with_models {
                    all.extend(model_parts(m, a, &mut slot));
                }
                // the same box from several model parts (pane posts): one box with all their faces
                let mut parts: Vec<Vec<u8>> = vec![];
                for p in all {
                    match parts.iter_mut().find(|q| q[..6] == p[..6]) {
                        Some(q) => {
                            for o in (6..PART_LEN).step_by(5) {
                                if q[o] == NO_FACE {
                                    q[o..o + 5].copy_from_slice(&p[o..o + 5]);
                                }
                            }
                        }
                        None => parts.push(p),
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                let mut bbox = [16, 16, 16, 0, 0, 0];
                for p in &parts {
                    for i in 0..3 {
                        bbox[i] = bbox[i].min(p[i]);
                        bbox[i + 3] = bbox[i + 3].max(p[i + 3]);
                    }
                }
                let shape = *shape_ids.entry(parts.clone()).or_insert_with(|| {
                    shapes.push(parts);
                    (shapes.len() - 1) as u32
                });
                let (top, color, a1) = texture(&mut jar, &top_tex).unwrap_or((0, [128, 128, 128], false));
                let (side, _, a2) = texture(&mut jar, &side_tex).unwrap_or((top, color, false));
                let (bottom, _, a3) = texture(&mut jar, &bottom_tex).unwrap_or((top, color, false));
                let extra: Vec<u32> = extra_tex.iter().map(|t| texture(&mut jar, t).map_or(side, |t| t.0)).collect();
                let flag = |f: &Option<(String, bool)>, bit: u8| if f.as_ref().is_some_and(|(_, t)| *t) { bit } else { 0 };
                let tinted = (if top_tinted { TINT_TOP } else { 0 }) | flag(&f.side, TINT_SIDE) | flag(&f.bottom, TINT_BOTTOM);
                // stairs are solid ground for the block stacks and seen from above in 2D
                let stairs = short.ends_with("_stairs");
                let b = Block {
                    name: if key.is_empty() { name.clone() } else { format!("{name}[{key}]") },
                    kind: if stairs { Kind::Cube } else { Kind::Parts },
                    visible: stairs,
                    top,
                    side,
                    bottom,
                    overlay: 0,
                    color,
                    tint: tint_for(&name, tinted != 0),
                    tinted,
                    transparent: a1 || a2 || a3,
                    bbox,
                    rot: 0,
                    slope: 0,
                    shape,
                    extra,
                };
                per_state.push((key.clone(), b));
            }
            // the plain entry (26.x stores blocks in their default state without properties)
            let mut values: BTreeMap<&str, Vec<String>> = BTreeMap::new();
            for (key, _) in &per_state {
                for (k, v) in key.split(',').filter_map(|kv| kv.split_once('=')) {
                    values.entry(k).or_default().push(v.to_string());
                }
            }
            let default_key = per_state.first().map(|(key, _)| {
                key.split(',')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, _)| format!("{k}={}", default_value(k, &values[k])))
                    .collect::<Vec<_>>()
                    .join(",")
            });
            if let Some((_, b)) = per_state.iter().find(|(k, _)| Some(k) == default_key.as_ref()).or(per_state.first()) {
                block = Block { name: name.clone(), ..b.clone() };
            }
            for (key, b) in per_state {
                if !key.is_empty() {
                    built.insert(b.name.clone(), b);
                }
            }
        }
        built.insert(name, block);
    }
    let unknown = Block {
        name: String::new(),
        kind: Kind::Cube,
        visible: true,
        top: 0,
        side: 0,
        bottom: 0,
        overlay: 0,
        color: [128, 128, 128],
        tint: Tint::None,
        tinted: 0,
        transparent: false,
        bbox: FULL,
        rot: 0,
        slope: 0,
        shape: 0,
        extra: vec![],
    };
    let mut blocks = vec![unknown];
    for b in old.blocks.iter().skip(1) {
        // known id: new data if the block still exists, else keep the old entry
        blocks.push(built.remove(&b.name).unwrap_or_else(|| Block { shape: 0, extra: vec![], ..b.clone() }));
    }
    let mut rest: Vec<Block> = built.into_values().collect();
    rest.sort_by(|a, b| a.name.cmp(&b.name));
    blocks.extend(rest);
    if blocks.len() > u16::MAX as usize {
        return Err("too many blocks".into());
    }

    // ----- biomes -----
    let grass_map = read_colormap(&mut jar, "grass");
    let foliage_map = read_colormap(&mut jar, "foliage");
    let mut found: HashMap<String, Biome> = HashMap::new();
    for path in jar.names("data/minecraft/worldgen/biome/", ".json") {
        let short = path.trim_start_matches("data/minecraft/worldgen/biome/").trim_end_matches(".json").to_string();
        if let Some(v) = jar.json(&path) {
            let name = format!("minecraft:{short}");
            found.insert(name.clone(), biome_from_json(name, &v, &grass_map, &foliage_map));
        }
    }
    let mut biomes = vec![Biome { name: String::new(), ..PLAINS }];
    for b in old.biomes.iter().skip(1) {
        biomes.push(found.remove(&b.name).unwrap_or_else(|| b.clone()));
    }
    let mut rest: Vec<Biome> = found.into_values().collect();
    rest.sort_by(|a, b| a.name.cmp(&b.name));
    biomes.extend(rest);
    biomes.truncate(256);

    write_atlas(&out.join("atlas.png"), &atlas).map_err(|e| format!("atlas: {e}"))?;
    Ok(Registry { client: stamp, blocks, biomes, shapes })
}

fn write_atlas(path: &Path, textures: &[Vec<u8>]) -> std::io::Result<()> {
    let rows = textures.len().div_ceil(ATLAS_COLS);
    let (w, h) = (ATLAS_COLS * TEX, rows * TEX);
    let mut img = vec![0u8; w * h * 4];
    for (i, t) in textures.iter().enumerate() {
        let (ox, oy) = ((i % ATLAS_COLS) * TEX, (i / ATLAS_COLS) * TEX);
        for y in 0..TEX {
            let dst = ((oy + y) * w + ox) * 4;
            img[dst..dst + TEX * 4].copy_from_slice(&t[y * TEX * 4..(y + 1) * TEX * 4]);
        }
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("png.tmp");
    {
        let mut enc = png::Encoder::new(BufWriter::new(File::create(&tmp)?), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(std::io::Error::other)?;
        writer.write_image_data(&img).map_err(std::io::Error::other)?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(elements: Value, textures: Value) -> Model {
        let textures = textures.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        Model { textures, elements: elements.as_array().unwrap().clone() }
    }

    fn rot(y: i64, uvlock: bool) -> Applied {
        Applied { model: String::new(), x: 0, y, uvlock }
    }

    #[test]
    fn multipart_states_enumerate_tested_properties() {
        let fence = json!({"multipart": [
            {"apply": {"model": "block/oak_fence_post"}},
            {"apply": {"model": "block/oak_fence_side", "uvlock": true}, "when": {"north": "true"}},
            {"apply": {"model": "block/oak_fence_side", "uvlock": true, "y": 90}, "when": {"east": "true"}}
        ]});
        let states = block_states(&fence);
        assert_eq!(states.len(), 4);
        let (key, models) = states.iter().find(|(k, _)| k == "east=true,north=false").unwrap();
        assert_eq!(key, "east=true,north=false");
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].y, 90);
        assert!(models[1].uvlock);
        // walls: "none" is never tested, but is a state; OR conditions
        let wall = json!({"multipart": [
            {"apply": {"model": "post"}, "when": {"OR": [{"up": "true"}, {"north": "tall"}]}},
            {"apply": {"model": "side"}, "when": {"north": "low"}}
        ]});
        let states = block_states(&wall);
        assert_eq!(states.len(), 6); // north: tall/low/none × up: true/false
        let models = |k: &str| states.iter().find(|(key, _)| key == k).unwrap().1.len();
        assert_eq!(models("north=none,up=false"), 0);
        assert_eq!(models("north=tall,up=false"), 1);
        assert_eq!(models("north=low,up=true"), 2);
    }

    #[test]
    fn default_values_of_missing_properties() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(default_value("facing", &v(&["east", "north", "south", "west"])), "north");
        assert_eq!(default_value("shape", &v(&["inner_left", "straight"])), "straight");
        assert_eq!(default_value("open", &v(&["true", "false"])), "false");
        assert_eq!(default_value("up", &v(&["false", "true"])), "true");
        assert_eq!(default_value("power", &v(&["3", "0"])), "3");
    }

    #[test]
    fn detailed_block_names() {
        for n in ["oak_stairs", "oak_fence", "oak_fence_gate", "cobblestone_wall", "glass_pane", "iron_bars", "oak_door", "oak_trapdoor", "torch", "wall_torch", "lantern", "iron_chain", "lightning_rod", "red_bed", "oak_sign", "oak_wall_sign", "oak_hanging_sign"] {
            assert!(detailed(n), "{n}");
        }
        for n in ["jack_o_lantern", "sea_lantern", "stone", "oak_slab"] {
            assert!(!detailed(n), "{n}");
        }
    }

    #[test]
    fn stairs_parts_rotate_with_the_state() {
        let stairs = model(json!([
            {"from": [0, 0, 0], "to": [16, 8, 16], "faces": {"up": {"texture": "#top"}, "north": {"texture": "#side"}}},
            {"from": [8, 8, 0], "to": [16, 16, 16], "faces": {"up": {"texture": "#top"}, "west": {"texture": "#side"}}}
        ]), json!({"top": "block/oak_planks", "side": "block/oak_planks"}));
        // facing east: no rotation, step on the east half
        let p = model_parts(&stairs, &rot(0, true), |_| 0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1][..6], [8, 8, 0, 16, 16, 16]);
        // facing north = y 270: step on the north half; its west face now faces south
        let p = model_parts(&stairs, &rot(270, true), |_| 0);
        assert_eq!(p[1][..6], [0, 8, 0, 16, 16, 8]);
        let face = |part: &Vec<u8>, dir: usize| part[6 + dir * 5..6 + dir * 5 + 5].to_vec();
        assert_eq!(face(&p[1], WEST), vec![NO_FACE; 5]);
        // uvlock: texture follows the world position (south face: u = x, v = 16 - y)
        assert_eq!(face(&p[1], SOUTH), vec![0, 0, 16, 8, 0]);
        // upside down (x 180): the slab goes to the top
        let top = Applied { x: 180, ..rot(0, true) };
        assert_eq!(model_parts(&stairs, &top, |_| 0)[0][..6], [0, 8, 0, 16, 16, 16]);
    }

    #[test]
    fn wall_torch_is_moved_and_keeps_its_uv() {
        let torch = model(json!([{"from": [-1, 3.5, 7], "to": [1, 13.5, 9],
            "rotation": {"origin": [0, 3.5, 8], "axis": "z", "angle": -22.5},
            "faces": {"north": {"uv": [7, 6, 9, 16], "texture": "#torch"}, "up": {"uv": [7, 6, 9, 8], "texture": "#torch"}}}]),
            json!({"torch": "block/torch"}));
        let p = model_parts(&torch, &rot(0, false), |_| 0);
        // leaning away from the wall (west): three pieces, each a bit further east, inside the block
        let boxes: Vec<&[u8]> = p.iter().map(|p| &p[..6]).collect();
        assert_eq!(boxes, [[0, 3, 7, 2, 7, 9], [1, 6, 7, 3, 10, 9], [2, 10, 7, 4, 13, 9]]);
        // each piece shows its share of the side texture, only the top piece has the top
        let north: Vec<&[u8]> = p.iter().map(|p| &p[6 + NORTH * 5..6 + NORTH * 5 + 4]).collect();
        assert_eq!(north, [[7, 13, 9, 16], [7, 9, 9, 13], [7, 6, 9, 9]]);
        assert_eq!(p.iter().filter(|p| p[6 + UP * 5] != NO_FACE).count(), 1);
        assert_eq!(p[2][6 + UP * 5..6 + UP * 5 + 4], [7, 6, 9, 8]);
        // facing south (y 90): against the north wall, the north face turned east
        let p = model_parts(&torch, &rot(90, false), |_| 0);
        assert_eq!(p[0][..6], [7, 3, 0, 9, 7, 2]);
        assert_eq!(p[2][..6], [7, 10, 2, 9, 13, 4]);
        assert_eq!(p[0][6 + EAST * 5..6 + EAST * 5 + 4], [7, 13, 9, 16]);
    }

    #[test]
    fn chests_and_heads_are_built_per_state() {
        let chest = entity_models("chest").unwrap();
        assert_eq!(chest.len(), 12);
        let (key, models) = &chest[1];
        assert_eq!(key, "facing=north,type=left");
        // a left half facing north reaches the east edge
        let p = model_parts(&models[0].0, &models[0].1, |_| 0);
        assert_eq!(p[0][..6], [1, 0, 1, 16, 14, 15]);
        assert_eq!(entity_models("waxed_oxidized_copper_chest").unwrap()[0].1[0].0.textures["top"], "crop|entity/chest/copper_oxidized|64|28,0,14,14");
        // a skull with rotation 8 looks north: the front texture on the north face
        let skull = entity_models("skeleton_skull").unwrap();
        assert_eq!(skull.len(), 16);
        let (m, a) = &skull[8].1[0];
        let mut slots = vec![];
        let p = model_parts(m, a, |t| { slots.push(t.to_string()); slots.len() as u8 - 1 });
        assert_eq!(p[0][..6], [4, 0, 4, 12, 8, 12]);
        assert!(slots[p[0][6 + NORTH * 5 + 4] as usize].ends_with("8,8,8,8"));
        // on a wall facing east: against the west edge
        let wall = entity_models("zombie_wall_head").unwrap();
        let (m, a) = &wall[1].1[0];
        assert_eq!(model_parts(m, a, |_| 0)[0][..6], [0, 4, 4, 8, 12, 12]);
        assert!(entity_models("dragon_head").is_none());
        assert!(entity_models("stone").is_none());
    }

    #[test]
    fn crop_specs() {
        assert_eq!(crop_rects("1,2,3,4;5,6,7,8,v"), vec![([1, 2, 3, 4], false), ([5, 6, 7, 8], true)]);
    }

    #[test]
    fn cube_faces_and_tint() {
        let m = model(
            json!([{"from": [0, 0, 0], "to": [16, 16, 16], "faces": {
                "up": {"texture": "#top", "tintindex": 0}, "north": {"texture": "#side"}}}]),
            json!({"top": "block/grass_block_top", "side": "minecraft:block/grass_block_side"}),
        );
        let f = faces(&m, "grass_block");
        assert_eq!(f.kind, Kind::Cube);
        assert_eq!(f.top, Some(("block/grass_block_top".into(), true)));
        assert_eq!(f.side, Some(("block/grass_block_side".into(), false)));
        assert_eq!(f.overlay, None);
    }

    #[test]
    fn block_shapes() {
        let cross = model(json!([{"from": [0.8, 0, 8], "to": [15.2, 16, 8], "faces": {"north": {"texture": "#cross", "tintindex": 0}}}]), json!({"cross": "block/fern"}));
        let f = faces(&cross, "fern");
        assert_eq!(f.kind, Kind::Cross);
        assert_eq!(f.top, Some(("block/fern".into(), true)));
        let vine = model(json!([{"from": [0, 0, 0.8], "to": [16, 16, 0.8], "faces": {"south": {"texture": "#vine", "tintindex": 0}}}]), json!({"vine": "block/vine"}));
        assert_eq!(faces(&vine, "vine").kind, Kind::Vine);
        let rail = model(json!([{"from": [0, 1, 0], "to": [16, 1, 16], "faces": {"up": {"texture": "#rail"}}}]), json!({"rail": "block/rail"}));
        assert_eq!(faces(&rail, "rail").kind, Kind::Flat);
        let torch = model(json!([{"from": [7, 0, 7], "to": [9, 10, 9], "faces": {"up": {"texture": "#torch"}}}]), json!({"torch": "block/torch"}));
        assert_eq!(faces(&torch, "torch").kind, Kind::None);
        let slab = model(json!([{"from": [0, 0, 0], "to": [16, 8, 16], "faces": {"up": {"texture": "#top"}}}]), json!({"top": "block/stone"}));
        assert_eq!(faces(&slab, "stone_slab").kind, Kind::Cube);
        assert_eq!(faces(&slab, "stone_slab").bbox, [0, 0, 0, 16, 8, 16]);
        // like the real cactus model: top/bottom from a full box, sides from inset boxes
        let cactus = model(json!([
            {"from": [0, 0, 0], "to": [16, 16, 16], "faces": {"up": {"texture": "#top"}, "down": {"texture": "#bottom"}}},
            {"from": [0, 0, 1], "to": [16, 16, 15], "faces": {"north": {"texture": "#side"}, "south": {"texture": "#side"}}},
            {"from": [1, 0, 0], "to": [15, 16, 16], "faces": {"west": {"texture": "#side"}, "east": {"texture": "#side"}}}
        ]), json!({"top": "block/cactus_top", "bottom": "block/cactus_bottom", "side": "block/cactus_side"}));
        let f = faces(&cactus, "cactus");
        assert_eq!(f.bbox, [1, 0, 1, 15, 16, 15]);
        assert_eq!(f.side, Some(("block/cactus_side".into(), false)));
        assert_eq!(f.top, Some(("block/cactus_top".into(), false)));
        let mut grass = model(json!([{"from": [0, 0, 0], "to": [16, 16, 16], "faces": {"up": {"texture": "#top"}}}]), json!({"top": "block/grass_block_top", "overlay": "block/grass_block_side_overlay"}));
        grass.textures.insert("side".into(), "block/grass_block_side".into());
        assert_eq!(faces(&grass, "grass_block").overlay, Some("block/grass_block_side_overlay".into()));
    }

    #[test]
    fn raised_rail_slopes() {
        let plane = |angle: f64| model(json!([{"from": [0, 9, 0], "to": [16, 9, 16],
            "rotation": {"origin": [8, 9, 8], "axis": "x", "angle": angle, "rescale": true},
            "faces": {"up": {"texture": "#rail"}}}]), json!({"rail": "block/rail"}));
        assert_eq!(slope(&plane(45.0)), 1);
        assert_eq!(slope(&plane(-45.0)), -1);
        let flat = model(json!([{"from": [0, 1, 0], "to": [16, 1, 16], "faces": {"up": {"texture": "#rail"}}}]), json!({"rail": "block/rail"}));
        assert_eq!(slope(&flat), 0);
        assert_eq!(faces(&plane(45.0), "rail").kind, Kind::Flat);
    }

    #[test]
    fn sprite_objects_in_models() {
        // 26.x: texture variables can be {"sprite": ..., "force_translucent": true}
        let dir = std::env::temp_dir().join(format!("mapgen-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jar_path = dir.join("client.jar");
        {
            let mut w = zip::ZipWriter::new(File::create(&jar_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            use std::io::Write;
            w.start_file("assets/minecraft/models/block/glass.json", opts).unwrap();
            w.write_all(br#"{"parent": "minecraft:block/cube_all", "textures": {"all": {"force_translucent": true, "sprite": "minecraft:block/glass"}}}"#).unwrap();
            w.start_file("assets/minecraft/models/block/cube_all.json", opts).unwrap();
            w.write_all(br##"{"elements": [{"from": [0,0,0], "to": [16,16,16], "faces": {"up": {"texture": "#all"}, "north": {"texture": "#all"}}}]}"##).unwrap();
            w.finish().unwrap();
        }
        let zip = zip::ZipArchive::new(File::open(&jar_path).unwrap()).unwrap();
        let mut jar = Jar { zip, json_cache: HashMap::new() };
        let m = load_model(&mut jar, "minecraft:block/glass").unwrap();
        let f = faces(&m, "glass");
        assert_eq!(f.kind, Kind::Cube);
        assert_eq!(f.top, Some(("block/glass".into(), false)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn texture_variables_resolve() {
        let m = model(json!([]), json!({"all": "minecraft:block/stone", "side": "#all"}));
        assert_eq!(resolve_texture(&m, "#side"), Some("block/stone".into()));
        assert_eq!(resolve_texture(&m, "#missing"), None);
        assert_eq!(resolve_texture(&m, "othermod:block/x"), None);
    }

    #[test]
    fn tints() {
        assert_eq!(tint_for("minecraft:oak_leaves", true), Tint::Foliage);
        assert_eq!(tint_for("minecraft:grass_block", true), Tint::Grass);
        assert_eq!(tint_for("minecraft:stone", false), Tint::None);
        assert_eq!(tint_for("minecraft:water", false), Tint::Water);
        assert!(matches!(tint_for("minecraft:birch_leaves", true), Tint::Fixed(_)));
    }

    #[test]
    fn biome_colors() {
        let swamp = biome_from_json("s".into(), &json!({"temperature": 0.8, "downfall": 0.9,
            "effects": {"grass_color_modifier": "swamp", "foliage_color": "#6a7039", "water_color": "#617b64"}}), &None, &None);
        assert_eq!(swamp.grass, [106, 112, 57]);
        assert_eq!(swamp.foliage, [0x6a, 0x70, 0x39]);
        assert_eq!(swamp.water, [0x61, 0x7b, 0x64]);
        // colormap corner: temperature 1, downfall 1 -> pixel (0, 0)
        let map = Some((vec![10, 20, 30, 255].repeat(256 * 256), 256));
        let b = biome_from_json("j".into(), &json!({"temperature": 1.0, "downfall": 1.0}), &map, &map);
        assert_eq!(b.grass, [10, 20, 30]);
    }

    #[test]
    fn average_ignores_transparent_pixels() {
        let t = [[200, 100, 0, 255], [0, 0, 0, 0]].concat();
        assert_eq!(average(&t), ([200, 100, 0], true));
        assert_eq!(average(&[1, 2, 3, 255]), ([1, 2, 3], false));
    }
}
