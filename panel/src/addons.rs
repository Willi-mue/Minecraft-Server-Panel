//! Mods (Fabric) and plugins (Paper) of one server: `<server>/mods/` resp. `<server>/plugins/`.
//!
//! - Metadata comes from the jar itself: `fabric.mod.json` (incl. jar-in-jar modules, e.g. Fabric
//!   API) resp. `plugin.yml` / `paper-plugin.yml`. Missing dependencies and a wrong Minecraft version
//!   are flagged.
//! - Disabling renames `x.jar` to `x.jar.disabled` (neither loader picks that up).
//! - Search, install (with required dependencies) and updates go through the Modrinth API; every
//!   download is checked against Modrinth's SHA-512 and must come from Modrinth's CDN.
//!
//! Changes on disk apply on the next server start.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{Map, Value, json};
use zip::ZipArchive;

use crate::error::{Error, Result};
use crate::http::{self, Hash};
use crate::instances::Instance;
use crate::util::{lock, strip_bom};
use crate::{invalid, not_found};

const MODRINTH: &str = "https://api.modrinth.com/v2";
pub const MAX_UPLOAD: u64 = 100 * 1024 * 1024;
pub const DISABLED: &str = ".disabled";
static FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w\-. +()\[\]!,'&]{1,120}\.jar$").unwrap());
static PROJECT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_\-]{1,64}$").unwrap());
const CDN_HOSTS: [&str; 2] = ["cdn.modrinth.com", "cdn-raw.modrinth.com"];
// Provided by the Fabric loader itself
const FABRIC_BUILTIN: [&str; 5] = [
    "minecraft",
    "java",
    "fabricloader",
    "fabric-loader",
    "mixinextras",
];

static META_CACHE: LazyLock<Mutex<HashMap<(PathBuf, u128, u64), Option<Value>>>> =
    LazyLock::new(Default::default);

/// "mods" (Fabric), "plugins" (Paper) or None (Vanilla has neither).
pub fn kind_of(flavor: &str) -> Option<&'static str> {
    match flavor {
        "fabric" => Some("mods"),
        "paper" => Some("plugins"),
        _ => None,
    }
}

fn loaders(kind: &str) -> Vec<&'static str> {
    if kind == "mods" {
        vec!["fabric"]
    } else {
        vec!["paper", "spigot", "bukkit"]
    }
}

pub fn folder(inst: &Instance, kind: &str) -> Result<PathBuf> {
    if kind != "mods" && kind != "plugins" {
        return invalid!("Unknown add-on kind: {kind}");
    }
    Ok(inst.server_dir().join(kind))
}

/// "x.jar" or "x.jar.disabled" (no paths, no hidden files).
pub fn validate_file(name: &str) -> Result<String> {
    let base = name.strip_suffix(DISABLED).unwrap_or(name);
    if !FILE_RE.is_match(base) || base.starts_with('.') {
        return invalid!("Invalid file name (expected a .jar)");
    }
    Ok(name.to_string())
}

pub fn path_of(inst: &Instance, kind: &str, name: &str) -> Result<PathBuf> {
    let path = folder(inst, kind)?.join(validate_file(name)?);
    if !path.is_file() {
        return not_found!("{name} is not installed");
    }
    Ok(path)
}

// ---------- metadata ----------

/// JSON like Python's `json.loads(strict=False)`: raw control characters inside strings are allowed.
fn lenient_json(raw: &[u8]) -> Option<Value> {
    let raw = strip_bom(raw);
    if let Ok(v) = serde_json::from_slice(raw) {
        return Some(v);
    }
    let text = String::from_utf8_lossy(raw);
    let mut out = String::with_capacity(text.len());
    let (mut in_str, mut escaped) = (false, false);
    for c in text.chars() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            } else if (c as u32) < 0x20 {
                out.push_str(&format!("\\u{:04x}", c as u32));
                continue;
            }
        } else if c == '"' {
            in_str = true;
        }
        out.push(c);
    }
    serde_json::from_str(&out).ok()
}

fn yaml_scalar(text: &str) -> Value {
    let text = text.trim();
    let b = text.as_bytes();
    if b.len() >= 2 && b[0] == b[b.len() - 1] && (b[0] == b'\'' || b[0] == b'"') {
        return json!(&text[1..text.len() - 1]);
    }
    if text.starts_with('[') && text.ends_with(']') {
        return Value::Array(
            text[1..text.len() - 1]
                .split(',')
                .filter(|x| !x.trim().is_empty())
                .map(yaml_scalar)
                .collect(),
        );
    }
    json!(text)
}

