//! Server jars: Vanilla (Mojang), Paper (PaperMC Fill API v3) and Fabric (Fabric meta API).
//!
//! Jars are stored as `<JARS_DIR>/<flavor>-<version>.jar` with a sidecar `<flavor>-<version>.json`
//! holding the required Java version, build and JVM flags. The Fabric jar is Fabric's server
//! launcher: on its first start it downloads the Vanilla server and the loader libraries into the
//! server folder itself. Its `build` is the loader version.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Value, json};

use crate::config::{self, cfg};
use crate::error::{Error, Result};
use crate::http::{self, Hash};
use crate::invalid;
use crate::util::{capitalize, lock, read_json, round_to};

pub const MOJANG_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const PAPER_API: &str = "https://fill.papermc.io/v3/projects/paper";
const FABRIC_API: &str = "https://meta.fabricmc.net/v2";
pub const FLAVORS: [&str; 3] = ["vanilla", "paper", "fabric"];
// Loader/installer versions from Fabric's API go into a URL: keep them plain
static FABRIC_VER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9A-Za-z.+\-]{1,40}$").unwrap());
const FABRIC_MAX_JAR: u64 = 64 << 20;
const JAR_MAX: u64 = 1 << 30;
const CACHE_TTL: Duration = Duration::from_secs(600);

static CACHE: LazyLock<Mutex<HashMap<String, (Instant, Value)>>> = LazyLock::new(Default::default);
/// JARS_DIR is shared by all servers: one download/update at a time (same .part file otherwise)
static DOWNLOAD_LOCK: Mutex<()> = Mutex::new(());

pub type Log<'a> = &'a (dyn Fn(&str) + Sync);

pub fn cached(url: &str) -> Result<Value> {
    if let Some((t, v)) = lock(&CACHE).get(url) {
        if t.elapsed() < CACHE_TTL {
            return Ok(v.clone());
        }
    }
    let data = http::get_json(url)?;
    lock(&CACHE).insert(url.to_string(), (Instant::now(), data.clone()));
    Ok(data)
}

