//! PNG tiles: writing atomically, reading back, and building zoomed-out levels.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use crate::render::SIZE;

pub fn tile_path(dim_dir: &Path, zoom: i32, x: i32, z: i32) -> PathBuf {
    dim_dir.join(zoom.to_string()).join(format!("{x}_{z}.png"))
}

pub fn height_path(dim_dir: &Path, x: i32, z: i32) -> PathBuf {
    dim_dir.join("h").join(format!("{x}_{z}.png"))
}

/// 3D block stacks of a region (only with texture tables).
pub fn voxel_path(dim_dir: &Path, x: i32, z: i32) -> PathBuf {
    dim_dir.join("v").join(format!("{x}_{z}.bin"))
}

/// Write any file atomically.
pub fn write_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// Block id + biome per column (only with texture tables), for the textured 2D/3D views.
pub fn block_path(dim_dir: &Path, x: i32, z: i32) -> PathBuf {
    dim_dir.join("b").join(format!("{x}_{z}.png"))
}

/// Write to a temp file and rename, so the web server never serves a half-written tile.
fn write_png(path: &Path, bytes: &[u8], color: png::ColorType, depth: png::BitDepth) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("png.tmp");
    {
        let mut enc = png::Encoder::new(BufWriter::new(File::create(&tmp)?), SIZE as u32, SIZE as u32);
        enc.set_color(color);
        enc.set_depth(depth);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(std::io::Error::other)?;
        writer.write_image_data(bytes).map_err(std::io::Error::other)?;
    }
    fs::rename(&tmp, path)
}

pub fn write_rgba(path: &Path, rgba: &[u8]) -> std::io::Result<()> {
    write_png(path, rgba, png::ColorType::Rgba, png::BitDepth::Eight)
}

pub fn write_heights(path: &Path, heights: &[u16]) -> std::io::Result<()> {
    let bytes: Vec<u8> = heights.iter().flat_map(|h| h.to_be_bytes()).collect();
    write_png(path, &bytes, png::ColorType::Grayscale, png::BitDepth::Sixteen)
}

pub fn read_heights(path: &Path) -> Option<Vec<u16>> {
    let decoder = png::Decoder::new(File::open(path).ok()?);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Grayscale || info.bit_depth != png::BitDepth::Sixteen {
        return None;
    }
    Some(buf[..info.buffer_size()].chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect())
}

pub fn read_rgba(path: &Path) -> Option<Vec<u8>> {
    let decoder = png::Decoder::new(File::open(path).ok()?);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    (info.color_type == png::ColorType::Rgba && info.width as usize == SIZE && info.height as usize == SIZE)
        .then(|| { buf.truncate(info.buffer_size()); buf })
}

/// Build a zoomed-out tile from its (up to) four children; None if none of them exists.
/// Each output pixel averages the visible pixels of a 2×2 block.
pub fn downscale(children: [Option<Vec<u8>>; 4]) -> Option<Vec<u8>> {
    if children.iter().all(Option::is_none) {
        return None;
    }
    let half = SIZE / 2;
    let mut out = vec![0u8; SIZE * SIZE * 4];
    for (q, child) in children.iter().enumerate() {
        let Some(child) = child else { continue };
        let (ox, oz) = ((q % 2) * half, (q / 2) * half);
        for z in 0..half {
            for x in 0..half {
                let (mut sum, mut n) = ([0u32; 3], 0u32);
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = ((z * 2 + dz) * SIZE + x * 2 + dx) * 4;
                    if child[p + 3] > 0 {
                        for c in 0..3 {
                            sum[c] += child[p + c] as u32;
                        }
                        n += 1;
                    }
                }
                if n > 0 {
                    let p = ((oz + z) * SIZE + ox + x) * 4;
                    out[p..p + 4].copy_from_slice(&[(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255]);
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_places_children_in_quadrants() {
        let mut red = vec![0u8; SIZE * SIZE * 4];
        for px in red.chunks_mut(4) {
            px.copy_from_slice(&[200, 0, 0, 255]);
        }
        // only the south-east child exists
        let out = downscale([None, None, None, Some(red)]).unwrap();
        let at = |x: usize, z: usize| &out[(z * SIZE + x) * 4..(z * SIZE + x) * 4 + 4];
        assert_eq!(at(0, 0), &[0, 0, 0, 0]);
        assert_eq!(at(SIZE - 1, SIZE - 1), &[200, 0, 0, 255]);
        assert_eq!(at(SIZE / 2, SIZE / 2), &[200, 0, 0, 255]);
        assert!(downscale([None, None, None, None]).is_none());
    }
}