/// The small part of YAML that plugin.yml files use: top-level `key: value`, flow lists `[a, b]`,
/// block lists (`- a`), block text (`|`, `>`) and nested maps (paper-plugin.yml `dependencies`).
pub fn parse_plugin_yml(text: &str) -> Map<String, Value> {
    static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+#.*$").unwrap());
    let mut result: Map<String, Value> = Map::new();
    let mut key: Option<String> = None;
    // open nested maps below the current top-level key: (indent, path of keys inside result[key])
    let mut stack: Vec<(usize, Vec<String>)> = vec![];
    for raw in text.lines() {
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        let line = COMMENT.replace(raw.trim_end(), "").to_string();
        let indent = line.len() - line.trim_start().len();
        let body = line.trim().to_string();
        if indent == 0 && !body.starts_with('-') {
            let (k, v) = body.split_once(':').unwrap_or((&body, ""));
            let (k, v) = (k.trim().to_string(), v.trim());
            stack.clear();
            let value = if ["|", ">", "|-", ">-", "|+", ">+"].contains(&v) {
                json!("")
            } else if v.is_empty() {
                Value::Null
            } else {
                yaml_scalar(v)
            };
            result.insert(k.clone(), value);
            key = Some(k);
            continue;
        }
        let Some(k) = key.clone() else { continue };
        let cur = result.get_mut(&k).unwrap();
        if body == "-" || body.starts_with("- ") {
            if cur.is_null() {
                *cur = json!([]);
            }
            if let Value::Array(a) = cur {
                a.push(yaml_scalar(&body[1..]));
            }
            continue;
        }
        if let Value::String(s) = cur {
            // block text / multi-line scalar continues
            *s = format!("{s} {body}").trim().to_string();
            continue;
        }
        if cur.is_null() {
            *cur = json!({});
        }
        if !cur.is_object() {
            continue;
        }
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let path = stack.last().map(|s| s.1.clone()).unwrap_or_default();
        let mut parent = cur;
        for p in &path {
            parent = parent.get_mut(p).unwrap();
        }
        let (k2, v) = body.split_once(':').unwrap_or((&body, ""));
        let k2 = k2.trim().trim_matches(['\'', '"']).to_string();
        if !v.trim().is_empty() {
            parent[&k2] = yaml_scalar(v);
        } else {
            parent[&k2] = json!({});
            let mut p = path.clone();
            p.push(k2);
            stack.push((indent, p));
        }
    }
    for v in result.values_mut() {
        if v.is_null() {
            *v = json!([]);
        }
    }
    result
}

fn authors(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::String(s)) => vec![json!(s)],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(json!(s)),
                Value::Object(o) => Some(json!(match o.get("name") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                })),
                _ => None,
            })
            .take(5)
            .collect(),
        _ => vec![],
    }
}

fn read_entry<R: Read + Seek>(zf: &mut ZipArchive<R>, name: &str) -> Option<Vec<u8>> {
    let f = zf.by_name(name).ok()?;
    let mut buf = Vec::new();
    f.take(64 << 20).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn names<R: Read + Seek>(zf: &ZipArchive<R>) -> HashSet<String> {
    zf.file_names()
        .filter_map(|n| n.ok().map(|n| n.to_string()))
        .collect()
}

/// Mod id + `provides` + ids of bundled jar-in-jar mods (Fabric API is a bundle of modules).
fn fabric_ids<R: Read + Seek>(
    meta: &Value,
    zf: &mut ZipArchive<R>,
    depth: usize,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    ids.insert(str_or_empty(meta.get("id")));
    for p in meta
        .get("provides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(p) = p.as_str() {
            ids.insert(p.to_string());
        }
    }
    if depth < 2 {
        for entry in meta
            .get("jars")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(inner) = entry.get("file").and_then(Value::as_str) else {
                continue;
            };
            let Some(bytes) = read_entry(zf, inner) else {
                continue;
            };
            let Ok(mut nested) = ZipArchive::new(Cursor::new(bytes)) else {
                continue;
            };
            let Some(nested_meta) =
                read_entry(&mut nested, "fabric.mod.json").and_then(|r| lenient_json(&r))
            else {
                continue;
            };
            if nested_meta.is_object() {
                ids.extend(fabric_ids(&nested_meta, &mut nested, depth + 1));
            }
        }
    }
    ids.retain(|i| !i.is_empty());
    ids
}

fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn truthy_str(v: Option<&Value>) -> Option<String> {
    let s = str_or_empty(v);
    (!s.is_empty()).then_some(s)
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn read_meta_uncached(path: &Path) -> Option<Value> {
    let mut zf = ZipArchive::new(fs::File::open(path).ok()?).ok()?;
    let all = names(&zf);
    if all.contains("fabric.mod.json") {
        let m = lenient_json(&read_entry(&mut zf, "fabric.mod.json")?)?;
        if !m.is_object() {
            return None;
        }
        let depends: Map<String, Value> = m
            .get("depends")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let name = truthy_str(m.get("name"))
            .or_else(|| truthy_str(m.get("id")))
            .unwrap_or_else(|| stem(path));
        let provides: Vec<String> = fabric_ids(&m, &mut zf, 0).into_iter().collect();
        return Some(json!({
            "type": "fabric",
            "id": str_or_empty(m.get("id")),
            "name": name,
            "version": str_or_empty(m.get("version")),
            "description": str_or_empty(m.get("description")).chars().take(300).collect::<String>(),
            "authors": authors(m.get("authors")),
            "environment": m.get("environment").map(|e| str_or_empty(Some(e))).unwrap_or_else(|| "*".into()),
            "depends": depends,
            "provides": provides,
        }));
    }
    for (yml, paper) in [("paper-plugin.yml", true), ("plugin.yml", false)] {
        if !all.contains(yml) {
            continue;
        }
        let raw = read_entry(&mut zf, yml)?;
        let y = parse_plugin_yml(&String::from_utf8_lossy(strip_bom(&raw)));
        let depends: Vec<String> = if paper {
            match y
                .get("dependencies")
                .and_then(|d| d.get("server"))
                .and_then(Value::as_object)
            {
                Some(server) => server
                    .iter()
                    .filter(|(_, v)| match v {
                        Value::Object(o) => {
                            str_or_empty(o.get("required")).to_lowercase() != "false"
                                || o.get("required").is_none()
                        }
                        _ => true,
                    })
                    .map(|(n, _)| n.clone())
                    .collect(),
                None => vec![],
            }
        } else {
            y.get("depend")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|d| str_or_empty(Some(d))).collect())
                .unwrap_or_default()
        };
        let name = truthy_str(y.get("name")).unwrap_or_else(|| stem(path));
        let mut provides = vec![name.clone()];
        if let Some(Value::Array(p)) = y.get("provides") {
            provides.extend(p.iter().map(|x| str_or_empty(Some(x))));
        }
        let author = y
            .get("authors")
            .filter(|a| !matches!(a, Value::Array(v) if v.is_empty()))
            .or_else(|| y.get("author"));
        return Some(json!({
            "type": "paper",
            "id": name,
            "name": name,
            "version": str_or_empty(y.get("version")),
            "description": str_or_empty(y.get("description")).chars().take(300).collect::<String>(),
            "authors": authors(author),
            "api_version": str_or_empty(y.get("api-version")),
            "depends": depends,
            "provides": provides,
        }));
    }
    if all.contains("META-INF/mods.toml") || all.contains("META-INF/neoforge.mods.toml") {
        return Some(json!({"type": "forge", "id": "", "name": stem(path)}));
    }
    if all.contains("quilt.mod.json") {
        return Some(json!({"type": "quilt", "id": "", "name": stem(path)}));
    }
    None
}

pub fn read_meta(path: &Path) -> Option<Value> {
    let m = fs::metadata(path).ok()?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let key = (path.to_path_buf(), mtime, m.len());
    if let Some(hit) = lock(&META_CACHE).get(&key) {
        return hit.clone();
    }
    let meta = read_meta_uncached(path);
    let mut cache = lock(&META_CACHE);
    if cache.len() > 500 {
        cache.clear();
    }
    cache.insert(key, meta.clone());
    meta
}

// ---------- version checks ----------

fn nums(version: &str) -> Option<Vec<u64>> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^v?(\d+(?:\.\d+)*)").unwrap());
    let c = RE.captures(version.trim())?;
    c[1].split('.').map(|x| x.parse().ok()).collect()
}

fn cmp(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    let get = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    (0..n)
        .map(|i| get(a, i).cmp(&get(b, i)))
        .find(|o| *o != Ordering::Equal)
        .unwrap_or(Ordering::Equal)
}

/// One Fabric version predicate (`>=1.21`, `~1.21.4`, `1.21.x`, ...). None = can't tell.
fn matches_one(version: &[u64], constraint: &str) -> Option<bool> {
    static OP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(>=|<=|>|<|=|~|\^)?\s*(.+)$").unwrap());
    static WILD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*)\.[xX*]").unwrap());
    let c = OP.captures(constraint.trim())?;
    let op = c.get(1).map(|m| m.as_str()).unwrap_or("=");
    let value = c.get(2)?.as_str();
    if matches!(value, "*" | "x" | "X") {
        return Some(true);
    }
    if let Some(w) = WILD.captures(value) {
        let prefix: Vec<u64> = w[1].split('.').filter_map(|x| x.parse().ok()).collect();
        return (op == "=")
            .then(|| version.len() >= prefix.len() && version[..prefix.len()] == prefix[..]);
    }
    let target = nums(value)?;
    let c = cmp(version, &target);
    let starts =
        |prefix: &[u64]| version.len() >= prefix.len() && version[..prefix.len()] == *prefix;
    Some(match op {
        "=" => c == Ordering::Equal,
        ">=" => c != Ordering::Less,
        "<=" => c != Ordering::Greater,
        ">" => c == Ordering::Greater,
        // "<1.21-" excludes pre-releases of 1.21 too: same as < 1.21 for releases
        "<" => c == Ordering::Less,
        "~" => c != Ordering::Less && starts(&target[..target.len().min(2)]), // same minor
        "^" => c != Ordering::Less && starts(&target[..target.len().min(1)]), // same major
        _ => return None,
    })
}

