# TODO – Dashboard (Panel)

Code-Aufgaben für das Panel. Entscheidungen/Infos vom Entwickler: [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md).
Die Live-Map hat eigene Listen: [TODO_LIVEMAP.md](TODO_LIVEMAP.md), [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).

**Zuständigkeit** (damit zwei Claudes parallel arbeiten können): `app/` außer `app/livemap.py`,
`app/mapserver.py` und `app/static/map/`; außerdem `docker/` und die Tests dieser Module.
`mapgen/` nicht anfassen. Gemeinsam genutzte Dateien (`app/main.py`, `app/server.py`,
`app/config.py`, `Dockerfile`, `docker-compose*.yml`, `README.md`, `CLAUDE.md`) nur gezielt und klein
ändern und direkt vorher neu lesen – dort arbeitet evtl. gleichzeitig die Live-Map-Seite.

**Ziel-Design:** Variante A aus dem Mockup https://claude.ai/artifact/JfZwYapBDq9HBfbEJUZvZk
(Seitenleiste mit Server-Umschalter, Seiten Übersicht / Spieler / Add-ons / Welt & Backups / Server,
Skin-Köpfe). Mehrere Server, das neue Frontend sowie Phase 3 und 4 (Fabric, Mods/Plugins, Bannliste,
Spielerprofile, Verlauf/TPS, Gamerules, Welt-Upload, Höhlen-Einstellungen) sind fertig (siehe CLAUDE.md).

## Offen

- [ ] Eigene Datapacks: Quellordner im Repo (optional)
  - Entscheidung 2026-11-10: Kein Editor im Panel. Datapacks schreibt der Entwickler zusammen mit Claude; im Panel sollen sie wie die anderen Packs installier- und umschaltbar sein. Das geht schon: ein Ordner-Pack (mit `pack.mcmeta`) in `DATA_DIR/datapacks/<name>/` erscheint in der Bibliothek, „Install“/Ein/Aus wie gewohnt, nach Änderungen zeigt das Panel „outdated“ → Update (+ `/reload` beim Umschalten).
  - Offen: `data/` ist in `.gitignore`, eigene Packs wären also nicht versioniert. Vorschlag: Quellen in `datapacks/` im Repo, das Panel liest diesen Ordner als zweite Bibliotheksquelle (z. B. `DATAPACK_SRC`, im Docker-Compose read-only gemountet) oder ein kleines Sync-Skript kopiert sie nach `DATA_DIR/datapacks/`. Erst umsetzen, wenn das erste eigene Pack entsteht.
  - Touches: `app/datapacks.py`, `app/config.py`, `docker-compose*.yml`, README
