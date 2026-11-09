//! mapgen: renders Minecraft worlds (any server type) into map-item style web map tiles.
//!
//! Usage:
//!   mapgen render --world <world dir> --out <output dir> [--extra <dir>]... [--threads N] [--force]
//!                 [--client <minecraft client jar>]
//!
//! Incremental: only region files changed since the last run are rendered again.
//! With `--client` (or tables from an earlier run) the map uses the real block textures/colours.
//! Prints a one-line JSON summary to stdout.

mod assets;
mod chunk;
mod colors;
mod level;
mod region;
mod render;
mod tiles;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Never more zoom-out levels than this (1 pixel = 2^8 = 256 blocks at the furthest)
const MAX_LEVELS: u32 = 8;

struct Args {
    world: PathBuf,
    extra: Vec<PathBuf>,
    out: PathBuf,
    client: Option<PathBuf>,
    threads: usize,
    force: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    if it.next().as_deref() != Some("render") {
        return Err("usage: mapgen render --world <dir> --out <dir> [--extra <dir>]... [--threads N] [--force] [--client <jar>]".into());
    }
    let (mut world, mut out, mut extra, mut threads, mut force, mut client) = (None, None, vec![], 2, false, None);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--world" => world = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            "--extra" => extra.extend(it.next().map(PathBuf::from)),
            "--threads" => threads = it.next().and_then(|t| t.parse().ok()).ok_or("--threads needs a number")?,
            "--force" => force = true,
            "--client" => client = it.next().map(PathBuf::from),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        world: world.ok_or("--world is required")?,
        out: out.ok_or("--out is required")?,
        extra,
        client,
        threads: threads.max(1),
        force,
    })
}

struct Dimension {
    id: String,
    slug: String,
    region_dir: PathBuf,
    /// Nether-like: start below the roof
    ceiling: Option<i32>,
}

fn dimension(id: &str, region_dir: PathBuf) -> Dimension {
    let slug = id.strip_prefix("minecraft:").unwrap_or(id).replace([':', '/', '\\'], "_");
    let ceiling = (id == "minecraft:the_nether").then_some(127);
    Dimension { id: id.to_string(), slug, region_dir, ceiling }
}

/// All dimensions with region files: 26.x `dimensions/<ns>/<name>/region`, the classic layout
/// (`region`, `DIM-1`, `DIM1`) and old Paper/Spigot sibling folders passed via --extra.
fn discover(world: &Path, extra: &[PathBuf]) -> Vec<Dimension> {
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    if let Ok(namespaces) = fs::read_dir(world.join("dimensions")) {
        for ns in namespaces.flatten() {
            let Ok(names) = fs::read_dir(ns.path()) else { continue };
            for name in names.flatten() {
                let region = name.path().join("region");
                if region.is_dir() {
                    let id = format!("{}:{}", ns.file_name().to_string_lossy(), name.file_name().to_string_lossy());
                    found.entry(id).or_insert(region);
                }
            }
        }
    }
    let classic = [("minecraft:overworld", "region"), ("minecraft:the_nether", "DIM-1/region"), ("minecraft:the_end", "DIM1/region")];
    for base in std::iter::once(world).chain(extra.iter().map(PathBuf::as_path)) {
        for (id, rel) in classic {
            let dir = base.join(rel);
            if dir.is_dir() {
                found.entry(id.to_string()).or_insert(dir);
            }
        }
    }
    found.into_iter().map(|(id, dir)| dimension(&id, dir)).collect()
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    /// "x,z" -> modification time (ns) of the region file at the last render
    regions: BTreeMap<String, u128>,
    levels: u32,
    /// texture tables the tiles were made with ("" = map-item colours); a change re-renders all
    #[serde(default)]
    assets: String,
}

fn mtime_ns(path: &Path) -> u128 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

fn key(x: i32, z: i32) -> String {
    format!("{x},{z}")
}

fn parse_key(k: &str) -> Option<(i32, i32)> {
    let (x, z) = k.split_once(',')?;
    Some((x.parse().ok()?, z.parse().ok()?))
}

