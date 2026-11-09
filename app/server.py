"""Minecraft server process management.

The Java process runs via subprocess.Popen in a reader thread (works with any
asyncio event loop, including on Windows). Events are fanned out to async
subscribers (the WebSockets) through `loop.call_soon_threadsafe`.
"""

import asyncio
import collections
import re
import subprocess
import threading
import time
from enum import Enum

from . import access, backup, config, datapacks, java, livemap, properties, versions, world

ANSI_RE = re.compile(r"\x1B\[[0-?]*[ -/]*[@-~]")
# Since 26.x, system messages and command feedback are logged as "]: System chat: ..."
MSG = r"\]: (?:System chat: )?"
DONE_RE = re.compile(r"\]: Done \(")
JOIN_RE = re.compile(MSG + r"(\w{1,16})(?: \(formerly known as \w{1,16}\))? joined the game")
LEFT_RE = re.compile(MSG + r"(\w{1,16}) left the game")
CHAT_RE = re.compile(r"\]: (?:\[Not Secure\] )?<(\w{1,16})> (.*)")
LIST_RE = re.compile(MSG + r"There are \d+ of a max(?: of)? \d+ players online:(.*)")
SAVED_RE = re.compile(MSG + r"Saved the game")
EULA_RE = re.compile(r"agree to the EULA", re.I)
# "Disconnecting Name (/1.2.3.4:5678): You are not whitelisted on this server!"
REJECTED_RE = re.compile(r"\]: Disconnecting (\w{1,16}) \(.*\): You are not white-?listed")

COUNTDOWN_MARKS = (60, 30, 15, 10, 5, 4, 3, 2, 1)


class State(str, Enum):
    STOPPED = "stopped"
    STARTING = "starting"
    RUNNING = "running"
    STOPPING = "stopping"


class BusyError(RuntimeError):
    pass


