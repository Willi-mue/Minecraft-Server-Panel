//! Runtime configuration.
//!
//! Static settings come from environment variables (see `.env.example`). Settings the user changes
//! in the web UI (version, RAM) are persisted per server in its `settings.json` (first server:
//! `<DATA_DIR>/settings.json`) and override the environment defaults.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::Result;
use crate::instances::Instance;
use crate::invalid;
use crate::util::{lock, read_json, to_json_indent};

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub server_dir: PathBuf,
    pub jars_dir: PathBuf,
    pub backup_dir: PathBuf,
    pub settings_file: PathBuf,
    pub web_host: String,
    pub web_port: u16,
    pub password: String,
    pub map_port: u16,
    pub map_public_port: u16,
    pub map_threads: u32,
    pub map_interval: u64,
    pub mapgen_bin: String,
    pub map_textures: bool,
    pub cave_reach_default: i64,
    pub all_caves_default: bool,
    pub backup_interval_hours: f64,
    pub backup_keep: usize,
    pub java_dir: PathBuf,
    pub server_ports: Vec<u16>,
    pub world_upload_max: u64,
    pub static_dir: PathBuf,
    /// Defaults of a new `settings.json`
    pub default_flavor: String,
    pub default_version: String,
    pub default_min_ram: String,
    pub default_max_ram: String,
    pub default_map_enabled: bool,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn absolute(p: PathBuf) -> PathBuf {
    let p = if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    // like Path.resolve() without requiring the path to exist
    p.canonicalize().map(strip_unc).unwrap_or(p)
}

/// `\\?\C:\...` -> `C:\...` (Windows canonical paths), so paths stay readable and comparable.
pub fn strip_unc(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

/// "25565-25569" -> [25565, ..., 25569]; a single port is allowed too.
pub fn port_range(text: &str) -> std::result::Result<Vec<u16>, String> {
    let (lo, hi) = text.trim().split_once('-').unwrap_or((text.trim(), ""));
    let start: u32 = lo
        .trim()
        .parse()
        .map_err(|_| format!("Invalid SERVER_PORTS: {text}"))?;
    let end: u32 = if hi.trim().is_empty() {
        start
    } else {
        hi.trim()
            .parse()
            .map_err(|_| format!("Invalid SERVER_PORTS: {text}"))?
    };
    if !(1 <= start && start <= end && end <= 65535) {
        return Err(format!("Invalid SERVER_PORTS: {text}"));
    }
    Ok((start..=end).map(|p| p as u16).collect())
}

fn find_static_dir() -> PathBuf {
    if let Some(dir) = env("STATIC_DIR") {
        return PathBuf::from(dir);
    }
    let mut candidates = vec![PathBuf::from("app/static"), PathBuf::from("../app/static")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("static"));
        }
    }
    // cargo run / target/release/panel inside the repository
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("app")
            .join("static"),
    );
    candidates
        .into_iter()
        .find(|c| c.join("index.html").is_file())
        .map(absolute)
        .unwrap_or_else(|| PathBuf::from("app/static"))
}

