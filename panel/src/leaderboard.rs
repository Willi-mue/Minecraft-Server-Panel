//! Hall of fame for the live map: advancements and statistics of every player of a server, for a
//! tier list and lots of "who is the best at X" records.
//!
//! Read from the world (26.x `world/players/{stats,advancements}/<uuid>.json`, before that
//! `world/{stats,advancements}/<uuid>.json`); names from `usercache.json`. Advancement titles,
//! descriptions and frames and the names of mobs come from the Minecraft client jar the live map
//! already downloaded; without it, ids are turned into names.
//!
//! Minecraft writes these files only on autosave (every 5 minutes) or when a player leaves: while
//! players are online, opening the hall of fame asks the server for a `save-all` (at most once a
//! minute, never during a backup) and the page reloads a few seconds later.
//!
//! Served on the public map port (`/stats.json`), so it holds only what the hall of fame shows:
//! names, skin ids, counts and advancement / mob ids. No positions, no inventories, no uuids.
//!
//! "Me" (`/me.json`): the address each player last logged in from is kept in memory only (never
//! on disk, never sent anywhere); a map visitor from exactly one player's address is offered that
//! player.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::instances::Instance;
use crate::util::{lock, mtime_ns, now_secs, read_json, round_to};
use crate::{livemap, players, skins};

const FRAMES: [&str; 3] = ["task", "goal", "challenge"];
const SAVE_EVERY: Duration = Duration::from_secs(60); // at most one save-all per server
const RECHECK_EVERY: Duration = Duration::from_secs(3); // how often the file times are compared

// tier by the share of all advancements done (recipes don't count)
const TIERS: [(&str, f64); 5] = [("S", 0.6), ("A", 0.4), ("B", 0.25), ("C", 0.1), ("D", 0.0)];

// podiums: group, key, title, unit ("m" = metres, shown as km from 1000 m; "♥" = hearts)
const RECORDS: [(&str, &str, &str, &str); 35] = [
    ("Movement", "walked", "Walked", "m"),
    ("Movement", "sprinted", "Sprinted", "m"),
    ("Movement", "swum", "Swum", "m"),
    ("Movement", "elytra", "Flown with elytra", "m"),
    ("Movement", "boat", "Sailed by boat", "m"),
    ("Movement", "horse", "Ridden on horseback", "m"),
    ("Movement", "minecart", "Rode a minecart", "m"),
    ("Movement", "climbed", "Climbed", "m"),
    ("Movement", "fallen", "Fallen", "m"),
    ("Movement", "sneaked", "Sneaked", "m"),
    ("Movement", "jumps", "Jumps", ""),
    ("Combat", "mob_kills", "Mobs killed", ""),
    ("Combat", "player_kills", "Players killed", ""),
    ("Combat", "damage_dealt", "Damage dealt", "♥"),
    ("Combat", "damage_taken", "Damage taken", "♥"),
    ("Combat", "deaths", "Deaths", ""),
    ("Combat", "alive", "Alive since the last death", "h"),
    ("Mining", "mined", "Blocks mined", ""),
    ("Mining", "ores", "Ores mined", ""),
    ("Mining", "diamonds", "Diamond ore mined", ""),
    ("Mining", "debris", "Ancient debris mined", ""),
    ("Mining", "logs", "Logs chopped", ""),
    ("Mining", "tools_broken", "Tools worn out", ""),
    ("Life", "play_time", "Time played", "h"),
    ("Life", "advancements", "Advancements", ""),
    ("Life", "challenges", "Challenges", ""),
    ("Life", "fish", "Fish caught", ""),
    ("Life", "bred", "Animals bred", ""),
    ("Life", "trades", "Villager trades", ""),
    ("Life", "crafted", "Items crafted", ""),
    ("Life", "enchants", "Items enchanted", ""),
    ("Life", "cake", "Cake slices eaten", ""),
    ("Life", "sleeps", "Nights slept", ""),
    ("Life", "chests", "Chests opened", ""),
    ("Life", "noteblocks", "Note blocks played", ""),
];

