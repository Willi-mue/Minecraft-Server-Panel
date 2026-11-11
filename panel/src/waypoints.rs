//! Waypoints on the live map: named spots per server, set by players on the (public) map page and
//! by the admin in the panel.
//!
//! Stored per server in `<Instance::root>/waypoints.json` (not under the map folder, which is
//! served). Every waypoint belongs to a player of that server. Who a map visitor is comes from the
//! same places as the hall of fame's "Me": the name the browser picked (not verified) or the player
//! whose last login came from the visitor's address (`leaderboard::me`). Because the picked name is
//! only a claim, changing / deleting is allowed to
//! - the browser that created the waypoint (a random key it keeps; only its SHA-256 is stored),
//! - the owner, recognised by address,
//! - an operator of the server, recognised by address,
//! - the panel (always).
//!
//! The map port stays a small, strictly validated write surface: names, coordinates, dimension and
//! colour are checked here, and both the count per player and in total are limited.

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::instances::Instance;
use crate::util::{ct_eq, hex_token, lock, now_secs, read_json, to_json_indent, write_atomic};
use crate::{access, invalid, leaderboard, players};

pub const NAME_MAX: usize = 32;
pub const MAX_PER_PLAYER: usize = 50;
pub const MAX_TOTAL: usize = 500;
const COORD_MAX: i64 = 30_000_000;
const Y_MIN: i64 = -2048;
const Y_MAX: i64 = 2048;
pub const COLORS: [&str; 7] = [
    "green", "blue", "red", "yellow", "purple", "orange", "white",
];
static DIM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9_.-]{1,64}$").unwrap());
static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9]{12}$").unwrap());
static KEY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{16,128}$").unwrap());

static LOCK: Mutex<()> = Mutex::new(());

pub enum WpError {
    NotFound,
    Forbidden(String),
    Invalid(String),
}

impl From<Error> for WpError {
    fn from(e: Error) -> Self {
        WpError::Invalid(e.to_string())
    }
}

impl From<std::io::Error> for WpError {
    fn from(e: std::io::Error) -> Self {
        WpError::Invalid(e.to_string())
    }
}

pub fn path(inst: &Instance) -> PathBuf {
    inst.root().join("waypoints.json")
}

fn load(inst: &Instance) -> Vec<Map<String, Value>> {
    let Some(items) = read_json(&path(inst)).and_then(|d| d.get("waypoints").cloned()) else {
        return vec![];
    };
    items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| w.as_object().cloned())
        .filter(|w| {
            let id = match w.get("id") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            };
            ID_RE.is_match(&id)
        })
        .collect()
}

fn save(inst: &Instance, items: &[Map<String, Value>]) -> std::io::Result<()> {
    let data = json!({"waypoints": items});
    write_atomic(&path(inst), to_json_indent(&data, 1).as_bytes())
}

// ---------- validation ----------

pub fn validate_name(value: Option<&Value>) -> Result<String> {
    let Some(Value::String(s)) = value else {
        return invalid!("Name missing");
    };
    let name = s.split_whitespace().collect::<Vec<_>>().join(" "); // also drops newlines / tabs
    if name.is_empty()
        || name.chars().count() > NAME_MAX
        || name.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
    {
        return invalid!("Name: 1-{NAME_MAX} characters");
    }
    Ok(name)
}

fn int(value: Option<&Value>, low: i64, high: i64, what: &str) -> Result<i64> {
    let v = match value {
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => i,
            None => match n.as_f64() {
                Some(f) if f.is_finite() => f.trunc() as i64,
                _ => return invalid!("{what} must be a number"),
            },
        },
        _ => return invalid!("{what} must be a number"),
    };
    if !(low..=high).contains(&v) {
        return invalid!("{what} out of range");
    }
    Ok(v)
}

/// The editable fields of a request body: name, dim, x, y (or null), z, color.
pub fn validate_fields(body: &Map<String, Value>, partial: bool) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    let has = |k: &str| body.contains_key(k) || !partial;
    if has("name") {
        out.insert("name".into(), json!(validate_name(body.get("name"))?));
    }
    if has("dim") {
        match body.get("dim").and_then(Value::as_str) {
            Some(d) if DIM_RE.is_match(d) => out.insert("dim".into(), json!(d)),
            _ => return invalid!("Unknown dimension"),
        };
    }
    for axis in ["x", "z"] {
        if has(axis) {
            out.insert(
                axis.into(),
                json!(int(body.get(axis), -COORD_MAX, COORD_MAX, axis)?),
            );
        }
    }
    if has("y") {
        let y = match body.get("y") {
            None | Some(Value::Null) => Value::Null,
            v => json!(int(v, Y_MIN, Y_MAX, "y")?),
        };
        out.insert("y".into(), y);
    }
    if has("color") {
        let color = body.get("color").cloned().unwrap_or(json!(COLORS[0]));
        match color.as_str() {
            Some(c) if COLORS.contains(&c) => out.insert("color".into(), color),
            _ => return invalid!("Unknown colour"),
        };
    }
    Ok(out)
}

