//! Chunk NBT (1.18+ format, including the 26.x palette encoding) reduced to what a map needs:
//! the surface heightmap and block lookups by position.

use std::collections::HashMap;

use fastnbt::LongArray;
use serde::Deserialize;

use crate::assets::{Assets, Kind};
use crate::colors;

#[derive(Deserialize)]
struct ChunkNbt {
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "Heightmaps")]
    heightmaps: Option<Heightmaps>,
    #[serde(default)]
    sections: Vec<SectionNbt>,
    /// Lowest section of the world (e.g. -4 = y -64). The section list itself also holds
    /// light-only sections one below/above, so it can't be used for this.
    #[serde(rename = "yPos")]
    y_pos: Option<i32>,
    /// only skulls (player head skins), banners (patterns), decorated pots (sherds) and conduits
    /// are looked at; every field optional, so an odd block entity never makes the chunk unreadable
    #[serde(default)]
    block_entities: Vec<BlockEntityNbt>,
}

#[derive(Deserialize)]
struct BlockEntityNbt {
    id: Option<fastnbt::Value>,
    x: Option<fastnbt::Value>,
    y: Option<fastnbt::Value>,
    z: Option<fastnbt::Value>,
    /// 1.20.5+: the head's profile (`properties` with the base64 `textures`)
    profile: Option<fastnbt::Value>,
    /// before 1.20.5
    #[serde(rename = "SkullOwner")]
    skull_owner: Option<fastnbt::Value>,
    /// banners, 1.20.5+: `[{pattern: "minecraft:creeper" | {asset_id: …}, color: "lime"}]`
    patterns: Option<fastnbt::Value>,
    /// banners before 1.20.5: `[{Pattern: "cre", Color: 5}]`
    #[serde(rename = "Patterns")]
    old_patterns: Option<fastnbt::Value>,
    /// decorated pots: the sides' items ("minecraft:brick" = plain), 26.x
    /// `{back: {id: …}, left, right, front}` (missing = brick), before a list of ids in that order
    sherds: Option<fastnbt::Value>,
}

/// What a block entity adds to its block model in the 3D view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decor {
    /// banner pattern layers, bottom first: (pattern id or asset id, dye colour name)
    Banner(Vec<(String, String)>),
    /// pottery sherds (item ids) of back, left, right, front
    Pot([String; 4]),
    /// a conduit: active (cage + eye) when its frame is complete, see `render`
    Conduit,
}

/// A block entity with something the 3D view shows, at its absolute block position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entity {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub decor: Decor,
}

/// Dye colour names by their old numeric ids (`Color` of pre-1.20.5 banner patterns).
const DYE_NAMES: [&str; 16] = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray",
    "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black",
];

/// Pre-1.20.5 banner pattern codes -> pattern ids.
const OLD_PATTERNS: [(&str, &str); 41] = [
    ("b", "base"), ("bl", "square_bottom_left"), ("br", "square_bottom_right"), ("tl", "square_top_left"),
    ("tr", "square_top_right"), ("bs", "stripe_bottom"), ("ts", "stripe_top"), ("ls", "stripe_left"),
    ("rs", "stripe_right"), ("cs", "stripe_center"), ("ms", "stripe_middle"), ("drs", "stripe_downright"),
    ("dls", "stripe_downleft"), ("ss", "small_stripes"), ("cr", "cross"), ("sc", "straight_cross"),
    ("bt", "triangle_bottom"), ("tt", "triangle_top"), ("bts", "triangles_bottom"), ("tts", "triangles_top"),
    ("ld", "diagonal_left"), ("rd", "diagonal_up_right"), ("lud", "diagonal_up_left"), ("rud", "diagonal_right"),
    ("mc", "circle"), ("mr", "rhombus"), ("vh", "half_vertical"), ("hh", "half_horizontal"),
    ("vhr", "half_vertical_right"), ("hhb", "half_horizontal_bottom"), ("bo", "border"), ("cbo", "curly_border"),
    ("gra", "gradient"), ("gru", "gradient_up"), ("bri", "bricks"), ("glb", "globe"), ("cre", "creeper"),
    ("sku", "skull"), ("flo", "flower"), ("moj", "mojang"), ("pig", "piglin"),
];

fn str_of(v: Option<&fastnbt::Value>) -> Option<&str> {
    match v? {
        fastnbt::Value::String(s) => Some(s),
        _ => None,
    }
}

fn list_of(v: Option<&fastnbt::Value>) -> &[fastnbt::Value] {
    match v {
        Some(fastnbt::Value::List(l)) => l,
        _ => &[],
    }
}

