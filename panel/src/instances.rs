//! Server instances: several independent Minecraft servers in one panel.
//!
//! The original installation is the instance `main` and keeps its old paths (`DATA_DIR/server`,
//! `BACKUP_DIR`, `DATA_DIR/settings.json`, `DATA_DIR/map`), so nothing has to be migrated. Further
//! instances live in `DATA_DIR/servers/<id>/` with their backups in `BACKUP_DIR/<id>/`. Jars, Java
//! runtimes and the data pack library are shared.
//!
//! Paths are computed on every access from `config`, so tests (and env overrides) can redirect them.

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::cfg;
use crate::error::{Error, Result};
use crate::util::{lock, read_json, to_json_indent};
use crate::{access, invalid};

pub const MAIN_ID: &str = "main";
// Becomes part of file paths: keep it strict
static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,31}$").unwrap());
const NAME_MAX: usize = 40;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Instance {
    pub id: String,
}

impl Instance {
    pub fn new(id: &str) -> Instance {
        Instance { id: id.to_string() }
    }

    pub fn main() -> Instance {
        Instance::new(MAIN_ID)
    }

    pub fn is_main(&self) -> bool {
        self.id == MAIN_ID
    }

    pub fn root(&self) -> PathBuf {
        let c = cfg();
        if self.is_main() {
            c.data_dir.clone()
        } else {
            c.data_dir.join("servers").join(&self.id)
        }
    }

    pub fn server_dir(&self) -> PathBuf {
        if self.is_main() {
            cfg().server_dir.clone()
        } else {
            self.root().join("server")
        }
    }

    pub fn backup_dir(&self) -> PathBuf {
        let c = cfg();
        if self.is_main() {
            c.backup_dir.clone()
        } else {
            c.backup_dir.join(&self.id)
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        if self.is_main() {
            cfg().settings_file.clone()
        } else {
            self.root().join("settings.json")
        }
    }

    pub fn map_dir(&self) -> PathBuf {
        if self.is_main() {
            cfg().data_dir.join("map")
        } else {
            self.root().join("map")
        }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.server_dir())?;
        std::fs::create_dir_all(self.backup_dir())
    }
}

pub fn validate_id(value: &str) -> Result<&str> {
    if !ID_RE.is_match(value) {
        return invalid!("Invalid server id");
    }
    Ok(value)
}

pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX || name.contains(['\r', '\n', '\t']) {
        return invalid!("Server name: 1-{NAME_MAX} characters, single line");
    }
    Ok(name.to_string())
}

// ---------- registry (DATA_DIR/servers.json) ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
}

fn registry_file() -> PathBuf {
    cfg().data_dir.join("servers.json")
}

/// [{id, name}], always starting with main.
fn read() -> Vec<Entry> {
    let mut entries = Vec::new();
    if let Some(Value::Array(items)) = read_json(&registry_file()) {
        for item in items {
            if let (Some(Value::String(id)), Some(Value::String(name))) =
                (item.get("id"), item.get("name"))
            {
                if ID_RE.is_match(id) {
                    entries.push(Entry {
                        id: id.clone(),
                        name: name.clone(),
                    });
                }
            }
        }
    }
    if !entries.iter().any(|e| e.id == MAIN_ID) {
        entries.insert(
            0,
            Entry {
                id: MAIN_ID.into(),
                name: "Server".into(),
            },
        );
    }
    entries
}

fn write(entries: &[Entry]) -> std::io::Result<()> {
    let path = registry_file();
    let value = serde_json::to_value(entries).unwrap_or(Value::Null);
    crate::util::write_atomic(&path, to_json_indent(&value, 2).as_bytes())
}

pub fn all_entries() -> Vec<Entry> {
    let _g = lock(&LOCK);
    read()
}

/// NotFound if there is no such server.
pub fn get(server_id: &str) -> Result<Instance> {
    if !all_entries().iter().any(|e| e.id == server_id) {
        return Err(Error::NotFound(server_id.to_string()));
    }
    Ok(Instance::new(server_id))
}

pub fn name_of(inst: &Instance) -> String {
    all_entries()
        .into_iter()
        .find(|e| e.id == inst.id)
        .map(|e| e.name)
        .unwrap_or_else(|| inst.id.clone())
}

fn slug(name: &str) -> String {
    static NON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
    let s = NON
        .replace_all(&name.to_lowercase(), "-")
        .trim_matches('-')
        .to_string();
    let s: String = s.chars().take(24).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "server".into() } else { s }
}

/// Register a new instance and create its folders. The caller writes its settings.
pub fn create(name: &str) -> Result<Instance> {
    let name = validate_name(name)?;
    let _g = lock(&LOCK);
    let mut entries = read();
    let base = slug(&name);
    let (mut sid, mut n) = (base.clone(), 2);
    while entries.iter().any(|e| e.id == sid) || sid == MAIN_ID {
        sid = format!("{base}-{n}");
        n += 1;
    }
    validate_id(&sid)?;
    let inst = Instance::new(&sid);
    inst.ensure_dirs()?;
    entries.push(Entry { id: sid, name });
    write(&entries)?;
    Ok(inst)
}

