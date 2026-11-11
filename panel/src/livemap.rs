//! Live map: renders the world files with `mapgen` (Rust, see mapgen/) and tracks players.
//!
//! Works for every server type because it reads the world files directly; nothing runs inside
//! Minecraft. Rendering is incremental (only changed region files) and runs with low priority.
//! Player positions are polled via `data get entity` only while somebody looks at the map. Every
//! server instance has its own map (`Instance::map_dir`), renderer and player tracker.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::config::{self, cfg};
use crate::error::{Error, Result};
use crate::http::{self, Hash};
use crate::instances::{self, Instance};
use crate::skins::{SKINS, skin_path};
use crate::util::{lock, read_json, to_json_indent};
use crate::{backup, invalid, versions};

static CLIENT_FAILED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

pub fn out_dir(inst: &Instance) -> PathBuf {
    inst.map_dir()
}

// ---------- what the 3D map shows below the surface ----------
// Normally caves are shown only as far as they are open to the sky (`cave_reach` columns from the
// opening). With `all_caves`, mapgen also writes stacks with every cave (`c/`), which the viewer
// loads while its height slice is on (that reveals hidden caves to everyone who can open the map).
// Kept in DATA_DIR/map_visibility.json (not in the map folder: that one is public).

fn visibility_file() -> PathBuf {
    cfg().data_dir.join("map_visibility.json")
}

/// Checked copy of {"cave_reach": int, "all_caves": bool}.
pub fn validate_visibility(data: &Map<String, Value>) -> Result<Value> {
    let c = cfg();
    let reach = match data.get("cave_reach") {
        None => c.cave_reach_default,
        Some(v) => match v.as_i64() {
            Some(n) if v.is_i64() && (0..=256).contains(&n) => n,
            _ => return invalid!("Cave reach must be a whole number from 0 to 256"),
        },
    };
    let all_caves = match data.get("all_caves") {
        None => c.all_caves_default,
        Some(Value::Bool(b)) => *b,
        Some(_) => return invalid!("all_caves must be true or false"),
    };
    Ok(json!({"cave_reach": reach, "all_caves": all_caves}))
}

pub fn load_visibility() -> Value {
    read_json(&visibility_file())
        .and_then(|v| v.as_object().cloned())
        .and_then(|m| validate_visibility(&m).ok())
        .unwrap_or_else(|| json!({"cave_reach": cfg().cave_reach_default, "all_caves": cfg().all_caves_default}))
}

/// Validate and store (keys left out keep their value); the next render re-renders the 3D stacks.
pub fn save_visibility(data: &Map<String, Value>) -> Result<Value> {
    let mut merged = load_visibility().as_object().cloned().unwrap_or_default();
    for (k, v) in data {
        merged.insert(k.clone(), v.clone());
    }
    let v = validate_visibility(&merged)?;
    std::fs::write(visibility_file(), to_json_indent(&v, 2))?;
    Ok(v)
}

fn visibility_args() -> Vec<String> {
    let v = load_visibility();
    let mut args = vec!["--cave-reach".to_string(), v["cave_reach"].to_string()];
    if v["all_caves"] == json!(true) {
        args.push("--all-caves".into());
    }
    args
}

pub fn available(inst: &Instance) -> bool {
    out_dir(inst).join("maps.json").exists()
}

/// Forget the rendered map (after a new world was created).
pub fn clear(inst: &Instance) {
    let _ = std::fs::remove_dir_all(out_dir(inst));
}

pub fn find_binary() -> Option<PathBuf> {
    let c = cfg();
    if !c.mapgen_bin.is_empty() {
        let p = PathBuf::from(&c.mapgen_bin);
        return p.exists().then_some(p);
    }
    if let Some(found) = crate::java::which_bin("mapgen") {
        return Some(found);
    }
    let exe = if cfg!(windows) {
        "mapgen.exe"
    } else {
        "mapgen"
    };
    let mut candidates = vec![
        PathBuf::from("mapgen/target/release").join(exe),
        PathBuf::from("../mapgen/target/release").join(exe),
    ];
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mapgen/target/release")
            .join(exe),
    );
    candidates.into_iter().find(|p| p.exists())
}

/// The map used to be the squaremap plugin (Paper only); remove it if still installed.
pub fn remove_squaremap(inst: &Instance) -> std::io::Result<bool> {
    let mut removed = false;
    if let Ok(rd) = std::fs::read_dir(inst.server_dir().join("plugins")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("squaremap") && name.ends_with(".jar") {
                std::fs::remove_file(e.path())?;
                removed = true;
            }
        }
    }
    Ok(removed)
}

