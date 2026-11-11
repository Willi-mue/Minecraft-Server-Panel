//! Render one region (512×512 blocks) into map-item style pixels plus a height map.

use std::path::Path;

use crate::assets::Assets;
use crate::chunk::{Chunk, Column, Decor, EMPTY};
use crate::colors::{self, HIGH, LOW, NORMAL};
use crate::region::Region;

pub const SIZE: usize = 512;

pub struct RegionImage {
    /// RGBA, row-major, z rows of x pixels
    pub rgba: Vec<u8>,
    /// Surface heights as u16 (`y + 32768`), 0 = no data. Basis for the 3D view.
    pub heights: Vec<u16>,
    /// With texture tables: RGBA per column = block id (u16, big endian), biome id, 255 (0 = none)
    pub blocks: Option<Vec<u8>>,
    /// With texture tables: RGBA per column = id (u16, big endian) of the plant / flat block over
    /// the coloured block, 0, 255 (0 = none)
    pub plants: Option<Vec<u8>>,
    /// player heads with a skin of their own that the 3D stacks show (`heads.json`)
    pub heads: Vec<HeadOut>,
    /// banner patterns and pot sherds that the 3D stacks show (`decor.json`)
    pub decor: Vec<DecorOut>,
    /// With texture tables: the 3D block stacks (zlib, see `voxels`)
    pub voxels: Option<Vec<u8>>,
    /// With texture tables and `Visibility::all_caves`: the same stacks with every cave shown
    pub caves: Option<Vec<u8>>,
    pub chunks: usize,
}

/// The parsed chunks of a region, row-major (cz * 32 + cx); None where nothing is generated.
/// Parsed once per render and shared by the colour tiles and the 3D stacks.
fn parse_chunks(region: &Region, rx: i32, rz: i32, assets: Option<&Assets>) -> Vec<Option<Chunk>> {
    (0..32 * 32)
        .map(|i| {
            let (cx, cz) = (i % 32, i / 32);
            let nbt = region.chunk(cx, cz, rx * 32 + cx as i32, rz * 32 + cz as i32)?;
            Chunk::parse(&nbt, assets)
        })
        .collect()
}

/// The columns of parsed chunks (see `parse_chunks`), EMPTY where nothing is generated.
fn columns_of(chunks: &[Option<Chunk>], ceiling: Option<i32>) -> Vec<Column> {
    let mut cols = vec![EMPTY; SIZE * SIZE];
    for (i, chunk) in chunks.iter().enumerate() {
        let Some(chunk) = chunk else { continue };
        let (cx, cz) = (i % 32, i / 32);
        for z in 0..16 {
            for x in 0..16 {
                cols[(cz * 16 + z) * SIZE + cx * 16 + x] = chunk.column(x, z, ceiling);
            }
        }
    }
    cols
}

/// All columns of a region (row-major by z), EMPTY where nothing is generated.
fn columns(region: &Region, rx: i32, rz: i32, ceiling: Option<i32>, only_row: Option<usize>, assets: Option<&Assets>) -> (Vec<Column>, usize) {
    let mut cols = vec![EMPTY; SIZE * SIZE];
    let mut chunks = 0;
    for cz in 0..32 {
        if only_row.is_some_and(|r| r != cz) {
            continue;
        }
        for cx in 0..32 {
            let Some(nbt) = region.chunk(cx, cz, rx * 32 + cx as i32, rz * 32 + cz as i32) else { continue };
            let Some(chunk) = Chunk::parse(&nbt, assets) else { continue };
            chunks += 1;
            for z in 0..16 {
                for x in 0..16 {
                    cols[(cz * 16 + z) * SIZE + cx * 16 + x] = chunk.column(x, z, ceiling);
                }
            }
        }
    }
    (cols, chunks)
}

/// Heights of the southernmost block row of the region north of (rx, rz), for seamless shading.
fn north_edge(dir: &Path, rx: i32, rz: i32, ceiling: Option<i32>) -> Option<Vec<i32>> {
    let region = Region::open(&dir.join(format!("r.{}.{}.mca", rx, rz - 1))).ok()?;
    let (cols, _) = columns(&region, rx, rz - 1, ceiling, Some(31), None);
    Some((0..SIZE).map(|x| cols[(SIZE - 1) * SIZE + x].y).collect())
}

/// Brightness like Minecraft's map items: slope towards the north for land,
/// depth with a checkerboard pattern for water.
pub fn brightness(col: &Column, north_y: i32, checker: bool) -> u8 {
    let parity = checker as i32 as f64;
    if col.color == colors::WATER {
        let d = col.depth as f64 * 0.1 + parity * 0.2;
        return if d < 0.5 { HIGH } else if d > 0.9 { LOW } else { NORMAL };
    }
    let north = if north_y == i32::MIN { col.y } else { north_y };
    let d = (col.y - north) as f64 * 0.8 + (parity - 0.5) * 0.4;
    if d > 0.6 { HIGH } else if d < -0.6 { LOW } else { NORMAL }
}