pub fn rename(inst: &Instance, name: &str) -> Result<()> {
    let name = validate_name(name)?;
    let _g = lock(&LOCK);
    let mut entries = read();
    for e in entries.iter_mut().filter(|e| e.id == inst.id) {
        e.name = name.clone();
    }
    write(&entries)?;
    Ok(())
}

/// Delete the server folder (world, settings, map). Its backups are kept.
pub fn remove(inst: &Instance) -> Result<()> {
    if inst.is_main() {
        return invalid!("The first server can't be deleted");
    }
    {
        let _g = lock(&LOCK);
        let entries: Vec<Entry> = read().into_iter().filter(|e| e.id != inst.id).collect();
        write(&entries)?;
    }
    let _ = std::fs::remove_dir_all(inst.root());
    Ok(())
}

// ---------- game ports ----------

/// server-port from server.properties (Minecraft's default if missing).
pub fn game_port(inst: &Instance) -> u16 {
    access::get_property(inst, "server-port")
        .and_then(|v| {
            if v.is_empty() {
                Some(25565)
            } else {
                v.parse().ok()
            }
        })
        .unwrap_or(25565)
}

/// Set the game port chosen in the UI: 1024–65535, not the panel's own ports, not used by another
/// server of this panel. (The caller makes sure the server is stopped.)
pub fn set_port(inst: &Instance, port: u16) -> Result<()> {
    if port < 1024 {
        return invalid!("Choose a port from 1024 to 65535");
    }
    let c = cfg();
    if port == c.web_port || port == c.map_port {
        return invalid!("Port {port} is used by the panel itself");
    }
    if let Some(other) = all_entries()
        .iter()
        .find(|e| e.id != inst.id && game_port(&Instance::new(&e.id)) == port)
    {
        return invalid!("Port {port} is already used by the server '{}'", other.name);
    }
    access::set_property(inst, "server-port", &port.to_string())?;
    Ok(())
}

/// First port of SERVER_PORTS that no other instance uses.
pub fn free_port(exclude: Option<&Instance>) -> Result<u16> {
    let used: Vec<u16> = all_entries()
        .iter()
        .filter(|e| exclude.is_none_or(|x| x.id != e.id))
        .map(|e| game_port(&Instance::new(&e.id)))
        .collect();
    let ports = &cfg().server_ports;
    ports
        .iter()
        .copied()
        .find(|p| !used.contains(p))
        .ok_or_else(|| {
            Error::Invalid(format!(
                "No free game port left in SERVER_PORTS ({}-{})",
                ports.first().unwrap_or(&0),
                ports.last().unwrap_or(&0)
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_keeps_old_paths() {
        let _t = crate::testutil::data_dir();
        let c = cfg();
        let m = Instance::main();
        assert_eq!(m.server_dir(), c.server_dir);
        assert_eq!(m.backup_dir(), c.backup_dir);
        assert_eq!(m.settings_file(), c.settings_file);
        assert_eq!(m.map_dir(), c.data_dir.join("map"));
        let o = Instance::new("creative");
        assert_eq!(o.server_dir(), c.data_dir.join("servers/creative/server"));
        assert_eq!(o.backup_dir(), c.backup_dir.join("creative"));
    }

    #[test]
    fn create_rename_remove() {
        let _t = crate::testutil::data_dir();
        assert_eq!(
            all_entries(),
            vec![Entry {
                id: "main".into(),
                name: "Server".into()
            }]
        );
        let a = create("Creative World!").unwrap();
        assert_eq!(a.id, "creative-world");
        let b = create("Creative World").unwrap();
        assert_eq!(b.id, "creative-world-2");
        assert!(a.server_dir().is_dir());
        let main = create("main").unwrap();
        assert_eq!(main.id, "main-2");
        rename(&a, "New name").unwrap();
        assert_eq!(name_of(&a), "New name");
        remove(&a).unwrap();
        assert!(!a.root().exists());
        assert!(get(&a.id).is_err());
        assert!(remove(&Instance::main()).is_err());
        assert_eq!(slug("!!!"), "server");
    }

    #[test]
    fn ids_and_names() {
        assert!(validate_id("main").is_ok());
        assert!(validate_id("../x").is_err());
        assert!(validate_id("Main").is_err());
        assert!(validate_name("  ").is_err());
        assert!(validate_name("a\nb").is_err());
        assert_eq!(validate_name(" Survival ").unwrap(), "Survival");
    }

    #[test]
    fn free_ports() {
        let _t = crate::testutil::data_dir();
        access::set_property(&Instance::main(), "server-port", "25565").unwrap();
        assert_eq!(free_port(None).unwrap(), 25566);
        let b = create("B").unwrap();
        access::set_property(&b, "server-port", "25566").unwrap();
        assert_eq!(free_port(None).unwrap(), 25567);
        assert_eq!(free_port(Some(&b)).unwrap(), 25566);
    }

    #[test]
    fn set_ports() {
        let _t = crate::testutil::data_dir();
        access::set_property(&Instance::main(), "server-port", "25565").unwrap();
        let b = create("B").unwrap();
        assert!(set_port(&b, 80).is_err());
        assert!(set_port(&b, 25565).is_err()); // main has it
        assert!(set_port(&b, cfg().web_port).is_err());
        set_port(&b, 30000).unwrap();
        assert_eq!(game_port(&b), 30000);
        set_port(&b, 30000).unwrap(); // its own port again is fine
    }
}