fn key_hash(key: Option<&str>) -> Option<String> {
    key.filter(|k| KEY_RE.is_match(k))
        .map(|k| hex::encode(Sha256::digest(k.as_bytes())))
}

/// The player's name as the server knows it (usercache / seen by the panel), else None.
pub fn known_player(inst: &Instance, name: Option<&str>) -> Option<String> {
    let name = name.filter(|n| !n.is_empty())?;
    if let Some((n, _)) = players::usercache(inst).get(&name.to_lowercase()) {
        return Some(n.clone());
    }
    players::seen(inst, name)
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn ops(inst: &Instance) -> Vec<String> {
    access::read_list(inst, "ops.json")
        .iter()
        .map(|e| {
            e.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase()
        })
        .collect()
}

// ---------- visitors of the map ----------

/// Who sent a map request: `claimed` = name picked in the browser, `key` = the browser's key,
/// address = the client address (only compared, never stored).
pub struct Visitor {
    pub recognised: Option<String>,
    pub claimed: Option<String>,
    pub key_hash: Option<String>,
    pub op: bool,
}

impl Visitor {
    pub fn new(
        inst: &Instance,
        claimed: Option<&str>,
        key: Option<&str>,
        address: Option<&str>,
    ) -> Visitor {
        let recognised = leaderboard::me(inst, address); // by the address of a login
        let op = recognised
            .as_ref()
            .is_some_and(|r| ops(inst).contains(&r.to_lowercase()));
        Visitor {
            claimed: known_player(inst, claimed),
            key_hash: key_hash(key),
            op,
            recognised,
        }
    }

    pub fn name(&self) -> Option<String> {
        self.claimed.clone().or_else(|| self.recognised.clone())
    }

    pub fn may_edit(&self, w: &Map<String, Value>) -> bool {
        if self.op {
            return true;
        }
        if let Some(h) = &self.key_hash {
            let stored = w.get("key").and_then(Value::as_str).unwrap_or("");
            if ct_eq(h.as_bytes(), stored.as_bytes()) {
                return true;
            }
        }
        let owner = w
            .get("owner")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        self.recognised
            .as_ref()
            .is_some_and(|r| r.to_lowercase() == owner)
    }
}

fn public(w: &Map<String, Value>, visitor: Option<&Visitor>) -> Value {
    let mut out = Map::new();
    for k in [
        "id", "name", "dim", "x", "y", "z", "color", "owner", "created",
    ] {
        out.insert(k.into(), w.get(k).cloned().unwrap_or(Value::Null));
    }
    out.insert(
        "can_edit".into(),
        json!(visitor.is_none_or(|v| v.may_edit(w))),
    );
    Value::Object(out)
}

/// All waypoints of a server (visitor None = the panel: may edit everything).
pub fn listing(inst: &Instance, visitor: Option<&Visitor>) -> Value {
    let items = {
        let _g = lock(&LOCK);
        load(inst)
    };
    let me = visitor.map(|v| json!({"name": v.name(), "recognised": v.recognised, "op": v.op}));
    // the names to pick yourself from (the hall of fame shows the same names)
    let mut names: Vec<String> = players::usercache(inst)
        .into_values()
        .map(|(n, _)| n)
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    json!({
        "waypoints": items.iter().map(|w| public(w, visitor)).collect::<Vec<_>>(),
        "me": me, "players": names, "colors": COLORS,
        "limits": {"name": NAME_MAX, "per_player": MAX_PER_PLAYER},
    })
}

fn new_id(items: &[Map<String, Value>]) -> String {
    loop {
        let wid = hex_token(6);
        if !items
            .iter()
            .any(|w| w.get("id").and_then(Value::as_str) == Some(wid.as_str()))
        {
            return wid;
        }
    }
}

/// A new waypoint. From the map: owned by the visitor (picked or recognised player); from the
/// panel: `owner` = any known player.
pub fn create(
    inst: &Instance,
    body: &Map<String, Value>,
    visitor: Option<&Visitor>,
    owner: Option<&str>,
) -> Result<Value> {
    let fields = validate_fields(body, false)?;
    let owner: Option<String> = match visitor {
        Some(v) => match v.name() {
            Some(n) => Some(n),
            None => return invalid!("Pick which player you are first"),
        },
        None => match owner.filter(|o| !o.is_empty()) {
            Some(o) => match known_player(inst, Some(o)) {
                Some(n) => Some(n),
                None => return invalid!("Unknown player"),
            },
            None => None, // the panel's own waypoints belong to nobody
        },
    };
    let _g = lock(&LOCK);
    let mut items = load(inst);
    if items.len() >= MAX_TOTAL {
        return invalid!("This server already has {MAX_TOTAL} waypoints");
    }
    if let Some(o) = &owner {
        let count = items
            .iter()
            .filter(|w| {
                w.get("owner")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase()
                    == o.to_lowercase()
            })
            .count();
        if count >= MAX_PER_PLAYER {
            return invalid!("{o} already has {MAX_PER_PLAYER} waypoints");
        }
    }
    let mut w = Map::new();
    w.insert("id".into(), json!(new_id(&items)));
    w.extend(fields);
    w.insert("owner".into(), json!(owner));
    w.insert("created".into(), json!(now_secs() as i64));
    w.insert(
        "key".into(),
        json!(visitor.and_then(|v| v.key_hash.clone())),
    );
    items.push(w.clone());
    save(inst, &items)?;
    Ok(public(&w, visitor))
}

fn find(items: &mut [Map<String, Value>], wid: &str) -> std::result::Result<usize, WpError> {
    if !ID_RE.is_match(wid) {
        return Err(WpError::NotFound);
    }
    items
        .iter()
        .position(|w| w.get("id").and_then(Value::as_str) == Some(wid))
        .ok_or(WpError::NotFound)
}

/// Change name / colour / position.
pub fn update(
    inst: &Instance,
    wid: &str,
    body: &Map<String, Value>,
    visitor: Option<&Visitor>,
) -> std::result::Result<Value, WpError> {
    let fields = validate_fields(body, true)?;
    let _g = lock(&LOCK);
    let mut items = load(inst);
    let i = find(&mut items, wid)?;
    if visitor.is_some_and(|v| !v.may_edit(&items[i])) {
        return Err(WpError::Forbidden(
            "Only its owner (or an operator) can change this waypoint".into(),
        ));
    }
    items[i].extend(fields);
    save(inst, &items)?;
    Ok(public(&items[i], visitor))
}

pub fn delete(
    inst: &Instance,
    wid: &str,
    visitor: Option<&Visitor>,
) -> std::result::Result<(), WpError> {
    let _g = lock(&LOCK);
    let mut items = load(inst);
    let i = find(&mut items, wid)?;
    if visitor.is_some_and(|v| !v.may_edit(&items[i])) {
        return Err(WpError::Forbidden(
            "Only its owner (or an operator) can delete this waypoint".into(),
        ));
    }
    items.remove(i);
    save(inst, &items)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn setup() -> Instance {
        let inst = Instance::main();
        std::fs::create_dir_all(inst.server_dir()).unwrap();
        std::fs::write(inst.server_dir().join("usercache.json"),
            r#"[{"name": "Steve", "uuid": "8667ba71-b85a-4004-af54-457a9734eed7"}, {"name": "Alex", "uuid": "8667ba71-b85a-4004-af54-457a9734eed8"}]"#).unwrap();
        inst
    }

    #[test]
    fn validation() {
        assert!(
            validate_fields(
                &body(json!({"name": "Base", "dim": "overworld", "x": 1, "z": 2})),
                false
            )
            .is_ok()
        );
        assert!(
            validate_fields(
                &body(json!({"name": "", "dim": "overworld", "x": 1, "z": 2})),
                false
            )
            .is_err()
        );
        assert!(
            validate_fields(
                &body(json!({"name": "x".repeat(33), "dim": "o", "x": 1, "z": 2})),
                false
            )
            .is_err()
        );
        assert!(
            validate_fields(
                &body(json!({"name": "a", "dim": "../x", "x": 1, "z": 2})),
                false
            )
            .is_err()
        );
        assert!(
            validate_fields(
                &body(json!({"name": "a", "dim": "o", "x": true, "z": 2})),
                false
            )
            .is_err()
        );
        assert!(
            validate_fields(
                &body(json!({"name": "a", "dim": "o", "x": 1, "z": 40000000})),
                false
            )
            .is_err()
        );
        assert!(
            validate_fields(
                &body(json!({"name": "a", "dim": "o", "x": 1, "z": 2, "color": "pink"})),
                false
            )
            .is_err()
        );
        let f = validate_fields(
            &body(json!({"name": " a \n b ", "dim": "o", "x": 1.9, "z": 2, "y": null})),
            false,
        )
        .unwrap();
        assert_eq!(f["name"], json!("a b"));
        assert_eq!(f["x"], json!(1));
        assert_eq!(f["color"], json!("green"));
        assert_eq!(
            validate_fields(&body(json!({"color": "red"})), true)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn owners_keys_and_panel() {
        let _t = crate::testutil::data_dir();
        let inst = setup();
        let key = "k".repeat(20);
        let steve = Visitor::new(&inst, Some("steve"), Some(&key), None);
        assert_eq!(steve.name().as_deref(), Some("Steve"));
        let w = create(
            &inst,
            &body(json!({"name": "Home", "dim": "overworld", "x": 1, "z": 2})),
            Some(&steve),
            None,
        )
        .unwrap();
        assert_eq!(w["owner"], json!("Steve"));
        assert_eq!(w["can_edit"], json!(true));
        let id = w["id"].as_str().unwrap().to_string();

        let other = Visitor::new(&inst, Some("Alex"), Some(&"o".repeat(20)), None);
        assert!(matches!(
            update(&inst, &id, &body(json!({"name": "Mine"})), Some(&other)),
            Err(WpError::Forbidden(_))
        ));
        assert!(matches!(
            delete(&inst, &id, Some(&other)),
            Err(WpError::Forbidden(_))
        ));
        assert_eq!(
            update(&inst, &id, &body(json!({"name": "Mine"})), Some(&steve))
                .ok()
                .unwrap()["name"],
            json!("Mine")
        );
        let listed = listing(&inst, Some(&other));
        assert_eq!(listed["waypoints"][0]["can_edit"], json!(false));
        assert_eq!(listed["players"], json!(["Alex", "Steve"]));
        assert!(
            !listed.to_string().contains("\"key\""),
            "key hashes are never sent"
        );

        let anonymous = Visitor::new(&inst, Some("Nobody"), None, None);
        assert!(
            create(
                &inst,
                &body(json!({"name": "x", "dim": "o", "x": 1, "z": 2})),
                Some(&anonymous),
                None
            )
            .is_err()
        );
        assert!(
            create(
                &inst,
                &body(json!({"name": "x", "dim": "o", "x": 1, "z": 2})),
                None,
                Some("Ghost")
            )
            .is_err()
        );
        let p = create(
            &inst,
            &body(json!({"name": "Spawn", "dim": "o", "x": 0, "z": 0})),
            None,
            None,
        )
        .unwrap();
        assert_eq!(p["owner"], Value::Null);

        // the panel may change everything
        delete(&inst, &id, None).ok().unwrap();
        assert!(matches!(delete(&inst, &id, None), Err(WpError::NotFound)));
        assert!(matches!(
            delete(&inst, "../x", None),
            Err(WpError::NotFound)
        ));
    }

    #[test]
    fn ops_by_address_and_limits() {
        let _t = crate::testutil::data_dir();
        let inst = setup();
        std::fs::write(
            inst.server_dir().join("ops.json"),
            r#"[{"name": "Alex", "level": 4}]"#,
        )
        .unwrap();
        leaderboard::remember_login(&inst, "Alex", "10.0.0.7");
        let steve = Visitor::new(&inst, Some("Steve"), None, None);
        let w = create(
            &inst,
            &body(json!({"name": "A", "dim": "o", "x": 1, "z": 2})),
            Some(&steve),
            None,
        )
        .unwrap();
        let op = Visitor::new(&inst, None, None, Some("10.0.0.7"));
        assert!(op.op);
        assert!(
            update(
                &inst,
                w["id"].as_str().unwrap(),
                &body(json!({"color": "red"})),
                Some(&op)
            )
            .is_ok()
        );
        for i in 1..MAX_PER_PLAYER {
            create(
                &inst,
                &body(json!({"name": format!("w{i}"), "dim": "o", "x": 1, "z": 2})),
                Some(&steve),
                None,
            )
            .unwrap();
        }
        assert!(
            create(
                &inst,
                &body(json!({"name": "one too many", "dim": "o", "x": 1, "z": 2})),
                Some(&steve),
                None
            )
            .is_err()
        );
    }
}