/// The Minecraft version whose client jar the map of this server needs (validated).
pub fn map_version(inst: &Instance) -> Option<String> {
    let s = config::load_settings(inst);
    let v = if s.world_version.is_empty() {
        s.version
    } else {
        s.world_version
    };
    config::validate_version(&v).ok()
}

/// The client jar of a version in the cache (may not exist yet).
pub fn client_jar_path(version: &str) -> PathBuf {
    cfg()
        .data_dir
        .join("cache")
        .join(format!("client-{version}.jar"))
}

/// The Minecraft client jar for the world's version (download on first use, SHA-1 verified). None
/// if textures are off or the download failed (retried after an hour).
pub fn client_jar(log: &dyn Fn(&str), inst: &Instance) -> Option<PathBuf> {
    if !cfg().map_textures {
        return None;
    }
    let version = map_version(inst)?;
    let path = client_jar_path(&version);
    if path.exists() {
        return Some(path);
    }
    if lock(&CLIENT_FAILED)
        .get(&version)
        .is_some_and(|t| t.elapsed() < Duration::from_secs(3600))
    {
        return None;
    }
    let result = (|| -> Result<()> {
        let entry = versions::manifest_entry(&version)?
            .ok_or_else(|| Error::Invalid(format!("Mojang doesn't know version {version}")))?;
        let info = http::get_json(entry["url"].as_str().unwrap_or(""))?;
        let client = &info["downloads"]["client"];
        if !client.is_object() {
            return invalid!("no client download");
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        log(&format!(
            "Live map: downloading the Minecraft {version} block textures from Mojang ({} MB, once) ...",
            client["size"].as_u64().unwrap_or(0) >> 20
        ));
        http::download(
            client["url"].as_str().unwrap_or(""),
            &path,
            None,
            Hash::Sha1(client["sha1"].as_str().unwrap_or("").into()),
            1 << 30,
        )
    })();
    if let Err(e) = result {
        lock(&CLIENT_FAILED).insert(version, Instant::now());
        log(&format!(
            "Live map: block textures unavailable ({e}), using map colours."
        ));
        return None;
    }
    // only the versions some server's map needs are kept
    let needed: Vec<PathBuf> = instances::all_entries()
        .iter()
        .filter_map(|e| map_version(&Instance::new(&e.id)))
        .map(|v| client_jar_path(&v))
        .collect();
    if let Ok(rd) = std::fs::read_dir(path.parent().unwrap()) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("client-")
                && name.ends_with(".jar")
                && p != path
                && !needed.contains(&p)
            {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    Some(path)
}

fn world_args(inst: &Instance) -> Vec<String> {
    let name = backup::level_name(inst);
    let mut args = vec![
        "--world".to_string(),
        inst.server_dir().join(&name).to_string_lossy().into_owned(),
    ];
    // Paper/Spigot before 26.x keep Nether and End in sibling folders
    for suffix in ["_nether", "_the_end"] {
        let extra = inst.server_dir().join(format!("{name}{suffix}"));
        if extra.is_dir() {
            args.push("--extra".into());
            args.push(extra.to_string_lossy().into_owned());
        }
    }
    args
}

/// One mapgen process at a time over all servers (each one already uses MAP_THREADS threads)
static RENDER_SLOT: Mutex<()> = Mutex::new(());

pub struct Renderer {
    inst: Instance,
    busy: AtomicBool,
    pub last: Mutex<Option<Value>>,
}

impl Renderer {
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    /// Run mapgen once. Returns its summary, or None if skipped/failed.
    pub fn render(&self, log: &dyn Fn(&str), force: bool, announce: bool) -> Option<Value> {
        let inst = &self.inst;
        if !inst.server_dir().join(backup::level_name(inst)).is_dir() {
            return None; // no world yet
        }
        let Some(binary) = find_binary() else {
            log(
                "Live map: renderer 'mapgen' not found (build it with: cargo build --release in mapgen/).",
            );
            return None;
        };
        if self.busy.swap(true, Ordering::SeqCst) {
            return None; // a render is already running
        }
        let result = {
            let _slot = lock(&RENDER_SLOT); // waits while another server's map renders
            self.run(&binary, log, force, announce)
        };
        self.busy.store(false, Ordering::SeqCst);
        result
    }

    fn run(&self, binary: &Path, log: &dyn Fn(&str), force: bool, announce: bool) -> Option<Value> {
        let inst = &self.inst;
        let c = cfg();
        let mut cmd = Command::new(binary);
        cmd.arg("render")
            .args(world_args(inst))
            .arg("--out")
            .arg(out_dir(inst))
            .args(["--threads", &c.map_threads.to_string()])
            .args(visibility_args());
        if force {
            cmd.arg("--force");
        }
        if let Some(jar) = client_jar(log, inst) {
            cmd.arg("--client").arg(jar);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        low_priority(&mut cmd);
        let (stdout, stderr) = match run_with_timeout(cmd, Duration::from_secs(3600)) {
            Ok(out) => out,
            Err(e) => {
                log(&format!("Live map render failed: {e}"));
                return None;
            }
        };
        let line = stdout.trim().lines().last().unwrap_or("").to_string();
        let Ok(summary) = serde_json::from_str::<Value>(&line) else {
            let text = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            log(&format!(
                "Live map render failed: {}",
                text.trim().chars().take(300).collect::<String>()
            ));
            return None;
        };
        *lock(&self.last) = Some(summary.clone());
        // skins of player heads placed in the world (heads.json): in the background
        let inst = self.inst.clone();
        std::thread::Builder::new()
            .name("head-skins".into())
            .spawn(move || crate::skins::fetch_head_skins(&inst, crate::skins::HEADS_PER_RUN))
            .ok();
        let dims = summary["dimensions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let rendered: u64 = dims
            .iter()
            .map(|d| d["rendered"].as_u64().unwrap_or(0))
            .sum();
        let errors: Vec<String> = dims
            .iter()
            .flat_map(|d| d["errors"].as_array().cloned().unwrap_or_default())
            .map(|e| {
                e.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| e.to_string())
            })
            .collect();
        if rendered > 0 || force || announce {
            log(&format!(
                "Live map updated: {rendered} changed region(s) in {:.1} s.",
                summary["ms"].as_f64().unwrap_or(0.0) / 1000.0
            ));
        }
        for e in errors.iter().take(3) {
            log(&format!("Live map: {e}"));
        }
        Some(summary)
    }
}

fn low_priority(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_4000 | 0x0800_0000); // below normal priority, no window
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // never compete with the game
        unsafe {
            cmd.pre_exec(|| {
                libc::nice(15);
                Ok(())
            });
        }
    }
}

/// Run a command, collect stdout/stderr, kill it after `timeout`.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<(String, String)> {
    let mut child = cmd.spawn()?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out.as_mut() {
            let mut buf = Vec::new();
            let _ = o.read_to_end(&mut buf);
            s = String::from_utf8_lossy(&buf).into_owned();
        }
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = err.as_mut() {
            let mut buf = Vec::new();
            let _ = e.read_to_end(&mut buf);
            s = String::from_utf8_lossy(&buf).into_owned();
        }
        s
    });
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other("timed out"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok((
        t_out.join().unwrap_or_default(),
        t_err.join().unwrap_or_default(),
    ))
}

