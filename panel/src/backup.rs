//! World backups as timestamped zip files with retention, and restore. Every function takes the
//! server instance whose world/backups it works on.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::config::cfg;
use crate::error::{Error, Result};
use crate::instances::Instance;
use crate::invalid;
use crate::util::{iso_local, local_now, mtime_secs, remove_path, round_to};

// Region/entity files are already compressed by Minecraft: storing them uncompressed saves a lot of
// CPU during (live) backups at almost no size cost.
const PRECOMPRESSED: [&str; 5] = ["mca", "mcc", "gz", "zip", "png"];
// Server folders a restore must never overwrite, even if a manipulated zip contains them
const NOT_A_WORLD: [&str; 7] = [
    "plugins",
    "cache",
    "libraries",
    "versions",
    "logs",
    "config",
    "crash-reports",
];
static TOP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\w\-. ]{1,64}$").unwrap());
const JUNK: [&str; 3] = ["__MACOSX/", ".DS_Store", "Thumbs.db"];

pub fn level_name(inst: &Instance) -> String {
    let props = inst.server_dir().join("server.properties");
    if let Ok(raw) = fs::read(&props) {
        for line in String::from_utf8_lossy(&raw).lines() {
            if let Some(v) = line.strip_prefix("level-name=") {
                let v = v.trim();
                return if v.is_empty() {
                    "world".into()
                } else {
                    v.to_string()
                };
            }
        }
    }
    "world".into()
}

pub fn world_dirs(inst: &Instance) -> Vec<PathBuf> {
    let name = level_name(inst);
    // Vanilla keeps nether/end inside the level dir; Spigot/Paper split them out.
    [
        name.clone(),
        format!("{name}_nether"),
        format!("{name}_the_end"),
    ]
    .iter()
    .map(|c| inst.server_dir().join(c))
    .filter(|p| p.is_dir())
    .collect()
}

fn stamp() -> String {
    local_now("%Y-%m-%d_%H-%M-%S")
}

pub fn create_backup(inst: &Instance, suffix: &str, protect: Option<&Path>) -> Result<PathBuf> {
    let name = format!(
        "backup_{}{}.zip",
        stamp(),
        if suffix.is_empty() {
            String::new()
        } else {
            format!("_{suffix}")
        }
    );
    let target = inst.backup_dir().join(name);
    zip_world(inst, &target)?;
    cleanup(inst, protect);
    Ok(target)
}

/// Permanent copy of the world (e.g. before a new world is created). Archives are named archive_*
/// and therefore never touched by cleanup().
pub fn create_archive(inst: &Instance, label: &str) -> Result<PathBuf> {
    static UNSAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w.\-]").unwrap());
    let safe: String = UNSAFE.replace_all(label, "-").chars().take(40).collect();
    let name = format!(
        "archive_{}{}.zip",
        stamp(),
        if safe.is_empty() {
            String::new()
        } else {
            format!("_{safe}")
        }
    );
    let target = inst.backup_dir().join(name);
    zip_world(inst, &target)?;
    Ok(target)
}

fn options_for(
    name: &str,
    size: u64,
    modified: Option<std::time::SystemTime>,
) -> SimpleFileOptions {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut opts = if PRECOMPRESSED.contains(&ext.as_str()) {
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
    } else {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(6))
    };
    if size >= 0xFFFF_FFFF {
        opts = opts.large_file(true);
    }
    if let Some(t) = modified.and_then(zip_time) {
        opts = opts.last_modified_time(t);
    }
    opts
}

fn zip_time(t: std::time::SystemTime) -> Option<zip::DateTime> {
    use chrono::{Datelike, Timelike};
    let local: chrono::DateTime<chrono::Local> = t.into();
    zip::DateTime::from_date_and_time(
        local.year().clamp(1980, 2107) as u16,
        local.month() as u8,
        local.day() as u8,
        local.hour() as u8,
        local.minute() as u8,
        local.second().min(59) as u8,
    )
    .ok()
}

