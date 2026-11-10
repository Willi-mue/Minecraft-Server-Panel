"""Minecraft server process management.

One `ServerManager` per server instance (see instances.py), all registered in `managers`.
The Java process runs via subprocess.Popen in a reader thread (works with any asyncio event
loop, including on Windows). Events of all servers are fanned out to the async subscribers
(the WebSockets) through one `EventHub`, tagged with the server id.
"""

import asyncio
import collections
import re
import subprocess
import threading
import time
from enum import Enum

from . import (access, backup, config, datapacks, instances, java, leaderboard, livemap, players, properties,
               servericon, versions, world)
from .instances import Instance

ANSI_RE = re.compile(r"\x1B\[[0-?]*[ -/]*[@-~]")
# Since 26.x, system messages and command feedback are logged as "]: System chat: ..."
MSG = r"\]: (?:System chat: )?"
DONE_RE = re.compile(r"\]: Done \(")
JOIN_RE = re.compile(MSG + r"(\w{1,16})(?: \(formerly known as \w{1,16}\))? joined the game")
LEFT_RE = re.compile(MSG + r"(\w{1,16}) left the game")
# "Name[/1.2.3.4:5678] logged in with entity id ..." (IPv6: "Name[/[::1]:5678] ...")
LOGIN_RE = re.compile(r"\]: (\w{1,16})\[/(.+):\d+\] logged in with entity id")
CHAT_RE = re.compile(r"\]: (?:\[Not Secure\] )?<(\w{1,16})> (.*)")
LIST_RE = re.compile(MSG + r"There are \d+ of a max(?: of)? \d+ players online:(.*)")
SAVED_RE = re.compile(MSG + r"Saved the game")
EULA_RE = re.compile(r"agree to the EULA", re.I)
# "Disconnecting Name (/1.2.3.4:5678): You are not whitelisted on this server!"
REJECTED_RE = re.compile(r"\]: Disconnecting (\w{1,16}) \(.*\): You are not white-?listed")

COUNTDOWN_MARKS = (60, 30, 15, 10, 5, 4, 3, 2, 1)

# Tick rate answers (swallowed, see ServerManager.ask). Vanilla/Fabric 1.20.3+: `tick query`
TICK_LINE_RE = re.compile(MSG + r"(The game is (?:running normally|frozen|sprinting)|Target tick rate|"
                          r"Average time per tick|Percentiles)")
MSPT_RE = re.compile(r"Average time per tick: ([\d.]+) ?ms")
TARGET_RE = re.compile(r"Target tick rate: ([\d.]+)")
# Paper: `tps` -> "TPS from last 1m, 5m, 15m: 20.0, ...", `mspt` -> header + "◴ 1.2/0.4/5.1, ..."
PAPER_TICK_RE = re.compile(r"\]: (?:System chat: )?(TPS from last|Server tick times|◴)")
PAPER_TPS_RE = re.compile(r"TPS from last [^:]*: \*?([\d.]+)")
PAPER_MSPT_RE = re.compile(r"◴ ([\d.]+)/")
HISTORY_POINTS = 120  # every 30 s: one hour


class _Query:
    """Console lines a caller is waiting for (they are not shown in the console)."""

    def __init__(self, pattern: re.Pattern, expected: int) -> None:
        self.pattern = pattern
        self.expected = expected
        self.lines: list[re.Match] = []
        self.done = threading.Event()


class State(str, Enum):
    STOPPED = "stopped"
    STARTING = "starting"
    RUNNING = "running"
    STOPPING = "stopping"


class BusyError(RuntimeError):
    pass


class EventHub:
    """WebSocket fan-out for all servers. Events are pushed from any thread."""

    def __init__(self) -> None:
        self._loop: asyncio.AbstractEventLoop | None = None
        self._subscribers: set[asyncio.Queue] = set()

    def bind_loop(self, loop: asyncio.AbstractEventLoop) -> None:
        self._loop = loop

    def subscribe(self) -> asyncio.Queue:
        q: asyncio.Queue = asyncio.Queue(maxsize=2000)
        self._subscribers.add(q)
        return q

    def unsubscribe(self, q: asyncio.Queue) -> None:
        self._subscribers.discard(q)

    def has_subscribers(self) -> bool:
        return bool(self._subscribers)

    def emit(self, event: dict) -> None:
        if not self._loop:
            return

        def push() -> None:
            for q in list(self._subscribers):
                if q.full():
                    continue  # slow client; drop instead of blocking everyone
                q.put_nowait(event)

        try:
            self._loop.call_soon_threadsafe(push)
        except RuntimeError:
            pass  # event loop already closed (app shutting down)