static RENDERERS: LazyLock<Mutex<HashMap<String, Arc<Renderer>>>> = LazyLock::new(Default::default);
static TRACKERS: LazyLock<Mutex<HashMap<String, Arc<PlayerTracker>>>> =
    LazyLock::new(Default::default);

pub fn renderer_for(inst: &Instance) -> Arc<Renderer> {
    lock(&RENDERERS)
        .entry(inst.id.clone())
        .or_insert_with(|| {
            Arc::new(Renderer {
                inst: inst.clone(),
                busy: AtomicBool::new(false),
                last: Mutex::new(None),
            })
        })
        .clone()
}

pub fn tracker_for(inst: &Instance) -> Arc<PlayerTracker> {
    lock(&TRACKERS)
        .entry(inst.id.clone())
        .or_insert_with(|| Arc::new(PlayerTracker::new(inst)))
        .clone()
}

// ---------- live player positions ----------

// "Steve has the following entity data: [12.5d, 64.0d, -3.25d]"
static POS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\]: (?:System chat: )?(\w{1,16}) has the following entity data: \[(-?[\d.E]+)d, (-?[\d.E]+)d, (-?[\d.E]+)d\]$").unwrap()
});
// 'Steve has the following entity data: "minecraft:overworld"'
static DIM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\]: (?:System chat: )?(\w{1,16}) has the following entity data: "([\w:./-]+)"$"#)
        .unwrap()
});

const WATCH: Duration = Duration::from_secs(30); // keep polling this long after the last map request
const PENDING: Duration = Duration::from_secs(5); // answers to our queries arrive within this time

pub struct PlayerTracker {
    inst: Instance,
    inner: Mutex<TrackerInner>,
}

#[derive(Default)]
struct TrackerInner {
    positions: HashMap<String, Map<String, Value>>,
    pending: HashMap<String, Instant>,
    last_view: Option<Instant>,
}

impl PlayerTracker {
    fn new(inst: &Instance) -> Self {
        PlayerTracker {
            inst: inst.clone(),
            inner: Mutex::new(TrackerInner::default()),
        }
    }

    pub fn touch(&self) {
        lock(&self.inner).last_view = Some(Instant::now());
    }

