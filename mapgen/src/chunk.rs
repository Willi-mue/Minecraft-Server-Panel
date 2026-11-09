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
    /// highest solid ground (not leaves, not water, not plants)
    pub ground: i32,
}

/// Vanilla's bedrock roof spans the top 5 blocks below the ceiling (Nether: y 123..=127).
const ROOF_DEPTH: i32 = 5;

pub const EMPTY: Column = Column { color: colors::NONE, y: i32::MIN, depth: 0, block: 0, biome: 0, top: i32::MIN, ground: i32::MIN };

pub struct Chunk {
    sections: Vec<Option<Section>>, // indexed by section y - min_section
    min_section: i32,
    surface: Option<Vec<i64>>,
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
        Some(Chunk { sections, min_section, surface })
    }

    fn min_y(&self) -> i32 {
        self.min_section * 16
    }

    fn section(&self, y: i32) -> Option<&Section> {
        let s = y.div_euclid(16) - self.min_section;
        if s < 0 {
            return None;
        }
        self.sections.get(s as usize)?.as_ref()
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

    /// Highest solid ground at or below `from` (not leaves, water or plants), i32::MIN if none.
    /// Under a thin ceiling (overhangs of ravines, bridges, cave mouths: air within
    /// `OVERHANG` blocks below the solid) the ground is the floor further down, so the
    /// neighbours export their walls down to it.
    fn ground(&self, x: usize, z: usize, from: i32) -> i32 {
        const OVERHANG: i32 = 8;
        let mut y = from;
        while y >= self.min_y() && !self.solid(x, y, z) {
            y -= 1;
        }
        if y < self.min_y() {
            return i32::MIN;
        }
        let mut ground = y;
        loop {
            let Some(air) = (1..=OVERHANG).map(|k| ground - k).take_while(|&a| a >= self.min_y()).find(|&a| !self.solid(x, a, z)) else {
                return ground;
            };
            let mut y = air;
            while y >= self.min_y() && !self.solid(x, y, z) {
                y -= 1;
            }
            if y < self.min_y() {
                return ground;
            }
            ground = y;
        }
    }

    /// The 3D block stack of a column from `top` down to `stop`: (y, block id) of every drawn
    /// block, top first. Of a run of water only the surface is kept (the viewer fills it down to the
    /// next block). The stack always ends on a solid block (it goes on below `stop` through caves,
    /// under bridges …, at most 128 blocks), so the viewer may treat everything below as solid.
    /// At most 255 entries.
    pub fn stack(&self, x: usize, z: usize, top: i32, stop: i32, out: &mut Vec<(i16, u16)>) -> u8 {
        let start = out.len();
        let mut prev_water = false;
        let mut solid = false;
        let mut y = top;
        while y >= self.min_y() && out.len() - start < 255 {
            if y < stop && (solid || y < stop - 128) {
                break;
            }
            let c = self.block(x, y, z);
            if c.water {
                if !prev_water {
                    out.push((y as i16, c.id));
                }
                prev_water = true;
                solid = false;
            } else {
                prev_water = false;
                if c.kind != 0 {
                    out.push((y as i16, c.id));
                }
                solid = c.kind == 1 && !c.leafy;
            }
            y -= 1;
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
        while y >= self.min_y() {
            let cell = self.block(x, y, z);
            if cell.water {
                let surface = y;
                let mut depth = 0u16;
                while y > self.min_y() && self.block(x, y - 1, z).water {
                    y -= 1;
                    depth += 1;
                }
                let biome = self.biome(x, surface, z);
                let ground = self.ground(x, z, y);
                return Column { color: colors::WATER, y: surface, depth: depth + 1, block: cell.id, biome, top, ground };
            }
            if cell.visible {
                // a block without map colour (glass …) that has a texture: use stone grey in 2D fallback
                let color = if cell.color == colors::NONE { colors::STONE } else { cell.color };
                let ground = self.ground(x, z, y);
                return Column { color, y, depth: 0, block: cell.id, biome: self.biome(x, y, z), top, ground };
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
        assert_eq!(chunk.column(3, 5, None), Column { color: colors::GRASS, y: -49, depth: 0, block: 0, biome: 0, top: -49, ground: -49 });
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
    fn block_stack_keeps_floating_leaves_and_one_water_surface() {
        let whole = |y: i8, name: &str| fastnbt::nbt!({"Y": y, "block_states": {"palette": [name]}});
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({
            "Status": "minecraft:full",
            "yPos": 0,
            "sections": [whole(0, "minecraft:stone"), whole(1, "minecraft:air"), whole(2, "minecraft:oak_leaves"),
                         whole(3, "minecraft:water"), whole(4, "minecraft:air")]
        })).unwrap();
        let chunk = Chunk::parse(&nbt, None).unwrap();
        // ground is the stone, not the leaves or the water above
        assert_eq!(chunk.ground(0, 0, 63), 15);
        let mut out = vec![];
        let n = chunk.stack(0, 0, 63, 10, &mut out);
        // water 63..48 -> one entry, leaves 47..32, air 31..16 skipped, stone 15..10
        assert_eq!(n as usize, 1 + 16 + 6);
        assert_eq!(out[0].0, 63);
        assert_eq!(out[1].0, 47);
        assert_eq!(out[16].0, 32);
        assert_eq!(out[17].0, 15);
        assert_eq!(out.last().unwrap().0, 10);
        // a stop inside the air gap goes on down to the stone
        out.clear();
        chunk.stack(0, 0, 63, 25, &mut out);
        assert_eq!(out.last().unwrap().0, 15);
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
    fn ground_looks_under_thin_ceilings() {
        // stone 0..15, air 16..31, a 16-block thick "ceiling" of stone 32..47 (too thick: ground on top)
        let whole = |y: i8, name: &str| fastnbt::nbt!({"Y": y, "block_states": {"palette": [name]}});
        let thick = fastnbt::to_bytes(&fastnbt::nbt!({"Status": "minecraft:full", "yPos": 0,
            "sections": [whole(0, "minecraft:stone"), whole(1, "minecraft:air"), whole(2, "minecraft:stone")]})).unwrap();
        assert_eq!(Chunk::parse(&thick, None).unwrap().ground(0, 0, 47), 47);
        // 4-block overhang (y 28..31) above air 16..27: the floor at 15 is the ground
        let thin = fastnbt::to_bytes(&fastnbt::nbt!({"Status": "minecraft:full", "yPos": 0,
            "sections": [whole(0, "minecraft:stone"),
                {"Y": 1i8, "block_states": {"palette": ["minecraft:air", "minecraft:stone"],
                 "data": fastnbt::LongArray::new(over_top_rows(12, 16))}}]})).unwrap();
        assert_eq!(Chunk::parse(&thin, None).unwrap().ground(0, 0, 31), 15);
    }

    /// Section data (4 bits per block, palette [air, stone]): stone in the layers >= `from`.
    fn over_top_rows(from: usize, layers: usize) -> Vec<i64> {
        let mut longs = vec![0i64; 256];
        for i in 0..4096 {
            if i / 256 >= from && i / 256 < layers {
                longs[i / 16] |= 1i64 << ((i % 16) * 4);
            }
        }
        longs
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
