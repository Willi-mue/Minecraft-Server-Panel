//! Whitelist, operator (OP) and ban management.
//!
//! While the server runs, changes go through its own commands (`whitelist add`, `op`, ...) so they
//! take effect immediately and the server writes the JSON files itself. While it is stopped,
//! `whitelist.json` / `ops.json` / `server.properties` are edited directly. Every function takes
//! the server instance whose files it works on.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use md5::{Digest, Md5};
use regex::Regex;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::instances::Instance;
use crate::util::{mtime_ns, read_json, to_json_indent};
use crate::{http, invalid};

static NAME_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\w{3,16}$").unwrap());
const PROFILE_URL: &str = "https://api.mojang.com/users/profiles/minecraft/";
pub const REASON_MAX: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Whitelist,
    Ops,
    Bans,
}

impl Kind {
    pub fn file(self) -> &'static str {
        match self {
            Kind::Whitelist => "whitelist.json",
            Kind::Ops => "ops.json",
            Kind::Bans => "banned-players.json",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Whitelist => "whitelist",
            Kind::Ops => "operators",
            Kind::Bans => "ban list",
        }
    }
}

pub fn validate_reason(reason: &str) -> Result<String> {
    let reason = reason.trim();
    if reason.chars().count() > REASON_MAX || reason.contains(['\n', '\r']) {
        return invalid!("Reason: up to {REASON_MAX} characters, single line");
    }
    Ok(reason.to_string())
}

pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    // \w in Python is Unicode-aware, Minecraft names are ASCII: keep it strict
    if !NAME_RE.is_match(name) || !name.is_ascii() {
        return invalid!("Player names have 3-16 characters: letters, digits, underscore");
    }
    Ok(name.to_string())
}

fn file(inst: &Instance, name: &str) -> PathBuf {
    inst.server_dir().join(name)
}

pub fn read_list(inst: &Instance, name: &str) -> Vec<Value> {
    match read_json(&file(inst, name)) {
        Some(Value::Array(a)) => a,
        _ => vec![],
    }
}

fn write_list(inst: &Instance, name: &str, entries: &[Value]) -> std::io::Result<()> {
    std::fs::write(
        file(inst, name),
        to_json_indent(&Value::Array(entries.to_vec()), 2),
    )
}

// ---------- server.properties ----------