fn zip_world(inst: &Instance, target: &Path) -> Result<()> {
    let dirs = world_dirs(inst);
    if dirs.is_empty() {
        return Err(Error::NotFound(
            "No world found to back up (has the server run yet?)".into(),
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("part");
    let result = (|| -> Result<()> {
        let mut zw = ZipWriter::new(BufWriter::new(File::create(&tmp)?));
        let base = inst.server_dir();
        for d in &dirs {
            for file in crate::util::walk_files(d) {
                // session.lock is held open by the running server
                if file.file_name().is_some_and(|n| n == "session.lock") {
                    continue;
                }
                let rel = file
                    .strip_prefix(&base)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .replace('\\', "/");
                let meta = match fs::metadata(&file) {
                    Ok(m) => m,
                    Err(_) => continue, // removed meanwhile (live backup)
                };
                let mut src = match File::open(&file) {
                    Ok(f) => f,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                zw.start_file(
                    rel.as_str(),
                    options_for(&rel, meta.len(), meta.modified().ok()),
                )?;
                io::copy(&mut src, &mut zw)?;
            }
        }
        zw.finish()?;
        fs::rename(&tmp, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn note(name: &str) -> Option<String> {
    static BEFORE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"_before-(vanilla|paper|fabric)$").unwrap());
    let stem = name.strip_suffix(".zip").unwrap_or(name);
    if name.starts_with("archive_") {
        return Some("archive · kept forever".into());
    }
    if stem.ends_with("_before-restore") {
        return Some("before restore".into());
    }
    if let Some(c) = BEFORE.captures(stem) {
        return Some(format!(
            "before switching to {}",
            crate::util::capitalize(&c[1])
        ));
    }
    if stem.ends_with("_upload") {
        return Some("uploaded".into());
    }
    None
}

/// Files in the backup folder whose name starts with `prefix` and ends with `.zip`.
fn glob(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return vec![];
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix) && n.ends_with(".zip"))
        })
        .collect()
}

pub fn list_backups(inst: &Instance) -> Value {
    let dir = inst.backup_dir();
    let mut files: Vec<(f64, PathBuf)> = glob(&dir, "backup_")
        .into_iter()
        .chain(glob(&dir, "archive_"))
        .map(|p| (mtime_secs(&p).unwrap_or(0.0), p))
        .collect();
    files.sort_by(|a, b| b.0.total_cmp(&a.0));
    Value::Array(
        files
            .into_iter()
            .map(|(mtime, f)| {
                let name = f
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let size = fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                json!({
                    "name": name,
                    "size_mb": round_to(size as f64 / 1024.0 / 1024.0, 1),
                    "created": iso_local(mtime),
                    "archive": name.starts_with("archive_"),
                    "note": note(&name),
                })
            })
            .collect(),
    )
}

pub fn backup_file(inst: &Instance, name: &str) -> Result<PathBuf> {
    let ok = !name.is_empty()
        && !name.contains(['/', '\\'])
        && name != ".."
        && (name.starts_with("backup_") || name.starts_with("archive_"))
        && name.ends_with(".zip");
    let path = inst.backup_dir().join(name);
    if !ok || !path.is_file() {
        return Err(Error::NotFound(name.to_string()));
    }
    Ok(path)
}

/// Delete the world folders (callers archive them first). Returns the removed names.
pub fn remove_world(inst: &Instance) -> Result<Vec<String>> {
    let mut removed = vec![];
    for d in world_dirs(inst) {
        fs::remove_dir_all(&d)?;
        removed.push(
            d.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        );
    }
    Ok(removed)
}

pub fn cleanup(inst: &Instance, protect: Option<&Path>) {
    let mut files: Vec<(f64, PathBuf)> = glob(&inst.backup_dir(), "backup_")
        .into_iter()
        .map(|p| (mtime_secs(&p).unwrap_or(0.0), p))
        .collect();
    files.sort_by(|a, b| a.0.total_cmp(&b.0));
    let keep_cfg = cfg().backup_keep;
    let keep = match protect {
        Some(p) => {
            let p = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            files.retain(|(_, f)| fs::canonicalize(f).unwrap_or_else(|_| f.clone()) != p);
            keep_cfg.saturating_sub(1) // the protected one counts towards the limit
        }
        None => keep_cfg,
    };
    let remove = files.len().saturating_sub(keep);
    for (_, old) in files.into_iter().take(remove) {
        let _ = fs::remove_file(old);
    }
}

fn parts(member: &str) -> Vec<&str> {
    member
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect()
}

/// Top-level folders in the zip; rejects anything that could escape the server dir.
fn world_folders(names: &[String]) -> Result<BTreeSet<String>> {
    let mut tops = BTreeSet::new();
    for member in names {
        let p = parts(member);
        if p.is_empty()
            || member.starts_with(['/', '\\'])
            || p.contains(&"..")
            || p[0].contains(':')
        {
            return invalid!("Unsafe path in backup: {member}");
        }
        let top = p[0];
        if p.len() == 1 && !member.ends_with('/') {
            return invalid!("Unexpected file in backup: {member}");
        }
        if !TOP_RE.is_match(top) || NOT_A_WORLD.contains(&top.to_lowercase().as_str()) {
            return invalid!("Backup contains a non-world folder: {top}");
        }
        tops.insert(top.to_string());
    }
    if tops.is_empty() {
        return invalid!("Backup is empty");
    }
    Ok(tops)
}

fn open_zip(path: &Path) -> Result<ZipArchive<BufReader<File>>> {
    ZipArchive::new(BufReader::new(File::open(path)?))
        .map_err(|_| Error::Invalid("Not a valid zip file".into()))
}

fn names(zf: &ZipArchive<BufReader<File>>) -> Vec<String> {
    zf.file_names()
        .filter_map(|n| n.ok().map(|n| n.to_string()))
        .collect()
}

/// Replace the world folders with the backup's. The server must be stopped. The current world is
/// saved first; returns that safety backup (None if there was no world).
pub fn restore(inst: &Instance, name: &str) -> Result<Option<PathBuf>> {
    let src = backup_file(inst, name)?;
    let mut zf = open_zip(&src)?;
    let tops = world_folders(&names(&zf))?;
    let safety = if world_dirs(inst).is_empty() {
        None
    } else {
        Some(create_backup(inst, "before-restore", Some(&src))?)
    };
    let staging = inst.server_dir().join(".restore-tmp");
    let _ = fs::remove_dir_all(&staging);
    let result = (|| -> Result<()> {
        zf.extract(&staging)?;
        for top in &tops {
            let dst = inst.server_dir().join(top);
            let _ = remove_path(&dst);
            fs::rename(staging.join(top), &dst)?;
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging);
    result.map(|_| safety)
}

// ---------- uploaded worlds ----------

/// Maps zip paths to paths in the server for an uploaded world. Accepts a server world
/// (`<level>/level.dat`, maybe with Paper's `_nether`/`_the_end` folders) and a single-player save
/// (level.dat in the zip root or in exactly one folder).
fn upload_layout(names: &[String], level: &str) -> Result<Box<dyn Fn(&str) -> String>> {
    if names.iter().any(|n| n == "level.dat") {
        // save zipped from the inside: everything belongs into the world folder
        let level = level.to_string();
        return Ok(Box::new(move |name| format!("{level}/{name}")));
    }
    if let Some(n) = names.iter().find(|n| !n.contains('/')) {
        return invalid!("Unexpected file in the zip root: {n}");
    }
    let tops: BTreeSet<&str> = names
        .iter()
        .filter_map(|n| n.split_once('/').map(|(t, _)| t))
        .collect();
    let with_level: Vec<&str> = tops
        .iter()
        .copied()
        .filter(|t| names.contains(&format!("{t}/level.dat")))
        .collect();
    if with_level.len() != 1 {
        return if with_level.is_empty() {
            invalid!("No world found: the zip needs a level.dat (in the root or in one folder)")
        } else {
            invalid!("The zip contains more than one world (several level.dat)")
        };
    }
    let base = with_level[0];
    let mut mapping: BTreeMap<String, String> = BTreeMap::new();
    mapping.insert(base.to_string(), level.to_string());
    for top in tops.iter().filter(|t| **t != base) {
        let suffix = top.strip_prefix(base).unwrap_or("");
        if suffix != "_nether" && suffix != "_the_end" {
            return invalid!("Unexpected folder next to the world: {top}");
        }
        mapping.insert(top.to_string(), format!("{level}{suffix}"));
    }
    Ok(Box::new(move |name: &str| {
        let (top, rest) = name.split_once('/').unwrap_or((name, ""));
        format!("{}/{rest}", mapping[top])
    }))
}

fn is_junk(name: &str) -> bool {
    JUNK.iter()
        .any(|j| name.starts_with(j) || name.ends_with(j))
        || name.contains("/__MACOSX/")
}

/// Check an uploaded world zip and store it as `backup_<stamp>_upload.zip` with the folder names
/// this server uses (so restore() can use it like any backup).
pub fn add_upload(inst: &Instance, src: &Path) -> Result<PathBuf> {
    let mut zf = open_zip(src)?;
    let mut files: Vec<(usize, String, u64)> = vec![];
    for i in 0..zf.len() {
        let entry = zf.by_index_raw(i)?;
        let name = entry.name()?.replace('\\', "/");
        if entry.is_dir() || is_junk(&name) {
            continue;
        }
        let p = parts(&name);
        if name.starts_with('/') || p.contains(&"..") || p.first().is_some_and(|f| f.contains(':'))
        {
            return invalid!("Unsafe path in the zip: {name}");
        }
        files.push((i, name, entry.size()));
    }
    if files.iter().map(|f| f.2).sum::<u64>() > 20 * cfg().world_upload_max {
        return invalid!("The unpacked world would be too large");
    }
    let list: Vec<String> = files.iter().map(|f| f.1.clone()).collect();
    let rename = upload_layout(&list, &level_name(inst))?;

    fs::create_dir_all(inst.backup_dir())?;
    let target = inst
        .backup_dir()
        .join(format!("backup_{}_upload.zip", stamp()));
    let tmp = target.with_extension("part");
    let result = (|| -> Result<()> {
        let mut out = ZipWriter::new(BufWriter::new(File::create(&tmp)?));
        for (i, name, size) in &files {
            if name.ends_with("session.lock") {
                continue;
            }
            let mut entry = zf.by_index(*i)?;
            let mut opts = options_for(name, *size, None);
            if let Some(t) = entry.last_modified() {
                opts = opts.last_modified_time(t);
            }
            out.start_file(rename(name).as_str(), opts)?;
            io::copy(&mut entry, &mut out)?;
        }
        out.finish()?;
        world_folders(&names(&open_zip(&tmp)?))?; // same rules as a restore
        fs::rename(&tmp, &target)?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp);
    result?;
    cleanup(inst, Some(&target));
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_world(inst: &Instance) {
        let w = inst.server_dir().join("world");
        fs::create_dir_all(w.join("region")).unwrap();
        fs::write(w.join("level.dat"), b"level").unwrap();
        fs::write(w.join("region/r.0.0.mca"), b"region").unwrap();
        fs::write(w.join("session.lock"), b"lock").unwrap();
        fs::create_dir_all(inst.server_dir().join("world_nether/DIM-1")).unwrap();
        fs::write(inst.server_dir().join("world_nether/DIM-1/x.mca"), b"n").unwrap();
    }

    fn zip_with(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zw = ZipWriter::new(File::create(path).unwrap());
        for (name, data) in entries {
            if name.ends_with('/') {
                zw.add_directory(*name, SimpleFileOptions::default())
                    .unwrap();
            } else {
                zw.start_file(*name, SimpleFileOptions::default()).unwrap();
                zw.write_all(data).unwrap();
            }
        }
        zw.finish().unwrap();
    }

    fn zip_names(path: &Path) -> Vec<String> {
        let mut n = names(&open_zip(path).unwrap());
        n.sort();
        n
    }

    #[test]
    fn backup_and_restore() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        make_world(&inst);
        let b = create_backup(&inst, "", None).unwrap();
        assert_eq!(
            zip_names(&b),
            vec![
                "world/level.dat",
                "world/region/r.0.0.mca",
                "world_nether/DIM-1/x.mca"
            ]
        );
        fs::write(inst.server_dir().join("world/level.dat"), b"changed").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100)); // new stamp
        let name = b.file_name().unwrap().to_str().unwrap();
        let safety = restore(&inst, name).unwrap().unwrap();
        assert!(safety.to_string_lossy().ends_with("_before-restore.zip"));
        assert_eq!(
            fs::read(inst.server_dir().join("world/level.dat")).unwrap(),
            b"level"
        );
        let list = list_backups(&inst);
        assert_eq!(list.as_array().unwrap().len(), 2);
        assert_eq!(list[0]["note"], json!("before restore"));
    }

    #[test]
    fn unsafe_zips_are_refused() {
        let ok = |n: &[&str]| world_folders(&n.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(ok(&["world/level.dat"]).is_ok());
        assert!(ok(&["../evil/x"]).is_err());
        assert!(ok(&["/abs/x"]).is_err());
        assert!(ok(&["level.dat"]).is_err());
        assert!(ok(&["plugins/x.jar"]).is_err());
        assert!(ok(&["C:/x/y"]).is_err());
        assert!(ok(&[]).is_err());
    }

    #[test]
    fn backup_names_are_checked() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        fs::create_dir_all(inst.backup_dir()).unwrap();
        fs::write(inst.backup_dir().join("backup_x.zip"), b"").unwrap();
        fs::write(inst.backup_dir().join("other.zip"), b"").unwrap();
        assert!(backup_file(&inst, "backup_x.zip").is_ok());
        assert!(backup_file(&inst, "other.zip").is_err());
        assert!(backup_file(&inst, "../backup_x.zip").is_err());
        assert!(backup_file(&inst, "backup_missing.zip").is_err());
    }

    #[test]
    fn retention_keeps_archives_and_protected() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let dir = inst.backup_dir();
        fs::create_dir_all(&dir).unwrap();
        for i in 0..10 {
            let p = dir.join(format!("backup_{i:02}.zip"));
            fs::write(&p, b"").unwrap();
            let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 + i);
            File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        fs::write(dir.join("archive_old.zip"), b"").unwrap();
        cleanup(&inst, Some(&dir.join("backup_00.zip")));
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "archive_old.zip",
                "backup_00.zip",
                "backup_04.zip",
                "backup_05.zip",
                "backup_06.zip",
                "backup_07.zip",
                "backup_08.zip",
                "backup_09.zip"
            ]
        );
    }

    #[test]
    fn uploads_get_this_servers_names() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let up = cfg().data_dir.join("up.zip");
        zip_with(
            &up,
            &[
                ("My Save/level.dat", b"l"),
                ("My Save/region/r.0.0.mca", b"r"),
                ("__MACOSX/My Save/._level.dat", b"x"),
                ("My Save/session.lock", b""),
            ],
        );
        let b = add_upload(&inst, &up).unwrap();
        assert_eq!(
            zip_names(&b),
            vec!["world/level.dat", "world/region/r.0.0.mca"]
        );
        assert_eq!(
            note(b.file_name().unwrap().to_str().unwrap()).as_deref(),
            Some("uploaded")
        );

        zip_with(&up, &[("level.dat", b"l"), ("data/x.dat", b"x")]);
        assert_eq!(
            zip_names(&add_upload(&inst, &up).unwrap()),
            vec!["world/data/x.dat", "world/level.dat"]
        );

        zip_with(&up, &[("w/level.dat", b"l"), ("w_nether/DIM-1/a", b"a")]);
        assert_eq!(
            zip_names(&add_upload(&inst, &up).unwrap()),
            vec!["world/level.dat", "world_nether/DIM-1/a"]
        );

        zip_with(&up, &[("a/level.dat", b"l"), ("b/level.dat", b"l")]);
        assert!(add_upload(&inst, &up).is_err());
        zip_with(&up, &[("a/x.txt", b"l")]);
        assert!(add_upload(&inst, &up).is_err());
        zip_with(&up, &[("w/level.dat", b"l"), ("../evil", b"l")]);
        assert!(add_upload(&inst, &up).is_err());
        fs::write(&up, b"no zip").unwrap();
        assert!(add_upload(&inst, &up).is_err());
    }
}
