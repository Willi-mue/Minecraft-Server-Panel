//! Region files (`r.<x>.<z>.mca`): 32×32 chunks, each stored compressed behind a 4 KiB header.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct Region {
    data: Vec<u8>,
    dir: PathBuf,
}

impl Region {
    pub fn open(path: &Path) -> std::io::Result<Region> {
        Ok(Region {
            data: fs::read(path)?,
            dir: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        })
    }

    /// Decompressed NBT of the chunk at local position (0..32, 0..32), if present.
    /// Errors in single chunks are reported as `None` so one bad chunk can't break a region.
    pub fn chunk(&self, cx: usize, cz: usize, abs_cx: i32, abs_cz: i32) -> Option<Vec<u8>> {
        let i = (cz * 32 + cx) * 4;
        let header = self.data.get(i..i + 4)?;
        let offset = (u32::from_be_bytes([0, header[0], header[1], header[2]]) as usize) * 4096;
        if offset == 0 || header[3] == 0 {
            return None;
        }
        let head = self.data.get(offset..offset + 5)?;
        let length = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let compression = head[4];
        if length == 0 {
            return None;
        }
        let external;
        let payload: &[u8] = if compression & 0x80 != 0 {
            // Oversized chunk stored next to the region file as c.<x>.<z>.mcc
            external = fs::read(self.dir.join(format!("c.{abs_cx}.{abs_cz}.mcc"))).ok()?;
            &external
        } else {
            self.data.get(offset + 5..offset + 4 + length)?
        };
        decompress(compression & 0x7f, payload)
    }
}

fn decompress(kind: u8, payload: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() * 4);
    match kind {
        1 => flate2::read::GzDecoder::new(payload).read_to_end(&mut out).ok()?,
        2 => flate2::read::ZlibDecoder::new(payload).read_to_end(&mut out).ok()?,
        3 => return Some(payload.to_vec()),
        4 => return lz4_java_block(payload),
        _ => return None,
    };
    Some(out)
}

/// Minecraft's optional LZ4 compression ("region-file-compression=lz4") uses the
/// LZ4 Java "block stream" format: blocks of [magic "LZ4Block", token, compressed len,
/// decompressed len, checksum] + data.
fn lz4_java_block(mut input: &[u8]) -> Option<Vec<u8>> {
    const MAGIC: &[u8] = b"LZ4Block";
    let mut out = Vec::new();
    while input.len() >= 21 {
        if &input[..8] != MAGIC {
            return None;
        }
        let token = input[8];
        let comp_len = u32::from_le_bytes(input[9..13].try_into().ok()?) as usize;
        let decomp_len = u32::from_le_bytes(input[13..17].try_into().ok()?) as usize;
        let body = input.get(21..21 + comp_len)?;
        match token & 0xF0 {
            0x10 => out.extend_from_slice(body),                                  // raw
            0x20 => out.extend(lz4_flex::block::decompress(body, decomp_len).ok()?), // lz4
            _ => return None,
        }
        input = &input[21 + comp_len..];
        if decomp_len == 0 {
            break; // end marker
        }
    }
    Some(out)
}

/// Parse "r.<x>.<z>.mca" into region coordinates.
pub fn coords_from_name(name: &str) -> Option<(i32, i32)> {
    let mut parts = name.strip_prefix("r.")?.strip_suffix(".mca")?.split('.');
    let x = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((x, z))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_names() {
        assert_eq!(coords_from_name("r.0.0.mca"), Some((0, 0)));
        assert_eq!(coords_from_name("r.-3.12.mca"), Some((-3, 12)));
        assert_eq!(coords_from_name("r.1.2.mcc"), None);
        assert_eq!(coords_from_name("r.1.2.3.mca"), None);
    }

    #[test]
    fn lz4_raw_block() {
        let mut data = b"LZ4Block".to_vec();
        data.push(0x10);
        data.extend(3u32.to_le_bytes());
        data.extend(3u32.to_le_bytes());
        data.extend(0u32.to_le_bytes());
        data.extend(b"abc");
        assert_eq!(lz4_java_block(&data).unwrap(), b"abc");
    }
}