    pub fn watching(&self) -> bool {
        lock(&self.inner)
            .last_view
            .is_some_and(|t| t.elapsed() < WATCH)
    }

    /// Commands to send now; remembers who was asked so the answers can be filtered.
    pub fn queries(&self, online: &[String]) -> Vec<String> {
        let now = Instant::now();
        {
            let mut g = lock(&self.inner);
            g.positions.retain(|name, _| online.contains(name));
            for name in online {
                g.pending.insert(name.clone(), now);
            }
        }
        SKINS.want(online.iter(), &self.inst);
        let mut sorted = online.to_vec();
        sorted.sort();
        sorted
            .iter()
            .flat_map(|n| {
                [
                    format!("data get entity {n} Pos"),
                    format!("data get entity {n} Dimension"),
                ]
            })
            .collect()
    }

    /// True if the console line is an answer to our own query (then it isn't shown).
    pub fn consume(&self, line: &str) -> bool {
        if !line.contains("has the following entity data") {
            return false;
        }
        let (pos, caps) = match POS_RE.captures(line) {
            Some(c) => (true, c),
            None => match DIM_RE.captures(line) {
                Some(c) => (false, c),
                None => return false,
            },
        };
        let name = caps[1].to_string();
        let mut g = lock(&self.inner);
        match g.pending.get(&name) {
            Some(asked) if asked.elapsed() <= PENDING => {}
            _ => return false, // somebody typed it in the console: show it
        }
        let entry = g.positions.entry(name.clone()).or_insert_with(|| {
            let mut m = Map::new();
            m.insert("name".into(), json!(name));
            m.insert("dimension".into(), json!("minecraft:overworld"));
            m
        });
        if pos {
            for (i, axis) in ["x", "y", "z"].iter().enumerate() {
                let v: f64 = caps[i + 2].parse().unwrap_or(0.0);
                entry.insert(axis.to_string(), json!(v));
            }
        } else {
            entry.insert("dimension".into(), json!(&caps[2]));
        }
        true
    }

    pub fn snapshot(&self) -> Vec<Value> {
        let out: Vec<Map<String, Value>> = lock(&self.inner)
            .positions
            .values()
            .filter(|p| p.contains_key("x"))
            .cloned()
            .collect();
        out.into_iter()
            .map(|mut p| {
                // the viewer shows the face of the skin (skins/<uuid>.png on the map server)
                let name = p["name"].as_str().unwrap_or("").to_string();
                if let Some(uuid) = SKINS.uuid_of(&name, &self.inst) {
                    if skin_path(&uuid).is_ok_and(|p| p.exists()) {
                        p.insert("skin".into(), json!(uuid));
                    }
                }
                Value::Object(p)
            })
            .collect()
    }

    pub fn clear(&self) {
        let mut g = lock(&self.inner);
        g.positions.clear();
        g.pending.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_is_validated_and_merged() {
        let _t = crate::testutil::data_dir();
        let mut m = Map::new();
        m.insert("cave_reach".into(), json!(64));
        assert_eq!(save_visibility(&m).unwrap()["cave_reach"], json!(64));
        let mut m = Map::new();
        m.insert("all_caves".into(), json!(true));
        let v = save_visibility(&m).unwrap();
        assert_eq!(v, json!({"cave_reach": 64, "all_caves": true}));
        assert_eq!(load_visibility(), v);
        m.insert("cave_reach".into(), json!(300));
        assert!(save_visibility(&m).is_err());
        m.insert("cave_reach".into(), json!(true));
        assert!(save_visibility(&m).is_err());
        assert_eq!(visibility_args(), vec!["--cave-reach", "64", "--all-caves"]);
    }

    #[test]
    fn tracker_swallows_only_its_own_answers() {
        let _t = crate::testutil::data_dir();
        let t = PlayerTracker::new(&Instance::main());
        let line = "[12:00:00] [Server thread/INFO]: Steve has the following entity data: [12.5d, 64.0d, -3.25d]";
        assert!(!t.consume(line), "not asked: shown in the console");
        assert_eq!(
            t.queries(&["Steve".into()]),
            vec![
                "data get entity Steve Pos",
                "data get entity Steve Dimension"
            ]
        );
        assert!(t.consume(line));
        assert!(t.consume("[12:00:00] [Server thread/INFO]: Steve has the following entity data: \"minecraft:the_nether\""));
        let snap = t.snapshot();
        assert_eq!(snap[0]["x"], json!(12.5));
        assert_eq!(snap[0]["dimension"], json!("minecraft:the_nether"));
        t.queries(&[]);
        assert!(t.snapshot().is_empty(), "players who left are dropped");
    }
}