// distances in minecraft:custom (cm) -> our keys (m)
const DISTANCES: [(&str, &[&str]); 10] = [
    ("walked", &["walk_one_cm"]),
    ("sprinted", &["sprint_one_cm"]),
    ("sneaked", &["crouch_one_cm"]),
    (
        "swum",
        &[
            "swim_one_cm",
            "walk_under_water_one_cm",
            "walk_on_water_one_cm",
        ],
    ),
    ("elytra", &["aviate_one_cm"]),
    ("boat", &["boat_one_cm"]),
    ("horse", &["horse_one_cm"]),
    ("minecart", &["minecart_one_cm"]),
    ("climbed", &["climb_one_cm"]),
    ("fallen", &["fall_one_cm"]),
];
// plain counters in minecraft:custom
const COUNTERS: [(&str, &str); 12] = [
    ("jumps", "jump"),
    ("mob_kills", "mob_kills"),
    ("player_kills", "player_kills"),
    ("deaths", "deaths"),
    ("fish", "fish_caught"),
    ("bred", "animals_bred"),
    ("trades", "traded_with_villager"),
    ("enchants", "enchant_item"),
    ("cake", "eat_cake_slice"),
    ("sleeps", "sleep_in_bed"),
    ("chests", "open_chest"),
    ("noteblocks", "play_noteblock"),
];

type Signature = Vec<(PathBuf, u128)>;

#[derive(Default)]
struct State {
    cache: HashMap<String, (Instant, Signature, Arc<Map<String, Value>>)>,
    saved: HashMap<String, Instant>,
    jar_cache: HashMap<PathBuf, (u128, Arc<(Map<String, Value>, HashMap<String, String>)>)>,
    logins: HashMap<String, HashMap<String, IpAddr>>, // server id -> player name -> address of the last login
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);

/// Asks a running server for `save-all` (set by the server module; avoids a dependency cycle).
pub type SaveHook = fn(&Instance) -> bool;
static SAVE_HOOK: Mutex<Option<SaveHook>> = Mutex::new(None);

pub fn set_save_hook(hook: SaveHook) {
    *lock(&SAVE_HOOK) = Some(hook);
}

/// Normalised IP (IPv4-mapped IPv6 as IPv4), None if it is none.
fn address(raw: &str) -> Option<IpAddr> {
    let ip: IpAddr = raw.trim_matches(['[', ']']).parse().ok()?;
    Some(match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    })
}

/// From the server log ("Name[/1.2.3.4:5678] logged in ..."); memory only.
pub fn remember_login(inst: &Instance, name: &str, raw_address: &str) {
    if let Some(a) = address(raw_address) {
        lock(&STATE)
            .logins
            .entry(inst.id.clone())
            .or_default()
            .insert(name.to_string(), a);
    }
}

/// The player who last logged in from this address, if it is exactly one.
pub fn me(inst: &Instance, raw_address: Option<&str>) -> Option<String> {
    let a = address(raw_address.unwrap_or(""))?;
    let g = lock(&STATE);
    let names: Vec<&String> = g
        .logins
        .get(&inst.id)
        .map(|m| m.iter().filter(|(_, v)| **v == a).map(|(n, _)| n).collect())
        .unwrap_or_default();
    (names.len() == 1).then(|| names[0].clone())
}

fn pretty(an_id: &str) -> String {
    let last = an_id.rsplit(':').next().unwrap_or(an_id);
    let last = last.rsplit('/').next().unwrap_or(last);
    crate::util::capitalize(&last.replace('_', " "))
}

