//! Small helpers shared by the modules: atomic writes, JSON files, events, time.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Lock a mutex, ignoring poisoning (a panicked thread must not take the panel down).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `path` with `suffix` appended to the file name (`a.json` + `.tmp` -> `a.json.tmp`).
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// `path` with its extension replaced (`a.json` -> `a.tmp`), like Python's `with_suffix`.
pub fn replace_ext(path: &Path, ext: &str) -> PathBuf {
    path.with_extension(ext.trim_start_matches('.'))
}

/// Write via a temporary file and rename, so readers never see half a file.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = replace_ext(path, "tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
    }
    fs::rename(&tmp, path)
}

pub fn read_json(path: &Path) -> Option<Value> {
    let text = fs::read(path).ok()?;
    serde_json::from_slice(strip_bom(&text)).ok()
}

pub fn strip_bom(data: &[u8]) -> &[u8] {
    data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)
}

/// JSON with an indent of `indent` spaces (Python's `json.dumps(indent=n)`).
pub fn to_json_indent(value: &Value, indent: usize) -> String {
    let spaces = vec![b' '; indent];
    let fmt = serde_json::ser::PrettyFormatter::with_indent(&spaces);
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
    serde::Serialize::serialize(value, &mut ser).expect("JSON serialisation");
    String::from_utf8(out).expect("JSON is UTF-8")
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn mtime_secs(path: &Path) -> Option<f64> {
    let m = fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

pub fn mtime_ns(path: &Path) -> Option<u128> {
    let m = fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_nanos())
}

/// Python's `round(x, n)` for display values.
pub fn round_to(x: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (x * f).round() / f
}

/// `round(x, 1)` as a JSON number.
pub fn json_round(x: f64, digits: i32) -> Value {
    serde_json::Number::from_f64(round_to(x, digits))
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// Remove a file or a folder tree; missing is fine.
pub fn remove_path(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// All files below `dir`, sorted by path.
pub fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => walk(&p, out),
                Ok(t) if t.is_file() => out.push(p),
                _ => {}
            }
        }
    }
    walk(dir, &mut out);
    out.sort();
    out
}

/// A resettable flag threads can wait on (Python's `threading.Event`).
#[derive(Default)]
pub struct Event {
    flag: Mutex<bool>,
    cv: Condvar,
}

impl Event {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self) {
        *lock(&self.flag) = true;
        self.cv.notify_all();
    }

    pub fn clear(&self) {
        *lock(&self.flag) = false;
    }

    #[cfg(test)]
    pub fn is_set(&self) -> bool {
        *lock(&self.flag)
    }

    /// True if the flag got set within `timeout`.
    pub fn wait(&self, timeout: Duration) -> bool {
        let guard = lock(&self.flag);
        let (guard, _) = self
            .cv
            .wait_timeout_while(guard, timeout, |set| !*set)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }
}

/// Python's `str.capitalize()` for ASCII words ("paper" -> "Paper").
pub fn capitalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut c = lower.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Local time formatted with a chrono format string.
pub fn local_now(fmt: &str) -> String {
    chrono::Local::now().format(fmt).to_string()
}

/// Seconds since the epoch -> local ISO time without fraction ("2026-11-11T12:34:56").
pub fn iso_local(secs: f64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(secs as i64, 0)
        .single()
        .map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Random URL-safe token (like Python's `secrets.token_urlsafe(n)`).
pub fn token(bytes: usize) -> String {
    use base64::Engine;
    let mut buf = vec![0u8; bytes];
    rand::Rng::fill(&mut rand::rng(), buf.as_mut_slice());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

pub fn hex_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::Rng::fill(&mut rand::rng(), buf.as_mut_slice());
    hex::encode(buf)
}

/// Constant-time comparison of two byte strings.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Log a line to stderr (the panel's own log, not a server console).
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { eprintln!("INFO panel: {}", format!($($arg)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { eprintln!("WARNING panel: {}", format!($($arg)*)) };
}
