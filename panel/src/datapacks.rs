//! Data packs (Vanilla and Paper): a library outside the world plus installed copies in the world.
//!
//! - Library: `<DATA_DIR>/datapacks/` keeps every uploaded pack, independent of the world (survives
//!   "New world" and restores) and shared by all servers of the panel.
//! - World: `<world>/datapacks/` holds copies of the installed packs; this is what Minecraft loads.
//! - Enabled/disabled is per world and lives in level.dat (`Data.DataPacks.Enabled/Disabled`, ids
//!   like `file/<name>`). A pack in the world folder that is in neither list is new and gets enabled
//!   on the next start or `/reload`. While the server runs, packs are switched with `/datapack`;
//!   while it is stopped, level.dat is edited directly.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::config::cfg;
use crate::error::{Error, Result};
use crate::instances::Instance;
use crate::nbt::{self, Compound, Tag};
use crate::util::{copy_tree, remove_path, strip_bom, walk_files, with_suffix};
use crate::{backup, invalid, world};

pub const MAX_UPLOAD: u64 = 100 * 1024 * 1024;
// No quotes or backslashes (the name goes into a quoted console command), no path separators
static NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w\-. ()+\[\]!&',]{1,100}$").unwrap());

pub fn library() -> PathBuf {
    cfg().data_dir.join("datapacks")
}

/// The world's datapacks folder.
pub fn folder(inst: &Instance) -> PathBuf {
    inst.server_dir()
        .join(backup::level_name(inst))
        .join("datapacks")
}

pub fn pack_id(name: &str) -> String {
    format!("file/{name}")
}

pub fn validate_name(name: &str) -> Result<String> {
    if !NAME_RE.is_match(name)
        || name.starts_with('.')
        || name.ends_with(".part")
        || name.ends_with(".tmp")
    {
        return invalid!("Invalid data pack name");
    }
    Ok(name.to_string())
}

fn existing(base: &Path, name: &str, where_: &str) -> Result<PathBuf> {
    let path = base.join(validate_name(name)?);
    if !path.exists() {
        return Err(Error::NotFound(format!(
            "Data pack {name} is not in the {where_}"
        )));
    }
    Ok(path)
}

/// Installed copy in the world.
pub fn path_of(inst: &Instance, name: &str) -> Result<PathBuf> {
    existing(&folder(inst), name, "world")
}

pub fn library_path(name: &str) -> Result<PathBuf> {
    existing(&library(), name, "library")
}

// ---------- reading ----------

/// pack.mcmeta description: plain string or text component(s).
fn text(desc: &Value) -> String {
    match desc {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(text).collect(),
        Value::Object(o) => {
            text(o.get("text").unwrap_or(&Value::Null))
                + &text(o.get("extra").unwrap_or(&Value::Null))
        }
        _ => String::new(),
    }
}

/// A file at the top of a folder or zip pack, at most `limit` bytes.
fn read_top(path: &Path, name: &str, limit: u64) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    if path.is_dir() {
        fs::File::open(path.join(name))
            .ok()?
            .take(limit + 1)
            .read_to_end(&mut buf)
            .ok()?;
    } else {
        let mut zf = zip::ZipArchive::new(fs::File::open(path).ok()?).ok()?;
        let f = zf.by_name(name).ok()?;
        f.take(limit + 1).read_to_end(&mut buf).ok()?;
    }
    (buf.len() as u64 <= limit).then_some(buf)
}

fn mcmeta(path: &Path) -> Option<Value> {
    let raw = read_top(path, "pack.mcmeta", 1024 * 1024)?;
    let meta: Value = serde_json::from_slice(strip_bom(&raw)).ok()?;
    meta.is_object().then_some(meta)
}

const ICON_LIMIT: u64 = 512 * 1024;

fn has_icon(path: &Path) -> bool {
    if path.is_dir() {
        return path.join("pack.png").is_file();
    }
    fs::File::open(path)
        .ok()
        .and_then(|f| zip::ZipArchive::new(f).ok())
        .is_some_and(|mut z| z.by_name("pack.png").is_ok())
}