/// A player head for `heads.json`: position, skin texture hash, y rotation of the model (degrees,
/// the face looks north at 0) and whether it hangs on a wall.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HeadOut {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub skin: String,
    pub rot: u16,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wall: bool,
}

/// What the 3D viewer draws over a block model (`decor.json`): banner pattern layers or pot
/// sherds. `rot` turns the model (front on the north side) clockwise seen from above, like the
/// blockstate `y` rotation of its block model.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecorOut {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub rot: u16,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wall: bool,
    /// banner layers, bottom first: atlas index of the flag front, of its back, dye colour "rrggbb"
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub banner: Vec<(u32, u32, String)>,
    /// pot sides: atlas index of back, left, right, front (0 = plain); left/right as seen from the front
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pot: Option<[u32; 4]>,
}

/// Rotation and wall flag of a banner block state, the same quarter turns as its block model
/// (`assets::entity_models`). None: not a banner.
pub fn banner_pose(state: &str) -> Option<(u16, bool)> {
    let (name, props) = state.split_once('[').unwrap_or((state, ""));
    let prop = |k: &str| props.trim_end_matches(']').split(',').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='));
    if name.ends_with("_wall_banner") {
        Some((facing_rot(prop("facing").unwrap_or("north")), true))
    } else if name.ends_with("_banner") {
        let r: usize = prop("rotation").and_then(|v| v.parse().ok()).unwrap_or(8); // the default state
        Some(([180, 270, 0, 90][((r + 2) / 4) % 4], false))
    } else {
        None
    }
}

/// Model rotation of a decorated pot: `facing` is where the placing player looked, the front
/// sherd faces that player.
pub fn pot_rot(state: &str) -> Option<u16> {
    let (name, props) = state.split_once('[').unwrap_or((state, ""));
    if name != "minecraft:decorated_pot" {
        return None;
    }
    let facing = props.trim_end_matches(']').split(',').find_map(|kv| kv.strip_prefix("facing=")).unwrap_or("north");
    Some((facing_rot(facing) + 180) % 360)
}

fn facing_rot(facing: &str) -> u16 {
    match facing {
        "east" => 90,
        "south" => 180,
        "west" => 270,
        _ => 0,
    }
}

/// Dye colour of a banner layer as "rrggbb".
fn dye_hex(name: &str) -> Option<&'static str> {
    crate::assets::DYES.iter().find(|(n, _)| *n == name.trim_start_matches("minecraft:")).map(|(_, rgb)| *rgb)
}

/// Is the conduit at (x, y, z) active? The game's rule: the 26 blocks around it are water and at
/// least 16 of the 42 frame positions (the three 5×5 rings through it) are prismarine, prismarine
/// bricks, dark prismarine or sea lanterns. `id_at` = block id (None outside the loaded chunks);
/// waterlogged blocks count as not water.
pub fn conduit_active(x: i32, y: i32, z: i32, id_at: impl Fn(i32, i32, i32) -> Option<u16>, water: u16, frame: &[u16]) -> bool {
    for i in -1..=1 {
        for j in -1..=1 {
            for k in -1..=1 {
                if (i, j, k) != (0, 0, 0) && id_at(x + i, y + j, z + k) != Some(water) {
                    return false;
                }
            }
        }
    }
    let mut n = 0;
    for i in -2i32..=2 {
        for j in -2i32..=2 {
            for k in -2i32..=2 {
                let (a, b, c) = (i.abs(), j.abs(), k.abs());
                let ring = (i == 0 && (b == 2 || c == 2)) || (j == 0 && (a == 2 || c == 2)) || (k == 0 && (a == 2 || b == 2));
                if (a > 1 || b > 1 || c > 1) && ring && id_at(x + i, y + j, z + k).is_some_and(|id| frame.contains(&id)) {
                    n += 1;
                }
            }
        }
    }
    n >= 16
}

/// Rotation and wall flag of a player head block state ("minecraft:player_head[rotation=8]"), the
/// same quarter turns its block model uses (see `assets::entity_models`). None: not a player head.
pub fn head_pose(state: &str) -> Option<(u16, bool)> {
    let (name, props) = state.split_once('[').unwrap_or((state, ""));
    let prop = |k: &str| props.trim_end_matches(']').split(',').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='));
    match name {
        "minecraft:player_head" => {
            let r: usize = prop("rotation").and_then(|v| v.parse().ok()).unwrap_or(0);
            Some(([180, 270, 0, 90][((r + 2) / 4) % 4], false))
        }
        "minecraft:player_wall_head" => {
            let rot = match prop("facing").unwrap_or("north") {
                "east" => 90,
                "south" => 180,
                "west" => 270,
                _ => 0,
            };
            Some((rot, true))
        }
        _ => None,
    }
}

