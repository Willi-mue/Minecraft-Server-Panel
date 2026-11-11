//! Minecraft server process management.
//!
//! One `ServerManager` per server instance, all registered in `MANAGERS`. The Java process runs as a
//! child process with a reader thread parsing its output line by line. Long actions (start, stop,
//! restart, backup, download, ...) run in their own thread, one at a time per server (`exclusive`).
//! Events of all servers are fanned out to the WebSocket clients through one `EventHub`, tagged with
//! the server id.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::config::{self, cfg};
use crate::error::{Error, Result};
use crate::instances::{self, Instance};
use crate::util::{Event, capitalize, local_now, lock, now_secs};
use crate::{
    access, backup, datapacks, invalid, java, leaderboard, livemap, players, properties,
    servericon, versions, world,
};

static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1B\[[0-?]*[ -/]*[@-~]").unwrap());
// Since 26.x, system messages and command feedback are logged as "]: System chat: ..."
const MSG: &str = r"\]: (?:System chat: )?";
fn re(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("{MSG}", MSG)).unwrap()
}
static DONE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\]: Done \("));
static JOIN_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"{MSG}(\w{1,16})(?: \(formerly known as \w{1,16}\))? joined the game"));
static LEFT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"{MSG}(\w{1,16}) left the game"));
// "Name[/1.2.3.4:5678] logged in with entity id ..." (IPv6: "Name[/[::1]:5678] ...")
static LOGIN_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"\]: (\w{1,16})\[/(.+):\d+\] logged in with entity id"));
static CHAT_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"\]: (?:\[Not Secure\] )?<(\w{1,16})> (.*)"));
static LIST_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"{MSG}There are \d+ of a max(?: of)? \d+ players online:(.*)"));
static SAVED_RE: LazyLock<Regex> = LazyLock::new(|| re(r"{MSG}Saved the game"));
static EULA_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)agree to the EULA"));
// "Disconnecting Name (/1.2.3.4:5678): You are not whitelisted on this server!"
static REJECTED_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"\]: Disconnecting (\w{1,16}) \(.*\): You are not white-?listed"));

const COUNTDOWN_MARKS: [u64; 9] = [60, 30, 15, 10, 5, 4, 3, 2, 1];

// Tick rate answers (swallowed, see ServerManager::ask). Vanilla/Fabric 1.20.3+: `tick query`.
// "Average time per tick" is the second line of a two-line message: it comes without log prefix.
static TICK_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?:{MSG}|^)(The game is (?:running normally|frozen|sprinting)|Target tick rate|Average time per tick|Percentiles)",
    )
});
static MSPT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"Average time per tick: ([\d.]+) ?ms"));
static TARGET_RE: LazyLock<Regex> = LazyLock::new(|| re(r"Target tick rate: ([\d.]+)"));
// Paper: `tps` -> "TPS from last 1m, 5m, 15m: 20.0, ...", `mspt` -> header + "◴ 1.2/0.4/5.1, ..."
static PAPER_TICK_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"\]: (?:System chat: )?(TPS from last|Server tick times|◴)"));
static PAPER_TPS_RE: LazyLock<Regex> = LazyLock::new(|| re(r"TPS from last [^:]*: \*?([\d.]+)"));
static PAPER_MSPT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"◴ ([\d.]+)/"));
pub const HISTORY_POINTS: usize = 120; // every 30 s: one hour

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Stopped,
    Starting,
    Running,
    Stopping,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Stopped => "stopped",
            State::Starting => "starting",
            State::Running => "running",
            State::Stopping => "stopping",
        }
    }
}

// ---------- event hub ----------

/// WebSocket fan-out for all servers. Events are pushed from any thread; slow clients lose events
/// instead of blocking everyone.
pub struct EventHub {
    tx: broadcast::Sender<Arc<str>>,
}

impl EventHub {
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<str>> {
        self.tx.subscribe()
    }

    pub fn has_subscribers(&self) -> bool {
        self.tx.receiver_count() > 0
    }

    pub fn emit(&self, event: &Value) {
        if self.has_subscribers() {
            let _ = self.tx.send(Arc::from(event.to_string()));
        }
    }
}

pub static HUB: LazyLock<EventHub> = LazyLock::new(|| EventHub {
    tx: broadcast::channel(2000).0,
});

// ---------- queries ----------

/// Console lines a caller is waiting for (they are not shown in the console).
struct Query {
    pattern: Regex,
    expected: usize,
    lines: Mutex<Vec<String>>,
    done: Event,
}

#[derive(Clone, Copy, Debug)]
pub struct Tick {
    pub tps: Option<f64>,
    pub mspt: Option<f64>,
    pub t: f64,
}

impl Tick {
    pub fn json(&self) -> Value {
        json!({"tps": self.tps, "mspt": self.mspt, "t": self.t})
    }
}

struct St {
    state: State,
    players: BTreeSet<String>,
    rejected: Vec<String>, // recently refused by the whitelist (newest first)
    started_at: Option<f64>,
    running_version: Option<String>,
    running_flavor: Option<String>,
    running_max_ram_mb: Option<u64>, // -Xmx of the running server
    task: Option<String>,            // long-running action in progress
    eula_missing: bool,
    log: VecDeque<String>,
    chat: VecDeque<Value>,
    history: VecDeque<Value>, // {t, cpu, ram, players, tps, mspt}
    tick: Option<Tick>,       // last tick rate while somebody watches the panel
    tick_supported: bool,
    generation: u64,
}