fn properties(inst: &Instance) -> Vec<String> {
    std::fs::read(file(inst, "server.properties"))
        .map(|b| {
            String::from_utf8_lossy(&b)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The value of a key (trimmed), None if it isn't in the file.
pub fn get_property(inst: &Instance, key: &str) -> Option<String> {
    properties(inst).iter().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

pub fn set_property(inst: &Instance, key: &str, value: &str) -> std::io::Result<()> {
    let mut lines = properties(inst);
    let pos = lines
        .iter()
        .position(|line| line.split_once('=').is_some_and(|(k, _)| k.trim() == key));
    match pos {
        Some(i) => lines[i] = format!("{key}={value}"),
        None => lines.push(format!("{key}={value}")),
    }
    let path = file(inst, "server.properties");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, lines.join("\n") + "\n")
}

pub fn online_mode(inst: &Instance) -> bool {
    get_property(inst, "online-mode")
        .unwrap_or_else(|| "true".into())
        .to_lowercase()
        != "false"
}

// ---------- UUIDs ----------

fn dashed(hex32: &str) -> String {
    let u = hex32.replace('-', "").to_lowercase();
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

/// Same as Java's UUID.nameUUIDFromBytes("OfflinePlayer:" + name)
pub fn offline_uuid(name: &str) -> String {
    let mut d: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    d[6] = (d[6] & 0x0F) | 0x30;
    d[8] = (d[8] & 0x3F) | 0x80;
    dashed(&hex::encode(d))
}

/// Name -> {"uuid", "name"} with the correct spelling.
pub fn resolve(inst: &Instance, name: &str) -> Result<(String, String)> {
    if !online_mode(inst) {
        return Ok((offline_uuid(name), name.to_string()));
    }
    let (status, data) = http::get_json_status(&format!("{PROFILE_URL}{}", http::quote(name)))?;
    if status == 204 || status == 404 {
        return Err(Error::NotFound(format!(
            "No Minecraft account named {name}"
        )));
    }
    if status != 200 {
        return Err(Error::Remote(format!("HTTP Error {status}")));
    }
    match (
        data.get("id").and_then(Value::as_str),
        data.get("name").and_then(Value::as_str),
    ) {
        (Some(id), Some(n)) => Ok((dashed(id), n.to_string())),
        _ => Err(Error::Remote("Unexpected answer from Mojang".into())),
    }
}

// ---------- reading ----------

fn str_of(e: &Value, key: &str, default: &str) -> Value {
    e.get(key)
        .cloned()
        .unwrap_or_else(|| Value::String(default.into()))
}

fn name_key(v: &Value) -> String {
    v.get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase()
}

pub fn state(inst: &Instance) -> Value {
    let mut whitelist: Vec<Value> = read_list(inst, "whitelist.json")
        .iter()
        .map(|e| str_of(e, "name", "?"))
        .collect();
    whitelist.sort_by_key(|v| v.as_str().unwrap_or("").to_lowercase());
    let mut ops: Vec<Value> = read_list(inst, "ops.json")
        .iter()
        .map(|e| json!({"name": str_of(e, "name", "?"), "level": e.get("level").cloned().unwrap_or(json!(4))}))
        .collect();
    ops.sort_by_key(name_key);
    let mut bans: Vec<Value> = read_list(inst, "banned-players.json")
        .iter()
        .map(|e| {
            json!({"name": str_of(e, "name", "?"), "reason": str_of(e, "reason", ""), "source": str_of(e, "source", ""),
                   "created": str_of(e, "created", ""), "expires": str_of(e, "expires", "forever")})
        })
        .collect();
    bans.sort_by_key(name_key);
    json!({
        "whitelist_enabled": get_property(inst, "white-list").unwrap_or_else(|| "false".into()).to_lowercase() == "true",
        "whitelist": whitelist,
        "ops": ops,
        "bans": bans,
    })
}

// ---------- changes while the server is stopped ----------

/// action: add (true) / remove (false). Returns a message.
pub fn offline_change(
    inst: &Instance,
    kind: Kind,
    add: bool,
    name: &str,
    reason: &str,
) -> Result<String> {
    let file = kind.file();
    let entries = read_list(inst, file);
    let same = |e: &Value| {
        e.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase()
            == name.to_lowercase()
    };
    if !add {
        let kept: Vec<Value> = entries.iter().filter(|e| !same(e)).cloned().collect();
        if kept.len() == entries.len() {
            return Err(Error::NotFound(match kind {
                Kind::Whitelist => format!("{name} is not on the whitelist"),
                Kind::Ops => format!("{name} is not an operator"),
                Kind::Bans => format!("{name} is not banned"),
            }));
        }
        write_list(inst, file, &kept)?;
        return Ok(if kind == Kind::Bans {
            format!("Unbanned {name}")
        } else {
            format!("Removed {name} from the {}", kind.label())
        });
    }
    if entries.iter().any(same) {
        return Err(Error::NotFound(match kind {
            Kind::Whitelist => format!("{name} is already on the whitelist"),
            Kind::Ops => format!("{name} is already an operator"),
            Kind::Bans => format!("{name} is already banned"),
        }));
    }
    let (uuid, real) = resolve(inst, name)?;
    let mut entry = json!({"uuid": uuid, "name": real});
    match kind {
        Kind::Ops => {
            entry["level"] = json!(4);
            entry["bypassesPlayerLimit"] = json!(false);
        }
        Kind::Bans => {
            // Same fields and date format the server writes
            entry["created"] = json!(crate::util::local_now("%Y-%m-%d %H:%M:%S %z"));
            entry["source"] = json!("Server panel");
            entry["expires"] = json!("forever");
            entry["reason"] = json!(if reason.is_empty() {
                "Banned by an operator."
            } else {
                reason
            });
        }
        Kind::Whitelist => {}
    }
    let mut entries = entries;
    entries.push(entry);
    write_list(inst, file, &entries)?;
    Ok(if kind == Kind::Bans {
        format!("Banned {real}")
    } else {
        format!("Added {real} to the {}", kind.label())
    })
}

pub fn mtime(inst: &Instance, name: &str) -> Option<u128> {
    mtime_ns(&file(inst, name))
}

/// After a server command, wait until the server has rewritten the file.
pub fn wait_for_change(inst: &Instance, name: &str, before: Option<u128>, timeout: Duration) {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if mtime(inst, name) != before {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_java() {
        // UUID.nameUUIDFromBytes("OfflinePlayer:Notch")
        assert_eq!(
            offline_uuid("Notch"),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
    }

    #[test]
    fn properties_keep_other_lines() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        std::fs::create_dir_all(inst.server_dir()).unwrap();
        std::fs::write(
            inst.server_dir().join("server.properties"),
            "#comment\nmotd=Hi\nwhite-list=false\n",
        )
        .unwrap();
        set_property(&inst, "white-list", "true").unwrap();
        set_property(&inst, "new-key", "x").unwrap();
        let text = std::fs::read_to_string(inst.server_dir().join("server.properties")).unwrap();
        assert_eq!(text, "#comment\nmotd=Hi\nwhite-list=true\nnew-key=x\n");
        assert_eq!(get_property(&inst, "motd").as_deref(), Some("Hi"));
        assert_eq!(get_property(&inst, "missing"), None);
    }

    #[test]
    fn offline_changes_in_offline_mode() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        set_property(&inst, "online-mode", "false").unwrap();
        assert_eq!(
            offline_change(&inst, Kind::Whitelist, true, "Steve", "").unwrap(),
            "Added Steve to the whitelist"
        );
        assert!(matches!(
            offline_change(&inst, Kind::Whitelist, true, "steve", ""),
            Err(Error::NotFound(_))
        ));
        offline_change(&inst, Kind::Ops, true, "Alex", "").unwrap();
        offline_change(&inst, Kind::Bans, true, "Griefer", "TNT").unwrap();
        let s = state(&inst);
        assert_eq!(s["whitelist"], json!(["Steve"]));
        assert_eq!(s["ops"][0]["level"], json!(4));
        assert_eq!(s["bans"][0]["reason"], json!("TNT"));
        assert_eq!(
            offline_change(&inst, Kind::Bans, false, "griefer", "").unwrap(),
            "Unbanned griefer"
        );
        assert!(matches!(
            offline_change(&inst, Kind::Ops, false, "Nobody", ""),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn validation() {
        assert!(validate_name("Steve_1").is_ok());
        assert!(validate_name("ab").is_err());
        assert!(validate_name("a b c").is_err());
        assert!(validate_name("Stéve").is_err());
        assert!(validate_reason("x\ny").is_err());
        assert!(validate_reason(&"x".repeat(201)).is_err());
    }
}