/// Banner pattern layers (pattern or asset id, dye name) from either NBT format; unknown entries
/// are skipped.
pub fn banner_patterns(new: Option<&fastnbt::Value>, old: Option<&fastnbt::Value>) -> Vec<(String, String)> {
    let mut out = vec![];
    for p in list_of(new) {
        let fastnbt::Value::Compound(m) = p else { continue };
        let id = match m.get("pattern") {
            Some(fastnbt::Value::String(s)) => s.as_str(),
            Some(fastnbt::Value::Compound(inline)) => match str_of(inline.get("asset_id")) {
                Some(a) => a,
                None => continue,
            },
            _ => continue,
        };
        let Some(color) = str_of(m.get("color")) else { continue };
        out.push((id.to_string(), color.to_string()));
    }
    for p in list_of(old) {
        let fastnbt::Value::Compound(m) = p else { continue };
        let Some(code) = str_of(m.get("Pattern")) else { continue };
        let Some((_, id)) = OLD_PATTERNS.iter().find(|(c, _)| *c == code) else { continue };
        let color = match m.get("Color") {
            Some(fastnbt::Value::Int(i)) => *i as usize,
            Some(fastnbt::Value::Byte(i)) => *i as usize,
            _ => 0,
        };
        out.push((format!("minecraft:{id}"), DYE_NAMES.get(color).unwrap_or(&"white").to_string()));
    }
    out
}

/// The four sherd item ids of a decorated pot (back, left, right, front), None when all sides are
/// plain.
pub fn pot_sherds(v: Option<&fastnbt::Value>) -> Option<[String; 4]> {
    const SIDES: [&str; 4] = ["back", "left", "right", "front"];
    // an item id, or (26.x) an item stack {id: …}
    fn item(v: Option<&fastnbt::Value>) -> Option<&str> {
        match v {
            Some(fastnbt::Value::Compound(m)) => str_of(m.get("id")),
            v => str_of(v),
        }
    }
    let side = |i: usize| {
        let s = match v {
            Some(fastnbt::Value::Compound(m)) => item(m.get(SIDES[i])),
            _ => item(list_of(v).get(i)),
        };
        s.unwrap_or("minecraft:brick").to_string()
    };
    let sides = [side(0), side(1), side(2), side(3)];
    sides.iter().any(|s| s != "minecraft:brick").then_some(sides)
}

/// A player head with its own skin: absolute block position and the skin's texture hash
/// (textures.minecraft.net/texture/<hash>).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub skin: String,
}

fn int(v: &Option<fastnbt::Value>) -> Option<i32> {
    match v.as_ref()? {
        fastnbt::Value::Int(i) => Some(*i),
        fastnbt::Value::Short(i) => Some(*i as i32),
        fastnbt::Value::Long(i) => i32::try_from(*i).ok(),
        _ => None,
    }
}