pub struct ServerManager {
    pub inst: Instance,
    st: Mutex<St>,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    action: AtomicBool,
    pub map_enabled: AtomicBool, // kept in sync by the settings API
    queries: Mutex<Vec<Arc<Query>>>,
    saved: Event,
    list_event: Event, // set when a `list` answer was parsed
    cancel: Event,
}

/// Snapshot for the stats loops.
pub struct Usage {
    pub pid: Option<u32>,
    pub max_ram_mb: Option<u64>,
    pub players: usize,
    pub tick: Option<Tick>,
    pub uptime: u64,
}

fn push_limited<T>(q: &mut VecDeque<T>, item: T, max: usize) {
    q.push_back(item);
    while q.len() > max {
        q.pop_front();
    }
}

impl ServerManager {
    fn new(inst: Instance) -> Arc<ServerManager> {
        let map_enabled = config::load_settings(&inst).map_enabled;
        Arc::new(ServerManager {
            inst,
            st: Mutex::new(St {
                state: State::Stopped,
                players: BTreeSet::new(),
                rejected: vec![],
                started_at: None,
                running_version: None,
                running_flavor: None,
                running_max_ram_mb: None,
                task: None,
                eula_missing: false,
                log: VecDeque::new(),
                chat: VecDeque::new(),
                history: VecDeque::new(),
                tick: None,
                tick_supported: true,
                generation: 0,
            }),
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            action: AtomicBool::new(false),
            map_enabled: AtomicBool::new(map_enabled),
            queries: Mutex::new(vec![]),
            saved: Event::new(),
            list_event: Event::new(),
            cancel: Event::new(),
        })
    }

    /// Where this server's map is on the map port.
    pub fn map_path(&self) -> String {
        format!("/s/{}/", self.inst.id)
    }

    // ---------- events ----------

    fn emit(&self, mut event: Value) {
        event["server"] = json!(self.inst.id);
        HUB.emit(&event);
    }

    /// Message from the panel itself (not from the Minecraft process).
    pub fn info(&self, text: &str) {
        let line = format!("[Panel] {text}");
        push_limited(&mut lock(&self.st).log, line.clone(), 1000);
        self.emit(json!({"type": "log", "line": line}));
    }

    pub fn state(&self) -> State {
        lock(&self.st).state
    }

    pub fn task(&self) -> Option<String> {
        lock(&self.st).task.clone()
    }

    pub fn players(&self) -> Vec<String> {
        lock(&self.st).players.iter().cloned().collect()
    }

    pub fn running(&self) -> (Option<String>, Option<String>) {
        let g = lock(&self.st);
        (g.running_flavor.clone(), g.running_version.clone())
    }

    pub fn log_lines(&self) -> Vec<String> {
        lock(&self.st).log.iter().cloned().collect()
    }

    pub fn chat(&self) -> Vec<Value> {
        lock(&self.st).chat.iter().cloned().collect()
    }

    pub fn history(&self) -> Vec<Value> {
        lock(&self.st).history.iter().cloned().collect()
    }

    pub fn push_history(&self, point: Value) {
        push_limited(&mut lock(&self.st).history, point, HISTORY_POINTS);
    }

    pub fn clear_history(&self) {
        lock(&self.st).history.clear();
    }

    pub fn pid(&self) -> Option<u32> {
        let mut c = lock(&self.child);
        let child = c.as_mut()?;
        matches!(child.try_wait(), Ok(None)).then(|| child.id())
    }

    pub fn is_alive(&self) -> bool {
        self.pid().is_some()
    }

    pub fn usage(&self) -> Usage {
        let pid = self.pid();
        let g = lock(&self.st);
        Usage {
            pid,
            max_ram_mb: g.running_max_ram_mb,
            players: g.players.len(),
            tick: g.tick,
            uptime: g
                .started_at
                .map(|s| (now_secs() - s).max(0.0) as u64)
                .unwrap_or(0),
        }
    }

    pub fn status(&self) -> Value {
        let pid = self.pid();
        let map_on = self.map_enabled.load(Ordering::SeqCst) && livemap::available(&self.inst);
        let g = lock(&self.st);
        json!({
            "server": self.inst.id,
            "state": g.state.as_str(),
            "task": g.task,
            "players": g.players.iter().collect::<Vec<_>>(),
            "rejected": g.rejected,
            // The panel serves the map files itself, so it is reachable even while stopped
            "map_port": if map_on { Some(cfg().map_public_port) } else { None },
            "map_path": self.map_path(),
            "version": g.running_version,
            "flavor": g.running_flavor,
            "uptime": g.started_at.map(|s| (now_secs() - s).max(0.0) as u64).unwrap_or(0),
            "pid": pid,
            "eula_missing": g.eula_missing,
        })
    }

    pub fn emit_status(&self) {
        let status = self.status();
        self.emit(json!({"type": "status", "status": status}));
    }

    fn set_state(&self, state: State) {
        lock(&self.st).state = state;
        self.emit_status();
    }

    fn set_task(&self, task: Option<&str>) {
        lock(&self.st).task = task.map(str::to_string);
        self.emit_status();
    }

    // ---------- actions ----------

