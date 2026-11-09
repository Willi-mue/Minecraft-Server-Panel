"""Find a Java runtime that matches the Minecraft version's requirement."""

import re
import shutil
import subprocess
from functools import lru_cache

from . import config

_VERSION_RE = re.compile(r'version "(\d+)(?:\.(\d+))?')


def _major_of(java_bin: str) -> int | None:
    try:
        out = subprocess.run(
            [java_bin, "-version"], capture_output=True, text=True, timeout=15
        ).stderr
    except (OSError, subprocess.SubprocessError):
        return None
    m = _VERSION_RE.search(out)
    if not m:
        return None
    major = int(m.group(1))
    # Java 8 reports itself as "1.8.0_..."
    return int(m.group(2)) if major == 1 and m.group(2) else major


@lru_cache(maxsize=1)
def runtimes() -> dict[int, str]:
    """All available runtimes as {major_version: java_binary}."""
    found: dict[int, str] = {}
    if config.JAVA_DIR.is_dir():
        for d in config.JAVA_DIR.iterdir():
            for exe in ("java", "java.exe"):
                b = d / "bin" / exe
                if b.exists() and (major := _major_of(str(b))):
                    found.setdefault(major, str(b))
    path_java = shutil.which("java")
    if path_java and (major := _major_of(path_java)):
        found.setdefault(major, path_java)
    return dict(sorted(found.items()))


def pick(required: int) -> tuple[str, int] | None:
    """Smallest runtime >= required, else the newest available (best effort)."""
    rts = runtimes()
    if not rts:
        return None
    for major, binary in rts.items():
        if major >= required:
            return binary, major
    major = max(rts)
    return rts[major], major