/// Fabric dependency spec (string or list = any of) against a Minecraft version. None if either side
/// can't be parsed (snapshots): then nothing is flagged.
pub fn matches(version: &str, spec: &Value) -> Option<bool> {
    static PLAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(\.\d+)*$").unwrap());
    let v = nums(version)?;
    if !PLAIN.is_match(version.trim()) {
        return None;
    }
    let specs: Vec<&Value> = match spec {
        Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    let mut results = vec![];
    for s in specs {
        let Some(s) = s.as_str() else { continue };
        let parts: Vec<Option<bool>> = s.split_whitespace().map(|p| matches_one(&v, p)).collect();
        let parts = if parts.is_empty() {
            vec![Some(true)]
        } else {
            parts
        };
        results.push(if parts.contains(&Some(false)) {
            Some(false)
        } else if parts.iter().all(|p| *p == Some(true)) {
            Some(true)
        } else {
            None
        });
    }
    if results.contains(&Some(true)) {
        return Some(true);
    }
    if !results.is_empty() && results.iter().all(|r| *r == Some(false)) {
        return Some(false);
    }
    None
}

// ---------- listing ----------

fn files(inst: &Instance, kind: &str) -> Vec<PathBuf> {
    let Ok(base) = folder(inst, kind) else {
        return vec![];
    };
    let Ok(rd) = fs::read_dir(&base) else {
        return vec![];
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| validate_file(n).is_ok())
        })
        .collect();
    out.sort_by_key(|p| {
        p.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    });
    out
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|x| str_or_empty(Some(x))).collect())
        .unwrap_or_default()
}

pub fn list_addons(inst: &Instance, kind: &str, game_version: &str) -> Vec<Value> {
    let items: Vec<(String, bool, u64, Option<Value>)> = files(inst, kind)
        .iter()
        .map(|p| {
            let name = file_name(p);
            let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            (
                name.clone(),
                !name.ends_with(DISABLED),
                (size as f64 / 1024.0).round() as u64,
                read_meta(p),
            )
        })
        .collect();
    let want = if kind == "mods" { "fabric" } else { "paper" };
    let mut provided: HashSet<String> = if kind == "mods" {
        FABRIC_BUILTIN.iter().map(|s| s.to_string()).collect()
    } else {
        HashSet::new()
    };
    for (_, enabled, _, meta) in &items {
        if let Some(m) = meta.as_ref().filter(|m| *enabled && m["type"] == want) {
            provided.extend(strs(&m["provides"]).iter().map(|x| x.to_lowercase()));
            provided.insert(str_or_empty(m.get("id")).to_lowercase());
        }
    }
    let mut result = vec![];
    for (file, enabled, size_kb, meta) in items {
        let m = meta.unwrap_or(json!({}));
        let mtype = m.get("type").and_then(Value::as_str).unwrap_or("");
        let mut problems: Vec<&str> = vec![];
        if mtype.is_empty() {
            problems.push("No mod/plugin metadata found");
        } else if mtype != want {
            problems.push(match mtype {
                "fabric" => "This is a Fabric mod, not a plugin",
                "paper" => "This is a plugin, not a Fabric mod",
                "forge" => "Forge/NeoForge mod: doesn't run on this server",
                _ => "Quilt mod: doesn't run on a Fabric server",
            });
        }
        let mut missing: Vec<String> = vec![];
        let mut incompatible: Option<String> = None;
        if mtype == want && want == "fabric" {
            let deps = m["depends"].as_object().cloned().unwrap_or_default();
            missing = deps
                .keys()
                .filter(|d| !provided.contains(&d.to_lowercase()))
                .cloned()
                .collect();
            if let Some(spec) = deps.get("minecraft") {
                if !game_version.is_empty() && matches(game_version, spec) == Some(false) {
                    let text = match spec {
                        Value::Array(a) => a
                            .iter()
                            .map(|x| str_or_empty(Some(x)))
                            .collect::<Vec<_>>()
                            .join(" or "),
                        other => str_or_empty(Some(other)),
                    };
                    incompatible = Some(format!("Made for Minecraft {text}"));
                }
            }
        } else if mtype == want && want == "paper" {
            missing = strs(&m["depends"])
                .into_iter()
                .filter(|d| !provided.contains(&d.to_lowercase()))
                .collect();
            let api_version = str_or_empty(m.get("api_version"));
            if let (Some(api), Some(gv)) = (nums(&api_version), nums(game_version)) {
                if cmp(&api, &gv[..gv.len().min(api.len())]) == Ordering::Greater {
                    incompatible = Some(format!("Needs Paper {api_version} or newer"));
                }
            }
        }
        missing.sort();
        let side = (mtype == "fabric").then(|| {
            match m.get("environment").and_then(Value::as_str).unwrap_or("*") {
                "server" => "server",
                "client" => "client",
                _ => "both",
            }
        });
        result.push(json!({
            "file": file,
            "enabled": enabled,
            "size_kb": size_kb,
            "id": str_or_empty(m.get("id")),
            "name": truthy_str(m.get("name")).unwrap_or_else(|| file.clone()),
            "version": str_or_empty(m.get("version")),
            "description": str_or_empty(m.get("description")),
            "authors": m.get("authors").cloned().unwrap_or(json!([])),
            // Fabric: "*" = players need it too, "server" = server only, "client" = useless on a server
            "side": side,
            "missing": missing,
            "incompatible": incompatible,
            "problems": problems,
        }));
    }
    result
}

