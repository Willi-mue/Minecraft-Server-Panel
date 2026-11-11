//! The server list icon (`server-icon.png`, 64×64 PNG in the server folder, read by Minecraft at
//! start). Without one the panel puts its logo there; users can upload their own.

use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::error::Result;
use crate::instances::Instance;
use crate::invalid;

pub const SIZE: u32 = 64;
pub const MAX_BYTES: usize = 256 * 1024;
const FILE: &str = "server-icon.png";
pub const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";

// The logo (app/static/map/logo.svg) on its 16×16 grid: rectangles (x, y, w, h, colour), transparent
// elsewhere. A test checks that both stay the same.
pub const LOGO_RECTS: [(u32, u32, u32, u32, &str); 63] = [
    (1, 1, 14, 1, "#1f2430"),
    (0, 2, 1, 1, "#1f2430"),
    (1, 2, 14, 1, "#2b2457"),
    (15, 2, 1, 1, "#1f2430"),
    (0, 3, 1, 1, "#1f2430"),
    (1, 3, 14, 1, "#2b2457"),
    (15, 3, 1, 1, "#1f2430"),
    (0, 4, 1, 1, "#1f2430"),
    (1, 4, 3, 1, "#5a3a85"),
    (4, 4, 1, 1, "#1b2f2a"),
    (5, 4, 10, 1, "#5a3a85"),
    (15, 4, 1, 1, "#1f2430"),
    (0, 5, 1, 1, "#1f2430"),
    (1, 5, 3, 1, "#b04f86"),
    (4, 5, 1, 1, "#1b2f2a"),
    (5, 5, 4, 1, "#b04f86"),
    (9, 5, 3, 1, "#ffd98a"),
    (12, 5, 3, 1, "#b04f86"),
    (15, 5, 1, 1, "#1f2430"),
    (0, 6, 1, 1, "#1f2430"),
    (1, 6, 2, 1, "#f08a6a"),
    (3, 6, 3, 1, "#1b2f2a"),
    (6, 6, 2, 1, "#f08a6a"),
    (8, 6, 5, 1, "#ffd98a"),
    (13, 6, 2, 1, "#f08a6a"),
    (15, 6, 1, 1, "#1f2430"),
    (0, 7, 1, 1, "#1f2430"),
    (1, 7, 1, 1, "#f08a6a"),
    (2, 7, 1, 1, "#5b3f78"),
    (3, 7, 3, 1, "#1b2f2a"),
    (6, 7, 2, 1, "#f08a6a"),
    (8, 7, 3, 1, "#ffd98a"),
    (11, 7, 2, 1, "#5b3f78"),
    (13, 7, 2, 1, "#f08a6a"),
    (15, 7, 1, 1, "#1f2430"),
    (0, 8, 1, 1, "#1f2430"),
    (1, 8, 3, 1, "#5b3f78"),
    (4, 8, 1, 1, "#1b2f2a"),
    (5, 8, 2, 1, "#5b3f78"),
    (7, 8, 1, 1, "#f08a6a"),
    (8, 8, 1, 1, "#ffd98a"),
    (9, 8, 6, 1, "#5b3f78"),
    (15, 8, 1, 1, "#1f2430"),
    (0, 9, 1, 1, "#1f2430"),
    (1, 9, 1, 1, "#5b3f78"),
    (2, 9, 3, 1, "#3f7a52"),
    (5, 9, 5, 1, "#5b3f78"),
    (10, 9, 3, 1, "#3f7a52"),
    (13, 9, 2, 1, "#5b3f78"),
    (15, 9, 1, 1, "#1f2430"),
    (0, 10, 1, 1, "#1f2430"),
    (1, 10, 1, 1, "#3f7a52"),
    (2, 10, 3, 1, "#2b5a3e"),
    (5, 10, 5, 1, "#3f7a52"),
    (10, 10, 3, 1, "#2b5a3e"),
    (13, 10, 2, 1, "#3f7a52"),
    (15, 10, 1, 1, "#1f2430"),
    (1, 11, 12, 1, "#1f2430"),
    (13, 11, 1, 1, "#5cf08a"),
    (14, 11, 1, 1, "#1f2430"),
    (6, 12, 4, 1, "#2b3140"),
    (6, 13, 4, 1, "#2b3140"),
    (4, 14, 8, 1, "#1f2430"),
];

fn rgb(hex: &str) -> [u8; 3] {
    let c = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
    [c(1), c(3), c(5)]
}

/// Encode RGBA pixels as PNG (filter 0, one IDAT).
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut h = crc32fast::Hasher::new();
        h.update(kind);
        h.update(data);
        out.extend_from_slice(&h.finalize().to_be_bytes());
    }
    let mut rows = Vec::with_capacity((width * 4 + 1) as usize * height as usize);
    for y in 0..height as usize {
        rows.push(0);
        rows.extend_from_slice(&rgba[y * width as usize * 4..(y + 1) * width as usize * 4]);
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
    z.write_all(&rows).expect("zlib into memory");
    let idat = z.finish().expect("zlib into memory");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = PNG_SIG.to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", b"");
    out
}

