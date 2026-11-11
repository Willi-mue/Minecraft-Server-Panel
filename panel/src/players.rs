//! Player profiles: name <-> uuid, play time, first/last seen, last position, game mode, skins.
//!
//! - `usercache.json` (written by the server) maps names to uuids.
//! - Statistics and player data live in the world: 26.x `world/players/{stats,data}/<uuid>`, before
//!   that `world/stats/<uuid>.json` and `world/playerdata/<uuid>.dat`.
//! - First/last seen is recorded by the panel on join/leave in `<instance root>/players.json`.
//! - Skins: only from Mojang, shared cache with the live map (`skins::skin_path`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::instances::Instance;
use crate::nbt::{self, Tag};
use crate::skins::{self, SKIN_RETRY};
use crate::util::{json_round, lock, now_secs, read_json, to_json_indent, write_atomic};
use crate::{access, backup};

pub const GAMEMODES: [&str; 4] = ["survival", "creative", "adventure", "spectator"];
static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-f]{8}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{4}-?[0-9a-f]{12}$").unwrap()
});

static SEEN_LOCK: Mutex<()> = Mutex::new(());
static SKIN_FAILED: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);

pub fn dashed(u: &str) -> String {
    let u = u.replace('-', "").to_lowercase();
    if u.len() != 32 {
        return u;
    }
    format!(
        "{}-{}-{}-{}-{}",
        &u[..8],
        &u[8..12],
        &u[12..16],
        &u[16..20],
        &u[20..]
    )
}

fn is_uuid(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    UUID_RE.is_match(&s.to_lowercase()).then(|| dashed(&s))
}

// ---------- name <-> uuid ----------

/// lower-case name -> (name, uuid with dashes).
pub fn usercache(inst: &Instance) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    if let Some(Value::Array(entries)) = read_json(&inst.server_dir().join("usercache.json")) {
        for e in entries {
            if let (Some(name), Some(uuid)) = (
                e.get("name").and_then(Value::as_str),
                e.get("uuid").and_then(is_uuid),
            ) {
                out.insert(name.to_lowercase(), (name.to_string(), uuid));
            }
        }
    }
    out
}

/// Dashed uuid from usercache.json, then from whitelist/ops/bans (the server writes uuids there).
pub fn uuid_of(inst: &Instance, name: &str) -> Option<String> {
    if let Some((_, uuid)) = usercache(inst).get(&name.to_lowercase()) {
        return Some(uuid.clone());
    }
    for file in ["whitelist.json", "ops.json", "banned-players.json"] {
        for e in access::read_list(inst, file) {
            let same = e
                .get("name")
                .map(|n| n.as_str().unwrap_or("").to_lowercase() == name.to_lowercase());
            if same == Some(true) {
                if let Some(u) = e.get("uuid").and_then(is_uuid) {
                    return Some(u);
                }
            }
        }
    }
    None
}

// ---------- first / last seen ----------

fn seen_file(inst: &Instance) -> PathBuf {
    inst.root().join("players.json")
}