// ---------- local changes ----------

/// Check an uploaded jar and move it into the folder (replacing a file of the same name).
pub fn add_upload(inst: &Instance, kind: &str, name: &str, src: &Path) -> Result<String> {
    validate_file(name)?;
    if name.ends_with(DISABLED) {
        return invalid!("Upload a .jar file");
    }
    let Some(meta) = read_meta_uncached(src) else {
        if fs::File::open(src)
            .ok()
            .and_then(|f| ZipArchive::new(f).ok())
            .is_none()
        {
            return invalid!("Not a valid .jar file");
        }
        return invalid!(
            "No fabric.mod.json / plugin.yml in the jar: not a Fabric mod or Paper plugin"
        );
    };
    let want = if kind == "mods" { "fabric" } else { "paper" };
    if meta["type"] != want {
        return invalid!(
            "{name} is not a {}",
            if kind == "mods" {
                "Fabric mod"
            } else {
                "Paper/Bukkit plugin"
            }
        );
    }
    let dir = folder(inst, kind)?;
    fs::create_dir_all(&dir)?;
    let target = dir.join(name);
    let _ = fs::remove_file(dir.join(format!("{name}{DISABLED}")));
    if fs::rename(src, &target).is_err() {
        fs::copy(src, &target)?;
        let _ = fs::remove_file(src);
    }
    Ok(name.to_string())
}

/// Rename to/from .jar.disabled. Returns the new file name.
pub fn set_enabled(inst: &Instance, kind: &str, name: &str, enabled: bool) -> Result<String> {
    let path = path_of(inst, kind, name)?;
    let base = name.strip_suffix(DISABLED).unwrap_or(name);
    let new = if enabled {
        base.to_string()
    } else {
        format!("{base}{DISABLED}")
    };
    if new == name {
        return Ok(new);
    }
    let target = path.with_file_name(&new);
    if target.exists() {
        return Err(Error::Exists(format!("{new} exists already")));
    }
    fs::rename(&path, &target)?;
    Ok(new)
}

pub fn delete(inst: &Instance, kind: &str, name: &str) -> Result<()> {
    fs::remove_file(path_of(inst, kind, name)?)?;
    Ok(())
}

// ---------- Modrinth ----------

fn modrinth(e: Error) -> Error {
    match e {
        Error::Status(code, _) => Error::Remote(format!("Modrinth answered {code}")),
        Error::Remote(m) => Error::Remote(format!("Could not reach Modrinth: {m}")),
        other => other,
    }
}

fn api_get(path: &str) -> Result<Value> {
    http::get_json(&format!("{MODRINTH}{path}")).map_err(modrinth)
}

fn api_post(path: &str, body: &Value) -> Result<Value> {
    http::post_json(&format!("{MODRINTH}{path}"), body)
}

pub fn validate_project(project_id: &str) -> Result<&str> {
    if !PROJECT_RE.is_match(project_id) {
        return invalid!("Invalid project id");
    }
    Ok(project_id)
}

pub fn search(kind: &str, game_version: &str, query: &str, offset: i64) -> Result<Value> {
    let query: String = query.trim().chars().take(100).collect();
    let mut facets: Vec<Vec<String>> = vec![vec![
        "server_side:required".into(),
        "server_side:optional".into(),
    ]];
    if kind == "mods" {
        facets.push(vec!["project_type:mod".into()]);
        facets.push(vec!["categories:fabric".into()]);
    } else {
        facets.push(
            loaders("plugins")
                .iter()
                .map(|l| format!("categories:{l}"))
                .collect(),
        );
    }
    if !game_version.is_empty() {
        facets.push(vec![format!("versions:{game_version}")]);
    }
    let params = http::query(&[
        ("query", query.clone()),
        ("facets", serde_json::to_string(&facets).unwrap_or_default()),
        ("limit", "20".into()),
        ("offset", offset.max(0).to_string()),
        (
            "index",
            if query.is_empty() {
                "downloads"
            } else {
                "relevance"
            }
            .into(),
        ),
    ]);
    let data = api_get(&format!("/search?{params}"))?;
    let hits: Vec<Value> = data["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| {
            json!({
                "project_id": h.get("project_id"), "slug": h.get("slug"), "title": h.get("title"),
                "description": str_or_empty(h.get("description")).chars().take(200).collect::<String>(),
                "author": h.get("author"), "downloads": h.get("downloads").cloned().unwrap_or(json!(0)),
                "icon_url": h.get("icon_url").filter(|u| u.as_str().is_some_and(|s| !s.is_empty())),
                "client_side": h.get("client_side"), "server_side": h.get("server_side"),
            })
        })
        .collect();
    Ok(json!({"total": data.get("total_hits").cloned().unwrap_or(json!(0)), "hits": hits}))
}