/// Standard base64 (padding optional); None on anything else.
fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|&c| c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// The skin hash in a head's profile, wherever the format keeps the base64 `textures` value
/// (its JSON names http://textures.minecraft.net/texture/<hash>).
pub fn skin_hash(v: &fastnbt::Value) -> Option<String> {
    match v {
        fastnbt::Value::String(s) if s.len() > 40 => {
            let text = String::from_utf8(base64(s)?).ok()?;
            const HOST: &str = "textures.minecraft.net/texture/";
            let start = text.find(HOST)? + HOST.len();
            let hash: String = text[start..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            (16..=128).contains(&hash.len()).then(|| hash.to_ascii_lowercase())
        }
        fastnbt::Value::Compound(m) => m.values().find_map(skin_hash),
        fastnbt::Value::List(l) => l.iter().find_map(skin_hash),
        _ => None,
    }
}

#[derive(Deserialize)]
struct Heightmaps {
    #[serde(rename = "WORLD_SURFACE")]
    world_surface: Option<LongArray>,
}

#[derive(Deserialize)]
struct SectionNbt {
    #[serde(rename = "Y")]
    y: i8,
    block_states: Option<BlockStates>,
    biomes: Option<Biomes>,
}

#[derive(Deserialize)]
struct Biomes {
    palette: Vec<PaletteEntry>,
    data: Option<LongArray>,
}

#[derive(Deserialize)]
struct BlockStates {
    palette: Vec<PaletteEntry>,
    data: Option<LongArray>,
}

/// Palette entries come in four shapes:
/// - `{Name: "minecraft:stone", Properties: {...}}` (classic)
/// - `"minecraft:stone"` (26.x, block in its default state)
/// - `{"": "minecraft:stone"}` (26.x, a string inside a list that also holds compounds)
/// - `{id: "minecraft:oak_leaves", properties: {...}}` (26.x, block with non-default properties)
#[derive(Deserialize)]
#[serde(untagged)]
enum PaletteEntry {
    Plain(String),
    Compound {
        #[serde(rename = "Name")]
        name: Option<String>,
        #[serde(rename = "")]
        wrapped: Option<String>,
        id: Option<String>,
        #[serde(rename = "Properties")]
        classic_props: Option<HashMap<String, fastnbt::Value>>,
        properties: Option<HashMap<String, fastnbt::Value>>,
    },
}

impl PaletteEntry {
    fn name(&self) -> &str {
        match self {
            PaletteEntry::Plain(s) => s,
            PaletteEntry::Compound { name: Some(n), .. } => n,
            PaletteEntry::Compound { wrapped: Some(n), .. } => n,
            PaletteEntry::Compound { id: Some(n), .. } => n,
            _ => "minecraft:air",
        }
    }

    /// A block state property as text (None in the default state of the plain 26.x forms).
    fn prop(&self, key: &str) -> Option<&str> {
        let PaletteEntry::Compound { classic_props, properties, .. } = self else { return None };
        match classic_props.as_ref().or(properties.as_ref())?.get(key)? {
            fastnbt::Value::String(s) => Some(s),
            _ => None,
        }
    }
}

/// One 16×16×16 section with its palette resolved to map colors / block ids.
struct Section {
    /// map color per palette entry (`colors::NONE` = transparent)
    colors: Vec<u8>,
    water: Vec<bool>,
    /// seen from above (not air, plants, torches …)
    visible: Vec<bool>,
    /// 0 = not drawn, 1 = box, 2 = plant / vine / flat (3D only)
    kinds: Vec<u8>,
    leafy: Vec<bool>,
    /// block id from the texture tables (0 = unknown / no tables)
    ids: Vec<u16>,
    data: Option<Vec<i64>>,
    bits: u32,
    /// biome ids per palette entry, 4×4×4 cells
    biomes: Vec<u8>,
    biome_data: Option<Vec<i64>>,
    biome_bits: u32,
}

/// Entry `i` of a packed array (entries don't cross long boundaries, 1.16+ format).
fn unpack(data: &Option<Vec<i64>>, bits: u32, i: usize) -> usize {
    match data {
        Some(longs) if bits > 0 => {
            let per_long = (64 / bits) as usize;
            let Some(&long) = longs.get(i / per_long) else { return 0 };
            let shift = (i % per_long) as u32 * bits;
            ((long as u64 >> shift) & ((1u64 << bits) - 1)) as usize
        }
        _ => 0,
    }
}

impl Section {
    fn index(&self, i: usize) -> usize {
        unpack(&self.data, self.bits, i)
    }
}

/// One block as the map sees it.
#[derive(Clone, Copy)]
struct Cell {
    color: u8,
    water: bool,
    visible: bool,
    kind: u8,
    leafy: bool,
    id: u16,
}

const AIR: Cell = Cell { color: colors::NONE, water: false, visible: false, kind: 0, leafy: false, id: 0 };

/// What a map sees in one block column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    pub color: u8,
    /// y of the colored block (water surface for water)
    pub y: i32,
    /// water depth below the surface, 0 for land
    pub depth: u16,
    /// block id from the texture tables (0 = unknown)
    pub block: u16,
    /// biome id from the texture tables (0 = unknown)
    pub biome: u8,
    /// highest non-air block (plants included), where the 3D block stack starts
    pub top: i32,
    /// block id of the highest plant / flat block (rails, lily pads) above the coloured block,
    /// drawn over it in the textured 2D view (0 = none)
    pub plant: u16,
}

/// Vanilla's bedrock roof spans the top 5 blocks below the ceiling (Nether: y 123..=127).
const ROOF_DEPTH: i32 = 5;

pub const EMPTY: Column = Column { color: colors::NONE, y: i32::MIN, depth: 0, block: 0, biome: 0, top: i32::MIN, plant: 0 };

pub struct Chunk {
    sections: Vec<Option<Section>>, // indexed by section y - min_section
    min_section: i32,
    surface: Option<Vec<i64>>,
    /// player heads with a skin of their own
    pub heads: Vec<Head>,
    /// banners with patterns, decorated pots with sherds, conduits
    pub decor: Vec<Entity>,
}