/// Zoom-out levels needed so the whole world fits into a few tiles.
fn levels_for(regions: &BTreeSet<(i32, i32)>) -> u32 {
    let (Some(min_x), Some(max_x)) = (regions.iter().map(|r| r.0).min(), regions.iter().map(|r| r.0).max()) else { return 0 };
    let min_z = regions.iter().map(|r| r.1).min().unwrap_or(0);
    let max_z = regions.iter().map(|r| r.1).max().unwrap_or(0);
    let span = (max_x - min_x + 1).max(max_z - min_z + 1) as u32;
    (u32::BITS - span.saturating_sub(1).leading_zeros()).clamp(1, MAX_LEVELS)
}

#[derive(Serialize)]
struct DimSummary {
    id: String,
    slug: String,
    rendered: usize,
    removed: usize,
    unchanged: usize,
    chunks: usize,
    errors: Vec<String>,
    levels: u32,
    bounds: Option<[i32; 4]>,
}

fn process(dim: &Dimension, out: &Path, force: bool, assets: Option<&assets::Assets>) -> DimSummary {
    let dim_dir = out.join(&dim.slug);
    let state_path = dim_dir.join("state.json");
    let old: State = fs::read(&state_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let assets_stamp = assets.map_or(String::new(), |a| a.stamp().to_string());
    let force = force || old.assets != assets_stamp;

    // Current region files
    let mut current: BTreeMap<(i32, i32), (PathBuf, u128)> = BTreeMap::new();
    if let Ok(entries) = fs::read_dir(&dim.region_dir) {
        for e in entries.flatten() {
            let name = e.file_name();
            if let Some((x, z)) = region::coords_from_name(&name.to_string_lossy()) {
                if e.metadata().map(|m| m.len() > 8192).unwrap_or(false) {
                    current.insert((x, z), (e.path(), mtime_ns(&e.path())));
                }
            }
        }
    }

    let dirty: Vec<(i32, i32)> = current
        .iter()
        // (regions without generated chunks have no tile, so a missing tile is no reason to re-render;
        //  use --force after deleting tiles by hand)
        .filter(|&(&(x, z), (_, mtime))| force || old.regions.get(&key(x, z)) != Some(mtime))
        .map(|(&c, _)| c)
        .collect();
    let removed: Vec<(i32, i32)> = old
        .regions
        .keys()
        .filter_map(|k| parse_key(k))
        .filter(|c| !current.contains_key(c))
        .collect();

    // Render changed regions in parallel
    let results: Vec<(i32, i32, Result<usize, String>)> = dirty
        .par_iter()
        .map(|&(x, z)| {
            let (path, _) = &current[&(x, z)];
            let res = render::render_region(path, x, z, dim.ceiling, assets)
                .and_then(|img| {
                    if img.chunks == 0 {
                        let _ = fs::remove_file(tiles::tile_path(&dim_dir, 0, x, z));
                        let _ = fs::remove_file(tiles::height_path(&dim_dir, x, z));
                        let _ = fs::remove_file(tiles::block_path(&dim_dir, x, z));
                        let _ = fs::remove_file(tiles::voxel_path(&dim_dir, x, z));
                        return Ok(0);
                    }
                    tiles::write_rgba(&tiles::tile_path(&dim_dir, 0, x, z), &img.rgba)?;
                    tiles::write_heights(&tiles::height_path(&dim_dir, x, z), &img.heights)?;
                    match &img.blocks {
                        Some(b) => tiles::write_rgba(&tiles::block_path(&dim_dir, x, z), b)?,
                        None => {
                            let _ = fs::remove_file(tiles::block_path(&dim_dir, x, z));
                        }
                    }
                    match &img.voxels {
                        Some(v) => tiles::write_bytes(&tiles::voxel_path(&dim_dir, x, z), v)?,
                        None => {
                            let _ = fs::remove_file(tiles::voxel_path(&dim_dir, x, z));
                        }
                    }
                    Ok(img.chunks)
                })
                .map_err(|e| format!("r.{x}.{z}.mca: {e}"));
            (x, z, res)
        })
        .collect();

    for &(x, z) in &removed {
        let _ = fs::remove_file(tiles::tile_path(&dim_dir, 0, x, z));
        let _ = fs::remove_file(tiles::height_path(&dim_dir, x, z));
        let _ = fs::remove_file(tiles::block_path(&dim_dir, x, z));
        let _ = fs::remove_file(tiles::voxel_path(&dim_dir, x, z));
    }

    // The top chunk row of the region south of a changed one is shaded against it: patch it
    let dirty_set: HashSet<(i32, i32)> = dirty.iter().copied().collect();
    let edges: Vec<(i32, i32)> = dirty
        .iter()
        .chain(removed.iter())
        .map(|&(x, z)| (x, z + 1))
        .filter(|c| !dirty_set.contains(c) && current.contains_key(c))
        .filter(|&(x, z)| tiles::tile_path(&dim_dir, 0, x, z).exists())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let patched: Vec<(i32, i32)> = edges
        .par_iter()
        .filter_map(|&(x, z)| {
            let tile = tiles::tile_path(&dim_dir, 0, x, z);
            let hpath = tiles::height_path(&dim_dir, x, z);
            let mut rgba = tiles::read_rgba(&tile)?;
            let mut heights = tiles::read_heights(&hpath)?;
            render::patch_north_edge(&current[&(x, z)].0, x, z, dim.ceiling, &mut rgba, &mut heights, assets).ok()?;
            tiles::write_rgba(&tile, &rgba).ok()?;
            tiles::write_heights(&hpath, &heights).ok()?;
            Some((x, z))
        })
        .collect();

    let mut errors = vec![];
    let mut chunks = 0;
    let mut new_state = State { regions: BTreeMap::new(), levels: 0, assets: assets_stamp };
    let mut failed = HashSet::new();
    for (x, z, res) in &results {
        match res {
            Ok(n) => chunks += n,
            Err(e) => {
                errors.push(e.clone());
                failed.insert((*x, *z));
            }
        }
    }
    for (&(x, z), (_, mtime)) in &current {
        if !failed.contains(&(x, z)) {
            new_state.regions.insert(key(x, z), *mtime);
        }
    }

    // Regions with a tile (= at least one generated chunk) define the map
    let present: BTreeSet<(i32, i32)> = current
        .keys()
        .copied()
        .filter(|&(x, z)| tiles::tile_path(&dim_dir, 0, x, z).exists())
        .collect();
    let levels = levels_for(&present);
    new_state.levels = levels;

    // Zoom-out levels: rebuild parents of changed tiles; new levels completely
    let mut changed: BTreeSet<(i32, i32)> = dirty.iter().chain(removed.iter()).chain(patched.iter()).copied().collect();
    let mut all = present.clone();
    for level in 1..=levels {
        let zoom = -(level as i32);
        let parents = |set: &BTreeSet<(i32, i32)>| -> BTreeSet<(i32, i32)> {
            set.iter().map(|&(x, z)| (x.div_euclid(2), z.div_euclid(2))).collect()
        };
        all = parents(&all);
        changed = if level > old.levels || force { all.clone() } else { parents(&changed) };
        changed.par_iter().for_each(|&(x, z)| {
            let child = |dx: i32, dz: i32| tiles::read_rgba(&tiles::tile_path(&dim_dir, zoom + 1, 2 * x + dx, 2 * z + dz));
            let path = tiles::tile_path(&dim_dir, zoom, x, z);
            match tiles::downscale([child(0, 0), child(1, 0), child(0, 1), child(1, 1)]) {
                Some(img) => {
                    let _ = tiles::write_rgba(&path, &img);
                }
                None => {
                    let _ = fs::remove_file(&path);
                }
            }
        });
    }
    // Levels that are no longer needed (world got smaller)
    for level in levels + 1..=old.levels.max(levels) {
        let _ = fs::remove_dir_all(dim_dir.join((-(level as i32)).to_string()));
    }

    let _ = fs::create_dir_all(&dim_dir);
    let _ = fs::write(&state_path, serde_json::to_vec(&new_state).unwrap_or_default());

    let bounds = (!present.is_empty()).then(|| {
        [
            present.iter().map(|r| r.0).min().unwrap(),
            present.iter().map(|r| r.1).min().unwrap(),
            present.iter().map(|r| r.0).max().unwrap(),
            present.iter().map(|r| r.1).max().unwrap(),
        ]
    });
    DimSummary {
        id: dim.id.clone(),
        slug: dim.slug.clone(),
        rendered: results.len() - failed.len(),
        removed: removed.len(),
        unchanged: current.len() - dirty.len(),
        chunks,
        errors,
        levels,
        bounds,
    }
}

#[derive(Serialize)]
struct MapsJson<'a> {
    generated: u64,
    tile_size: usize,
    spawn: Option<level::Spawn>,
    /// texture tables (blocks.json / atlas.png / b/ tiles) exist; the value changes with them
    #[serde(skip_serializing_if = "Option::is_none")]
    blocks: Option<&'a str>,
    dimensions: Vec<MapDim<'a>>,
}