fn best_version(project_id: &str, kind: &str, game_version: &str) -> Result<Value> {
    let mut params = vec![(
        "loaders",
        serde_json::to_string(&loaders(kind)).unwrap_or_default(),
    )];
    if !game_version.is_empty() {
        params.push((
            "game_versions",
            serde_json::to_string(&[game_version]).unwrap_or_default(),
        ));
    }
    let versions = api_get(&format!(
        "/project/{}/version?{}",
        http::quote(validate_project(project_id)?),
        http::query(&params)
    ))?;
    let mut versions = versions.as_array().cloned().unwrap_or_default();
    if versions.is_empty() {
        return not_found!(
            "No version for Minecraft {} on {}",
            if game_version.is_empty() {
                "?"
            } else {
                game_version
            },
            loaders(kind).join(", ")
        );
    }
    // Releases first, newest first (the API already sorts by date)
    versions.sort_by_key(|v| v["version_type"] != "release");
    Ok(versions.swap_remove(0))
}

fn primary(version: &Value) -> Result<Value> {
    let files = version["files"].as_array().cloned().unwrap_or_default();
    files
        .iter()
        .find(|f| f["primary"].as_bool() == Some(true))
        .or(files.first())
        .cloned()
        .ok_or_else(|| Error::Invalid("Modrinth version has no files".into()))
}

fn sha512(path: &Path) -> String {
    http::file_hash::<sha2::Sha512>(path).unwrap_or_default()
}

/// Download a Modrinth file into the folder, SHA-512 verified. Returns the file name.
fn download(file: &Value, target_dir: &Path, disabled: bool) -> Result<String> {
    let name = validate_file(file["filename"].as_str().unwrap_or(""))?;
    let url = file["url"].as_str().unwrap_or("");
    let parsed = url::Url::parse(url)
        .map_err(|_| Error::Invalid(format!("Unexpected download host: {url}")))?;
    if parsed.scheme() != "https" || !CDN_HOSTS.contains(&parsed.host_str().unwrap_or("")) {
        return invalid!(
            "Unexpected download host: {}",
            parsed.host_str().unwrap_or("")
        );
    }
    let Some(expected) = file["hashes"]["sha512"].as_str() else {
        return invalid!("Modrinth gave no SHA-512 for this file");
    };
    fs::create_dir_all(target_dir)?;
    let final_name = format!("{name}{}", if disabled { DISABLED } else { "" });
    http::download(
        url,
        &target_dir.join(&final_name),
        Some(&target_dir.join(format!(".{name}.part"))),
        Hash::Sha512(expected.into()),
        MAX_UPLOAD,
    )
    .map_err(modrinth)?;
    Ok(final_name)
}