fn read_zip_entry(z: &mut zip::ZipArchive<fs::File>, name: &str) -> Option<Vec<u8>> {
    let mut f = z.by_name(name).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// (advancements, lang) from the client jar; empty without one.
fn jar_tables(jar: Option<&Path>) -> Arc<(Map<String, Value>, HashMap<String, String>)> {
    let empty = || Arc::new((Map::new(), HashMap::new()));
    let Some(jar) = jar.filter(|j| j.exists()) else {
        return empty();
    };
    let mtime = mtime_ns(jar).unwrap_or(0);
    if let Some((m, t)) = lock(&STATE).jar_cache.get(jar) {
        if *m == mtime {
            return t.clone();
        }
    }
    let Ok(mut z) = fs::File::open(jar)
        .map_err(|_| ())
        .and_then(|f| zip::ZipArchive::new(f).map_err(|_| ()))
    else {
        return empty();
    };
    let lang: Map<String, Value> = read_zip_entry(&mut z, "assets/minecraft/lang/en_us.json")
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default();
    let tr = |key: &str| lang.get(key).and_then(Value::as_str).map(str::to_string);
    let names: Vec<String> = z
        .file_names()
        .filter_map(|n| n.ok().map(|n| n.to_string()))
        .collect();
    let mut advs = Map::new();
    let mut found: Vec<(String, String)> = vec![];
    for name in &names {
        for folder in [
            "data/minecraft/advancement/",
            "data/minecraft/advancements/",
        ] {
            if let Some(path) = name
                .strip_prefix(folder)
                .and_then(|p| p.strip_suffix(".json"))
            {
                if !path.starts_with("recipes/") {
                    found.push((name.clone(), path.to_string()));
                }
            }
        }
    }
    found.sort_by(|a, b| a.1.cmp(&b.1));
    for (name, path) in found {
        let Some(display) = read_zip_entry(&mut z, &name)
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .and_then(|v| v.get("display").cloned())
            .filter(Value::is_object)
        else {
            continue;
        };
        let text = |part: &str| {
            display
                .get(part)
                .and_then(|p| p.get("translate"))
                .and_then(Value::as_str)
                .and_then(tr)
        };
        let frame = display
            .get("frame")
            .and_then(Value::as_str)
            .filter(|f| FRAMES.contains(f))
            .unwrap_or("task");
        advs.insert(
            format!("minecraft:{path}"),
            json!({
                "title": text("title").unwrap_or_else(|| pretty(&path)),
                "description": text("description").unwrap_or_default(),
                "frame": frame,
                "category": path.split('/').next().unwrap_or(""),
            }),
        );
    }
    let lang: HashMap<String, String> = lang
        .iter()
        .filter(|(k, _)| k.starts_with("entity.minecraft.") || k.starts_with("block.minecraft."))
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
        .collect();
    let tables = Arc::new((advs, lang));
    lock(&STATE)
        .jar_cache
        .insert(jar.to_path_buf(), (mtime, tables.clone()));
    tables
}

/// The client jar the live map uses for this world, only if it is already there (never downloads).
fn client_jar(inst: &Instance) -> Option<PathBuf> {
    let path = livemap::client_jar_path(&livemap::map_version(inst)?);
    path.exists().then_some(path)
}

/// uuid -> file of `kind` (stats / advancements); the 26.x layout wins.
fn player_files(world: &Path, kind: &str) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    for folder in [world.join(kind), world.join("players").join(kind)] {
        let Ok(rd) = fs::read_dir(&folder) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                out.insert(
                    p.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase(),
                    p,
                );
            }
        }
    }
    out
}

fn num(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        _ => 0,
    }
}

fn group(s: &Map<String, Value>, name: &str) -> BTreeMap<String, i64> {
    match s.get(&format!("minecraft:{name}")) {
        Some(Value::Object(g)) => g.iter().map(|(k, v)| (k.clone(), num(v))).collect(),
        _ => BTreeMap::new(),
    }
}

fn round_half_even(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && (r as i64) % 2 != 0 {
        (r - x.signum()) as i64
    } else {
        r as i64
    }
}

