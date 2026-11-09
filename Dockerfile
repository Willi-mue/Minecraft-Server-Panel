# Minecraft Server Panel
#
# Java 25 comes with the base image (Minecraft 26.x). Further runtimes are only added if
# listed in EXTRA_JAVA, to keep the image small:
#   21 -> Minecraft 1.20.5 - 1.21.x (default) | 17 -> 1.17 - 1.20.4 | 8 -> up to 1.16

# ---- Live map renderer (Rust). Only the compiled program goes into the final image. ----
FROM rust:1.99-slim-bookworm AS mapgen
WORKDIR /build
# Dependencies first, so they are cached when only the source changes
COPY mapgen/Cargo.toml mapgen/Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release --locked && rm -rf src
COPY mapgen/src ./src
RUN touch src/main.rs && cargo build --release --locked && cargo test --release --locked

# ---- Panel ----
FROM eclipse-temurin:25-jre-noble

ARG EXTRA_JAVA="21"

COPY requirements.txt docker/install-java.py /tmp/build/

RUN set -eu \
 && apt-get update \
 && apt-get install -y --no-install-recommends python3 python3-venv \
 && rm -rf /var/lib/apt/lists/* \
 && ln -s /opt/java/openjdk /opt/java/25 \
 && python3 /tmp/build/install-java.py $EXTRA_JAVA \
 && python3 -m venv /venv \
 && /venv/bin/pip install --no-cache-dir -r /tmp/build/requirements.txt \
 && find /venv -name "__pycache__" -prune -exec rm -rf {} + \
 && rm -rf /tmp/build \
 && usermod -l minecraft -d /home/minecraft -m ubuntu \
 && groupmod -n minecraft ubuntu \
 && mkdir -p /data && chown minecraft:minecraft /data
# (The Ubuntu base already has uid 1000 "ubuntu", renamed to "minecraft". The entrypoint
#  starts as root only to fix mount ownership, then drops to uid 1000.)

COPY --from=mapgen /build/target/release/mapgen /usr/local/bin/mapgen

WORKDIR /app
COPY app ./app
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN sed -i 's/\r$//' /usr/local/bin/docker-entrypoint.sh && chmod +x /usr/local/bin/docker-entrypoint.sh

ENV HOME=/home/minecraft \
    DATA_DIR=/data \
    JAVA_DIR=/opt/java \
    WEB_HOST=0.0.0.0 \
    WEB_PORT=47318 \
    MAP_PORT=47319 \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

VOLUME /data
EXPOSE 47318 47319 25565

# Panel alive? (checks the web app only; a stopped Minecraft server is a normal state)
HEALTHCHECK --interval=60s --timeout=5s --start-period=20s --retries=3 \
  CMD ["/venv/bin/python", "-c", "import os,urllib.request; urllib.request.urlopen(f'http://127.0.0.1:{os.environ[\"WEB_PORT\"]}/api/health', timeout=4)"]

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["/venv/bin/python", "-m", "app"]