    fn exclusive<F>(self: &Arc<Self>, name: &str, f: F) -> Result<()>
    where
        F: FnOnce(&Arc<ServerManager>) -> Result<()> + Send + 'static,
    {
        if self.action.swap(true, Ordering::SeqCst) {
            return Err(Error::Busy(format!(
                "Another action is running: {}",
                self.task().unwrap_or_default()
            )));
        }
        let me = self.clone();
        let name = name.to_string();
        std::thread::spawn(move || {
            me.set_task(Some(&name));
            if let Err(e) = f(&me) {
                me.info(&format!("{name} failed: {e}"));
            }
            me.action.store(false, Ordering::SeqCst);
            me.set_task(None);
        });
        Ok(())
    }

    pub fn start(self: &Arc<Self>) -> Result<()> {
        self.exclusive("start", |m| m.do_start())
    }

    pub fn stop(self: &Arc<Self>, countdown: u64) -> Result<()> {
        self.exclusive("stop", move |m| {
            m.do_stop(countdown, "stopping").map(|_| ())
        })
    }

    pub fn restart(self: &Arc<Self>, countdown: u64) -> Result<()> {
        self.exclusive("restart", move |m| {
            if m.do_stop(countdown, "restarting")? {
                m.do_start()?;
            }
            Ok(())
        })
    }

    pub fn make_backup(self: &Arc<Self>) -> Result<()> {
        self.exclusive("backup", |m| m.do_backup())
    }

    /// 'Render now': flush the world to disk first if the server runs, then render.
    pub fn render_map(self: &Arc<Self>) -> Result<()> {
        let renderer = livemap::renderer_for(&self.inst);
        if renderer.busy() {
            return Err(Error::Busy("The map is already being rendered".into()));
        }
        let me = self.clone();
        std::thread::spawn(move || {
            if me.state() == State::Running {
                me.saved.clear();
                // flush: wait until the chunks are written (Paper saves asynchronously otherwise)
                if me.send_command("save-all flush").is_ok()
                    && !me.saved.wait(Duration::from_secs(60))
                {
                    me.info("Live map: no save confirmation, rendering what is on disk.");
                }
            }
            renderer.render(&|t| me.info(t), false, true);
            me.emit_status();
        });
        Ok(())
    }

    /// Blocking `save-all` (writes level.dat too); true once the server confirmed it.
    pub fn save_and_wait(&self, timeout: Duration) -> bool {
        self.saved.clear();
        if self.send_command("save-all").is_err() {
            return false;
        }
        self.saved.wait(timeout)
    }

    pub fn restore(self: &Arc<Self>, name: &str) -> Result<()> {
        if self.is_alive() {
            return invalid!("Stop the server before restoring a backup");
        }
        backup::backup_file(&self.inst, name)?; // 404 early instead of failing in the background
        let name = name.to_string();
        self.exclusive("restore", move |m| {
            m.info(&format!("Restoring {name} ..."));
            if let Some(safety) = backup::restore(&m.inst, &name)? {
                m.info(&format!(
                    "Current world saved first as {}.",
                    file_name(&safety)
                ));
            }
            // The restored world may come from another version: let level.dat tell
            let mut s = config::load_settings(&m.inst);
            s.world_version.clear();
            s.world_flavor.clear();
            config::save_settings(&s, &m.inst)?;
            m.info(&format!("Backup {name} restored."));
            Ok(())
        })
    }

    pub fn new_world(self: &Arc<Self>, seed: &str, packs: &[String]) -> Result<()> {
        if self.is_alive() {
            return invalid!("Stop the server before creating a new world");
        }
        let seed = seed.trim().to_string();
        if seed.chars().count() > 64 || seed.contains(['\n', '\r']) {
            return invalid!("Seed: up to 64 characters, single line");
        }
        let packs: Vec<String> = packs
            .iter()
            .map(|n| datapacks::validate_name(n))
            .collect::<Result<_>>()?;
        self.exclusive("new world", move |m| m.do_new_world(&seed, &packs))
    }

    fn do_new_world(&self, seed: &str, packs: &[String]) -> Result<()> {
        let inst = &self.inst;
        // Save world-only packs to the library first: the old world is removed below
        let packs = datapacks::prepare_new_world(inst, packs)?;
        if !backup::world_dirs(inst).is_empty() {
            let s = config::load_settings(inst);
            let label = if s.world_version.is_empty() {
                world::saved_version(inst).unwrap_or_default()
            } else {
                s.world_version
            };
            self.info("Archiving the current world ...");
            let archive = backup::create_archive(inst, &label)?;
            self.info(&format!(
                "Old world archived as {} (never deleted automatically).",
                file_name(&archive)
            ));
            backup::remove_world(inst)?;
        }
        access::set_property(inst, "level-seed", seed)?;
        // No experiments in new worlds (also clears a value an older panel version may have left)
        access::set_property(inst, "initial-enabled-packs", "vanilla")?;
        // In place before the first start, so world generation already uses them
        datapacks::install_many(inst, &packs)?;
        livemap::clear(inst);
        let mut s = config::load_settings(inst);
        s.world_version.clear();
        s.world_flavor.clear();
        config::save_settings(&s, inst)?;
        self.info(&format!(
            "New world prepared{}{}. It is generated on the next start.",
            if seed.is_empty() {
                " (random seed)".to_string()
            } else {
                format!(" with seed {seed}")
            },
            if packs.is_empty() {
                String::new()
            } else {
                format!(" and {} data pack(s)", packs.len())
            }
        ));
        Ok(())
    }

