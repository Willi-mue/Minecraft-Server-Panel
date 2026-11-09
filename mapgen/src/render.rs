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
    /// With texture tables: the 3D block stacks (zlib, see `voxels`)
    pub voxels: Option<Vec<u8>>,
    pub chunks: usize,
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

pub fn render_region(path: &Path, rx: i32, rz: i32, ceiling: Option<i32>, assets: Option<&Assets>) -> std::io::Result<RegionImage> {
    let region = Region::open(path)?;
    let (cols, chunks) = columns(&region, rx, rz, ceiling, None, assets);
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
    let voxels = match assets {
        Some(_) => Some(voxels(&region, path.parent().unwrap_or(Path::new(".")), rx, rz, ceiling, &cols, assets)?),
        None => None,
    };
    Ok(RegionImage { rgba, heights, blocks, voxels, chunks })
}

/// How far (in columns) the stacks follow a cave from where it is open to view.
const CAVE_REACH: u8 = 8;

/// The 3D block stacks of a region: per column every drawn block from the top down to the lowest
/// solid ground of the column and its 4 neighbours (so cliff walls show their real blocks and tree
/// crowns float), and wherever a neighbour shows air (a cave, an overhang) down to the lowest air
/// it shows, so cave walls are real blocks. Format (zlib): "MGV1", u8 count per column (row-major
/// z, x), then the entries in the same order, each i16 y + u16 block id, little endian.
fn voxels(region: &Region, dir: &Path, rx: i32, rz: i32, ceiling: Option<i32>, cols: &[Column], assets: Option<&Assets>) -> std::io::Result<Vec<u8>> {
    // Everything is computed on a grid padded by one chunk on every side with the adjacent chunks
    // of the 4 neighbour regions, so caves crossing a region border get the same walls as the
    // neighbour region exports (no placeholder stripes along the border).
    const PAD: usize = 16;
    const W: usize = SIZE + 2 * PAD;
    const CW: usize = W / 16; // chunks per padded row
    const NO: i32 = i32::MIN; // no column
    let neighbours: Vec<Option<Region>> = [(0, -1), (0, 1), (-1, 0), (1, 0)]
        .iter()
        .map(|&(dx, dz)| Region::open(&dir.join(format!("r.{}.{}.mca", rx + dx, rz + dz))).ok())
        .collect();
    let load = |pcx: usize, pcz: usize| -> Option<Chunk> {
        let (cx, cz) = (pcx as i32 - 1, pcz as i32 - 1);
        let (ox, oz) = (cx.div_euclid(32), cz.div_euclid(32));
        let r = match (ox, oz) {
            (0, 0) => region,
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

    // 1. per column: top, ground and the runs of non-solid blocks (air, plants, water …)
    let mut tops = vec![NO; W * W];
    let mut ground = vec![NO; W * W];
    let mut run_at = vec![0u32; W * W];
    let mut run_len = vec![0u32; W * W];
    let mut runs: Vec<(i16, i16)> = Vec::new();
    for pcz in 0..CW {
        for pcx in 0..CW {
            let chunk = load(pcx, pcz);
            for z in 0..16 {
                for x in 0..16 {
                    let (gx, gz) = (pcx * 16 + x, pcz * 16 + z);
                    let i = gz * W + gx;
                    let Some(chunk) = &chunk else { continue };
                    let c = if own(gx, gz) { cols[(gz - PAD) * SIZE + gx - PAD] } else { chunk.column(x, z, ceiling) };
                    if c.top == NO {
                        continue;
                    }
                    tops[i] = c.top;
                    ground[i] = if c.ground == NO { c.y } else { c.ground };
                    run_at[i] = runs.len() as u32;
                    chunk.air_runs(x, z, c.top, &mut runs);
                    run_len[i] = runs.len() as u32 - run_at[i];
                }
            }
        }
    }
    let runs_of = |i: usize| &runs[run_at[i] as usize..(run_at[i] + run_len[i]) as usize];
    // where the stack of column i ends with stop s (see Chunk::stack): in a run of air it goes on
    // to the solid block below (at most 128 blocks)
    let low_of = |i: usize, s: i32| -> i32 {
        for &(t, b) in runs_of(i) {
            let (t, b) = (t as i32, b as i32);
            if t >= s && s >= b {
                return (b - 1).max(s - 128);
            }
        }
        s
    };
    // the lowest air the stack of column i shows when it ends at `low`
    let need_of = |i: usize, low: i32| -> i32 {
        runs_of(i).iter().filter(|&&(t, _)| t as i32 >= low).map(|&(_, b)| (b as i32).max(low)).min().unwrap_or(i32::MAX)
    };

    // 2. stops: the grounds of the neighbours, then (until nothing changes) the lowest air the
    // neighbours show; this only spreads through air, solid rock stops it
    let mut stop = vec![NO; W * W];
    let mut low = vec![NO; W * W];
    let mut need = vec![i32::MAX; W * W];
    for i in 0..W * W {
        if tops[i] == NO {
            continue;
        }
        stop[i] = nbrs(i).iter().flatten().map(|&n| ground[n]).filter(|&g| g != NO).fold(ground[i], i32::min);
        low[i] = low_of(i, stop[i]);
        need[i] = need_of(i, low[i]);
    }
    // hops from where the air became visible: far inside a cave nobody looks any more
    let mut hops = vec![0u8; W * W];
    let mut work: std::collections::VecDeque<usize> = (0..W * W).filter(|&i| need[i] != i32::MAX).collect();
    while let Some(i) = work.pop_front() {
        for n in nbrs(i).into_iter().flatten() {
            if tops[n] == NO || need[i] >= stop[n] {
                continue;
            }
            stop[n] = need[i];
            low[n] = low_of(n, stop[n]);
            let nn = need_of(n, low[n]);
            if nn < need[n] && hops[i] < CAVE_REACH {
                need[n] = nn;
                hops[n] = hops[i] + 1;
                work.push_back(n);
            }
        }
    }

    // 3. the stacks of the region's own columns
    let mut counts = vec![0u8; SIZE * SIZE];
    let mut spans = vec![0u32; SIZE * SIZE];
    let mut pool: Vec<(i16, u16)> = Vec::new();
    for cz in 0..32 {
        for cx in 0..32 {
            let Some(chunk) = load(cx + 1, cz + 1) else { continue };
            for z in 0..16 {
                for x in 0..16 {
                    let (lx, lz) = (cx * 16 + x, cz * 16 + z);
                    let i = (lz + PAD) * W + lx + PAD;
                    if tops[i] == NO {
                        continue;
                    }
                    spans[lz * SIZE + lx] = pool.len() as u32;
                    counts[lz * SIZE + lx] = chunk.stack(x, z, tops[i], stop[i], &mut pool);
                }
            }
        }
    }
    let mut raw = Vec::with_capacity(4 + SIZE * SIZE + pool.len() * 4);
    raw.extend_from_slice(b"MGV1");
    raw.extend_from_slice(&counts);
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
