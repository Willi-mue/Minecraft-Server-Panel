#!/bin/sh
# Fix ownership of the mounted folders (Docker creates missing bind-mount dirs as root),
# then drop privileges to the unprivileged "minecraft" user (uid 1000).
set -e

if [ "$(id -u)" = "0" ]; then
  for dir in /data "${BACKUP_DIR:-/data/backups}"; do
    mkdir -p "$dir"
    if [ "$(stat -c %u "$dir")" != "1000" ]; then
      # Some bind mounts (e.g. Docker Desktop on Windows) don't support chown; that's fine there.
      chown -R 1000:1000 "$dir" 2>/dev/null || echo "Note: could not chown $dir, continuing" >&2
    fi
  done
  exec setpriv --reuid=1000 --regid=1000 --init-groups "$@"
fi

exec "$@"
