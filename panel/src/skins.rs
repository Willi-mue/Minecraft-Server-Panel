//! Player skins (for the heads on the map and in the panel). Downloaded once per player from Mojang
//! (session server -> textures.minecraft.net) into `DATA_DIR/cache/skins/<uuid>.png`; the viewer and
//! the panel cut out the face. Offline-mode servers have no Mojang profiles: then a plain head.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use base64::Engine;
use regex::Regex;
use serde_json::Value;

use crate::config::cfg;
use crate::error::{Error, Result};
use crate::http;
use crate::instances::Instance;
use crate::util::{lock, mtime_secs, now_secs, read_json, write_atomic};

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{32}$").unwrap());
pub const SKIN_MAX_BYTES: u64 = 64 * 1024;
pub const SKIN_MAX_AGE: f64 = 24.0 * 3600.0; // re-download once a day (players change skins)
pub const SKIN_RETRY: f64 = 3600.0; // after a failure (offline-mode uuid, no network)

pub fn valid_uuid(uuid: &str) -> bool {
    UUID_RE.is_match(uuid)
}

pub fn skin_path(uuid: &str) -> Result<PathBuf> {
    if !valid_uuid(uuid) {
        return Err(Error::Invalid("invalid uuid".into()));
    }
    Ok(cfg()
        .data_dir
        .join("cache")
        .join("skins")
        .join(format!("{uuid}.png")))
}

pub fn is_fresh(path: &std::path::Path) -> bool {
    mtime_secs(path).is_some_and(|m| now_secs() - m < SKIN_MAX_AGE)
}

/// The skin URL of a session server profile, only if it is on Mojang's texture server.
fn skin_url(profile: &Value) -> Option<String> {
    for prop in profile.get("properties")?.as_array()? {
        if prop.get("name").and_then(Value::as_str) != Some("textures") {
            continue;
        }
        let raw = base64::engine::general_purpose::STANDARD
            .decode(prop.get("value")?.as_str()?)
            .ok()?;
        let textures: Value = serde_json::from_slice(&raw).ok()?;
        let url = textures.pointer("/textures/SKIN/url")?.as_str()?;
        let mut u = url::Url::parse(url).ok()?;
        if matches!(u.scheme(), "http" | "https") && u.host_str() == Some("textures.minecraft.net")
        {
            u.set_scheme("https").ok()?;
            return Some(u.to_string());
        }
    }
    None
}

/// Download the skin of a player (uuid without dashes).
pub fn fetch_skin(uuid: &str) -> Result<PathBuf> {
    let path = skin_path(uuid)?;
    let raw = http::get_bytes(
        &format!("https://sessionserver.mojang.com/session/minecraft/profile/{uuid}"),
        16 * 1024,
    )?;
    let profile: Value = serde_json::from_slice(&raw)?;
    let url = skin_url(&profile).ok_or_else(|| Error::Invalid("no skin".into()))?;
    let png = http::get_bytes(&url, SKIN_MAX_BYTES)?;
    if !png.starts_with(crate::servericon::PNG_SIG) {
        return Err(Error::Invalid("not a PNG".into()));
    }
    write_atomic(&path, &png)?;
    Ok(path)
}

// ---------- skins of player heads placed in the world ----------
// mapgen lists the player heads the 3D map shows with their skin's texture hash (<dim>/heads.json);
// the skins are downloaded once from textures.minecraft.net (a hash never changes its picture) into
// DATA_DIR/cache/heads/<hash>.png and served by the map server.

static HASH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{16,128}$").unwrap());
pub const HEADS_PER_RUN: usize = 32; // downloads per render (more follow with the next ones)
static HEAD_FAILED: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);
static HEAD_BUSY: Mutex<()> = Mutex::new(());

pub fn head_skin_path(skin: &str) -> Result<PathBuf> {
    if !HASH_RE.is_match(skin) {
        return Err(Error::Invalid("invalid skin id".into()));
    }
    Ok(cfg()
        .data_dir
        .join("cache")
        .join("heads")
        .join(format!("{skin}.png")))
}