impl Config {
    pub fn from_env() -> std::result::Result<Config, String> {
        let int = |key: &str, default: i64| -> std::result::Result<i64, String> {
            match env(key) {
                Some(v) => v
                    .trim()
                    .parse()
                    .map_err(|_| format!("{key} must be a whole number")),
                None => Ok(default),
            }
        };
        let data_dir = absolute(PathBuf::from(
            env("DATA_DIR").unwrap_or_else(|| "data".into()),
        ));
        let backup_dir = absolute(
            env("BACKUP_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("backups")),
        );
        let map_port = int("MAP_PORT", 47319)? as u16;
        let upload_gb: f64 = env("WORLD_UPLOAD_MAX_GB")
            .and_then(|v| v.parse().ok())
            .unwrap_or(4.0);
        Ok(Config {
            server_dir: data_dir.join("server"),
            jars_dir: data_dir.join("jars"),
            settings_file: data_dir.join("settings.json"),
            backup_dir,
            web_host: env("WEB_HOST").unwrap_or_else(|| "0.0.0.0".into()),
            web_port: int("WEB_PORT", 47318)? as u16,
            // MCGUI_PASSWORD = old name, still read so existing .env files keep working
            password: env("PANEL_PASSWORD")
                .or_else(|| env("MCGUI_PASSWORD"))
                .unwrap_or_default(),
            map_port,
            map_public_port: int("MAP_PORT_HOST", map_port as i64)? as u16,
            map_threads: int("MAP_THREADS", 2)?.max(1) as u32,
            map_interval: int("MAP_INTERVAL", 60)?.max(15) as u64,
            mapgen_bin: env("MAPGEN_BIN").unwrap_or_default(),
            map_textures: env("MAP_TEXTURES")
                .map(|v| v.to_lowercase() != "false")
                .unwrap_or(true),
            cave_reach_default: int("MAP_CAVE_REACH", 32)?.clamp(0, 256),
            all_caves_default: env("MAP_ALL_CAVES")
                .map(|v| {
                    matches!(
                        v.trim().to_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false),
            backup_interval_hours: env("BACKUP_INTERVAL_HOURS")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(24.0)
                .max(0.0),
            // At least 1, otherwise cleanup would delete the backup that was just created
            backup_keep: int("BACKUP_KEEP", 7)?.max(1) as usize,
            java_dir: PathBuf::from(env("JAVA_DIR").unwrap_or_else(|| "/opt/java".into())),
            server_ports: port_range(&env("SERVER_PORTS").unwrap_or_else(|| "25565-25569".into()))?,
            world_upload_max: (upload_gb * (1u64 << 30) as f64) as u64,
            static_dir: find_static_dir(),
            default_flavor: env("MC_FLAVOR").unwrap_or_else(|| "paper".into()),
            default_version: env("MC_VERSION").unwrap_or_default(),
            default_min_ram: env("MIN_RAM").unwrap_or_else(|| "1G".into()),
            default_max_ram: env("MAX_RAM").unwrap_or_else(|| "3G".into()),
            default_map_enabled: env("MAP_ENABLED")
                .map(|v| v.to_lowercase() != "false")
                .unwrap_or(true),
            data_dir,
        })
    }

    /// Configuration rooted in `dir` (tests).
    #[cfg(test)]
    pub fn for_dir(dir: &Path) -> Config {
        let mut c = Config::from_env().expect("config");
        c.data_dir = dir.to_path_buf();
        c.server_dir = dir.join("server");
        c.jars_dir = dir.join("jars");
        c.backup_dir = dir.join("backups");
        c.settings_file = dir.join("settings.json");
        c.java_dir = dir.join("java");
        c.backup_keep = 7;
        c.password = "test-password".into();
        c.map_textures = false;
        c.server_ports = (25565..=25569).collect();
        c
    }
}

static CONFIG: LazyLock<RwLock<Arc<Config>>> = LazyLock::new(|| {
    RwLock::new(Arc::new(
        Config::from_env().unwrap_or_else(|e| panic!("{e}")),
    ))
});

/// The current configuration.
pub fn cfg() -> Arc<Config> {
    CONFIG.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Replace the configuration (startup validation, tests).
pub fn set(c: Config) {
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(c);
}

pub fn ensure_dirs() -> std::io::Result<()> {
    let c = cfg();
    for d in [&c.data_dir, &c.server_dir, &c.jars_dir, &c.backup_dir] {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}

// ---------- per-server settings.json ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Settings {
    pub flavor: String,
    pub version: String,
    pub min_ram: String,
    pub max_ram: String,
    pub map_enabled: bool,
    /// Version/flavor that last ran the world (for upgrade/downgrade warnings)
    pub world_version: String,
    pub world_flavor: String,
}

impl Default for Settings {
    fn default() -> Self {
        let c = cfg();
        Settings {
            flavor: c.default_flavor.clone(),
            version: c.default_version.clone(),
            min_ram: c.default_min_ram.clone(),
            max_ram: c.default_max_ram.clone(),
            map_enabled: c.default_map_enabled,
            world_version: String::new(),
            world_flavor: String::new(),
        }
    }
}

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

pub fn load_settings(inst: &Instance) -> Settings {
    let path = inst.settings_file();
    let _g = lock(&SETTINGS_LOCK);
    let mut s = Settings::default();
    if let Some(Value::Object(data)) = read_json(&path) {
        // ignore wrong/missing types
        let text = |key: &str, target: &mut String| {
            if let Some(Value::String(v)) = data.get(key) {
                *target = v.clone();
            }
        };
        text("flavor", &mut s.flavor);
        text("version", &mut s.version);
        text("min_ram", &mut s.min_ram);
        text("max_ram", &mut s.max_ram);
        text("world_version", &mut s.world_version);
        text("world_flavor", &mut s.world_flavor);
        if let Some(Value::Bool(b)) = data.get("map_enabled") {
            s.map_enabled = *b;
        }
    }
    s
}

pub fn save_settings(s: &Settings, inst: &Instance) -> std::io::Result<()> {
    let path = inst.settings_file();
    let _g = lock(&SETTINGS_LOCK);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let value = serde_json::to_value(s).unwrap_or(Value::Object(Map::new()));
    std::fs::write(&path, to_json_indent(&value, 2))
}

static RAM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[1-9]\d*[MG]$").unwrap());
// Mojang version ids: "1.21.4", "26.4-snapshot-7", "1.14 Pre-Release 1", ... (no path separators)
static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._\- ]{0,63}$").unwrap());

pub fn validate_ram(value: &str) -> Result<String> {
    let value = value.trim().to_uppercase();
    if !RAM_RE.is_match(&value) {
        return invalid!("RAM must look like 512M or 4G");
    }
    Ok(value)
}

pub fn validate_version(value: &str) -> Result<String> {
    let value = value.trim();
    if !VERSION_RE.is_match(value) {
        return invalid!("Invalid version id");
    }
    Ok(value.to_string())
}

pub fn ram_mb(value: &str) -> u64 {
    let n: u64 = value[..value.len().saturating_sub(1)].parse().unwrap_or(0);
    if value.ends_with('G') { n * 1024 } else { n }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports() {
        assert_eq!(
            port_range("25565-25567").unwrap(),
            vec![25565, 25566, 25567]
        );
        assert_eq!(port_range("25570").unwrap(), vec![25570]);
        assert!(port_range("5-1").is_err());
        assert!(port_range("x").is_err());
    }

    #[test]
    fn ram_and_versions() {
        assert_eq!(validate_ram(" 4g ").unwrap(), "4G");
        assert!(validate_ram("0G").is_err());
        assert!(validate_ram("4GB").is_err());
        assert_eq!(ram_mb("2G"), 2048);
        assert_eq!(ram_mb("512M"), 512);
        assert!(validate_version("1.21.4").is_ok());
        assert!(validate_version("1.14 Pre-Release 1").is_ok());
        assert!(validate_version("../x").is_err());
        assert!(validate_version("a/b").is_err());
    }

    #[test]
    fn settings_round_trip_and_wrong_types() {
        let _t = crate::testutil::data_dir();
        let inst = Instance::main();
        let mut s = load_settings(&inst);
        s.version = "1.21.4".into();
        s.map_enabled = false;
        save_settings(&s, &inst).unwrap();
        assert_eq!(load_settings(&inst), s);
        std::fs::write(inst.settings_file(), r#"{"version": 5, "max_ram": "8G"}"#).unwrap();
        let s = load_settings(&inst);
        assert_eq!(s.version, Settings::default().version);
        assert_eq!(s.max_ram, "8G");
    }
}