hub = EventHub()


class ServerManager:
    def __init__(self, inst: Instance) -> None:
        self.inst = inst
        self.process: subprocess.Popen | None = None
        self.state = State.STOPPED
        self.players: set[str] = set()
        self.rejected: list[str] = []  # recently refused by the whitelist (newest first)
        self.started_at: float | None = None
        self.running_version: str | None = None
        self.running_flavor: str | None = None
        self.running_max_ram_mb: int | None = None  # -Xmx of the running server
        self.map_enabled = config.load_settings(inst).map_enabled  # kept in sync by the settings API
        self.task: str | None = None  # long-running action in progress
        self.eula_missing = False
        self.log = collections.deque(maxlen=1000)
        self.chat = collections.deque(maxlen=300)
        self.history = collections.deque(maxlen=HISTORY_POINTS)  # {t, cpu, ram, players, tps, mspt}
        self.tick: dict | None = None  # last {tps, mspt, t} while somebody watches the panel
        self._tick_supported = True
        self._queries: list[_Query] = []
        self._queries_lock = threading.Lock()

        self._action_lock = threading.Lock()
        self._stdin_lock = threading.Lock()
        self._saved_event = threading.Event()
        self._list_event = threading.Event()  # set when a `list` answer was parsed
        self._cancel_countdown = threading.Event()

    @property
    def has_map(self) -> bool:
        """Every server has its own live map (kept for the API / UI flag)."""
        return True

    @property
    def map_path(self) -> str:
        """Where this server's map is on the map port (/ = the server the browser showed last)."""
        return f"/s/{self.inst.id}/"

    # ---------- events ----------

    def _emit(self, event: dict) -> None:
        hub.emit({**event, "server": self.inst.id})

    def info(self, text: str) -> None:
        """Message from the panel itself (not from the Minecraft process)."""
        line = f"[Panel] {text}"
        self.log.append(line)
        self._emit({"type": "log", "line": line})

    def status(self) -> dict:
        return {
            "server": self.inst.id,
            "state": self.state.value,
            "task": self.task,
            "players": sorted(self.players),
            "rejected": list(self.rejected),
            # The panel serves the map files itself, so it is reachable even while stopped
            "map_port": config.MAP_PUBLIC_PORT if self.map_enabled and livemap.available(self.inst) else None,
            "map_path": self.map_path,
            "version": self.running_version,
            "flavor": self.running_flavor,
            "uptime": int(time.time() - self.started_at) if self.started_at else 0,
            "pid": self.process.pid if self.is_alive() else None,
            "eula_missing": self.eula_missing,
        }

    def emit_status(self) -> None:
        self._emit({"type": "status", "status": self.status()})

    def _set_state(self, state: State) -> None:
        self.state = state
        self.emit_status()

    def _set_task(self, task: str | None) -> None:
        self.task = task
        self.emit_status()

    def is_alive(self) -> bool:
        return self.process is not None and self.process.poll() is None

    # ---------- actions ----------

    def _exclusive(self, name: str, fn, *args) -> None:
        if not self._action_lock.acquire(blocking=False):
            raise BusyError(f"Another action is running: {self.task}")

        def run() -> None:
            self._set_task(name)
            try:
                fn(*args)
            except Exception as e:  # noqa: BLE001 - report everything to the UI
                self.info(f"{name} failed: {e}")
            finally:
                self._action_lock.release()
                self._set_task(None)

        threading.Thread(target=run, daemon=True).start()

    def start(self) -> None:
        self._exclusive("start", self._start)

    def stop(self, countdown: int = 0) -> None:
        self._exclusive("stop", self._stop, countdown, "stopping")

    def restart(self, countdown: int = 0) -> None:
        self._exclusive("restart", self._restart, countdown)

    def make_backup(self) -> None:
        self._exclusive("backup", self._backup)

    def render_map(self) -> None:
        """'Render now': flush the world to disk first if the server runs, then render."""
        renderer = livemap.renderer_for(self.inst)
        if renderer.busy:
            raise BusyError("The map is already being rendered")

        def run() -> None:
            if self.state == State.RUNNING:
                self._saved_event.clear()
                # flush: wait until the chunks are written (Paper saves asynchronously otherwise)
                self.send_command("save-all flush")
                if not self._saved_event.wait(timeout=60):
                    self.info("Live map: no save confirmation, rendering what is on disk.")
            renderer.render(self.info, announce=True)
            self.emit_status()

        threading.Thread(target=run, daemon=True).start()

    def save_and_wait(self, timeout: float = 30) -> bool:
        """Blocking `save-all` (writes level.dat too); True once the server confirmed it."""
        self._saved_event.clear()
        self.send_command("save-all")
        return self._saved_event.wait(timeout=timeout)

    def restore(self, name: str) -> None:
        if self.is_alive():
            raise RuntimeError("Stop the server before restoring a backup")
        backup.backup_file(self.inst, name)  # 404 early instead of failing in the background
        self._exclusive("restore", self._restore, name)

    def _restore(self, name: str) -> None:
        self.info(f"Restoring {name} ...")
        safety = backup.restore(self.inst, name)
        if safety:
            self.info(f"Current world saved first as {safety.name}.")
        # The restored world may come from another version: let level.dat tell
        s = config.load_settings(self.inst)
        s.world_version = s.world_flavor = ""
        config.save_settings(s, self.inst)
        self.info(f"Backup {name} restored.")

    def new_world(self, seed: str, packs: list[str] | None = None) -> None:
        if self.is_alive():
            raise RuntimeError("Stop the server before creating a new world")
        seed = seed.strip()
        if len(seed) > 64 or "\n" in seed or "\r" in seed:
            raise ValueError("Seed: up to 64 characters, single line")
        packs = [datapacks.validate_name(n) for n in packs or []]
        self._exclusive("new world", self._new_world, seed, packs)

    def _new_world(self, seed: str, packs: list[str] | None = None) -> None:
        inst = self.inst
        # Save world-only packs to the library first: the old world is removed below
        packs = datapacks.prepare_new_world(inst, packs or [])
        if backup.world_dirs(inst):
            s = config.load_settings(inst)
            label = s.world_version or world.saved_version(inst) or ""
            self.info("Archiving the current world ...")
            archive = backup.create_archive(inst, label)
            self.info(f"Old world archived as {archive.name} (never deleted automatically).")
            backup.remove_world(inst)
        access.set_property(inst, "level-seed", seed)
        # No experiments in new worlds (also clears a value an older panel version may have left)
        access.set_property(inst, "initial-enabled-packs", "vanilla")
        # In place before the first start, so world generation already uses them
        datapacks.install_many(inst, packs)
        livemap.clear(inst)
        s = config.load_settings(inst)
        s.world_version = s.world_flavor = ""
        config.save_settings(s, inst)
        self.info("New world prepared" + (f" with seed {seed}" if seed else " (random seed)")
                  + (f" and {len(packs)} data pack(s)" if packs else "")
                  + ". It is generated on the next start.")

    def install(self, flavor: str, version_id: str) -> None:
        self._exclusive("download", versions.download, flavor, version_id, self.info)

    def update_jar(self, flavor: str, version_id: str) -> None:
        """Newest Paper build / Fabric loader."""
        self._exclusive("download", versions.update, flavor, version_id, self.info)

    def backup_before(self, suffix: str) -> None:
        """Blocking backup of a stopped server's world (e.g. before switching the server type)."""
        if self.is_alive():
            raise RuntimeError("Stop the server first")
        if not self._action_lock.acquire(blocking=False):
            raise BusyError(f"Another action is running: {self.task}")
        try:
            if backup.world_dirs(self.inst):
                self.info("Creating a backup first ...")
                path = backup.create_backup(self.inst, suffix)
                self.info(f"Backup saved: {path.name}")
        finally:
            self._action_lock.release()

    def kill(self) -> None:
        self._cancel_countdown.set()
        if self.is_alive():
            self.process.kill()
            self.info("Server process killed.")

    def cancel_countdown(self) -> None:
        self._cancel_countdown.set()

    def send_command(self, cmd: str) -> None:
        proc = self.process
        if proc is None or proc.poll() is not None:
            raise RuntimeError("Server is not running")
        try:
            with self._stdin_lock:
                proc.stdin.write(cmd + "\n")
                proc.stdin.flush()
        except (OSError, ValueError) as e:  # broken pipe / closed stdin
            raise RuntimeError(f"Could not send command: {e}") from e

    # ---------- implementation ----------

    def _check_port(self) -> None:
        """Two running servers can't share a game port."""
        port = instances.game_port(self.inst)
        for other in list(managers.values()):
            if other is not self and other.is_alive() and instances.game_port(other.inst) == port:
                raise RuntimeError(f"Port {port} is already used by the running server "
                                   f"'{instances.name_of(other.inst)}'")

    def _start(self) -> None:
        if self.is_alive():
            self.info("Server is already running.")
            return

        inst = self.inst
        settings = config.load_settings(inst)
        if not settings.version:
            raise RuntimeError("No Minecraft version selected: choose one in the Server card")
        flavor, version = settings.flavor, settings.version
        self._check_port()

        jar = versions.download(flavor, version, self.info)

        if not self._eula_accepted():
            self.eula_missing = True
            self.emit_status()
            raise RuntimeError("Minecraft EULA not accepted yet")

        try:
            properties.apply_presets(inst)
        except OSError as e:
            self.info(f"Could not write default view/simulation distance: {e}")
        try:
            servericon.ensure(inst)  # the panel logo in Minecraft's server list, until the user sets one
        except OSError as e:
            self.info(f"Could not write the server icon: {e}")

        try:
            if livemap.remove_squaremap(self.inst):
                self.info("Removed the old squaremap plugin: the live map is now built into the panel.")
        except OSError as e:  # the map must never block the server start
            self.info(f"Could not remove the old squaremap plugin: {e}")

        required = versions.required_java(flavor, version)
        runtime = java.pick(required)
        if runtime is None:
            raise RuntimeError("No Java runtime found (install Java or use the Docker image)")
        java_bin, java_major = runtime
        if java_major < required:
            self.info(f"Warning: {version} needs Java {required}, only Java {java_major} available.")

        cmd = [
            java_bin,
            f"-Xms{settings.min_ram}",
            f"-Xmx{settings.max_ram}",
            # Paper ships recommended GC flags (Aikar's flags); empty for Vanilla
            *versions.jvm_flags(flavor, version),
            "-jar",
            str(jar),
            "nogui",
        ]
        self.info(f"Starting {flavor.capitalize()} {version} with Java {java_major}")
        self.players.clear()
        inst.ensure_dirs()
        self.process = subprocess.Popen(
            cmd,
            cwd=inst.server_dir,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
        )
        self.running_version = version
        self.running_flavor = flavor
        self.running_max_ram_mb = config.ram_mb(settings.max_ram)
        self.started_at = time.time()
        self._set_state(State.STARTING)
        threading.Thread(target=self._reader, args=(self.process,), daemon=True).start()

    def _countdown(self, seconds: int, word: str) -> bool:
        """Announce in chat; returns False if cancelled."""
        self._cancel_countdown.clear()
        if seconds <= 0:
            return True
        if not self.players and self.state == State.RUNNING:
            # Don't trust a possibly stale list: ask the server before skipping the warning
            self._list_event.clear()
            self.send_command("list")
            self._list_event.wait(timeout=3)
        if not self.players:
            # The countdown only warns players in chat; nobody to warn
            self.info(f"Nobody online, {word} without countdown.")
            return True
        remaining = seconds
        for mark in [m for m in COUNTDOWN_MARKS if m <= seconds] + [0]:
            if self._cancel_countdown.wait(remaining - mark):
                self.send_command(f"say Server {word} cancelled")
                self.info("Countdown cancelled.")
                return False
            remaining = mark
            if mark:
                self.send_command(f"say Server {word} in {mark} second{'s' if mark != 1 else ''}")
        return True

    def _stop(self, countdown: int, word: str) -> bool:
        if not self.is_alive():
            self.info("Server is not running.")
            return True
        if not self._countdown(countdown, word):
            return False
        if not self.is_alive():  # crashed/killed during the countdown
            return True
        self._set_state(State.STOPPING)
        self.send_command("stop")
        try:
            self.process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            self.info("Server did not stop within 120 s, killing it.")
            self.process.kill()
            self.process.wait()
        return True

    def _restart(self, countdown: int) -> None:
        if self._stop(countdown, "restarting"):
            self._start()

    def _backup(self) -> None:
        if self.is_alive() and self.state != State.RUNNING:
            # Starting/stopping: the world is being written, a copy would be inconsistent
            raise RuntimeError(f"Server is {self.state.value}, try again in a moment")
        if not self.is_alive():
            self.info("Creating backup ...")
            path = backup.create_backup(self.inst)
            self.info(f"Backup saved: {path.name}")
            return

        # Live backup: flush the world to disk and pause autosave while zipping.
        self.info("Creating live backup (autosave paused) ...")
        self.send_command("say Backup started, the server may lag briefly")
        self._saved_event.clear()
        self.send_command("save-off")
        self.send_command("save-all flush")
        if not self._saved_event.wait(timeout=120):
            self.info("Warning: no 'Saved the game' confirmation, backing up anyway.")
        try:
            path = backup.create_backup(self.inst)
        finally:
            if self.is_alive():
                self.send_command("save-on")
        self.send_command("say Backup finished")
        self.info(f"Backup saved: {path.name}")

    # ---------- output parsing ----------

    def _reader(self, proc: subprocess.Popen) -> None:
        for raw in proc.stdout:
            line = ANSI_RE.sub("", raw.rstrip())
            if line:
                self._handle_line(line)
        proc.wait()
        if proc is self.process:
            self.players.clear()
            self.tick = None
            self._tick_supported = True
            try:
                players.all_offline(self.inst)
            except OSError:
                pass
            self.started_at = None
            self.running_version = None
            self.running_flavor = None
            livemap.tracker_for(self.inst).clear()
            self.info(f"Server process exited (code {proc.returncode}).")
            self._set_state(State.STOPPED)
            if self.map_enabled:
                # Everything is saved now: bring the map up to date
                threading.Thread(target=livemap.renderer_for(self.inst).render, args=(self.info,), daemon=True).start()

    def ask(self, commands: list[str], pattern: re.Pattern, expected: int = 1, timeout: float = 3.0) -> list[re.Match]:
        """Send commands and collect the console lines matching `pattern` (up to `expected` of them,
        within `timeout`). Matching lines are swallowed, so the console stays clean."""
        q = _Query(pattern, expected)
        with self._queries_lock:
            self._queries.append(q)
        try:
            for cmd in commands:
                self.send_command(cmd)
            q.done.wait(timeout)
        finally:
            with self._queries_lock:
                self._queries.remove(q)
        return q.lines

    def _consume_query(self, line: str) -> bool:
        with self._queries_lock:
            for q in self._queries:
                if len(q.lines) < q.expected and (m := q.pattern.search(line)):
                    q.lines.append(m)
                    if len(q.lines) >= q.expected:
                        q.done.set()
                    return True
        return False

    def query_tick(self) -> dict | None:
        """TPS / milliseconds per tick (Paper: tps + mspt, else `tick query` on 1.20.3+)."""
        if self.state != State.RUNNING or not self._tick_supported:
            return None
        tps = mspt = None
        if self.running_flavor == "paper":
            for m in self.ask(["tps", "mspt"], PAPER_TICK_RE, expected=3, timeout=2):
                line = m.string
                if t := PAPER_TPS_RE.search(line):
                    tps = float(t.group(1))
                elif t := PAPER_MSPT_RE.search(line):
                    mspt = float(t.group(1))
        else:
            if not tick_command_supported(self.running_version or ""):
                self._tick_supported = False
                return None
            target = 20.0
            for m in self.ask(["tick query"], TICK_LINE_RE, expected=4, timeout=2):
                if t := MSPT_RE.search(m.string):
                    mspt = float(t.group(1))
                elif t := TARGET_RE.search(m.string):
                    target = float(t.group(1))
            if mspt is not None:
                tps = round(min(target, 1000 / mspt) if mspt > 0 else target, 1)
        if tps is None and mspt is None:
            return None
        self.tick = {"tps": tps, "mspt": mspt, "t": time.time()}
        return self.tick

    def _handle_line(self, line: str) -> None:
        if self._queries and self._consume_query(line):
            return  # answer to the panel's own query
        if livemap.tracker_for(self.inst).consume(line):
            return  # answer to the map's own position query, not interesting in the console
        self.log.append(line)
        self._emit({"type": "log", "line": line})

        if self.state == State.STARTING and DONE_RE.search(line):
            self._remember_world_version()
            self._set_state(State.RUNNING)
            self.send_command("list")
            if self.running_flavor == "paper":
                # Paper has fetched the Mojang jar by now; offer it as a Vanilla server too
                threading.Thread(target=versions.import_paper_cache, args=(self.inst.server_dir, self.info),
                                 daemon=True).start()
        elif m := LOGIN_RE.search(line):
            leaderboard.remember_login(self.inst, m.group(1), m.group(2))  # memory only: "Me" on the map
        elif m := JOIN_RE.search(line):
            self.players.add(m.group(1))
            players.record(self.inst, m.group(1), "join")
            self._chat(f"→ {m.group(1)} joined")
        elif m := LEFT_RE.search(line):
            self.players.discard(m.group(1))
            players.record(self.inst, m.group(1), "leave")
            self._chat(f"← {m.group(1)} left")
        elif m := CHAT_RE.search(line):
            self._chat(f"<{m.group(1)}> {m.group(2)}")
        elif m := LIST_RE.search(line):
            names = {n.strip() for n in m.group(1).split(",") if n.strip()}
            self.players = names
            self._list_event.set()
            self.emit_status()
        elif m := REJECTED_RE.search(line):
            name = m.group(1)
            self.rejected = [name] + [n for n in self.rejected if n != name][:4]
            self.emit_status()
        elif SAVED_RE.search(line):
            self._saved_event.set()
        elif EULA_RE.search(line):
            self.eula_missing = True

    def _chat(self, text: str) -> None:
        entry = {"time": time.strftime("%H:%M:%S"), "text": text}
        self.chat.append(entry)
        self._emit({"type": "chat", "entry": entry})
        self.emit_status()

    def _remember_world_version(self) -> None:
        """The world has now been loaded (and converted) by this version."""
        s = config.load_settings(self.inst)
        s.world_version = self.running_version or ""
        s.world_flavor = self.running_flavor or ""
        config.save_settings(s, self.inst)

    def _eula_accepted(self) -> bool:
        eula = self.inst.server_dir / "eula.txt"
        if not eula.exists():
            return False
        for line in eula.read_text(encoding="utf-8", errors="replace").splitlines():
            key, _, value = line.strip().partition("=")
            if key.strip().lower() == "eula":
                return value.strip().lower() == "true"
        return False

    def accept_eula(self) -> None:
        self.inst.ensure_dirs()
        (self.inst.server_dir / "eula.txt").write_text(
            "# Accepted via Minecraft Server Panel (https://aka.ms/MinecraftEULA)\neula=true\n"
        )
        self.eula_missing = False
        self.emit_status()

    def shutdown(self) -> None:
        """Called when the web app exits (e.g. `docker stop`)."""
        self._cancel_countdown.set()
        if self.is_alive():
            try:
                self.send_command("stop")
                self.process.wait(timeout=60)
            except Exception:  # noqa: BLE001
                self.process.kill()