/// Texture hashes of the player heads on a server's map.
pub fn head_skins(inst: &Instance) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(inst.map_dir()) else {
        return out;
    };
    for e in rd.flatten() {
        let Some(Value::Array(items)) = read_json(&e.path().join("heads.json")) else {
            continue;
        };
        for h in items {
            if let Some(skin) = h
                .get("skin")
                .and_then(Value::as_str)
                .filter(|s| HASH_RE.is_match(s))
            {
                out.insert(skin.to_string());
            }
        }
    }
    out
}

/// Download missing head skins (only from textures.minecraft.net, PNG, <= 64 KB); a failed one is
/// retried after an hour. Returns how many were downloaded.
pub fn fetch_head_skins(inst: &Instance, limit: usize) -> usize {
    let Ok(_busy) = HEAD_BUSY.try_lock() else {
        return 0;
    }; // another server's map is on it
    fetch_heads_with(inst, limit, |url| http::get_bytes(url, SKIN_MAX_BYTES))
}

fn fetch_heads_with(inst: &Instance, limit: usize, get: impl Fn(&str) -> Result<Vec<u8>>) -> usize {
    let mut done = 0;
    for skin in head_skins(inst) {
        let Ok(path) = head_skin_path(&skin) else {
            continue;
        };
        let failed = lock(&HEAD_FAILED).get(&skin).copied().unwrap_or(f64::MIN);
        if path.exists() || now_secs() - failed < SKIN_RETRY {
            continue;
        }
        if done >= limit {
            break;
        }
        let result =
            get(&format!("https://textures.minecraft.net/texture/{skin}")).and_then(|png| {
                if !png.starts_with(crate::servericon::PNG_SIG) {
                    return Err(Error::Invalid("not a PNG".into()));
                }
                write_atomic(&path, &png)?;
                Ok(())
            });
        match result {
            Ok(()) => done += 1,
            Err(_) => {
                lock(&HEAD_FAILED).insert(skin, now_secs());
            }
        }
    }
    done
}

/// Name -> uuid from the server's usercache.json; downloads in one background thread.
pub struct Skins {
    inner: Mutex<SkinsInner>,
}

struct SkinsInner {
    cache: HashMap<String, (f64, HashMap<String, String>)>, // server id -> (mtime, names)
    failed: HashMap<String, f64>,
    queue: VecDeque<String>,
    working: bool,
}

pub static SKINS: LazyLock<Skins> = LazyLock::new(|| Skins {
    inner: Mutex::new(SkinsInner {
        cache: HashMap::new(),
        failed: HashMap::new(),
        queue: VecDeque::new(),
        working: false,
    }),
});

