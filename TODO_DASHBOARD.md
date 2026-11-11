# TODO – Dashboard (Panel)

Code-Aufgaben für das Panel. Entscheidungen/Infos vom Entwickler: [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md).
Die Live-Map hat eigene Listen: [TODO_LIVEMAP.md](TODO_LIVEMAP.md), [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).

**Zuständigkeit** (damit zwei Claudes parallel arbeiten können): `panel/` außer `panel/src/livemap.rs`,
`mapserver.rs`, `leaderboard.rs`, `waypoints.rs`; `app/static/` außer `app/static/map/`; außerdem
`docker/`. `mapgen/` nicht anfassen. Gemeinsam genutzte Dateien (`panel/src/main.rs`, `api.rs`,
`server.rs`, `config.rs`, `tasks.rs`, `Dockerfile`, `docker-compose*.yml`, `README.md`, `CLAUDE.md`) nur
gezielt und klein ändern und direkt vorher neu lesen – dort arbeitet evtl. gleichzeitig die Live-Map-Seite.

## Offen

- [ ] Eigene Datapacks: Quellordner im Repo (optional)
  - Entscheidung 2026-11-10: Kein Editor im Panel. Datapacks schreibt der Entwickler zusammen mit Claude; im Panel sollen sie wie die anderen Packs installier- und umschaltbar sein. Das geht schon: ein Ordner-Pack (mit `pack.mcmeta`) in `DATA_DIR/datapacks/<name>/` erscheint in der Bibliothek, „Install“/Ein/Aus wie gewohnt, nach Änderungen zeigt das Panel „outdated“ → Update (+ `/reload` beim Umschalten).
  - Offen: `data/` ist in `.gitignore`, eigene Packs wären also nicht versioniert. Vorschlag: Quellen in `datapacks/` im Repo, das Panel liest diesen Ordner als zweite Bibliotheksquelle (z. B. `DATAPACK_SRC`, im Docker-Compose read-only gemountet) oder ein kleines Sync-Skript kopiert sie nach `DATA_DIR/datapacks/`. Erst umsetzen, wenn das erste eigene Pack entsteht.
  - Touches: `panel/src/datapacks.rs`, `panel/src/config.rs`, `docker-compose*.yml`, README