def tick_command_supported(version: str) -> bool:
    """`/tick query` exists since 1.20.3 (and in every 26.x release); snapshots: assume yes."""
    m = re.match(r"^(\d+)\.(\d+)(?:\.(\d+))?$", version)
    if not m:
        return True
    major, minor, patch = int(m.group(1)), int(m.group(2)), int(m.group(3) or 0)
    return major > 1 or (minor, patch) >= (20, 3)


# ---------- all servers ----------

managers: dict[str, ServerManager] = {}
_managers_lock = threading.Lock()


def load_managers() -> None:
    """One manager per registered instance (called at startup)."""
    with _managers_lock:
        for entry in instances.all_entries():
            if entry["id"] not in managers:
                managers[entry["id"]] = ServerManager(Instance(entry["id"]))


def get_manager(server_id: str) -> ServerManager:
    """KeyError if the server doesn't exist."""
    with _managers_lock:
        if server_id not in managers:
            # Registered meanwhile (or before load_managers ran, e.g. in tests)
            managers[server_id] = ServerManager(instances.get(server_id))
        return managers[server_id]


def add_manager(inst: Instance) -> ServerManager:
    with _managers_lock:
        m = managers[inst.id] = ServerManager(inst)
        return m


def remove_manager(server_id: str) -> None:
    with _managers_lock:
        managers.pop(server_id, None)


def shutdown_all() -> None:
    """Stop every running server in parallel (Docker's stop_grace_period is shared)."""
    threads = [threading.Thread(target=m.shutdown) for m in list(managers.values()) if m.is_alive()]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