pub fn validate_flavor(flavor: &str) -> Result<&str> {
    if !FLAVORS.contains(&flavor) {
        return invalid!("Unknown server type: {flavor}");
    }
    Ok(flavor)
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

// ---------- version ordering (for upgrade/downgrade detection) ----------

/// Version id -> position in Mojang's manifest (0 = newest).
fn mojang_ranks() -> HashMap<String, usize> {
    match cached(MOJANG_MANIFEST) {
        Ok(m) => arr(&m["versions"])
            .iter()
            .enumerate()
            .map(|(i, v)| (s(v, "id").to_string(), i))
            .collect(),
        Err(_) => HashMap::new(),
    }
}

pub fn rank(version_id: &str) -> Option<usize> {
    mojang_ranks().get(version_id).copied()
}

/// Mojang's manifest entry of one version.
pub fn manifest_entry(version_id: &str) -> Result<Option<Value>> {
    let m = cached(MOJANG_MANIFEST)?;
    Ok(arr(&m["versions"])
        .iter()
        .find(|v| s(v, "id") == version_id)
        .cloned())
}

// ---------- listing ----------

pub fn list_versions(flavor: &str, include_snapshots: bool) -> Result<Value> {
    validate_flavor(flavor)?;
    let ranks = mojang_ranks();
    let entry =
        |id: &str, snapshot: bool| json!({"id": id, "snapshot": snapshot, "rank": ranks.get(id)});
    if flavor == "vanilla" {
        let m = cached(MOJANG_MANIFEST)?;
        let versions: Vec<Value> = arr(&m["versions"])
            .iter()
            .filter(|v| {
                s(v, "type") == "release" || (include_snapshots && s(v, "type") == "snapshot")
            })
            .map(|v| entry(s(v, "id"), s(v, "type") != "release"))
            .collect();
        return Ok(json!({"latest": m["latest"]["release"], "versions": versions}));
    }
    if flavor == "fabric" {
        let list = cached(&format!("{FABRIC_API}/versions/game"))?;
        let versions: Vec<Value> = arr(&list)
            .iter()
            .filter(|v| v["stable"].as_bool() == Some(true) || include_snapshots)
            .map(|v| entry(s(v, "version"), v["stable"].as_bool() != Some(true)))
            .collect();
        let latest = versions
            .iter()
            .find(|v| v["snapshot"] == json!(false))
            .map(|v| v["id"].clone());
        return Ok(json!({"latest": latest, "versions": versions}));
    }
    let project = cached(PAPER_API)?;
    let mut versions: Vec<Value> = vec![];
    if let Some(groups) = project["versions"].as_object() {
        for group in groups.values() {
            for vid in arr(group).iter().filter_map(Value::as_str) {
                let snapshot = vid.contains('-'); // "26.3-rc-3", "1.21-pre1", ...
                if snapshot && !include_snapshots {
                    continue;
                }
                versions.push(entry(vid, snapshot));
            }
        }
    }
    // Newest first: by Mojang order where known, unknown ids keep API order at the end
    versions.sort_by_key(|v| (v["rank"].is_null(), v["rank"].as_u64().unwrap_or(0)));
    let latest = versions
        .iter()
        .find(|v| v["snapshot"] == json!(false))
        .map(|v| v["id"].clone());
    Ok(json!({"latest": latest, "versions": versions}))
}

// ---------- installed jars ----------

fn path(flavor: &str, version_id: &str, ext: &str) -> Result<PathBuf> {
    // Validation prevents path traversal through user-supplied ids
    let flavor = validate_flavor(flavor)?;
    let version = config::validate_version(version_id)?;
    Ok(cfg().jars_dir.join(format!("{flavor}-{version}{ext}")))
}

pub fn jar_path(flavor: &str, version_id: &str) -> Result<PathBuf> {
    path(flavor, version_id, ".jar")
}

fn meta_path(flavor: &str, version_id: &str) -> Result<PathBuf> {
    path(flavor, version_id, ".json")
}

pub fn meta(flavor: &str, version_id: &str) -> Value {
    meta_path(flavor, version_id)
        .ok()
        .and_then(|p| read_json(&p))
        .filter(Value::is_object)
        .unwrap_or(json!({}))
}

pub fn required_java(flavor: &str, version_id: &str) -> u32 {
    meta(flavor, version_id)["java"].as_u64().unwrap_or(8) as u32
}

pub fn jvm_flags(flavor: &str, version_id: &str) -> Vec<String> {
    arr(&meta(flavor, version_id)["flags"])
        .iter()
        .filter_map(Value::as_str)
        .filter(|f| f.starts_with("-XX:"))
        .map(str::to_string)
        .collect()
}

pub fn installed() -> Value {
    let Ok(rd) = fs::read_dir(&cfg().jars_dir) else {
        return json!([]);
    };
    let mut jars: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jar"))
        .collect();
    jars.sort();
    let mut result: Vec<Value> = vec![];
    for jar in jars {
        let stem = jar
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some((flavor, version)) = stem.split_once('-') else {
            continue;
        };
        if !FLAVORS.contains(&flavor) {
            continue;
        }
        let info = meta(flavor, version);
        let size = fs::metadata(&jar).map(|m| m.len()).unwrap_or(0);
        result.push(json!({
            "flavor": flavor, "version": version, "build": info.get("build"), "java": info.get("java"),
            "size_mb": round_to(size as f64 / 1024.0 / 1024.0, 1), "rank": rank(version),
        }));
    }
    result.sort_by(|a, b| {
        let key = |v: &Value| {
            (
                v["rank"].is_null(),
                v["rank"].as_u64().unwrap_or(0),
                v["flavor"].as_str().unwrap_or("").to_string(),
            )
        };
        key(a).cmp(&key(b))
    });
    Value::Array(result)
}

fn ignored_file() -> PathBuf {
    cfg().jars_dir.join("ignored-paper-cache.json")
}

fn ignored() -> BTreeSet<String> {
    read_json(&ignored_file())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn write_ignored(set: &BTreeSet<String>) -> std::io::Result<()> {
    fs::write(
        ignored_file(),
        serde_json::to_string(set).unwrap_or_default(),
    )
}

/// server_dirs: the servers' folders, to remember jars that came from a Paper cache.
pub fn delete(flavor: &str, version_id: &str, server_dirs: &[PathBuf]) -> Result<()> {
    let jar = jar_path(flavor, version_id)?;
    if !jar.exists() {
        return Err(Error::NotFound(format!(
            "{flavor} {version_id} is not downloaded"
        )));
    }
    fs::remove_file(&jar)?;
    let _ = fs::remove_file(meta_path(flavor, version_id)?);
    if flavor == "vanilla"
        && server_dirs.iter().any(|d| {
            d.join("cache")
                .join(format!("mojang_{version_id}.jar"))
                .exists()
        })
    {
        // Don't re-import it from Paper's cache after the user deleted it
        let mut set = ignored();
        set.insert(version_id.to_string());
        write_ignored(&set)?;
    }
    Ok(())
}

// ---------- download ----------

fn write_meta(flavor: &str, version_id: &str, info: &Value) -> Result<()> {
    fs::write(meta_path(flavor, version_id)?, info.to_string())?;
    Ok(())
}

fn download_vanilla(version_id: &str, log: Log) -> Result<Value> {
    let Some(entry) = manifest_entry(version_id)? else {
        return invalid!("Unknown Minecraft version: {version_id}");
    };
    let info = http::get_json(s(&entry, "url"))?;
    let server = &info["downloads"]["server"];
    if !server.is_object() {
        return invalid!("Mojang provides no server jar for {version_id}");
    }
    log(&format!(
        "Downloading Vanilla {version_id} ({} MB) ...",
        server["size"].as_u64().unwrap_or(0) >> 20
    ));
    http::download(
        s(server, "url"),
        &jar_path("vanilla", version_id)?,
        None,
        Hash::Sha1(s(server, "sha1").into()),
        JAR_MAX,
    )?;
    Ok(json!({"java": info["javaVersion"]["majorVersion"].as_u64().unwrap_or(8)}))
}

/// (version info, newest build): the newest STABLE build, else the newest of any channel.
fn latest_paper_build(version_id: &str, log: Log) -> Result<(Value, Value)> {
    let v = http::quote(version_id);
    let info = http::get_json(&format!("{PAPER_API}/versions/{v}"))?["version"].clone();
    let mut builds = http::get_json(&format!("{PAPER_API}/versions/{v}/builds?channel=STABLE"))?;
    if arr(&builds).is_empty() {
        builds = http::get_json(&format!("{PAPER_API}/versions/{v}/builds"))?;
        if arr(&builds).is_empty() {
            return invalid!("Paper has no builds for {version_id}");
        }
        log(&format!(
            "Note: no stable Paper build for {version_id}, using {} build.",
            s(&builds[0], "channel").to_lowercase()
        ));
    }
    Ok((info, builds[0].clone()))
}

fn fetch_paper(version_id: &str, info: &Value, build: &Value, log: Log) -> Result<Value> {
    let dl = &build["downloads"]["server:default"];
    log(&format!(
        "Downloading Paper {version_id} build {} ({} MB) ...",
        build["id"],
        dl["size"].as_u64().unwrap_or(0) >> 20
    ));
    http::download(
        s(dl, "url"),
        &jar_path("paper", version_id)?,
        None,
        Hash::Sha256(s(&dl["checksums"], "sha256").into()),
        JAR_MAX,
    )?;
    let java = &info["java"];
    Ok(json!({
        "java": java["version"]["minimum"].as_u64().unwrap_or(21),
        "build": build["id"],
        "flags": java["flags"]["recommended"].as_array().cloned().unwrap_or_default(),
    }))
}

fn in_use(e: Error) -> Error {
    match e {
        Error::InUse(_) => {
            Error::Invalid("The jar is in use: stop the server first, then update".into())
        }
        other => other,
    }
}

/// Replace a downloaded Paper jar with the newest build. Returns true if it was replaced.
fn update_paper(version_id: &str, log: Log) -> Result<bool> {
    let _g = lock(&DOWNLOAD_LOCK);
    if !jar_path("paper", version_id)?.exists() {
        return Err(Error::NotFound(format!(
            "Paper {version_id} is not downloaded"
        )));
    }
    let current = meta("paper", version_id)["build"].as_i64();
    let (info, build) = latest_paper_build(version_id, log)?;
    let new_id = build["id"].as_i64().unwrap_or(0);
    if let Some(cur) = current.filter(|c| new_id <= *c) {
        log(&format!("Paper {version_id} is up to date (build {cur})."));
        return Ok(false);
    }
    let new_meta = fetch_paper(version_id, &info, &build, log).map_err(in_use)?;
    write_meta("paper", version_id, &new_meta)?;
    log(&format!(
        "Paper {version_id} updated to build {new_id}{}. Takes effect on the next server start.",
        current.map(|c| format!(" (was {c})")).unwrap_or_default()
    ));
    Ok(true)
}

// ---------- Fabric ----------

/// Java major Mojang's version JSON asks for (8 if unknown).
fn mojang_java(version_id: &str) -> Result<u64> {
    let Some(entry) = manifest_entry(version_id)? else {
        return Ok(8);
    };
    Ok(
        http::get_json(s(&entry, "url"))?["javaVersion"]["majorVersion"]
            .as_u64()
            .unwrap_or(8),
    )
}

/// First stable entry (Fabric's lists are newest first), else the first one.
fn newest_stable(entries: &[Value], key: impl Fn(&Value) -> &Value) -> Result<String> {
    if entries.is_empty() {
        return invalid!("Fabric's API returned no versions");
    }
    let pick = entries
        .iter()
        .find(|e| key(e)["stable"].as_bool() == Some(true))
        .unwrap_or(&entries[0]);
    let version = s(key(pick), "version").to_string();
    if !FABRIC_VER_RE.is_match(&version) {
        return invalid!("Unexpected Fabric version id: {version}");
    }
    Ok(version)
}

/// Newest stable Fabric loader for this Minecraft version.
fn fabric_loader(version_id: &str) -> Result<String> {
    let loaders = http::get_json(&format!(
        "{FABRIC_API}/versions/loader/{}",
        http::quote(version_id)
    ))?;
    if arr(&loaders).is_empty() {
        return invalid!("Fabric doesn't support Minecraft {version_id}");
    }
    newest_stable(arr(&loaders), |e| &e["loader"])
}

/// Fabric publishes no checksum for the launcher jar: check size and that it is a jar (zip).
fn fetch_zip(url: &str, target: &Path) -> Result<()> {
    let tmp = crate::util::with_suffix(target, ".check");
    let result = (|| -> Result<()> {
        http::download(
            url,
            &tmp,
            Some(&crate::util::with_suffix(target, ".part")),
            Hash::None,
            FABRIC_MAX_JAR,
        )?;
        let ok = zip::ZipArchive::new(fs::File::open(&tmp)?)
            .ok()
            .is_some_and(|mut zf| {
                (0..zf.len()).all(|i| zf.by_index(i).is_ok())
                    && zf.by_name("META-INF/MANIFEST.MF").is_ok()
            });
        if !ok {
            return invalid!("Downloaded Fabric launcher is not a valid jar");
        }
        fs::rename(&tmp, target)?;
        Ok(())
    })();
    let _ = fs::remove_file(&tmp);
    result
}

fn fetch_fabric(version_id: &str, loader: &str, log: Log) -> Result<Value> {
    let installers = http::get_json(&format!("{FABRIC_API}/versions/installer"))?;
    let installer = newest_stable(arr(&installers), |e| e)?;
    log(&format!(
        "Downloading Fabric {version_id} (loader {loader}) ..."
    ));
    let url = format!(
        "{FABRIC_API}/versions/loader/{}/{loader}/{installer}/server/jar",
        http::quote(version_id)
    );
    fetch_zip(&url, &jar_path("fabric", version_id)?)?;
    Ok(json!({"java": mojang_java(version_id)?, "build": loader, "installer": installer}))
}

/// Switch a downloaded Fabric server to the newest stable loader. True if it was replaced.
fn update_fabric(version_id: &str, log: Log) -> Result<bool> {
    let _g = lock(&DOWNLOAD_LOCK);
    if !jar_path("fabric", version_id)?.exists() {
        return Err(Error::NotFound(format!(
            "Fabric {version_id} is not downloaded"
        )));
    }
    let current = meta("fabric", version_id)["build"]
        .as_str()
        .map(str::to_string);
    let loader = fabric_loader(version_id)?;
    if current.as_deref() == Some(loader.as_str()) {
        log(&format!(
            "Fabric {version_id} is up to date (loader {loader})."
        ));
        return Ok(false);
    }
    let new_meta = fetch_fabric(version_id, &loader, log).map_err(in_use)?;
    write_meta("fabric", version_id, &new_meta)?;
    log(&format!(
        "Fabric {version_id} updated to loader {loader}{}. Takes effect on the next server start.",
        current.map(|c| format!(" (was {c})")).unwrap_or_default()
    ));
    Ok(true)
}

/// Newest Paper build / Fabric loader for a downloaded jar.
pub fn update(flavor: &str, version_id: &str, log: Log) -> Result<bool> {
    match flavor {
        "paper" => update_paper(version_id, log),
        "fabric" => update_fabric(version_id, log),
        _ => {
            invalid!("Only Paper and Fabric servers have updates (Vanilla: choose a newer version)")
        }
    }
}

/// Paper downloads the original Mojang jar to `<server_dir>/cache/mojang_<version>.jar`. Register
/// those as downloaded Vanilla servers so they needn't be fetched again. Each file is verified
/// against Mojang's SHA1 first. Returns the imported versions.
pub fn import_paper_cache(server_dir: &Path, log: Log) -> Vec<String> {
    let mut imported = vec![];
    let ignored = ignored();
    let Ok(rd) = fs::read_dir(server_dir.join("cache")) else {
        return imported;
    };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    files.sort();
    for cached_jar in files {
        let name = cached_jar
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some(version_id) = name
            .strip_prefix("mojang_")
            .and_then(|n| n.strip_suffix(".jar"))
        else {
            continue;
        };
        if ignored.contains(version_id) {
            continue;
        }
        let Ok(target) = jar_path("vanilla", version_id) else {
            continue;
        };
        if target.exists() {
            continue;
        }
        let result = (|| -> Result<bool> {
            let Some(entry) = manifest_entry(version_id)? else {
                return Ok(false);
            };
            let info = http::get_json(s(&entry, "url"))?;
            let server = &info["downloads"]["server"];
            if !server.is_object()
                || http::file_hash::<sha1::Sha1>(&cached_jar)? != s(server, "sha1")
            {
                return Ok(false); // incomplete or not the official jar
            }
            if fs::hard_link(&cached_jar, &target).is_err() {
                fs::copy(&cached_jar, &target)?; // no extra disk space with the link
            }
            write_meta(
                "vanilla",
                version_id,
                &json!({"java": info["javaVersion"]["majorVersion"].as_u64().unwrap_or(8)}),
            )?;
            Ok(true)
        })();
        if let Ok(true) = result {
            imported.push(version_id.to_string());
            log(&format!(
                "Vanilla {version_id} taken over from Paper's cache, added to downloaded servers."
            ));
        }
        // offline or unexpected data: try again next time
    }
    imported
}

/// Download and verify a server jar (no-op if already present).
pub fn download(flavor: &str, version_id: &str, log: Log) -> Result<PathBuf> {
    let target = jar_path(flavor, version_id)?;
    if target.exists() {
        return Ok(target);
    }
    let _g = lock(&DOWNLOAD_LOCK);
    if target.exists() {
        return Ok(target); // another server fetched it while we waited
    }
    let info = match flavor {
        "vanilla" => download_vanilla(version_id, log)?,
        "paper" => {
            let (info, build) = latest_paper_build(version_id, log)?;
            fetch_paper(version_id, &info, &build, log)?
        }
        _ => fetch_fabric(version_id, &fabric_loader(version_id)?, log)?,
    };
    write_meta(flavor, version_id, &info)?;
    let mut set = ignored();
    if flavor == "vanilla" && set.remove(version_id) {
        write_ignored(&set)?;
    }
    log(&format!(
        "{} {version_id} downloaded (needs Java {}).",
        capitalize(flavor),
        info["java"]
    ));
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_validated() {
        let _t = crate::testutil::data_dir();
        assert!(
            jar_path("paper", "1.21.4")
                .unwrap()
                .ends_with("paper-1.21.4.jar")
        );
        assert!(jar_path("forge", "1.21.4").is_err());
        assert!(jar_path("paper", "../../x").is_err());
    }

    #[test]
    fn installed_and_meta() {
        let _t = crate::testutil::data_dir();
        let dir = cfg().jars_dir.clone();
        fs::write(dir.join("paper-1.21.4.jar"), vec![0u8; 1024]).unwrap();
        fs::write(
            dir.join("paper-1.21.4.json"),
            r#"{"java": 21, "build": 100, "flags": ["-XX:+UseG1GC", "-Dx=y"]}"#,
        )
        .unwrap();
        fs::write(dir.join("other.jar"), b"").unwrap();
        assert_eq!(required_java("paper", "1.21.4"), 21);
        assert_eq!(jvm_flags("paper", "1.21.4"), vec!["-XX:+UseG1GC"]);
        assert_eq!(required_java("vanilla", "1.0"), 8);
        // rank needs the network: only the shape is checked
        let list = installed();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["build"], json!(100));
        delete("paper", "1.21.4", &[]).unwrap();
        assert!(!dir.join("paper-1.21.4.json").exists());
        assert!(matches!(
            delete("paper", "1.21.4", &[]),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn deleted_paper_cache_jars_are_ignored() {
        let _t = crate::testutil::data_dir();
        let dir = cfg().jars_dir.clone();
        let server = cfg().server_dir.clone();
        fs::create_dir_all(server.join("cache")).unwrap();
        fs::write(server.join("cache/mojang_1.21.4.jar"), b"x").unwrap();
        fs::write(dir.join("vanilla-1.21.4.jar"), b"x").unwrap();
        delete("vanilla", "1.21.4", &[server.clone()]).unwrap();
        assert!(ignored().contains("1.21.4"));
        assert!(import_paper_cache(&server, &|_| {}).is_empty());
    }
}