#[derive(Serialize)]
struct MapDim<'a> {
    id: &'a str,
    slug: &'a str,
    /// region bounds [min_x, min_z, max_x, max_z]
    bounds: Option<[i32; 4]>,
    min_zoom: i32,
    /// only when smaller than the vanilla default
    #[serde(skip_serializing_if = "Option::is_none")]
    border: Option<level::Border>,
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let _ = rayon::ThreadPoolBuilder::new().num_threads(args.threads).build_global();
    let start = Instant::now();

    let assets = match assets::Assets::load(args.client.as_deref(), &args.out) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("block textures unavailable, using map colours: {e}");
            None
        }
    };
    let dims = discover(&args.world, &args.extra);
    let summaries: Vec<DimSummary> = dims.iter().map(|d| process(d, &args.out, args.force, assets.as_ref())).collect();

    let maps = MapsJson {
        generated: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        tile_size: render::SIZE,
        spawn: level::spawn(&args.world),
        blocks: assets.as_ref().map(|a| a.stamp()),
        dimensions: dims
            .iter()
            .zip(&summaries)
            .filter(|(_, s)| s.bounds.is_some())
            .map(|(d, s)| MapDim {
                id: &s.id,
                slug: &s.slug,
                bounds: s.bounds,
                min_zoom: -(s.levels as i32),
                border: level::border(&args.world, d.region_dir.parent(), d.id == "minecraft:overworld"),
            })
            .collect(),
    };
    let _ = fs::create_dir_all(&args.out);
    let tmp = args.out.join("maps.json.tmp");
    if fs::write(&tmp, serde_json::to_vec(&maps).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, args.out.join("maps.json"));
    }

    let ok = summaries.iter().all(|s| s.errors.is_empty());
    println!(
        "{}",
        serde_json::json!({ "ok": ok, "ms": start.elapsed().as_millis() as u64, "dimensions": summaries })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_levels() {
        let set = |v: &[(i32, i32)]| v.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(levels_for(&set(&[])), 0);
        assert_eq!(levels_for(&set(&[(0, 0)])), 1);
        assert_eq!(levels_for(&set(&[(-1, 0), (0, 0)])), 1);
        assert_eq!(levels_for(&set(&[(-2, 0), (1, 0)])), 2);
        assert_eq!(levels_for(&set(&[(-100, 0), (100, 0)])), MAX_LEVELS);
    }

    #[test]
    fn parent_tiles_of_negative_regions() {
        // floor division keeps -1 and 0 in different parents
        assert_eq!((-1i32).div_euclid(2), -1);
        assert_eq!(0i32.div_euclid(2), 0);
        assert_eq!((-2i32).div_euclid(2), -1);
    }

    #[test]
    fn slugs() {
        assert_eq!(dimension("minecraft:the_nether", PathBuf::new()).slug, "the_nether");
        assert_eq!(dimension("mymod:sky/islands", PathBuf::new()).slug, "mymod_sky_islands");
        assert_eq!(dimension("minecraft:the_nether", PathBuf::new()).ceiling, Some(127));
        assert_eq!(dimension("minecraft:overworld", PathBuf::new()).ceiling, None);
    }
}
