//! World info the map shows besides the tiles: spawn point (`level.dat`) and world border
//! (26.x: per dimension in `data/minecraft/world_border.dat`, before: `level.dat`).

use std::io::Read;
use std::path::Path;

use fastnbt::IntArray;
use serde::{Deserialize, Serialize};

/// Vanilla default border size; borders at least this large are not drawn.
const DEFAULT_BORDER: f64 = 59_999_968.0;

#[derive(Deserialize)]
struct Level {
    #[serde(rename = "Data")]
    data: LevelData,
}

#[derive(Deserialize)]
struct LevelData {
    /// 26.x: {pos: [I; x, y, z], dimension: "minecraft:overworld", ...}
    spawn: Option<SpawnNbt>,
    /// before 26.x
    #[serde(rename = "SpawnX")]
    spawn_x: Option<i32>,
    #[serde(rename = "SpawnZ")]
    spawn_z: Option<i32>,
    #[serde(rename = "BorderCenterX")]
    border_center_x: Option<f64>,
    #[serde(rename = "BorderCenterZ")]
    border_center_z: Option<f64>,
    #[serde(rename = "BorderSize")]
    border_size: Option<f64>,
}

#[derive(Deserialize)]
struct SpawnNbt {
    pos: Option<IntArray>,
    dimension: Option<String>,
}

/// 26.x `world_border.dat`: {DataVersion, data: {center_x, center_z, size, ...}}
#[derive(Deserialize)]
struct BorderFile {
    data: BorderNbt,
}

#[derive(Deserialize)]
struct BorderNbt {
    center_x: f64,
    center_z: f64,
    size: f64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Spawn {
    pub x: i32,
    pub z: i32,
    pub dimension: String,
}

/// Square border: centre and edge length in blocks.
#[derive(Serialize, Debug, PartialEq)]
pub struct Border {
    pub center_x: f64,
    pub center_z: f64,
    pub size: f64,
}

fn read_gz(path: &Path) -> Option<Vec<u8>> {
    let raw = std::fs::read(path).ok()?;
    let mut nbt = Vec::new();
    flate2::read::GzDecoder::new(&raw[..]).read_to_end(&mut nbt).ok()?;
    Some(nbt)
}

pub fn spawn(world: &Path) -> Option<Spawn> {
    parse_spawn(&read_gz(&world.join("level.dat"))?)
}

/// Border of one dimension. `dim_dir` is the folder holding `region/` (26.x: also `data/`);
/// the `level.dat` fallback (pre-26.x worlds) only applies to the overworld.
pub fn border(world: &Path, dim_dir: Option<&Path>, overworld: bool) -> Option<Border> {
    let file = dim_dir.map(|d| d.join("data").join("minecraft").join("world_border.dat"));
    if let Some(nbt) = file.as_deref().and_then(read_gz) {
        return parse_border_file(&nbt);
    }
    if overworld {
        return parse_level_border(&read_gz(&world.join("level.dat"))?);
    }
    None
}

fn parse_spawn(nbt: &[u8]) -> Option<Spawn> {
    let level: Level = fastnbt::from_bytes(nbt).ok()?;
    let d = level.data;
    if let Some(s) = d.spawn {
        let pos = s.pos?;
        return Some(Spawn {
            x: *pos.first()?,
            z: *pos.get(2)?,
            dimension: s.dimension.unwrap_or_else(|| "minecraft:overworld".into()),
        });
    }
    Some(Spawn { x: d.spawn_x?, z: d.spawn_z?, dimension: "minecraft:overworld".into() })
}

fn limited(center_x: f64, center_z: f64, size: f64) -> Option<Border> {
    (size.is_finite() && size > 0.0 && size < DEFAULT_BORDER).then_some(Border { center_x, center_z, size })
}

fn parse_border_file(nbt: &[u8]) -> Option<Border> {
    let f: BorderFile = fastnbt::from_bytes(nbt).ok()?;
    limited(f.data.center_x, f.data.center_z, f.data.size)
}

fn parse_level_border(nbt: &[u8]) -> Option<Border> {
    let d = fastnbt::from_bytes::<Level>(nbt).ok()?.data;
    limited(d.border_center_x.unwrap_or(0.0), d.border_center_z.unwrap_or(0.0), d.border_size?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_26x_format() {
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({"Data": {
            "spawn": {"pos": fastnbt::IntArray::new(vec![12, 102, -40]), "dimension": "minecraft:overworld", "yaw": 0.0f32}
        }})).unwrap();
        assert_eq!(parse_spawn(&nbt), Some(Spawn { x: 12, z: -40, dimension: "minecraft:overworld".into() }));
    }

    #[test]
    fn spawn_classic_format() {
        let nbt = fastnbt::to_bytes(&fastnbt::nbt!({"Data": {"SpawnX": 5, "SpawnY": 70, "SpawnZ": 7}})).unwrap();
        assert_eq!(parse_spawn(&nbt), Some(Spawn { x: 5, z: 7, dimension: "minecraft:overworld".into() }));
    }

    fn border_file(size: f64) -> Vec<u8> {
        fastnbt::to_bytes(&fastnbt::nbt!({"DataVersion": 5023, "data": {
            "center_x": 100.5f64, "center_z": -20.0f64, "size": size, "lerp_time": 0i64,
            "lerp_target": size, "warning_blocks": 5, "damage_per_block": 0.2f64
        }})).unwrap()
    }

    #[test]
    fn border_26x_format() {
        assert_eq!(parse_border_file(&border_file(2000.0)), Some(Border { center_x: 100.5, center_z: -20.0, size: 2000.0 }));
        // default border: nothing to draw
        assert_eq!(parse_border_file(&border_file(59_999_968.0)), None);
    }

    #[test]
    fn border_classic_format() {
        let nbt = |size: f64| {
            fastnbt::to_bytes(&fastnbt::nbt!({"Data": {"BorderCenterX": 0.0f64, "BorderCenterZ": 8.0f64, "BorderSize": size}})).unwrap()
        };
        assert_eq!(parse_level_border(&nbt(500.0)), Some(Border { center_x: 0.0, center_z: 8.0, size: 500.0 }));
        assert_eq!(parse_level_border(&nbt(60_000_000.0)), None);
        let none = fastnbt::to_bytes(&fastnbt::nbt!({"Data": {"SpawnX": 1}})).unwrap();
        assert_eq!(parse_level_border(&none), None);
    }
}