/// The logo as a 64×64 PNG (4 pixels per grid unit, transparent outside its rectangles).
pub fn default_png() -> &'static [u8] {
    static PNG: LazyLock<Vec<u8>> = LazyLock::new(|| {
        let scale = SIZE / 16;
        let mut px = vec![0u8; (SIZE * SIZE * 4) as usize];
        for (rx, ry, rw, rh, c) in LOGO_RECTS {
            for y in ry * scale..(ry + rh) * scale {
                for x in rx * scale..(rx + rw) * scale {
                    let i = ((y * SIZE + x) * 4) as usize;
                    px[i..i + 3].copy_from_slice(&rgb(c));
                    px[i + 3] = 255;
                }
            }
        }
        encode_png(SIZE, SIZE, &px)
    });
    &PNG
}

pub fn path(inst: &Instance) -> PathBuf {
    inst.server_dir().join(FILE)
}

/// A PNG of 64×64 pixels, not too big.
pub fn validate(data: &[u8]) -> Result<()> {
    if data.len() > MAX_BYTES {
        return invalid!("The icon is too big (at most 256 KB)");
    }
    if data.len() < 33 || !data.starts_with(PNG_SIG) || &data[12..16] != b"IHDR" {
        return invalid!("The icon must be a PNG image");
    }
    let width = u32::from_be_bytes(data[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(data[20..24].try_into().unwrap());
    if (width, height) != (SIZE, SIZE) {
        return invalid!("The icon must be {SIZE}×{SIZE} pixels (this one is {width}×{height})");
    }
    Ok(())
}

/// Store an uploaded icon (Minecraft reads it on the next start).
pub fn save(inst: &Instance, data: &[u8]) -> Result<()> {
    validate(data)?;
    crate::util::write_atomic(&path(inst), data)?;
    Ok(())
}

/// Back to the panel logo.
pub fn reset(inst: &Instance) -> Result<()> {
    save(inst, default_png())
}

/// Put the panel logo in place when the server has no icon yet (called before each start).
pub fn ensure(inst: &Instance) -> Result<()> {
    if !path(inst).exists() {
        reset(inst)?;
    }
    Ok(())
}

/// The icon the server shows (the default one when there is none yet).
pub fn read(inst: &Instance) -> Vec<u8> {
    std::fs::read(path(inst)).unwrap_or_else(|_| default_png().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn logo_png_matches_the_svg() {
        let svg =
            std::fs::read_to_string(crate::config::cfg().static_dir.join("map").join("logo.svg"))
                .unwrap();
        let re = regex::Regex::new(
            r##"<rect x="(\d+)" y="(\d+)" width="(\d+)" height="(\d+)" fill="(#[0-9a-f]{6})"/>"##,
        )
        .unwrap();
        let rects: Vec<(u32, u32, u32, u32, String)> = re
            .captures_iter(&svg)
            .map(|c| {
                (
                    c[1].parse().unwrap(),
                    c[2].parse().unwrap(),
                    c[3].parse().unwrap(),
                    c[4].parse().unwrap(),
                    c[5].to_string(),
                )
            })
            .collect();
        let ours: Vec<(u32, u32, u32, u32, String)> = LOGO_RECTS
            .iter()
            .map(|r| (r.0, r.1, r.2, r.3, r.4.to_string()))
            .collect();
        assert_eq!(rects, ours);
    }

    #[test]
    fn default_icon_is_a_64px_png_with_the_logo() {
        let png = default_png();
        validate(png).unwrap();
        let idat = png.windows(4).position(|w| w == b"IDAT").unwrap();
        let len = u32::from_be_bytes(png[idat - 4..idat].try_into().unwrap()) as usize;
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&png[idat + 4..idat + 4 + len])
            .read_to_end(&mut raw)
            .unwrap();
        let at = |x: usize, y: usize| {
            let i = y * (64 * 4 + 1) + 1 + x * 4;
            (raw[i], raw[i + 1], raw[i + 2], raw[i + 3])
        };
        assert_eq!(at(0, 0).3, 0); // transparent above the monitor
        assert_eq!(at(2, 10), (0x1f, 0x24, 0x30, 255)); // bezel
        assert_eq!(at(10, 10), (0x2b, 0x24, 0x57, 255)); // sky
        assert_eq!(at(42, 26), (0xff, 0xd9, 0x8a, 255)); // sun
        assert_eq!(at(53, 46), (0x5c, 0xf0, 0x8a, 255)); // power LED
    }

    #[test]
    fn upload_checks() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        assert_eq!(read(&inst), default_png());
        assert!(save(&inst, b"not a png").is_err());
        let small = encode_png(32, 32, &vec![0u8; 32 * 32 * 4]);
        assert!(save(&inst, &small).is_err());
        let ok = encode_png(64, 64, &vec![255u8; 64 * 64 * 4]);
        save(&inst, &ok).unwrap();
        assert_eq!(read(&inst), ok);
        ensure(&inst).unwrap();
        assert_eq!(read(&inst), ok, "ensure keeps an uploaded icon");
        reset(&inst).unwrap();
        assert_eq!(read(&inst), default_png());
    }
}