fn read_seen(inst: &Instance) -> Map<String, Value> {
    match read_json(&seen_file(inst)) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// event: join (true) / leave (false).
pub fn record(inst: &Instance, name: &str, join: bool, now: Option<i64>) {
    let now = now.unwrap_or(now_secs() as i64);
    let _g = lock(&SEEN_LOCK);
    let mut data = read_seen(inst);
    let entry = data
        .entry(name.to_lowercase())
        .or_insert_with(|| json!({"name": name, "first_seen": now}));
    if !entry.is_object() {
        *entry = json!({"name": name, "first_seen": now});
    }
    entry["name"] = json!(name);
    entry["last_seen"] = json!(now);
    entry["online"] = json!(join);
    let _ = write_atomic(
        &seen_file(inst),
        to_json_indent(&Value::Object(data), 1).as_bytes(),
    );
}

/// Server stopped: everybody who was online left now.
pub fn all_offline(inst: &Instance) {
    let online: Vec<String> = read_seen(inst)
        .values()
        .filter(|e| e.get("online").and_then(Value::as_bool) == Some(true))
        .map(|e| {
            e.get("name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string()
        })
        .collect();
    for name in online {
        record(inst, &name, false, None);
    }
}

pub fn seen(inst: &Instance, name: &str) -> Map<String, Value> {
    match read_seen(inst).remove(&name.to_lowercase()) {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

// ---------- world data ----------

pub fn world(inst: &Instance) -> PathBuf {
    inst.server_dir().join(backup::level_name(inst))
}

fn first(paths: &[PathBuf]) -> Option<&PathBuf> {
    paths.iter().find(|p| p.is_file())
}

fn as_i64(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        _ => 0,
    }
}

pub fn stats(inst: &Instance, uuid: &str) -> Option<Value> {
    let w = world(inst);
    let path = first(&[
        w.join("players/stats").join(format!("{uuid}.json")),
        w.join("stats").join(format!("{uuid}.json")),
    ])?
    .clone();
    let data = read_json(&path)?;
    let empty = Map::new();
    let custom = data
        .pointer("/stats/minecraft:custom")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let ticks = custom
        .get("minecraft:play_time")
        .or_else(|| custom.get("minecraft:play_one_minute"));
    let cm: i64 = custom
        .iter()
        .filter(|(k, _)| k.ends_with("_one_cm"))
        .map(|(_, v)| as_i64(Some(v)))
        .sum();
    Some(json!({
        "play_time": as_i64(ticks) / 20, // seconds
        "deaths": as_i64(custom.get("minecraft:deaths")),
        "mob_kills": as_i64(custom.get("minecraft:mob_kills")),
        "player_kills": as_i64(custom.get("minecraft:player_kills")),
        "distance_km": json_round(cm as f64 / 100_000.0, 1),
    }))
}

/// Last saved position, dimension, game mode, health, level (as of the last world save).
pub fn player_data(inst: &Instance, uuid: &str) -> Map<String, Value> {
    let w = world(inst);
    let mut out = Map::new();
    let Some(path) = first(&[
        w.join("players/data").join(format!("{uuid}.dat")),
        w.join("playerdata").join(format!("{uuid}.dat")),
    ])
    .cloned() else {
        return out;
    };
    let Ok((_, root)) = nbt::load(&path) else {
        return out;
    };
    if let Some(Tag::List(_, pos)) = root.get("Pos") {
        if pos.len() == 3 {
            out.insert(
                "pos".into(),
                Value::Array(
                    pos.iter()
                        .map(|v| json_round(v.as_f64().unwrap_or(0.0), 1))
                        .collect(),
                ),
            );
        }
    }
    match root.get("Dimension") {
        Some(Tag::String(s)) => {
            out.insert("dimension".into(), json!(s));
        }
        Some(t) => {
            if let Some(n) = t.as_i64() {
                let name = match n {
                    -1 => "minecraft:the_nether".to_string(),
                    0 => "minecraft:overworld".to_string(),
                    1 => "minecraft:the_end".to_string(),
                    other => other.to_string(),
                };
                out.insert("dimension".into(), json!(name));
            }
        }
        None => {}
    }
    if let Some(mode) = root.get("playerGameType").and_then(Tag::as_i64) {
        if (0..4).contains(&mode) {
            out.insert("gamemode".into(), json!(GAMEMODES[mode as usize]));
        }
    }
    for (key, name) in [
        ("Health", "health"),
        ("XpLevel", "level"),
        ("foodLevel", "food"),
    ] {
        if let Some(v) = root.get(key).and_then(Tag::as_f64) {
            out.insert(
                name.into(),
                if key == "Health" {
                    json_round(v, 1)
                } else {
                    json!(v as i64)
                },
            );
        }
    }
    out
}

/// (dimension, [x, y, z]) of the world spawn from level.dat (26.x `Data.spawn`, else SpawnX/Y/Z).
pub fn world_spawn(inst: &Instance) -> Option<(String, [i64; 3])> {
    let (_, root) = nbt::load(&world(inst).join("level.dat")).ok()?;
    let data = root.compound("Data")?;
    if let Some(spawn) = data.compound("spawn") {
        let dim = spawn
            .string("dimension")
            .unwrap_or("minecraft:overworld")
            .to_string();
        if let Some(Tag::Ints(pos)) = spawn.get("pos") {
            if pos.len() == 3 {
                return Some((dim, [pos[0] as i64, pos[1] as i64, pos[2] as i64]));
            }
        }
    }
    let get = |k: &str| data.get(k).and_then(Tag::as_i64);
    Some((
        "minecraft:overworld".into(),
        [get("SpawnX")?, get("SpawnY")?, get("SpawnZ")?],
    ))
}

pub fn ban_entry(inst: &Instance, name: &str) -> Value {
    for e in access::read_list(inst, "banned-players.json") {
        if e.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase()
            == name.to_lowercase()
        {
            let mut out = Map::new();
            for k in ["reason", "source", "created", "expires"] {
                out.insert(k.into(), e.get(k).cloned().unwrap_or(Value::Null));
            }
            return Value::Object(out);
        }
    }
    Value::Null
}

pub fn profile(inst: &Instance, name: &str) -> Map<String, Value> {
    let uuid = uuid_of(inst, name);
    let s = seen(inst, name);
    let shown = s
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .or_else(|| {
            usercache(inst)
                .get(&name.to_lowercase())
                .map(|e| e.0.clone())
        })
        .unwrap_or_else(|| name.to_string());
    let mut out = Map::new();
    out.insert("name".into(), json!(shown));
    out.insert("uuid".into(), json!(uuid));
    out.insert(
        "first_seen".into(),
        s.get("first_seen").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "last_seen".into(),
        s.get("last_seen").cloned().unwrap_or(Value::Null),
    );
    out.insert("ban".into(), ban_entry(inst, name));
    if let Some(uuid) = uuid {
        if let Some(st) = stats(inst, &uuid) {
            out.insert("stats".into(), st);
        }
        out.extend(player_data(inst, &uuid));
    }
    out
}

// ---------- skins ----------

/// The cached skin PNG of a player, downloaded from Mojang if missing/old. None = use a default
/// head (offline-mode server, unknown player, Mojang unreachable).
pub fn skin(inst: &Instance, name: &str) -> Option<PathBuf> {
    let uuid = uuid_of(inst, name)?;
    let raw = uuid.replace('-', "");
    let path = skins::skin_path(&raw).ok()?;
    let existing = |p: &Path| p.exists().then(|| p.to_path_buf());
    if skins::is_fresh(&path) {
        return Some(path);
    }
    if !access::online_mode(inst) {
        return existing(&path); // offline uuids aren't Mojang accounts
    }
    if now_secs() - lock(&SKIN_FAILED).get(&raw).copied().unwrap_or(0.0) < SKIN_RETRY {
        return existing(&path);
    }
    match skins::fetch_skin(&raw) {
        Ok(p) => Some(p),
        Err(_) => {
            lock(&SKIN_FAILED).insert(raw, now_secs());
            existing(&path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::Compound;

    const UUID: &str = "8667ba71-b85a-4004-af54-457a9734eed7";

    #[test]
    fn profile_from_world_files() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let dir = inst.server_dir();
        std::fs::create_dir_all(dir.join("world/players/stats")).unwrap();
        std::fs::create_dir_all(dir.join("world/players/data")).unwrap();
        std::fs::write(
            dir.join("usercache.json"),
            format!(r#"[{{"name": "Steve", "uuid": "{UUID}"}}]"#),
        )
        .unwrap();
        std::fs::write(dir.join(format!("world/players/stats/{UUID}.json")),
            r#"{"stats": {"minecraft:custom": {"minecraft:play_time": 72000, "minecraft:walk_one_cm": 150000, "minecraft:deaths": 2}}}"#).unwrap();
        let root = Compound(vec![
            (
                "Pos".into(),
                Tag::List(
                    nbt::DOUBLE,
                    vec![Tag::Double(1.25), Tag::Double(64.0), Tag::Double(-3.33)],
                ),
            ),
            (
                "Dimension".into(),
                Tag::String("minecraft:the_nether".into()),
            ),
            ("playerGameType".into(), Tag::Int(1)),
            ("Health".into(), Tag::Float(19.5)),
            ("XpLevel".into(), Tag::Int(7)),
        ]);
        nbt::save(
            &dir.join(format!("world/players/data/{UUID}.dat")),
            "",
            &root,
        )
        .unwrap();
        record(&inst, "Steve", true, Some(100));
        record(&inst, "Steve", false, Some(200));

        let p = profile(&inst, "steve");
        assert_eq!(p["name"], json!("Steve"));
        assert_eq!(p["uuid"], json!(UUID));
        assert_eq!(p["first_seen"], json!(100));
        assert_eq!(p["last_seen"], json!(200));
        assert_eq!(p["stats"]["play_time"], json!(3600));
        assert_eq!(p["stats"]["deaths"], json!(2));
        assert_eq!(p["stats"]["distance_km"], json!(1.5));
        assert_eq!(p["pos"], json!([1.3, 64.0, -3.3]));
        assert_eq!(p["dimension"], json!("minecraft:the_nether"));
        assert_eq!(p["gamemode"], json!("creative"));
        assert_eq!(p["health"], json!(19.5));
        assert_eq!(p["level"], json!(7));
        assert_eq!(p["ban"], Value::Null);
    }

    #[test]
    fn all_offline_closes_sessions() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        record(&inst, "A", true, Some(1));
        all_offline(&inst);
        assert_eq!(seen(&inst, "a")["online"], json!(false));
    }

    #[test]
    fn spawn_old_and_new() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let p = world(&inst).join("level.dat");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let data = Compound(vec![
            ("SpawnX".into(), Tag::Int(10)),
            ("SpawnY".into(), Tag::Int(70)),
            ("SpawnZ".into(), Tag::Int(-5)),
        ]);
        nbt::save(
            &p,
            "",
            &Compound(vec![("Data".into(), Tag::Compound(data))]),
        )
        .unwrap();
        assert_eq!(
            world_spawn(&inst),
            Some(("minecraft:overworld".into(), [10, 70, -5]))
        );
        let spawn = Compound(vec![
            ("dimension".into(), Tag::String("minecraft:the_end".into())),
            ("pos".into(), Tag::Ints(vec![1, 2, 3])),
        ]);
        let data = Compound(vec![("spawn".into(), Tag::Compound(spawn))]);
        nbt::save(
            &p,
            "",
            &Compound(vec![("Data".into(), Tag::Compound(data))]),
        )
        .unwrap();
        assert_eq!(
            world_spawn(&inst),
            Some(("minecraft:the_end".into(), [1, 2, 3]))
        );
    }
}