    pub fn install(self: &Arc<Self>, flavor: &str, version: &str) -> Result<()> {
        let (flavor, version) = (flavor.to_string(), version.to_string());
        self.exclusive("download", move |m| {
            versions::download(&flavor, &version, &|t| m.info(t)).map(|_| ())
        })
    }

    /// Newest Paper build / Fabric loader.
    pub fn update_jar(self: &Arc<Self>, flavor: &str, version: &str) -> Result<()> {
        let (flavor, version) = (flavor.to_string(), version.to_string());
        self.exclusive("download", move |m| {
            versions::update(&flavor, &version, &|t| m.info(t)).map(|_| ())
        })
    }

    /// Blocking backup of a stopped server's world (e.g. before switching the server type).
    pub fn backup_before(&self, suffix: &str) -> Result<()> {
        if self.is_alive() {
            return invalid!("Stop the server first");
        }
        if self.action.swap(true, Ordering::SeqCst) {
            return Err(Error::Busy(format!(
                "Another action is running: {}",
                self.task().unwrap_or_default()
            )));
        }
        let result = (|| -> Result<()> {
            if !backup::world_dirs(&self.inst).is_empty() {
                self.info("Creating a backup first ...");
                let path = backup::create_backup(&self.inst, suffix, None)?;
                self.info(&format!("Backup saved: {}", file_name(&path)));
            }
            Ok(())
        })();
        self.action.store(false, Ordering::SeqCst);
        result
    }