impl Chunk {
    /// Returns None for chunks that are not fully generated yet. With texture tables (`assets`),
    /// blocks get ids/biomes and visibility follows the block models instead of the map colors.
    pub fn parse(nbt: &[u8], assets: Option<&Assets>) -> Option<Chunk> {
        let raw: ChunkNbt = fastnbt::from_bytes(nbt).ok()?;
        if !raw.status.as_deref().is_some_and(|s| s.ends_with("full")) {
            return None;
        }
        let with_blocks = || raw.sections.iter().filter(|s| s.block_states.is_some()).map(|s| s.y as i32);
        let min_section = raw.y_pos.or_else(|| with_blocks().min())?;
        let max_section = with_blocks().max()?.max(min_section);
        let mut sections: Vec<Option<Section>> = (min_section..=max_section).map(|_| None).collect();
        for s in raw.sections {
            let Some(states) = s.block_states else { continue };
            if (s.y as i32) < min_section || (s.y as i32) > max_section {
                continue;
            }
            if states.palette.is_empty() {
                continue;
            }
            let colors: Vec<u8> = states.palette.iter().map(|p| colors::map_color(p.name())).collect();
            let water: Vec<bool> = states.palette.iter().map(|p| {
                colors::is_water(p.name().strip_prefix("minecraft:").unwrap_or(p.name()))
            }).collect();
            let water_id = assets.map_or(0, |a| a.block_id("minecraft:water"));
            let mut ids = Vec::with_capacity(colors.len());
            let mut visible = Vec::with_capacity(colors.len());
            let mut kinds = Vec::with_capacity(colors.len());
            let mut leafy = Vec::with_capacity(colors.len());
            for (i, p) in states.palette.iter().enumerate() {
                // waterlogged plants (seagrass, kelp) count as water
                let id = if water[i] { water_id } else { assets.map_or(0, |a| a.block_state_id(p.name(), |k| p.prop(k))) };
                ids.push(id);
                let kind = match assets {
                    _ if water[i] => Kind::Cube,
                    Some(a) if id != 0 => a.block(id).kind,
                    _ if colors[i] != colors::NONE => Kind::Cube,
                    _ => Kind::None,
                };
                visible.push(kind == Kind::Cube);
                kinds.push(match kind {
                    Kind::None => 0,
                    Kind::Cube => 1,
                    // drawn over the ground in the 2D view
                    Kind::Cross | Kind::Flat => 3,
                    _ => 2,
                });
                leafy.push(p.name().ends_with("_leaves"));
            }
            let bits = (usize::BITS - (states.palette.len() - 1).leading_zeros()).max(4);
            let data = states.data.map(|d| d.into_inner()).filter(|d| !d.is_empty());
            let (biomes, biome_data, biome_bits) = match (s.biomes, assets) {
                (Some(b), Some(a)) if !b.palette.is_empty() => (
                    b.palette.iter().map(|p| a.biome_id(p.name())).collect(),
                    b.data.map(|d| d.into_inner()).filter(|d| !d.is_empty()),
                    usize::BITS - (b.palette.len() - 1).leading_zeros(),
                ),
                _ => (vec![], None, 0),
            };
            sections[(s.y as i32 - min_section) as usize] =
                Some(Section { colors, water, visible, kinds, leafy, ids, data, bits, biomes, biome_data, biome_bits });
        }
        let surface = raw.heightmaps.and_then(|h| h.world_surface).map(|l| l.into_inner());
        let heads = raw
            .block_entities
            .iter()
            .filter(|e| matches!(&e.id, Some(fastnbt::Value::String(id)) if id == "minecraft:skull"))
            .filter_map(|e| {
                let skin = e.profile.as_ref().or(e.skull_owner.as_ref()).and_then(skin_hash)?;
                Some(Head { x: int(&e.x)?, y: int(&e.y)?, z: int(&e.z)?, skin })
            })
            .collect();
        let decor = raw
            .block_entities
            .iter()
            .filter_map(|e| {
                let decor = match str_of(e.id.as_ref())? {
                    "minecraft:banner" => {
                        let layers = banner_patterns(e.patterns.as_ref(), e.old_patterns.as_ref());
                        if layers.is_empty() {
                            return None;
                        }
                        Decor::Banner(layers)
                    }
                    "minecraft:decorated_pot" => Decor::Pot(pot_sherds(e.sherds.as_ref())?),
                    "minecraft:conduit" => Decor::Conduit,
                    _ => return None,
                };
                Some(Entity { x: int(&e.x)?, y: int(&e.y)?, z: int(&e.z)?, decor })
            })
            .collect();
        Some(Chunk { sections, min_section, surface, heads, decor })
    }

    pub fn min_y(&self) -> i32 {
        self.min_section * 16
    }

    fn section(&self, y: i32) -> Option<&Section> {
        let s = y.div_euclid(16) - self.min_section;
        if s < 0 {
            return None;
        }
        self.sections.get(s as usize)?.as_ref()
    }

    /// Block id (texture tables) at local x/z (0..16) and absolute y; 0 = air / unknown.
    pub fn block_id(&self, x: usize, y: i32, z: usize) -> u16 {
        self.block(x, y, z).id
    }

    /// Block at local x/z (0..16) and absolute y. Missing = air.
    fn block(&self, x: usize, y: i32, z: usize) -> Cell {
        let Some(section) = self.section(y) else { return AIR };
        let p = section.index((y.rem_euclid(16) as usize) * 256 + z * 16 + x);
        if p >= section.colors.len() {
            return AIR;
        }
        Cell {
            color: section.colors[p],
            water: section.water[p],
            visible: section.visible[p],
            kind: section.kinds[p],
            leafy: section.leafy[p],
            id: section.ids[p],
        }
    }

    fn solid(&self, x: usize, y: i32, z: usize) -> bool {
        let c = self.block(x, y, z);
        c.kind == 1 && !c.water && !c.leafy
    }

