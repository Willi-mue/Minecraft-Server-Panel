"""Install extra Eclipse Temurin JREs into /opt/java/<version> (used at image build time).

Usage: python3 install-java.py 21 17 8
Each archive is verified against the SHA256 checksum published by the Adoptium API.
"""

import hashlib
import json
import os
import platform
import sys
import tarfile
import tempfile
import urllib.request

ARCH = {"x86_64": "x64", "aarch64": "aarch64"}[platform.machine()]
API = "https://api.adoptium.net/v3/assets/latest/{v}/hotspot?architecture={arch}&image_type=jre&os=linux&vendor=eclipse"
# The Adoptium API rejects Python's default User-Agent
HEADERS = {"User-Agent": "minecraft-server-gui-build/1.0"}


def _open(url: str):
    return urllib.request.urlopen(urllib.request.Request(url, headers=HEADERS), timeout=120)


def install(version: str) -> None:
    target = f"/opt/java/{version}"
    if os.path.exists(target):
        return
    with _open(API.format(v=version, arch=ARCH)) as resp:
        pkg = json.load(resp)[0]["binary"]["package"]
    with tempfile.NamedTemporaryFile(suffix=".tar.gz") as tmp:
        digest = hashlib.sha256()
        with _open(pkg["link"]) as resp:
            while chunk := resp.read(1 << 20):
                digest.update(chunk)
                tmp.write(chunk)
        if digest.hexdigest() != pkg["checksum"]:
            sys.exit(f"Checksum mismatch for Java {version}")
        tmp.flush()
        with tarfile.open(tmp.name) as tf:
            root = tf.getnames()[0].split("/")[0]
            tf.extractall("/opt/java", filter="tar")
    os.rename(f"/opt/java/{root}", target)
    print(f"Installed Java {version}: {pkg['name']}")


for v in sys.argv[1:]:
    install(v)