    pub fn kill(&self) {
        self.cancel.set();
        let mut c = lock(&self.child);
        if let Some(child) = c.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                drop(c);
                self.info("Server process killed.");
            }
        }
    }

    pub fn cancel_countdown(&self) {
        self.cancel.set();
    }

    pub fn send_command(&self, cmd: &str) -> Result<()> {
        if !self.is_alive() {
            return invalid!("Server is not running");
        }
        let mut stdin = lock(&self.stdin);
        let Some(pipe) = stdin.as_mut() else {
            return invalid!("Server is not running");
        };
        pipe.write_all(format!("{cmd}\n").as_bytes())
            .and_then(|_| pipe.flush())
            .map_err(|e| Error::Invalid(format!("Could not send command: {e}")))
    }

    // ---------- implementation ----------

    /// Two running servers can't share a game port.
    fn check_port(&self) -> Result<()> {
        let port = instances::game_port(&self.inst);
        for other in all() {
            if other.inst != self.inst
                && other.is_alive()
                && instances::game_port(&other.inst) == port
            {
                return invalid!(
                    "Port {port} is already used by the running server '{}'",
                    instances::name_of(&other.inst)
                );
            }
        }
        Ok(())
    }

    fn do_start(self: &Arc<Self>) -> Result<()> {
        if self.is_alive() {
            self.info("Server is already running.");
            return Ok(());
        }
        let inst = &self.inst;
        let settings = config::load_settings(inst);
        if settings.version.is_empty() {
            return invalid!("No Minecraft version selected: choose one in the Server card");
        }
        let (flavor, version) = (settings.flavor.clone(), settings.version.clone());
        self.check_port()?;

        let jar = versions::download(&flavor, &version, &|t| self.info(t))?;

        if !self.eula_accepted() {
            lock(&self.st).eula_missing = true;
            self.emit_status();
            return invalid!("Minecraft EULA not accepted yet");
        }
        if let Err(e) = properties::apply_presets(inst) {
            self.info(&format!(
                "Could not write default view/simulation distance: {e}"
            ));
        }
        if let Err(e) = servericon::ensure(inst) {
            self.info(&format!("Could not write the server icon: {e}")); // the panel logo until the user sets one
        }
        match livemap::remove_squaremap(inst) {
            Ok(true) => self.info(
                "Removed the old squaremap plugin: the live map is now built into the panel.",
            ),
            Ok(false) => {}
            Err(e) => self.info(&format!("Could not remove the old squaremap plugin: {e}")), // never blocks the start
        }

        let required = versions::required_java(&flavor, &version);
        let Some((java_bin, java_major)) = java::pick(required) else {
            return invalid!("No Java runtime found (install Java or use the Docker image)");
        };
        if java_major < required {
            self.info(&format!(
                "Warning: {version} needs Java {required}, only Java {java_major} available."
            ));
        }

        let mut cmd = Command::new(&java_bin);
        cmd.arg(format!("-Xms{}", settings.min_ram))
            .arg(format!("-Xmx{}", settings.max_ram))
            // Paper ships recommended GC flags (Aikar's flags); empty for Vanilla
            .args(versions::jvm_flags(&flavor, &version))
            .arg("-jar")
            .arg(&jar)
            .arg("nogui")
            .current_dir(inst.server_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // no console window
        }
        self.info(&format!(
            "Starting {} {version} with Java {java_major}",
            capitalize(&flavor)
        ));
        inst.ensure_dirs()?;
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let generation = {
            let mut g = lock(&self.st);
            g.players.clear();
            g.running_version = Some(version);
            g.running_flavor = Some(flavor);
            g.running_max_ram_mb = Some(config::ram_mb(&settings.max_ram));
            g.started_at = Some(now_secs());
            g.generation += 1;
            g.generation
        };
        *lock(&self.stdin) = child.stdin.take();
        *lock(&self.child) = Some(child);
        self.set_state(State::Starting);
        // stderr goes into the same console (like stderr=STDOUT)
        let me = self.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).split(b'\n').map_while(|l| l.ok()) {
                me.feed(&line);
            }
        });
        let me = self.clone();
        std::thread::spawn(move || me.reader(stdout, generation));
        Ok(())
    }

    fn feed(&self, raw: &[u8]) {
        let text = String::from_utf8_lossy(raw);
        let line = ANSI_RE.replace_all(text.trim_end(), "");
        if !line.is_empty() {
            self.handle_line(&line);
        }
    }

    /// Announce in chat; returns false if cancelled.
    fn countdown(&self, seconds: u64, word: &str) -> Result<bool> {
        self.cancel.clear();
        if seconds == 0 {
            return Ok(true);
        }
        if self.players().is_empty() && self.state() == State::Running {
            // Don't trust a possibly stale list: ask the server before skipping the warning
            self.list_event.clear();
            self.send_command("list")?;
            self.list_event.wait(Duration::from_secs(3));
        }
        if self.players().is_empty() {
            // The countdown only warns players in chat; nobody to warn
            self.info(&format!("Nobody online, {word} without countdown."));
            return Ok(true);
        }
        let mut remaining = seconds;
        let marks: Vec<u64> = COUNTDOWN_MARKS
            .iter()
            .copied()
            .filter(|m| *m <= seconds)
            .chain([0])
            .collect();
        for mark in marks {
            if self.cancel.wait(Duration::from_secs(remaining - mark)) {
                self.send_command(&format!("say Server {word} cancelled"))?;
                self.info("Countdown cancelled.");
                return Ok(false);
            }
            remaining = mark;
            if mark > 0 {
                self.send_command(&format!(
                    "say Server {word} in {mark} second{}",
                    if mark != 1 { "s" } else { "" }
                ))?;
            }
        }
        Ok(true)
    }

    fn do_stop(&self, countdown: u64, word: &str) -> Result<bool> {
        if !self.is_alive() {
            self.info("Server is not running.");
            return Ok(true);
        }
        if !self.countdown(countdown, word)? {
            return Ok(false);
        }
        if !self.is_alive() {
            return Ok(true); // crashed/killed during the countdown
        }
        self.set_state(State::Stopping);
        self.send_command("stop")?;
        if !self.wait_exit(Duration::from_secs(120)) {
            self.info("Server did not stop within 120 s, killing it.");
            if let Some(c) = lock(&self.child).as_mut() {
                let _ = c.kill();
            }
            self.wait_exit(Duration::from_secs(30));
        }
        Ok(true)
    }

    /// Wait until the process has exited (true) or the timeout passed (false).
    fn wait_exit(&self, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if !self.is_alive() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        !self.is_alive()
    }

    fn do_backup(&self) -> Result<()> {
        let state = self.state();
        if self.is_alive() && state != State::Running {
            // Starting/stopping: the world is being written, a copy would be inconsistent
            return invalid!("Server is {}, try again in a moment", state.as_str());
        }
        if !self.is_alive() {
            self.info("Creating backup ...");
            let path = backup::create_backup(&self.inst, "", None)?;
            self.info(&format!("Backup saved: {}", file_name(&path)));
            return Ok(());
        }
        // Live backup: flush the world to disk and pause autosave while zipping.
        self.info("Creating live backup (autosave paused) ...");
        self.send_command("say Backup started, the server may lag briefly")?;
        self.saved.clear();
        self.send_command("save-off")?;
        self.send_command("save-all flush")?;
        if !self.saved.wait(Duration::from_secs(120)) {
            self.info("Warning: no 'Saved the game' confirmation, backing up anyway.");
        }
        let result = backup::create_backup(&self.inst, "", None);
        if self.is_alive() {
            let _ = self.send_command("save-on");
        }
        let path = result?;
        let _ = self.send_command("say Backup finished");
        self.info(&format!("Backup saved: {}", file_name(&path)));
        Ok(())
    }

    // ---------- output parsing ----------

    fn reader(self: Arc<Self>, stdout: std::process::ChildStdout, generation: u64) {
        for line in BufReader::new(stdout).split(b'\n').map_while(|l| l.ok()) {
            self.feed(&line);
        }
        // stdout closed: the process is exiting
        let code = loop {
            let mut c = lock(&self.child);
            match c.as_mut().map(|ch| ch.try_wait()) {
                Some(Ok(Some(status))) => {
                    break status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "killed".into());
                }
                Some(Ok(None)) => {}
                _ => break "?".into(),
            }
            drop(c);
            std::thread::sleep(Duration::from_millis(100));
        };
        if lock(&self.st).generation != generation {
            return;
        }
        {
            let mut g = lock(&self.st);
            g.players.clear();
            g.tick = None;
            g.tick_supported = true;
            g.started_at = None;
            g.running_version = None;
            g.running_flavor = None;
        }
        *lock(&self.stdin) = None;
        players::all_offline(&self.inst);
        livemap::tracker_for(&self.inst).clear();
        self.info(&format!("Server process exited (code {code})."));
        self.set_state(State::Stopped);
        if self.map_enabled.load(Ordering::SeqCst) {
            // Everything is saved now: bring the map up to date
            let me = self.clone();
            std::thread::spawn(move || {
                livemap::renderer_for(&me.inst).render(&|t| me.info(t), false, false);
            });
        }
    }

    /// Send commands and collect the console lines matching `pattern` (up to `expected` of them,
    /// within `timeout`). Matching lines are swallowed, so the console stays clean.
    pub fn ask(
        &self,
        commands: &[String],
        pattern: &Regex,
        expected: usize,
        timeout: Duration,
    ) -> Vec<String> {
        let q = Arc::new(Query {
            pattern: pattern.clone(),
            expected,
            lines: Mutex::new(vec![]),
            done: Event::new(),
        });
        lock(&self.queries).push(q.clone());
        let sent = commands.iter().try_for_each(|c| self.send_command(c));
        if sent.is_ok() {
            q.done.wait(timeout);
        }
        lock(&self.queries).retain(|x| !Arc::ptr_eq(x, &q));
        let lines = lock(&q.lines).clone();
        lines
    }

    fn consume_query(&self, line: &str) -> bool {
        let queries = lock(&self.queries);
        for q in queries.iter() {
            let mut lines = lock(&q.lines);
            if lines.len() < q.expected && q.pattern.is_match(line) {
                lines.push(line.to_string());
                if lines.len() >= q.expected {
                    q.done.set();
                }
                return true;
            }
        }
        false
    }

    /// TPS / milliseconds per tick (Paper: tps + mspt, else `tick query` on 1.20.3+).
    pub fn query_tick(&self) -> Option<Tick> {
        let (flavor, version, supported) = {
            let g = lock(&self.st);
            (
                g.running_flavor.clone(),
                g.running_version.clone(),
                g.tick_supported,
            )
        };
        if self.state() != State::Running || !supported {
            return None;
        }
        let (mut tps, mut mspt) = (None, None);
        if flavor.as_deref() == Some("paper") {
            for line in self.ask(
                &["tps".into(), "mspt".into()],
                &PAPER_TICK_RE,
                3,
                Duration::from_secs(2),
            ) {
                if let Some(c) = PAPER_TPS_RE.captures(&line) {
                    tps = c[1].parse().ok();
                } else if let Some(c) = PAPER_MSPT_RE.captures(&line) {
                    mspt = c[1].parse().ok();
                }
            }
        } else {
            if !tick_command_supported(version.as_deref().unwrap_or("")) {
                lock(&self.st).tick_supported = false;
                return None;
            }
            let mut target = 20.0;
            for line in self.ask(
                &["tick query".into()],
                &TICK_LINE_RE,
                4,
                Duration::from_secs(2),
            ) {
                if let Some(c) = MSPT_RE.captures(&line) {
                    mspt = c[1].parse().ok();
                } else if let Some(c) = TARGET_RE.captures(&line) {
                    target = c[1].parse().unwrap_or(20.0);
                }
            }
            if let Some(ms) = mspt {
                tps = Some(crate::util::round_to(
                    if ms > 0.0 {
                        f64::min(target, 1000.0 / ms)
                    } else {
                        target
                    },
                    1,
                ));
            }
        }
        if tps.is_none() && mspt.is_none() {
            return None;
        }
        let tick = Tick {
            tps,
            mspt,
            t: now_secs(),
        };
        lock(&self.st).tick = Some(tick);
        Some(tick)
    }

    fn handle_line(self: &Self, line: &str) {
        if !lock(&self.queries).is_empty() && self.consume_query(line) {
            return; // answer to the panel's own query
        }
        if livemap::tracker_for(&self.inst).consume(line) {
            return; // answer to the map's own position query, not interesting in the console
        }
        push_limited(&mut lock(&self.st).log, line.to_string(), 1000);
        self.emit(json!({"type": "log", "line": line}));

        if self.state() == State::Starting && DONE_RE.is_match(line) {
            self.remember_world_version();
            self.set_state(State::Running);
            let _ = self.send_command("list");
            if lock(&self.st).running_flavor.as_deref() == Some("paper") {
                // Paper has fetched the Mojang jar by now; offer it as a Vanilla server too
                if let Some(me) = get(&self.inst.id) {
                    std::thread::spawn(move || {
                        versions::import_paper_cache(&me.inst.server_dir(), &|t| me.info(t));
                    });
                }
            }
        } else if let Some(c) = LOGIN_RE.captures(line) {
            leaderboard::remember_login(&self.inst, &c[1], &c[2]); // memory only: "Me" on the map
        } else if let Some(c) = JOIN_RE.captures(line) {
            lock(&self.st).players.insert(c[1].to_string());
            players::record(&self.inst, &c[1], true, None);
            self.chat_entry(&format!("→ {} joined", &c[1]));
        } else if let Some(c) = LEFT_RE.captures(line) {
            lock(&self.st).players.remove(&c[1]);
            players::record(&self.inst, &c[1], false, None);
            self.chat_entry(&format!("← {} left", &c[1]));
        } else if let Some(c) = CHAT_RE.captures(line) {
            self.chat_entry(&format!("<{}> {}", &c[1], &c[2]));
        } else if let Some(c) = LIST_RE.captures(line) {
            let names: BTreeSet<String> = c[1]
                .split(',')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect();
            lock(&self.st).players = names;
            self.list_event.set();
            self.emit_status();
        } else if let Some(c) = REJECTED_RE.captures(line) {
            let name = c[1].to_string();
            {
                let mut g = lock(&self.st);
                let mut list: Vec<String> = vec![name.clone()];
                list.extend(g.rejected.iter().filter(|n| **n != name).take(4).cloned());
                g.rejected = list;
            }
            self.emit_status();
        } else if SAVED_RE.is_match(line) {
            self.saved.set();
        } else if EULA_RE.is_match(line) {
            lock(&self.st).eula_missing = true;
        }
    }

    fn chat_entry(&self, text: &str) {
        let entry = json!({"time": local_now("%H:%M:%S"), "text": text});
        push_limited(&mut lock(&self.st).chat, entry.clone(), 300);
        self.emit(json!({"type": "chat", "entry": entry}));
        self.emit_status();
    }

    /// The world has now been loaded (and converted) by this version.
    fn remember_world_version(&self) {
        let (flavor, version) = self.running();
        let mut s = config::load_settings(&self.inst);
        s.world_version = version.unwrap_or_default();
        s.world_flavor = flavor.unwrap_or_default();
        let _ = config::save_settings(&s, &self.inst);
    }

    fn eula_accepted(&self) -> bool {
        let Ok(raw) = std::fs::read(self.inst.server_dir().join("eula.txt")) else {
            return false;
        };
        for line in String::from_utf8_lossy(&raw).lines() {
            if let Some((key, value)) = line.trim().split_once('=') {
                if key.trim().to_lowercase() == "eula" {
                    return value.trim().to_lowercase() == "true";
                }
            }
        }
        false
    }

    pub fn accept_eula(&self) -> Result<()> {
        self.inst.ensure_dirs()?;
        std::fs::write(
            self.inst.server_dir().join("eula.txt"),
            "# Accepted via Minecraft Server Panel (https://aka.ms/MinecraftEULA)\neula=true\n",
        )?;
        lock(&self.st).eula_missing = false;
        self.emit_status();
        Ok(())
    }

    /// Called when the panel exits (e.g. `docker stop`).
    pub fn shutdown(&self) {
        self.cancel.set();
        if self.is_alive() {
            let sent = self.send_command("stop").is_ok();
            if !sent || !self.wait_exit(Duration::from_secs(60)) {
                if let Some(c) = lock(&self.child).as_mut() {
                    let _ = c.kill();
                }
            }
        }
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// `/tick query` exists since 1.20.3 (and in every 26.x release); snapshots: assume yes.
pub fn tick_command_supported(version: &str) -> bool {
    static V: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d+)\.(\d+)(?:\.(\d+))?$").unwrap());
    let Some(c) = V.captures(version) else {
        return true;
    };
    let n = |i: usize| {
        c.get(i)
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .unwrap_or(0)
    };
    n(1) > 1 || (n(2), n(3)) >= (20, 3)
}