    /// The 3D block stack of a column from `top` down to `bottom` as the map may show it (see
    /// `render::voxels`): non-solid blocks where `seen(y)`, solid blocks next to seen air (above,
    /// below, or beside: `side(y)`), everything else as FILL runs; visible air after a FILL run
    /// starts with AIR_MARK. Of a run of water only the surface is kept. At most 255 entries;
    /// trailing FILL is dropped (below the last entry counts as solid anyway). Seen air down to the
    /// world bottom (the End's void) ends with an AIR_MARK at the bottom.
    pub fn visible_stack(&self, x: usize, z: usize, top: i32, bottom: i32, seen: impl Fn(i32) -> bool, side: impl Fn(i32) -> bool, out: &mut Vec<(i16, u16)>) -> u8 {
        use crate::render::{AIR_MARK, FILL};
        let start = out.len();
        let mut prev_water = false;
        let mut y = top;
        while y >= bottom.max(self.min_y()) && out.len() - start < 255 {
            let c = self.block(x, y, z);
            let solid = c.kind == 1 && !c.water && !c.leafy;
            let after_fill = out.len() > start && out[out.len() - 1].1 == FILL;
            if !solid && seen(y) {
                if c.water {
                    if !prev_water {
                        out.push((y as i16, c.id));
                    }
                    prev_water = true;
                } else {
                    prev_water = false;
                    if c.kind != 0 {
                        out.push((y as i16, c.id));
                    } else if after_fill {
                        out.push((y as i16, AIR_MARK));
                    }
                }
            } else if solid && (y >= top || seen(y + 1) || seen(y - 1) || side(y)) {
                prev_water = false;
                out.push((y as i16, c.id));
            } else {
                prev_water = false;
                if !after_fill {
                    out.push((y as i16, FILL));
                }
            }
            y -= 1;
        }
        while out.len() > start && out[out.len() - 1].1 == FILL {
            out.pop();
        }
        let floor = self.min_y();
        if y < floor && out.len() - start < 255 && !self.solid(x, floor, z) && seen(floor) && self.block(x, floor, z).kind == 0 {
            out.push((floor as i16, AIR_MARK));
        }
        (out.len() - start) as u8
    }

    /// The runs of non-solid blocks (air, plants, leaves, water: where `stack` does not stop) of a
    /// column from `top` down to the world bottom, as (highest y, lowest y), top first.
    pub fn air_runs(&self, x: usize, z: usize, top: i32, out: &mut Vec<(i16, i16)>) {
        let mut run: Option<i32> = None;
        let mut y = top;
        while y >= self.min_y() {
            // a section that does not exist is all air
            if self.section(y).is_none() {
                run.get_or_insert(y);
                y = (y.div_euclid(16)) * 16 - 1;
                continue;
            }
            if self.solid(x, y, z) {
                if let Some(t) = run.take() {
                    out.push((t as i16, (y + 1) as i16));
                }
            } else {
                run.get_or_insert(y);
            }
            y -= 1;
        }
        if let Some(t) = run {
            out.push((t as i16, self.min_y() as i16));
        }
    }

    /// Biome id at local x/z and absolute y (0 = unknown).
    fn biome(&self, x: usize, y: i32, z: usize) -> u8 {
        let Some(section) = self.section(y) else { return 0 };
        let i = ((y.rem_euclid(16) as usize) >> 2) * 16 + (z >> 2) * 4 + (x >> 2);
        section.biomes.get(unpack(&section.biome_data, section.biome_bits, i)).copied().unwrap_or(0)
    }

    /// y of the highest non-air block from the WORLD_SURFACE heightmap.
    pub fn surface_y(&self, x: usize, z: usize) -> Option<i32> {
        let longs = self.surface.as_ref()?;
        // 256 entries packed without crossing long boundaries; width follows from the length
        let per_long = 256usize.div_ceil(longs.len());
        let bits = 64 / per_long as u32;
        let i = z * 16 + x;
        let v = ((longs.get(i / per_long)? >> ((i % per_long) as u32 * bits)) as u64) & ((1u64 << bits) - 1);
        // the heightmap stores "first free block above"; 0 means an empty column
        (v > 0).then(|| self.min_y() + v as i32 - 1)
    }