pub fn render_region(path: &Path, rx: i32, rz: i32, ceiling: Option<i32>, assets: Option<&Assets>, vis: &Visibility) -> std::io::Result<RegionImage> {
    let region = Region::open(path)?;
    let parsed = parse_chunks(&region, rx, rz, assets);
    let chunks = parsed.iter().filter(|c| c.is_some()).count();
    let cols = columns_of(&parsed, ceiling);
    let edge = north_edge(path.parent().unwrap_or(Path::new(".")), rx, rz, ceiling);

    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let mut heights = vec![0u16; SIZE * SIZE];
    shade(&cols, edge.as_deref(), rx, rz, 0..SIZE, &mut rgba, &mut heights, assets);
    let blocks = assets.map(|_| {
        let mut b = vec![0u8; SIZE * SIZE * 4];
        for (i, c) in cols.iter().enumerate() {
            if c.color != colors::NONE {
                b[i * 4..i * 4 + 4].copy_from_slice(&[(c.block >> 8) as u8, c.block as u8, c.biome, 255]);
            }
        }
        b
    });
    let plants = assets.map(|_| {
        let mut p = vec![0u8; SIZE * SIZE * 4];
        for (i, c) in cols.iter().enumerate() {
            if c.plant != 0 {
                p[i * 4..i * 4 + 4].copy_from_slice(&[(c.plant >> 8) as u8, c.plant as u8, 0, 255]);
            }
        }
        p
    });
    let (voxels, caves, heads, decor) = match assets {
        Some(_) => {
            let (v, c, h, d) = voxels(&parsed, path.parent().unwrap_or(Path::new(".")), rx, rz, ceiling, &cols, assets, vis)?;
            (Some(v), c, h, d)
        }
        None => (None, None, vec![], vec![]),
    };
    Ok(RegionImage { rgba, heights, blocks, plants, voxels, caves, chunks, heads, decor })
}

/// Version of the `v/` stack files (part of the state, so a new format re-renders everything).
pub const VOXEL_FORMAT: &str = "MGV2";
/// Revision of the rules that decide what the stacks contain (part of the state as well).
pub const VOXEL_RULES: u32 = 2;

/// What the 3D view may show below the surface.
#[derive(Clone, Debug, Default)]
pub struct Visibility {
    /// how far (in columns) caves are shown from where they are open to the sky
    pub reach: u16,
    /// also write a second set of stacks (`c/`) in which every cave is shown; the viewer loads
    /// them only while its height slice is on
    pub all_caves: bool,
}

/// Stack entry: from here down to the next entry everything is solid and not shown (rock the map
/// does not reveal: hidden caves, the inside of mountains).
pub const FILL: u16 = 0xFFFF;
/// Stack entry: visible air starts here (after a FILL run; not written by the current rules, which
/// always show the rock around visible air, but understood by the viewer).
pub const AIR_MARK: u16 = 0xFFFE;