impl Skins {
    pub fn uuid_of(&self, name: &str, inst: &Instance) -> Option<String> {
        let path = inst.server_dir().join("usercache.json");
        let mtime = mtime_secs(&path)?;
        let mut g = lock(&self.inner);
        if g.cache.get(&inst.id).map(|c| c.0) != Some(mtime) {
            let mut names = HashMap::new();
            if let Some(Value::Array(entries)) = read_json(&path) {
                for e in entries {
                    if let (Some(n), Some(u)) = (e.get("name"), e.get("uuid")) {
                        let n = n
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| n.to_string());
                        let u = u
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| u.to_string());
                        names.insert(n.to_lowercase(), u.replace('-', "").to_lowercase());
                    }
                }
            }
            g.cache.insert(inst.id.clone(), (mtime, names));
        }
        let uuid = g.cache[&inst.id].1.get(&name.to_lowercase()).cloned()?;
        valid_uuid(&uuid).then_some(uuid)
    }

    /// Queue downloads for these players if their skin is missing or old.
    pub fn want<'a>(&'static self, names: impl IntoIterator<Item = &'a String>, inst: &Instance) {
        let now = now_secs();
        for name in names {
            let Some(uuid) = self.uuid_of(name, inst) else {
                continue;
            };
            let Ok(path) = skin_path(&uuid) else { continue };
            let fresh = is_fresh(&path);
            let mut g = lock(&self.inner);
            if fresh
                || g.queue.contains(&uuid)
                || now - g.failed.get(&uuid).copied().unwrap_or(0.0) < SKIN_RETRY
            {
                continue;
            }
            g.queue.push_back(uuid);
            if !g.working {
                g.working = true;
                std::thread::Builder::new()
                    .name("skins".into())
                    .spawn(move || self.work())
                    .ok();
            }
        }
    }

    fn work(&self) {
        loop {
            let uuid = {
                let mut g = lock(&self.inner);
                match g.queue.front() {
                    Some(u) => u.clone(),
                    None => {
                        g.working = false;
                        return;
                    }
                }
            };
            let failed = fetch_skin(&uuid).is_err();
            let mut g = lock(&self.inner);
            if failed {
                g.failed.insert(uuid.clone(), now_secs());
            }
            g.queue.retain(|u| u != &uuid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_mojang_texture_urls() {
        let enc = |url: &str| {
            let t = json!({"textures": {"SKIN": {"url": url}}}).to_string();
            json!({"properties": [{"name": "textures", "value": base64::engine::general_purpose::STANDARD.encode(t)}]})
        };
        assert_eq!(
            skin_url(&enc("http://textures.minecraft.net/texture/abc")).as_deref(),
            Some("https://textures.minecraft.net/texture/abc")
        );
        assert_eq!(skin_url(&enc("https://evil.example/texture/abc")), None);
        assert!(skin_path("../x").is_err());
        assert!(skin_path(&"a".repeat(32)).is_ok());
    }

    #[test]
    fn head_skins_are_downloaded_once_and_checked() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let skin = "ab".repeat(32);
        let dir = inst.map_dir().join("overworld");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("heads.json"),
            json!([
                {"x": 1, "y": 2, "z": 3, "skin": skin, "rot": 0},
                {"x": 1, "y": 2, "z": 4, "skin": "../../etc/passwd", "rot": 0},
            ])
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            head_skins(&inst).into_iter().collect::<Vec<_>>(),
            vec![skin.clone()]
        );
        let png = [crate::servericon::PNG_SIG, b"rest"].concat();
        let urls = Mutex::new(vec![]);
        let get = |url: &str| {
            lock(&urls).push(url.to_string());
            Ok(png.clone())
        };
        assert_eq!(fetch_heads_with(&inst, 32, get), 1);
        assert_eq!(fetch_heads_with(&inst, 32, get), 0, "already there");
        assert_eq!(
            *lock(&urls),
            vec![format!("https://textures.minecraft.net/texture/{skin}")]
        );
        assert!(head_skin_path("nothex").is_err());

        let other = "cd".repeat(32);
        std::fs::write(dir.join("heads.json"), json!([{"skin": other}]).to_string()).unwrap();
        assert_eq!(fetch_heads_with(&inst, 32, |_| Ok(b"<html>".to_vec())), 0);
        assert!(!head_skin_path(&other).unwrap().exists());
        assert!(
            lock(&HEAD_FAILED).contains_key(&other),
            "retried after an hour, not on every render"
        );
    }

    #[test]
    fn uuid_from_usercache() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        std::fs::create_dir_all(inst.server_dir()).unwrap();
        std::fs::write(
            inst.server_dir().join("usercache.json"),
            r#"[{"name": "Steve", "uuid": "8667ba71-b85a-4004-af54-457a9734eed7"}]"#,
        )
        .unwrap();
        assert_eq!(
            SKINS.uuid_of("steve", &inst).as_deref(),
            Some("8667ba71b85a4004af54457a9734eed7")
        );
        assert_eq!(SKINS.uuid_of("alex", &inst), None);
    }
}