    /// The column as a map item would draw it.
    /// `ceiling`: start below a solid roof (Nether): skip down to the first air below `ceiling`.
    pub fn column(&self, x: usize, z: usize, ceiling: Option<i32>) -> Column {
        let top = match ceiling {
            Some(c) => {
                let mut y = c.min(self.surface_y(x, z).unwrap_or(c));
                // inside the roof: go down to the first air (or transparent) block
                while y >= self.min_y() && self.block(x, y, z).visible {
                    y -= 1;
                }
                if y < self.min_y() {
                    // solid all the way down (no cave here): show the top block under the roof
                    c - ROOF_DEPTH
                } else {
                    y
                }
            }
            None => match self.surface_y(x, z) {
                Some(y) => y,
                None => return EMPTY,
            },
        };
        let mut y = top;
        let mut plant = 0;
        while y >= self.min_y() {
            let cell = self.block(x, y, z);
            if cell.kind == 3 && plant == 0 {
                plant = cell.id;
            }
            if cell.water {
                let surface = y;
                let mut depth = 0u16;
                while y > self.min_y() && self.block(x, y - 1, z).water {
                    y -= 1;
                    depth += 1;
                }
                let biome = self.biome(x, surface, z);
                return Column { color: colors::WATER, y: surface, depth: depth + 1, block: cell.id, biome, top, plant };
            }
            if cell.visible {
                // a block without map colour (glass …) that has a texture: use stone grey in 2D fallback
                let color = if cell.color == colors::NONE { colors::STONE } else { cell.color };
                return Column { color, y, depth: 0, block: cell.id, biome: self.biome(x, y, z), top, plant };
            }
            y -= 1;
        }
        EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(palette_len: usize, bits: u32, longs: Vec<i64>) -> Section {
        Section {
            colors: vec![1; palette_len],
            water: vec![false; palette_len],
            visible: vec![true; palette_len],
            kinds: vec![1; palette_len],
            leafy: vec![false; palette_len],
            ids: vec![0; palette_len],
            data: Some(longs),
            bits,
            biomes: vec![],
            biome_data: None,
            biome_bits: 0,
        }
    }

    #[test]
    fn unpacks_without_crossing_longs() {
        // 5 bits -> 12 entries per long; entry 12 starts the second long
        let first: u64 = (3 << 5) | 1; // entry0 = 1, entry1 = 3
        let s = section(20, 5, vec![first as i64, 7]);
        assert_eq!(s.index(0), 1);
        assert_eq!(s.index(1), 3);
        assert_eq!(s.index(2), 0);
        assert_eq!(s.index(12), 7);
    }

    #[test]
    fn surface_uses_ypos_not_light_sections() {
        // 26.x overworld chunk: light-only section Y=-5, blocks start at Y=-4 (y = -64).
        // Heightmap value 16 everywhere (9 bits, 7 per long) -> top block y = -64 + 16 - 1 = -49.
        let mut hm = vec![0i64; 37];
        for i in 0..256 {
            hm[i / 7] |= 16i64 << ((i % 7) * 9);
        }
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({
            "Status": "minecraft:full",
            "yPos": -4,
            "Heightmaps": {"WORLD_SURFACE": fastnbt::LongArray::new(hm)},
            "sections": [
                {"Y": -5i8},
                {"Y": -4i8, "block_states": {"palette": ["minecraft:grass_block"]}},
                {"Y": -3i8, "block_states": {"palette": ["minecraft:air"]}}
            ]
        })).unwrap();
        let chunk = Chunk::parse(&nbt, None).unwrap();
        assert_eq!(chunk.surface_y(3, 5), Some(-49));
        assert_eq!(chunk.column(3, 5, None), Column { color: colors::GRASS, y: -49, depth: 0, block: 0, biome: 0, top: -49, plant: 0 });
    }