/// The 3D block stacks of a region: per column the blocks that can be seen: everything around air
/// that is open to the sky (cliffs, overhangs, ravines, cave mouths and caves up to `reach` columns
/// from where they open). What lies between is written as FILL runs, so the files do not give away hidden caves either.
/// Format (zlib): "MGV2", u8 count per column (row-major z, x), then the entries in the same order,
/// each i16 y + u16 block id (or FILL / AIR_MARK), little endian. Gaps between entries are air,
/// below a water entry water down to the next entry, below the last entry solid, except when the
/// last entry is AIR_MARK: then air down to the world bottom (the End's void under an island).
/// Returns these stacks and, with `vis.all_caves`, the same format with every cave shown.
#[allow(clippy::type_complexity)]
fn voxels(own_chunks: &[Option<Chunk>], dir: &Path, rx: i32, rz: i32, ceiling: Option<i32>, cols: &[Column], assets: Option<&Assets>, vis: &Visibility) -> std::io::Result<(Vec<u8>, Option<Vec<u8>>, Vec<HeadOut>, Vec<DecorOut>)> {
    // Everything is computed on a grid padded by two chunks on every side with the adjacent chunks
    // of the 4 neighbour regions, so caves crossing a region border look the same from both sides.
    const PAD: usize = 32;
    const PAD_CHUNKS: usize = PAD / 16;
    const W: usize = SIZE + 2 * PAD;
    const CW: usize = W / 16; // chunks per padded row
    const NO: i32 = i32::MIN; // no column
    const VOID: i32 = i32::MIN + 1; // a generated column without blocks (the End's void): all air
    let neighbours: Vec<Option<Region>> = [(0, -1), (0, 1), (-1, 0), (1, 0)]
        .iter()
        .map(|&(dx, dz)| Region::open(&dir.join(format!("r.{}.{}.mca", rx + dx, rz + dz))).ok())
        .collect();
    // chunks of the neighbour regions (the region's own are already parsed: `own_chunks`)
    let load = |pcx: usize, pcz: usize| -> Option<Chunk> {
        let (cx, cz) = (pcx as i32 - PAD_CHUNKS as i32, pcz as i32 - PAD_CHUNKS as i32);
        let (ox, oz) = (cx.div_euclid(32), cz.div_euclid(32));
        let r = match (ox, oz) {
            (0, 0) => return None,
            (0, -1) => neighbours[0].as_ref()?,
            (0, 1) => neighbours[1].as_ref()?,
            (-1, 0) => neighbours[2].as_ref()?,
            (1, 0) => neighbours[3].as_ref()?,
            _ => return None, // diagonal corners
        };
        let (lx, lz) = (cx.rem_euclid(32) as usize, cz.rem_euclid(32) as usize);
        let nbt = r.chunk(lx, lz, (rx + ox) * 32 + lx as i32, (rz + oz) * 32 + lz as i32)?;
        Chunk::parse(&nbt, assets)
    };
    let own = |gx: usize, gz: usize| (PAD..PAD + SIZE).contains(&gx) && (PAD..PAD + SIZE).contains(&gz);
    let nbrs = |i: usize| {
        let (x, z) = (i % W, i / W);
        [(x > 0).then(|| i - 1), (x + 1 < W).then(|| i + 1), (z > 0).then(|| i - W), (z + 1 < W).then(|| i + W)]
    };

    // 1. per column: top and the runs of non-solid blocks (air, plants, leaves, water …)
    let mut tops = vec![NO; W * W];
    let mut run_at = vec![0u32; W * W];
    let mut run_len = vec![0u32; W * W];
    let mut runs: Vec<(i16, i16)> = Vec::new();
    let mut run_col: Vec<u32> = Vec::new();
    let mut bottomless: Vec<bool> = Vec::new(); // run r reaches the world bottom: open to the void
    for pcz in 0..CW {
        for pcx in 0..CW {
            let owned;
            let chunk: Option<&Chunk> = if (PAD_CHUNKS..PAD_CHUNKS + 32).contains(&pcx) && (PAD_CHUNKS..PAD_CHUNKS + 32).contains(&pcz) {
                own_chunks[(pcz - PAD_CHUNKS) * 32 + pcx - PAD_CHUNKS].as_ref()
            } else {
                owned = load(pcx, pcz);
                owned.as_ref()
            };
            for z in 0..16 {
                for x in 0..16 {
                    let (gx, gz) = (pcx * 16 + x, pcz * 16 + z);
                    let i = gz * W + gx;
                    let Some(chunk) = chunk else { continue };
                    let c = if own(gx, gz) { cols[(gz - PAD) * SIZE + gx - PAD] } else { chunk.column(x, z, ceiling) };
                    if c.top == NO {
                        tops[i] = VOID;
                        continue;
                    }
                    tops[i] = c.top;
                    run_at[i] = runs.len() as u32;
                    chunk.air_runs(x, z, c.top, &mut runs);
                    run_len[i] = runs.len() as u32 - run_at[i];
                    let floor = chunk.min_y() as i16;
                    bottomless.extend(runs[run_at[i] as usize..].iter().map(|r| r.1 <= floor));
                    run_col.extend(std::iter::repeat_n(i as u32, run_len[i] as usize));
                }
            }
        }
    }
    let runs_of = |i: usize| run_at[i] as usize..(run_at[i] + run_len[i]) as usize;

    // 2. which runs are open to view (hops from the sky, see `open_runs`)
    let hops = open_runs(W, &tops, &run_at, &run_len, &runs, &run_col, &bottomless, vis.reach);
    // is run r shown? (`all`: the every-cave stacks)
    let shown = |r: usize, all: bool| all || hops[r] <= vis.reach;
    // is the non-solid block at height y of column i in view? (above the top: open air)
    let seen = |i: usize, y: i32, all: bool| -> bool {
        y > tops[i]
            || runs_of(i).any(|r| {
                let (t, b) = (runs[r].0 as i32, runs[r].1 as i32);
                (b..=t).contains(&y) && shown(r, all)
            })
    };
    // lowest air in view per column (above the top when nothing below it is)
    let lowest = |i: usize, all: bool| -> i32 {
        let mut low = tops[i].saturating_add(1);
        for r in runs_of(i) {
            if shown(r, all) {
                low = low.min(runs[r].1 as i32);
            }
        }
        low
    };

    // 3. the stacks of the region's own columns (both sets in one pass over the chunks)
    let sets: &[bool] = if vis.all_caves { &[false, true] } else { &[false] };
    let mut counts = vec![vec![0u8; SIZE * SIZE]; sets.len()];
    let mut spans = vec![vec![0u32; SIZE * SIZE]; sets.len()];
    let mut pools: Vec<Vec<(i16, u16)>> = vec![Vec::new(); sets.len()];
    for cz in 0..32 {
        for cx in 0..32 {
            let Some(chunk) = &own_chunks[cz * 32 + cx] else { continue };
            for z in 0..16 {
                for x in 0..16 {
                    let (lx, lz) = (cx * 16 + x, cz * 16 + z);
                    let i = (lz + PAD) * W + lx + PAD;
                    if tops[i] == NO || tops[i] == VOID {
                        continue;
                    }
                    let near: Vec<usize> = nbrs(i).into_iter().flatten().filter(|&n| tops[n] != NO).collect();
                    for (k, &all) in sets.iter().enumerate() {
                        let bottom = near.iter().map(|&n| lowest(n, all)).fold(lowest(i, all) - 1, i32::min);
                        let side = |y: i32| near.iter().any(|&n| seen(n, y, all));
                        spans[k][lz * SIZE + lx] = pools[k].len() as u32;
                        counts[k][lz * SIZE + lx] = chunk.visible_stack(x, z, tops[i], bottom, |y| seen(i, y, all), side, &mut pools[k]);
                    }
                }
            }
        }
    }
    // block entities of the region's own chunks with their local column
    let entities = || {
        own_chunks.iter().enumerate().filter_map(|(ci, c)| c.as_ref().map(|c| (ci, c))).flat_map(move |(ci, chunk)| {
            chunk.decor.iter().filter_map(move |e| {
                let (lx, lz) = (e.x - rx * SIZE as i32, e.z - rz * SIZE as i32);
                let inside = (0..SIZE as i32).contains(&lx) && (0..SIZE as i32).contains(&lz) && (lz as usize / 16) * 32 + lx as usize / 16 == ci;
                inside.then_some((chunk, lx as usize, lz as usize, e))
            })
        })
    };
    // where in a stack set a block is listed (only what the stacks show gets decorated)
    let listed = |pools: &[Vec<(i16, u16)>], k: usize, col: usize, y: i32, id: u16| -> Option<usize> {
        let start = spans[k][col] as usize;
        (start..start + counts[k][col] as usize).find(|&p| pools[k][p] == (y as i16, id))
    };
    // active conduits (frame complete) get the `active=true` state: cage and eye
    if let Some(a) = assets {
        let (off, on) = (a.block_id("minecraft:conduit[active=false]"), a.block_id("minecraft:conduit[active=true]"));
        let water = a.block_id("minecraft:water");
        let frame: Vec<u16> = ["prismarine", "prismarine_bricks", "dark_prismarine", "sea_lantern"]
            .iter()
            .map(|n| a.block_id(&format!("minecraft:{n}")))
            .filter(|&i| i != 0)
            .collect();
        // the frame may reach into the neighbour regions: their chunks next to a conduit, loaded
        // once (none for the diagonal corners)
        let mut outside: std::collections::HashMap<(i32, i32), Option<Chunk>> = std::collections::HashMap::new();
        for (_, _, _, e) in entities().filter(|(_, _, _, e)| e.decor == Decor::Conduit) {
            for (dx, dz) in [(-2, -2), (-2, 2), (2, -2), (2, 2)] {
                let (cx, cz) = ((e.x + dx - rx * SIZE as i32).div_euclid(16), (e.z + dz - rz * SIZE as i32).div_euclid(16));
                if !(0..32).contains(&cx) || !(0..32).contains(&cz) {
                    outside.entry((cx, cz)).or_insert_with(|| load((cx + PAD_CHUNKS as i32) as usize, (cz + PAD_CHUNKS as i32) as usize));
                }
            }
        }
        let id_at = |x: i32, y: i32, z: i32| -> Option<u16> {
            let (lx, lz) = (x - rx * SIZE as i32, z - rz * SIZE as i32);
            let (cx, cz) = (lx.div_euclid(16), lz.div_euclid(16));
            let c = if (0..32).contains(&cx) && (0..32).contains(&cz) {
                own_chunks[(cz * 32 + cx) as usize].as_ref()?
            } else {
                outside.get(&(cx, cz))?.as_ref()?
            };
            Some(c.block_id(lx.rem_euclid(16) as usize, y, lz.rem_euclid(16) as usize))
        };
        if off != 0 && on != 0 && water != 0 {
            let active: Vec<(usize, i32)> = entities()
                .filter(|(_, _, _, e)| e.decor == Decor::Conduit && conduit_active(e.x, e.y, e.z, id_at, water, &frame))
                .map(|(_, lx, lz, e)| (lz * SIZE + lx, e.y))
                .collect();
            for (col, y) in active {
                for k in 0..sets.len() {
                    if let Some(p) = listed(&pools, k, col, y, off) {
                        pools[k][p].1 = on;
                    }
                }
            }
        }
    }
    // banner patterns and pot sherds over the block models
    let mut decor = vec![];
    if let Some(a) = assets {
        for (chunk, lx, lz, e) in entities() {
            let id = chunk.block_id(lx % 16, e.y, lz % 16);
            if listed(&pools, 0, lz * SIZE + lx, e.y, id).is_none() {
                continue;
            }
            let state = &a.block(id).name;
            let (x, y, z) = (e.x, e.y, e.z);
            match &e.decor {
                Decor::Banner(layers) => {
                    let Some((rot, wall)) = banner_pose(state) else { continue };
                    let banner: Vec<(u32, u32, String)> = layers
                        .iter()
                        .filter_map(|(p, c)| {
                            let [front, back] = a.pattern(p)?;
                            Some((front, back, dye_hex(c)?.to_string()))
                        })
                        .collect();
                    if !banner.is_empty() {
                        decor.push(DecorOut { x, y, z, rot, wall, banner, pot: None });
                    }
                }
                Decor::Pot(sides) => {
                    let Some(rot) = pot_rot(state) else { continue };
                    let pot = [0, 1, 2, 3].map(|i| a.sherd(&sides[i]).unwrap_or(0));
                    if pot != [0; 4] {
                        decor.push(DecorOut { x, y, z, rot, wall: false, banner: vec![], pot: Some(pot) });
                    }
                }
                Decor::Conduit => {}
            }
        }
    }
    let encode = |k: usize| -> std::io::Result<Vec<u8>> {
        let (counts, spans, pool) = (&counts[k], &spans[k], &pools[k]);
        let mut raw = Vec::with_capacity(4 + SIZE * SIZE + pool.len() * 4);
        raw.extend_from_slice(b"MGV2");
        raw.extend_from_slice(counts);
        for i in 0..SIZE * SIZE {
            let start = spans[i] as usize;
            for &(y, id) in &pool[start..start + counts[i] as usize] {
                raw.extend_from_slice(&y.to_le_bytes());
                raw.extend_from_slice(&id.to_le_bytes());
            }
        }
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut enc, &raw)?;
        enc.finish()
    };
    // player heads with their own skin, only where the normal stacks show them (a head in a hidden
    // cave stays hidden)
    let mut heads = vec![];
    for (ci, chunk) in own_chunks.iter().enumerate() {
        let Some(chunk) = chunk else { continue };
        for h in &chunk.heads {
            let (lx, lz) = (h.x - rx * SIZE as i32, h.z - rz * SIZE as i32);
            if !(0..SIZE as i32).contains(&lx) || !(0..SIZE as i32).contains(&lz) || (lz as usize / 16) * 32 + lx as usize / 16 != ci {
                continue;
            }
            let id = chunk.block_id(lx as usize % 16, h.y, lz as usize % 16);
            let Some((rot, wall)) = assets.and_then(|a| head_pose(&a.block(id).name)) else { continue };
            let col = lz as usize * SIZE + lx as usize;
            let start = spans[0][col] as usize;
            if pools[0][start..start + counts[0][col] as usize].iter().any(|&(y, i)| y as i32 == h.y && i == id) {
                heads.push(HeadOut { x: h.x, y: h.y, z: h.z, skin: h.skin.clone(), rot, wall });
            }
        }
    }
    let caves = if sets.len() > 1 { Some(encode(1)?) } else { None };
    Ok((encode(0)?, caves, heads, decor))
}