// ---------- all servers ----------

static MANAGERS: LazyLock<Mutex<BTreeMap<String, Arc<ServerManager>>>> =
    LazyLock::new(Default::default);

/// One manager per registered instance (called at startup).
pub fn load_managers() {
    let mut m = lock(&MANAGERS);
    for entry in instances::all_entries() {
        m.entry(entry.id.clone())
            .or_insert_with(|| ServerManager::new(Instance::new(&entry.id)));
    }
}

/// The manager of a server (None if the server doesn't exist).
pub fn get(server_id: &str) -> Option<Arc<ServerManager>> {
    let mut m = lock(&MANAGERS);
    if let Some(found) = m.get(server_id) {
        return Some(found.clone());
    }
    // Registered meanwhile (or before load_managers ran)
    let inst = instances::get(server_id).ok()?;
    let manager = ServerManager::new(inst);
    m.insert(server_id.to_string(), manager.clone());
    Some(manager)
}

pub fn all() -> Vec<Arc<ServerManager>> {
    lock(&MANAGERS).values().cloned().collect()
}

pub fn add(inst: Instance) -> Arc<ServerManager> {
    let m = ServerManager::new(inst.clone());
    lock(&MANAGERS).insert(inst.id, m.clone());
    m
}

pub fn remove(server_id: &str) {
    lock(&MANAGERS).remove(server_id);
}