    #[test]
    fn solid_nether_column_shows_block_under_roof() {
        // no cave between the roof and the bottom: used to come out empty (hole in the map)
        let solid = |y: i8| fastnbt::nbt!({"Y": y, "block_states": {"palette": ["minecraft:netherrack"]}});
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({
            "Status": "minecraft:full",
            "yPos": 0,
            "sections": [solid(0), solid(1), solid(2), solid(3), solid(4), solid(5), solid(6), solid(7)]
        })).unwrap();
        let chunk = Chunk::parse(&nbt, None).unwrap();
        let col = chunk.column(2, 9, Some(127));
        assert_eq!(col.y, 122);
        assert_ne!(col.color, colors::NONE);
    }

    #[test]
    fn nether_cave_below_roof() {
        // air from y 112 to 127 under a roof-less test chunk: the first block below the air is shown
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({
            "Status": "minecraft:full",
            "yPos": 0,
            "sections": [
                {"Y": 6i8, "block_states": {"palette": ["minecraft:netherrack"]}},
                {"Y": 7i8, "block_states": {"palette": ["minecraft:air"]}}
            ]
        })).unwrap();
        assert_eq!(Chunk::parse(&nbt, None).unwrap().column(0, 0, Some(127)).y, 111);
    }

    #[test]
    fn visible_stack_hides_what_is_not_seen() {
        use crate::render::{AIR_MARK, FILL};
        let whole = |y: i8, name: &str| fastnbt::nbt!({"Y": y, "block_states": {"palette": [name]}});
        // stone 0..15, air 16..31 (a cave), stone 32..47, water 48..63
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({
            "Status": "minecraft:full",
            "yPos": 0,
            "sections": [whole(0, "minecraft:stone"), whole(1, "minecraft:air"), whole(2, "minecraft:stone"), whole(3, "minecraft:water")]
        })).unwrap();
        let chunk = Chunk::parse(&nbt, None).unwrap();
        let mut out = vec![];
        // only the water is in view: one water entry, the lake floor, then nothing (solid below)
        chunk.visible_stack(0, 0, 63, 0, |y| y >= 48, |_| false, &mut out);
        assert_eq!(out.iter().map(|e| e.0).collect::<Vec<_>>(), vec![63, 47]);
        assert_eq!(out[1].1, 0); // a block (no texture tables: id 0), not FILL
        // the cave is in view too: rock between lake and cave is FILL, its ceiling and floor are shown
        out.clear();
        chunk.visible_stack(0, 0, 63, 0, |y| y >= 48 || (16..=31).contains(&y), |_| false, &mut out);
        assert_eq!(out, vec![(63, 0), (47, 0), (46, FILL), (32, 0), (15, 0)]);
        // only the lower part of the cave (a reveal box ending at y 25): the hidden air above it is
        // FILL too, the visible air starts with AIR_MARK
        out.clear();
        chunk.visible_stack(0, 0, 63, 0, |y| y >= 48 || (16..=25).contains(&y), |_| false, &mut out);
        assert_eq!(out, vec![(63, 0), (47, 0), (46, FILL), (25, AIR_MARK), (15, 0)]);
        // a neighbour sees into the rock at y 40 (cave beside): that block is shown
        out.clear();
        chunk.visible_stack(0, 0, 63, 0, |y| y >= 48, |y| y == 40, &mut out);
        assert_eq!(out, vec![(63, 0), (47, 0), (46, FILL), (40, 0)]);
    }

    #[test]
    fn visible_stack_ends_with_air_over_the_void() {
        use crate::render::{AIR_MARK, FILL};
        let whole = |y: i8, name: &str| fastnbt::nbt!({"Y": y, "block_states": {"palette": [name]}});
        // an End island: void 0..31, end stone 32..47
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({
            "Status": "minecraft:full",
            "yPos": 0,
            "sections": [whole(0, "minecraft:air"), whole(1, "minecraft:air"), whole(2, "minecraft:end_stone")]
        })).unwrap();
        let chunk = Chunk::parse(&nbt, None).unwrap();
        let mut out = vec![];
        chunk.visible_stack(0, 0, 47, -1, |y| y >= 48 || y <= 31, |_| false, &mut out);
        assert_eq!(out, vec![(47, 0), (46, FILL), (32, 0), (0, AIR_MARK)]);
        // the void not in view: solid below the last entry as before
        out.clear();
        chunk.visible_stack(0, 0, 47, 47, |y| y >= 48, |_| false, &mut out);
        assert_eq!(out, vec![(47, 0)]);
    }

    #[test]
    fn air_runs_of_a_column() {
        // stone at 0..=3, air 4..=6, stone 7, air 8..=15 and above (missing sections are air)
        let palette = |rows: &[usize]| {
            // 4 bits per entry, 16 entries per long: row y (256 entries) = longs y*16 .. y*16+16
            let mut longs = vec![0i64; 256];
            for &y in rows {
                for l in &mut longs[y * 16..y * 16 + 16] {
                    *l = 0x1111_1111_1111_1111;
                }
            }
            longs
        };
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({
            "Status": "minecraft:full",
            "yPos": 0i32,
            "sections": [{"Y": 0i8, "block_states": {
                "palette": ["minecraft:stone", "minecraft:air"],
                "data": fastnbt::LongArray::new(palette(&[4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15]))
            }}]
        }))
        .unwrap();
        let chunk = Chunk::parse(&nbt, None).unwrap();
        let mut runs = Vec::new();
        chunk.air_runs(0, 0, 7, &mut runs);
        assert_eq!(runs, vec![(6, 4)]);
        runs.clear();
        chunk.air_runs(0, 0, 20, &mut runs);
        assert_eq!(runs, vec![(20, 8), (6, 4)]);
    }


    #[test]
    fn skin_hash_from_head_profiles() {
        use fastnbt::Value;
        use std::collections::HashMap;
        // {"textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/<hash>"}}}
        let json = r#"{"textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/3b60a1f6d562f52aaebbf1434f1de147933a3affe0e764fa49ea057536623cd3"}}}"#;
        let b64 = {
            const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut s = String::new();
            for c in json.as_bytes().chunks(3) {
                let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                for k in 0..=c.len() {
                    s.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
                }
            }
            s
        };
        assert_eq!(base64(&b64).unwrap(), json.as_bytes());
        // 1.20.5+: profile.properties = [{name: "textures", value: ...}]
        let prop = Value::Compound(HashMap::from([("name".into(), Value::String("textures".into())), ("value".into(), Value::String(b64.clone()))]));
        let profile = Value::Compound(HashMap::from([("name".into(), Value::String("Steve".into())), ("properties".into(), Value::List(vec![prop]))]));
        assert_eq!(skin_hash(&profile).as_deref(), Some("3b60a1f6d562f52aaebbf1434f1de147933a3affe0e764fa49ea057536623cd3"));
        // a name only: no skin of its own
        assert_eq!(skin_hash(&Value::Compound(HashMap::from([("name".into(), Value::String("Steve".into()))]))), None);
        assert_eq!(skin_hash(&Value::String("x".repeat(60))), None);
    }

    #[test]
    fn banner_patterns_and_pot_sherds() {
        use fastnbt::Value;
        use std::collections::HashMap;
        let s = |v: &str| Value::String(v.into());
        let c = |kv: Vec<(&str, Value)>| Value::Compound(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<HashMap<_, _>>());
        // 1.20.5+: registry id or an inline pattern with its asset id
        let new = Value::List(vec![
            c(vec![("pattern", s("minecraft:creeper")), ("color", s("lime"))]),
            c(vec![("pattern", c(vec![("asset_id", s("minecraft:globe"))])), ("color", s("blue"))]),
            c(vec![("color", s("red"))]),
        ]);
        assert_eq!(
            banner_patterns(Some(&new), None),
            vec![("minecraft:creeper".into(), "lime".into()), ("minecraft:globe".into(), "blue".into())]
        );
        // before 1.20.5: short codes and numeric colours
        let old = Value::List(vec![c(vec![("Pattern", s("cre")), ("Color", Value::Int(5))]), c(vec![("Pattern", s("??")), ("Color", Value::Int(1))])]);
        assert_eq!(banner_patterns(None, Some(&old)), vec![("minecraft:creeper".into(), "lime".into())]);
        // pots: plain sides are bricks; all plain = nothing to draw
        let sherds = Value::List(vec![s("minecraft:brick"), s("minecraft:angler_pottery_sherd")]);
        assert_eq!(pot_sherds(Some(&sherds)).unwrap()[1], "minecraft:angler_pottery_sherd");
        assert_eq!(pot_sherds(Some(&sherds)).unwrap()[3], "minecraft:brick");
        assert_eq!(pot_sherds(Some(&Value::List(vec![s("minecraft:brick")]))), None);
        // 26.x: a map by side, plain sides left out
        let map = c(vec![("front", c(vec![("id", s("minecraft:skull_pottery_sherd"))])), ("left", s("minecraft:heart_pottery_sherd"))]);
        let sides = pot_sherds(Some(&map)).unwrap();
        assert_eq!([sides[0].as_str(), &sides[1], &sides[2], &sides[3]], ["minecraft:brick", "minecraft:heart_pottery_sherd", "minecraft:brick", "minecraft:skull_pottery_sherd"]);
        assert_eq!(pot_sherds(None), None);
    }

    #[test]
    fn palette_entry_shapes() {
        #[derive(Deserialize)]
        struct P {
            palette: Vec<PaletteEntry>,
        }
        // classic compound
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({"palette": [{"Name": "minecraft:stone"}]})).unwrap();
        let p: P = fastnbt::from_bytes(&nbt).unwrap();
        assert_eq!(p.palette[0].name(), "minecraft:stone");
        // 26.x plain strings
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({"palette": ["minecraft:dirt", "minecraft:air"]})).unwrap();
        let p: P = fastnbt::from_bytes(&nbt).unwrap();
        assert_eq!(p.palette[0].name(), "minecraft:dirt");
        // 26.x wrapped strings mixed with compounds
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({"palette": [
            {"": "minecraft:air"},
            {"Name": "minecraft:water", "Properties": {"level": "0"}}
        ]})).unwrap();
        let p: P = fastnbt::from_bytes(&nbt).unwrap();
        assert_eq!(p.palette[0].name(), "minecraft:air");
        assert_eq!(p.palette[1].name(), "minecraft:water");
        // 26.x blocks with non-default properties (leaves, rotated logs, vines ...)
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({"palette": [
            {"": "minecraft:air"},
            {"id": "minecraft:oak_leaves", "properties": {"distance": "4", "persistent": "false"}}
        ]})).unwrap();
        let p: P = fastnbt::from_bytes(&nbt).unwrap();
        assert_eq!(p.palette[1].name(), "minecraft:oak_leaves");
        assert_eq!(p.palette[1].prop("distance"), Some("4"));
        assert_eq!(p.palette[0].prop("distance"), None);
        // classic properties (rail shapes)
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({"palette": [{"Name": "minecraft:rail", "Properties": {"shape": "east_west"}}]})).unwrap();
        let p: P = fastnbt::from_bytes(&nbt).unwrap();
        assert_eq!(p.palette[0].prop("shape"), Some("east_west"));
    }
}