/// How many columns each run of non-solid blocks is away from the sky (u16::MAX = further than
/// `reach` or not connected). Runs touching the sky count 0: the open space above a column's top,
/// also beside it (above a lower neighbour: cave mouths in cliffs). From there it spreads through
/// overlapping runs of the 4 neighbour columns, one per column. `bottomless[r]`: run r reaches the
/// world bottom (no bedrock: the End's void), open as well. `w`: width of the grid (row-major);
/// `tops[i] == i32::MIN`: no column; runs of column i: `runs[run_at[i]..run_at[i] + run_len[i]]`
/// as (top y, bottom y), `run_col[r]` = column of run r.
#[allow(clippy::too_many_arguments)]
fn open_runs(w: usize, tops: &[i32], run_at: &[u32], run_len: &[u32], runs: &[(i16, i16)], run_col: &[u32], bottomless: &[bool], reach: u16) -> Vec<u16> {
    let runs_of = |i: usize| run_at[i] as usize..(run_at[i] + run_len[i]) as usize;
    let rows = tops.len() / w;
    let nbrs = |i: usize| {
        let (x, z) = (i % w, i / w);
        [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (z > 0).then(|| i - w), (z + 1 < rows).then(|| i + w)]
    };
    let mut hops = vec![u16::MAX; runs.len()];
    let mut work: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for i in 0..tops.len() {
        for r in runs_of(i) {
            let t = runs[r].0 as i32;
            if t >= tops[i] || bottomless[r] || nbrs(i).into_iter().flatten().any(|n| tops[n] != i32::MIN && t > tops[n]) {
                hops[r] = 0;
                work.push_back(r);
            }
        }
    }
    while let Some(r) = work.pop_front() {
        let h = hops[r];
        if h >= reach {
            continue;
        }
        let (t, b) = runs[r];
        for n in nbrs(run_col[r] as usize).into_iter().flatten() {
            for q in runs_of(n) {
                let (t2, b2) = runs[q];
                if b <= t2 && b2 <= t && hops[q] > h + 1 {
                    hops[q] = h + 1;
                    work.push_back(q);
                }
            }
        }
    }
    hops
}