/// (stats, killed, killed_by) of a stats file: our keys -> numbers, entity id -> count.
pub fn stats_of(
    data: &Value,
) -> (
    Map<String, Value>,
    BTreeMap<String, i64>,
    BTreeMap<String, i64>,
) {
    let empty = Map::new();
    let s = data
        .get("stats")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let custom = group(s, "custom");
    let mined = group(s, "mined");
    let c = |key: &str| {
        custom
            .get(&format!("minecraft:{key}"))
            .copied()
            .unwrap_or(0)
    };
    let mut out = Map::new();
    for (k, keys) in DISTANCES {
        out.insert(
            k.into(),
            json!(round_half_even(
                keys.iter().map(|x| c(x)).sum::<i64>() as f64 / 100.0
            )),
        );
    }
    for (k, v) in COUNTERS {
        out.insert(k.into(), json!(c(v)));
    }
    let play = if c("play_time") != 0 {
        c("play_time")
    } else {
        c("play_one_minute")
    };
    out.insert(
        "play_time".into(),
        json!(round_to(play as f64 / 72000.0, 1)),
    ); // ticks -> hours
    let since = c("time_since_death");
    out.insert(
        "alive".into(),
        if since != 0 {
            json!(round_to(since as f64 / 72000.0, 1))
        } else {
            json!(0)
        },
    );
    out.insert(
        "damage_dealt".into(),
        json!(round_half_even(c("damage_dealt") as f64 / 20.0)),
    ); // tenths of a health point -> hearts
    out.insert(
        "damage_taken".into(),
        json!(round_half_even(c("damage_taken") as f64 / 20.0)),
    );
    out.insert("mined".into(), json!(mined.values().sum::<i64>()));
    out.insert(
        "ores".into(),
        json!(
            mined
                .iter()
                .filter(|(k, _)| k.ends_with("_ore") || *k == "minecraft:ancient_debris")
                .map(|(_, v)| v)
                .sum::<i64>()
        ),
    );
    let m = |k: &str| mined.get(k).copied().unwrap_or(0);
    out.insert(
        "diamonds".into(),
        json!(m("minecraft:diamond_ore") + m("minecraft:deepslate_diamond_ore")),
    );
    out.insert("debris".into(), json!(m("minecraft:ancient_debris")));
    out.insert(
        "logs".into(),
        json!(
            mined
                .iter()
                .filter(|(k, _)| k.ends_with("_log") || k.ends_with("_stem"))
                .map(|(_, v)| v)
                .sum::<i64>()
        ),
    );
    out.insert(
        "crafted".into(),
        json!(group(s, "crafted").values().sum::<i64>()),
    );
    out.insert(
        "tools_broken".into(),
        json!(group(s, "broken").values().sum::<i64>()),
    );
    (out, group(s, "killed"), group(s, "killed_by"))
}

fn done(data: &Value) -> Vec<String> {
    let mut out: Vec<String> = data
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, v)| {
            v.get("done").and_then(Value::as_bool) == Some(true)
                && k.contains(':')
                && !k
                    .split_once(':')
                    .map(|(_, p)| p.starts_with("recipes/"))
                    .unwrap_or(false)
        })
        .map(|(k, _)| k.clone())
        .collect();
    out.sort();
    out
}

pub fn tier_of(share: f64) -> &'static str {
    TIERS
        .iter()
        .find(|(_, low)| share >= *low)
        .map(|(t, _)| *t)
        .unwrap_or("D")
}

/// What the result depends on: the player files, usercache.json and the client jar.
fn signature(inst: &Instance) -> Signature {
    let world = players::world(inst);
    let mut files = vec![inst.server_dir().join("usercache.json")];
    for kind in ["stats", "advancements"] {
        files.extend(player_files(&world, kind).into_values());
    }
    files.extend(client_jar(inst));
    let mut sig: Signature = files
        .into_iter()
        .filter_map(|f| mtime_ns(&f).map(|m| (f, m)))
        .collect();
    sig.sort();
    sig
}

/// While players are online, ask the server to write their stats (save-all), at most once a minute
/// and never during another action (a backup runs with saving turned off).
fn request_save(inst: &Instance) -> bool {
    if lock(&STATE)
        .saved
        .get(&inst.id)
        .is_some_and(|t| t.elapsed() < SAVE_EVERY)
    {
        return false;
    }
    let Some(hook) = *lock(&SAVE_HOOK) else {
        return false;
    };
    if !hook(inst) {
        return false;
    }
    lock(&STATE).saved.insert(inst.id.clone(), Instant::now());
    true
}

