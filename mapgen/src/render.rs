//! Render one region (512×512 blocks) into map-item style pixels plus a height map.

use std::path::Path;

use crate::assets::Assets;
use crate::chunk::{Chunk, Column, EMPTY};
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
    let (voxels, caves) = match assets {
        Some(_) => {
            let (v, c) = voxels(&parsed, path.parent().unwrap_or(Path::new(".")), rx, rz, ceiling, &cols, assets, vis)?;
            (Some(v), c)
        }
        None => (None, None),
    };
    Ok(RegionImage { rgba, heights, blocks, plants, voxels, caves, chunks })
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
fn voxels(own_chunks: &[Option<Chunk>], dir: &Path, rx: i32, rz: i32, ceiling: Option<i32>, cols: &[Column], assets: Option<&Assets>, vis: &Visibility) -> std::io::Result<(Vec<u8>, Option<Vec<u8>>)> {
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
    let caves = if sets.len() > 1 { Some(encode(1)?) } else { None };
    Ok((encode(0)?, caves))
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
