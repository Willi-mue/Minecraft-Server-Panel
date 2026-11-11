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

# ---- Panel (Rust). Tests need the frontend (logo check), so app/static comes along. ----
FROM rust:1.99-slim-bookworm AS panel
WORKDIR /build/panel
COPY panel/Cargo.toml panel/Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release --locked && rm -rf src
COPY panel/src ./src
COPY app/static /build/app/static
RUN touch src/main.rs && cargo build --release --locked && cargo test --release --locked

# ---- Extra Java runtimes (installer script needs Python; only the runtimes are kept) ----
FROM eclipse-temurin:25-jre-noble AS java
ARG EXTRA_JAVA="21"
COPY docker/install-java.py /tmp/install-java.py
RUN set -eu \
 && apt-get update \
 && apt-get install -y --no-install-recommends python3 \
 && python3 /tmp/install-java.py $EXTRA_JAVA \
 && mkdir /extra \
 && for v in $EXTRA_JAVA; do mv "/opt/java/$v" /extra/; done

# ---- Final image ----
FROM eclipse-temurin:25-jre-noble

RUN set -eu \
 && ln -s /opt/java/openjdk /opt/java/25 \
 && usermod -l minecraft -d /home/minecraft -m ubuntu \
 && groupmod -n minecraft ubuntu \
 && mkdir -p /data && chown minecraft:minecraft /data
# (The Ubuntu base already has uid 1000 "ubuntu", renamed to "minecraft". The entrypoint
#  starts as root only to fix mount ownership, then drops to uid 1000.)

COPY --from=java /extra/ /opt/java/
COPY --from=mapgen /build/target/release/mapgen /usr/local/bin/mapgen
COPY --from=panel /build/panel/target/release/panel /usr/local/bin/panel

WORKDIR /app
COPY app/static ./app/static
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN sed -i 's/\r$//' /usr/local/bin/docker-entrypoint.sh && chmod +x /usr/local/bin/docker-entrypoint.sh

ENV HOME=/home/minecraft \
    DATA_DIR=/data \
    JAVA_DIR=/opt/java \
    STATIC_DIR=/app/app/static \
    WEB_HOST=0.0.0.0 \
    WEB_PORT=47318 \
    MAP_PORT=47319

VOLUME /data
EXPOSE 47318 47319 25565-25569

# Panel alive? (checks the web app only; a stopped Minecraft server is a normal state)
HEALTHCHECK --interval=60s --timeout=5s --start-period=20s --retries=3 \
  CMD ["panel", "health"]

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["panel"]
