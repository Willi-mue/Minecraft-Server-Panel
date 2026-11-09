"""Runtime configuration.

Static settings come from environment variables (see `.env.example`).
Settings the user changes in the web UI (version, RAM) are persisted in
`<DATA_DIR>/settings.json` and override the environment defaults.
"""

import json
import os
import re
import threading
from dataclasses import asdict, dataclass
from pathlib import Path

DATA_DIR = Path(os.environ.get("DATA_DIR", "data")).resolve()
SERVER_DIR = DATA_DIR / "server"
JARS_DIR = DATA_DIR / "jars"
# Empty/unset = inside the data directory. (In Docker, compose mounts the chosen host folder here.)
BACKUP_DIR = Path(os.environ.get("BACKUP_DIR") or DATA_DIR / "backups").resolve()
SETTINGS_FILE = DATA_DIR / "settings.json"

WEB_HOST = os.environ.get("WEB_HOST", "0.0.0.0")
WEB_PORT = int(os.environ.get("WEB_PORT", "47318"))
PASSWORD = os.environ.get("MCGUI_PASSWORD", "")

# Live map: port the public map server listens on, and the port players use to reach it
# (differs in Docker, where compose maps MAP_PORT_HOST -> MAP_PORT)
MAP_PORT = int(os.environ.get("MAP_PORT", "47319"))
MAP_PUBLIC_PORT = int(os.environ.get("MAP_PORT_HOST") or MAP_PORT)
# Renderer (mapgen): threads it may use, how often to check for changes while the server runs
MAP_THREADS = max(1, int(os.environ.get("MAP_THREADS") or 2))
MAP_INTERVAL = max(15, int(os.environ.get("MAP_INTERVAL") or 60))
MAPGEN_BIN = os.environ.get("MAPGEN_BIN", "")

# Automatic backups every N hours while the server runs (0 = off)
BACKUP_INTERVAL_HOURS = max(0.0, float(os.environ.get("BACKUP_INTERVAL_HOURS") or 24))
# At least 1, otherwise cleanup would delete the backup that was just created
BACKUP_KEEP = max(1, int(os.environ.get("BACKUP_KEEP", "7")))
# Directory containing Java runtimes named by major version, e.g. /opt/java/21
JAVA_DIR = Path(os.environ.get("JAVA_DIR", "/opt/java"))

_RAM_RE = re.compile(r"^[1-9]\d*[MG]$")
# Mojang version ids: "1.21.4", "26.4-snapshot-7", "1.14 Pre-Release 1", ... (no path separators)
_VERSION_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._\- ]{0,63}$")


@dataclass
class Settings:
    flavor: str = os.environ.get("MC_FLAVOR", "paper")  # "paper" (default) or "vanilla"
    version: str = os.environ.get("MC_VERSION", "")
    min_ram: str = os.environ.get("MIN_RAM", "1G")
    max_ram: str = os.environ.get("MAX_RAM", "3G")
    map_enabled: bool = os.environ.get("MAP_ENABLED", "true").lower() != "false"
    # Version/flavor that last ran the world (for upgrade/downgrade warnings)
    world_version: str = ""
    world_flavor: str = ""


_lock = threading.Lock()


def ensure_dirs() -> None:
    for d in (DATA_DIR, SERVER_DIR, JARS_DIR, BACKUP_DIR):
        d.mkdir(parents=True, exist_ok=True)


def load_settings() -> Settings:
    with _lock:
        s = Settings()
        if SETTINGS_FILE.exists():
            try:
                data = json.loads(SETTINGS_FILE.read_text(encoding="utf-8"))
                for key, default in asdict(s).items():
                    if type(data.get(key)) is type(default):  # ignore wrong/missing types
                        setattr(s, key, data[key])
            except (OSError, ValueError):
                pass
        return s


def save_settings(s: Settings) -> None:
    with _lock:
        SETTINGS_FILE.write_text(json.dumps(asdict(s), indent=2), encoding="utf-8")


def validate_ram(value: str) -> str:
    value = value.strip().upper()
    if not _RAM_RE.match(value):
        raise ValueError("RAM must look like 512M or 4G")
    return value


def validate_version(value: str) -> str:
    value = value.strip()
    if not _VERSION_RE.match(value):
        raise ValueError("Invalid version id")
    return value


def ram_mb(value: str) -> int:
    n = int(value[:-1])
    return n * 1024 if value.endswith("G") else n