class ServerManager:
    def __init__(self) -> None:
        self.process: subprocess.Popen | None = None
        self.state = State.STOPPED
        self.players: set[str] = set()
        self.rejected: list[str] = []  # recently refused by the whitelist (newest first)
        self.started_at: float | None = None
        self.running_version: str | None = None
        self.running_flavor: str | None = None
        self.running_max_ram_mb: int | None = None  # -Xmx of the running server
        self.map_enabled = config.load_settings().map_enabled  # kept in sync by the settings API
        self.task: str | None = None  # long-running action in progress
        self.eula_missing = False
        self.log = collections.deque(maxlen=1000)
        self.chat = collections.deque(maxlen=300)

        self._action_lock = threading.Lock()
        self._stdin_lock = threading.Lock()
        self._saved_event = threading.Event()
        self._list_event = threading.Event()  # set when a `list` answer was parsed
        self._cancel_countdown = threading.Event()
        self._loop: asyncio.AbstractEventLoop | None = None
        self._subscribers: set[asyncio.Queue] = set()

    # ---------- events ----------

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

    def _emit(self, event: dict) -> None:
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

    def info(self, text: str) -> None:
        """Message from the GUI itself (not from the Minecraft process)."""
        line = f"[GUI] {text}"
        self.log.append(line)
        self._emit({"type": "log", "line": line})

    def status(self) -> dict:
        return {
            "state": self.state.value,
            "task": self.task,
            "players": sorted(self.players),
            "rejected": list(self.rejected),
            # The panel serves the map files itself, so it is reachable even while stopped
            "map_port": config.MAP_PUBLIC_PORT if self.map_enabled and livemap.available() else None,
            "version": self.running_version,
            "flavor": self.running_flavor,
            "uptime": int(time.time() - self.started_at) if self.started_at else 0,
            "pid": self.process.pid if self.is_alive() else None,
            "eula_missing": self.eula_missing,
        }

    def _set_state(self, state: State) -> None:
        self.state = state
        self._emit({"type": "status", "status": self.status()})

    def _set_task(self, task: str | None) -> None:
        self.task = task
        self._emit({"type": "status", "status": self.status()})

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
        if livemap.renderer.busy:
            raise BusyError("The map is already being rendered")

        def run() -> None:
            if self.state == State.RUNNING:
                self._saved_event.clear()
                # flush: wait until the chunks are written (Paper saves asynchronously otherwise)
                self.send_command("save-all flush")
                if not self._saved_event.wait(timeout=60):
                    self.info("Live map: no save confirmation, rendering what is on disk.")
            livemap.renderer.render(self.info, announce=True)
            self._emit({"type": "status", "status": self.status()})

        threading.Thread(target=run, daemon=True).start()

    def save_and_wait(self, timeout: float = 30) -> bool:
        """Blocking `save-all` (writes level.dat too); True once the server confirmed it."""
        self._saved_event.clear()
        self.send_command("save-all")
        return self._saved_event.wait(timeout=timeout)

    def restore(self, name: str) -> None:
        if self.is_alive():
            raise RuntimeError("Stop the server before restoring a backup")
        backup.backup_file(name)  # 404 early instead of failing in the background
        self._exclusive("restore", self._restore, name)

    def _restore(self, name: str) -> None:
        self.info(f"Restoring {name} ...")
        safety = backup.restore(name)
        if safety:
            self.info(f"Current world saved first as {safety.name}.")
        # The restored world may come from another version: let level.dat tell
        s = config.load_settings()
        s.world_version = s.world_flavor = ""
        config.save_settings(s)
        self.info(f"Backup {name} restored.")

    def new_world(self, seed: str, packs: list[str] | None = None, experiments: list[str] | None = None) -> None:
        if self.is_alive():
            raise RuntimeError("Stop the server before creating a new world")
        seed = seed.strip()
        if len(seed) > 64 or "\n" in seed or "\r" in seed:
            raise ValueError("Seed: up to 64 characters, single line")
        packs = [datapacks.validate_name(n) for n in packs or []]
        initial = datapacks.initial_packs_value(experiments or [])  # validates
        self._exclusive("new world", self._new_world, seed, packs, initial)

    def _new_world(self, seed: str, packs: list[str] | None = None, initial_packs: str = "vanilla") -> None:
        # Save world-only packs to the library first: the old world is removed below
        packs = datapacks.prepare_new_world(packs or [])
        if backup.world_dirs():
            s = config.load_settings()
            label = s.world_version or world.saved_version() or ""
            self.info("Archiving the current world ...")
            archive = backup.create_archive(label)
            self.info(f"Old world archived as {archive.name} (never deleted automatically).")
            backup.remove_world()
        access.set_property("level-seed", seed)
        # Experimental feature packs can only be switched on when the world is created
        access.set_property("initial-enabled-packs", initial_packs)
        # In place before the first start, so world generation already uses them
        datapacks.install_many(packs)
        livemap.clear()
        s = config.load_settings()
        s.world_version = s.world_flavor = ""
        config.save_settings(s)
        self.info("New world prepared" + (f" with seed {seed}" if seed else " (random seed)")
                  + (f" and {len(packs)} data pack(s)" if packs else "")
                  + (f", experiments: {initial_packs.removeprefix('vanilla,')}" if initial_packs != "vanilla" else "")
                  + ". It is generated on the next start.")

    def install(self, flavor: str, version_id: str) -> None:
        self._exclusive("download", versions.download, flavor, version_id, self.info)

    def update_paper(self, version_id: str) -> None:
        self._exclusive("download", versions.update_paper, version_id, self.info)

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

    def _start(self) -> None:
        if self.is_alive():
            self.info("Server is already running.")
            return

        settings = config.load_settings()
        if not settings.version:
            raise RuntimeError("No Minecraft version selected: choose one in the Server card")
        flavor, version = settings.flavor, settings.version

        jar = versions.download(flavor, version, self.info)

        if not self._eula_accepted():
            self.eula_missing = True
            self._emit({"type": "status", "status": self.status()})
            raise RuntimeError("Minecraft EULA not accepted yet")

        try:
            properties.apply_presets()
        except OSError as e:
            self.info(f"Could not write default view/simulation distance: {e}")

        try:
            if livemap.remove_squaremap():
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
        self.process = subprocess.Popen(
            cmd,
            cwd=config.SERVER_DIR,
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
            path = backup.create_backup()
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
            path = backup.create_backup()
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
            self.started_at = None
            self.running_version = None
            self.running_flavor = None
            livemap.players.clear()
            self.info(f"Server process exited (code {proc.returncode}).")
            self._set_state(State.STOPPED)
            if self.map_enabled:
                # Everything is saved now: bring the map up to date
                threading.Thread(target=livemap.renderer.render, args=(self.info,), daemon=True).start()

    def _handle_line(self, line: str) -> None:
        if livemap.players.consume(line):
            return  # answer to the map's own position query, not interesting in the console
        self.log.append(line)
        self._emit({"type": "log", "line": line})

        if self.state == State.STARTING and DONE_RE.search(line):
            self._remember_world_version()
            self._set_state(State.RUNNING)
            self.send_command("list")
            if self.running_flavor == "paper":
                # Paper has fetched the Mojang jar by now; offer it as a Vanilla server too
                threading.Thread(target=versions.import_paper_cache, args=(self.info,), daemon=True).start()
        elif m := JOIN_RE.search(line):
            self.players.add(m.group(1))
            self._chat(f"→ {m.group(1)} joined")
        elif m := LEFT_RE.search(line):
            self.players.discard(m.group(1))
            self._chat(f"← {m.group(1)} left")
        elif m := CHAT_RE.search(line):
            self._chat(f"<{m.group(1)}> {m.group(2)}")
        elif m := LIST_RE.search(line):
            names = {n.strip() for n in m.group(1).split(",") if n.strip()}
            self.players = names
            self._list_event.set()
            self._emit({"type": "status", "status": self.status()})
        elif m := REJECTED_RE.search(line):
            name = m.group(1)
            self.rejected = [name] + [n for n in self.rejected if n != name][:4]
            self._emit({"type": "status", "status": self.status()})
        elif SAVED_RE.search(line):
            self._saved_event.set()
        elif EULA_RE.search(line):
            self.eula_missing = True

    def _chat(self, text: str) -> None:
        entry = {"time": time.strftime("%H:%M:%S"), "text": text}
        self.chat.append(entry)
        self._emit({"type": "chat", "entry": entry})
        self._emit({"type": "status", "status": self.status()})

    def _remember_world_version(self) -> None:
        """The world has now been loaded (and converted) by this version."""
        s = config.load_settings()
        s.world_version = self.running_version or ""
        s.world_flavor = self.running_flavor or ""
        config.save_settings(s)

    @staticmethod
    def _eula_accepted() -> bool:
        eula = config.SERVER_DIR / "eula.txt"
        if not eula.exists():
            return False
        for line in eula.read_text(encoding="utf-8", errors="replace").splitlines():
            key, _, value = line.strip().partition("=")
            if key.strip().lower() == "eula":
                return value.strip().lower() == "true"
        return False

    def accept_eula(self) -> None:
        (config.SERVER_DIR / "eula.txt").write_text(
            "# Accepted via Minecraft Server GUI (https://aka.ms/MinecraftEULA)\neula=true\n"
        )
        self.eula_missing = False
        self._emit({"type": "status", "status": self.status()})

    def shutdown(self) -> None:
        """Called when the web app exits (e.g. `docker stop`)."""
        self._cancel_countdown.set()
        if self.is_alive():
            try:
                self.send_command("stop")
                self.process.wait(timeout=60)
            except Exception:  # noqa: BLE001
                self.process.kill()


manager = ServerManager()