fn compute(inst: &Instance) -> Map<String, Value> {
    let world = players::world(inst);
    let tables = jar_tables(client_jar(inst).as_deref());
    let (advs, lang) = (&tables.0, &tables.1);
    let names: HashMap<String, String> = players::usercache(inst)
        .into_values()
        .map(|(n, u)| (u, n))
        .collect();
    let stat_files = player_files(&world, "stats");
    let adv_files = player_files(&world, "advancements");
    let uuids: BTreeSet<&String> = stat_files.keys().chain(adv_files.keys()).collect();
    let mut rows: Vec<Map<String, Value>> = vec![];
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for uuid in uuids {
        let dashed = if uuid.replace('-', "").len() == 32 {
            players::dashed(uuid)
        } else {
            uuid.clone()
        };
        let Some(name) = names.get(&dashed) else {
            continue; // unknown to this server's usercache (e.g. a copied world): no name to show
        };
        let mut finished = adv_files
            .get(uuid)
            .and_then(|p| read_json(p))
            .map(|d| done(&d))
            .unwrap_or_default();
        if !advs.is_empty() {
            finished.retain(|a| advs.contains_key(a));
        }
        let data = stat_files
            .get(uuid)
            .and_then(|p| read_json(p))
            .filter(Value::is_object)
            .unwrap_or(json!({}));
        let (mut stats, killed, killed_by) = stats_of(&data);
        stats.insert("advancements".into(), json!(finished.len()));
        let challenges = finished
            .iter()
            .filter(|a| advs.get(*a).and_then(|x| x.get("frame")) == Some(&json!("challenge")))
            .count();
        stats.insert("challenges".into(), json!(challenges));
        seen_ids.extend(killed.keys().cloned());
        seen_ids.extend(killed_by.keys().cloned());
        let plain = dashed.replace('-', ""); // skins are stored without dashes
        let skin = skins::skin_path(&plain)
            .ok()
            .filter(|p| p.exists())
            .map(|_| plain);
        let mut row = Map::new();
        row.insert("name".into(), json!(name));
        row.insert("skin".into(), json!(skin));
        row.insert("done".into(), json!(finished));
        row.insert("stats".into(), Value::Object(stats));
        row.insert("killed".into(), json!(killed));
        row.insert("killed_by".into(), json!(killed_by));
        rows.push(row);
    }
    let max_done = rows
        .iter()
        .map(|r| r["done"].as_array().map_or(0, Vec::len))
        .max()
        .unwrap_or(0);
    let total = if !advs.is_empty() {
        advs.len()
    } else if max_done > 0 {
        max_done
    } else {
        1
    };
    for r in rows.iter_mut() {
        let share = round_to(
            r["done"].as_array().map_or(0, Vec::len) as f64 / total as f64,
            3,
        );
        r.insert("share".into(), json!(share));
        r.insert("tier".into(), json!(tier_of(share)));
    }
    rows.sort_by(|a, b| {
        let key = |r: &Map<String, Value>| {
            (
                -r["share"].as_f64().unwrap_or(0.0),
                -r["stats"]["play_time"].as_f64().unwrap_or(0.0),
                r["name"].as_str().unwrap_or("").to_lowercase(),
            )
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.total_cmp(&kb.0)
            .then(ka.1.total_cmp(&kb.1))
            .then(ka.2.cmp(&kb.2))
    });
    let known: Map<String, Value> = if !advs.is_empty() {
        advs.clone()
    } else {
        let mut m = Map::new();
        for r in &rows {
            for a in r["done"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let category = a
                    .split_once(':')
                    .map(|(_, p)| p)
                    .unwrap_or(a)
                    .split('/')
                    .next()
                    .unwrap_or("");
                m.insert(a.into(), json!({"title": pretty(a), "description": "", "frame": "task", "category": category}));
            }
        }
        m
    };
    let mobs: Map<String, Value> = seen_ids
        .iter()
        .map(|i| {
            (
                i.clone(),
                json!(
                    lang.get(&format!("entity.{}", i.replace(':', ".")))
                        .cloned()
                        .unwrap_or_else(|| pretty(i))
                ),
            )
        })
        .collect();
    let mut out = Map::new();
    out.insert("generated".into(), json!(now_secs() as i64));
    out.insert("total".into(), json!(total));
    out.insert(
        "tiers".into(),
        json!(
            TIERS
                .iter()
                .map(|(t, low)| json!({"tier": t, "from": low}))
                .collect::<Vec<_>>()
        ),
    );
    out.insert(
        "records".into(),
        json!(
            RECORDS
                .iter()
                .map(|(g, k, t, u)| json!({"group": g, "key": k, "title": t, "unit": u}))
                .collect::<Vec<_>>()
        ),
    );
    out.insert("advancements".into(), Value::Object(known));
    out.insert("mobs".into(), Value::Object(mobs));
    out.insert(
        "players".into(),
        Value::Array(rows.into_iter().map(Value::Object).collect()),
    );
    out
}

/// The hall of fame of a server; recomputed when a player file changed. `saving`: a save-all was
/// just requested, newer numbers follow in a few seconds.
pub fn build(inst: &Instance) -> Value {
    let saving = request_save(inst);
    let hit = lock(&STATE).cache.get(&inst.id).cloned();
    let with_saving = |r: &Map<String, Value>| {
        let mut r = r.clone();
        r.insert("saving".into(), json!(saving));
        Value::Object(r)
    };
    if let Some((checked, _, result)) = &hit {
        if checked.elapsed() < RECHECK_EVERY {
            return with_saving(result);
        }
    }
    let sig = signature(inst);
    let result = match &hit {
        Some((_, old, result)) if *old == sig => result.clone(),
        _ => Arc::new(compute(inst)),
    };
    lock(&STATE)
        .cache
        .insert(inst.id.clone(), (Instant::now(), sig, result.clone()));
    with_saving(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins_by_address() {
        let inst = Instance::new("lb-test");
        remember_login(&inst, "Steve", "192.168.1.5");
        remember_login(&inst, "Alex", "[::ffff:192.168.1.6]");
        assert_eq!(me(&inst, Some("192.168.1.5")).as_deref(), Some("Steve"));
        assert_eq!(
            me(&inst, Some("::ffff:192.168.1.6")).as_deref(),
            Some("Alex")
        );
        remember_login(&inst, "Herobrine", "192.168.1.5");
        assert_eq!(
            me(&inst, Some("192.168.1.5")),
            None,
            "two players from one address"
        );
        assert_eq!(me(&inst, Some("nonsense")), None);
        assert_eq!(me(&inst, None), None);
    }

    #[test]
    fn stats_and_tiers() {
        let data = json!({"stats": {
            "minecraft:custom": {"minecraft:walk_one_cm": 12345, "minecraft:play_time": 144000, "minecraft:jump": 7,
                                  "minecraft:damage_dealt": 30},
            "minecraft:mined": {"minecraft:diamond_ore": 2, "minecraft:oak_log": 5, "minecraft:stone": 10},
            "minecraft:killed": {"minecraft:zombie": 3}
        }});
        let (s, killed, _) = stats_of(&data);
        assert_eq!(s["walked"], json!(123));
        assert_eq!(s["play_time"], json!(2.0));
        assert_eq!(s["jumps"], json!(7));
        assert_eq!(
            s["damage_dealt"],
            json!(2),
            "1.5 rounds to even like Python"
        );
        assert_eq!(s["mined"], json!(17));
        assert_eq!(s["ores"], json!(2));
        assert_eq!(s["diamonds"], json!(2));
        assert_eq!(s["logs"], json!(5));
        assert_eq!(killed["minecraft:zombie"], 3);
        assert_eq!(tier_of(0.6), "S");
        assert_eq!(tier_of(0.39), "B");
        assert_eq!(tier_of(0.0), "D");
        assert_eq!(pretty("minecraft:story/mine_diamond"), "Mine diamond");
    }

    #[test]
    fn build_from_world_files() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let dir = inst.server_dir();
        let uuid = "8667ba71-b85a-4004-af54-457a9734eed7";
        fs::create_dir_all(dir.join("world/players/advancements")).unwrap();
        fs::write(
            dir.join("usercache.json"),
            format!(r#"[{{"name": "Steve", "uuid": "{uuid}"}}]"#),
        )
        .unwrap();
        fs::write(dir.join(format!("world/players/advancements/{uuid}.json")),
            r#"{"minecraft:story/root": {"done": true}, "minecraft:recipes/x": {"done": true}, "minecraft:story/mine_stone": {"done": false}, "DataVersion": 1}"#).unwrap();
        let b = build(&inst);
        assert_eq!(b["players"][0]["name"], json!("Steve"));
        assert_eq!(b["players"][0]["done"], json!(["minecraft:story/root"]));
        assert_eq!(b["players"][0]["tier"], json!("S"));
        assert_eq!(
            b["advancements"]["minecraft:story/root"]["title"],
            json!("Root")
        );
        assert_eq!(b["saving"], json!(false));
        let s = b.to_string();
        assert!(!s.contains(uuid), "no uuids on the public port");
    }
}