/// Install the best matching version of a project plus its required dependencies.
pub fn install(
    inst: &Instance,
    kind: &str,
    game_version: &str,
    project_id: &str,
    log: &(dyn Fn(&str) + Sync),
) -> Result<Vec<String>> {
    let base = folder(inst, kind)?;
    let local_files = files(inst, kind);
    let local_hashes: HashSet<String> = local_files.iter().map(|p| sha512(p)).collect();
    let local_ids: HashSet<String> = local_files
        .iter()
        .map(|p| {
            read_meta(p)
                .map(|m| str_or_empty(m.get("id")).to_lowercase())
                .unwrap_or_default()
        })
        .collect();
    let mut installed = vec![];
    let mut seen = HashSet::new();
    let mut todo: Vec<(String, usize)> = vec![(validate_project(project_id)?.to_string(), 0)];
    while let Some((pid, depth)) = todo.pop() {
        if seen.contains(&pid) || depth > 4 {
            continue;
        }
        seen.insert(pid.clone());
        let version = best_version(&pid, kind, game_version)?;
        let file = primary(&version)?;
        if file["hashes"]["sha512"]
            .as_str()
            .is_some_and(|h| local_hashes.contains(h))
        {
            continue; // exactly this file is there already
        }
        if depth > 0 && !local_ids.is_empty() {
            let project = api_get(&format!(
                "/project/{}",
                http::quote(validate_project(&pid)?)
            ))?;
            if local_ids.contains(&str_or_empty(project.get("slug")).to_lowercase()) {
                continue; // dependency installed under another file (e.g. uploaded by hand)
            }
        }
        let name = download(&file, &base, false)?;
        log(&format!("Installed {name}"));
        installed.push(name);
        let deps: Vec<String> = version["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["dependency_type"] == "required")
            .filter_map(|d| d["project_id"].as_str().map(str::to_string))
            .collect();
        for dep in deps.into_iter().rev() {
            todo.push((dep, depth + 1));
        }
    }
    Ok(installed)
}

fn update_query(hashes: Vec<String>, kind: &str, game_version: &str) -> Value {
    let mut body = json!({"hashes": hashes, "algorithm": "sha512", "loaders": loaders(kind)});
    if !game_version.is_empty() {
        body["game_versions"] = json!([game_version]);
    }
    body
}

/// {file: {"version": "1.2.3", "filename": ...}} for files Modrinth has a newer version of.
pub fn check_updates(inst: &Instance, kind: &str, game_version: &str) -> Result<Value> {
    let local: Vec<(String, String)> = files(inst, kind)
        .iter()
        .map(|p| (file_name(p), sha512(p)))
        .collect();
    if local.is_empty() {
        return Ok(json!({}));
    }
    let body = update_query(
        local.iter().map(|(_, h)| h.clone()).collect(),
        kind,
        game_version,
    );
    let latest = match api_post("/version_files/update", &body) {
        Ok(v) => v,
        Err(Error::Status(404, _)) => return Ok(json!({})),
        Err(e) => return Err(modrinth(e)),
    };
    let mut result = Map::new();
    for (name, h) in local {
        let Some(v) = latest.get(&h).filter(|v| v.is_object()) else {
            continue;
        };
        let f = primary(v)?;
        if f["hashes"]["sha512"].as_str() != Some(h.as_str()) {
            result.insert(name, json!({"version": str_or_empty(v.get("version_number")), "filename": f.get("filename")}));
        }
    }
    Ok(Value::Object(result))
}

/// Replace one file with Modrinth's newest compatible version. Returns the new file name.
pub fn update(
    inst: &Instance,
    kind: &str,
    game_version: &str,
    name: &str,
    log: &(dyn Fn(&str) + Sync),
) -> Result<String> {
    let path = path_of(inst, kind, name)?;
    let h = sha512(&path);
    let answer = api_post(
        "/version_files/update",
        &update_query(vec![h.clone()], kind, game_version),
    )
    .map_err(modrinth)?;
    let Some(v) = answer.get(&h).filter(|v| v.is_object()) else {
        return not_found!("Modrinth knows no newer version of {name}");
    };
    let f = primary(v)?;
    if f["hashes"]["sha512"].as_str() == Some(h.as_str()) {
        return Ok(name.to_string());
    }
    let new = download(
        &f,
        path.parent().unwrap_or(Path::new(".")),
        name.ends_with(DISABLED),
    )?;
    if new != name {
        let _ = fs::remove_file(&path);
    }
    let number = truthy_str(v.get("version_number")).unwrap_or_else(|| new.clone());
    log(&format!("Updated {name} to {number}"));
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn jar(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in files {
            zw.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(content).unwrap();
        }
        zw.finish().unwrap().into_inner()
    }

    fn fmod(id: &str, depends: Value, env: &str, nested: &[&str]) -> Vec<u8> {
        let mut meta = json!({"schemaVersion": 1, "id": id, "version": "1.0", "name": id, "environment": env, "depends": depends});
        let mut files = vec![];
        if !nested.is_empty() {
            meta["jars"] = json!(
                nested
                    .iter()
                    .map(|n| json!({"file": format!("META-INF/jars/{n}.jar")}))
                    .collect::<Vec<_>>()
            );
            for n in nested {
                let inner = jar(&[(
                    "fabric.mod.json",
                    json!({"schemaVersion": 1, "id": n, "version": "1"})
                        .to_string()
                        .into_bytes(),
                )]);
                files.push((format!("META-INF/jars/{n}.jar"), inner));
            }
        }
        let mut all: Vec<(&str, Vec<u8>)> =
            vec![("fabric.mod.json", meta.to_string().into_bytes())];
        for (n, b) in &files {
            all.push((n.as_str(), b.clone()));
        }
        jar(&all)
    }

    fn put(kind: &str, name: &str, content: &[u8]) {
        let d = crate::config::cfg().server_dir.join(kind);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(name), content).unwrap();
    }

    #[test]
    fn plugin_yml_subset() {
        let y = parse_plugin_yml(
            "name: Essentials\nversion: '2.20.1'\napi-version: \"1.20\"\nauthors: [a, b]\n\
             depend: [Vault]\nsoftdepend:\n  - LuckPerms\ndescription: >\n  Line one\n  line two\n\
             dependencies:\n  server:\n    Vault:\n      required: true\n    Opt:\n      required: false\n",
        );
        assert_eq!(y["name"], json!("Essentials"));
        assert_eq!(y["version"], json!("2.20.1"));
        assert_eq!(y["api-version"], json!("1.20"));
        assert_eq!(y["authors"], json!(["a", "b"]));
        assert_eq!(y["depend"], json!(["Vault"]));
        assert_eq!(y["softdepend"], json!(["LuckPerms"]));
        assert_eq!(y["description"], json!("Line one line two"));
        assert_eq!(
            y["dependencies"]["server"]["Opt"],
            json!({"required": "false"})
        );
    }

    #[test]
    fn fabric_version_ranges() {
        let cases: Vec<(&str, Value, Option<bool>)> = vec![
            ("1.21.4", json!(">=1.21"), Some(true)),
            ("1.21.4", json!("~1.21.4"), Some(true)),
            ("1.21.4", json!("1.21.x"), Some(true)),
            ("1.20.1", json!(">=1.21"), Some(false)),
            ("1.21.4", json!([">=1.20 <1.21", "1.21.4"]), Some(true)),
            ("1.21.4", json!("~1.20"), Some(false)),
            ("1.21.4", json!(">=1.21- <1.22-"), Some(true)),
            ("25w14a", json!(">=1.21"), None),
            ("1.21.4", json!("*"), Some(true)),
        ];
        for (v, spec, expected) in cases {
            assert_eq!(matches(v, &spec), expected, "{v} {spec}");
        }
    }

    #[test]
    fn list_flags_missing_dependency_wrong_version_and_side() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        put(
            "mods",
            "fabric-api.jar",
            &fmod(
                "fabric-api",
                json!({}),
                "*",
                &["fabric-api-base", "fabric-networking-api-v1"],
            ),
        );
        put(
            "mods",
            "sodium.jar",
            &fmod(
                "sodium",
                json!({"minecraft": "~1.20.1", "fabric-api-base": "*"}),
                "client",
                &[],
            ),
        );
        put(
            "mods",
            "lithium.jar",
            &fmod(
                "lithium",
                json!({"minecraft": ">=1.21", "fabricloader": ">=0.16", "cloth-config": "*"}),
                "*",
                &[],
            ),
        );
        put(
            "mods",
            "old.jar.disabled",
            &fmod("old", json!({"minecraft": "1.21.4"}), "server", &[]),
        );
        let items: HashMap<String, Value> = list_addons(&inst, "mods", "1.21.4")
            .into_iter()
            .map(|i| (i["id"].as_str().unwrap().to_string(), i))
            .collect();
        assert_eq!(items["sodium"]["missing"], json!([]));
        assert_eq!(
            items["sodium"]["incompatible"],
            json!("Made for Minecraft ~1.20.1")
        );
        assert_eq!(items["sodium"]["side"], json!("client"));
        assert_eq!(items["lithium"]["missing"], json!(["cloth-config"]));
        assert_eq!(items["lithium"]["incompatible"], Value::Null);
        assert_eq!(items["lithium"]["side"], json!("both"));
        assert_eq!(items["old"]["enabled"], json!(false));
        assert_eq!(items["old"]["side"], json!("server"));
    }

    #[test]
    fn plugins_and_toggles() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let yml = "name: Shop\nversion: 1.0\napi-version: '1.22'\ndepend: [Vault]\nauthor: me\n";
        put(
            "plugins",
            "shop.jar",
            &jar(&[("plugin.yml", yml.as_bytes().to_vec())]),
        );
        put(
            "plugins",
            "forge.jar",
            &jar(&[("META-INF/mods.toml", b"x".to_vec())]),
        );
        let items = list_addons(&inst, "plugins", "1.21.4");
        let forge = items.iter().find(|i| i["file"] == "forge.jar").unwrap();
        assert_eq!(
            forge["problems"],
            json!(["Forge/NeoForge mod: doesn't run on this server"])
        );
        let shop = items.iter().find(|i| i["file"] == "shop.jar").unwrap();
        assert_eq!(shop["missing"], json!(["Vault"]));
        assert_eq!(shop["incompatible"], json!("Needs Paper 1.22 or newer"));
        assert_eq!(shop["authors"], json!(["me"]));
        assert_eq!(shop["side"], Value::Null);

        assert_eq!(
            set_enabled(&inst, "plugins", "shop.jar", false).unwrap(),
            "shop.jar.disabled"
        );
        assert_eq!(
            set_enabled(&inst, "plugins", "shop.jar.disabled", true).unwrap(),
            "shop.jar"
        );
        delete(&inst, "plugins", "shop.jar").unwrap();
        assert!(path_of(&inst, "plugins", "shop.jar").is_err());
        assert!(validate_file("../x.jar").is_err());
        assert!(validate_file("x.zip").is_err());
    }

    #[test]
    fn uploads_are_checked() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let up = crate::config::cfg().data_dir.join("up");
        fs::write(&up, fmod("x", json!({}), "*", &[])).unwrap();
        assert!(
            add_upload(&inst, "plugins", "x.jar", &up).is_err(),
            "a mod is not a plugin"
        );
        add_upload(&inst, "mods", "x.jar", &up).unwrap();
        fs::write(&up, b"no zip").unwrap();
        assert!(add_upload(&inst, "mods", "y.jar", &up).is_err());
    }

    #[test]
    fn lenient_fabric_json() {
        let v = lenient_json(b"{\"description\": \"line one\nline two\"}").unwrap();
        assert_eq!(v["description"], json!("line one\nline two"));
    }
}