/// Re-shade only the northernmost chunk row of an existing tile (after the region north of it
/// changed, the slope shading along the border may differ). Parses 32 chunks instead of 1024.
pub fn patch_north_edge(path: &Path, rx: i32, rz: i32, ceiling: Option<i32>, rgba: &mut [u8], heights: &mut [u16], assets: Option<&Assets>) -> std::io::Result<()> {
    let region = Region::open(path)?;
    let (cols, _) = columns(&region, rx, rz, ceiling, Some(0), assets);
    let edge = north_edge(path.parent().unwrap_or(Path::new(".")), rx, rz, ceiling);
    for z in 0..16 {
        rgba[z * SIZE * 4..(z + 1) * SIZE * 4].fill(0);
        heights[z * SIZE..(z + 1) * SIZE].fill(0);
    }
    shade(&cols, edge.as_deref(), rx, rz, 0..16, rgba, heights, assets);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn shade(cols: &[Column], edge: Option<&[i32]>, rx: i32, rz: i32, rows: std::ops::Range<usize>, rgba: &mut [u8], heights: &mut [u16], assets: Option<&Assets>) {
    for z in rows {
        for x in 0..SIZE {
            let col = &cols[z * SIZE + x];
            if col.color == colors::NONE {
                continue;
            }
            let north_y = if z > 0 {
                cols[(z - 1) * SIZE + x].y
            } else {
                edge.map_or(i32::MIN, |e| e[x])
            };
            // checkerboard on absolute block coordinates, so it continues across tiles
            let abs = (rx as i64 * SIZE as i64 + x as i64) + (rz as i64 * SIZE as i64 + z as i64);
            let level = brightness(col, north_y, abs & 1 == 1);
            let [r, g, b] = match assets {
                // real block colours (texture average × biome tint) with the map's relief shading
                Some(a) if col.block != 0 => colors::relief(a.color(col.block, col.biome), level),
                _ => colors::shaded(col.color, level),
            };
            let p = (z * SIZE + x) * 4;
            rgba[p..p + 4].copy_from_slice(&[r, g, b, 255]);
            heights[z * SIZE + x] = (col.y + 32768).clamp(1, u16::MAX as i32) as u16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn land(y: i32) -> Column {
        Column { color: colors::GRASS, y, depth: 0, ..EMPTY }
    }

    #[test]
    fn head_poses() {
        assert_eq!(head_pose("minecraft:player_head[rotation=8]"), Some((0, false)));
        assert_eq!(head_pose("minecraft:player_head"), Some((180, false)));
        assert_eq!(head_pose("minecraft:player_wall_head[facing=east]"), Some((90, true)));
        assert_eq!(head_pose("minecraft:zombie_head[rotation=8]"), None);
    }

    #[test]
    fn banner_and_pot_poses() {
        assert_eq!(banner_pose("minecraft:red_banner[rotation=8]"), Some((0, false)));
        assert_eq!(banner_pose("minecraft:red_wall_banner[facing=west]"), Some((270, true)));
        assert_eq!(banner_pose("minecraft:stone"), None);
        // a pot placed looking north shows its front to the south
        assert_eq!(pot_rot("minecraft:decorated_pot[facing=north,waterlogged=false]"), Some(180));
        assert_eq!(pot_rot("minecraft:decorated_pot[facing=east]"), Some(270));
        assert_eq!(pot_rot("minecraft:flower_pot"), None);
        assert_eq!(dye_hex("lime"), Some("80c71f"));
    }

    #[test]
    fn conduit_frame() {
        const WATER: u16 = 1;
        const PRISMARINE: u16 = 2;
        // a conduit at 0,0,0 in water with `frame` prismarine blocks on the frame positions
        let world = |frame: usize| {
            let mut spots = vec![];
            for i in -2i32..=2 {
                for j in -2i32..=2 {
                    for k in -2i32..=2 {
                        let (a, b, c) = (i.abs(), j.abs(), k.abs());
                        let ring = (i == 0 && (b == 2 || c == 2)) || (j == 0 && (a == 2 || c == 2)) || (k == 0 && (a == 2 || b == 2));
                        if (a > 1 || b > 1 || c > 1) && ring {
                            spots.push((i, j, k));
                        }
                    }
                }
            }
            assert_eq!(spots.len(), 42);
            spots.truncate(frame);
            move |x, y, z| Some(if spots.contains(&(x, y, z)) { PRISMARINE } else { WATER })
        };
        assert!(conduit_active(0, 0, 0, world(16), WATER, &[PRISMARINE]));
        assert!(!conduit_active(0, 0, 0, world(15), WATER, &[PRISMARINE]));
        // not in water: inactive
        let dry = |x: i32, y: i32, z: i32| if (x, y, z) == (1, 0, 0) { Some(0) } else { world(42)(x, y, z) };
        assert!(!conduit_active(0, 0, 0, dry, WATER, &[PRISMARINE]));
    }

    #[test]
    fn slopes_shade_like_maps() {
        assert_eq!(brightness(&land(70), 68, false), HIGH); // rising towards the south
        assert_eq!(brightness(&land(66), 68, false), LOW);  // falling
        assert_eq!(brightness(&land(68), 68, false), NORMAL);
        assert_eq!(brightness(&land(68), i32::MIN, true), NORMAL); // unknown neighbour = flat
    }

    #[test]
    fn water_gets_darker_with_depth() {
        let w = |depth| Column { color: colors::WATER, y: 62, depth, ..EMPTY };
        assert_eq!(brightness(&w(1), 0, false), HIGH);
        assert_eq!(brightness(&w(6), 0, false), NORMAL);
        assert_eq!(brightness(&w(12), 0, false), LOW);
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;

    /// A row of columns (width w, one row): (top, runs) per column.
    fn grid(cols: &[(i32, &[(i16, i16)])]) -> (Vec<i32>, Vec<u32>, Vec<u32>, Vec<(i16, i16)>, Vec<u32>) {
        let (mut tops, mut at, mut len, mut runs, mut col) = (vec![], vec![], vec![], vec![], vec![]);
        for (i, (top, rs)) in cols.iter().enumerate() {
            tops.push(*top);
            at.push(runs.len() as u32);
            len.push(rs.len() as u32);
            runs.extend_from_slice(rs);
            col.extend(std::iter::repeat_n(i as u32, rs.len()));
        }
        (tops, at, len, runs, col)
    }

    #[test]
    fn caves_open_to_the_sky_up_to_the_reach() {
        // column 0: a cliff foot at y 60 (open sky above 60); columns 1..: rock up to 80 with a
        // cave at y 62..64 that opens into the cliff, and a sealed cave at 30..32 under column 1
        let cave: &[(i16, i16)] = &[(64, 62)];
        let (tops, at, len, runs, col) = grid(&[(60, &[]), (80, &[(64, 62), (32, 30)]), (80, cave), (80, cave), (80, cave)]);
        let hops = open_runs(5, &tops, &at, &len, &runs, &col, &vec![false; runs.len()], 2);
        // the cave mouth touches the sky beside column 0
        assert_eq!(hops[0], 0);
        // the sealed cave is not connected
        assert_eq!(hops[1], u16::MAX);
        // into the cave: 1 and 2 columns, then the reach ends
        assert_eq!(&hops[2..], &[1, 2, u16::MAX]);
    }

    #[test]
    fn a_run_starting_at_the_top_is_open() {
        // tall grass on top (a non-solid block at the top), and a missing neighbour column
        let (tops, at, len, runs, col) = grid(&[(70, &[(70, 70)]), (i32::MIN, &[])]);
        assert_eq!(open_runs(2, &tops, &at, &len, &runs, &col, &[false], 32), vec![0]);
    }

    #[test]
    fn void_under_an_island_is_open() {
        // an End island: void (air down to the world bottom) under the rock of both columns
        let void: &[(i16, i16)] = &[(39, 0)];
        let (tops, at, len, runs, col) = grid(&[(60, void), (64, void)]);
        assert_eq!(open_runs(2, &tops, &at, &len, &runs, &col, &[true, true], 32), vec![0, 0]);
    }
}
