"""Login sessions that survive a panel restart.

Only a SHA-256 of each token is stored (`DATA_DIR/sessions.json`), so the file alone can't be used
to log in. The file also holds a fingerprint of the panel password: changing PANEL_PASSWORD logs
everybody out. Sessions slide: using one renews it once less than half of its lifetime is left.
"""

import hashlib
import hmac
import json
import os
import secrets
import threading
import time
from pathlib import Path

from . import config

TTL = 30 * 24 * 3600
_lock = threading.Lock()
_sessions: dict[str, float] | None = None  # token hash -> expiry (loaded lazily)


def _file() -> Path:
    return config.DATA_DIR / "sessions.json"


def _fingerprint() -> str:
    return hmac.new(config.PASSWORD.encode(), b"panel-sessions", hashlib.sha256).hexdigest()


def _hash(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


def _load() -> dict[str, float]:
    global _sessions
    if _sessions is None:
        _sessions = {}
        try:
            data = json.loads(_file().read_text(encoding="utf-8"))
            if isinstance(data, dict) and hmac.compare_digest(str(data.get("password", "")), _fingerprint()):
                now = time.time()
                _sessions = {h: float(exp) for h, exp in data.get("sessions", {}).items()
                             if isinstance(h, str) and isinstance(exp, (int, float)) and exp > now}
        except (OSError, ValueError, AttributeError):
            pass
    return _sessions


def _save() -> None:
    try:
        path = _file()
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps({"password": _fingerprint(), "sessions": _sessions}), encoding="utf-8")
        try:
            os.chmod(tmp, 0o600)
        except OSError:
            pass
        tmp.replace(path)
    except OSError:
        pass  # still works for this run, only the restart survival is lost


def create() -> str:
    token = secrets.token_urlsafe(32)
    now = time.time()
    with _lock:
        s = _load()
        for h in [h for h, exp in s.items() if exp < now]:
            del s[h]
        s[_hash(token)] = now + TTL
        _save()
    return token


def check(token: str | None) -> str:
    """'' = invalid, 'ok' = valid, 'renewed' = valid and its lifetime was just extended
    (the caller re-sends the cookie)."""
    if not token:
        return ""
    h = _hash(token)
    now = time.time()
    with _lock:
        s = _load()
        exp = s.get(h)
        if not exp or exp < now:
            if s.pop(h, None) is not None:
                _save()
            return ""
        if exp - now < TTL / 2:
            s[h] = now + TTL
            _save()
            return "renewed"
    return "ok"


def remove(token: str | None) -> None:
    if not token:
        return
    with _lock:
        if _load().pop(_hash(token), None) is not None:
            _save()


def reset_cache() -> None:
    """Tests: forget the loaded sessions (re-read from the file)."""
    global _sessions
    with _lock:
        _sessions = None