/// Stop every running server in parallel (Docker's stop_grace_period is shared).
pub fn shutdown_all() {
    let threads: Vec<_> = all()
        .into_iter()
        .filter(|m| m.is_alive())
        .map(|m| std::thread::spawn(move || m.shutdown()))
        .collect();
    for t in threads {
        let _ = t.join();
    }
}

/// The hall of fame asks for a `save-all` while players are online, never during another action.
pub fn leaderboard_save(inst: &Instance) -> bool {
    let Some(m) = lock(&MANAGERS).get(&inst.id).cloned() else {
        return false;
    };
    if !m.is_alive() || m.task().is_some() || m.players().is_empty() {
        return false;
    }
    m.send_command("save-all").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> Arc<ServerManager> {
        ServerManager::new(Instance::main())
    }

    #[test]
    fn parses_joins_chat_lists_and_rejections() {
        let _t = crate::testutil::data_dir();
        let m = manager();
        m.handle_line("[12:00:00] [Server thread/INFO]: Steve joined the game");
        m.handle_line("[12:00:01] [Server thread/INFO]: System chat: Alex (formerly known as Al) joined the game");
        assert_eq!(m.players(), vec!["Alex", "Steve"]);
        m.handle_line("[12:00:02] [Server thread/INFO]: [Not Secure] <Steve> hello there");
        assert_eq!(
            m.chat().last().unwrap()["text"],
            json!("<Steve> hello there")
        );
        m.handle_line("[12:00:03] [Server thread/INFO]: Steve left the game");
        assert_eq!(m.players(), vec!["Alex"]);
        m.handle_line(
            "[12:00:04] [Server thread/INFO]: There are 2 of a max of 20 players online: Alex, Bob",
        );
        assert_eq!(m.players(), vec!["Alex", "Bob"]);
        m.handle_line(
            "[12:00:05] [Server thread/INFO]: There are 0 of a max of 20 players online:",
        );
        assert!(m.players().is_empty());
        m.handle_line("[12:00:06] [Server thread/INFO]: Disconnecting Eve (/1.2.3.4:5678): You are not whitelisted on this server!");
        assert_eq!(m.status()["rejected"], json!(["Eve"]));
        m.handle_line("[12:00:07] [Server thread/INFO]: Saved the game");
        assert!(m.saved.is_set());
        m.handle_line("[12:00:08] [ServerMain/WARN]: You need to agree to the EULA in order to run the server.");
        assert_eq!(m.status()["eula_missing"], json!(true));
        let seen = players::seen(&m.inst, "steve");
        assert_eq!(seen["online"], json!(false));
    }

    #[test]
    fn done_switches_to_running_and_remembers_the_world_version() {
        let _t = crate::testutil::data_dir();
        let m = manager();
        {
            let mut g = lock(&m.st);
            g.state = State::Starting;
            g.running_version = Some("1.21.4".into());
            g.running_flavor = Some("vanilla".into());
        }
        m.handle_line(r#"[12:00:00] [Server thread/INFO]: Done (3.2s)! For help, type "help""#);
        assert_eq!(m.state(), State::Running);
        let s = config::load_settings(&m.inst);
        assert_eq!(
            (s.world_version.as_str(), s.world_flavor.as_str()),
            ("1.21.4", "vanilla")
        );
    }

    #[test]
    fn logins_are_remembered_for_the_map() {
        let _t = crate::testutil::data_dir();
        let m = manager();
        m.handle_line("[12:00:00] [Server thread/INFO]: Steve[/192.168.1.20:51234] logged in with entity id 7 at (1, 2, 3)");
        assert_eq!(
            leaderboard::me(&m.inst, Some("192.168.1.20")).as_deref(),
            Some("Steve")
        );
    }

    #[test]
    fn queries_swallow_their_answers() {
        let _t = crate::testutil::data_dir();
        let m = manager();
        let q = Arc::new(Query {
            pattern: TICK_LINE_RE.clone(),
            expected: 2,
            lines: Mutex::new(vec![]),
            done: Event::new(),
        });
        lock(&m.queries).push(q.clone());
        m.handle_line("[12:00:00] [Server thread/INFO]: The game is running normally");
        m.handle_line("[12:00:00] [Server thread/INFO]: Average time per tick: 3.2ms");
        m.handle_line("[12:00:00] [Server thread/INFO]: Percentiles: whatever");
        assert!(q.done.is_set());
        assert_eq!(lock(&q.lines).len(), 2);
        assert_eq!(
            m.log_lines().len(),
            1,
            "only the third line reaches the console"
        );
        // the continuation line of the two-line answer has no log prefix (seen on 1.21.4)
        assert!(TICK_LINE_RE.is_match("Average time per tick: 0.2ms (Target: 50.0ms)"));
        assert!(!TICK_LINE_RE.is_match("<Steve> Average time per tick is great"));
        assert_eq!(
            MSPT_RE.captures("Average time per tick: 3.2ms").unwrap()[1].to_string(),
            "3.2"
        );
        assert_eq!(
            PAPER_TPS_RE
                .captures("]: TPS from last 1m, 5m, 15m: *19.5, 20.0, 20.0")
                .unwrap()[1]
                .to_string(),
            "19.5"
        );
        assert_eq!(
            PAPER_MSPT_RE
                .captures("]: ◴ 1.2/0.4/5.1, 1.0/0.3/4.0")
                .unwrap()[1]
                .to_string(),
            "1.2"
        );
    }

    #[test]
    fn tick_command_versions() {
        assert!(!tick_command_supported("1.20.2"));
        assert!(tick_command_supported("1.20.3"));
        assert!(tick_command_supported("1.21"));
        assert!(tick_command_supported("26.1"));
        assert!(tick_command_supported("25w14a"));
        assert!(!tick_command_supported("1.8.9"));
    }

    #[test]
    fn eula_file() {
        let _t = crate::testutil::data_dir();
        let m = manager();
        assert!(!m.eula_accepted());
        std::fs::write(m.inst.server_dir().join("eula.txt"), "#x\neula=false\n").unwrap();
        assert!(!m.eula_accepted());
        m.accept_eula().unwrap();
        assert!(m.eula_accepted());
    }

    #[test]
    fn ansi_is_removed() {
        assert_eq!(ANSI_RE.replace_all("\x1b[32mgreen\x1b[0m", ""), "green");
    }
}
