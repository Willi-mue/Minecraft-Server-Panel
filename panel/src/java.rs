//! Find a Java runtime that matches the Minecraft version's requirement.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};

use regex::Regex;

use crate::config::cfg;
use crate::util::lock;

static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"version "(\d+)(?:\.(\d+))?"#).unwrap());
static CACHE: Mutex<Option<BTreeMap<u32, String>>> = Mutex::new(None);

pub fn parse_major(output: &str) -> Option<u32> {
    let c = VERSION_RE.captures(output)?;
    let major: u32 = c[1].parse().ok()?;
    // Java 8 reports itself as "1.8.0_..."
    match (major, c.get(2)) {
        (1, Some(minor)) => minor.as_str().parse().ok(),
        _ => Some(major),
    }
}

fn major_of(java_bin: &str) -> Option<u32> {
    let mut cmd = Command::new(java_bin);
    cmd.arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let out = cmd.output().ok()?;
    parse_major(&String::from_utf8_lossy(&out.stderr))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or(".EXE;.BAT;.CMD".into())
            .split(';')
            .map(|e| e.to_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Public for `mapgen` discovery too.
pub fn which_bin(name: &str) -> Option<PathBuf> {
    which(name)
}

/// All available runtimes as {major_version: java_binary} (looked up once).
pub fn runtimes() -> BTreeMap<u32, String> {
    if let Some(found) = lock(&CACHE).as_ref() {
        return found.clone();
    }
    let mut found: BTreeMap<u32, String> = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(&cfg().java_dir) {
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for d in dirs {
            for exe in ["java", "java.exe"] {
                let b = d.join("bin").join(exe);
                if b.exists() {
                    let b = b.to_string_lossy().into_owned();
                    if let Some(major) = major_of(&b) {
                        found.entry(major).or_insert(b);
                    }
                }
            }
        }
    }
    if let Some(p) = which("java") {
        let p = p.to_string_lossy().into_owned();
        if let Some(major) = major_of(&p) {
            found.entry(major).or_insert(p);
        }
    }
    *lock(&CACHE) = Some(found.clone());
    found
}

/// Smallest runtime >= required, else the newest available (best effort).
pub fn pick(required: u32) -> Option<(String, u32)> {
    let rts = runtimes();
    if let Some((major, bin)) = rts.iter().find(|(m, _)| **m >= required) {
        return Some((bin.clone(), *major));
    }
    rts.iter().next_back().map(|(m, b)| (b.clone(), *m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_output() {
        assert_eq!(
            parse_major(r#"openjdk version "21.0.5" 2024-10-15"#),
            Some(21)
        );
        assert_eq!(parse_major(r#"java version "1.8.0_402""#), Some(8));
        assert_eq!(parse_major(r#"openjdk version "25" 2025-09-16"#), Some(25));
        assert_eq!(parse_major("nothing"), None);
    }
}