/// The pack's pack.png (library copy first, else the world's), only if it really is a PNG.
pub fn icon(inst: &Instance, name: &str) -> Result<Vec<u8>> {
    let path = library_path(name).or_else(|_| path_of(inst, name))?;
    match read_top(&path, "pack.png", ICON_LIMIT) {
        Some(data) if data.starts_with(b"\x89PNG\r\n\x1a\n") => Ok(data),
        _ => Err(Error::NotFound(format!("{name} has no icon"))),
    }
}

/// Optional `"panel": {"title", "category"}` section of pack.mcmeta (ignored by Minecraft).
fn panel_field(meta: Option<&Value>, key: &str) -> String {
    meta.and_then(|m| m.get("panel"))
        .and_then(|p| p.get(key))
        .and_then(Value::as_str)
        .map(|s| s.trim().chars().take(60).collect())
        .unwrap_or_default()
}

fn files(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        vec![path.to_path_buf()]
    } else {
        walk_files(path)
    }
}

fn size(path: &Path) -> u64 {
    files(path)
        .iter()
        .filter_map(|p| fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

fn digest(path: &Path) -> String {
    let mut h = Sha1::new();
    let is_dir = path.is_dir();
    for p in files(path) {
        if is_dir {
            h.update(
                p.strip_prefix(path)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/")
                    .as_bytes(),
            );
        }
        h.update(fs::read(&p).unwrap_or_default());
    }
    hex::encode(h.finalize())
}

fn entries(base: &Path) -> BTreeMap<String, PathBuf> {
    let Ok(rd) = fs::read_dir(base) else {
        return BTreeMap::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_string();
            let zip = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
            ((p.is_dir() || zip) && NAME_RE.is_match(&name) && !name.starts_with('.'))
                .then_some((name, p))
        })
        .collect()
}

/// The Enabled and Disabled string lists of level.dat (created if missing).
fn lists(root: &mut Compound) -> (&mut Vec<Tag>, &mut Vec<Tag>) {
    let packs = root.compound_mut("Data").compound_mut("DataPacks");
    for key in ["Enabled", "Disabled"] {
        let fix = match packs.get(key) {
            Some(Tag::List(t, items)) if *t == nbt::STRING || items.is_empty() => {
                // empty list stored with item type END
                (*t != nbt::STRING).then(|| Tag::List(nbt::STRING, vec![]))
            }
            _ => Some(Tag::List(nbt::STRING, vec![])),
        };
        if let Some(tag) = fix {
            packs.insert(key, tag);
        }
    }
    let mut en = None;
    let mut dis = None;
    for (k, v) in packs.0.iter_mut() {
        if let Tag::List(_, items) = v {
            match k.as_str() {
                "Enabled" => en = Some(items),
                "Disabled" => dis = Some(items),
                _ => {}
            }
        }
    }
    (en.unwrap(), dis.unwrap())
}

fn names_of(items: &[Tag]) -> BTreeSet<String> {
    items
        .iter()
        .filter_map(|t| {
            if let Tag::String(s) = t {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect()
}

pub type PackState = (BTreeSet<String>, BTreeSet<String>);

/// (enabled ids, disabled ids) from level.dat, None if the world doesn't exist yet.
pub fn state(inst: &Instance) -> Result<Option<PackState>> {
    let path = world::level_dat(inst);
    if !path.exists() {
        return Ok(None);
    }
    let (_, mut root) = nbt::load(&path)?;
    let (en, dis) = lists(&mut root);
    Ok(Some((names_of(en), names_of(dis))))
}

// Built-in packs that are always on (not experiments)
const BASE_PACKS: [&str; 2] = ["vanilla", "paper"];

/// Built-in feature packs (experiments) the current world has on; shown as a note only.
fn experiments(st: &Option<PackState>) -> Value {
    let enabled: Vec<&String> = match st {
        Some((en, _)) => en
            .iter()
            .filter(|i| !i.starts_with("file/") && !BASE_PACKS.contains(&i.as_str()))
            .collect(),
        None => vec![],
    };
    json!({"enabled": enabled})
}

/// Library and world packs merged. `status`: enabled / disabled / new (installed, enabled on next
/// start) / None (not installed in this world).
pub fn list_packs(inst: &Instance) -> Value {
    let st = state(inst).ok().flatten();
    let lib = entries(&library());
    let in_world = entries(&folder(inst));
    let mut names: Vec<&String> = lib
        .keys()
        .chain(in_world.keys())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    let packs: Vec<Value> = names
        .into_iter()
        .map(|name| {
            let src = lib.get(name).or_else(|| in_world.get(name)).unwrap();
            let meta = mcmeta(src);
            let pid = pack_id(name);
            let status = if !in_world.contains_key(name) {
                Value::Null
            } else {
                match &st {
                    Some((en, dis)) if en.contains(&pid) || dis.contains(&pid) => {
                        json!(if en.contains(&pid) {
                            "enabled"
                        } else {
                            "disabled"
                        })
                    }
                    _ => json!("new"),
                }
            };
            let desc = meta
                .as_ref()
                .and_then(|m| m.get("pack"))
                .filter(|p| p.is_object())
                .map(|p| text(p.get("description").unwrap_or(&json!(""))))
                .unwrap_or_default();
            json!({
                "name": name,
                "in_library": lib.contains_key(name),
                "installed": in_world.contains_key(name),
                "status": status,
                // Library has a different version than the world: offer to update the world copy
                "outdated": match (lib.get(name), in_world.get(name)) {
                    (Some(a), Some(b)) => digest(a) != digest(b),
                    _ => false,
                },
                "valid": meta.is_some(),
                "title": panel_field(meta.as_ref(), "title"),
                "category": panel_field(meta.as_ref(), "category"),
                "icon": has_icon(src),
                "description": desc.chars().take(300).collect::<String>(),
                "size_kb": (size(src) as f64 / 1024.0).round() as u64,
            })
        })
        .collect();
    json!({"world": st.is_some(), "packs": packs, "experiments": experiments(&st)})
}

// ---------- library ----------

/// Check an uploaded file is a data pack zip and move it into the library (replacing an older
/// version of the same name).
pub fn add_upload(name: &str, src: &Path) -> Result<PathBuf> {
    validate_name(name)?;
    if !name.to_lowercase().ends_with(".zip") {
        return invalid!("Upload a .zip file");
    }
    check_zip(src)?;
    let target = library().join(name);
    if target.is_dir() {
        return Err(Error::Exists(format!(
            "A folder pack named {name} is already in the library"
        )));
    }
    fs::create_dir_all(library())?;
    if fs::rename(src, &target).is_err() {
        fs::copy(src, &target)?;
        let _ = fs::remove_file(src);
    }
    Ok(target)
}

fn check_zip(path: &Path) -> Result<()> {
    let names: Vec<String> = match zip::ZipArchive::new(fs::File::open(path)?) {
        Ok(zf) => zf
            .file_names()
            .filter_map(|n| n.ok().map(|n| n.to_string()))
            .collect(),
        Err(_) => return invalid!("Not a valid zip file"),
    };
    if names.iter().any(|n| n == "pack.mcmeta") {
        if mcmeta(path).is_none() {
            return invalid!("pack.mcmeta is not valid JSON");
        }
        return Ok(());
    }
    if names.iter().any(|n| n.ends_with("/pack.mcmeta")) {
        return invalid!(
            "pack.mcmeta must be at the top of the zip (it is zipped one folder too deep)"
        );
    }
    invalid!("Not a data pack: pack.mcmeta is missing")
}

/// Copy a pack (zip or folder), replacing the target via a temporary name.
fn copy(src: &Path, target: &Path) -> Result<()> {
    let tmp = with_suffix(target, ".tmp");
    remove_path(&tmp)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if src.is_dir() {
        copy_tree(src, &tmp)?;
    } else {
        fs::copy(src, &tmp)?;
    }
    remove_path(target)?;
    fs::rename(&tmp, target)?;
    Ok(())
}

/// Copy a pack that only exists in the world (e.g. from a restored backup) into the library.
pub fn adopt(inst: &Instance, name: &str) -> Result<()> {
    copy(&path_of(inst, name)?, &library().join(name))
}

pub fn delete_from_library(name: &str) -> Result<()> {
    remove_path(&library_path(name)?)?;
    Ok(())
}

// ---------- world ----------

/// Copy the library version into the world (also updates an older installed copy).
pub fn install(inst: &Instance, name: &str) -> Result<()> {
    copy(&library_path(name)?, &folder(inst).join(name))
}

pub fn uninstall(inst: &Instance, name: &str) -> Result<()> {
    remove_path(&path_of(inst, name)?)?;
    Ok(())
}

/// Before the old world is removed: copy packs that only exist in the world into the library, so
/// the selected ones can be installed into the new world. Returns the names that are available.
pub fn prepare_new_world(inst: &Instance, names: &[String]) -> Result<Vec<String>> {
    let lib = entries(&library());
    let in_world = entries(&folder(inst));
    for name in names {
        if !lib.contains_key(name) && in_world.contains_key(name) {
            adopt(inst, name)?;
        }
    }
    Ok(names
        .iter()
        .filter(|n| library().join(n).exists())
        .cloned()
        .collect())
}

pub fn install_many(inst: &Instance, names: &[String]) -> Result<()> {
    for name in names {
        install(inst, name)?;
    }
    Ok(())
}

/// Edit level.dat while the server is stopped. Returns a message for the log.
pub fn set_enabled_offline(inst: &Instance, name: &str, enabled: bool) -> Result<String> {
    path_of(inst, name)?;
    let pid = pack_id(name);
    let dat = world::level_dat(inst);
    if !dat.exists() {
        if enabled {
            return Ok(format!(
                "Data pack {name} will be enabled when the world is created."
            ));
        }
        return invalid!("The world doesn't exist yet: start the server once, then disable packs");
    }
    let (root_name, mut root) = nbt::load(&dat)?;
    {
        let (en, dis) = lists(&mut root);
        let is_pid = |t: &Tag| matches!(t, Tag::String(s) if *s == pid);
        en.retain(|t| !is_pid(t));
        dis.retain(|t| !is_pid(t));
        // end of Enabled = highest priority, like /datapack enable
        if enabled {
            en.push(Tag::String(pid.clone()))
        } else {
            dis.push(Tag::String(pid.clone()))
        }
    }
    nbt::save(&dat, &root_name, &root)?;
    Ok(format!(
        "Data pack {name} {} (applies on next start).",
        if enabled { "enabled" } else { "disabled" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn pack_zip(path: &Path, mcmeta: Option<&str>) {
        let mut zw = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        if let Some(m) = mcmeta {
            zw.start_file("pack.mcmeta", o).unwrap();
            zw.write_all(m.as_bytes()).unwrap();
        }
        zw.start_file("data/x/function/a.mcfunction", o).unwrap();
        zw.write_all(b"say hi").unwrap();
        zw.finish().unwrap();
    }

    fn level_dat(inst: &Instance) {
        let p = world::level_dat(inst);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let packs = Compound(vec![
            (
                "Enabled".into(),
                Tag::List(
                    nbt::STRING,
                    vec![
                        Tag::String("vanilla".into()),
                        Tag::String("minecraft_bundle".into()),
                    ],
                ),
            ),
            ("Disabled".into(), Tag::List(nbt::END, vec![])),
        ]);
        let data = Compound(vec![("DataPacks".into(), Tag::Compound(packs))]);
        nbt::save(
            &p,
            "",
            &Compound(vec![("Data".into(), Tag::Compound(data))]),
        )
        .unwrap();
    }

    #[test]
    fn upload_install_toggle() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let up = cfg().data_dir.join("up");
        pack_zip(
            &up,
            Some(r#"{"pack": {"pack_format": 61, "description": [{"text": "Hello "}, "world"]}}"#),
        );
        add_upload("tools.zip", &up).unwrap();
        let l = list_packs(&inst);
        assert_eq!(l["world"], json!(false));
        assert_eq!(l["packs"][0]["status"], Value::Null);
        assert_eq!(l["packs"][0]["description"], json!("Hello world"));

        install(&inst, "tools.zip").unwrap();
        assert_eq!(list_packs(&inst)["packs"][0]["status"], json!("new"));
        level_dat(&inst);
        let l = list_packs(&inst);
        assert_eq!(l["packs"][0]["status"], json!("new"));
        assert_eq!(l["experiments"]["enabled"], json!(["minecraft_bundle"]));

        set_enabled_offline(&inst, "tools.zip", false).unwrap();
        assert_eq!(list_packs(&inst)["packs"][0]["status"], json!("disabled"));
        set_enabled_offline(&inst, "tools.zip", true).unwrap();
        let st = state(&inst).unwrap().unwrap();
        assert!(st.0.contains("file/tools.zip") && !st.1.contains("file/tools.zip"));
        assert_eq!(list_packs(&inst)["packs"][0]["outdated"], json!(false));

        pack_zip(&up, Some(r#"{"pack": {"description": "v2"}}"#));
        add_upload("tools.zip", &up).unwrap();
        assert_eq!(list_packs(&inst)["packs"][0]["outdated"], json!(true));

        uninstall(&inst, "tools.zip").unwrap();
        delete_from_library("tools.zip").unwrap();
        assert_eq!(list_packs(&inst)["packs"], json!([]));
    }

    #[test]
    fn bad_uploads_and_names() {
        let _t = crate::testutil::data_dir();
        let up = cfg().data_dir.join("up");
        pack_zip(&up, None);
        assert!(add_upload("x.zip", &up).is_err());
        pack_zip(&up, Some("{not json"));
        assert!(add_upload("x.zip", &up).is_err());
        fs::write(&up, b"nope").unwrap();
        assert!(add_upload("x.zip", &up).is_err());
        assert!(validate_name("a\"b").is_err());
        assert!(validate_name("../x").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("My Pack (v2).zip").is_ok());
    }

    #[test]
    fn folder_packs_and_adopt() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let p = folder(&inst).join("mypack");
        fs::create_dir_all(p.join("data")).unwrap();
        fs::write(
            p.join("pack.mcmeta"),
            r#"{"pack": {"description": "mine"}}"#,
        )
        .unwrap();
        let l = list_packs(&inst);
        assert_eq!(l["packs"][0]["in_library"], json!(false));
        assert_eq!(
            prepare_new_world(&inst, &["mypack".into(), "gone".into()]).unwrap(),
            vec!["mypack"]
        );
        assert!(library().join("mypack/pack.mcmeta").is_file());
    }

    #[test]
    fn title_category_icon() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let p = library().join("nice");
        fs::create_dir_all(p.join("data")).unwrap();
        fs::write(
            p.join("pack.mcmeta"),
            r#"{"pack": {"description": "d"}, "panel": {"title": " Nice Pack ", "category": "Komfort"}}"#,
        )
        .unwrap();
        assert!(icon(&inst, "nice").is_err(), "no pack.png yet");
        fs::write(p.join("pack.png"), b"not a png").unwrap();
        assert!(icon(&inst, "nice").is_err(), "only real PNGs");
        fs::write(p.join("pack.png"), b"\x89PNG\r\n\x1a\nrest").unwrap();
        assert!(icon(&inst, "nice").unwrap().starts_with(b"\x89PNG"));
        assert!(icon(&inst, "../nice").is_err());
        pack_zip(&library().join("plain.zip"), Some(r#"{"pack": {"description": "z"}}"#));
        let l = list_packs(&inst);
        let nice = &l["packs"][0];
        assert_eq!(nice["title"], json!("Nice Pack"));
        assert_eq!(nice["category"], json!("Komfort"));
        assert_eq!(nice["icon"], json!(true));
        let plain = &l["packs"][1];
        assert_eq!((plain["title"].as_str(), plain["icon"].as_bool()), (Some(""), Some(false)));
    }
}
