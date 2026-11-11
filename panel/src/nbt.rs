//! Typed NBT reader/writer that round-trips byte for byte (used to read and edit level.dat,
//! player data and game rules). Compounds keep their order.

use std::io::Read;
use std::path::Path;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

pub const END: u8 = 0;
pub const BYTE: u8 = 1;
pub const SHORT: u8 = 2;
pub const INT: u8 = 3;
pub const LONG: u8 = 4;
pub const FLOAT: u8 = 5;
pub const DOUBLE: u8 = 6;
pub const BYTES: u8 = 7;
pub const STRING: u8 = 8;
pub const LIST: u8 = 9;
pub const COMPOUND: u8 = 10;
pub const INTS: u8 = 11;
pub const LONGS: u8 = 12;

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Bytes(Vec<u8>),
    String(String),
    /// item type + payloads (an empty list may have item type END)
    List(u8, Vec<Tag>),
    Compound(Compound),
    Ints(Vec<i32>),
    Longs(Vec<i64>),
}

/// Ordered name -> tag map.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Compound(pub Vec<(String, Tag)>);

impl Compound {
    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Tag> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Set a value, keeping the position of an existing key.
    pub fn insert(&mut self, key: &str, value: Tag) {
        match self.get_mut(key) {
            Some(slot) => *slot = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    pub fn compound(&self, key: &str) -> Option<&Compound> {
        match self.get(key) {
            Some(Tag::Compound(c)) => Some(c),
            _ => None,
        }
    }

    /// The compound at `key`, created (or replaced) when missing or of another type.
    pub fn compound_mut(&mut self, key: &str) -> &mut Compound {
        if !matches!(self.get(key), Some(Tag::Compound(_))) {
            self.insert(key, Tag::Compound(Compound::default()));
        }
        match self.get_mut(key) {
            Some(Tag::Compound(c)) => c,
            _ => unreachable!(),
        }
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Tag::String(s)) => Some(s),
            _ => None,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &(String, Tag)> {
        self.0.iter()
    }
}

impl Tag {
    pub fn type_id(&self) -> u8 {
        match self {
            Tag::Byte(_) => BYTE,
            Tag::Short(_) => SHORT,
            Tag::Int(_) => INT,
            Tag::Long(_) => LONG,
            Tag::Float(_) => FLOAT,
            Tag::Double(_) => DOUBLE,
            Tag::Bytes(_) => BYTES,
            Tag::String(_) => STRING,
            Tag::List(..) => LIST,
            Tag::Compound(_) => COMPOUND,
            Tag::Ints(_) => INTS,
            Tag::Longs(_) => LONGS,
        }
    }

    /// Any number as f64.
    pub fn as_f64(&self) -> Option<f64> {
        Some(match self {
            Tag::Byte(v) => *v as f64,
            Tag::Short(v) => *v as f64,
            Tag::Int(v) => *v as f64,
            Tag::Long(v) => *v as f64,
            Tag::Float(v) => *v as f64,
            Tag::Double(v) => *v,
            _ => return None,
        })
    }

    /// Any integer as i64.
    pub fn as_i64(&self) -> Option<i64> {
        Some(match self {
            Tag::Byte(v) => *v as i64,
            Tag::Short(v) => *v as i64,
            Tag::Int(v) => *v as i64,
            Tag::Long(v) => *v,
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub struct NbtError(pub String);

impl std::fmt::Display for NbtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<NbtError> for crate::error::Error {
    fn from(e: NbtError) -> Self {
        crate::error::Error::Invalid(e.0)
    }
}

type R<T> = Result<T, NbtError>;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(NbtError("truncated NBT".into()));
        }
        let chunk = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(chunk)
    }

    fn arr<const N: usize>(&mut self) -> R<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    fn i8(&mut self) -> R<i8> {
        Ok(i8::from_be_bytes(self.arr()?))
    }

    fn i32(&mut self) -> R<i32> {
        Ok(i32::from_be_bytes(self.arr()?))
    }

    fn len(&mut self) -> R<usize> {
        Ok(self.i32()?.max(0) as usize)
    }

    fn string(&mut self) -> R<String> {
        let n = u16::from_be_bytes(self.arr()?) as usize;
        Ok(mutf8_decode(self.take(n)?))
    }

    fn payload(&mut self, t: u8, depth: usize) -> R<Tag> {
        if depth > 512 {
            return Err(NbtError("NBT nested too deeply".into()));
        }
        Ok(match t {
            BYTE => Tag::Byte(self.i8()?),
            SHORT => Tag::Short(i16::from_be_bytes(self.arr()?)),
            INT => Tag::Int(self.i32()?),
            LONG => Tag::Long(i64::from_be_bytes(self.arr()?)),
            FLOAT => Tag::Float(f32::from_be_bytes(self.arr()?)),
            DOUBLE => Tag::Double(f64::from_be_bytes(self.arr()?)),
            BYTES => {
                let n = self.len()?;
                Tag::Bytes(self.take(n)?.to_vec())
            }
            STRING => Tag::String(self.string()?),
            LIST => {
                let item = self.i8()? as u8;
                let n = self.len()?;
                if n > self.data.len() {
                    return Err(NbtError("truncated NBT".into()));
                }
                let mut items = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    items.push(self.payload(item, depth + 1)?);
                }
                Tag::List(item, items)
            }
            COMPOUND => {
                let mut c = Compound::default();
                loop {
                    let child = self.i8()? as u8;
                    if child == END {
                        break;
                    }
                    let name = self.string()?;
                    let value = self.payload(child, depth + 1)?;
                    c.0.push((name, value));
                }
                Tag::Compound(c)
            }
            INTS => {
                let n = self.len()?;
                let raw = self.take(
                    n.checked_mul(4)
                        .ok_or_else(|| NbtError("truncated NBT".into()))?,
                )?;
                Tag::Ints(
                    raw.chunks_exact(4)
                        .map(|c| i32::from_be_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            LONGS => {
                let n = self.len()?;
                let raw = self.take(
                    n.checked_mul(8)
                        .ok_or_else(|| NbtError("truncated NBT".into()))?,
                )?;
                Tag::Longs(
                    raw.chunks_exact(8)
                        .map(|c| i64::from_be_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            other => return Err(NbtError(format!("unknown NBT tag {other}"))),
        })
    }
}

/// Java's "modified UTF-8": NUL as C0 80, characters above U+FFFF as two 3-byte surrogates.
fn mutf8_encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if cp == 0 {
            out.extend_from_slice(&[0xC0, 0x80]);
        } else if cp >= 0x10000 {
            let v = cp - 0x10000;
            for unit in [0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF)] {
                out.extend_from_slice(&[
                    0xE0 | (unit >> 12) as u8,
                    0x80 | ((unit >> 6) & 0x3F) as u8,
                    0x80 | (unit & 0x3F) as u8,
                ]);
            }
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

fn mutf8_decode(raw: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(raw) {
        if !s.contains(' ') {
            return s.to_string();
        }
    }
    // UTF-16 units first, then join surrogate pairs
    let mut units: Vec<u16> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        let (unit, n) = if b < 0x80 {
            (b as u32, 1)
        } else if b & 0xE0 == 0xC0 && i + 1 < raw.len() {
            ((((b & 0x1F) as u32) << 6) | (raw[i + 1] & 0x3F) as u32, 2)
        } else if b & 0xF0 == 0xE0 && i + 2 < raw.len() {
            (
                (((b & 0x0F) as u32) << 12)
                    | (((raw[i + 1] & 0x3F) as u32) << 6)
                    | (raw[i + 2] & 0x3F) as u32,
                3,
            )
        } else if b & 0xF8 == 0xF0 && i + 3 < raw.len() {
            // plain 4-byte UTF-8 (not written by Java, but accept it)
            let cp = (((b & 0x07) as u32) << 18)
                | (((raw[i + 1] & 0x3F) as u32) << 12)
                | (((raw[i + 2] & 0x3F) as u32) << 6)
                | (raw[i + 3] & 0x3F) as u32;
            let mut buf = [0u16; 2];
            if let Some(c) = char::from_u32(cp) {
                units.extend_from_slice(c.encode_utf16(&mut buf));
            }
            i += 4;
            continue;
        } else {
            (0xFFFD, 1)
        };
        units.push(unit as u16);
        i += n;
    }
    String::from_utf16_lossy(&units)
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    let raw = mutf8_encode(s);
    out.extend_from_slice(&(raw.len() as u16).to_be_bytes());
    out.extend_from_slice(&raw);
}

fn write_payload(out: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Bytes(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.extend_from_slice(v);
        }
        Tag::String(s) => write_string(out, s),
        Tag::List(item, values) => {
            out.push(*item);
            out.extend_from_slice(&(values.len() as i32).to_be_bytes());
            for v in values {
                write_payload(out, v);
            }
        }
        Tag::Compound(c) => {
            for (name, v) in &c.0 {
                out.push(v.type_id());
                write_string(out, name);
                write_payload(out, v);
            }
            out.push(END);
        }
        Tag::Ints(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        Tag::Longs(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
}

/// Uncompressed NBT -> (root name, root compound).
pub fn decode(data: &[u8]) -> R<(String, Compound)> {
    let mut r = Reader { data, pos: 0 };
    if r.i8()? as u8 != COMPOUND {
        return Err(NbtError("NBT root is not a compound".into()));
    }
    let name = r.string()?;
    match r.payload(COMPOUND, 0)? {
        Tag::Compound(c) => Ok((name, c)),
        _ => unreachable!(),
    }
}

pub fn encode(name: &str, root: &Compound) -> Vec<u8> {
    let mut out = vec![COMPOUND];
    write_string(&mut out, name);
    write_payload(&mut out, &Tag::Compound(root.clone()));
    out
}

pub fn gunzip(data: &[u8]) -> R<Vec<u8>> {
    let mut out = Vec::new();
    GzDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|e| NbtError(format!("not gzip: {e}")))?;
    Ok(out)
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).expect("gzip into memory");
    enc.finish().expect("gzip into memory")
}

/// gzip NBT file -> (root name, root).
pub fn load(path: &Path) -> crate::error::Result<(String, Compound)> {
    let raw = std::fs::read(path)?;
    Ok(decode(&gunzip(&raw)?)?)
}

/// Write gzip NBT atomically; the previous file is kept as `<name>_old` (like Minecraft does).
pub fn save(path: &Path, name: &str, root: &Compound) -> std::io::Result<()> {
    let data = gzip(&encode(name, root));
    let tmp = crate::util::with_suffix(path, ".tmp");
    std::fs::write(&tmp, data)?;
    if path.exists() {
        std::fs::copy(path, crate::util::with_suffix(path, "_old"))?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A small level.dat-like compound with every tag type.
    pub fn sample() -> Compound {
        let mut data = Compound::default();
        data.insert(
            "Version",
            Tag::Compound(Compound(vec![(
                "Name".into(),
                Tag::String("1.21.4".into()),
            )])),
        );
        data.insert("b", Tag::Byte(-3));
        data.insert("s", Tag::Short(300));
        data.insert("i", Tag::Int(-70000));
        data.insert("l", Tag::Long(1 << 40));
        data.insert("f", Tag::Float(1.5));
        data.insert("d", Tag::Double(-2.25));
        data.insert("ba", Tag::Bytes(vec![1, 2, 3]));
        data.insert("ia", Tag::Ints(vec![1, -2, 3]));
        data.insert("la", Tag::Longs(vec![5, -6]));
        data.insert("empty", Tag::List(END, vec![]));
        data.insert(
            "strs",
            Tag::List(
                STRING,
                vec![Tag::String("vanilla".into()), Tag::String("ä€".into())],
            ),
        );
        Compound(vec![("Data".into(), Tag::Compound(data))])
    }

    #[test]
    fn round_trip_is_byte_exact() {
        let bytes = encode("", &sample());
        let (name, root) = decode(&bytes).unwrap();
        assert_eq!(name, "");
        assert_eq!(root, sample());
        assert_eq!(encode(&name, &root), bytes);
    }

    #[test]
    fn java_modified_utf8() {
        let s = "a b😀";
        let raw = mutf8_encode(s);
        assert_eq!(raw.len(), 1 + 2 + 1 + 6);
        assert_eq!(mutf8_decode(&raw), s);
        assert_eq!(mutf8_decode("plain ä".as_bytes()), "plain ä");
    }

    #[test]
    fn truncated_and_wrong_root() {
        let bytes = encode("x", &sample());
        assert!(decode(&bytes[..bytes.len() - 3]).is_err());
        assert!(decode(&[1, 0, 0, 5]).is_err());
    }

    #[test]
    fn save_keeps_old_copy() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("level.dat");
        save(&p, "", &sample()).unwrap();
        assert!(!dir.path().join("level.dat_old").exists());
        save(&p, "", &Compound::default()).unwrap();
        assert!(dir.path().join("level.dat_old").exists());
        assert_eq!(load(&p).unwrap().1, Compound::default());
    }
}
